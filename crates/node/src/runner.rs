use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tonic::transport::Server;

use mmo_core::BoundingBox3D;
use mmo_schema::cluster::director_service_client::DirectorServiceClient;
use mmo_schema::cluster::gateway_service_client::GatewayServiceClient;
use mmo_schema::cluster::node_service_server::NodeServiceServer;
use mmo_schema::cluster::{BoundingBox, HeartbeatRequest, RegisterNodeRequest, Vector3};
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

    let sim = Arc::new(RwLock::new(tick_sim));

    // 1. Start gRPC NodeService server with graceful shutdown support
    let (grpc_shutdown_tx, grpc_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let node_service = NodeServiceImpl::new(Arc::clone(&sim));
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
    let director_endpoint = director_addr.clone();
    let sim_clone = Arc::clone(&sim);
    let telemetry_clone = telemetry.clone();
    let heartbeat_handle = tokio::spawn(async move {
        println!("[Node {node_id}] Attempting connection to Director at {director_endpoint}...");
        let mut client = loop {
            match DirectorServiceClient::connect(director_endpoint.clone()).await {
                Ok(c) => {
                    println!("[Node {node_id}] Connected to Director.");
                    break c;
                }
                Err(_) => {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        };

        // Register node
        let proto_bounds = Some(BoundingBox {
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
        });

        let reg_req = RegisterNodeRequest {
            node_id: u32::from(node_id),
            listen_addr: listen_addr.to_string(),
            bounds: proto_bounds,
        };

        match client.register_node(reg_req).await {
            Ok(resp) => {
                let success = resp.into_inner().success;
                println!("[Node {node_id}] Registered with Director: success={success}");
                if success {
                    telemetry_clone.mark_ready();
                }
            }
            Err(e) => {
                eprintln!("[Node {node_id}] Failed to register with Director: {e}");
            }
        }

        // Heartbeat loop
        let mut hb_interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            hb_interval.tick().await;
            let (entity_count, tick_duration_ms, entity_centroid) = {
                let s = sim_clone.read().await;
                let count = s.entity_store.count() as u32;
                let dur = s.last_tick_duration_ms();
                let centroid = if count > 0 {
                    let c = s.compute_clamped_centroid(&s.octree.bounds);
                    Some(Vector3 {
                        x: c.0,
                        y: c.1,
                        z: c.2,
                    })
                } else {
                    None
                };
                (count, dur, centroid)
            };

            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);

            let hb = HeartbeatRequest {
                node_id: u32::from(node_id),
                tick_rate: 20.0,
                entity_count,
                cpu_percent: 5.0,
                memory_bytes: 10_000_000,
                timestamp: now,
                tick_duration_ms,
                entity_centroid,
            };

            match client.heartbeat(hb).await {
                Ok(resp) => {
                    let inner = resp.into_inner();
                    if !inner.acknowledged {
                        let reg_req = RegisterNodeRequest {
                            node_id: u32::from(node_id),
                            listen_addr: listen_addr.to_string(),
                            bounds: proto_bounds,
                        };
                        if let Ok(reg_resp) = client.register_node(reg_req).await {
                            println!(
                                "[Node {node_id}] Re-registered with Director after unacknowledged heartbeat: success={}",
                                reg_resp.into_inner().success
                            );
                        }
                    } else if inner.should_split {
                        let proto_bounds = Some(BoundingBox {
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
                        });
                        let split_req = mmo_schema::cluster::SplitReportRequest {
                            node_id: u32::from(node_id),
                            original_bounds: proto_bounds,
                            child_bounds: vec![],
                            total_entities: entity_count,
                        };
                        if let Ok(split_resp) = client.report_split(split_req).await {
                            println!(
                                "[Node {node_id}] Split reported, assigned nodes: {:?}",
                                split_resp.into_inner().assigned_node_ids
                            );
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[Node {node_id}] Heartbeat send failed: {e}");
                }
            }
        }
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
                                if let Some(obs) = sim_guard.unregister_aoi_observer(sid) {
                                    sim_guard.entity_store.despawn_by_guid(obs.player_guid);
                                    sim_guard.octree.remove_by_guid(obs.player_guid);
                                }
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
                _ = tokio::time::sleep_until(next_tick) => {}
            }
        } else {
            tokio::select! {
                biased;
                _ = &mut shutdown_fut => {
                    println!("[Node {node_id}] Shutdown signal received, initiating drain...");
                    break;
                }
                _ = std::future::ready(()) => {}
            }
        }

        let mut catchup = 0;
        while tokio::time::Instant::now() >= next_tick && catchup < MAX_CATCHUP_TICKS {
            let mut sim_guard = sim.write().await;
            sim_guard.step_tick();
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

        // Clamp residual lag to prevent spiral of death
        let now_after = tokio::time::Instant::now();
        if now_after >= next_tick {
            next_tick = now_after + tick_duration;
        }
    }

    drop(broadcast_tx);
    let _ = tokio::time::timeout(Duration::from_secs(1), broadcast_handle).await;

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
