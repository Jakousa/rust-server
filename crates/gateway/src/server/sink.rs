use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use dashmap::DashMap;
use tokio::sync::mpsc;

use super::dispatcher::{PacketDispatcher, DEFAULT_STREAM_CHANNEL_CAPACITY};
use super::intercept::GatewayInterceptHandler;
use super::{GatewayError, RoutedPacket};
use crate::router::SessionRouter;
use crate::social::GatewaySubscriptionManager;
use mmo_core::{EntityType, Guid};
use mmo_schema::client::{AuthResponsePacket, ClientPacket, PongPacket};
use mmo_storage::cache::SessionCacheBackend;

/// Sinks for forwarding outgoing packets to a connected client session.
#[derive(Clone)]
pub struct SessionSink {
    /// Bounded channel for unreliable datagrams (e.g. movement, entity deltas). Drops on full.
    pub datagram_tx: mpsc::Sender<Bytes>,
    /// Bounded channel for reliable packets (e.g. interaction results, trades, auth). Drops or disconnects on backpressure.
    pub stream_tx: mpsc::Sender<Bytes>,
}

impl SessionSink {
    pub fn new(datagram_tx: mpsc::Sender<Bytes>, stream_tx: mpsc::Sender<Bytes>) -> Self {
        Self {
            datagram_tx,
            stream_tx,
        }
    }
}

impl From<mpsc::Sender<Bytes>> for SessionSink {
    fn from(datagram_tx: mpsc::Sender<Bytes>) -> Self {
        let (stream_tx, _) = mpsc::channel(DEFAULT_STREAM_CHANNEL_CAPACITY);
        Self {
            datagram_tx,
            stream_tx,
        }
    }
}

/// RAII guard that ensures session routes, sinks, and writer tasks are cleanly
/// deregistered when a client connection handler terminates or is cancelled.
pub struct SessionDropGuard {
    pub session_id: u64,
    pub router: Arc<SessionRouter>,
    pub session_sinks: Arc<DashMap<u64, SessionSink>>,
    pub cache: Option<Arc<dyn SessionCacheBackend>>,
    pub subscription_manager: Option<Arc<GatewaySubscriptionManager>>,
    pub writer_handle: Option<tokio::task::JoinHandle<()>>,
    pub intercept_handler: Option<Arc<crate::server::intercept::GatewayInterceptHandler>>,
}

impl Drop for SessionDropGuard {
    fn drop(&mut self) {
        let _ = self.router.remove_session(self.session_id);
        self.session_sinks.remove(&self.session_id);
        metrics::gauge!("mmo_gateway_active_sessions").set(self.session_sinks.len() as f64);
        if let Some(ref intercept) = self.intercept_handler {
            intercept.remove_session(self.session_id);
        } else if let Some(ref sub_mgr) = self.subscription_manager {
            sub_mgr.remove_session(self.session_id);
        }
        if let Some(ref handle) = self.writer_handle {
            handle.abort();
        }
        if let Some(ref cache) = self.cache {
            let cache = cache.clone();
            let session_id = self.session_id;
            if let Ok(rt) = tokio::runtime::Handle::try_current() {
                rt.spawn(async move {
                    let _ = cache.remove_session(session_id).await;
                });
            }
        }
    }
}

/// Constructs a `ClientPacket::Pong` echoing `client_timestamp` with the current server UNIX epoch milliseconds.
pub fn build_pong_packet(client_timestamp: u64) -> ClientPacket {
    let server_timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    ClientPacket::Pong(PongPacket {
        client_timestamp,
        server_timestamp,
    })
}

/// Registers an unauthenticated (PoC) session in the router, subscribes it to `"chat:global"`,
/// and returns the resulting `ClientPacket::AuthResponse`.
pub fn handle_unauthenticated_auth_request(
    session_id: u64,
    router: &SessionRouter,
    default_node_address: Option<&str>,
    intercept_handler: Option<&GatewayInterceptHandler>,
) -> ClientPacket {
    let default_guid = Guid::new(EntityType::Player, 1, (session_id & 0xFFFF_FFFF) as u32);
    let assigned_node = default_node_address
        .unwrap_or("127.0.0.1:50052")
        .to_string();
    router.register_session(session_id, default_guid, assigned_node.clone());
    if let Some(handler) = intercept_handler {
        let _ = handler
            .subscription_manager
            .subscribe(session_id, "chat:global".to_string());
    }
    ClientPacket::AuthResponse(AuthResponsePacket {
        success: true,
        error_code: 0,
        session_id,
        player_guid: default_guid.raw(),
        assigned_node,
        error_message: String::new(),
    })
}

/// Runs edge packet interceptors, dispatches the packet to the session's assigned node, and forwards it.
pub async fn process_authenticated_packet(
    session_id: u64,
    packet: ClientPacket,
    dispatcher: &PacketDispatcher,
    packet_tx: &Option<mpsc::Sender<RoutedPacket>>,
    intercept_handler: Option<&GatewayInterceptHandler>,
) -> Result<(), GatewayError> {
    if let Some(handler) = intercept_handler {
        if handler.handle_packet(session_id, &packet).await {
            return Ok(());
        }
    }

    let routed = dispatcher.dispatch_packet(session_id, packet)?;
    if let Some(ref tx) = packet_tx {
        let _ = tx.send(routed).await;
    }
    Ok(())
}
