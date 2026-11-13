use mmo_gateway::{GatewayServer, SessionRouter};
use mmo_schema::cluster::node_service_client::NodeServiceClient;
use mmo_schema::cluster::ForwardClientPacketRequest;
use mmo_storage::cache::{MemoryCache, RedisCache, SessionCacheBackend};
use std::sync::Arc;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let listen_addr = std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
    let wt_bind_addr =
        std::env::var("WEBTRANSPORT_ADDR").unwrap_or_else(|_| "0.0.0.0:4433".to_string());
    let ws_bind_addr =
        std::env::var("WEBSOCKET_ADDR").unwrap_or_else(|_| "0.0.0.0:8443".to_string());
    let ws_tls_enabled = std::env::var("WEBSOCKET_TLS")
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false);

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

    let gateway_id = std::env::var("GATEWAY_ID").unwrap_or_else(|_| "gw-local-1".to_string());
    let region = std::env::var("REGION").unwrap_or_else(|_| "us-east".to_string());
    let probe_addr = std::env::var("PROBE_ADDR").unwrap_or_else(|_| "0.0.0.0:8081".to_string());
    let capacity: u32 = std::env::var("CAPACITY")
        .unwrap_or_else(|_| "10000".to_string())
        .parse()
        .unwrap_or(10000);

    let redis_url = std::env::var("REDIS_URL").ok();

    println!("============================================================");
    println!("          MMO Edge Gateway Daemon Initialized               ");
    println!("============================================================");
    println!("Gateway ID          : {gateway_id}");
    println!("Region              : {region}");
    println!("Listen Address      : {listen_addr} (TCP)");
    println!("WebTransport Address: {wt_bind_addr} (UDP)");
    println!(
        "WebSocket Address   : {ws_bind_addr} ({})",
        if ws_tls_enabled { "WSS/TCP" } else { "WS/TCP" }
    );
    println!("Probe Address       : {probe_addr}");
    println!("Capacity            : {capacity}");
    println!("Admin Address       : {admin_addr}");
    println!("gRPC Address        : {grpc_addr}");
    println!("Default Node Route  : {default_node_addr}");
    println!("Director Address    : {director_addr}");
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
                println!("[Gateway] Connected to Redis session cache at {url}");
                Arc::new(rc)
            }
            Err(e) => {
                eprintln!(
                    "[Gateway] Failed to connect to Redis ({e}), falling back to MemoryCache"
                );
                Arc::new(MemoryCache::new())
            }
        },
        None => Arc::new(MemoryCache::new()),
    };

    let router = Arc::new(SessionRouter::new());
    let (packet_tx, mut packet_rx) = mpsc::channel(10000);
    let server = Arc::new(
        GatewayServer::with_packet_channel(router, packet_tx)
            .with_cache(cache)
            .with_default_node_address(default_node_addr.clone()),
    );

    // Background worker to consume routed client packets and forward to node cluster via BackhaulPool
    let backhaul_pool = Arc::clone(server.backhaul_pool());
    let router_clone = Arc::clone(server.router());
    tokio::spawn(async move {
        while let Some(mut pkt) = packet_rx.recv().await {
            let (ch, rx_opt) = backhaul_pool.get_or_create(&pkt.node_address);
            if let Some(mut rx) = rx_opt {
                let node_addr = pkt.node_address.clone();
                let ch_clone = Arc::clone(&ch);
                tokio::spawn(async move {
                    use mmo_gateway::backhaul::BackhaulState;
                    use tracing::Instrument;
                    let endpoint = if node_addr.starts_with("http") {
                        node_addr
                    } else {
                        format!("http://{}", node_addr)
                    };
                    let mut client_opt = None;
                    let probe_duration =
                        std::time::Duration::from_millis(ch_clone.probe_interval_ms());
                    let mut probe_interval = tokio::time::interval(probe_duration);
                    probe_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

                    loop {
                        tokio::select! {
                            pkt_opt = rx.recv() => {
                                match pkt_opt {
                                    Some(packet) => {
                                        let opcode = packet.packet.opcode().as_str();
                                        let span = tracing::info_span!(
                                            "client_packet_forward",
                                            session_id = packet.session_id,
                                            opcode = opcode,
                                            target_node = %endpoint
                                        );
                                        async {
                                            if client_opt.is_none() {
                                                client_opt =
                                                    NodeServiceClient::connect(endpoint.clone()).await.ok();
                                            }
                                            if let Some(ref mut client) = client_opt {
                                                let req = ForwardClientPacketRequest {
                                                    session_id: packet.session_id,
                                                    guid: packet.guid.map(|g| g.raw()).unwrap_or(0),
                                                    packet_data: packet.packet.encode(),
                                                    session_epoch: packet.session_epoch,
                                                };
                                                match client.forward_client_packet(req).await {
                                                    Ok(_) => ch_clone.record_success(),
                                                    Err(e) => {
                                                        ch_clone.record_failure();
                                                        tracing::error!(error = %e, "Failed forwarding client packet to node");
                                                        eprintln!(
                                                            "[Gateway Backhaul] Failed forwarding to node: {e}"
                                                        );
                                                        client_opt = None;
                                                    }
                                                }
                                            } else {
                                                ch_clone.record_failure();
                                            }
                                        }
                                        .instrument(span)
                                        .await;
                                    }
                                    None => break,
                                }
                            }
                            _ = probe_interval.tick() => {
                                if ch_clone.state() == BackhaulState::HalfOpen {
                                    if client_opt.is_none() {
                                        client_opt =
                                            NodeServiceClient::connect(endpoint.clone()).await.ok();
                                    }
                                    if let Some(ref mut client) = client_opt {
                                        let probe_req = ForwardClientPacketRequest {
                                            session_id: 0,
                                            guid: 0,
                                            packet_data: mmo_schema::client::ClientPacket::Ping(
                                                mmo_schema::client::PingPacket {
                                                    timestamp: std::time::SystemTime::now()
                                                        .duration_since(std::time::UNIX_EPOCH)
                                                        .unwrap_or_default()
                                                        .as_millis() as u64,
                                                },
                                            )
                                            .encode(),
                                            session_epoch: 0,
                                        };
                                        match client.forward_client_packet(probe_req).await {
                                            Ok(_) => {
                                                ch_clone.record_success();
                                                tracing::info!(
                                                    target_node = %endpoint,
                                                    "Backhaul circuit breaker successfully recovered to Connected via probe"
                                                );
                                            }
                                            Err(e) => {
                                                ch_clone.record_failure();
                                                tracing::debug!(
                                                    target_node = %endpoint,
                                                    error = %e,
                                                    "Backhaul probe failed"
                                                );
                                                client_opt = None;
                                            }
                                        }
                                    } else {
                                        ch_clone.record_failure();
                                    }
                                }
                            }
                        }
                    }
                });
            }
            if let Err(e) = backhaul_pool.send_packet(pkt.clone()) {
                match e {
                    mmo_gateway::backhaul::BackhaulError::CircuitBreakerOpen(ref dead_node)
                    | mmo_gateway::backhaul::BackhaulError::NodeUnreachable(ref dead_node) => {
                        let healthy = backhaul_pool.healthy_channels();
                        if let Some(fallback_node) =
                            healthy.into_iter().find(|addr| addr != dead_node)
                        {
                            tracing::warn!(
                                dead_node = %dead_node,
                                fallback_node = %fallback_node,
                                "Reassigning routes from broken node to healthy node"
                            );
                            router_clone.reassign_node_routes(dead_node, &fallback_node);
                            pkt.node_address = fallback_node;
                            if let Err(err) = backhaul_pool.send_packet(pkt) {
                                eprintln!(
                                    "[Gateway] Failed forwarding packet to reassigned node: {err:?}"
                                );
                            }
                        } else {
                            eprintln!("[Gateway] Failed forwarding packet to BackhaulPool: {e:?}");
                        }
                    }
                    _ => {
                        eprintln!("[Gateway] Failed forwarding packet to BackhaulPool: {e:?}");
                    }
                }
            }
        }
    });

    let (grpc_shutdown_tx, grpc_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let grpc_service = server.create_service();
    let grpc_handle = tokio::spawn(async move {
        println!("[Gateway] gRPC Service listening on {grpc_addr}");
        if let Err(e) = tonic::transport::Server::builder()
            .add_service(grpc_service.into_server())
            .serve_with_shutdown(grpc_addr, async move {
                let _ = grpc_shutdown_rx.await;
            })
            .await
        {
            eprintln!("[Gateway] gRPC Server failed: {e}");
        }
    });

    // Spawn UDP Anycast Probe Responder
    match tokio::net::UdpSocket::bind(&probe_addr).await {
        Ok(probe_sock) => {
            let sinks = server.session_sinks();
            let active_counter = Arc::new(std::sync::atomic::AtomicU32::new(0));
            let s_counter = Arc::clone(&active_counter);
            let s_region = region.clone();
            tokio::spawn(async move {
                mmo_gateway::probe::run_probe_server(probe_sock, s_region, capacity, s_counter)
                    .await;
            });

            // Keep active session counter synchronized for lock-free probing
            let s_counter_sync = Arc::clone(&active_counter);
            let sinks_sync = Arc::clone(&sinks);
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    s_counter_sync.store(
                        sinks_sync.len() as u32,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                }
            });
            println!("Anycast Latency & Health Probe active on UDP {probe_addr}");
        }
        Err(e) => {
            eprintln!("[Gateway] Failed to bind probe socket on {probe_addr}: {e}");
        }
    }

    // Spawn Director Gateway Heartbeat Loop
    let heartbeat_handle = GatewayServer::spawn_heartbeat_loop(
        director_addr.clone(),
        gateway_id.clone(),
        region,
        listen_addr.clone(),
        grpc_addr.to_string(),
        probe_addr,
        capacity,
        server.session_sinks(),
    );

    // Bind and launch WebTransport HTTP/3 listener (with self-signed cert discovery)
    let mut wt_shutdown_tx = None;
    if let Ok(wt_addr) = wt_bind_addr.parse::<std::net::SocketAddr>() {
        match server.bind_webtransport(wt_addr).await {
            Ok(wt_server) => {
                let digest = wt_server.cert_digest().to_string();
                println!("[Gateway] WebTransport HTTP/3 listener bound on UDP {wt_addr}");
                println!("[Gateway] WebTransport SHA-256 Certificate Fingerprint: {digest}");
                telemetry.set_cert_hash(&digest);

                let (_wt_handle, tx) = wt_server.run_background();
                wt_shutdown_tx = Some(tx);
            }
            Err(e) => {
                eprintln!(
                    "[Gateway] Failed to bind WebTransport HTTP/3 listener on {wt_addr}: {e}"
                );
            }
        }
    } else {
        eprintln!("[Gateway] Invalid WEBTRANSPORT_ADDR: {wt_bind_addr}");
    }

    // Bind and launch WebSocket / TLS over TCP fallback listener
    let mut ws_shutdown_tx = None;
    if let Ok(ws_addr) = ws_bind_addr.parse::<std::net::SocketAddr>() {
        let ws_res = if ws_tls_enabled {
            let _ = rustls::crypto::ring::default_provider().install_default();
            match mmo_gateway::generate_self_signed_cert(&["localhost", "127.0.0.1"]) {
                Ok((certs, key)) => match rustls::ServerConfig::builder()
                    .with_no_client_auth()
                    .with_single_cert(certs, key)
                {
                    Ok(cfg) => server.bind_websocket_tls(ws_addr, Arc::new(cfg)).await,
                    Err(e) => Err(mmo_gateway::websocket::WebSocketError::Tls(e.to_string())),
                },
                Err(e) => Err(mmo_gateway::websocket::WebSocketError::Tls(e.to_string())),
            }
        } else {
            server.bind_websocket(ws_addr).await
        };

        match ws_res {
            Ok(ws_server) => {
                let proto = if ws_server.is_tls() { "WSS" } else { "WS" };
                println!("[Gateway] WebSocket ({proto}) fallback listener bound on TCP {ws_addr}");
                let (_ws_handle, tx) = ws_server.run_background();
                ws_shutdown_tx = Some(tx);
            }
            Err(e) => {
                eprintln!("[Gateway] Failed to bind WebSocket fallback listener on {ws_addr}: {e}");
            }
        }
    } else {
        eprintln!("[Gateway] Invalid WEBSOCKET_ADDR: {ws_bind_addr}");
    }

    let listener = server.bind(&listen_addr).await?;
    telemetry.mark_ready();
    println!("Edge Gateway actively listening for client connections on {listen_addr}");

    let connection_semaphore = Arc::new(tokio::sync::Semaphore::new(capacity as usize));
    let shutdown_fut = mmo_core::shutdown_signal();
    tokio::pin!(shutdown_fut);

    loop {
        tokio::select! {
            _ = &mut shutdown_fut => {
                println!("[Gateway] Shutdown signal received, stopping TCP accept loop...");
                break;
            }
            accept_res = listener.accept() => {
                match accept_res {
                    Ok((socket, _remote_addr)) => {
                        let permit = match connection_semaphore.clone().try_acquire_owned() {
                            Ok(permit) => permit,
                            Err(_) => {
                                metrics::counter!("mmo_gateway_connections_rejected_max_capacity")
                                    .increment(1);
                                eprintln!(
                                    "[Gateway] Connection rejected: maximum capacity ({capacity}) reached"
                                );
                                continue;
                            }
                        };

                        let session_id = server.next_session_id();
                        let server_clone = Arc::clone(&server);
                        let target_node = default_node_addr.clone();

                        // Register initial session routing to default node
                        let _ = server_clone.register_session_fallback(session_id, target_node);

                        tokio::spawn(async move {
                            let _permit = permit;
                            if let Err(e) = server_clone.handle_connection(socket, session_id).await {
                                // Connection ended or error
                                let _ = e;
                            }
                        });
                    }
                    Err(e) => {
                        eprintln!("[Gateway] Failed to accept client connection: {e}");
                    }
                }
            }
        }
    }

    let drained = server
        .drain_and_shutdown(
            &telemetry,
            Some(heartbeat_handle),
            &director_addr,
            &gateway_id,
            std::time::Duration::from_secs(3),
        )
        .await;
    if let Some(tx) = wt_shutdown_tx {
        let _ = tx.send(());
    }
    if let Some(tx) = ws_shutdown_tx {
        let _ = tx.send(());
    }
    let _ = grpc_shutdown_tx.send(());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), grpc_handle).await;
    println!("[Gateway] Clean shutdown complete (closed {drained} active session(s)).");
    Ok(())
}
