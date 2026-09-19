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

    // Background dead node detection loop
    let reg_clone = Arc::clone(&registry);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            let dead = reg_clone
                .write()
                .await
                .check_dead_nodes(Duration::from_secs(5));
            if !dead.is_empty() {
                eprintln!("[Director] Expired dead nodes removed: {:?}", dead);
            }
        }
    });

    let service = DirectorServiceImpl::new(registry, coordinator);
    let addr: SocketAddr = "0.0.0.0:50051".parse()?;

    println!("Director gRPC service listening on {}", addr);
    Server::builder()
        .add_service(DirectorServiceServer::new(service))
        .serve(addr)
        .await?;

    Ok(())
}
