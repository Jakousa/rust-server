use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tonic::transport::Server;

use mmo_core::BoundingBox3D;
use mmo_schema::cluster::director_service_client::DirectorServiceClient;
use mmo_schema::cluster::gateway_service_client::GatewayServiceClient;
use mmo_schema::cluster::node_service_server::NodeServiceServer;
use mmo_schema::cluster::{
    BoundingBox, HeartbeatRequest, PartitionAssignment, RegisterNodeRequest, Vector3,
};
use mmo_storage::db::Database;
use mmo_storage::repository::CharacterRepository;

use crate::network::NodeServiceImpl;
use crate::sim::TickSimulation;
use crate::snapshotter::{drain_and_shutdown_node, WriteBehindSnapshotter};

/// Reusable entry point running an authoritative simulation node daemon with a custom pre-initialized simulation.
pub async fn run_node_daemon_with_sim(
    node_id: u16,
    bounds: BoundingBox3D,
    mut tick_sim: TickSimulation,
) -> Result<(), Box<dyn std::error::Error>> {
    let listen_addr: SocketAddr = std::env::var("LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:50052".to_string())
        .parse()?;

    let admin_addr: SocketAddr = std::env::var("ADMIN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:9091".to_string())
        .parse()?;

    let director_addr =
        std::env::var("DIRECTOR_ADDR").unwrap_or_else(|_| "http://127.0.0.1:50051".to_string());

    let gateway_grpc_addr =
        std::env::var("GATEWAY_GRPC_ADDR").unwrap_or_else(|_| "http://127.0.0.1:50050".to_string());

    let db_url = std::env::var("DATABASE_URL").ok();

    println!("============================================================");
    println!("        MMO Simulation Node Daemon Initialized             ");
    println!("============================================================");
    println!("Node ID          : {}", node_id);
    println!("Listen Address   : {}", listen_addr);
    println!("Admin Address    : {}", admin_addr);
    println!("Director Address : {}", director_addr);
    println!("Gateway Address  : {}", gateway_grpc_addr);
    println!(
        "Database URL     : {}",
        db_url
            .as_deref()
            .unwrap_or("Disabled (DATABASE_URL not set)")
    );
    println!("------------------------------------------------------------");

    let telemetry = mmo_core::TelemetryServer::init()?;
    telemetry.spawn(admin_addr);

    let (snapshotter, snapshot_worker_handle) = if let Some(ref url) = db_url {
        if tick_sim.snapshot_sender.is_none() {
            match Database::connect(url).await {
                Ok(db) => {
                    println!("[Node {}] Connected to database at {}", node_id, url);
                    let repo = CharacterRepository::new(db);
                    let snapshotter = Arc::new(WriteBehindSnapshotter::new(repo));
                    tick_sim.set_snapshot_sender(snapshotter.sender());
                    let handle = snapshotter.clone().start_worker();
                    println!(
                        "[Node {}] Asynchronous write-behind snapshot worker active",
                        node_id
                    );
                    (Some(snapshotter), Some(handle))
                }
                Err(e) => {
                    eprintln!("[Node {}] Database connection failed: {}", node_id, e);
                    (None, None)
                }
            }
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    tick_sim.gateway_grpc_addr = gateway_grpc_addr.clone();
    let input_tx = tick_sim.input_sender();
    let ghost_store = tick_sim.ghost_store.clone();
    let sim = Arc::new(RwLock::new(tick_sim));

    let (migration_tx, mut migration_rx) = tokio::sync::mpsc::channel::<()>(16);
    let migration_sim = Arc::clone(&sim);
    let migration_handle = tokio::spawn(async move {
        let pool = crate::network::GrpcClientPool::new();
        let self_node_id = u32::from(node_id);
        while migration_rx.recv().await.is_some() {
            while migration_rx.try_recv().is_ok() {}
            flush_pending_boundary_migrations(self_node_id, &migration_sim, &pool).await;
        }
    });

    // 1. Start gRPC NodeService server with graceful shutdown support
    let (grpc_shutdown_tx, grpc_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let node_service = NodeServiceImpl::new(Arc::clone(&sim), input_tx, ghost_store);
    let grpc_handle = tokio::spawn(async move {
        println!(
            "[Node {}] gRPC service listening on {}",
            node_id, listen_addr
        );
        if let Err(e) = Server::builder()
            .add_service(NodeServiceServer::new(node_service))
            .serve_with_shutdown(listen_addr, async move {
                let _ = grpc_shutdown_rx.await;
            })
            .await
        {
            eprintln!("[Node {node_id}] gRPC Server failed: {e}");
        }
    });

    // 2. Connect and register with Director (with retry backoff)
    let (fenced_tx, mut fenced_rx) = tokio::sync::mpsc::channel::<String>(1);
    let lease_timeout = std::env::var("DIRECTOR_LEASE_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&ms| ms > 0)
        .map(Duration::from_millis);
    let hb_config = DirectorHeartbeatConfig {
        node_id,
        listen_addr,
        bounds,
        director_endpoint: director_addr.clone(),
        retry_interval: Duration::from_secs(1),
        heartbeat_interval: Duration::from_secs(1),
        rpc_timeout: Duration::from_secs(2),
        lease_timeout,
    };
    let sim_clone = Arc::clone(&sim);
    let telemetry_clone = telemetry.clone();
    let heartbeat_handle = tokio::spawn(async move {
        run_director_registration_and_heartbeat_loop(
            hb_config,
            sim_clone,
            telemetry_clone,
            fenced_tx,
        )
        .await;
    });

    // 3. AoI Broadcast Delivery Decoupler & Gateway Worker
    let (broadcast_tx, mut broadcast_rx) =
        tokio::sync::mpsc::channel::<Vec<crate::aoi::AoiBroadcastJob>>(128);
    let sim_for_cleanup = Arc::clone(&sim);
    let gw_endpoint = gateway_grpc_addr.clone();
    let broadcast_handle = tokio::spawn(async move {
        println!("[Node {node_id}] AoI broadcast delivery worker active (Gateway: {gw_endpoint})");
        let mut client_opt = None;
        while let Some(jobs) = broadcast_rx.recv().await {
            if client_opt.is_none() {
                client_opt = GatewayServiceClient::connect(gw_endpoint.clone())
                    .await
                    .ok();
            }

            if let Some(ref mut client) = client_opt {
                let session_batches = jobs
                    .into_iter()
                    .map(|job| {
                        let packets = job.packets.into_iter().map(|p| p.encode()).collect();
                        mmo_schema::cluster::SessionPacketBatch {
                            session_id: job.session_id,
                            packets,
                        }
                    })
                    .collect();

                let req = mmo_schema::cluster::DeliverBroadcastJobsRequest {
                    jobs: session_batches,
                };

                match client.deliver_broadcast_jobs(req).await {
                    Ok(resp) => {
                        let inner = resp.into_inner();
                        if !inner.disconnected_sessions.is_empty() {
                            let mut sim_guard = sim_for_cleanup.write().await;
                            for sid in inner.disconnected_sessions {
                                sim_guard.disconnect_session_by_id(sid);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "[Node {node_id}] Failed delivering broadcast jobs to Gateway: {e}"
                        );
                        client_opt = None;
                    }
                }
            } else {
                metrics::counter!("mmo_node_broadcast_jobs_gateway_unreachable_total").increment(1);
            }
        }
    });

    // 4. Fixed 20 Hz (50ms) simulation game loop with clamped tick accumulator
    println!("[Node {node_id}] Starting 20 Hz simulation tick loop...");
    let tick_duration = Duration::from_millis(50);
    const MAX_CATCHUP_TICKS: usize = 3;
    let mut next_tick = tokio::time::Instant::now() + tick_duration;
    let shutdown_fut = mmo_core::shutdown_signal();
    tokio::pin!(shutdown_fut);

    loop {
        let now = tokio::time::Instant::now();
        if now < next_tick {
            tokio::select! {
                _ = &mut shutdown_fut => {
                    println!("[Node {node_id}] Shutdown signal received, initiating drain...");
                    break;
                }
                reason = fenced_rx.recv() => {
                    println!("[Node {node_id}] Simulation halted due to node fencing: {:?}", reason);
                    break;
                }
                _ = tokio::time::sleep_until(next_tick) => {}
            }
        } else {
            tokio::select! {
                biased;
                _ = &mut shutdown_fut => {
                    println!("[Node {node_id}] Shutdown signal received, initiating drain...");
                    break;
                }
                reason = fenced_rx.recv() => {
                    println!("[Node {node_id}] Simulation halted due to node fencing: {:?}", reason);
                    break;
                }
                _ = std::future::ready(()) => {}
            }
        }

        let mut catchup = 0;
        let mut notify_migration = false;
        while tokio::time::Instant::now() >= next_tick && catchup < MAX_CATCHUP_TICKS {
            let mut sim_guard = sim.write().await;
            sim_guard.step_tick();
            if !sim_guard.pending_migrations.is_empty() {
                notify_migration = true;
            }
            let jobs = sim_guard.drain_broadcast_jobs();
            if !jobs.is_empty() {
                if let Err(tokio::sync::mpsc::error::TrySendError::Full(_)) =
                    broadcast_tx.try_send(jobs)
                {
                    metrics::counter!("mmo_node_broadcast_jobs_dropped_backpressure").increment(1);
                }
            }
            catchup += 1;
            next_tick += tick_duration;
        }
        if notify_migration {
            let _ = migration_tx.try_send(());
        }

        // Clamp residual lag to prevent spiral of death
        let now_after = tokio::time::Instant::now();
        if now_after >= next_tick {
            next_tick = now_after + tick_duration;
        }
    }

    drop(broadcast_tx);
    drop(migration_tx);
    let _ = tokio::time::timeout(Duration::from_secs(1), broadcast_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(1), migration_handle).await;

    drain_and_shutdown_node(
        node_id,
        &telemetry,
        heartbeat_handle,
        snapshotter,
        snapshot_worker_handle,
        &director_addr,
        grpc_shutdown_tx,
    )
    .await;

    let _ = tokio::time::timeout(Duration::from_secs(2), grpc_handle).await;
    println!("[Node {node_id}] Clean shutdown complete.");
    Ok(())
}

/// Configuration for the simulation node's Director registration and heartbeat loop.
#[derive(Debug, Clone)]
pub struct DirectorHeartbeatConfig {
    pub node_id: u16,
    pub listen_addr: SocketAddr,
    pub bounds: BoundingBox3D,
    pub director_endpoint: String,
    pub retry_interval: Duration,
    pub heartbeat_interval: Duration,
    pub rpc_timeout: Duration,
    pub lease_timeout: Option<Duration>,
}

fn normalize_endpoint(endpoint: &str) -> String {
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        endpoint.to_string()
    } else {
        format!("http://{endpoint}")
    }
}

/// Resolves the dialable gRPC node address advertised to the Director during registration.
///
/// Honors explicit `ADVERTISE_ADDR` overrides, derives Kubernetes StatefulSet DNS names from
/// `HOSTNAME` (`<hostname>.mmo-node:<port>`) when bound to an unspecified address (`0.0.0.0`/`::`),
/// and otherwise returns `listen_addr` verbatim.
pub fn resolve_advertise_addr(listen_addr: SocketAddr) -> String {
    let advertise_addr = std::env::var("ADVERTISE_ADDR").ok();
    let hostname = std::env::var("HOSTNAME").ok();
    resolve_advertise_addr_with(listen_addr, advertise_addr.as_deref(), hostname.as_deref())
}

fn resolve_advertise_addr_with(
    listen_addr: SocketAddr,
    advertise_addr: Option<&str>,
    hostname: Option<&str>,
) -> String {
    if let Some(addr) = advertise_addr.map(str::trim).filter(|s| !s.is_empty()) {
        return addr.to_string();
    }
    if listen_addr.ip().is_unspecified() {
        if let Some(host) = hostname.map(str::trim).filter(|h| !h.is_empty()) {
            if host.starts_with("mmo-node-") {
                return format!("{host}.mmo-node:{}", listen_addr.port());
            }
        }
    }
    listen_addr.to_string()
}

fn proto_bbox_to_bounding_box_3d(bbox: &BoundingBox) -> Option<BoundingBox3D> {
    let min = bbox.min.as_ref()?;
    let max = bbox.max.as_ref()?;
    if !(min.x.is_finite()
        && min.y.is_finite()
        && min.z.is_finite()
        && max.x.is_finite()
        && max.y.is_finite()
        && max.z.is_finite())
    {
        return None;
    }
    if min.x > max.x || min.y > max.y || min.z > max.z {
        return None;
    }
    Some(BoundingBox3D::new(
        (min.x, min.y, min.z),
        (max.x, max.y, max.z),
    ))
}

fn parse_partitions(assignments: &[PartitionAssignment]) -> Vec<(BoundingBox3D, u32, String)> {
    assignments
        .iter()
        .filter_map(|p| {
            let bounds = p.bounds.as_ref().and_then(proto_bbox_to_bounding_box_3d)?;
            Some((bounds, p.node_id, p.node_address.trim().to_string()))
        })
        .collect()
}

fn build_split_partitions(
    split_inner: &mmo_schema::cluster::SplitReportResponse,
    current_bounds: &BoundingBox3D,
) -> Vec<(BoundingBox3D, u32, String)> {
    let parsed = parse_partitions(&split_inner.partitions);
    if !parsed.is_empty() {
        return parsed;
    }

    let children = current_bounds.subdivide();
    children
        .into_iter()
        .zip(split_inner.assigned_node_ids.iter().copied())
        .enumerate()
        .map(|(idx, (bounds, nid))| {
            let addr = split_inner
                .assigned_node_addresses
                .get(idx)
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            (bounds, nid, addr)
        })
        .collect()
}

const MAX_ENTITIES_PER_MIGRATION_RPC: usize = 500;

async fn dispatch_migration_batches(
    self_node_id: u32,
    batches: std::collections::HashMap<(u32, String), Vec<mmo_core::Guid>>,
    gateway_addr: String,
    sim: &Arc<RwLock<TickSimulation>>,
    pool: &crate::network::GrpcClientPool,
) {
    let normalized_gw =
        (!gateway_addr.trim().is_empty()).then(|| normalize_endpoint(gateway_addr.trim()));
    let gw_ref = normalized_gw.as_deref();

    for ((target_node_id, target_addr), guids) in batches {
        let trimmed_addr = target_addr.trim();
        if target_node_id == self_node_id || trimmed_addr.is_empty() || guids.is_empty() {
            continue;
        }

        for chunk in guids.chunks(MAX_ENTITIES_PER_MIGRATION_RPC) {
            match crate::network::execute_batch_entity_migration_pooled(
                pool,
                Arc::clone(sim),
                chunk,
                target_node_id,
                trimmed_addr,
                gw_ref,
            )
            .await
            {
                Ok(report) => {
                    println!(
                        "[Node {self_node_id}] Migrated {}/{} entities to node {target_node_id} ({trimmed_addr}) [success={}]",
                        report.migrated_count,
                        report.total_requested,
                        report.success
                    );
                }
                Err(err) => {
                    eprintln!(
                        "[Node {self_node_id}] Migration batch ({} entities) to node {target_node_id} ({trimmed_addr}) failed: {err}",
                        chunk.len()
                    );
                }
            }
        }
    }
}

async fn apply_partitions_and_migrate(
    self_node_id: u32,
    partitions: &[(BoundingBox3D, u32, String)],
    sim: &Arc<RwLock<TickSimulation>>,
    pool: &crate::network::GrpcClientPool,
    current_bounds: &mut BoundingBox3D,
) {
    if partitions.is_empty() {
        return;
    }

    let (batches, gateway_addr) = {
        let mut s = sim.write().await;
        s.apply_topology_partitions(partitions);
        if let Some(&first_owned) = s.owned_bounds.first() {
            *current_bounds = first_owned;
        }
        (
            s.collect_entities_for_peer_partitions(),
            s.gateway_grpc_addr.clone(),
        )
    };

    dispatch_migration_batches(self_node_id, batches, gateway_addr, sim, pool).await;
}

async fn flush_pending_boundary_migrations(
    self_node_id: u32,
    sim: &Arc<RwLock<TickSimulation>>,
    pool: &crate::network::GrpcClientPool,
) {
    let (batches, gateway_addr) = {
        let mut s = sim.write().await;
        if s.pending_migrations.is_empty() {
            return;
        }
        (
            s.drain_boundary_migrations_by_peer(),
            s.gateway_grpc_addr.clone(),
        )
    };

    dispatch_migration_batches(self_node_id, batches, gateway_addr, sim, pool).await;
}

fn build_bounds_proto(bounds: &BoundingBox3D) -> Option<BoundingBox> {
    Some(BoundingBox {
        min: Some(Vector3 {
            x: bounds.min.0,
            y: bounds.min.1,
            z: bounds.min.2,
        }),
        max: Some(Vector3 {
            x: bounds.max.0,
            y: bounds.max.1,
            z: bounds.max.2,
        }),
    })
}

fn is_transient_transport_error(status: &tonic::Status) -> bool {
    matches!(
        status.code(),
        tonic::Code::Unavailable
            | tonic::Code::Unknown
            | tonic::Code::DeadlineExceeded
            | tonic::Code::Cancelled
    )
}

fn read_process_rss_bytes() -> u64 {
    #[cfg(target_os = "linux")]
    {
        if let Ok(statm) = std::fs::read_to_string("/proc/self/statm") {
            if let Some(rss_pages_str) = statm.split_whitespace().nth(1) {
                if let Ok(rss_pages) = rss_pages_str.parse::<u64>() {
                    return rss_pages.saturating_mul(4096);
                }
            }
        }
    }
    0
}

fn next_retry_delay(
    retry_interval: Duration,
    lease: Option<(std::time::Instant, Duration)>,
) -> Option<Duration> {
    match lease {
        Some((last_ack, lease_timeout)) => {
            let remaining = lease_timeout.saturating_sub(last_ack.elapsed());
            if remaining.is_zero() {
                None
            } else {
                Some(retry_interval.min(remaining))
            }
        }
        None => Some(retry_interval),
    }
}

async fn register_until_acknowledged(
    client: &mut Option<DirectorServiceClient<tonic::transport::Channel>>,
    endpoint: &str,
    config: &DirectorHeartbeatConfig,
    reg_req: &RegisterNodeRequest,
    telemetry: &mmo_core::TelemetryServer,
    last_ack: Option<std::time::Instant>,
) -> Option<std::time::Instant> {
    let node_id = config.node_id;
    let lease = last_ack.zip(config.lease_timeout);

    loop {
        if let Some((ack_at, lease_timeout)) = lease {
            if ack_at.elapsed() >= lease_timeout {
                telemetry.mark_unready();
                return None;
            }
        }

        if client.is_none() {
            match tokio::time::timeout(
                config.rpc_timeout,
                DirectorServiceClient::connect(endpoint.to_string()),
            )
            .await
            {
                Ok(Ok(c)) => {
                    println!("[Node {node_id}] Connected to Director.");
                    *client = Some(c);
                }
                Ok(Err(_)) | Err(_) => {
                    let Some(sleep_dur) = next_retry_delay(config.retry_interval, lease) else {
                        telemetry.mark_unready();
                        return None;
                    };
                    tokio::time::sleep(sleep_dur).await;
                    continue;
                }
            }
        }

        let Some(ref mut c) = client else {
            continue;
        };

        match tokio::time::timeout(config.rpc_timeout, c.register_node(reg_req.clone())).await {
            Ok(Ok(resp)) => {
                let inner = resp.into_inner();
                println!(
                    "[Node {node_id}] Registered with Director: success={}",
                    inner.success
                );
                if inner.success {
                    telemetry.mark_ready();
                    return Some(std::time::Instant::now());
                }
                eprintln!(
                    "[Node {node_id}] Director rejected registration: {}",
                    inner.message
                );
            }
            Ok(Err(e)) => {
                eprintln!("[Node {node_id}] Failed to register with Director: {e}");
                if is_transient_transport_error(&e) {
                    *client = None;
                }
            }
            Err(_) => {
                eprintln!("[Node {node_id}] Director register_node timed out");
                *client = None;
            }
        }

        let Some(sleep_dur) = next_retry_delay(config.retry_interval, lease) else {
            telemetry.mark_unready();
            return None;
        };
        tokio::time::sleep(sleep_dur).await;
    }
}

/// Runs the resilient Director registration and heartbeat loop with automatic retry on startup
/// and self-healing re-registration if Director restarts and loses in-memory state.
pub async fn run_director_registration_and_heartbeat_loop(
    config: DirectorHeartbeatConfig,
    sim: Arc<RwLock<TickSimulation>>,
    telemetry: mmo_core::TelemetryServer,
    fenced_tx: tokio::sync::mpsc::Sender<String>,
) {
    let node_id = config.node_id;
    let endpoint = normalize_endpoint(&config.director_endpoint);
    println!("[Node {node_id}] Attempting connection to Director at {endpoint}...");

    let mut current_bounds = config.bounds;
    let advertise_addr = resolve_advertise_addr(config.listen_addr);
    let initial_reg_req = RegisterNodeRequest {
        node_id: u32::from(node_id),
        listen_addr: advertise_addr.clone(),
        bounds: build_bounds_proto(&current_bounds),
    };

    let mut client: Option<DirectorServiceClient<tonic::transport::Channel>> = None;
    let mut last_ack = register_until_acknowledged(
        &mut client,
        &endpoint,
        &config,
        &initial_reg_req,
        &telemetry,
        None,
    )
    .await
    .expect("initial registration without lease timeout cannot expire");

    let mut prev_tick_counter = sim.read().await.tick_counter;
    let mut prev_hb_at = std::time::Instant::now();
    let mut rolling_tick_rate: Option<f32> = None;
    let migration_pool = crate::network::GrpcClientPool::new();
    let mut last_applied_partitions: Vec<(BoundingBox3D, u32, String)> = Vec::new();

    let mut hb_interval = tokio::time::interval(config.heartbeat_interval);
    hb_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        hb_interval.tick().await;

        if let Some(limit) = config.lease_timeout {
            let elapsed = last_ack.elapsed();
            if elapsed > limit {
                eprintln!(
                    "[Node {node_id}] Director heartbeat lease expired ({elapsed:?} > {limit:?}). Self-fencing simulation."
                );
                telemetry.mark_unready();
                let _ = fenced_tx
                    .send("Director heartbeat lease expired".to_string())
                    .await;
                break;
            }
        }

        if client.is_none() {
            match tokio::time::timeout(
                config.rpc_timeout,
                DirectorServiceClient::connect(endpoint.clone()),
            )
            .await
            {
                Ok(Ok(c)) => {
                    client = Some(c);
                }
                Ok(Err(_)) | Err(_) => {
                    continue;
                }
            }
        }

        let (entity_count, tick_duration_ms, entity_centroid, curr_tick_counter, active_instances) = {
            let s = sim.read().await;
            let count = s.owned_entity_count();
            let dur = s.last_tick_duration_ms();
            let centroid = if count > 0 {
                let c = s.compute_clamped_centroid(&current_bounds);
                Some(Vector3 {
                    x: c.0,
                    y: c.1,
                    z: c.2,
                })
            } else {
                None
            };
            let ticks = s.tick_counter;
            let instances = s.instances.active_instance_heartbeats();
            (count, dur, centroid, ticks, instances)
        };

        let now_instant = std::time::Instant::now();
        let elapsed_secs = now_instant.duration_since(prev_hb_at).as_secs_f32();
        let delta_ticks = curr_tick_counter.saturating_sub(prev_tick_counter);
        let instant_tps = if elapsed_secs > 1e-3 {
            delta_ticks as f32 / elapsed_secs
        } else {
            0.0
        };
        let tick_rate = match rolling_tick_rate {
            Some(prev) => 0.5 * prev + 0.5 * instant_tps,
            None => instant_tps,
        };
        rolling_tick_rate = Some(tick_rate);
        prev_tick_counter = curr_tick_counter;
        prev_hb_at = now_instant;

        let tick_budget_ms = TickSimulation::TICK_INTERVAL.as_secs_f32() * 1000.0;
        let cpu_percent = if tick_budget_ms > 0.0 {
            (tick_duration_ms / tick_budget_ms * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };
        let memory_bytes = read_process_rss_bytes();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let hb = HeartbeatRequest {
            node_id: u32::from(node_id),
            tick_rate,
            entity_count,
            cpu_percent,
            memory_bytes,
            timestamp: now,
            tick_duration_ms,
            entity_centroid,
        };

        let Some(ref mut c) = client else {
            continue;
        };

        match tokio::time::timeout(config.rpc_timeout, c.heartbeat(hb)).await {
            Ok(Ok(resp)) => {
                let inner = resp.into_inner();
                if !inner.acknowledged {
                    eprintln!(
                        "[Node {node_id}] Director did not acknowledge heartbeat (director may have restarted); re-registering..."
                    );
                    let rereg_req = RegisterNodeRequest {
                        node_id: u32::from(node_id),
                        listen_addr: advertise_addr.clone(),
                        bounds: build_bounds_proto(&current_bounds),
                    };
                    match register_until_acknowledged(
                        &mut client,
                        &endpoint,
                        &config,
                        &rereg_req,
                        &telemetry,
                        Some(last_ack),
                    )
                    .await
                    {
                        Some(ack_time) => {
                            last_ack = ack_time;
                            continue;
                        }
                        None => {
                            eprintln!(
                                "[Node {node_id}] Director heartbeat lease expired during re-registration. Self-fencing simulation."
                            );
                            telemetry.mark_unready();
                            let _ = fenced_tx
                                .send("Director heartbeat lease expired".to_string())
                                .await;
                            break;
                        }
                    }
                }

                last_ack = std::time::Instant::now();

                if !inner.partitions.is_empty() {
                    let incoming_partitions = parse_partitions(&inner.partitions);
                    if !incoming_partitions.is_empty() {
                        let should_apply = {
                            let s = sim.read().await;
                            incoming_partitions != last_applied_partitions
                                || !s.collect_entities_for_peer_partitions().is_empty()
                        };
                        if should_apply {
                            apply_partitions_and_migrate(
                                u32::from(node_id),
                                &incoming_partitions,
                                &sim,
                                &migration_pool,
                                &mut current_bounds,
                            )
                            .await;
                            last_applied_partitions = incoming_partitions;
                        }
                    }
                }

                for (instance_id, player_count, total_entity_count, is_idle) in active_instances {
                    let inst_hb = mmo_schema::cluster::InstanceHeartbeatRequest {
                        instance_id,
                        node_id: u32::from(node_id),
                        player_count,
                        total_entity_count,
                        is_idle,
                    };
                    if let Ok(Ok(inst_resp)) =
                        tokio::time::timeout(config.rpc_timeout, c.instance_heartbeat(inst_hb))
                            .await
                    {
                        if inst_resp.into_inner().should_terminate {
                            let mut s = sim.write().await;
                            let _ = s.instances.teardown_instance(instance_id);
                        }
                    }
                }

                if inner.should_split {
                    let split_req = mmo_schema::cluster::SplitReportRequest {
                        node_id: u32::from(node_id),
                        original_bounds: build_bounds_proto(&current_bounds),
                        child_bounds: vec![],
                        total_entities: entity_count,
                    };
                    if let Ok(Ok(split_resp)) =
                        tokio::time::timeout(config.rpc_timeout, c.report_split(split_req)).await
                    {
                        let split_inner = split_resp.into_inner();
                        println!(
                            "[Node {node_id}] Split reported, assigned nodes: {:?}",
                            split_inner.assigned_node_ids
                        );
                        if split_inner.acknowledged {
                            let split_partitions =
                                build_split_partitions(&split_inner, &current_bounds);
                            if !split_partitions.is_empty() {
                                apply_partitions_and_migrate(
                                    u32::from(node_id),
                                    &split_partitions,
                                    &sim,
                                    &migration_pool,
                                    &mut current_bounds,
                                )
                                .await;
                                last_applied_partitions = split_partitions;
                            }
                        }
                    }
                }

                if !sim.read().await.pending_migrations.is_empty() {
                    flush_pending_boundary_migrations(u32::from(node_id), &sim, &migration_pool)
                        .await;
                }
            }
            Ok(Err(e)) => {
                eprintln!("[Node {node_id}] Heartbeat send failed: {e}");
                if e.code() == tonic::Code::FailedPrecondition {
                    eprintln!(
                        "[Node {node_id}] Fenced by Director (split-brain prevention): {e}. Halting simulation."
                    );
                    telemetry.mark_unready();
                    let _ = fenced_tx.send(e.to_string()).await;
                    break;
                }
                if is_transient_transport_error(&e) {
                    client = None;
                }
            }
            Err(_) => {
                eprintln!("[Node {node_id}] Heartbeat RPC timed out");
                client = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_advertise_addr_with_env_and_statefulset_hostname() {
        let wildcard: SocketAddr = "0.0.0.0:50052".parse().expect("valid wildcard addr");
        let explicit: SocketAddr = "10.1.2.3:50052".parse().expect("valid explicit addr");

        // Explicit ADVERTISE_ADDR wins over everything
        assert_eq!(
            resolve_advertise_addr_with(wildcard, Some("  custom-node:6000  "), Some("mmo-node-0")),
            "custom-node:6000"
        );
        // Wildcard listen_addr with StatefulSet hostname synthesizes headless DNS name
        assert_eq!(
            resolve_advertise_addr_with(wildcard, Some("   "), Some("mmo-node-1")),
            "mmo-node-1.mmo-node:50052"
        );
        // Wildcard listen_addr with non-statefulset hostname falls back to listen_addr
        assert_eq!(
            resolve_advertise_addr_with(wildcard, None, Some("dev-laptop")),
            "0.0.0.0:50052"
        );
        // Non-unspecified listen_addr ignores StatefulSet hostname
        assert_eq!(
            resolve_advertise_addr_with(explicit, None, Some("mmo-node-0")),
            "10.1.2.3:50052"
        );
    }

    #[test]
    fn test_parse_partitions_and_build_split_partitions_fallback() {
        let b1 = BoundingBox3D::new((-100.0, -100.0, -100.0), (0.0, 100.0, 100.0));
        let b2 = BoundingBox3D::new((0.0, -100.0, -100.0), (100.0, 100.0, 100.0));

        let proto = vec![
            PartitionAssignment {
                bounds: build_bounds_proto(&b1),
                node_id: 1,
                node_address: " mmo-node-0.mmo-node:50052 ".to_string(),
            },
            PartitionAssignment {
                bounds: Some(BoundingBox {
                    min: Some(Vector3 {
                        x: f32::NAN,
                        y: 0.0,
                        z: 0.0,
                    }),
                    max: Some(Vector3 {
                        x: 1.0,
                        y: 1.0,
                        z: 1.0,
                    }),
                }),
                node_id: 99,
                node_address: "invalid".to_string(),
            },
            PartitionAssignment {
                bounds: build_bounds_proto(&b2),
                node_id: 2,
                node_address: "mmo-node-1.mmo-node:50052".to_string(),
            },
        ];
        let parsed = parse_partitions(&proto);
        assert_eq!(
            parsed,
            vec![
                (b1, 1, "mmo-node-0.mmo-node:50052".to_string()),
                (b2, 2, "mmo-node-1.mmo-node:50052".to_string()),
            ]
        );

        let full = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
        let fallback_resp = mmo_schema::cluster::SplitReportResponse {
            acknowledged: true,
            assigned_node_ids: vec![1, 2],
            topology_epoch: 1,
            assigned_node_addresses: vec![
                "127.0.0.1:50051".to_string(),
                "127.0.0.1:50052".to_string(),
            ],
            partitions: vec![],
        };
        let built = build_split_partitions(&fallback_resp, &full);
        assert_eq!(built.len(), 2);
        assert_eq!(built[0].1, 1);
        assert_eq!(built[0].2, "127.0.0.1:50051");
        assert_eq!(built[1].1, 2);
        assert_eq!(built[1].2, "127.0.0.1:50052");
    }
}
