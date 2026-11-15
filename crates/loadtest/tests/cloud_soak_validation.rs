//! Multi-Node Split, Live Batch Migration, Failover & Persistence Soak Integration Tests.
//!
//! Validates end-to-end load/soak cluster invariants without modifying core engine crates:
//! 1. Concurrent virtual bot swarm authentication (HS256 JWT) and 20 Hz kinematic movement
//!    against a live `GatewayServer` and multi-node `NodeServiceImpl` (`TickSimulation`) cluster.
//! 2. Authoritative batch entity migration (`execute_batch_entity_migration`) with Gateway
//!    `SessionRouter` route updates (`UpdateBatchRoutes`) under concurrent bot movement,
//!    confirming bots observe cross-node route handovers via server tick timeline transitions
//!    without connection drops.
//! 3. Node failover route evacuation (`GatewayService::reassign_node_routes`) and asynchronous
//!    write-behind SQLite snapshot persistence integrity (`WriteBehindSnapshotter` /
//!    `CharacterRepository`).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mmo_core::guid::Guid;
use mmo_core::BoundingBox3D;
use mmo_gateway::auth::EdgeAuthenticator;
use mmo_gateway::router::SessionRouter;
use mmo_gateway::server::{GatewayServer, RoutedPacket};
use mmo_loadtest::swarm::{SwarmConfig, SwarmManager};
use mmo_node::network::{execute_batch_entity_migration, NodeServiceImpl};
use mmo_node::snapshotter::WriteBehindSnapshotter;
use mmo_node::TickSimulation;
use mmo_schema::cluster::gateway_service_client::GatewayServiceClient;
use mmo_schema::cluster::node_service_client::NodeServiceClient;
use mmo_schema::cluster::{
    DeliverBroadcastJobsRequest, ForwardClientPacketRequest, ReassignNodeRoutesRequest,
    SessionPacketBatch,
};
use mmo_storage::db::Database;
use mmo_storage::repository::CharacterRepository;
use tokio::sync::{mpsc, RwLock};
use tokio::task::JoinHandle;

struct NodeHandle {
    addr: String,
    sim: Arc<RwLock<TickSimulation>>,
    grpc_task: JoinHandle<()>,
    tick_task: JoinHandle<()>,
}

impl NodeHandle {
    fn shutdown(self) {
        self.tick_task.abort();
        self.grpc_task.abort();
    }
}

async fn spawn_simulation_node(
    node_id: u16,
    world_bounds: BoundingBox3D,
    initial_tick: u64,
    snapshotter: &Arc<WriteBehindSnapshotter>,
    gw_grpc_addr: String,
) -> NodeHandle {
    let mut tick_sim = TickSimulation::new(node_id, world_bounds);
    tick_sim.tick_counter = initial_tick;
    tick_sim.set_snapshot_sender(snapshotter.sender());
    tick_sim.set_snapshot_interval_ticks(1);

    let input_tx = tick_sim.input_sender();
    let ghost_store = tick_sim.ghost_store.clone();
    let sim = Arc::new(RwLock::new(tick_sim));

    let node_listener =
        std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind node gRPC port");
    let node_addr: SocketAddr = node_listener
        .local_addr()
        .expect("Failed to read node socket addr");
    drop(node_listener);

    let node_service = NodeServiceImpl::new(Arc::clone(&sim), input_tx, ghost_store);
    let grpc_task = tokio::spawn(async move {
        let _ = tonic::transport::Server::builder()
            .add_service(node_service.into_server())
            .serve(node_addr)
            .await;
    });

    let tick_sim_arc = Arc::clone(&sim);
    let tick_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(50));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let gw_endpoint = format!("http://{gw_grpc_addr}");
        let mut gw_client_opt: Option<GatewayServiceClient<tonic::transport::Channel>> = None;

        loop {
            interval.tick().await;
            let jobs = {
                let mut guard = tick_sim_arc.write().await;
                let guids: Vec<Guid> = guard.entity_store.iter().map(|(_, e)| e.guid).collect();
                for guid in guids {
                    if let Some(sid) = guard.aoi.get_session_by_guid(guid) {
                        if let Some(obs) = guard.aoi.get_observer_mut(sid) {
                            obs.config.enable_delta_compression = true;
                        }
                    }
                }
                guard.step_tick();
                guard.drain_broadcast_jobs()
            };

            if jobs.is_empty() {
                continue;
            }

            if gw_client_opt.is_none() {
                gw_client_opt = GatewayServiceClient::connect(gw_endpoint.clone())
                    .await
                    .ok();
            }
            if let Some(ref mut client) = gw_client_opt {
                let batches = jobs
                    .into_iter()
                    .map(|job| SessionPacketBatch {
                        session_id: job.session_id,
                        packets: job.packets.into_iter().map(|p| p.encode()).collect(),
                    })
                    .collect();
                let req = DeliverBroadcastJobsRequest { jobs: batches };
                if client.deliver_broadcast_jobs(req).await.is_err() {
                    gw_client_opt = None;
                }
            }
        }
    });

    NodeHandle {
        addr: node_addr.to_string(),
        sim,
        grpc_task,
        tick_task,
    }
}

fn spawn_gateway_forwarder(mut packet_rx: mpsc::Receiver<RoutedPacket>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut node_clients: HashMap<String, NodeServiceClient<tonic::transport::Channel>> =
            HashMap::new();

        while let Some(routed) = packet_rx.recv().await {
            if !node_clients.contains_key(&routed.node_address) {
                let endpoint = format!("http://{}", routed.node_address);
                if let Ok(client) = NodeServiceClient::connect(endpoint).await {
                    node_clients.insert(routed.node_address.clone(), client);
                }
            }

            if let Some(client) = node_clients.get_mut(&routed.node_address) {
                let req = ForwardClientPacketRequest {
                    session_id: routed.session_id,
                    guid: routed.guid.map(|g| g.raw()).unwrap_or(0),
                    packet_data: routed.packet.encode(),
                    session_epoch: routed.session_epoch,
                };
                if client.forward_client_packet(req).await.is_err() {
                    node_clients.remove(&routed.node_address);
                }
            }
        }
    })
}

async fn wait_until<F, Fut>(timeout: Duration, mut pred: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if pred().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("Condition not satisfied within {:?}", timeout);
}

#[tokio::test]
async fn test_multi_node_swarm_split_migration_and_failover() {
    let secret = "cloud_soak_validation_jwt_secret_123456";
    let auth = Arc::new(EdgeAuthenticator::new_hs256(secret.as_bytes(), None, None));

    // 1. Initialize in-memory SQLite storage and WriteBehindSnapshotter
    let db = Database::connect("sqlite::memory:")
        .await
        .expect("Failed to initialize sqlite::memory:");
    let repo = CharacterRepository::new(db);
    let snapshotter = Arc::new(WriteBehindSnapshotter::new(repo.clone()));
    let snapshot_worker = snapshotter.clone().start_worker();

    // 2. Initialize Edge Gateway TCP + gRPC listeners
    let router = Arc::new(SessionRouter::new());
    let (packet_tx, packet_rx) = mpsc::channel(2048);

    let gw_grpc_listener =
        std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind gateway gRPC port");
    let gw_grpc_addr = gw_grpc_listener
        .local_addr()
        .expect("Failed to read gateway gRPC addr")
        .to_string();
    drop(gw_grpc_listener);

    // 3. Spawn two simulation nodes (Node 1 at tick 500, Node 2 at tick 20)
    let world_bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
    let node1_bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (0.0, 100.0, 100.0));
    let node2_bounds = BoundingBox3D::new((0.0, -100.0, -100.0), (100.0, 100.0, 100.0));

    let node1 =
        spawn_simulation_node(1, world_bounds, 500, &snapshotter, gw_grpc_addr.clone()).await;
    let node2 =
        spawn_simulation_node(2, world_bounds, 20, &snapshotter, gw_grpc_addr.clone()).await;

    let partitions = vec![
        (node1_bounds, 1, node1.addr.clone()),
        (node2_bounds, 2, node2.addr.clone()),
    ];
    node1
        .sim
        .write()
        .await
        .apply_topology_partitions(&partitions);
    node2
        .sim
        .write()
        .await
        .apply_topology_partitions(&partitions);

    let gateway = Arc::new(
        GatewayServer::with_packet_channel(Arc::clone(&router), packet_tx)
            .with_authenticator(auth)
            .with_default_node_address(node1.addr.clone()),
    );

    let gw_service = gateway.create_service();
    let gw_grpc_sock: SocketAddr = gw_grpc_addr.parse().unwrap();
    let gw_grpc_task = tokio::spawn(async move {
        let _ = tonic::transport::Server::builder()
            .add_service(gw_service.into_server())
            .serve(gw_grpc_sock)
            .await;
    });

    let tcp_listener = gateway
        .bind("127.0.0.1:0")
        .await
        .expect("Failed to bind gateway TCP port");
    let gateway_addr = tcp_listener.local_addr().unwrap().to_string();

    let gw_accept = Arc::clone(&gateway);
    let accept_task = tokio::spawn(async move {
        while let Ok((socket, _)) = tcp_listener.accept().await {
            let gw = Arc::clone(&gw_accept);
            let session_id = gw.next_session_id();
            tokio::spawn(async move {
                let _ = gw.handle_connection(socket, session_id).await;
            });
        }
    });
    let forwarder_task = spawn_gateway_forwarder(packet_rx);

    // Wait briefly so gRPC listeners are ready before swarm starts connecting
    tokio::time::sleep(Duration::from_millis(50)).await;

    // 4. Launch concurrent 8-bot swarm authenticating via JWT and streaming 20 Hz movement
    let bot_count = 8usize;
    let config = SwarmConfig {
        gateway_addr,
        bot_count,
        duration: Duration::from_millis(2200),
        tick_rate_hz: 20,
        ping_interval: Duration::from_millis(250),
        stagger_delay: Duration::from_millis(2),
        max_bot_speed: 8.0,
        ..Default::default()
    }
    .with_auth_secret(secret);

    let manager = SwarmManager::new(config);
    let swarm_handle = tokio::spawn(async move { manager.run().await });

    // Wait for all bots to hydrate and tick on Node 1
    wait_until(Duration::from_secs(5), || async {
        let guard = node1.sim.read().await;
        router.session_count() == bot_count
            && guard.entity_store.count() == bot_count
            && guard.tick_counter >= 505
    })
    .await;

    // 5. Execute live batch migration for half the swarm (4 bots) from Node 1 -> Node 2
    let mut all_guids: Vec<Guid> = {
        let guard = node1.sim.read().await;
        guard.entity_store.iter().map(|(_, e)| e.guid).collect()
    };
    all_guids.sort_by_key(|g| g.raw());
    let migrate_guids: Vec<Guid> = all_guids.iter().copied().take(4).collect();
    let stay_guids: Vec<Guid> = all_guids.iter().copied().skip(4).collect();

    let migration_report = execute_batch_entity_migration(
        Arc::clone(&node1.sim),
        &migrate_guids,
        2,
        &node2.addr,
        Some(&gw_grpc_addr),
    )
    .await
    .expect("Batch migration should succeed");

    assert!(migration_report.success);
    assert_eq!(migration_report.migrated_count, 4);

    // Verify simulation ownership and Gateway routing table immediately post-migration
    {
        let n1 = node1.sim.read().await;
        let n2 = node2.sim.read().await;
        assert_eq!(n1.entity_store.count(), 4);
        assert_eq!(n2.entity_store.count(), 4);
        for g in &migrate_guids {
            assert!(!n1.entity_store.contains_guid(*g));
            assert!(n2.entity_store.contains_guid(*g));
            assert_eq!(
                router.get_route_by_guid(*g).as_deref(),
                Some(node2.addr.as_str())
            );
        }
        for g in &stay_guids {
            assert_eq!(
                router.get_route_by_guid(*g).as_deref(),
                Some(node1.addr.as_str())
            );
        }
    }

    // Let bots stream movement to both Node 1 and Node 2 so AoI EntityDeltaBatch ticks emit
    // from Node 2 (lower tick timeline) and trigger handover telemetry without disconnection
    tokio::time::sleep(Duration::from_millis(650)).await;

    // 6. Trigger Node 2 failover route reassignment via GatewayService gRPC while swarm runs
    let mut gw_client = GatewayServiceClient::connect(format!("http://{gw_grpc_addr}"))
        .await
        .expect("Failed to connect to GatewayService gRPC");
    let reassign_resp = gw_client
        .reassign_node_routes(ReassignNodeRoutesRequest {
            dead_node_address: node2.addr.clone(),
            new_node_address: node1.addr.clone(),
        })
        .await
        .expect("reassign_node_routes RPC failed")
        .into_inner();
    assert!(reassign_resp.success);
    // 4 migrated bots * (guid_to_node + session_to_node) = 8 route entries reassigned
    assert_eq!(reassign_resp.reassigned_count, 8);

    for g in &all_guids {
        assert_eq!(
            router.get_route_by_guid(*g).as_deref(),
            Some(node1.addr.as_str())
        );
    }

    // Wait for swarm completion and assert extended telemetry
    let snapshot = swarm_handle.await.expect("Swarm task panicked");
    assert_eq!(snapshot.authenticated_sessions, bot_count as u64);
    assert_eq!(snapshot.auth_failures, 0);
    assert_eq!(snapshot.connection_errors, 0);
    assert!(snapshot.auth_throughput_sps > 0.0);
    assert!(snapshot.entity_deltas_received > 0);
    assert!(
        snapshot.route_handovers_observed >= 4,
        "Expected >= 4 cross-node handovers from migrated bots, observed {}",
        snapshot.route_handovers_observed
    );
    assert!(snapshot.total_bytes_received > 0);
    assert!(snapshot.recv_bandwidth_kbps > 0.0);
    assert!(snapshot.total_packets_sent > 0);
    assert!(snapshot.total_packets_received > 0);

    // 7. Capture final authoritative positions on Node 1 before stopping, flush snapshots, and verify SQLite
    let expected_node1_positions: Vec<(Guid, (f32, f32, f32))> = {
        let guard = node1.sim.read().await;
        stay_guids
            .iter()
            .map(|g| (*g, guard.entity_store.get_by_guid(*g).unwrap().position))
            .collect()
    };

    node1.shutdown();
    node2.shutdown();
    accept_task.abort();
    gw_grpc_task.abort();
    forwarder_task.abort();

    let _ = snapshotter.flush_batch().await;
    snapshotter.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(3), snapshot_worker).await;

    for g in &all_guids {
        let persisted = repo
            .get_snapshot(g.raw())
            .await
            .expect("SQLite snapshot query failed")
            .expect("Bot snapshot must be persisted in SQLite");
        assert!(persisted.version >= 1);
        assert!(persisted.position.0.is_finite());
        assert!(persisted.position.1.is_finite());
        assert!(persisted.position.2.is_finite());
    }

    for (g, expected_pos) in expected_node1_positions {
        let persisted = repo.get_snapshot(g.raw()).await.unwrap().unwrap();
        assert_eq!(
            persisted.position, expected_pos,
            "Persisted Node 1 snapshot for {g:?} must match final authoritative position"
        );
    }
}

#[tokio::test]
async fn test_soak_write_behind_snapshot_integrity_under_sustained_movement() {
    let secret = "soak_persistence_secret_abcdef_987654";
    let auth = Arc::new(EdgeAuthenticator::new_hs256(secret.as_bytes(), None, None));

    let db = Database::connect("sqlite::memory:")
        .await
        .expect("Failed to initialize sqlite::memory:");
    let repo = CharacterRepository::new(db);
    let snapshotter = Arc::new(WriteBehindSnapshotter::new(repo.clone()));
    let snapshot_worker = snapshotter.clone().start_worker();

    let router = Arc::new(SessionRouter::new());
    let (packet_tx, packet_rx) = mpsc::channel(2048);

    let gw_grpc_listener =
        std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind gateway gRPC port");
    let gw_grpc_addr = gw_grpc_listener.local_addr().unwrap().to_string();
    drop(gw_grpc_listener);

    let world_bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
    let node1 = spawn_simulation_node(1, world_bounds, 1, &snapshotter, gw_grpc_addr.clone()).await;

    let gateway = Arc::new(
        GatewayServer::with_packet_channel(Arc::clone(&router), packet_tx)
            .with_authenticator(auth)
            .with_default_node_address(node1.addr.clone()),
    );

    let gw_service = gateway.create_service();
    let gw_grpc_sock: SocketAddr = gw_grpc_addr.parse().unwrap();
    let gw_grpc_task = tokio::spawn(async move {
        let _ = tonic::transport::Server::builder()
            .add_service(gw_service.into_server())
            .serve(gw_grpc_sock)
            .await;
    });

    let tcp_listener = gateway.bind("127.0.0.1:0").await.unwrap();
    let gateway_addr = tcp_listener.local_addr().unwrap().to_string();

    let gw_accept = Arc::clone(&gateway);
    let accept_task = tokio::spawn(async move {
        while let Ok((socket, _)) = tcp_listener.accept().await {
            let gw = Arc::clone(&gw_accept);
            let session_id = gw.next_session_id();
            tokio::spawn(async move {
                let _ = gw.handle_connection(socket, session_id).await;
            });
        }
    });
    let forwarder_task = spawn_gateway_forwarder(packet_rx);

    tokio::time::sleep(Duration::from_millis(50)).await;

    let bot_count = 10usize;
    let config = SwarmConfig {
        gateway_addr,
        bot_count,
        duration: Duration::from_millis(1200),
        tick_rate_hz: 20,
        ping_interval: Duration::from_millis(250),
        stagger_delay: Duration::from_millis(2),
        max_bot_speed: 8.0,
        ..Default::default()
    }
    .with_auth_secret(secret);

    let snapshot = SwarmManager::new(config).run().await;
    assert_eq!(snapshot.authenticated_sessions, bot_count as u64);
    assert_eq!(snapshot.auth_failures, 0);
    assert_eq!(snapshot.connection_errors, 0);

    // Stop incoming movement, step one final tick so all dirty states emit, then flush cleanly
    accept_task.abort();
    forwarder_task.abort();

    let expected_positions: Vec<(Guid, (f32, f32, f32))> = {
        let mut guard = node1.sim.write().await;
        guard.step_tick();
        guard
            .entity_store
            .iter()
            .map(|(_, e)| (e.guid, e.position))
            .collect()
    };
    assert_eq!(expected_positions.len(), bot_count);

    node1.shutdown();
    gw_grpc_task.abort();

    let _ = snapshotter.flush_batch().await;
    snapshotter.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(3), snapshot_worker).await;

    // Verify all 10 bots persisted accurately and restore cleanly into a fresh TickSimulation
    let mut restored_sim = TickSimulation::new(1, world_bounds);
    for (guid, expected_pos) in &expected_positions {
        let persisted = repo
            .get_snapshot(guid.raw())
            .await
            .expect("SQLite snapshot query failed")
            .expect("Expected persisted snapshot for every bot");
        assert_eq!(persisted.position, *expected_pos);
        assert!(persisted.version >= 2);
        restored_sim.spawn_entity(*guid, persisted.position);
    }
    assert_eq!(restored_sim.entity_store.count(), bot_count);
}
