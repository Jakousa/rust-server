use std::sync::Arc;

use bytes::Bytes;
use dashmap::DashMap;
use tokio::sync::mpsc;

use super::dispatcher::DEFAULT_STREAM_CHANNEL_CAPACITY;
use crate::router::SessionRouter;
use crate::social::GatewaySubscriptionManager;
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
            tokio::spawn(async move {
                let _ = cache.remove_session(session_id).await;
            });
        }
    }
}
