//! End-to-end integration test for AoI broadcast draining, gateway delivery, and backhaul packet forwarding.
//!
//! Verifies:
//! 1. `GatewayService::DeliverBroadcastJobs` dispatches packets to client session sinks and reports disconnected sessions.
//! 2. `NodeService::ForwardClientPacket` receives client movement and updates node simulation state.
//! 3. Invalid movements generate authoritative `MovementCorrection` snapback packets.
//! 4. Tick loop drains `pending_broadcast_jobs` so memory does not leak.

use std::sync::Arc;

use dashmap::DashMap;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::sync::RwLock;
use tonic::transport::Server;

use mmo_core::guid::{EntityType, Guid};
use mmo_core::BoundingBox3D;
use mmo_gateway::server::{GatewayServiceImpl, SessionSink};
use mmo_gateway::SessionRouter;
use mmo_node::network::NodeServiceImpl;
use mmo_node::sim::TickSimulation;
use mmo_schema::client::{
    ClientPacket, InteractionResultPacket, MovementCorrectionReason, MovementPacket, Vec3,
};
use mmo_schema::cluster::gateway_service_client::GatewayServiceClient;
use mmo_schema::cluster::gateway_service_server::GatewayServiceServer;
use mmo_schema::cluster::node_service_client::NodeServiceClient;
use mmo_schema::cluster::node_service_server::NodeServiceServer;
use mmo_schema::cluster::{
    DeliverBroadcastJobsRequest, ForwardClientPacketRequest, SessionPacketBatch,
};

#[tokio::test]
async fn test_e2e_aoi_broadcast_jobs_delivery_and_disconnected_feedback() {
    // 1. Setup GatewayService on ephemeral port with registered SessionSink
    let router = Arc::new(SessionRouter::new());
    let sinks = Arc::new(DashMap::new());
    let (dtx, mut drx) = mpsc::channel(100);
    let (stx, mut srx) = mpsc::channel(100);
    let active_session_id = 42;
    sinks.insert(active_session_id, SessionSink::new(dtx, stx));

    let gw_service = GatewayServiceImpl::with_sinks(router, Arc::clone(&sinks));
    let gw_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_addr = gw_listener.local_addr().unwrap();

    tokio::spawn(async move {
        Server::builder()
            .add_service(GatewayServiceServer::new(gw_service))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(gw_listener))
            .await
            .unwrap();
    });

    let mut gw_client = GatewayServiceClient::connect(format!("http://{gw_addr}"))
        .await
        .expect("connect to gateway gRPC");

    // 2. Prepare test packets (1 movement ephemeral packet, 1 interaction result reliable packet)
    let move_pkt = ClientPacket::Movement(MovementPacket {
        entity_id: 101,
        sequence: 1,
        position: Vec3::new(10.0, 0.0, 10.0),
        velocity: Vec3::new(1.0, 0.0, 0.0),
        yaw: 0.0,
        pitch: 0.0,
        timestamp: 1000,
    });
    let interact_result_pkt = ClientPacket::InteractionResult(InteractionResultPacket {
        action_id: 1,
        success: true,
        error_code: 0,
        payload: vec![1, 2, 3],
    });

    let active_batch = SessionPacketBatch {
        session_id: active_session_id,
        packets: vec![move_pkt.encode(), interact_result_pkt.encode()],
    };
    let dead_session_id = 9999;
    let dead_batch = SessionPacketBatch {
        session_id: dead_session_id,
        packets: vec![move_pkt.encode()],
    };

    let req = DeliverBroadcastJobsRequest {
        jobs: vec![active_batch, dead_batch],
    };

    // 3. Deliver broadcast jobs to gateway
    let resp = gw_client
        .deliver_broadcast_jobs(req)
        .await
        .expect("deliver broadcast jobs")
        .into_inner();

    assert!(resp.success);
    assert_eq!(resp.delivered_count, 2);
    assert_eq!(resp.disconnected_sessions, vec![dead_session_id]);

    // 4. Verify ephemeral packet arrived on datagram channel
    let datagram_bytes = drx.recv().await.expect("receive datagram");
    assert!(!datagram_bytes.is_empty());

    // 5. Verify reliable packet arrived on stream channel
    let stream_bytes = srx.recv().await.expect("receive stream");
    assert!(!stream_bytes.is_empty());
}

#[tokio::test]
async fn test_e2e_forward_client_packet_and_movement_correction_snapback() {
    // 1. Setup NodeService on ephemeral port
    let bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
    let mut sim = TickSimulation::new(1, bounds);
    let player_guid = Guid::new(EntityType::Player, 1, 555);
    let session_id = 77;
    assert!(sim.spawn_entity(player_guid, (0.0, 0.0, 0.0)));
    sim.register_aoi_observer(session_id, player_guid, Default::default());

    let sim_arc = Arc::new(RwLock::new(sim));
    let node_service = NodeServiceImpl::new(Arc::clone(&sim_arc));
    let node_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let node_addr = node_listener.local_addr().unwrap();

    tokio::spawn(async move {
        Server::builder()
            .add_service(NodeServiceServer::new(node_service))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(
                node_listener,
            ))
            .await
            .unwrap();
    });

    let mut node_client = NodeServiceClient::connect(format!("http://{node_addr}"))
        .await
        .expect("connect to node gRPC");

    // 2. Send valid movement packet (0.2m move within 50ms is 4 m/s <= 10 m/s max player speed)
    let valid_move = ClientPacket::Movement(MovementPacket {
        entity_id: player_guid.raw(),
        sequence: 1,
        position: Vec3::new(0.2, 0.0, 0.0),
        velocity: Vec3::new(1.0, 0.0, 0.0),
        yaw: 0.0,
        pitch: 0.0,
        timestamp: 100,
    });

    let req = ForwardClientPacketRequest {
        session_id,
        guid: player_guid.raw(),
        packet_data: valid_move.encode(),
        session_epoch: 0,
    };

    let resp = node_client
        .forward_client_packet(req)
        .await
        .expect("forward valid movement")
        .into_inner();
    assert!(resp.success);

    // Drain simulation inputs to apply queued movement
    sim_arc.write().await.drain_client_inputs();

    // Verify entity position updated in sim
    {
        let s = sim_arc.read().await;
        let ent = s.entity_store.get_by_guid(player_guid).unwrap();
        assert_eq!(ent.position, (0.2, 0.0, 0.0));
    }

    // 3. Send invalid movement (teleport / speed hack violation)
    let invalid_move = ClientPacket::Movement(MovementPacket {
        entity_id: player_guid.raw(),
        sequence: 2,
        position: Vec3::new(90.0, 0.0, 0.0), // 89m delta in 1 tick violates max speed
        velocity: Vec3::new(90.0, 0.0, 0.0),
        yaw: 0.0,
        pitch: 0.0,
        timestamp: 150,
    });

    let req_invalid = ForwardClientPacketRequest {
        session_id,
        guid: player_guid.raw(),
        packet_data: invalid_move.encode(),
        session_epoch: 0,
    };

    let resp_invalid = node_client
        .forward_client_packet(req_invalid)
        .await
        .expect("forward invalid movement")
        .into_inner();

    assert!(resp_invalid.success);

    // Drain simulation inputs to detect kinematic violation and emit snapback correction
    sim_arc.write().await.drain_client_inputs();

    // 4. Verify snapback MovementCorrection was queued in pending_broadcast_jobs
    {
        let mut s = sim_arc.write().await;
        let jobs = s.drain_broadcast_jobs();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].session_id, session_id);
        assert_eq!(jobs[0].packets.len(), 1);
        match &jobs[0].packets[0] {
            ClientPacket::MovementCorrection(corr) => {
                assert_eq!(corr.entity_id, player_guid.raw());
                assert_eq!(corr.sequence, 2);
                assert_eq!(corr.corrected_position, Vec3::new(0.2, 0.0, 0.0));
                assert_eq!(
                    corr.reason_enum(),
                    MovementCorrectionReason::KinematicViolation
                );
            }
            other => panic!("Expected MovementCorrection packet, got {other:?}"),
        }

        // Verify pending jobs are now drained and memory is freed
        assert!(s.drain_broadcast_jobs().is_empty());
    }
}

#[tokio::test]
async fn test_tick_loop_drains_broadcast_jobs_preventing_leak() {
    let bounds = BoundingBox3D::new((-50.0, -50.0, -50.0), (50.0, 50.0, 50.0));
    let mut sim = TickSimulation::new(1, bounds);
    let p1 = Guid::new(EntityType::Player, 1, 1);
    let p2 = Guid::new(EntityType::Player, 1, 2);

    sim.spawn_entity(p1, (0.0, 0.0, 0.0));
    sim.register_aoi_observer(1, p1, Default::default());

    sim.spawn_entity(p2, (5.0, 0.0, 0.0));
    sim.register_aoi_observer(2, p2, Default::default());

    // Run 5 ticks with simulated draining
    for _ in 0..5 {
        sim.step_tick();
        // Emulate what main.rs does:
        let drained = sim.drain_broadcast_jobs();
        // On ticks where entities are visible, broadcast jobs are generated and drained
        assert!(
            sim.pending_broadcast_jobs.is_empty(),
            "pending_broadcast_jobs must be empty after drain"
        );
        let _ = drained;
    }

    assert_eq!(sim.pending_broadcast_jobs.len(), 0);
}
