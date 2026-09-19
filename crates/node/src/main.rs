use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tonic::transport::Server;

use mmo_core::BoundingBox3D;
use mmo_node::{NodeServiceImpl, TickSimulation};
use mmo_schema::cluster::director_service_client::DirectorServiceClient;
use mmo_schema::cluster::node_service_server::NodeServiceServer;
use mmo_schema::cluster::{BoundingBox, HeartbeatRequest, RegisterNodeRequest, Vector3};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let node_id: u16 = std::env::var("NODE_ID")
        .unwrap_or_else(|_| "1".to_string())
        .parse()?;

    let listen_addr: SocketAddr = std::env::var("LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:50052".to_string())
        .parse()?;

    let director_addr = std::env::var("DIRECTOR_ADDR")
        .unwrap_or_else(|_| "http://127.0.0.1:50051".to_string());

    println!("============================================================");
    println!("        MMO Simulation Node Daemon Initialized             ");
    println!("============================================================");
    println!("Node ID          : {}", node_id);
    println!("Listen Address   : {}", listen_addr);
    println!("Director Address : {}", director_addr);
    println!("------------------------------------------------------------");

    // Default world bounds (-100.0 to 100.0)
    let bounds = BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
    let sim = Arc::new(RwLock::new(TickSimulation::new(node_id, bounds)));

    // 1. Start gRPC NodeService server
    let node_service = NodeServiceImpl::new(Arc::clone(&sim));
    tokio::spawn(async move {
        println!("[Node {}] gRPC service listening on {}", node_id, listen_addr);
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
    tokio::spawn(async move {
        println!("[Node {}] Attempting connection to Director at {}...", node_id, director_endpoint);
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
                println!("[Node {}] Registered with Director: success={}", node_id, resp.into_inner().success);
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

            if let Err(e) = client.heartbeat(hb).await {
                eprintln!("[Node {}] Heartbeat send failed: {}", node_id, e);
            }
        }
    });

    // 3. Fixed 20 Hz (50ms) simulation game loop
    println!("[Node {}] Starting 20 Hz simulation tick loop...", node_id);
    let mut tick_interval = tokio::time::interval(Duration::from_millis(50));
    tick_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);

    loop {
        tick_interval.tick().await;
        let mut sim_guard = sim.write().await;
        sim_guard.step_tick();
    }
}
