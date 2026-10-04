//! End-to-end multi-node live entity migration integration test.
//!
//! Spawns an Edge Gateway, a donor simulation node, and a recipient simulation node.
//! Verifies seamless entity transfer across spatial boundaries, Edge Gateway route update,
//! and subsequent client packet routing without transport interruption or entity duplication.

use std::sync::Arc;
use std::time::Duration;

use mmo_core::guid::{EntityType, Guid};
use mmo_core::BoundingBox3D;
use mmo_gateway::{GatewayServer, GatewayServiceImpl, SessionRouter};
use mmo_node::network::{execute_entity_migration, NodeServiceImpl};
use mmo_node::TickSimulation;
use mmo_schema::client::{ClientPacket, MovementPacket};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

#[tokio::test]
async fn test_end_to_end_multi_node_live_migration_and_gateway_invalidation() {
    // -------------------------------------------------------------------------
    // 1. Start Edge Gateway with gRPC GatewayService
    // -------------------------------------------------------------------------
    let router = Arc::new(SessionRouter::new());
    let gateway_server = Arc::new(GatewayServer::new(Arc::clone(&router)));
    let gateway_grpc = GatewayServiceImpl::new(Arc::clone(&router));

    let gw_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_grpc_addr = gw_listener.local_addr().unwrap();

    tokio::spawn(async move {
        Server::builder()
            .add_service(gateway_grpc.into_server())
            .serve_with_incoming(TcpListenerStream::new(gw_listener))
            .await
            .unwrap();
    });

    // -------------------------------------------------------------------------
    // 2. Start Recipient Node (Node 2: [50.0..100.0, 0.0..100.0, 0.0..100.0])
    // -------------------------------------------------------------------------
    let recipient_bounds = BoundingBox3D::new((50.0, 0.0, 0.0), (100.0, 100.0, 100.0));
    let recipient_sim = Arc::new(tokio::sync::RwLock::new(TickSimulation::new(
        2,
        recipient_bounds,
    )));
    let recipient_service = NodeServiceImpl::new(Arc::clone(&recipient_sim));

    let recipient_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let recipient_addr = recipient_listener.local_addr().unwrap();

    tokio::spawn(async move {
        Server::builder()
            .add_service(recipient_service.into_server())
            .serve_with_incoming(TcpListenerStream::new(recipient_listener))
            .await
            .unwrap();
    });

    // -------------------------------------------------------------------------
    // 3. Start Donor Node (Node 1: [0.0..50.0, 0.0..100.0, 0.0..100.0])
    // -------------------------------------------------------------------------
    let donor_bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (50.0, 100.0, 100.0));
    let donor_sim = Arc::new(tokio::sync::RwLock::new(TickSimulation::new(
        1,
        donor_bounds,
    )));
    let donor_service = NodeServiceImpl::new(Arc::clone(&donor_sim));

    let donor_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let donor_addr = donor_listener.local_addr().unwrap();

    tokio::spawn(async move {
        Server::builder()
            .add_service(donor_service.into_server())
            .serve_with_incoming(TcpListenerStream::new(donor_listener))
            .await
            .unwrap();
    });

    // -------------------------------------------------------------------------
    // 4. Register client session on Gateway routed to Node 1
    // -------------------------------------------------------------------------
    let session_id = 1001;
    let player_guid = Guid::new(EntityType::Player, 1, 42);
    let initial_pos = (48.0, 10.0, 10.0);

    router.register_session(session_id, player_guid, donor_addr.to_string());
    assert_eq!(
        router.get_route_by_guid(player_guid).as_deref(),
        Some(donor_addr.to_string().as_str())
    );

    // Spawn player entity on Donor Node
    {
        let mut d = donor_sim.write().await;
        assert!(d.spawn_entity(player_guid, initial_pos));
        d.register_aoi_observer(session_id, player_guid, Default::default());
    }

    // -------------------------------------------------------------------------
    // 5. Ingress packet before migration routes to Node 1
    // -------------------------------------------------------------------------
    let move_pkt = ClientPacket::Movement(MovementPacket {
        entity_id: player_guid.raw(),
        position: mmo_schema::client::Vec3::new(49.0, 10.0, 10.0),
        velocity: mmo_schema::client::Vec3::ZERO,
        yaw: 0.0,
        pitch: 0.0,
        timestamp: 100,
        sequence: 1,
    });
    let routed_before = gateway_server
        .dispatcher()
        .dispatch_packet(session_id, move_pkt)
        .expect("Dispatch before migration");
    assert_eq!(routed_before.node_address, donor_addr.to_string());

    // -------------------------------------------------------------------------
    // 6. Player crosses boundary into Node 2 bounds (55.0, 10.0, 10.0)
    // -------------------------------------------------------------------------
    let target_pos = (55.0, 10.0, 10.0);
    {
        let mut d = donor_sim.write().await;
        // Verify boundary migration trigger condition:
        // Position is outside Node 1 bounds [0..50] by 5.0 units >= margin (1.0)
        assert!(d.check_boundary_migration(player_guid, target_pos, 1.0, 20));
        // Staging target position
        d.entity_store
            .get_mut_by_guid(player_guid)
            .unwrap()
            .position = target_pos;
    }

    // Execute Two-Phase Live Entity Migration with Gateway Route Commit
    let migration_result = execute_entity_migration(
        Arc::clone(&donor_sim),
        player_guid,
        2,
        &recipient_addr.to_string(),
        Some(&gw_grpc_addr.to_string()),
    )
    .await;

    assert_eq!(migration_result, Ok(true));

    // Allow async route invalidation propagation
    tokio::time::sleep(Duration::from_millis(50)).await;

    // -------------------------------------------------------------------------
    // 7. Verification: Zero entity duplication and zero loss
    // -------------------------------------------------------------------------
    // Donor Node 1: entity despawned, lease released, kinematic state cleaned up
    {
        let d = donor_sim.read().await;
        assert!(!d.entity_store.contains_guid(player_guid));
        assert!(!d.is_migrating(player_guid));
        assert!(!d.kinematic_states.contains_key(&player_guid));
        assert!(d.octree.query_radius(target_pos, 0.1).is_empty());
    }

    // Recipient Node 2: entity adopted, in entity store and octree at target_pos
    {
        let r = recipient_sim.read().await;
        assert!(r.entity_store.contains_guid(player_guid));
        assert_eq!(
            r.entity_store.get_by_guid(player_guid).unwrap().position,
            target_pos
        );
        let found = r.octree.query_radius(target_pos, 0.1);
        assert_eq!(found, vec![player_guid]);
    }

    // -------------------------------------------------------------------------
    // 8. Verification: Gateway routes subsequent packets seamlessly to Node 2
    // -------------------------------------------------------------------------
    assert_eq!(
        router.get_route_by_guid(player_guid).as_deref(),
        Some(recipient_addr.to_string().as_str())
    );
    assert_eq!(
        router.get_route_by_session(session_id).as_deref(),
        Some(recipient_addr.to_string().as_str())
    );

    let subsequent_move = ClientPacket::Movement(MovementPacket {
        entity_id: player_guid.raw(),
        position: mmo_schema::client::Vec3::new(56.0, 10.0, 10.0),
        velocity: mmo_schema::client::Vec3::ZERO,
        yaw: 0.0,
        pitch: 0.0,
        timestamp: 150,
        sequence: 2,
    });

    let routed_after = gateway_server
        .dispatcher()
        .dispatch_packet(session_id, subsequent_move)
        .expect("Dispatch after migration");

    assert_eq!(routed_after.node_address, recipient_addr.to_string());
}
