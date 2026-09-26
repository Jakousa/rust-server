use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tonic::transport::Server;

use mmo_core::BoundingBox3D;
use mmo_node::{NodeServiceImpl, TickSimulation, WriteBehindSnapshotter};
use mmo_schema::cluster::director_service_client::DirectorServiceClient;
use mmo_schema::cluster::node_service_server::NodeServiceServer;
use mmo_schema::cluster::{BoundingBox, HeartbeatRequest, RegisterNodeRequest, Vector3};
use mmo_storage::db::Database;
use mmo_storage::repository::CharacterRepository;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let node_id: u16 = std::env::var("NODE_ID")
        .unwrap_or_else(|_| "1".to_string())
        .parse()?;

    let listen_addr: SocketAddr = std::env::var("LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:50052".to_string())
        .parse()?;

    let admin_addr: SocketAddr = std::env::var("ADMIN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:9091".to_string())
        .parse()?;

    let director_addr =
        std::env::var("DIRECTOR_ADDR").unwrap_or_else(|_| "http://127.0.0.1:50051".to_string());

    let db_url = std::env::var("DATABASE_URL").ok();

    println!("============================================================");
    println!("        MMO Simulation Node Daemon Initialized             ");
    println!("============================================================");
    println!("Node ID          : {}", node_id);
    println!("Listen Address   : {}", listen_addr);
    println!("Admin Address    : {}", admin_addr);
    println!("Director Address : {}", director_addr);
    println!(
        "Database URL     : {}",
        db_url
            .as_deref()
            .unwrap_or("Disabled (DATABASE_URL not set)")
    );
    println!("------------------------------------------------------------");

    let telemetry = mmo_core::TelemetryServer::init()?;
    telemetry.spawn(admin_addr);

    let _snapshotter = if let Some(ref url) = db_url {
        match Database::connect(url).await {
            Ok(db) => {
                println!("[Node {}] Connected to database at {}", node_id, url);
                let repo = CharacterRepository::new(db);
                Some(WriteBehindSnapshotter::new(repo))
            }
            Err(e) => {
                eprintln!("[Node {}] Database connection failed: {}", node_id, e);
                None
            }
        }
    } else {
        None
    };

    // Default world bounds (-100.0 to 100.0)
    let bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
    let sim = Arc::new(RwLock::new(TickSimulation::new(node_id, bounds)));

    // 1. Start gRPC NodeService server
    let node_service = NodeServiceImpl::new(Arc::clone(&sim));
    tokio::spawn(async move {
        println!(
            "[Node {}] gRPC service listening on {}",
            node_id, listen_addr
        );
        if let Err(e) = Server::builder()
            .add_service(NodeServiceServer::new(node_service))
            .serve(listen_addr)
            .await
        {
            eprintln!("[Node {}] gRPC Server failed: {}", node_id, e);
        }
    });

    // 2. Connect and register with Director (with retry backoff)
    let director_endpoint = director_addr.clone();
    let sim_clone = Arc::clone(&sim);
    let telemetry_clone = telemetry.clone();
    tokio::spawn(async move {
        println!(
            "[Node {}] Attempting connection to Director at {}...",
            node_id, director_endpoint
        );
        let mut client = loop {
            match DirectorServiceClient::connect(director_endpoint.clone()).await {
                Ok(c) => {
                    println!("[Node {}] Connected to Director.", node_id);
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
            node_id: node_id as u32,
            listen_addr: listen_addr.to_string(),
            bounds: proto_bounds,
        };

        match client.register_node(reg_req).await {
            Ok(resp) => {
                let success = resp.into_inner().success;
                println!(
                    "[Node {}] Registered with Director: success={}",
                    node_id, success
                );
                if success {
                    telemetry_clone.mark_ready();
                }
            }
            Err(e) => {
                eprintln!("[Node {}] Failed to register with Director: {}", node_id, e);
            }
        }

        // Heartbeat loop
        let mut hb_interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            hb_interval.tick().await;
            let entity_count = {
                let s = sim_clone.read().await;
                s.entity_store.count() as u32
            };

            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;

            let hb = HeartbeatRequest {
                node_id: node_id as u32,
                tick_rate: 20.0,
                entity_count,
                cpu_percent: 5.0,
                memory_bytes: 10_000_000,
                timestamp: now,
            };

            match client.heartbeat(hb).await {
                Ok(resp) => {
                    let inner = resp.into_inner();
                    if !inner.acknowledged {
                        let reg_req = RegisterNodeRequest {
                            node_id: node_id as u32,
                            listen_addr: listen_addr.to_string(),
                            bounds: proto_bounds,
                        };
                        if let Ok(reg_resp) = client.register_node(reg_req).await {
                            println!(
                                "[Node {}] Re-registered with Director after unacknowledged heartbeat: success={}",
                                node_id,
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
                            node_id: node_id as u32,
                            original_bounds: proto_bounds,
                            child_bounds: vec![],
                            total_entities: entity_count,
                        };
                        if let Ok(split_resp) = client.report_split(split_req).await {
                            println!(
                                "[Node {}] Split reported, assigned nodes: {:?}",
                                node_id,
                                split_resp.into_inner().assigned_node_ids
                            );
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[Node {}] Heartbeat send failed: {}", node_id, e);
                }
            }
        }
    });

    // 3. Fixed 20 Hz (50ms) simulation game loop with clamped tick accumulator
    println!("[Node {}] Starting 20 Hz simulation tick loop...", node_id);
    let tick_duration = Duration::from_millis(50);
    const MAX_CATCHUP_TICKS: usize = 3;
    let mut next_tick = tokio::time::Instant::now() + tick_duration;

    loop {
        let now = tokio::time::Instant::now();
        if now < next_tick {
            tokio::time::sleep_until(next_tick).await;
        }

        let mut catchup = 0;
        while tokio::time::Instant::now() >= next_tick && catchup < MAX_CATCHUP_TICKS {
            let mut sim_guard = sim.write().await;
            sim_guard.step_tick();
            catchup += 1;
            next_tick += tick_duration;
        }

        // Clamp residual lag to prevent spiral of death
        let now_after = tokio::time::Instant::now();
        if now_after >= next_tick {
            next_tick = now_after + tick_duration;
        }
    }
}
