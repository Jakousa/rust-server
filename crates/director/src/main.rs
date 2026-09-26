use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tonic::transport::Server;

use mmo_director::{DirectorServiceImpl, GlobalOctreeCoordinator, NodeRegistry};
use mmo_schema::cluster::director_service_server::DirectorServiceServer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("MMO Director orchestrator initializing...");

    let registry = Arc::new(RwLock::new(NodeRegistry::new()));
    let coordinator = Arc::new(RwLock::new(GlobalOctreeCoordinator::default()));

    let service = DirectorServiceImpl::new(registry, coordinator);

    // Background dead node detection and orphaned partition recovery loop
    let svc_clone = service.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            let recovery_events = svc_clone
                .check_and_recover_dead_nodes(Duration::from_secs(5))
                .await;
            if !recovery_events.is_empty() {
                eprintln!(
                    "[Director] Recovered {} orphaned partition(s) from expired dead nodes (epoch {}): {:?}",
                    recovery_events.len(),
                    recovery_events[0].epoch,
                    recovery_events
                );
            }
        }
    });

    let addr: SocketAddr = std::env::var("LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:50051".to_string())
        .parse()?;

    let admin_addr: SocketAddr = std::env::var("ADMIN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:9091".to_string())
        .parse()?;

    let telemetry = mmo_core::TelemetryServer::init()?;
    telemetry.spawn(admin_addr);
    telemetry.mark_ready();

    println!("Director gRPC service listening on {}", addr);
    println!("Director Admin service listening on {}", admin_addr);
    Server::builder()
        .add_service(DirectorServiceServer::new(service))
        .serve(addr)
        .await?;

    Ok(())
}
