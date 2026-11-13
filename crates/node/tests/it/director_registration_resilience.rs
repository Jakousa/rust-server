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
