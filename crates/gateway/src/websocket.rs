//! WebSocket (WS / WSS over TCP) fallback transport for Edge Gateway.
//!
//! Provides a TCP-based WebSocket server with optional `tokio-rustls` TLS termination
//! for restricted network environments where UDP / HTTP/3 WebTransport is blocked.
//! Multiplexes both unreliable (`datagram_tx`) and reliable (`stream_tx`) `SessionSink`
//! channels over a single WebSocket binary stream using the exact `0x4D4D` framing
//! and `PacketDispatcher` routing used by TCP and WebTransport.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use mmo_schema::client::{ClientCodecError, ClientPacket, MAX_PACKET_PAYLOAD_SIZE};
use mmo_storage::cache::SessionCacheBackend;

use crate::router::SessionRouter;
use crate::server::{PacketDispatcher, RoutedPacket, SessionDropGuard, SessionSink};

/// Maximum WebSocket message/frame and accumulation buffer size (256 KiB) to allow
/// coalesced multi-frame binary messages while preventing memory exhaustion DoS.
/// Individual packets remain strictly bounded by `MAX_PACKET_PAYLOAD_SIZE` (64 KiB).
pub const MAX_WS_BUFFER_SIZE: usize = 256 * 1024;

/// Maximum duration allowed for TLS handshake and WebSocket HTTP upgrade.
const WS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// WebSocket fallback transport errors.
#[derive(Debug, Error)]
pub enum WebSocketError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("TLS error: {0}")]
    Tls(String),

    #[error("WebSocket protocol error: {0}")]
    WebSocket(#[from] Box<tokio_tungstenite::tungstenite::Error>),

    #[error("Packet codec error: {0}")]
    Codec(#[from] ClientCodecError),
}

/// WebSocket fallback server accepting `ws://` and optional `wss://` connections.
pub struct WebSocketServer {
    listener: TcpListener,
    tls_acceptor: Option<TlsAcceptor>,
    router: Arc<SessionRouter>,
    dispatcher: PacketDispatcher,
    session_counter: Arc<AtomicU64>,
    session_sinks: Arc<DashMap<u64, SessionSink>>,
    packet_tx: Option<mpsc::Sender<RoutedPacket>>,
    authenticator: Option<Arc<crate::auth::EdgeAuthenticator>>,
    default_node_address: Option<String>,
    intercept_handler: Option<crate::server::GatewayInterceptHandler>,
    cache: Option<Arc<dyn SessionCacheBackend>>,
    connection_semaphore: Option<Arc<tokio::sync::Semaphore>>,
}

impl WebSocketServer {
    /// Binds a plain WebSocket (`ws://`) TCP listener to `bind_addr`.
    pub async fn bind(
        bind_addr: SocketAddr,
        router: Arc<SessionRouter>,
        session_sinks: Arc<DashMap<u64, SessionSink>>,
        session_counter: Arc<AtomicU64>,
        packet_tx: Option<mpsc::Sender<RoutedPacket>>,
    ) -> Result<Self, WebSocketError> {
        let listener = TcpListener::bind(bind_addr).await?;
        let dispatcher = PacketDispatcher::new(router.clone());

        Ok(Self {
            listener,
            tls_acceptor: None,
            router,
            dispatcher,
            session_counter,
            session_sinks,
            packet_tx,
            authenticator: None,
            default_node_address: None,
            intercept_handler: None,
            cache: None,
            connection_semaphore: None,
        })
    }

    /// Enables TLS (`wss://`) termination using the provided `rustls::ServerConfig`.
    pub fn with_tls_config(mut self, tls_config: Arc<rustls::ServerConfig>) -> Self {
        self.tls_acceptor = Some(TlsAcceptor::from(tls_config));
        self
    }

    /// Configures an edge authenticator for admission control.
    pub fn with_authenticator(
        mut self,
        authenticator: Arc<crate::auth::EdgeAuthenticator>,
    ) -> Self {
        self.authenticator = Some(authenticator);
        self
    }

    /// Sets a default simulation node address for initial session assignment.
    pub fn with_default_node_address(mut self, node_address: impl Into<String>) -> Self {
        self.default_node_address = Some(node_address.into());
        self
    }

    /// Configures a gateway packet intercept handler for social and marketplace events.
    pub fn with_intercept_handler(
        mut self,
        intercept_handler: crate::server::GatewayInterceptHandler,
    ) -> Self {
        self.intercept_handler = Some(intercept_handler);
        self
    }

    /// Configures an optional distributed session cache backend.
    pub fn with_cache(mut self, cache: Arc<dyn SessionCacheBackend>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Configures a shared connection capacity semaphore.
    pub fn with_connection_semaphore(
        mut self,
        connection_semaphore: Arc<tokio::sync::Semaphore>,
    ) -> Self {
        self.connection_semaphore = Some(connection_semaphore);
        self
    }

    /// Returns the local bound socket address.
    pub fn local_addr(&self) -> Result<SocketAddr, WebSocketError> {
        self.listener.local_addr().map_err(WebSocketError::Io)
    }

    /// Returns true if TLS (`wss://`) is enabled on this listener.
    pub fn is_tls(&self) -> bool {
        self.tls_acceptor.is_some()
    }

    /// Runs the TCP accept loop, spawning handlers for incoming WebSocket sessions until shutdown.
    pub async fn run(self, mut shutdown_rx: tokio::sync::broadcast::Receiver<()>) {
        loop {
            tokio::select! {
                accept_res = self.listener.accept() => {
                    let (tcp_stream, _remote_addr) = match accept_res {
                        Ok(pair) => pair,
                        Err(_) => continue,
                    };

                    let permit = if let Some(ref sem) = self.connection_semaphore {
                        match sem.clone().try_acquire_owned() {
                            Ok(permit) => Some(permit),
                            Err(_) => {
                                metrics::counter!("mmo_gateway_connections_rejected_max_capacity")
                                    .increment(1);
                                continue;
                            }
                        }
                    } else {
                        None
                    };

                    let _ = tcp_stream.set_nodelay(true);

                    let session_id = self.session_counter.fetch_add(1, Ordering::Relaxed);
                    let tls_acceptor = self.tls_acceptor.clone();
                    let router = self.router.clone();
                    let dispatcher = self.dispatcher.clone();
                    let session_sinks = self.session_sinks.clone();
                    let packet_tx = self.packet_tx.clone();
                    let authenticator = self.authenticator.clone();
                    let default_node = self.default_node_address.clone();
                    let intercept_handler = self.intercept_handler.clone();
                    let cache = self.cache.clone();

                    tokio::spawn(async move {
                        let _permit = permit;
                        Self::accept_and_serve(
                            tcp_stream,
                            tls_acceptor,
                            session_id,
                            router,
                            dispatcher,
                            session_sinks,
                            packet_tx,
                            authenticator,
                            default_node,
                            intercept_handler,
                            cache,
                        )
                        .await;
                    });
                }
                _ = shutdown_rx.recv() => {
                    break;
                }
            }
        }
    }

    /// Spawns the server in a background task and returns a cancellation sender.
    pub fn run_background(
        self,
    ) -> (
        tokio::task::JoinHandle<()>,
        tokio::sync::broadcast::Sender<()>,
    ) {
        let (tx, rx) = tokio::sync::broadcast::channel(1);
        let handle = tokio::spawn(async move {
            self.run(rx).await;
        });
        (handle, tx)
    }

    fn ws_config() -> WebSocketConfig {
        let mut config = WebSocketConfig::default();
        config.max_message_size = Some(MAX_WS_BUFFER_SIZE);
        config.max_frame_size = Some(MAX_WS_BUFFER_SIZE);
        config
    }

    #[allow(clippy::too_many_arguments)]
    async fn accept_and_serve(
        tcp_stream: TcpStream,
        tls_acceptor: Option<TlsAcceptor>,
        session_id: u64,
        router: Arc<SessionRouter>,
        dispatcher: PacketDispatcher,
        session_sinks: Arc<DashMap<u64, SessionSink>>,
        packet_tx: Option<mpsc::Sender<RoutedPacket>>,
        authenticator: Option<Arc<crate::auth::EdgeAuthenticator>>,
        default_node_address: Option<String>,
        intercept_handler: Option<crate::server::GatewayInterceptHandler>,
        cache: Option<Arc<dyn SessionCacheBackend>>,
    ) {
        let ws_config = Some(Self::ws_config());

        if let Some(acceptor) = tls_acceptor {
            let tls_stream =
                match tokio::time::timeout(WS_HANDSHAKE_TIMEOUT, acceptor.accept(tcp_stream)).await
                {
                    Ok(Ok(stream)) => stream,
                    _ => return,
                };
            let ws_stream = match tokio::time::timeout(
                WS_HANDSHAKE_TIMEOUT,
                tokio_tungstenite::accept_async_with_config(tls_stream, ws_config),
            )
            .await
            {
                Ok(Ok(ws)) => ws,
                _ => return,
            };
            Self::handle_ws_stream(
                ws_stream,
                session_id,
                router,
                dispatcher,
                session_sinks,
                packet_tx,
                authenticator,
                default_node_address,
                intercept_handler,
                cache,
            )
            .await;
        } else {
            let ws_stream = match tokio::time::timeout(
                WS_HANDSHAKE_TIMEOUT,
                tokio_tungstenite::accept_async_with_config(tcp_stream, ws_config),
            )
            .await
            {
                Ok(Ok(ws)) => ws,
                _ => return,
            };
            Self::handle_ws_stream(
                ws_stream,
                session_id,
                router,
                dispatcher,
                session_sinks,
                packet_tx,
                authenticator,
                default_node_address,
                intercept_handler,
                cache,
            )
            .await;
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn handle_ws_stream<S>(
        ws_stream: WebSocketStream<S>,
        session_id: u64,
        router: Arc<SessionRouter>,
        dispatcher: PacketDispatcher,
        session_sinks: Arc<DashMap<u64, SessionSink>>,
        packet_tx: Option<mpsc::Sender<RoutedPacket>>,
        authenticator: Option<Arc<crate::auth::EdgeAuthenticator>>,
        default_node_address: Option<String>,
        intercept_handler: Option<crate::server::GatewayInterceptHandler>,
        cache: Option<Arc<dyn SessionCacheBackend>>,
    ) where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (mut ws_write, mut ws_read) = ws_stream.split();
        let (datagram_tx, mut datagram_rx) = mpsc::channel::<Bytes>(1024);
        let (stream_tx, mut stream_rx) =
            mpsc::channel::<Bytes>(crate::server::DEFAULT_STREAM_CHANNEL_CAPACITY);
        let (ctrl_tx, mut ctrl_rx) = mpsc::channel::<Bytes>(64);
        let writer_done = Arc::new(tokio::sync::Notify::new());
        let writer_done_signal = writer_done.clone();

        // Writer task: biased select prioritizes control pongs & reliable stream packets
        // over ephemeral datagram broadcasts.
        let writer_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    Some(pong_payload) = ctrl_rx.recv() => {
                        if ws_write.send(Message::Pong(pong_payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(bytes) = stream_rx.recv() => {
                        if ws_write.send(Message::Binary(bytes)).await.is_err() {
                            break;
                        }
                    }
                    Some(bytes) = datagram_rx.recv() => {
                        if ws_write.send(Message::Binary(bytes)).await.is_err() {
                            break;
                        }
                    }
                    else => break,
                }
            }
            let _ = ws_write.close().await;
            writer_done_signal.notify_waiters();
        });

        let _guard = SessionDropGuard {
            session_id,
            router: router.clone(),
            session_sinks: session_sinks.clone(),
            cache: cache.clone(),
            subscription_manager: None,
            writer_handle: Some(writer_handle),
            intercept_handler: intercept_handler.clone().map(Arc::new),
        };

        let mut buf = BytesMut::with_capacity(4096);

        // Enforce authentication handshake when EdgeAuthenticator is configured.
        if let Some(ref auth) = authenticator {
            let timeout = auth.auth_timeout();
            let auth_fut = async {
                loop {
                    match ClientPacket::decode_frame(&mut buf) {
                        Ok(Some(pkt)) => return Ok::<ClientPacket, ()>(pkt),
                        Ok(None) => {}
                        Err(_) => {
                            metrics::counter!("mmo_gateway_framing_errors_total").increment(1);
                            return Err(());
                        }
                    }
                    if buf.len() > MAX_PACKET_PAYLOAD_SIZE {
                        return Err(());
                    }
                    match ws_read.next().await {
                        Some(Ok(Message::Binary(data))) => {
                            if buf.len() + data.len() > MAX_WS_BUFFER_SIZE {
                                metrics::counter!("mmo_gateway_buffer_overflow_dropped_total")
                                    .increment(1);
                                return Err(());
                            }
                            buf.extend_from_slice(&data);
                        }
                        Some(Ok(Message::Ping(payload))) => {
                            let _ = ctrl_tx.try_send(payload);
                        }
                        Some(Ok(Message::Pong(_))) => {}
                        Some(Ok(Message::Text(_))) => {
                            metrics::counter!("mmo_gateway_framing_errors_total").increment(1);
                            return Err(());
                        }
                        _ => return Err(()),
                    }
                }
            };

            let first_pkt = match tokio::time::timeout(timeout, auth_fut).await {
                Ok(Ok(pkt)) => pkt,
                _ => {
                    metrics::counter!("mmo_gateway_auth_failures_total", "reason" => "timeout")
                        .increment(1);
                    return;
                }
            };

            match first_pkt {
                ClientPacket::AuthRequest(auth_req) => {
                    match crate::server::auth_handshake::authenticate_and_register_session(
                        auth,
                        &auth_req.token,
                        session_id,
                        &router,
                        default_node_address.as_deref(),
                        cache.as_ref(),
                        intercept_handler.as_ref(),
                    )
                    .await
                    {
                        Ok(auth_sess) => {
                            // Register SessionSink ONLY after authentication succeeds.
                            session_sinks.insert(
                                session_id,
                                SessionSink::new(datagram_tx.clone(), stream_tx.clone()),
                            );
                            metrics::gauge!("mmo_gateway_active_sessions")
                                .set(session_sinks.len() as f64);
                            let resp = ClientPacket::AuthResponse(auth_sess.auth_response);
                            let _ = stream_tx.send(resp.encode().into()).await;
                        }
                        Err((_err, err_resp)) => {
                            let resp = ClientPacket::AuthResponse(err_resp);
                            let _ = stream_tx.send(resp.encode().into()).await;
                            tokio::time::sleep(Duration::from_millis(15)).await;
                            return;
                        }
                    }
                }
                _ => {
                    metrics::counter!(
                        "mmo_gateway_auth_failures_total",
                        "reason" => "unauthenticated_action"
                    )
                    .increment(1);
                    return;
                }
            }
        } else {
            if let Some(ref target_node) = default_node_address {
                // Auto-register default player route in PoC / unauthenticated mode.
                let default_guid = mmo_core::Guid::new(
                    mmo_core::EntityType::Player,
                    1,
                    (session_id & 0xFFFF_FFFF) as u32,
                );
                router.register_session(session_id, default_guid, target_node.clone());
            }
            session_sinks.insert(
                session_id,
                SessionSink::new(datagram_tx.clone(), stream_tx.clone()),
            );
            metrics::gauge!("mmo_gateway_active_sessions").set(session_sinks.len() as f64);
        }

        // Process any packets already buffered after the AuthRequest frame.
        if !Self::process_buffered_frames(
            &mut buf,
            session_id,
            &router,
            &dispatcher,
            &packet_tx,
            &session_sinks,
            authenticator.is_some(),
            default_node_address.as_deref(),
            intercept_handler.as_ref(),
        )
        .await
        {
            return;
        }

        // Drop stack sender clones so `session_sinks.remove(&session_id)` (e.g. session revocation)
        // immediately closes the writer channels and terminates the WebSocket writer task.
        drop(datagram_tx);
        drop(stream_tx);

        // Main ingress loop.
        loop {
            tokio::select! {
                _ = writer_done.notified() => break,
                msg_opt = ws_read.next() => {
                    match msg_opt {
                        Some(Ok(Message::Binary(data))) => {
                            if buf.len() + data.len() > MAX_WS_BUFFER_SIZE {
                                metrics::counter!("mmo_gateway_buffer_overflow_dropped_total")
                                    .increment(1);
                                buf.clear();
                                return;
                            }
                            buf.extend_from_slice(&data);
                            if !Self::process_buffered_frames(
                                &mut buf,
                                session_id,
                                &router,
                                &dispatcher,
                                &packet_tx,
                                &session_sinks,
                                authenticator.is_some(),
                                default_node_address.as_deref(),
                                intercept_handler.as_ref(),
                            )
                            .await
                            {
                                return;
                            }
                        }
                        Some(Ok(Message::Ping(payload))) => {
                            let _ = ctrl_tx.try_send(payload);
                        }
                        Some(Ok(Message::Pong(_))) => {}
                        Some(Ok(Message::Close(_))) | None => break,
                        Some(Ok(Message::Text(_))) => {
                            metrics::counter!("mmo_gateway_framing_errors_total").increment(1);
                            return;
                        }
                        Some(Ok(Message::Frame(_))) => {}
                        Some(Err(_)) => break,
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn process_buffered_frames(
        buf: &mut BytesMut,
        session_id: u64,
        router: &SessionRouter,
        dispatcher: &PacketDispatcher,
        packet_tx: &Option<mpsc::Sender<RoutedPacket>>,
        session_sinks: &DashMap<u64, SessionSink>,
        has_authenticator: bool,
        default_node_address: Option<&str>,
        intercept_handler: Option<&crate::server::GatewayInterceptHandler>,
    ) -> bool {
        loop {
            match ClientPacket::decode_frame(buf) {
                Ok(Some(pkt)) => {
                    metrics::counter!(
                        "mmo_gateway_packets_received_total",
                        "opcode" => pkt.opcode().as_str()
                    )
                    .increment(1);

                    let Some(sink_entry) = session_sinks.get(&session_id) else {
                        return false;
                    };

                    // Unauthenticated / PoC mode: respond to AuthRequest with assigned player Guid.
                    if let ClientPacket::AuthRequest(_) = &pkt {
                        if !has_authenticator {
                            let default_guid = mmo_core::Guid::new(
                                mmo_core::EntityType::Player,
                                1,
                                (session_id & 0xFFFF_FFFF) as u32,
                            );
                            let assigned_node = default_node_address
                                .unwrap_or("127.0.0.1:50052")
                                .to_string();
                            router.register_session(
                                session_id,
                                default_guid,
                                assigned_node.clone(),
                            );
                            let resp = ClientPacket::AuthResponse(
                                mmo_schema::client::AuthResponsePacket {
                                    success: true,
                                    error_code: 0,
                                    session_id,
                                    player_guid: default_guid.raw(),
                                    assigned_node,
                                    error_message: String::new(),
                                },
                            );
                            let _ = sink_entry.stream_tx.try_send(resp.encode().into());
                            continue;
                        }
                    }

                    if let ClientPacket::Ping(ref ping) = pkt {
                        let pong = ClientPacket::Pong(mmo_schema::client::PongPacket {
                            client_timestamp: ping.timestamp,
                            server_timestamp: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as u64,
                        });
                        let _ = sink_entry.datagram_tx.try_send(pong.encode().into());
                        continue;
                    }
                    drop(sink_entry);

                    if let Some(handler) = intercept_handler {
                        if handler.handle_packet(session_id, &pkt).await {
                            continue;
                        }
                    }

                    if let Ok(routed) = dispatcher.dispatch_packet(session_id, pkt) {
                        if let Some(ref tx) = packet_tx {
                            let _ = tx.send(routed).await;
                        }
                    }
                }
                Ok(None) => return true,
                Err(_) => {
                    metrics::counter!("mmo_gateway_framing_errors_total").increment(1);
                    buf.clear();
                    return false;
                }
            }
        }
    }
}
