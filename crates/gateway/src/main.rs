use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
use mmo_gateway::{GatewayServer, SessionRouter};
use mmo_storage::cache::{MemoryCache, RedisCache, SessionCacheBackend};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let listen_addr = std::env::var("LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8080".to_string());

    let default_node_addr = std::env::var("DEFAULT_NODE_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:50052".to_string());

    let director_addr = std::env::var("DIRECTOR_ADDR")
        .unwrap_or_else(|_| "http://127.0.0.1:50051".to_string());

    let redis_url = std::env::var("REDIS_URL").ok();

    println!("============================================================");
    println!("          MMO Edge Gateway Daemon Initialized               ");
    println!("============================================================");
    println!("Listen Address      : {}", listen_addr);
    println!("Default Node Route  : {}", default_node_addr);
    println!("Director Address    : {}", director_addr);
    println!(
        "Session Cache       : {}",
        redis_url
            .as_deref()
            .unwrap_or("In-Memory (REDIS_URL not set)")
    );
    println!("------------------------------------------------------------");

    let cache: Arc<dyn SessionCacheBackend> = match redis_url {
        Some(ref url) => match RedisCache::new(url) {
            Ok(rc) => {
                println!("[Gateway] Connected to Redis session cache at {}", url);
                Arc::new(rc)
            }
            Err(e) => {
                eprintln!("[Gateway] Failed to connect to Redis ({}), falling back to MemoryCache", e);
                Arc::new(MemoryCache::new())
            }
        },
        None => Arc::new(MemoryCache::new()),
    };

    let router = Arc::new(RwLock::new(SessionRouter::new()));
    let (packet_tx, mut packet_rx) = mpsc::channel(10000);
    let server = Arc::new(
        GatewayServer::with_packet_channel(router, packet_tx).with_cache(cache),
    );

    // Background worker to consume/log routed packets
    tokio::spawn(async move {
        while let Some(pkt) = packet_rx.recv().await {
            // Forward or log routed packet
            if pkt.is_movement() {
                // In production, forward to destination node via internal channel
            }
        }
    });

    let listener = server.bind(&listen_addr).await?;
    println!("Edge Gateway actively listening for client connections on {}", listen_addr);

    loop {
        match listener.accept().await {
            Ok((socket, _remote_addr)) => {
                let session_id = server.next_session_id();
                let server_clone = Arc::clone(&server);
                let target_node = default_node_addr.clone();

                // Register initial session routing to default node
                {
                    let mut r = server_clone.router().write().unwrap();
                    r.register_session_fallback(session_id, target_node);
                }

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
