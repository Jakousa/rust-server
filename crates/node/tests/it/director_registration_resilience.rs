use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::{oneshot, RwLock};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

use mmo_core::guid::{EntityType, Guid};
use mmo_core::{BoundingBox3D, TelemetryServer};
use mmo_director::coordinator::GlobalOctreeCoordinator;
use mmo_director::registry::NodeRegistry;
use mmo_director::service::DirectorServiceImpl;
use mmo_gateway::router::SessionRouter;
use mmo_gateway::server::{GatewayServer, GatewayServiceServer};
use mmo_node::network::NodeServiceImpl;
use mmo_node::runner::{run_director_registration_and_heartbeat_loop, DirectorHeartbeatConfig};
use mmo_node::sim::TickSimulation;
use mmo_schema::cluster::director_service_server::DirectorServiceServer;

async fn spawn_director_on(
    listener: TcpListener,
    bounds: BoundingBox3D,
) -> (
    Arc<RwLock<NodeRegistry>>,
    DirectorServiceImpl,
    oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let registry = Arc::new(RwLock::new(NodeRegistry::new()));
    let coordinator = Arc::new(RwLock::new(GlobalOctreeCoordinator::new(bounds)));
    let service = DirectorServiceImpl::new(Arc::clone(&registry), coordinator);
    let svc_for_server = service.clone();

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let incoming = TcpListenerStream::new(listener);
    let handle = tokio::spawn(async move {
        let _ = Server::builder()
            .add_service(DirectorServiceServer::new(svc_for_server))
            .serve_with_incoming_shutdown(incoming, async move {
                let _ = shutdown_rx.await;
            })
            .await;
    });

    (registry, service, shutdown_tx, handle)
}

fn make_sim(node_id: u16, bounds: BoundingBox3D) -> Arc<RwLock<TickSimulation>> {
    Arc::new(RwLock::new(TickSimulation::new(node_id, bounds)))
}

#[tokio::test]
async fn test_node_retries_registration_until_director_starts() {
    let bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));

    // Reserve an ephemeral port, then drop the listener so Director is initially unreachable.
    let tmp_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let director_addr: SocketAddr = tmp_listener.local_addr().unwrap();
    drop(tmp_listener);

    let telemetry = TelemetryServer::init().unwrap();
    assert!(!telemetry.is_ready());

    let (fenced_tx, mut fenced_rx) = tokio::sync::mpsc::channel::<String>(1);
    let config = DirectorHeartbeatConfig {
        node_id: 1,
        listen_addr: "127.0.0.1:51001".parse().unwrap(),
        bounds,
        director_endpoint: format!("http://{director_addr}"),
        retry_interval: Duration::from_millis(40),
        heartbeat_interval: Duration::from_millis(40),
        rpc_timeout: Duration::from_millis(200),
        lease_timeout: None,
    };

    let sim = make_sim(1, bounds);
    let telemetry_clone = telemetry.clone();
    let node_handle = tokio::spawn(async move {
        run_director_registration_and_heartbeat_loop(config, sim, telemetry_clone, fenced_tx).await;
    });

    // While Director is down, node must remain unready and must not fence.
    tokio::time::sleep(Duration::from_millis(180)).await;
    assert!(
        !telemetry.is_ready(),
        "node should remain unready before Director starts"
    );
    assert!(fenced_rx.try_recv().is_err());

    // Now start Director on the target address.
    let listener = TcpListener::bind(director_addr).await.unwrap();
    let (registry, _svc, shutdown_tx, dir_handle) = spawn_director_on(listener, bounds).await;

    // Wait for node to retry, register, and mark ready.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while !telemetry.is_ready() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "node did not become ready after Director started"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    assert_eq!(registry.read().await.len(), 1);
    assert!(registry.read().await.get_node(1).is_some());

    node_handle.abort();
    let _ = shutdown_tx.send(());
    let _ = dir_handle.await;
}

#[tokio::test]
async fn test_node_reregisters_on_director_restart() {
    let bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
    let listener1 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let director_addr: SocketAddr = listener1.local_addr().unwrap();

    let (reg1, _svc1, shutdown_tx1, dir_handle1) = spawn_director_on(listener1, bounds).await;

    let telemetry = TelemetryServer::init().unwrap();
    let (fenced_tx, mut fenced_rx) = tokio::sync::mpsc::channel::<String>(1);
    let config = DirectorHeartbeatConfig {
        node_id: 2,
        listen_addr: "127.0.0.1:51002".parse().unwrap(),
        bounds,
        director_endpoint: format!("http://{director_addr}"),
        retry_interval: Duration::from_millis(40),
        heartbeat_interval: Duration::from_millis(40),
        rpc_timeout: Duration::from_millis(200),
        lease_timeout: None,
    };

    let sim = make_sim(1, bounds);
    let telemetry_clone = telemetry.clone();
    let node_handle = tokio::spawn(async move {
        run_director_registration_and_heartbeat_loop(config, sim, telemetry_clone, fenced_tx).await;
    });

    // Wait for initial registration with Director #1.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while !telemetry.is_ready() || reg1.read().await.is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "node did not register with Director #1"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Shut down Director #1 to simulate a pod restart.
    let _ = shutdown_tx1.send(());
    let _ = dir_handle1.await;

    // Start Director #2 on the same address with a fresh empty in-memory registry.
    let listener2 = TcpListener::bind(director_addr).await.unwrap();
    let (reg2, _svc2, shutdown_tx2, dir_handle2) = spawn_director_on(listener2, bounds).await;

    // Node must automatically re-register with Director #2 without fencing.
    let deadline2 = tokio::time::Instant::now() + Duration::from_secs(3);
    while reg2.read().await.get_node(2).is_none() {
        assert!(
            tokio::time::Instant::now() < deadline2,
            "node did not re-register after Director restart"
        );
        if let Ok(reason) = fenced_rx.try_recv() {
            panic!("node unexpectedly fenced during Director restart: {reason}");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    assert!(telemetry.is_ready());
    assert!(fenced_rx.try_recv().is_err());

    node_handle.abort();
    let _ = shutdown_tx2.send(());
    let _ = dir_handle2.await;
}

#[tokio::test]
async fn test_evicted_node_still_fenced_with_failed_precondition() {
    let bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let director_addr: SocketAddr = listener.local_addr().unwrap();

    let (registry, service, shutdown_tx, dir_handle) = spawn_director_on(listener, bounds).await;

    let telemetry = TelemetryServer::init().unwrap();
    let (fenced_tx, mut fenced_rx) = tokio::sync::mpsc::channel::<String>(1);
    let config = DirectorHeartbeatConfig {
        node_id: 3,
        listen_addr: "127.0.0.1:51003".parse().unwrap(),
        bounds,
        director_endpoint: format!("http://{director_addr}"),
        retry_interval: Duration::from_millis(40),
        heartbeat_interval: Duration::from_millis(50),
        rpc_timeout: Duration::from_millis(200),
        lease_timeout: None,
    };

    let sim = make_sim(1, bounds);
    let telemetry_clone = telemetry.clone();
    let node_handle = tokio::spawn(async move {
        run_director_registration_and_heartbeat_loop(config, sim, telemetry_clone, fenced_tx).await;
    });

    // Wait for node 3 to register.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while registry.read().await.get_node(3).is_none() {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Force-evict node 3 by expiring its heartbeat and running dead-node recovery.
    {
        let mut reg = registry.write().await;
        reg.set_last_heartbeat(3, std::time::Instant::now() - Duration::from_secs(10));
    }
    service
        .check_and_recover_dead_nodes(Duration::from_secs(5))
        .await;

    // Next heartbeat from node 3 must receive FailedPrecondition and trigger fencing.
    let reason = tokio::time::timeout(Duration::from_secs(3), fenced_rx.recv())
        .await
        .expect("timed out waiting for evicted node to be fenced")
        .expect("fenced_rx closed unexpectedly");
    assert!(
        reason.contains("evicted") || reason.contains("fenced"),
        "unexpected fence reason: {reason}"
    );

    let _ = node_handle.await;
    let _ = shutdown_tx.send(());
    let _ = dir_handle.await;
}

#[tokio::test]
async fn test_reregistration_lease_timeout_marks_unready_and_fences() {
    let bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
    let listener1 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let director_addr: SocketAddr = listener1.local_addr().unwrap();

    let (reg1, _svc1, shutdown_tx1, dir_handle1) = spawn_director_on(listener1, bounds).await;

    let telemetry = TelemetryServer::init().unwrap();
    let (fenced_tx, mut fenced_rx) = tokio::sync::mpsc::channel::<String>(1);
    let config = DirectorHeartbeatConfig {
        node_id: 4,
        listen_addr: "127.0.0.1:51004".parse().unwrap(),
        bounds,
        director_endpoint: format!("http://{director_addr}"),
        retry_interval: Duration::from_millis(30),
        heartbeat_interval: Duration::from_millis(40),
        rpc_timeout: Duration::from_millis(100),
        lease_timeout: Some(Duration::from_millis(200)),
    };

    let sim = make_sim(4, bounds);
    let telemetry_clone = telemetry.clone();
    let node_handle = tokio::spawn(async move {
        run_director_registration_and_heartbeat_loop(config, sim, telemetry_clone, fenced_tx).await;
    });

    // Wait for initial registration and readiness.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while !telemetry.is_ready() || reg1.read().await.is_empty() {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Deregister node on Director so next heartbeat returns acknowledged=false,
    // then shut down Director so re-registration stalls past lease_timeout.
    {
        let mut reg = reg1.write().await;
        reg.deregister_node(4);
    }
    // Give one heartbeat cycle to enter register_until_acknowledged or expire lease,
    // and shut down Director so register_node cannot succeed.
    let _ = shutdown_tx1.send(());
    let _ = dir_handle1.await;

    let reason = tokio::time::timeout(Duration::from_secs(3), fenced_rx.recv())
        .await
        .expect("timed out waiting for lease expiration self-fencing")
        .expect("fenced_rx closed unexpectedly");
    assert!(
        reason.contains("lease expired"),
        "unexpected reason: {reason}"
    );
    assert!(
        !telemetry.is_ready(),
        "telemetry must be marked unready when lease expires"
    );

    let _ = node_handle.await;
}

#[derive(Default)]
struct TelemetryInspectingDirector {
    heartbeats: std::sync::Mutex<Vec<mmo_schema::cluster::HeartbeatRequest>>,
    split_reports: std::sync::Mutex<Vec<mmo_schema::cluster::SplitReportRequest>>,
    instance_heartbeats: std::sync::Mutex<Vec<mmo_schema::cluster::InstanceHeartbeatRequest>>,
    trigger_split: std::sync::atomic::AtomicBool,
}

#[tonic::async_trait]
impl mmo_schema::cluster::director_service_server::DirectorService for TelemetryInspectingDirector {
    async fn register_node(
        &self,
        request: tonic::Request<mmo_schema::cluster::RegisterNodeRequest>,
    ) -> Result<tonic::Response<mmo_schema::cluster::RegisterNodeResponse>, tonic::Status> {
        let req = request.into_inner();
        Ok(tonic::Response::new(
            mmo_schema::cluster::RegisterNodeResponse {
                success: true,
                message: "ok".to_string(),
                assigned_node_id: req.node_id,
                topology_epoch: 1,
            },
        ))
    }

    async fn heartbeat(
        &self,
        request: tonic::Request<mmo_schema::cluster::HeartbeatRequest>,
    ) -> Result<tonic::Response<mmo_schema::cluster::HeartbeatResponse>, tonic::Status> {
        self.heartbeats.lock().unwrap().push(request.into_inner());
        let should_split = self.trigger_split.load(std::sync::atomic::Ordering::SeqCst);
        Ok(tonic::Response::new(
            mmo_schema::cluster::HeartbeatResponse {
                acknowledged: true,
                should_split,
                topology_epoch: 1,
                partitions: Vec::new(),
            },
        ))
    }

    async fn report_split(
        &self,
        request: tonic::Request<mmo_schema::cluster::SplitReportRequest>,
    ) -> Result<tonic::Response<mmo_schema::cluster::SplitReportResponse>, tonic::Status> {
        let req = request.into_inner();
        let nid = req.node_id;
        self.split_reports.lock().unwrap().push(req);
        Ok(tonic::Response::new(
            mmo_schema::cluster::SplitReportResponse {
                acknowledged: true,
                assigned_node_ids: vec![nid, 99, 99, 99, 99, 99, 99, 99],
                topology_epoch: 2,
                assigned_node_addresses: Vec::new(),
                partitions: Vec::new(),
            },
        ))
    }

    async fn register_gateway(
        &self,
        _request: tonic::Request<mmo_schema::cluster::RegisterGatewayRequest>,
    ) -> Result<tonic::Response<mmo_schema::cluster::RegisterGatewayResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("unused"))
    }

    async fn gateway_heartbeat(
        &self,
        _request: tonic::Request<mmo_schema::cluster::GatewayHeartbeatRequest>,
    ) -> Result<tonic::Response<mmo_schema::cluster::GatewayHeartbeatResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("unused"))
    }

    async fn create_instance(
        &self,
        _request: tonic::Request<mmo_schema::cluster::CreateInstanceRequest>,
    ) -> Result<tonic::Response<mmo_schema::cluster::CreateInstanceResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("unused"))
    }

    async fn terminate_instance(
        &self,
        _request: tonic::Request<mmo_schema::cluster::TerminateInstanceRequest>,
    ) -> Result<tonic::Response<mmo_schema::cluster::TerminateInstanceResponse>, tonic::Status>
    {
        Err(tonic::Status::unimplemented("unused"))
    }

    async fn instance_heartbeat(
        &self,
        request: tonic::Request<mmo_schema::cluster::InstanceHeartbeatRequest>,
    ) -> Result<tonic::Response<mmo_schema::cluster::InstanceHeartbeatResponse>, tonic::Status>
    {
        self.instance_heartbeats
            .lock()
            .unwrap()
            .push(request.into_inner());
        Ok(tonic::Response::new(
            mmo_schema::cluster::InstanceHeartbeatResponse {
                acknowledged: true,
                should_terminate: false,
            },
        ))
    }

    async fn issue_realm_transfer_claim(
        &self,
        _request: tonic::Request<mmo_schema::cluster::IssueRealmTransferClaimRequest>,
    ) -> Result<tonic::Response<mmo_schema::cluster::IssueRealmTransferClaimResponse>, tonic::Status>
    {
        Err(tonic::Status::unimplemented("unused"))
    }

    async fn verify_realm_transfer_claim(
        &self,
        _request: tonic::Request<mmo_schema::cluster::VerifyRealmTransferClaimRequest>,
    ) -> Result<tonic::Response<mmo_schema::cluster::VerifyRealmTransferClaimResponse>, tonic::Status>
    {
        Err(tonic::Status::unimplemented("unused"))
    }

    async fn deregister_node(
        &self,
        _request: tonic::Request<mmo_schema::cluster::DeregisterNodeRequest>,
    ) -> Result<tonic::Response<mmo_schema::cluster::DeregisterNodeResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("unused"))
    }

    async fn deregister_gateway(
        &self,
        _request: tonic::Request<mmo_schema::cluster::DeregisterGatewayRequest>,
    ) -> Result<tonic::Response<mmo_schema::cluster::DeregisterGatewayResponse>, tonic::Status>
    {
        Err(tonic::Status::unimplemented("unused"))
    }
}

#[tokio::test]
async fn test_heartbeat_reports_real_telemetry_and_updates_current_bounds_on_split() {
    let bounds = BoundingBox3D::new((0.0, 0.0, 0.0), (100.0, 100.0, 100.0));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let director_addr: SocketAddr = listener.local_addr().unwrap();

    let mock = Arc::new(TelemetryInspectingDirector::default());
    let mock_for_srv = Arc::clone(&mock);
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let incoming = TcpListenerStream::new(listener);
    let dir_handle = tokio::spawn(async move {
        let _ = Server::builder()
            .add_service(DirectorServiceServer::from_arc(mock_for_srv))
            .serve_with_incoming_shutdown(incoming, async move {
                let _ = shutdown_rx.await;
            })
            .await;
    });

    let sim = make_sim(5, bounds);
    {
        let mut s = sim.write().await;
        s.instances
            .create_instance(777, 1, bounds, Duration::from_secs(60))
            .unwrap();
        // Advance 2 ticks before first heartbeat so tick_rate is non-zero and not hardcoded 20.0
        s.step_tick();
        s.step_tick();
    }

    let telemetry = TelemetryServer::init().unwrap();
    let (fenced_tx, _fenced_rx) = tokio::sync::mpsc::channel::<String>(1);
    let config = DirectorHeartbeatConfig {
        node_id: 5,
        listen_addr: "127.0.0.1:51005".parse().unwrap(),
        bounds,
        director_endpoint: format!("http://{director_addr}"),
        retry_interval: Duration::from_millis(30),
        heartbeat_interval: Duration::from_millis(60),
        rpc_timeout: Duration::from_millis(200),
        lease_timeout: None,
    };

    let sim_clone = Arc::clone(&sim);
    let node_handle = tokio::spawn(async move {
        run_director_registration_and_heartbeat_loop(config, sim_clone, telemetry, fenced_tx).await;
    });

    // Wait for at least 2 heartbeats and instance heartbeats
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while mock.heartbeats.lock().unwrap().len() < 2
        || mock.instance_heartbeats.lock().unwrap().is_empty()
    {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    {
        let hbs = mock.heartbeats.lock().unwrap();
        // Second heartbeat had 0 ticks in ~60ms, so rolling tick_rate must differ from hardcoded 20.0
        assert!(
            hbs[1].tick_rate >= 0.0 && hbs[1].tick_rate != 20.0,
            "expected real rolling tick_rate rather than hardcoded 20.0, got {}",
            hbs[1].tick_rate
        );
        #[cfg(target_os = "linux")]
        assert!(
            hbs[0].memory_bytes > 0 && hbs[0].memory_bytes != 10_000_000,
            "expected real RSS memory_bytes, got {}",
            hbs[0].memory_bytes
        );

        let inst_hbs = mock.instance_heartbeats.lock().unwrap();
        assert_eq!(inst_hbs[0].instance_id, 777);
        assert_eq!(inst_hbs[0].node_id, 5);
        assert!(inst_hbs[0].is_idle);
    }

    // Trigger split reports and verify second split uses narrowed child octant (0..50, 0..50, 0..50)
    mock.trigger_split
        .store(true, std::sync::atomic::Ordering::SeqCst);

    let deadline2 = tokio::time::Instant::now() + Duration::from_secs(3);
    while mock.split_reports.lock().unwrap().len() < 2 {
        assert!(tokio::time::Instant::now() < deadline2);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    {
        let splits = mock.split_reports.lock().unwrap();
        let first_orig = mmo_director::proto_bbox_to_bounding_box_3d(
            splits[0].original_bounds.as_ref().unwrap(),
        )
        .unwrap();
        let second_orig = mmo_director::proto_bbox_to_bounding_box_3d(
            splits[1].original_bounds.as_ref().unwrap(),
        )
        .unwrap();

        assert_eq!(first_orig, bounds);
        assert_eq!(
            second_orig,
            BoundingBox3D::new((0.0, 0.0, 0.0), (50.0, 50.0, 50.0)),
            "second split must report updated active child octant bounds"
        );
    }

    node_handle.abort();
    let _ = shutdown_tx.send(());
    let _ = dir_handle.await;
}

async fn spawn_node_grpc_on(
    node_id: u16,
    bounds: BoundingBox3D,
    gateway_grpc_addr: SocketAddr,
) -> (
    Arc<RwLock<TickSimulation>>,
    SocketAddr,
    tokio::task::JoinHandle<()>,
) {
    let mut tick_sim = TickSimulation::new(node_id, bounds);
    tick_sim.gateway_grpc_addr = format!("http://{gateway_grpc_addr}");
    let input_tx = tick_sim.input_sender();
    let ghost_store = tick_sim.ghost_store.clone();
    let sim = Arc::new(RwLock::new(tick_sim));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let service = NodeServiceImpl::new(Arc::clone(&sim), input_tx, ghost_store);
    let incoming = TcpListenerStream::new(listener);
    let handle = tokio::spawn(async move {
        let _ = Server::builder()
            .add_service(service.into_server())
            .serve_with_incoming(incoming)
            .await;
    });

    (sim, addr, handle)
}

struct SplitClusterFixture {
    registry: Arc<RwLock<NodeRegistry>>,
    director_service: DirectorServiceImpl,
    router: Arc<SessionRouter>,
    gateway_server: Arc<GatewayServer>,
    node1_sim: Arc<RwLock<TickSimulation>>,
    node2_sim: Arc<RwLock<TickSimulation>>,
    node1_addr: SocketAddr,
    node2_addr: SocketAddr,
    spawned_players: Vec<(Guid, u64, (f32, f32, f32))>,
    dir_shutdown_tx: oneshot::Sender<()>,
    dir_handle: tokio::task::JoinHandle<()>,
    gw_grpc_handle: tokio::task::JoinHandle<()>,
    gw_hb_handle: tokio::task::JoinHandle<()>,
    node1_grpc_handle: tokio::task::JoinHandle<()>,
    node2_grpc_handle: tokio::task::JoinHandle<()>,
    node1_hb_handle: tokio::task::JoinHandle<()>,
    node2_hb_handle: tokio::task::JoinHandle<()>,
}

impl SplitClusterFixture {
    async fn start_and_split() -> Self {
        let bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));

        // 1. Start real DirectorServiceImpl on ephemeral port
        let dir_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let director_addr = dir_listener.local_addr().unwrap();
        let (registry, director_service, dir_shutdown_tx, dir_handle) =
            spawn_director_on(dir_listener, bounds).await;

        // 2. Start real GatewayServer gRPC service & heartbeat loop
        let router = Arc::new(SessionRouter::new());
        let gateway_server = Arc::new(
            GatewayServer::new(Arc::clone(&router)).with_default_node_address("127.0.0.1:59999"),
        );
        let gw_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gw_grpc_addr = gw_listener.local_addr().unwrap();
        let gw_service = gateway_server.create_service();
        let gw_incoming = TcpListenerStream::new(gw_listener);
        let gw_grpc_handle = tokio::spawn(async move {
            let _ = Server::builder()
                .add_service(GatewayServiceServer::new(gw_service))
                .serve_with_incoming(gw_incoming)
                .await;
        });

        let gw_hb_handle = GatewayServer::spawn_heartbeat_loop_with_router(
            format!("http://{director_addr}"),
            "gw-e2e-split".to_string(),
            "us-east".to_string(),
            "127.0.0.1:8080".to_string(),
            gw_grpc_addr.to_string(),
            "127.0.0.1:8081".to_string(),
            1000,
            gateway_server.session_sinks(),
            Some(Arc::clone(&router)),
        );

        // 3. Start Node 1 and Node 2 gRPC servers
        let (node1_sim, node1_addr, node1_grpc_handle) =
            spawn_node_grpc_on(1, bounds, gw_grpc_addr).await;
        let (node2_sim, node2_addr, node2_grpc_handle) =
            spawn_node_grpc_on(2, bounds, gw_grpc_addr).await;

        // 4. Register Node 1 first and wait until it owns the root partition
        let telemetry1 = TelemetryServer::init().unwrap();
        let (fenced_tx1, _fenced_rx1) = tokio::sync::mpsc::channel::<String>(1);
        let cfg1 = DirectorHeartbeatConfig {
            node_id: 1,
            listen_addr: node1_addr,
            bounds,
            director_endpoint: format!("http://{director_addr}"),
            retry_interval: Duration::from_millis(30),
            heartbeat_interval: Duration::from_millis(40),
            rpc_timeout: Duration::from_millis(500),
            lease_timeout: None,
        };
        let node1_hb_handle = tokio::spawn(run_director_registration_and_heartbeat_loop(
            cfg1,
            Arc::clone(&node1_sim),
            telemetry1,
            fenced_tx1,
        ));

        let reg_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while registry.read().await.get_node(1).is_none() {
            assert!(
                tokio::time::Instant::now() < reg_deadline,
                "Node 1 failed to register with Director"
            );
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        assert_eq!(
            director_service
                .coordinator
                .read()
                .await
                .get_assigned_bounds_for_node(1),
            vec![bounds]
        );

        // 5. Start Node 2 with identical world bounds and assert Node 1 retains root ownership
        let telemetry2 = TelemetryServer::init().unwrap();
        let (fenced_tx2, _fenced_rx2) = tokio::sync::mpsc::channel::<String>(1);
        let cfg2 = DirectorHeartbeatConfig {
            node_id: 2,
            listen_addr: node2_addr,
            bounds,
            director_endpoint: format!("http://{director_addr}"),
            retry_interval: Duration::from_millis(30),
            heartbeat_interval: Duration::from_millis(40),
            rpc_timeout: Duration::from_millis(500),
            lease_timeout: None,
        };
        let node2_hb_handle = tokio::spawn(run_director_registration_and_heartbeat_loop(
            cfg2,
            Arc::clone(&node2_sim),
            telemetry2,
            fenced_tx2,
        ));

        let reg2_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while registry.read().await.get_node(2).is_none() {
            assert!(
                tokio::time::Instant::now() < reg2_deadline,
                "Node 2 failed to register with Director"
            );
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        {
            let coord = director_service.coordinator.read().await;
            assert_eq!(coord.topology_epoch(), 1);
            assert_eq!(coord.get_assigned_bounds_for_node(1), vec![bounds]);
            assert!(coord.get_assigned_bounds_for_node(2).is_empty());
        }

        // 6. Spawn 64 movable player entities across all 8 octant centers on Node 1
        let octants = bounds.subdivide();
        let mut spawned_players = Vec::with_capacity(64);
        {
            let mut sim1 = node1_sim.write().await;
            for (octant_idx, octant_bounds) in octants.iter().enumerate() {
                let center = octant_bounds.center();
                for k in 0..8usize {
                    let idx = (octant_idx * 8 + k) as u32;
                    let guid = Guid::new(EntityType::Player, 1, idx + 1);
                    let session_id = 1000 + u64::from(idx);
                    assert!(sim1.spawn_entity(guid, center));
                    sim1.register_aoi_observer(session_id, guid, Default::default());
                    router.register_session(session_id, guid, node1_addr.to_string());
                    spawned_players.push((guid, session_id, center));
                }
            }
        }

        // 7. Wait for split (epoch == 2), batch migration, partition sync on both nodes, and gateway routes
        let split_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let epoch = director_service.coordinator.read().await.topology_epoch();
            let (n1_cnt, n1_owned, n1_peers) = {
                let s = node1_sim.read().await;
                (
                    s.entity_store.count(),
                    s.owned_bounds.len(),
                    s.peer_partitions.len(),
                )
            };
            let (n2_cnt, n2_owned, n2_peers) = {
                let s = node2_sim.read().await;
                (
                    s.entity_store.count(),
                    s.owned_bounds.len(),
                    s.peer_partitions.len(),
                )
            };
            let routes_settled = {
                let reg = registry.read().await;
                let coord = director_service.coordinator.read().await;
                let parts = coord.collect_partitions(&reg);
                parts.len() == 8
                    && spawned_players.iter().all(|(guid, _, pos)| {
                        parts
                            .iter()
                            .find(|(b, _, _)| b.contains(*pos))
                            .and_then(|(_, nid, _)| {
                                let expected = if *nid == 1 {
                                    node1_addr.to_string()
                                } else {
                                    node2_addr.to_string()
                                };
                                router.get_route_by_guid(*guid).map(|r| r == expected)
                            })
                            .unwrap_or(false)
                    })
            };

            if epoch == 2
                && n1_cnt == 32
                && n2_cnt == 32
                && n1_owned == 4
                && n1_peers == 4
                && n2_owned == 4
                && n2_peers == 4
                && routes_settled
            {
                break;
            }

            assert!(
                tokio::time::Instant::now() < split_deadline,
                "timed out waiting for split and migration to settle: epoch={epoch}, n1=({n1_cnt},{n1_owned},{n1_peers}), n2=({n2_cnt},{n2_owned},{n2_peers}), routes_settled={routes_settled}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        Self {
            registry,
            director_service,
            router,
            gateway_server,
            node1_sim,
            node2_sim,
            node1_addr,
            node2_addr,
            spawned_players,
            dir_shutdown_tx,
            dir_handle,
            gw_grpc_handle,
            gw_hb_handle,
            node1_grpc_handle,
            node2_grpc_handle,
            node1_hb_handle,
            node2_hb_handle,
        }
    }

    async fn shutdown(self) {
        self.node1_hb_handle.abort();
        self.node2_hb_handle.abort();
        self.gw_hb_handle.abort();
        self.node1_grpc_handle.abort();
        self.node2_grpc_handle.abort();
        self.gw_grpc_handle.abort();
        let _ = self.dir_shutdown_tx.send(());
        let _ = self.dir_handle.await;
    }
}

#[tokio::test]
async fn test_multi_node_split_migrates_entities_and_prevents_split_loop() {
    let fixture = SplitClusterFixture::start_and_split().await;
    let node1_str = fixture.node1_addr.to_string();
    let node2_str = fixture.node2_addr.to_string();

    let partitions = {
        let reg = fixture.registry.read().await;
        let coord = fixture.director_service.coordinator.read().await;
        assert_eq!(coord.topology_epoch(), 2);
        coord.collect_partitions(&reg)
    };
    assert_eq!(partitions.len(), 8);
    assert_eq!(partitions.iter().filter(|(_, nid, _)| *nid == 1).count(), 4);
    assert_eq!(partitions.iter().filter(|(_, nid, _)| *nid == 2).count(), 4);

    // Verify exact entity ownership and Gateway routes with zero duplicates or losses
    let s1 = fixture.node1_sim.read().await;
    let s2 = fixture.node2_sim.read().await;
    assert_eq!(s1.entity_store.count(), 32);
    assert_eq!(s2.entity_store.count(), 32);

    let mut migrated_count = 0usize;
    let mut retained_count = 0usize;
    for (guid, session_id, pos) in &fixture.spawned_players {
        let (_, owner_id, _) = partitions
            .iter()
            .find(|(b, _, _)| b.contains(*pos))
            .expect("spawned entity position covered by partition");
        if *owner_id == 2 {
            migrated_count += 1;
            assert!(
                s2.entity_store.contains_guid(*guid),
                "migrated entity {guid:?} missing from Node 2"
            );
            assert!(
                !s1.entity_store.contains_guid(*guid),
                "migrated entity {guid:?} still present on Node 1"
            );
            assert_eq!(s2.aoi.get_session_by_guid(*guid), Some(*session_id));
            assert_eq!(
                fixture.router.get_route_by_guid(*guid).as_deref(),
                Some(node2_str.as_str())
            );
            assert_eq!(
                fixture.router.get_route_by_session(*session_id).as_deref(),
                Some(node2_str.as_str())
            );
        } else {
            retained_count += 1;
            assert_eq!(*owner_id, 1);
            assert!(
                s1.entity_store.contains_guid(*guid),
                "retained entity {guid:?} missing from Node 1"
            );
            assert!(
                !s2.entity_store.contains_guid(*guid),
                "retained entity {guid:?} duplicated on Node 2"
            );
            assert_eq!(s1.aoi.get_session_by_guid(*guid), Some(*session_id));
            assert_eq!(
                fixture.router.get_route_by_guid(*guid).as_deref(),
                Some(node1_str.as_str())
            );
            assert_eq!(
                fixture.router.get_route_by_session(*session_id).as_deref(),
                Some(node1_str.as_str())
            );
        }
    }
    assert_eq!(migrated_count, 32);
    assert_eq!(retained_count, 32);
    drop(s1);
    drop(s2);

    // Continue heartbeating for > 5 more intervals (40ms each) and verify no split loop
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(
        fixture
            .director_service
            .coordinator
            .read()
            .await
            .topology_epoch(),
        2
    );
    assert_eq!(fixture.node1_sim.read().await.entity_store.count(), 32);
    assert_eq!(fixture.node2_sim.read().await.entity_store.count(), 32);

    // Verify Gateway heartbeat syncs topology_epoch == 2 and all 8 partitions,
    // and resolve_initial_node balances new sessions across Node 1 and Node 2
    let gw_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while fixture.router.topology_epoch() < 2 || fixture.router.get_topology_partitions().len() != 8
    {
        assert!(
            tokio::time::Instant::now() < gw_deadline,
            "Gateway heartbeat did not receive split topology partitions in time"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let p1 = fixture
        .gateway_server
        .resolve_initial_node(Guid::new(EntityType::Player, 1, 9001))
        .await
        .unwrap();
    let p2 = fixture
        .gateway_server
        .resolve_initial_node(Guid::new(EntityType::Player, 1, 9002))
        .await
        .unwrap();
    let picks: std::collections::HashSet<String> = [p1, p2].into_iter().collect();
    assert_eq!(picks, [node1_str, node2_str].into_iter().collect());

    fixture.shutdown().await;
}

#[tokio::test]
async fn test_boundary_crossing_after_split_migrates_entity() {
    let fixture = SplitClusterFixture::start_and_split().await;
    let node2_str = fixture.node2_addr.to_string();

    // Octant 0 is always retained by Node 1 (x in -100..0), and Octant 1 is assigned to Node 2 (x in 0..100).
    let (mover_guid, mover_session, _) = fixture.spawned_players[0];
    assert!(fixture
        .node1_sim
        .read()
        .await
        .entity_store
        .contains_guid(mover_guid));

    // 1. Move just across X=0 within hysteresis margin (0.5 < 1.0) -> stays on Node 1
    {
        let mut s1 = fixture.node1_sim.write().await;
        let old_pos = s1.entity_store.get_by_guid(mover_guid).unwrap().position;
        let within_margin = (0.5, -50.0, -50.0);
        s1.entity_store
            .get_mut_by_guid(mover_guid)
            .unwrap()
            .position = within_margin;
        s1.octree.remove(mover_guid, old_pos);
        s1.octree.insert(mover_guid, within_margin);
        s1.step_tick();
        assert!(
            !s1.pending_migrations.contains(&mover_guid),
            "entity inside hysteresis margin must not trigger boundary migration"
        );
    }

    // 2. Move beyond hysteresis margin into Node 2's Octant 1 (1.5 >= 1.0) and step tick
    let across_boundary = (1.5, -50.0, -50.0);
    {
        let mut s1 = fixture.node1_sim.write().await;
        let old_pos = s1.entity_store.get_by_guid(mover_guid).unwrap().position;
        s1.entity_store
            .get_mut_by_guid(mover_guid)
            .unwrap()
            .position = across_boundary;
        s1.octree.remove(mover_guid, old_pos);
        s1.octree.insert(mover_guid, across_boundary);
        s1.step_tick();
        assert!(
            s1.pending_migrations.contains(&mover_guid),
            "entity crossing boundary beyond hysteresis margin must queue for migration"
        );
    }

    // 3. Wait for Node 1 heartbeat loop to flush pending boundary migration to Node 2 and Gateway
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let on_node2 = fixture
            .node2_sim
            .read()
            .await
            .entity_store
            .contains_guid(mover_guid);
        let off_node1 = !fixture
            .node1_sim
            .read()
            .await
            .entity_store
            .contains_guid(mover_guid);
        let gw_updated =
            fixture.router.get_route_by_guid(mover_guid).as_deref() == Some(node2_str.as_str());
        if on_node2 && off_node1 && gw_updated {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for boundary crossing migration (on_node2={on_node2}, off_node1={off_node1}, gw_updated={gw_updated})"
        );
        tokio::time::sleep(Duration::from_millis(15)).await;
    }

    {
        let s1 = fixture.node1_sim.read().await;
        let s2 = fixture.node2_sim.read().await;
        assert_eq!(s1.entity_store.count(), 31);
        assert_eq!(s2.entity_store.count(), 33);
        assert_eq!(
            s2.entity_store.get_by_guid(mover_guid).unwrap().position,
            across_boundary
        );
        assert_eq!(s2.aoi.get_session_by_guid(mover_guid), Some(mover_session));
    }
    assert_eq!(
        fixture
            .router
            .get_route_by_session(mover_session)
            .as_deref(),
        Some(node2_str.as_str())
    );
    assert_eq!(
        fixture
            .director_service
            .coordinator
            .read()
            .await
            .topology_epoch(),
        2
    );

    fixture.shutdown().await;
}
