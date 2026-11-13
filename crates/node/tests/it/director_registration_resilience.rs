use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::{oneshot, RwLock};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

use mmo_core::{BoundingBox3D, TelemetryServer};
use mmo_director::coordinator::GlobalOctreeCoordinator;
use mmo_director::registry::NodeRegistry;
use mmo_director::service::DirectorServiceImpl;
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
