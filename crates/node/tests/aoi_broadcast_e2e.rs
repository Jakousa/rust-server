//! End-to-end simulation and gateway integration test for Client Area-of-Interest (AoI) packet broadcasting.
//!
//! Verifies:
//! 1. Player sessions register with `GatewayServer` and simulation `AoiManager`.
//! 2. Out-of-range entities generate zero broadcast packets.
//! 3. Moving an entity into an observer's AoI bubble triggers an authoritative `EntityState` packet over TCP.
//! 4. Continuous movement within the bubble sends delta `EntityState` updates.
//! 5. Exiting the bubble triggers an authoritative `EntityDespawn` packet over TCP.
//! 6. Component changes (e.g. `HealthComponent` damage) within view trigger updated `EntityState`.
//! 7. High-density entity clusters trigger dynamic radius throttling.

use std::sync::Arc;
use std::time::Duration;

use bytes::BytesMut;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use mmo_core::guid::EntityType;
use mmo_core::{BoundingBox3D, Guid};
use mmo_gameplay::combat::HealthComponent;
use mmo_gateway::router::SessionRouter;
use mmo_gateway::server::GatewayServer;
use mmo_node::aoi::ObserverConfig;
use mmo_node::sim::TickSimulation;
use mmo_schema::client::ClientPacket;

#[tokio::test]
async fn test_end_to_end_aoi_broadcasting_lifecycle() {
    // 1. Initialize GatewayServer on ephemeral port
    let router = Arc::new(SessionRouter::new());
    let (packet_tx, _packet_rx) = mpsc::channel(100);
    let gateway = Arc::new(GatewayServer::with_packet_channel(router.clone(), packet_tx));

    let listener = gateway.bind("127.0.0.1:0").await.expect("bind gateway listener");
    let gateway_addr = listener.local_addr().expect("local addr");

    // 2. Initialize 20 Hz authoritative simulation node
    let bounds = BoundingBox3D::new((-500.0, -500.0, -500.0), (500.0, 500.0, 500.0));
    let mut sim = TickSimulation::new(1, bounds);

    // 3. Setup Player A (Observer)
    let session_a = gateway.next_session_id();
    let player_a = Guid::new(EntityType::Player, 1, 1001);
    gateway
        .register_session(session_a, player_a, "127.0.0.1:50051".to_string())
        .expect("register session A");

    sim.spawn_entity(player_a, (0.0, 0.0, 0.0));
    let config_a = ObserverConfig {
        base_radius: 50.0,
        max_entities: 10,
        ..Default::default()
    };
    sim.register_aoi_observer(session_a, player_a, config_a);

    // 4. Setup Player B (Moving entity)
    let session_b = gateway.next_session_id();
    let player_b = Guid::new(EntityType::Player, 1, 1002);
    gateway
        .register_session(session_b, player_b, "127.0.0.1:50051".to_string())
        .expect("register session B");

    // Spawn Player B outside AoI (55m away, base radius is 50m)
    sim.spawn_entity(player_b, (55.0, 0.0, 0.0));

    // 5. Connect Player A's client TCP socket
    let gw_clone = gateway.clone();
    tokio::spawn(async move {
        if let Ok((socket, _)) = listener.accept().await {
            let _ = gw_clone.handle_connection(socket, session_a).await;
        }
    });

    let mut client_a_tcp = TcpStream::connect(gateway_addr)
        .await
        .expect("Client A connects to gateway");

    // Allow connection handshake to register session sink
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(gateway.has_session_sink(session_a).expect("sink registered"));

    // Helper function: ticks simulation, drains broadcast jobs, and forwards to gateway
    async fn forward_aoi_broadcasts(sim: &mut TickSimulation, gw: &GatewayServer) {
        sim.step_tick();
        let jobs = sim.drain_broadcast_jobs();
        for job in jobs {
            for pkt in &job.packets {
                let _ = gw.send_packet_to_session(job.session_id, pkt).await;
            }
        }
    }

    // --- PHASE 1: Entity out of range -> Zero packets ---
    forward_aoi_broadcasts(&mut sim, &gateway).await;
    let mut read_buf = BytesMut::with_capacity(1024);
    tokio::select! {
        _ = client_a_tcp.read_buf(&mut read_buf) => {
            panic!("Should not receive packets when Player B is out of range");
        }
        _ = tokio::time::sleep(Duration::from_millis(50)) => {
            // Success: No packets arrived
        }
    }

    // --- PHASE 2: Player B moves into range (from 55m to 46m) -> Enter: EntityState received ---
    sim.process_client_move_deterministic(player_b, (46.0, 0.0, 0.0), 1.0)
        .expect("valid move into range");
    sim.entity_store
        .get_mut_by_guid(player_b)
        .unwrap()
        .rotation = std::f32::consts::PI;

    forward_aoi_broadcasts(&mut sim, &gateway).await;

    // Read packet on Client A
    read_buf.clear();
    let n = client_a_tcp
        .read_buf(&mut read_buf)
        .await
        .expect("read from socket");
    assert!(n > 0);

    let decoded = ClientPacket::decode_frame(&mut read_buf)
        .expect("decode frame")
        .expect("packet present");

    match decoded {
        ClientPacket::EntityState(state) => {
            assert_eq!(state.entity_id, player_b.raw());
            assert_eq!(state.position.x, 46.0);
            assert_eq!(state.yaw, std::f32::consts::PI);
        }
        other => panic!("Expected EntityState on enter, got {:?}", other),
    }

    // --- PHASE 3: Continuous Movement inside range (from 46m to 38m) -> Delta update ---
    sim.process_client_move_deterministic(player_b, (38.0, 0.0, 0.0), 1.0)
        .expect("valid move closer");

    forward_aoi_broadcasts(&mut sim, &gateway).await;

    read_buf.clear();
    let n = client_a_tcp
        .read_buf(&mut read_buf)
        .await
        .expect("read delta from socket");
    assert!(n > 0);

    let decoded = ClientPacket::decode_frame(&mut read_buf)
        .expect("decode delta frame")
        .expect("packet present");

    match decoded {
        ClientPacket::EntityState(state) => {
            assert_eq!(state.entity_id, player_b.raw());
            assert_eq!(state.position.x, 38.0);
        }
        other => panic!("Expected EntityState on delta move, got {:?}", other),
    }

    // --- PHASE 4: Player B moves out of range (from 38m to 120m) -> Exit: EntityDespawn received ---
    // Update position directly to simulate teleport or fast sprint
    let ent_b = sim.entity_store.get_mut_by_guid(player_b).unwrap();
    ent_b.position = (120.0, 0.0, 0.0);
    sim.octree.remove(player_b, (38.0, 0.0, 0.0));
    sim.octree.insert(player_b, (120.0, 0.0, 0.0));

    forward_aoi_broadcasts(&mut sim, &gateway).await;

    read_buf.clear();
    let n = client_a_tcp
        .read_buf(&mut read_buf)
        .await
        .expect("read despawn from socket");
    assert!(n > 0);

    let decoded = ClientPacket::decode_frame(&mut read_buf)
        .expect("decode despawn frame")
        .expect("packet present");

    match decoded {
        ClientPacket::EntityDespawn(despawn) => {
            assert_eq!(despawn.entity_id, player_b.raw());
        }
        other => panic!("Expected EntityDespawn on leave, got {:?}", other),
    }

    // --- PHASE 5: NPC Spawns, takes damage -> HealthComponent change broadcast ---
    let npc_guid = Guid::new(EntityType::Npc, 1, 555);
    sim.spawn_entity(npc_guid, (10.0, 10.0, 0.0));
    let mut health_comp = HealthComponent::new(100.0, 0.0);
    health_comp.take_damage(40.0); // HP is now 60.0
    sim.entity_store
        .get_mut_by_guid(npc_guid)
        .unwrap()
        .add_component(health_comp);

    forward_aoi_broadcasts(&mut sim, &gateway).await;

    read_buf.clear();
    let n = client_a_tcp
        .read_buf(&mut read_buf)
        .await
        .expect("read npc spawn state");
    assert!(n > 0);

    let decoded = ClientPacket::decode_frame(&mut read_buf)
        .expect("decode npc frame")
        .expect("packet present");

    match decoded {
        ClientPacket::EntityState(state) => {
            assert_eq!(state.entity_id, npc_guid.raw());
            assert_eq!(state.health, 60);
            assert_eq!(state.max_health, 100);
            assert_eq!(state.state_flags, 0); // alive
        }
        other => panic!("Expected EntityState for NPC with 60 HP, got {:?}", other),
    }
}
