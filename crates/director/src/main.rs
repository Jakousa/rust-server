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
    let gateway_grpc_addr = std::env::var("GATEWAY_GRPC_ADDR")
        .unwrap_or_else(|_| "http://mmo-gateway:50050".to_string());

    tokio::spawn(async move {
        let dead_timeout_ms: u64 = std::env::var("NODE_DEAD_TIMEOUT_MS")
            .unwrap_or_else(|_| "5000".to_string())
            .parse()
            .unwrap_or(5000);
        let dead_timeout = Duration::from_millis(dead_timeout_ms);
        eprintln!("[Director] Starting dead node detector loop (timeout={dead_timeout_ms}ms)");
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        let mut tick_count = 0u64;
        loop {
            interval.tick().await;
            tick_count += 1;
            if tick_count.is_multiple_of(10) {
                let reg_len = svc_clone.registry.read().await.len();
                eprintln!(
                    "[Director] Dead node check tick #{tick_count}, registered nodes: {reg_len}"
                );
            }
            let recovery_events = svc_clone.check_and_recover_dead_nodes(dead_timeout).await;
            if !recovery_events.is_empty() {
                eprintln!(
                    "[Director] Recovered {} orphaned partition(s) from expired dead nodes (epoch {}): {:?}",
                    recovery_events.len(),
                    recovery_events[0].epoch,
                    recovery_events
                );

                if let Ok(mut client) =
                    mmo_schema::cluster::gateway_service_client::GatewayServiceClient::connect(
                        gateway_grpc_addr.clone(),
                    )
                    .await
                {
                    for ev in &recovery_events {
                        let dead_addr = format!(
                            "mmo-node-{}.mmo-node:50052",
                            ev.previous_node_id.saturating_sub(1)
                        );
                        let new_addr = format!(
                            "mmo-node-{}.mmo-node:50052",
                            ev.new_node_id.saturating_sub(1)
                        );
                        let req = mmo_schema::cluster::ReassignNodeRoutesRequest {
                            dead_node_address: dead_addr.clone(),
                            new_node_address: new_addr.clone(),
                        };
                        if let Ok(resp) = client.reassign_node_routes(req).await {
                            eprintln!(
                                "[Director] Propagated route reassignment: {dead_addr} -> {new_addr} ({} routes updated)",
                                resp.into_inner().reassigned_count
                            );
                        }
                    }
                }
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

    println!("Director gRPC service listening on {addr}");
    println!("Director Admin service listening on {admin_addr}");
    Server::builder()
        .add_service(DirectorServiceServer::new(service))
        .serve(addr)
        .await?;

    Ok(())
}
