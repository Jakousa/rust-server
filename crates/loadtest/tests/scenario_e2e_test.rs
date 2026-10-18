//! End-to-end integration test executing declarative scenario manifests against a live
//! Edge Gateway and authoritative Node service cluster.

use std::sync::Arc;

use mmo_core::guid::{EntityType, Guid};
use mmo_core::BoundingBox3D;
use mmo_gateway::auth::EdgeAuthenticator;
use mmo_gateway::router::SessionRouter;
use mmo_gateway::server::GatewayServer;
use mmo_loadtest::scenario::{ScenarioManifest, ScenarioRunner, ScenarioStep};
use mmo_node::network::NodeServiceImpl;
use mmo_node::TickSimulation;
use mmo_schema::client::{ClientPacket, EntityStatePacket};

#[tokio::test]
async fn test_scenario_runner_e2e_cluster_lifecycle() {
    let test_secret = b"cluster_e2e_jwt_secret_key_scenario_testing_12345";
    let auth = Arc::new(EdgeAuthenticator::new_hs256(test_secret, None, None));

    // 1. Initialize authoritative simulation node
    let bounds = BoundingBox3D::new((-500.0, -500.0, -500.0), (500.0, 500.0, 500.0));
    let sim = Arc::new(tokio::sync::RwLock::new(TickSimulation::new(1, bounds)));
    let node_service = NodeServiceImpl::new(Arc::clone(&sim));

    let node_listener =
        std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind ephemeral node port");
    let node_addr = node_listener
        .local_addr()
        .expect("Failed to read node socket addr");
    drop(node_listener);

    let node_server = node_service.into_server();
    tokio::spawn(async move {
        let _ = tonic::transport::Server::builder()
            .add_service(node_server)
            .serve(node_addr)
            .await;
    });

    // 2. Initialize Edge Gateway with packet routing channel and authenticator
    let router = Arc::new(SessionRouter::new());
    let (packet_tx, mut packet_rx) = tokio::sync::mpsc::channel(100);

    let gateway = Arc::new(
        GatewayServer::with_packet_channel(Arc::clone(&router), packet_tx)
            .with_authenticator(auth.clone())
            .with_default_node_address(node_addr.to_string()),
    );

    let gateway_listener = gateway
        .bind("127.0.0.1:0")
        .await
        .expect("Failed to bind gateway TCP listener");
    let gateway_addr = gateway_listener
        .local_addr()
        .expect("Failed to get gateway local addr");

    // Spawn Gateway TCP connection accept loop
    let gw_accept = Arc::clone(&gateway);
    let accept_task = tokio::spawn(async move {
        while let Ok((socket, _)) = gateway_listener.accept().await {
            let gw = Arc::clone(&gw_accept);
            let session_id = gw.next_session_id();
            tokio::spawn(async move {
                let _ = gw.handle_connection(socket, session_id).await;
            });
        }
    });

    // 3. Spawn simulation worker processing movement routed from gateway and broadcasting state
    let sim_worker = Arc::clone(&sim);
    let gw_worker = Arc::clone(&gateway);
    let worker_task = tokio::spawn(async move {
        while let Some(routed) = packet_rx.recv().await {
            if let ClientPacket::Movement(mv) = routed.packet {
                let guid = Guid::from_raw(mv.entity_id);
                let mut s = sim_worker.write().await;
                if s.entity_store.get_by_guid(guid).is_none() {
                    s.spawn_entity(guid, (mv.position.x, mv.position.y, mv.position.z));
                }
                let _ = s.process_client_move(
                    guid,
                    (mv.position.x, mv.position.y, mv.position.z),
                    0.05,
                );

                let state_packet = ClientPacket::EntityState(EntityStatePacket {
                    entity_id: guid.raw(),
                    position: mv.position,
                    velocity: mv.velocity,
                    yaw: mv.yaw,
                    pitch: mv.pitch,
                    health: 100,
                    max_health: 100,
                    state_flags: 0,
                    timestamp: 1,
                });
                let _ = gw_worker.broadcast_packet(&state_packet).await;
            }
        }
    });

    // 4. Generate valid authentication tokens
    let bot1_guid = Guid::new(EntityType::Player, 1, 101);
    let bot2_guid = Guid::new(EntityType::Player, 1, 102);

    let token1 = EdgeAuthenticator::create_test_jwt(bot1_guid, test_secret, 3600, None, None);
    let token2 = EdgeAuthenticator::create_test_jwt(bot2_guid, test_secret, 3600, None, None);

    // 5. Construct declarative scenario manifest
    let manifest = ScenarioManifest {
        name: "e2e_cluster_multi_bot_lifecycle".to_string(),
        gateway_addr: gateway_addr.to_string(),
        max_p99_rtt_ms: Some(500.0),
        steps: vec![
            ScenarioStep::Connect { bot_id: 1 },
            ScenarioStep::Authenticate {
                bot_id: 1,
                token: token1,
                expect_success: true,
            },
            ScenarioStep::Ping { bot_id: 1 },
            ScenarioStep::Connect { bot_id: 2 },
            ScenarioStep::Authenticate {
                bot_id: 2,
                token: token2,
                expect_success: true,
            },
            ScenarioStep::Ping { bot_id: 2 },
            ScenarioStep::Move {
                bot_id: 1,
                position: [10.0, 0.0, 10.0],
                velocity: [1.0, 0.0, 0.0],
                yaw: 0.0,
            },
            ScenarioStep::WaitMs { duration_ms: 100 },
            ScenarioStep::AssertKnownEntity {
                bot_id: 2,
                target_guid: bot1_guid.raw(),
                max_distance: Some(25.0),
            },
            ScenarioStep::Chat {
                bot_id: 1,
                channel: "global".to_string(),
                message: "Greetings from bot 1".to_string(),
            },
            ScenarioStep::Disconnect { bot_id: 1 },
            ScenarioStep::Disconnect { bot_id: 2 },
        ],
    };

    // 6. Execute scenario runner and assert success
    let report = ScenarioRunner::run(&manifest)
        .await
        .expect("Scenario execution should succeed");

    assert_eq!(report.scenario_name, "e2e_cluster_multi_bot_lifecycle");
    assert_eq!(report.steps_executed, 12);
    assert!(report.p50_rtt_ms >= 0.0);
    assert!(report.p99_rtt_ms <= 500.0);
    assert!(report.errors.is_empty());

    // Clean up background tasks
    accept_task.abort();
    worker_task.abort();
}

#[tokio::test]
async fn test_scenario_runner_auth_rejection() {
    let test_secret = b"cluster_e2e_jwt_secret_key_scenario_testing_12345";
    let auth = Arc::new(EdgeAuthenticator::new_hs256(test_secret, None, None));
    let router = Arc::new(SessionRouter::new());
    let (packet_tx, _packet_rx) = tokio::sync::mpsc::channel(16);

    let gateway = Arc::new(
        GatewayServer::with_packet_channel(Arc::clone(&router), packet_tx)
            .with_authenticator(auth)
            .with_default_node_address("127.0.0.1:50051".to_string()),
    );

    let listener = gateway.bind("127.0.0.1:0").await.unwrap();
    let gateway_addr = listener.local_addr().unwrap();

    let gw_accept = Arc::clone(&gateway);
    let accept_task = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let gw = Arc::clone(&gw_accept);
            let session_id = gw.next_session_id();
            tokio::spawn(async move {
                let _ = gw.handle_connection(socket, session_id).await;
            });
        }
    });

    let manifest = ScenarioManifest {
        name: "auth_rejection_scenario".to_string(),
        gateway_addr: gateway_addr.to_string(),
        max_p99_rtt_ms: None,
        steps: vec![
            ScenarioStep::Connect { bot_id: 99 },
            ScenarioStep::Authenticate {
                bot_id: 99,
                token: "invalid.bogus.jwt".to_string(),
                expect_success: false,
            },
            ScenarioStep::Disconnect { bot_id: 99 },
        ],
    };

    let report = ScenarioRunner::run(&manifest)
        .await
        .expect("Expected negative auth assertion to pass");
    assert_eq!(report.steps_executed, 3);
    accept_task.abort();
}
