//! End-to-end Geo-Distributed Gateway Handover & Takeover Integration Tests.
//!
//! Validates:
//! 1. Roaming session handover from Gateway A to Gateway B using signed HandoverTicket.
//! 2. Node authoritative epoch advancement and TakeoverSessionRoute execution.
//! 3. Clean local session revocation on Gateway A without entity despawn on the node.
//! 4. Upstream stale packet epoch filtering on the simulation node.
//! 5. Stale or replayed epoch takeover rejection.

use std::sync::Arc;
use tokio::net::TcpListener;
use tonic::transport::Server;

use crate::common::TestClusterBuilder;
use mmo_core::guid::{EntityType, Guid};
use mmo_core::BoundingBox3D;
use mmo_gateway::handover::{HandoverRateLimiter, HandoverTicket};
use mmo_gateway::server::{GatewayServiceImpl, SessionSink};
use mmo_gateway::SessionRouter;
use mmo_schema::cluster::gateway_service_client::GatewayServiceClient;
use mmo_schema::cluster::gateway_service_server::GatewayServiceServer;
use mmo_schema::cluster::node_service_client::NodeServiceClient;
use mmo_schema::cluster::{RevokeGatewaySessionRequest, TakeoverSessionRouteRequest};

#[tokio::test]
async fn test_end_to_end_geo_gateway_roaming_handover_and_epoch_guard() {
    let secret_key = b"cluster_global_production_secret_32bytes!!";
    let node_bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));

    // 1. Build cluster with Node 1
    let cluster = TestClusterBuilder::new()
        .with_spawn_grpc(true)
        .with_node(1, node_bounds)
        .build()
        .await;

    let node_sim = cluster.node_sim(1);
    let node_addr = format!("http://{}", cluster.node_addr(1));

    // 2. Setup Gateway A (US-East)
    let router_a = Arc::new(SessionRouter::new());
    let sinks_a = Arc::new(dashmap::DashMap::new());
    let (dtx_a, _drx_a) = tokio::sync::mpsc::channel(10);
    let (stx_a, _srx_a) = tokio::sync::mpsc::channel(10);
    sinks_a.insert(1001, SessionSink::new(dtx_a, stx_a));

    let gw_a_service = GatewayServiceImpl::with_sinks(Arc::clone(&router_a), Arc::clone(&sinks_a));
    let gw_a_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_a_addr = gw_a_listener.local_addr().unwrap();

    tokio::spawn(async move {
        Server::builder()
            .add_service(GatewayServiceServer::new(gw_a_service))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(
                gw_a_listener,
            ))
            .await
            .unwrap();
    });

    // 3. Setup Gateway B (EU-Central)
    let router_b = Arc::new(SessionRouter::new());
    let sinks_b = Arc::new(dashmap::DashMap::new());
    let (dtx_b, _drx_b) = tokio::sync::mpsc::channel(10);
    let (stx_b, _srx_b) = tokio::sync::mpsc::channel(10);
    sinks_b.insert(2002, SessionSink::new(dtx_b, stx_b));

    let gw_b_service = GatewayServiceImpl::with_sinks(Arc::clone(&router_b), Arc::clone(&sinks_b));
    let gw_b_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gw_b_addr = gw_b_listener.local_addr().unwrap();

    tokio::spawn(async move {
        Server::builder()
            .add_service(GatewayServiceServer::new(gw_b_service))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(
                gw_b_listener,
            ))
            .await
            .unwrap();
    });

    // 4. Initial state: Player connected through Gateway A to Node 1
    let player_guid = Guid::new(EntityType::Player, 1, 42);
    let initial_session_id = 1001;

    router_a.register_session(initial_session_id, player_guid, node_addr.clone());

    {
        let mut sim = node_sim.write().await;
        assert!(sim.spawn_entity(player_guid, (10.0, 10.0, 10.0)));
        sim.session_epochs.insert(player_guid, 1);
        sim.register_aoi_observer(initial_session_id, player_guid, Default::default());
    }

    assert_eq!(
        router_a.get_route_by_guid(player_guid).as_deref(),
        Some(node_addr.as_str())
    );

    // 5. Gateway A issues HandoverTicket to client
    let ticket = HandoverTicket::create(
        player_guid,
        initial_session_id,
        1,
        "gw-us-1",
        secret_key,
        10000,
    );
    assert!(ticket.verify(secret_key).is_ok());

    // 6. Client roams to Gateway B and presents ticket
    let rate_limiter = HandoverRateLimiter::new();
    assert!(rate_limiter.check_and_consume(player_guid));

    let new_session_id = 2002;
    let next_epoch = ticket.session_epoch + 1; // 2

    // Gateway B invokes TakeoverSessionRoute on Node 1
    let mut node_client = NodeServiceClient::connect(node_addr.clone()).await.unwrap();
    let takeover_resp = node_client
        .takeover_session_route(TakeoverSessionRouteRequest {
            guid: player_guid.raw(),
            new_session_id,
            new_gateway_id: "gw-eu-1".to_string(),
            new_gateway_grpc_addr: format!("http://{}", gw_b_addr),
            session_epoch: next_epoch,
            handover_signature: ticket.signature.clone(),
        })
        .await
        .unwrap()
        .into_inner();

    assert!(takeover_resp.success);
    assert_eq!(takeover_resp.authoritative_epoch, 2);

    // Gateway B establishes local routing
    router_b.register_session(new_session_id, player_guid, node_addr.clone());
    assert_eq!(
        router_b.get_route_by_guid(player_guid).as_deref(),
        Some(node_addr.as_str())
    );

    // Gateway B tells Gateway A to revoke old session
    let mut gw_a_client = GatewayServiceClient::connect(format!("http://{}", gw_a_addr))
        .await
        .unwrap();

    let revoke_resp = gw_a_client
        .revoke_gateway_session(RevokeGatewaySessionRequest {
            guid: player_guid.raw(),
            session_id: initial_session_id,
            superseded_by_epoch: next_epoch,
        })
        .await
        .unwrap()
        .into_inner();

    assert!(revoke_resp.success);

    // Verify Gateway A cleanly cleaned up without entity despawn on Node 1
    assert_eq!(router_a.get_route_by_guid(player_guid), None);
    assert!(!sinks_a.contains_key(&initial_session_id));

    // Entity on Node 1 is STILL alive and active!
    {
        let sim = node_sim.read().await;
        assert!(sim.entity_store.contains_guid(player_guid));
        assert_eq!(*sim.session_epochs.get(&player_guid).unwrap(), 2);

        // 7. Verify Upstream Stale Packet Filtering
        // Delayed frame from Gateway A with epoch 1 is dropped
        assert!(!sim.is_valid_upstream_epoch(player_guid, 1));
        // Current frame from Gateway B with epoch 2 is accepted
        assert!(sim.is_valid_upstream_epoch(player_guid, 2));
    }

    // 8. Verify Stale Takeover Replay is Rejected
    let stale_takeover_resp = node_client
        .takeover_session_route(TakeoverSessionRouteRequest {
            guid: player_guid.raw(),
            new_session_id: 3003,
            new_gateway_id: "gw-evil-1".to_string(),
            new_gateway_grpc_addr: "http://127.0.0.1:9999".to_string(),
            session_epoch: 2, // Replaying epoch 2 when node is already at 2
            handover_signature: vec![],
        })
        .await
        .unwrap()
        .into_inner();

    assert!(!stale_takeover_resp.success);
    assert!(stale_takeover_resp
        .error_message
        .contains("Stale session epoch"));
    assert_eq!(stale_takeover_resp.authoritative_epoch, 2);
}
