use mmo_gateway::{GatewayServer, SessionRouter};
use mmo_storage::cache::{MemoryCache, RedisCache, SessionCacheBackend};
use std::sync::Arc;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let listen_addr = std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());

    let admin_addr: std::net::SocketAddr = std::env::var("ADMIN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:9091".to_string())
        .parse()?;

    let grpc_addr: std::net::SocketAddr = std::env::var("GRPC_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:50050".to_string())
        .parse()?;

    let default_node_addr =
        std::env::var("DEFAULT_NODE_ADDR").unwrap_or_else(|_| "127.0.0.1:50052".to_string());

    let director_addr =
        std::env::var("DIRECTOR_ADDR").unwrap_or_else(|_| "http://127.0.0.1:50051".to_string());

    let redis_url = std::env::var("REDIS_URL").ok();

    println!("============================================================");
    println!("          MMO Edge Gateway Daemon Initialized               ");
    println!("============================================================");
    println!("Listen Address      : {}", listen_addr);
    println!("Admin Address       : {}", admin_addr);
    println!("gRPC Address        : {}", grpc_addr);
    println!("Default Node Route  : {}", default_node_addr);
    println!("Director Address    : {}", director_addr);
    println!(
        "Session Cache       : {}",
        redis_url
            .as_deref()
            .unwrap_or("In-Memory (REDIS_URL not set)")
    );
    println!("------------------------------------------------------------");

    let telemetry = mmo_core::TelemetryServer::init()?;
    telemetry.spawn(admin_addr);

    let cache: Arc<dyn SessionCacheBackend> = match redis_url {
        Some(ref url) => match RedisCache::new(url) {
            Ok(rc) => {
                println!("[Gateway] Connected to Redis session cache at {}", url);
                Arc::new(rc)
            }
            Err(e) => {
                eprintln!(
                    "[Gateway] Failed to connect to Redis ({}), falling back to MemoryCache",
                    e
                );
                Arc::new(MemoryCache::new())
            }
        },
        None => Arc::new(MemoryCache::new()),
    };

    let router = Arc::new(SessionRouter::new());
    let (packet_tx, mut packet_rx) = mpsc::channel(10000);
    let server = Arc::new(GatewayServer::with_packet_channel(router, packet_tx).with_cache(cache));

    // Background worker to consume/log routed packets
    tokio::spawn(async move {
        while let Some(pkt) = packet_rx.recv().await {
            // Forward or log routed packet
            if pkt.is_movement() {
                // In production, forward to destination node via internal channel
            }
        }
    });

    let grpc_service = server.create_service();
    tokio::spawn(async move {
        println!("[Gateway] gRPC Service listening on {}", grpc_addr);
        if let Err(e) = tonic::transport::Server::builder()
            .add_service(grpc_service.into_server())
            .serve(grpc_addr)
            .await
        {
            eprintln!("[Gateway] gRPC Server failed: {}", e);
        }
    });

    let listener = server.bind(&listen_addr).await?;
    telemetry.mark_ready();
    println!(
        "Edge Gateway actively listening for client connections on {}",
        listen_addr
    );

    loop {
        match listener.accept().await {
            Ok((socket, _remote_addr)) => {
                let session_id = server.next_session_id();
                let server_clone = Arc::clone(&server);
                let target_node = default_node_addr.clone();

                // Register initial session routing to default node
                let _ = server_clone.register_session_fallback(session_id, target_node);

                tokio::spawn(async move {
                    if let Err(e) = server_clone.handle_connection(socket, session_id).await {
                        // Connection ended or error
                        let _ = e;
                    }
                });
            }
            Err(e) => {
                eprintln!("[Gateway] Failed to accept client connection: {}", e);
            }
        }
    }
}
