//! End-to-End Cluster Integration Test
//!
//! Tests the full multi-node cluster topology with:
//! 1. Central Director (NodeRegistry, GlobalOctreeCoordinator)
//! 2. Edge Gateway (SessionRouter, PacketDispatcher)
//! 3. Two Simulation Nodes (Node 1 for [-100, 0], Node 2 for [0, 100])
//!
//! Simulates:
//! - Initial player session routing to Node 1
//! - In-flight client movement towards boundary
//! - Boundary ghost replication from Node 1 to Node 2
//! - Boundary transition and dynamic gateway route handover from Node 1 to Node 2
//! - Post-handover authoritative movement and interaction on Node 2

use bytes::BytesMut;
use std::sync::Arc;

use mmo_core::guid::{EntityType, Guid};
use mmo_core::BoundingBox3D;
use mmo_director::NodeRegistry;
use mmo_gateway::router::SessionRouter;
use mmo_gateway::server::PacketDispatcher;
use mmo_node::network::{EntityVelocity, GhostBroadcaster, GhostStore};
use mmo_node::sim::TickSimulation;
use mmo_schema::client::{ClientPacket, InteractionPacket, InteractionType, MovementPacket, Vec3};
use mmo_schema::cluster::GhostSyncRequest;

#[tokio::test]
async fn test_end_to_end_cluster_player_movement_and_boundary_handover() {
    // ------------------------------------------------------------------------
    // Step 1: Initialize Director Control Plane
    // ------------------------------------------------------------------------
    let mut registry = NodeRegistry::new();

    let node1_bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (0.0, 100.0, 100.0));
    let node2_bounds = BoundingBox3D::new((0.0, -100.0, -100.0), (100.0, 100.0, 100.0));

    let node1_addr = "127.0.0.1:50052".to_string();
    let node2_addr = "127.0.0.1:50053".to_string();

    let node1_id = registry.register_node(node1_addr.clone(), vec![node1_bounds]);
    let node2_id = registry.register_node(node2_addr.clone(), vec![node2_bounds]);

    assert_eq!(node1_id, 1);
    assert_eq!(node2_id, 2);
    assert_eq!(registry.len(), 2);

    // ------------------------------------------------------------------------
    // Step 2: Initialize 2 Simulation Nodes
    // ------------------------------------------------------------------------
    let mut node1_sim = TickSimulation::new(1, node1_bounds);
    let mut node2_sim = TickSimulation::new(2, node2_bounds);

    let mut node2_ghost_store = GhostStore::new();

    // ------------------------------------------------------------------------
    // Step 3: Initialize Edge Gateway
    // ------------------------------------------------------------------------
    let router = Arc::new(SessionRouter::new());
    let dispatcher = PacketDispatcher::new(router.clone());

    let session_id = 9001u64;
    let player_guid = Guid::new(EntityType::Player, 1, 42);

    // Register initial player session routed to Node 1
    router.register_session(session_id, player_guid, node1_addr.clone());

    // Spawn player authoritatively on Node 1 at (-3.0, 0.0, 0.0)
    let spawned = node1_sim.spawn_entity(player_guid, (-3.0, 0.0, 0.0));
    assert!(spawned, "Player should spawn inside Node 1 bounds");
    assert_eq!(node1_sim.entity_store.count(), 1);

    // ------------------------------------------------------------------------
    // Step 4: Client sends Movement Packet to Gateway (Target: -2.6, 0.0, 0.0)
    // ------------------------------------------------------------------------
    let move_pkt1 = MovementPacket {
        entity_id: player_guid.raw(),
        position: Vec3::new(-2.6, 0.0, 0.0),
        velocity: Vec3::new(8.0, 0.0, 0.0),
        yaw: 0.0,
        pitch: 0.0,
        timestamp: 1000,
        sequence: 1,
    };
    let mut frame_buf = BytesMut::new();
    frame_buf.extend_from_slice(&ClientPacket::Movement(move_pkt1.clone()).encode());

    let routed_packets = dispatcher
        .process_frames(session_id, &mut frame_buf)
        .unwrap();
    assert_eq!(routed_packets.len(), 1);
    assert_eq!(routed_packets[0].node_address, node1_addr);
    assert_eq!(routed_packets[0].session_id, session_id);
    assert_eq!(routed_packets[0].guid, Some(player_guid));

    // Node 1 validates movement
    let result = node1_sim.process_client_move(player_guid, (-2.6, 0.0, 0.0), 0.05);
    assert_eq!(result, Ok((-2.6, 0.0, 0.0)));

    // ------------------------------------------------------------------------
    // Step 5: Player approaches boundary face (x = 0.0) at (-2.2, 0.0, 0.0)
    // ------------------------------------------------------------------------
    let move_pkt2 = MovementPacket {
        entity_id: player_guid.raw(),
        position: Vec3::new(-2.2, 0.0, 0.0),
        velocity: Vec3::new(8.0, 0.0, 0.0),
        yaw: 0.0,
        pitch: 0.0,
        timestamp: 1050,
        sequence: 2,
    };
    frame_buf.clear();
    frame_buf.extend_from_slice(&ClientPacket::Movement(move_pkt2.clone()).encode());

    let routed_packets = dispatcher
        .process_frames(session_id, &mut frame_buf)
        .unwrap();
    assert_eq!(routed_packets[0].node_address, node1_addr);

    // Node 1 processes move
    let result = node1_sim.process_client_move(player_guid, (-2.2, 0.0, 0.0), 0.05);
    assert_eq!(result, Ok((-2.2, 0.0, 0.0)));

    // Attach velocity component
    node1_sim
        .entity_store
        .get_mut_by_guid(player_guid)
        .unwrap()
        .components
        .add_component(EntityVelocity {
            x: 8.0,
            y: 0.0,
            z: 0.0,
        });

    // Node 1 GhostBroadcaster detects player is near boundary face (margin = 5.0m)
    let broadcaster = GhostBroadcaster::new(1, 5.0);

    let boundary_ghosts = broadcaster.find_boundary_entities(
        &node1_sim.octree,
        &node1_sim.entity_store,
        node1_sim.tick_counter,
    );
    assert_eq!(
        boundary_ghosts.len(),
        1,
        "Player at -2.2m must be identified as boundary ghost"
    );
    assert_eq!(boundary_ghosts[0].guid, player_guid.raw());

    let sync_req: GhostSyncRequest =
        broadcaster.build_sync_request(node1_sim.tick_counter, boundary_ghosts);

    // Node 2 receives GhostSyncRequest and updates its GhostStore
    let count = node2_ghost_store.update_from_request(&sync_req);
    assert_eq!(count, 1);
    let ghost = node2_ghost_store
        .get(player_guid)
        .expect("Ghost entity must exist in Node 2 GhostStore");
    assert_eq!(ghost.position, (-2.2, 0.0, 0.0));
    assert_eq!(ghost.velocity, (8.0, 0.0, 0.0));

    // ------------------------------------------------------------------------
    // Step 6: Player crosses boundary into Node 2 domain at (5.0, 0.0, 0.0)
    // ------------------------------------------------------------------------
    let target_pos = (5.0f32, 0.0f32, 0.0f32);

    // Verify boundary membership
    assert!(!node1_bounds.contains(target_pos));
    assert!(node2_bounds.contains(target_pos));

    // Dynamic handoff: Edge Gateway updates routing table
    let updated = router.update_route(player_guid, node2_addr.clone());
    assert!(updated, "Existing route should be updated");
    assert_eq!(
        router.get_route_by_guid(player_guid).as_deref(),
        Some(node2_addr.as_str())
    );
    assert_eq!(
        router.get_route_by_session(session_id).as_deref(),
        Some(node2_addr.as_str())
    );

    // Authoritative entity handoff:
    // Node 1 despawns player from its simulation
    let despawned = node1_sim.despawn_entity(player_guid);
    assert!(despawned, "Entity must be despawned from Node 1");
    assert_eq!(node1_sim.entity_store.count(), 0);

    // Node 2 removes ghost replica and promotes to authoritative entity
    node2_ghost_store.remove(player_guid);
    assert_eq!(node2_ghost_store.len(), 0);

    let spawned = node2_sim.spawn_entity(player_guid, target_pos);
    assert!(spawned, "Entity must spawn authoritatively on Node 2");
    assert_eq!(node2_sim.entity_store.count(), 1);

    // ------------------------------------------------------------------------
    // Step 7: Subsequent Movement & Interaction Packets Route to Node 2
    // ------------------------------------------------------------------------
    // Movement packet to (5.4, 0.0, 0.0)
    let move_pkt3 = MovementPacket {
        entity_id: player_guid.raw(),
        position: Vec3::new(5.4, 0.0, 0.0),
        velocity: Vec3::new(8.0, 0.0, 0.0),
        yaw: 0.0,
        pitch: 0.0,
        timestamp: 1100,
        sequence: 3,
    };
    frame_buf.clear();
    frame_buf.extend_from_slice(&ClientPacket::Movement(move_pkt3.clone()).encode());

    let routed_packets = dispatcher
        .process_frames(session_id, &mut frame_buf)
        .unwrap();
    assert_eq!(routed_packets.len(), 1);
    assert_eq!(
        routed_packets[0].node_address, node2_addr,
        "Packet must now be routed to Node 2"
    );

    // Node 2 processes movement authoritatively
    let result = node2_sim.process_client_move(player_guid, (5.4, 0.0, 0.0), 0.05);
    assert_eq!(result, Ok((5.4, 0.0, 0.0)));
    let ent = node2_sim.entity_store.get_by_guid(player_guid).unwrap();
    assert_eq!(ent.position, (5.4, 0.0, 0.0));

    // Client interaction packet (Talk)
    let interact_pkt = InteractionPacket {
        source_guid: player_guid.raw(),
        target_guid: 0,
        interaction_type: InteractionType::Talk,
        action_id: 1,
        payload: bytes::Bytes::from_static(&[1, 2, 3]),
    };
    frame_buf.clear();
    frame_buf.extend_from_slice(&ClientPacket::Interaction(interact_pkt).encode());

    let routed_packets = dispatcher
        .process_frames(session_id, &mut frame_buf)
        .unwrap();
    assert_eq!(routed_packets.len(), 1);
    assert_eq!(routed_packets[0].node_address, node2_addr);
    assert_eq!(routed_packets[0].guid, Some(player_guid));

    // Tick Node 2 simulation to confirm 20 Hz tick loop executes with new player
    node2_sim.step_tick();
    assert_eq!(node2_sim.tick_counter, 1);
}
