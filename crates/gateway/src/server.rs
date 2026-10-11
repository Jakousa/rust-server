//! Edge Gateway Server and Packet Dispatcher.
//!
//! Handles client connection listening, binary packet framing via FlatBuffers /
//! `mmo_schema::client::ClientPacket::decode_frame`, extracting acting GUIDs / session IDs,
//! and dynamically dispatching packets to authoritative simulation nodes via `SessionRouter`.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use dashmap::DashMap;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use mmo_core::guid::Guid;
use mmo_schema::client::{ClientCodecError, ClientPacket};
use mmo_schema::cluster::director_service_client::DirectorServiceClient;
use mmo_schema::cluster::gateway_service_server::{GatewayService, GatewayServiceServer};
use mmo_schema::cluster::{
    BatchRouteUpdateRequest, BatchRouteUpdateResponse, GatewayHeartbeatRequest,
    MultiGatewayRouteBroadcastRequest, MultiGatewayRouteBroadcastResponse,
    ReassignNodeRoutesRequest, ReassignNodeRoutesResponse, RegisterGatewayRequest,
    RevokeGatewaySessionRequest, RevokeGatewaySessionResponse, RouteUpdateRequest,
    RouteUpdateResponse,
};
use mmo_storage::cache::{SessionCacheBackend, SessionData};
use mmo_storage::db::StorageError;

use crate::backhaul::BackhaulPool;
use crate::marketplace::GatewayMarketplaceService;
use crate::quic::{create_quic_server, generate_self_signed_cert, QuicTransportError};
use crate::router::SessionRouter;
use crate::social::GatewaySubscriptionManager;

/// Errors that can occur within the Edge Gateway server or packet dispatcher.
#[derive(Debug, Error)]
pub enum GatewayError {
    #[error("Packet codec error: {0}")]
    Codec(#[from] ClientCodecError),

    #[error("No authoritative route found for session {session_id}, guid: {guid:?}")]
    NoRouteFound { session_id: u64, guid: Option<Guid> },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Storage error: {0}")]
    Storage(#[from] StorageError),

    #[error("QUIC transport error: {0}")]
    Quic(#[from] QuicTransportError),

    #[error("Certificate error: {0}")]
    Certificate(#[from] rcgen::Error),

    #[error("Session {0} not found")]
    SessionNotFound(u64),

    #[error("Router lock poisoned")]
    LockPoisoned,

    #[error("Identity mismatch: packet acting GUID {actual:?} does not match session {session_id} authenticated GUID {expected:?}")]
    IdentityMismatch {
        session_id: u64,
        expected: Guid,
        actual: Guid,
    },

    #[error("Authentication error: {0}")]
    Auth(#[from] crate::auth::AuthError),
}

/// Represents a client packet that has been resolved and routed to an authoritative simulation node.
#[derive(Debug, Clone, PartialEq)]
pub struct RoutedPacket {
    /// Client transport session ID.
    pub session_id: u64,
    /// Acting player/entity GUID extracted from packet, if present.
    pub guid: Option<Guid>,
    /// Destination authoritative simulation node network address (e.g. "127.0.0.1:50051").
    pub node_address: String,
    /// The decoded client packet.
    pub packet: ClientPacket,
}

impl RoutedPacket {
    pub fn is_movement(&self) -> bool {
        matches!(self.packet, ClientPacket::Movement(_))
    }

    pub fn is_interaction(&self) -> bool {
        matches!(self.packet, ClientPacket::Interaction(_))
    }
}

/// Extracts the primary acting entity `Guid` from a `ClientPacket`, if applicable.
pub fn extract_acting_guid(packet: &ClientPacket) -> Option<Guid> {
    match packet {
        ClientPacket::Movement(p) => Some(Guid::from_raw(p.entity_id)),
        ClientPacket::MovementCorrection(p) => Some(Guid::from_raw(p.entity_id)),
        ClientPacket::Interaction(p) => Some(Guid::from_raw(p.source_guid)),
        ClientPacket::EntityState(p) => Some(Guid::from_raw(p.entity_id)),
        ClientPacket::EntityDespawn(p) => Some(Guid::from_raw(p.entity_id)),
        ClientPacket::SocialMessage(p) => Some(Guid::from_raw(p.sender_guid)),
        ClientPacket::InteractionResult(_)
        | ClientPacket::Ping(_)
        | ClientPacket::Pong(_)
        | ClientPacket::EntityDeltaBatch(_)
        | ClientPacket::ClientAck(_)
        | ClientPacket::WorldObjectSpawn(_)
        | ClientPacket::WorldObjectDelta(_)
        | ClientPacket::WorldObjectDespawn(_)
        | ClientPacket::WorldObjectResync(_)
        | ClientPacket::PlaceObject(_)
        | ClientPacket::InteractObject(_)
        | ClientPacket::AuthRequest(_)
        | ClientPacket::AuthResponse(_)
        | ClientPacket::SocialSubscribe(_)
        | ClientPacket::SocialUnsubscribe(_)
        | ClientPacket::MarketListRequest(_)
        | ClientPacket::MarketBuyRequest(_)
        | ClientPacket::MarketCancelRequest(_)
        | ClientPacket::MarketQueryRequest(_)
        | ClientPacket::MarketResponse(_) => None,
    }
}

/// Packet framer and dispatcher responsible for decoding streaming client buffers
/// and dynamically routing packets based on `SessionRouter` tables.
#[derive(Clone)]
pub struct PacketDispatcher {
    router: Arc<SessionRouter>,
}

impl PacketDispatcher {
    /// Creates a new `PacketDispatcher` backed by a shared lock-free `SessionRouter`.
    pub fn new(router: Arc<SessionRouter>) -> Self {
        Self { router }
    }

    /// Accesses the underlying shared router.
    pub fn router(&self) -> &Arc<SessionRouter> {
        &self.router
    }

    /// Dispatches a single client packet by resolving its destination node address.
    ///
    /// Resolution checks the acting `Guid` first, falling back to the `session_id`.
    pub fn dispatch_packet(
        &self,
        session_id: u64,
        packet: ClientPacket,
    ) -> Result<RoutedPacket, GatewayError> {
        Self::dispatch_with_router(&self.router, session_id, packet)
    }

    /// Dispatches a single client packet using a direct reference to a `SessionRouter`.
    pub fn dispatch_with_router(
        router: &SessionRouter,
        session_id: u64,
        packet: ClientPacket,
    ) -> Result<RoutedPacket, GatewayError> {
        let guid = extract_acting_guid(&packet);

        // Anti-Spoofing: If the packet contains an acting GUID, it must strictly match
        // the session's authenticated GUID in the routing table (if bound).
        if let Some(acting) = guid {
            if let Some(session_guid) = router.get_guid_by_session(session_id) {
                if acting != session_guid {
                    metrics::counter!("mmo_gateway_spoofed_packets_dropped_total").increment(1);
                    return Err(GatewayError::IdentityMismatch {
                        session_id,
                        expected: session_guid,
                        actual: acting,
                    });
                }
            }
        }

        let node_address = guid
            .and_then(|g| router.get_route_by_guid(g))
            .or_else(|| router.get_route_by_session(session_id));

        match node_address {
            Some(addr) => Ok(RoutedPacket {
                session_id,
                guid,
                node_address: addr,
                packet,
            }),
            None => Err(GatewayError::NoRouteFound { session_id, guid }),
        }
    }

    /// Frames and decodes all complete client packets currently in `buffer` using
    /// `ClientPacket::decode_frame`, and dispatches each to its destination node address.
    ///
    /// Incomplete frames remain in `buffer` for subsequent reads.
    pub fn process_frames(
        &self,
        session_id: u64,
        buffer: &mut BytesMut,
    ) -> Result<Vec<RoutedPacket>, GatewayError> {
        Self::process_frames_with_router(&self.router, session_id, buffer)
    }

    /// Frames and decodes client packets directly against a given `SessionRouter`.
    pub fn process_frames_with_router(
        router: &SessionRouter,
        session_id: u64,
        buffer: &mut BytesMut,
    ) -> Result<Vec<RoutedPacket>, GatewayError> {
        let mut routed = Vec::new();
        while let Some(packet) = ClientPacket::decode_frame(buffer).map_err(GatewayError::Codec)? {
            let dispatched = Self::dispatch_with_router(router, session_id, packet)?;
            routed.push(dispatched);
        }
        Ok(routed)
    }
}

/// Default bounded capacity for reliable session stream queues.
pub const DEFAULT_STREAM_CHANNEL_CAPACITY: usize = 1024;

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
}

impl Drop for SessionDropGuard {
    fn drop(&mut self) {
        let _ = self.router.remove_session(self.session_id);
        self.session_sinks.remove(&self.session_id);
        metrics::gauge!("mmo_gateway_active_sessions").set(self.session_sinks.len() as f64);
        if let Some(ref sub_mgr) = self.subscription_manager {
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

/// Intercept handler for social pub/sub messaging and asynchronous marketplace transactions.
///
/// Packets matching social channels (chat, whispers, topic subscriptions) or marketplace
/// actions (listing, buying, cancelling, querying) are intercepted at the edge gateway layer
/// and completely bypass downstream simulation nodes.
#[derive(Clone)]
pub struct GatewayInterceptHandler {
    pub router: Arc<SessionRouter>,
    pub subscription_manager: Arc<GatewaySubscriptionManager>,
    pub marketplace_service: Option<Arc<GatewayMarketplaceService>>,
    pub social_bus: Option<Arc<dyn mmo_storage::SocialBus>>,
    pub session_sinks: Arc<DashMap<u64, SessionSink>>,
    pub active_bus_subscriptions: Arc<DashMap<String, tokio::task::JoinHandle<()>>>,
}

impl GatewayInterceptHandler {
    /// Creates a new `GatewayInterceptHandler`.
    pub fn new(
        router: Arc<SessionRouter>,
        subscription_manager: Arc<GatewaySubscriptionManager>,
        marketplace_service: Option<Arc<GatewayMarketplaceService>>,
        social_bus: Option<Arc<dyn mmo_storage::SocialBus>>,
        session_sinks: Arc<DashMap<u64, SessionSink>>,
        active_bus_subscriptions: Arc<DashMap<String, tokio::task::JoinHandle<()>>>,
    ) -> Self {
        Self {
            router,
            subscription_manager,
            marketplace_service,
            social_bus,
            session_sinks,
            active_bus_subscriptions,
        }
    }

    /// Subscribes a session to a named topic and ensures a background bus listener is active if needed.
    pub async fn subscribe_session_topic(&self, session_id: u64, topic: String) {
        let is_new = self
            .subscription_manager
            .subscribe(session_id, topic.clone());
        if is_new {
            self.ensure_bus_listener(&topic).await;
        }
    }

    /// Ensures a background listener task is subscribed to `topic` on `social_bus`.
    pub async fn ensure_bus_listener(&self, topic: &str) {
        if let Some(ref bus) = self.social_bus {
            if self.active_bus_subscriptions.contains_key(topic) {
                return;
            }
            if let Ok(mut rx) = bus.subscribe(topic).await {
                let topic_name = topic.to_string();
                let sub_mgr = self.subscription_manager.clone();
                let sinks = self.session_sinks.clone();
                let handle = tokio::spawn(async move {
                    while let Ok(envelope) = rx.recv().await {
                        if let mmo_storage::SocialEnvelope::Chat(chat) = envelope {
                            let packet = ClientPacket::SocialMessage(
                                mmo_schema::client::SocialMessagePacket {
                                    channel_type: match chat.channel {
                                        mmo_storage::ChatChannel::Global => 0,
                                        mmo_storage::ChatChannel::Whisper => 1,
                                        mmo_storage::ChatChannel::Party => 2,
                                        mmo_storage::ChatChannel::Guild => 3,
                                        mmo_storage::ChatChannel::Area => 4,
                                    },
                                    target_id: chat.target_guid.unwrap_or(0),
                                    sender_guid: chat.sender_guid,
                                    sender_name: chat.sender_name,
                                    content: chat.content,
                                    timestamp: chat.timestamp,
                                },
                            );
                            let encoded = Bytes::from(packet.encode());
                            let subscribers = sub_mgr.get_subscribers(&topic_name);
                            for sid in subscribers {
                                if let Some(sink) = sinks.get(&sid) {
                                    if sink.stream_tx.try_send(encoded.clone()).is_err() {
                                        metrics::counter!(
                                            "mmo_gateway_session_stream_full_dropped"
                                        )
                                        .increment(1);
                                    }
                                }
                            }
                        }
                    }
                });
                self.active_bus_subscriptions
                    .insert(topic.to_string(), handle);
            }
        }
    }

    /// Cleanly deregisters all subscriptions for `session_id`, terminating idle whisper listeners.
    pub fn remove_session(&self, session_id: u64) {
        let removed = self.subscription_manager.remove_session(session_id);
        for topic in removed {
            if topic.starts_with("whisper:")
                && self.subscription_manager.get_subscribers(&topic).is_empty()
            {
                if let Some((_, handle)) = self.active_bus_subscriptions.remove(&topic) {
                    handle.abort();
                }
            }
        }
    }

    /// Sends a packet directly to a session's reliable stream sink.
    pub fn send_packet_to_session(&self, session_id: u64, packet: &ClientPacket) {
        if let Some(sink) = self.session_sinks.get(&session_id) {
            let encoded = Bytes::from(packet.encode());
            if sink.stream_tx.try_send(encoded).is_err() {
                metrics::counter!("mmo_gateway_session_stream_full_dropped").increment(1);
            }
        }
    }

    /// Attempts to intercept and process a packet if it belongs to social pub/sub or marketplace.
    ///
    /// Returns `true` if intercepted (do not route to simulation node), `false` otherwise.
    pub async fn handle_packet(&self, session_id: u64, packet: &ClientPacket) -> bool {
        match packet {
            ClientPacket::SocialMessage(msg) => {
                // Anti-Spoofing: If the session has an authenticated player GUID, sender_guid must match.
                if let Some(bound_guid) = self.router.get_guid_by_session(session_id) {
                    if msg.sender_guid != bound_guid.raw() {
                        metrics::counter!("mmo_gateway_spoofed_social_packets_dropped_total")
                            .increment(1);
                        return true;
                    }
                }

                let topic = match msg.channel_type {
                    0 => "chat:global".to_string(),
                    1 => format!("whisper:{}", msg.target_id),
                    2 => format!("chat:party:{}", msg.target_id),
                    3 => format!("chat:guild:{}", msg.target_id),
                    4 => "chat:area".to_string(),
                    _ => format!("custom:{}", msg.target_id),
                };

                let chat_channel = match msg.channel_type {
                    0 => mmo_storage::ChatChannel::Global,
                    1 => mmo_storage::ChatChannel::Whisper,
                    2 => mmo_storage::ChatChannel::Party,
                    3 => mmo_storage::ChatChannel::Guild,
                    4 => mmo_storage::ChatChannel::Area,
                    _ => mmo_storage::ChatChannel::Global,
                };

                let chat_msg = mmo_storage::ChatMessage {
                    channel: chat_channel,
                    sender_guid: msg.sender_guid,
                    sender_name: msg.sender_name.clone(),
                    target_guid: if msg.target_id > 0 {
                        Some(msg.target_id)
                    } else {
                        None
                    },
                    content: msg.content.clone(),
                    timestamp: msg.timestamp,
                };

                if let Some(ref bus) = self.social_bus {
                    let _ = bus
                        .publish(&topic, &mmo_storage::SocialEnvelope::Chat(chat_msg))
                        .await;
                } else {
                    let encoded = Bytes::from(packet.encode());
                    let subscribers = self.subscription_manager.get_subscribers(&topic);
                    for sid in subscribers {
                        if let Some(sink) = self.session_sinks.get(&sid) {
                            if sink.stream_tx.try_send(encoded.clone()).is_err() {
                                metrics::counter!("mmo_gateway_session_stream_full_dropped")
                                    .increment(1);
                            }
                        }
                    }
                }
                true
            }
            ClientPacket::SocialSubscribe(sub) => {
                let topic = if !sub.channel_name.is_empty() {
                    sub.channel_name.clone()
                } else {
                    match sub.channel_type {
                        0 => "chat:global".to_string(),
                        1 => format!("whisper:{}", sub.target_id),
                        2 => format!("chat:party:{}", sub.target_id),
                        3 => format!("chat:guild:{}", sub.target_id),
                        4 => "chat:area".to_string(),
                        _ => format!("custom:{}", sub.target_id),
                    }
                };
                self.subscribe_session_topic(session_id, topic).await;
                true
            }
            ClientPacket::SocialUnsubscribe(unsub) => {
                let topic = if !unsub.channel_name.is_empty() {
                    unsub.channel_name.clone()
                } else {
                    match unsub.channel_type {
                        0 => "chat:global".to_string(),
                        1 => format!("whisper:{}", unsub.target_id),
                        2 => format!("chat:party:{}", unsub.target_id),
                        3 => format!("chat:guild:{}", unsub.target_id),
                        4 => "chat:area".to_string(),
                        _ => format!("custom:{}", unsub.target_id),
                    }
                };
                self.subscription_manager.unsubscribe(session_id, &topic);
                true
            }
            ClientPacket::MarketListRequest(req) => {
                let player_guid = self.router.get_guid_by_session(session_id).map(|g| g.raw());
                let resp = match (player_guid, &self.marketplace_service) {
                    (Some(guid), Some(service)) => service.handle_list(guid, req.clone()).await,
                    (None, _) => mmo_schema::client::MarketResponsePacket {
                        request_type: 1,
                        success: false,
                        error_code: mmo_schema::client::market_error_codes::UNAUTHORIZED,
                        error_message: "Unauthenticated session".to_string(),
                        listings: Vec::new(),
                    },
                    (_, None) => mmo_schema::client::MarketResponsePacket {
                        request_type: 1,
                        success: false,
                        error_code: mmo_schema::client::market_error_codes::INTERNAL_ERROR,
                        error_message: "Marketplace service unavailable".to_string(),
                        listings: Vec::new(),
                    },
                };
                self.send_packet_to_session(session_id, &ClientPacket::MarketResponse(resp));
                true
            }
            ClientPacket::MarketBuyRequest(req) => {
                let player_guid = self.router.get_guid_by_session(session_id).map(|g| g.raw());
                let resp = match (player_guid, &self.marketplace_service) {
                    (Some(guid), Some(service)) => service.handle_buy(guid, req.clone()).await,
                    (None, _) => mmo_schema::client::MarketResponsePacket {
                        request_type: 2,
                        success: false,
                        error_code: mmo_schema::client::market_error_codes::UNAUTHORIZED,
                        error_message: "Unauthenticated session".to_string(),
                        listings: Vec::new(),
                    },
                    (_, None) => mmo_schema::client::MarketResponsePacket {
                        request_type: 2,
                        success: false,
                        error_code: mmo_schema::client::market_error_codes::INTERNAL_ERROR,
                        error_message: "Marketplace service unavailable".to_string(),
                        listings: Vec::new(),
                    },
                };
                self.send_packet_to_session(session_id, &ClientPacket::MarketResponse(resp));
                true
            }
            ClientPacket::MarketCancelRequest(req) => {
                let player_guid = self.router.get_guid_by_session(session_id).map(|g| g.raw());
                let resp = match (player_guid, &self.marketplace_service) {
                    (Some(guid), Some(service)) => service.handle_cancel(guid, req.clone()).await,
                    (None, _) => mmo_schema::client::MarketResponsePacket {
                        request_type: 3,
                        success: false,
                        error_code: mmo_schema::client::market_error_codes::UNAUTHORIZED,
                        error_message: "Unauthenticated session".to_string(),
                        listings: Vec::new(),
                    },
                    (_, None) => mmo_schema::client::MarketResponsePacket {
                        request_type: 3,
                        success: false,
                        error_code: mmo_schema::client::market_error_codes::INTERNAL_ERROR,
                        error_message: "Marketplace service unavailable".to_string(),
                        listings: Vec::new(),
                    },
                };
                self.send_packet_to_session(session_id, &ClientPacket::MarketResponse(resp));
                true
            }
            ClientPacket::MarketQueryRequest(req) => {
                let resp = match &self.marketplace_service {
                    Some(service) => service.handle_query(req.clone()).await,
                    None => mmo_schema::client::MarketResponsePacket {
                        request_type: 4,
                        success: false,
                        error_code: mmo_schema::client::market_error_codes::INTERNAL_ERROR,
                        error_message: "Marketplace service unavailable".to_string(),
                        listings: Vec::new(),
                    },
                };
                self.send_packet_to_session(session_id, &ClientPacket::MarketResponse(resp));
                true
            }
            ClientPacket::MarketResponse(_) => {
                // Intercept and drop any unsolicited MarketResponse from client
                true
            }
            _ => false,
        }
    }
}

/// gRPC service implementation for dynamic Edge Gateway route updates.
#[derive(Clone)]
pub struct GatewayServiceImpl {
    router: Arc<SessionRouter>,
    session_sinks: Option<Arc<DashMap<u64, SessionSink>>>,
}

impl GatewayServiceImpl {
    /// Creates a new `GatewayServiceImpl` backed by the given `SessionRouter`.
    pub fn new(router: Arc<SessionRouter>) -> Self {
        Self {
            router,
            session_sinks: None,
        }
    }

    /// Creates a new `GatewayServiceImpl` with router and session sinks.
    pub fn with_sinks(
        router: Arc<SessionRouter>,
        session_sinks: Arc<DashMap<u64, SessionSink>>,
    ) -> Self {
        Self {
            router,
            session_sinks: Some(session_sinks),
        }
    }

    /// Wraps this implementation in a Tonic gRPC `GatewayServiceServer`.
    pub fn into_server(self) -> GatewayServiceServer<Self> {
        GatewayServiceServer::new(self)
    }
}

#[tonic::async_trait]
impl GatewayService for GatewayServiceImpl {
    async fn update_route(
        &self,
        request: tonic::Request<RouteUpdateRequest>,
    ) -> Result<tonic::Response<RouteUpdateResponse>, tonic::Status> {
        let req = request.into_inner();
        let guid = Guid::from_raw(req.guid);
        self.router
            .update_route_with_session(guid, req.session_id, req.node_address);
        metrics::counter!("mmo_gateway_route_updates_total").increment(1);
        Ok(tonic::Response::new(RouteUpdateResponse {
            success: true,
            error_message: String::new(),
        }))
    }

    async fn update_batch_routes(
        &self,
        request: tonic::Request<BatchRouteUpdateRequest>,
    ) -> Result<tonic::Response<BatchRouteUpdateResponse>, tonic::Status> {
        let req = request.into_inner();
        let mut count = 0;
        for u in req.updates {
            let guid = Guid::from_raw(u.guid);
            self.router
                .update_route_with_session(guid, u.session_id, u.node_address);
            count += 1;
        }
        metrics::counter!("mmo_gateway_batch_route_updates_total").increment(1);
        metrics::counter!("mmo_gateway_route_updates_total").increment(count as u64);
        Ok(tonic::Response::new(BatchRouteUpdateResponse {
            success: true,
            updated_count: count as u32,
            error_message: String::new(),
        }))
    }

    async fn reassign_node_routes(
        &self,
        request: tonic::Request<ReassignNodeRoutesRequest>,
    ) -> Result<tonic::Response<ReassignNodeRoutesResponse>, tonic::Status> {
        let req = request.into_inner();
        let count = self
            .router
            .reassign_node_routes(&req.dead_node_address, &req.new_node_address);
        metrics::counter!("mmo_gateway_route_reassignments_total").increment(count as u64);
        Ok(tonic::Response::new(ReassignNodeRoutesResponse {
            success: true,
            reassigned_count: count as u32,
        }))
    }

    async fn revoke_gateway_session(
        &self,
        request: tonic::Request<RevokeGatewaySessionRequest>,
    ) -> Result<tonic::Response<RevokeGatewaySessionResponse>, tonic::Status> {
        let req = request.into_inner();
        let session_id = req.session_id;
        let _ = self.router.remove_session(session_id);
        if let Some(ref sinks) = self.session_sinks {
            sinks.remove(&session_id);
            metrics::gauge!("mmo_gateway_active_sessions").set(sinks.len() as f64);
        }
        metrics::counter!("mmo_gateway_revoked_sessions_total").increment(1);
        Ok(tonic::Response::new(RevokeGatewaySessionResponse {
            success: true,
        }))
    }

    async fn broadcast_route(
        &self,
        request: tonic::Request<MultiGatewayRouteBroadcastRequest>,
    ) -> Result<tonic::Response<MultiGatewayRouteBroadcastResponse>, tonic::Status> {
        let req = request.into_inner();
        let guid = Guid::from_raw(req.guid);
        self.router
            .update_route_with_session(guid, req.session_id, req.new_node_address);
        metrics::counter!("mmo_gateway_broadcast_routes_received_total").increment(1);
        Ok(tonic::Response::new(MultiGatewayRouteBroadcastResponse {
            success: true,
            acknowledged_gateways: 1,
        }))
    }
}

/// Gateway server managing connection lifecycles, streaming packet framing,
/// dynamic routing to authoritative simulation nodes, and optional distributed session caching.
pub struct GatewayServer {
    router: Arc<SessionRouter>,
    dispatcher: PacketDispatcher,
    session_counter: Arc<AtomicU64>,
    packet_tx: Option<mpsc::Sender<RoutedPacket>>,
    cache: Option<Arc<dyn SessionCacheBackend>>,
    social_bus: Option<Arc<dyn mmo_storage::SocialBus>>,
    session_sinks: Arc<DashMap<u64, SessionSink>>,
    backhaul_pool: Arc<BackhaulPool>,
    authenticator: Option<Arc<crate::auth::EdgeAuthenticator>>,
    default_node_address: Option<String>,
    subscription_manager: Arc<GatewaySubscriptionManager>,
    marketplace_service: Option<Arc<GatewayMarketplaceService>>,
    active_bus_subscriptions: Arc<DashMap<String, tokio::task::JoinHandle<()>>>,
    handshake_timeout: Option<Duration>,
}

impl GatewayServer {
    /// Creates a new `GatewayServer` with the given routing table.
    pub fn new(router: Arc<SessionRouter>) -> Self {
        metrics::gauge!("mmo_gateway_active_sessions").set(0.0);
        let dispatcher = PacketDispatcher::new(router.clone());
        Self {
            router,
            dispatcher,
            session_counter: Arc::new(AtomicU64::new(1)),
            packet_tx: None,
            cache: None,
            social_bus: None,
            session_sinks: Arc::new(DashMap::new()),
            backhaul_pool: Arc::new(BackhaulPool::new(10000)),
            authenticator: None,
            default_node_address: None,
            subscription_manager: Arc::new(GatewaySubscriptionManager::new()),
            marketplace_service: None,
            active_bus_subscriptions: Arc::new(DashMap::new()),
            handshake_timeout: None,
        }
    }

    /// Creates a new `GatewayServer` that emits all routed packets into an `mpsc::Sender`.
    pub fn with_packet_channel(
        router: Arc<SessionRouter>,
        packet_tx: mpsc::Sender<RoutedPacket>,
    ) -> Self {
        metrics::gauge!("mmo_gateway_active_sessions").set(0.0);
        let dispatcher = PacketDispatcher::new(router.clone());
        Self {
            router,
            dispatcher,
            session_counter: Arc::new(AtomicU64::new(1)),
            packet_tx: Some(packet_tx),
            cache: None,
            social_bus: None,
            session_sinks: Arc::new(DashMap::new()),
            backhaul_pool: Arc::new(BackhaulPool::new(10000)),
            authenticator: None,
            default_node_address: None,
            subscription_manager: Arc::new(GatewaySubscriptionManager::new()),
            marketplace_service: None,
            active_bus_subscriptions: Arc::new(DashMap::new()),
            handshake_timeout: None,
        }
    }

    /// Configures an edge authenticator for admission control and identity verification.
    pub fn with_authenticator(
        mut self,
        authenticator: Arc<crate::auth::EdgeAuthenticator>,
    ) -> Self {
        self.authenticator = Some(authenticator);
        self
    }

    /// Sets a default simulation node address for initial session assignment when no director is available.
    pub fn with_default_node_address(mut self, node_address: impl Into<String>) -> Self {
        self.default_node_address = Some(node_address.into());
        self
    }

    /// Accesses the edge authenticator, if configured.
    pub fn authenticator(&self) -> Option<&Arc<crate::auth::EdgeAuthenticator>> {
        self.authenticator.as_ref()
    }

    /// Accesses the default simulation node address, if configured.
    pub fn default_node_address(&self) -> Option<&str> {
        self.default_node_address.as_deref()
    }

    /// Sets an unauthenticated handshake timeout (default: 10 seconds) for Slowloris mitigation.
    pub fn with_handshake_timeout(mut self, timeout: Duration) -> Self {
        self.handshake_timeout = Some(timeout);
        self
    }

    /// Returns the configured unauthenticated handshake timeout (defaults to 10 seconds).
    pub fn handshake_timeout(&self) -> Duration {
        self.handshake_timeout.unwrap_or(Duration::from_secs(10))
    }

    /// Resolves the destination simulation node address for a player during initial authentication or handover.
    pub async fn resolve_initial_node(&self, guid: Guid) -> Result<String, GatewayError> {
        if let Some(route) = self.router.get_route_by_guid(guid) {
            return Ok(route);
        }

        if let Some(session_id) = self.router.get_session_by_guid(guid) {
            if let Some(route) = self.resolve_route_or_cache(session_id).await? {
                return Ok(route);
            }
        }

        if let Some(ref default_node) = self.default_node_address {
            return Ok(default_node.clone());
        }

        Err(GatewayError::NoRouteFound {
            session_id: 0,
            guid: Some(guid),
        })
    }

    /// Accesses the persistent backhaul connection pool.
    pub fn backhaul_pool(&self) -> &Arc<BackhaulPool> {
        &self.backhaul_pool
    }

    /// Sets an optional distributed session cache backend.
    pub fn with_cache(mut self, cache: Arc<dyn SessionCacheBackend>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Sets an optional social pub/sub message bus backend.
    pub fn with_social_bus(mut self, social_bus: Arc<dyn mmo_storage::SocialBus>) -> Self {
        self.social_bus = Some(social_bus);
        self
    }

    /// Accesses the social bus, if configured.
    pub fn social_bus(&self) -> Option<&Arc<dyn mmo_storage::SocialBus>> {
        self.social_bus.as_ref()
    }

    /// Publishes a chat message to the social bus.
    pub async fn dispatch_chat(
        &self,
        message: mmo_storage::ChatMessage,
    ) -> Result<(), mmo_storage::SocialBusError> {
        if let Some(bus) = &self.social_bus {
            let topic = match message.channel {
                mmo_storage::ChatChannel::Global => "chat:global".to_string(),
                mmo_storage::ChatChannel::Area => "chat:area".to_string(),
                mmo_storage::ChatChannel::Guild => {
                    format!("chat:guild:{}", message.target_guid.unwrap_or(0))
                }
                mmo_storage::ChatChannel::Party => {
                    format!("chat:party:{}", message.target_guid.unwrap_or(0))
                }
                mmo_storage::ChatChannel::Whisper => {
                    format!("whisper:{}", message.target_guid.unwrap_or(0))
                }
            };
            bus.publish(&topic, &mmo_storage::SocialEnvelope::Chat(message))
                .await?;
        }
        Ok(())
    }

    /// Accesses the underlying `SessionRouter`.
    pub fn router(&self) -> &Arc<SessionRouter> {
        &self.router
    }

    /// Configures a custom gateway subscription manager.
    pub fn with_subscription_manager(
        mut self,
        subscription_manager: Arc<GatewaySubscriptionManager>,
    ) -> Self {
        self.subscription_manager = subscription_manager;
        self
    }

    /// Configures an asynchronous marketplace service operating at the edge gateway.
    pub fn with_marketplace(mut self, marketplace_service: Arc<GatewayMarketplaceService>) -> Self {
        self.marketplace_service = Some(marketplace_service);
        self
    }

    /// Accesses the gateway subscription manager.
    pub fn subscription_manager(&self) -> &Arc<GatewaySubscriptionManager> {
        &self.subscription_manager
    }

    /// Accesses the marketplace service, if configured.
    pub fn marketplace_service(&self) -> Option<&Arc<GatewayMarketplaceService>> {
        self.marketplace_service.as_ref()
    }

    /// Constructs a `GatewayInterceptHandler` capturing current routing, subscription, and marketplace services.
    pub fn intercept_handler(&self) -> GatewayInterceptHandler {
        GatewayInterceptHandler {
            router: self.router.clone(),
            subscription_manager: self.subscription_manager.clone(),
            marketplace_service: self.marketplace_service.clone(),
            social_bus: self.social_bus.clone(),
            session_sinks: self.session_sinks.clone(),
            active_bus_subscriptions: self.active_bus_subscriptions.clone(),
        }
    }

    /// Creates a [`GatewayServiceImpl`] backed by this server's routing table and session sinks.
    pub fn create_service(&self) -> GatewayServiceImpl {
        GatewayServiceImpl::with_sinks(Arc::clone(&self.router), Arc::clone(&self.session_sinks))
    }

    /// Spawns a background task registering this gateway with the Director and emitting periodic heartbeats.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_heartbeat_loop(
        director_addr: String,
        gateway_id: String,
        region: String,
        listen_addr: String,
        grpc_addr: String,
        probe_addr: String,
        capacity: u32,
        session_sinks: Arc<DashMap<u64, SessionSink>>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut client = None;
            let mut attempts = 0;
            while client.is_none() && attempts < 10 {
                match DirectorServiceClient::connect(director_addr.clone()).await {
                    Ok(c) => client = Some(c),
                    Err(_) => {
                        attempts += 1;
                        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    }
                }
            }

            let mut client = match client {
                Some(c) => c,
                None => {
                    eprintln!("[Gateway] Failed to connect to Director at {director_addr}");
                    return;
                }
            };

            let reg_req = RegisterGatewayRequest {
                gateway_id: gateway_id.clone(),
                region,
                listen_addr,
                grpc_addr,
                probe_addr,
                capacity,
            };

            let mut interval_ms = 1000;
            if let Ok(res) = client.register_gateway(reg_req).await {
                interval_ms = res.into_inner().heartbeat_interval_ms.max(200);
            }

            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(interval_ms)).await;
                let active = session_sinks.len() as u32;
                let hb = GatewayHeartbeatRequest {
                    gateway_id: gateway_id.clone(),
                    active_sessions: active,
                    cpu_percent: 0.0,
                    memory_percent: 0.0,
                    timestamp_ms: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0),
                };
                if client.gateway_heartbeat(hb).await.is_err() {
                    // Try reconnecting next loop
                    if let Ok(c) = DirectorServiceClient::connect(director_addr.clone()).await {
                        client = c;
                    }
                }
            }
        })
    }

    /// Accesses the session sinks map.
    pub fn session_sinks(&self) -> Arc<DashMap<u64, SessionSink>> {
        Arc::clone(&self.session_sinks)
    }

    /// Accesses the packet dispatcher.
    pub fn dispatcher(&self) -> &PacketDispatcher {
        &self.dispatcher
    }

    /// Accesses the session cache backend, if configured.
    pub fn cache(&self) -> Option<&Arc<dyn SessionCacheBackend>> {
        self.cache.as_ref()
    }

    /// Generates the next monotonic unique session ID.
    pub fn next_session_id(&self) -> u64 {
        self.session_counter.fetch_add(1, Ordering::Relaxed)
    }

    /// Registers a session in the gateway routing table.
    pub fn register_session(
        &self,
        session_id: u64,
        guid: Guid,
        node_address: String,
    ) -> Result<(), GatewayError> {
        self.router.register_session(session_id, guid, node_address);
        Ok(())
    }

    /// Registers a fallback route for a session in the gateway routing table.
    pub fn register_session_fallback(
        &self,
        session_id: u64,
        node_address: String,
    ) -> Result<(), GatewayError> {
        self.router
            .register_session_fallback(session_id, node_address);
        Ok(())
    }

    /// Registers a session in the gateway routing table, and updates the distributed session cache if configured.
    pub async fn register_session_with_cache(
        &self,
        session_id: u64,
        guid: Guid,
        node_address: String,
        ttl_secs: u64,
    ) -> Result<(), GatewayError> {
        self.register_session(session_id, guid, node_address.clone())?;
        if let Some(ref cache) = self.cache {
            let session_data = SessionData {
                session_id,
                guid: guid.raw(),
                node_address,
                expires_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
                    + ttl_secs,
            };
            cache.set_session(&session_data, ttl_secs).await?;
        }
        Ok(())
    }

    /// Updates the destination route for a player during seamless node handovers.
    pub fn update_route(&self, guid: Guid, new_node_address: String) -> Result<bool, GatewayError> {
        Ok(self.router.update_route(guid, new_node_address))
    }

    /// Resolves the current authoritative route for a player GUID.
    pub fn get_route_by_guid(&self, guid: Guid) -> Result<Option<String>, GatewayError> {
        Ok(self.router.get_route_by_guid(guid))
    }

    /// Resolves the current authoritative route for a session ID.
    pub fn get_route_by_session(&self, session_id: u64) -> Result<Option<String>, GatewayError> {
        Ok(self.router.get_route_by_session(session_id))
    }

    /// Resolves the authoritative route for a session ID. Checks local router first,
    /// falling back to querying the distributed session cache (and synchronizing into the local router if found).
    pub async fn resolve_route_or_cache(
        &self,
        session_id: u64,
    ) -> Result<Option<String>, GatewayError> {
        if let Some(addr) = self.get_route_by_session(session_id)? {
            return Ok(Some(addr));
        }

        if let Some(ref cache) = self.cache {
            if let Some(data) = cache.get_session(session_id).await? {
                let guid = Guid::from_raw(data.guid);
                let addr = data.node_address.clone();
                self.register_session(session_id, guid, addr.clone())?;
                return Ok(Some(addr));
            }
        }

        Ok(None)
    }

    /// Removes a session from the routing table.
    pub fn remove_session(&self, session_id: u64) -> Result<Option<Guid>, GatewayError> {
        Ok(self.router.remove_session(session_id))
    }

    /// Removes a session synchronously without allocating or erroring.
    pub fn remove_session_sync(&self, session_id: u64) -> Option<Guid> {
        self.router.remove_session(session_id)
    }

    /// Removes a session from the local routing table and distributed cache.
    pub async fn remove_session_with_cache(
        &self,
        session_id: u64,
    ) -> Result<Option<Guid>, GatewayError> {
        let guid = self.remove_session(session_id)?;
        if let Some(ref cache) = self.cache {
            let _ = cache.remove_session(session_id).await?;
        }
        Ok(guid)
    }

    /// Registers a downstream packet sink for a client session.
    pub fn register_session_sink(
        &self,
        session_id: u64,
        sink: impl Into<SessionSink>,
    ) -> Result<(), GatewayError> {
        self.session_sinks.insert(session_id, sink.into());
        metrics::gauge!("mmo_gateway_active_sessions").set(self.session_sinks.len() as f64);
        Ok(())
    }

    /// Removes a downstream packet sink for a client session.
    pub fn remove_session_sink(
        &self,
        session_id: u64,
    ) -> Result<Option<SessionSink>, GatewayError> {
        let res = self.session_sinks.remove(&session_id).map(|(_, s)| s);
        metrics::gauge!("mmo_gateway_active_sessions").set(self.session_sinks.len() as f64);
        Ok(res)
    }

    /// Returns the number of active downstream session sinks.
    pub fn active_sink_count(&self) -> Result<usize, GatewayError> {
        Ok(self.session_sinks.len())
    }

    /// Checks if a downstream session sink is registered for the given session ID.
    pub fn has_session_sink(&self, session_id: u64) -> Result<bool, GatewayError> {
        Ok(self.session_sinks.contains_key(&session_id))
    }

    /// Sends raw framed bytes downstream to a specific client session without blocking.
    ///
    /// Ephemeral packets (e.g. movement, entity deltas) are routed to the bounded datagram channel
    /// (dropping on saturation), while reliable packets (e.g. interactions, trades, auth) are routed
    /// to the unbounded reliable stream channel (never dropped under backpressure).
    pub async fn send_bytes_to_session(
        &self,
        session_id: u64,
        bytes: Bytes,
    ) -> Result<bool, GatewayError> {
        if let Some(entry) = self.session_sinks.get(&session_id) {
            let byte_len = bytes.len() as u64;
            let sink = entry.value();
            let is_ephemeral = is_ephemeral_packet(&bytes);
            let sent = if is_ephemeral {
                match sink.datagram_tx.try_send(bytes) {
                    Ok(_) => true,
                    Err(_) => {
                        metrics::counter!("mmo_gateway_packets_dropped_total").increment(1);
                        false
                    }
                }
            } else {
                match sink.stream_tx.try_send(bytes) {
                    Ok(_) => true,
                    Err(_) => {
                        metrics::counter!("mmo_gateway_session_stream_full_dropped").increment(1);
                        metrics::counter!("mmo_gateway_packets_dropped_total").increment(1);
                        false
                    }
                }
            };
            if sent {
                metrics::counter!("mmo_gateway_bytes_sent_total").increment(byte_len);
            }
            Ok(sent)
        } else {
            Ok(false)
        }
    }

    /// Encodes and sends a `ClientPacket` downstream to a specific client session.
    pub async fn send_packet_to_session(
        &self,
        session_id: u64,
        packet: &ClientPacket,
    ) -> Result<bool, GatewayError> {
        metrics::counter!("mmo_gateway_packets_sent_total", "opcode" => packet.opcode().as_str())
            .increment(1);
        let encoded = Bytes::from(packet.encode());
        self.send_bytes_to_session(session_id, encoded).await
    }

    /// Broadcasts a `ClientPacket` non-blockingly to all connected client sessions.
    pub async fn broadcast_packet(&self, packet: &ClientPacket) -> Result<usize, GatewayError> {
        let encoded = Bytes::from(packet.encode());
        let byte_len = encoded.len() as u64;
        let is_ephemeral = is_ephemeral_packet(&encoded);
        let mut sent_count = 0;
        for entry in self.session_sinks.iter() {
            let sink = entry.value();
            let success = if is_ephemeral {
                sink.datagram_tx.try_send(encoded.clone()).is_ok()
            } else {
                let res = sink.stream_tx.try_send(encoded.clone());
                if res.is_err() {
                    metrics::counter!("mmo_gateway_session_stream_full_dropped").increment(1);
                }
                res.is_ok()
            };
            if success {
                sent_count += 1;
            } else {
                metrics::counter!("mmo_gateway_packets_dropped_total").increment(1);
            }
        }
        if sent_count > 0 {
            metrics::counter!("mmo_gateway_packets_sent_total", "opcode" => packet.opcode().as_str())
                .increment(sent_count as u64);
            metrics::counter!("mmo_gateway_bytes_sent_total")
                .increment(byte_len * sent_count as u64);
        }
        Ok(sent_count)
    }

    /// Binds a TCP connection listener to the specified address.
    pub async fn bind(&self, addr: &str) -> Result<TcpListener, GatewayError> {
        TcpListener::bind(addr).await.map_err(GatewayError::Io)
    }

    /// Handles an active client connection socket:
    /// Splits the stream into concurrent read and write tasks, streams incoming bytes
    /// into a framing buffer, decodes client packets, and forwards downstream broadcasts.
    pub async fn handle_connection(
        &self,
        socket: TcpStream,
        session_id: u64,
    ) -> Result<(), GatewayError> {
        let (mut read_half, mut write_half) = socket.into_split();
        let (datagram_tx, mut datagram_rx) = mpsc::channel::<Bytes>(1024);
        let (stream_tx, mut stream_rx) = mpsc::channel::<Bytes>(DEFAULT_STREAM_CHANNEL_CAPACITY);

        self.register_session_sink(session_id, SessionSink::new(datagram_tx, stream_tx))?;

        let writer_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    Some(bytes) = stream_rx.recv() => {
                        if write_half.write_all(&bytes).await.is_err() {
                            break;
                        }
                    }
                    Some(bytes) = datagram_rx.recv() => {
                        if write_half.write_all(&bytes).await.is_err() {
                            break;
                        }
                    }
                    else => break,
                }
            }
        });

        let intercept_handler = self.intercept_handler();
        let _drop_guard = SessionDropGuard {
            session_id,
            router: self.router.clone(),
            session_sinks: self.session_sinks.clone(),
            cache: self.cache.clone(),
            subscription_manager: Some(self.subscription_manager.clone()),
            writer_handle: Some(writer_handle),
        };

        let mut buf = BytesMut::with_capacity(8192);

        // Enforce edge authentication handshake if authenticator is configured
        let mut first_packet = None;
        if let Some(ref auth) = self.authenticator {
            let timeout_duration = auth.auth_timeout();
            let auth_fut = async {
                loop {
                    if let Some(packet) =
                        ClientPacket::decode_frame(&mut buf).map_err(GatewayError::Codec)?
                    {
                        return Ok::<ClientPacket, GatewayError>(packet);
                    }
                    let bytes_read = read_half
                        .read_buf(&mut buf)
                        .await
                        .map_err(GatewayError::Io)?;
                    if bytes_read == 0 {
                        return Err(GatewayError::Io(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "Client disconnected before authentication",
                        )));
                    }
                    metrics::counter!("mmo_gateway_bytes_received_total")
                        .increment(bytes_read as u64);
                }
            };

            let first_pkt = match tokio::time::timeout(timeout_duration, auth_fut).await {
                Ok(res) => res?,
                Err(_) => {
                    metrics::counter!("mmo_gateway_auth_failures_total", "reason" => "timeout")
                        .increment(1);
                    return Err(GatewayError::Auth(crate::auth::AuthError::Timeout));
                }
            };

            match first_pkt {
                ClientPacket::AuthRequest(auth_req) => match auth.authenticate(&auth_req.token) {
                    Ok(auth_res) => {
                        let initial_node = match self.resolve_initial_node(auth_res.guid).await {
                            Ok(node) => node,
                            Err(_) => {
                                let resp = ClientPacket::AuthResponse(mmo_schema::client::AuthResponsePacket {
                                        success: false,
                                        error_code: mmo_schema::client::auth_error_codes::CLUSTER_UNAVAILABLE,
                                        session_id: 0,
                                        player_guid: 0,
                                        assigned_node: String::new(),
                                        error_message: "Cluster unavailable".to_string(),
                                    });
                                let _ = self.send_packet_to_session(session_id, &resp).await;
                                return Err(GatewayError::NoRouteFound {
                                    session_id,
                                    guid: Some(auth_res.guid),
                                });
                            }
                        };

                        self.register_session(session_id, auth_res.guid, initial_node.clone())?;
                        intercept_handler
                            .subscribe_session_topic(session_id, "chat:global".to_string())
                            .await;
                        intercept_handler
                            .subscribe_session_topic(
                                session_id,
                                format!("whisper:{}", auth_res.guid.raw()),
                            )
                            .await;

                        let resp =
                            ClientPacket::AuthResponse(mmo_schema::client::AuthResponsePacket {
                                success: true,
                                error_code: mmo_schema::client::auth_error_codes::SUCCESS,
                                session_id,
                                player_guid: auth_res.guid.raw(),
                                assigned_node: initial_node,
                                error_message: String::new(),
                            });
                        let _ = self.send_packet_to_session(session_id, &resp).await;
                    }
                    Err(e) => {
                        let resp =
                            ClientPacket::AuthResponse(mmo_schema::client::AuthResponsePacket {
                                success: false,
                                error_code: e.to_error_code(),
                                session_id: 0,
                                player_guid: 0,
                                assigned_node: String::new(),
                                error_message: e.to_string(),
                            });
                        let _ = self.send_packet_to_session(session_id, &resp).await;
                        return Err(GatewayError::Auth(e));
                    }
                },
                _ => {
                    metrics::counter!("mmo_gateway_auth_failures_total", "reason" => "unauthenticated_action").increment(1);
                    return Err(GatewayError::Auth(crate::auth::AuthError::InvalidToken(
                        "Expected AuthRequest packet".to_string(),
                    )));
                }
            }
        } else {
            // Unauthenticated connection: enforce handshake timeout (Slowloris mitigation)
            let timeout_duration = self.handshake_timeout();
            let handshake_fut = async {
                loop {
                    if let Some(packet) =
                        ClientPacket::decode_frame(&mut buf).map_err(GatewayError::Codec)?
                    {
                        return Ok::<ClientPacket, GatewayError>(packet);
                    }
                    let bytes_read = read_half
                        .read_buf(&mut buf)
                        .await
                        .map_err(GatewayError::Io)?;
                    if bytes_read == 0 {
                        return Err(GatewayError::Io(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "Client disconnected before completing initial handshake frame",
                        )));
                    }
                    metrics::counter!("mmo_gateway_bytes_received_total")
                        .increment(bytes_read as u64);
                }
            };

            match tokio::time::timeout(timeout_duration, handshake_fut).await {
                Ok(res) => {
                    first_packet = Some(res?);
                }
                Err(_) => {
                    metrics::counter!("mmo_gateway_handshake_timeout").increment(1);
                    return Err(GatewayError::Io(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "Client connection handshake timed out (Slowloris mitigation)",
                    )));
                }
            }
        }

        let handle_incoming_packet = |packet: ClientPacket| async {
            metrics::counter!("mmo_gateway_packets_received_total", "opcode" => packet.opcode().as_str())
                .increment(1);
            if let ClientPacket::Ping(ping) = &packet {
                let pong = ClientPacket::Pong(mmo_schema::client::PongPacket {
                    client_timestamp: ping.timestamp,
                    server_timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64,
                });
                let _ = self.send_packet_to_session(session_id, &pong).await;
                return Ok::<(), GatewayError>(());
            }

            if intercept_handler.handle_packet(session_id, &packet).await {
                return Ok(());
            }

            let routed = self.dispatcher.dispatch_packet(session_id, packet)?;
            if let Some(ref tx) = self.packet_tx {
                let _ = tx.send(routed).await;
            }
            Ok(())
        };

        if let Some(packet) = first_packet {
            handle_incoming_packet(packet).await?;
        }

        loop {
            let bytes_read = read_half
                .read_buf(&mut buf)
                .await
                .map_err(GatewayError::Io)?;
            if bytes_read == 0 {
                // Client cleanly closed connection
                break;
            }
            metrics::counter!("mmo_gateway_bytes_received_total").increment(bytes_read as u64);

            while let Some(packet) =
                ClientPacket::decode_frame(&mut buf).map_err(GatewayError::Codec)?
            {
                handle_incoming_packet(packet).await?;
            }
        }

        Ok(())
    }

    /// Binds a QUIC UDP connection listener to the specified address with given TLS certificates.
    pub fn bind_quic(
        &self,
        addr: SocketAddr,
        certs: Vec<CertificateDer<'static>>,
        key: PrivateKeyDer<'static>,
    ) -> Result<quinn::Endpoint, GatewayError> {
        create_quic_server(addr, certs, key).map_err(GatewayError::from)
    }

    /// Convenience method to bind a QUIC listener with automatically generated self-signed certificates.
    pub fn bind_quic_self_signed(
        &self,
        addr: SocketAddr,
        subject_alt_names: &[&str],
    ) -> Result<quinn::Endpoint, GatewayError> {
        let (certs, key) = generate_self_signed_cert(subject_alt_names)?;
        self.bind_quic(addr, certs, key)
    }

    /// Binds a WebTransport HTTP/3 UDP connection listener to the specified address with development self-signed certificates.
    pub async fn bind_webtransport(
        &self,
        addr: SocketAddr,
    ) -> Result<crate::webtransport::WebTransportServer, crate::webtransport::WebTransportError>
    {
        let mut server = crate::webtransport::WebTransportServer::bind(
            addr,
            self.router.clone(),
            self.session_sinks.clone(),
            self.session_counter.clone(),
            self.packet_tx.clone(),
        )
        .await?
        .with_intercept_handler(self.intercept_handler());

        if let Some(ref auth) = self.authenticator {
            server = server.with_authenticator(auth.clone());
        }
        if let Some(ref node) = self.default_node_address {
            server = server.with_default_node_address(node.clone());
        }

        Ok(server)
    }

    /// Handles an active client QUIC connection:
    /// Multiplexes unreliable datagrams (movement, entity state sync) and reliable streams (interactions, chat),
    /// decodes client packets, forwards routed packets to simulation nodes, and handles downstream broadcasts.
    pub async fn handle_quic_connection(
        &self,
        connection: quinn::Connection,
        session_id: u64,
    ) -> Result<(), GatewayError> {
        let is_authenticated = Arc::new(std::sync::atomic::AtomicBool::new(
            self.authenticator.is_none(),
        ));
        let active_send_stream: Arc<tokio::sync::Mutex<Option<quinn::SendStream>>> =
            Arc::new(tokio::sync::Mutex::new(None));
        let (datagram_tx, mut datagram_rx) = mpsc::channel::<Bytes>(1024);
        let (stream_tx, mut stream_rx) = mpsc::channel::<Bytes>(DEFAULT_STREAM_CHANNEL_CAPACITY);

        self.register_session_sink(session_id, SessionSink::new(datagram_tx, stream_tx))?;

        let active_stream_writer = active_send_stream.clone();
        let conn_writer = connection.clone();
        let downstream_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(bytes) = datagram_rx.recv() => {
                        let _ = conn_writer.send_datagram(bytes);
                    }
                    Some(bytes) = stream_rx.recv() => {
                        let mut lock = active_stream_writer.lock().await;
                        if let Some(ref mut send) = *lock {
                            if send.write_all(&bytes).await.is_err() {
                                break;
                            }
                        } else if let Ok((mut send, _recv)) = conn_writer.open_bi().await {
                            if send.write_all(&bytes).await.is_err() {
                                break;
                            }
                            *lock = Some(send);
                        }
                    }
                    else => break,
                }
            }
        });

        let intercept_handler = self.intercept_handler();
        let _drop_guard = SessionDropGuard {
            session_id,
            router: self.router.clone(),
            session_sinks: self.session_sinks.clone(),
            cache: self.cache.clone(),
            subscription_manager: Some(self.subscription_manager.clone()),
            writer_handle: Some(downstream_handle),
        };

        loop {
            tokio::select! {
                dgram_res = connection.read_datagram() => {
                    match dgram_res {
                        Ok(dg) => {
                            // Zero-cost atomic packet filter: drop unauthenticated datagrams instantly
                            if !is_authenticated.load(Ordering::Acquire) {
                                metrics::counter!("mmo_gateway_unauthenticated_packets_dropped_total").increment(1);
                                continue;
                            }

                            metrics::counter!("mmo_gateway_bytes_received_total").increment(dg.len() as u64);
                            let mut dgram_buf = BytesMut::from(dg.as_ref());
                            while let Ok(Some(packet)) = ClientPacket::decode_frame(&mut dgram_buf) {
                                metrics::counter!("mmo_gateway_packets_received_total", "opcode" => packet.opcode().as_str())
                                    .increment(1);
                                if let ClientPacket::Ping(ref ping) = packet {
                                    let pong = ClientPacket::Pong(mmo_schema::client::PongPacket {
                                        client_timestamp: ping.timestamp,
                                        server_timestamp: std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap()
                                            .as_millis() as u64,
                                    });
                                    let pong_bytes = Bytes::from(pong.encode());
                                    metrics::counter!("mmo_gateway_packets_sent_total", "opcode" => "Pong").increment(1);
                                    metrics::counter!("mmo_gateway_bytes_sent_total").increment(pong_bytes.len() as u64);
                                    let _ = connection.send_datagram(pong_bytes);
                                }
                                match self.dispatcher.dispatch_packet(session_id, packet) {
                                    Ok(routed) => {
                                        if let Some(ref tx) = self.packet_tx {
                                            let _ = tx.send(routed).await;
                                        }
                                    }
                                    Err(_) => {
                                        metrics::counter!("mmo_gateway_packets_dropped_total").increment(1);
                                    }
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
                bi_res = connection.accept_bi() => {
                    match bi_res {
                        Ok((send, mut recv)) => {
                            *active_send_stream.lock().await = Some(send);
                            let active_stream = active_send_stream.clone();
                            let dispatcher = self.dispatcher.clone();
                            let packet_tx = self.packet_tx.clone();
                            let authenticator = self.authenticator.clone();
                            let is_auth = is_authenticated.clone();
                            let router = self.router.clone();
                            let cache = self.cache.clone();
                            let default_node = self.default_node_address.clone();
                            let intercept = intercept_handler.clone();
                            tokio::spawn(async move {
                                let mut buf = BytesMut::with_capacity(8192);
                                let mut chunk = [0u8; 4096];

                                // Enforce authentication handshake on stream if authenticator is configured
                                if !is_auth.load(Ordering::Acquire) {
                                    if let Some(ref auth) = authenticator {
                                        let timeout = auth.auth_timeout();
                                        let auth_fut = async {
                                            loop {
                                                if let Ok(Some(pkt)) = ClientPacket::decode_frame(&mut buf) {
                                                    return Ok::<ClientPacket, GatewayError>(pkt);
                                                }
                                                match recv.read(&mut chunk).await {
                                                    Ok(Some(n)) if n > 0 => {
                                                        metrics::counter!("mmo_gateway_bytes_received_total").increment(n as u64);
                                                        buf.extend_from_slice(&chunk[..n]);
                                                    }
                                                    _ => return Err(GatewayError::Io(std::io::Error::new(
                                                        std::io::ErrorKind::UnexpectedEof,
                                                        "Connection closed before auth",
                                                    ))),
                                                }
                                            }
                                        };

                                        let first_pkt = match tokio::time::timeout(timeout, auth_fut).await {
                                            Ok(res) => match res {
                                                Ok(p) => p,
                                                Err(_) => return,
                                            },
                                            Err(_) => {
                                                metrics::counter!("mmo_gateway_auth_failures_total", "reason" => "timeout").increment(1);
                                                return;
                                            }
                                        };

                                        match first_pkt {
                                            ClientPacket::AuthRequest(auth_req) => {
                                                match auth.authenticate(&auth_req.token) {
                                                    Ok(auth_res) => {
                                                        let initial_node = router.get_route_by_guid(auth_res.guid)
                                                            .or_else(|| default_node.clone());

                                                        let initial_node = match initial_node {
                                                            Some(node) => node,
                                                            None => {
                                                                let resp = ClientPacket::AuthResponse(mmo_schema::client::AuthResponsePacket {
                                                                    success: false,
                                                                    error_code: mmo_schema::client::auth_error_codes::CLUSTER_UNAVAILABLE,
                                                                    session_id: 0,
                                                                    player_guid: 0,
                                                                    assigned_node: String::new(),
                                                                    error_message: "Cluster unavailable".to_string(),
                                                                });
                                                                let mut lock = active_stream.lock().await;
                                                                if let Some(ref mut s) = *lock {
                                                                    let _ = s.write_all(&resp.encode()).await;
                                                                }
                                                                return;
                                                            }
                                                        };

                                                        router.register_session(session_id, auth_res.guid, initial_node.clone());
                                                        intercept.subscribe_session_topic(session_id, "chat:global".to_string()).await;
                                                        intercept.subscribe_session_topic(session_id, format!("whisper:{}", auth_res.guid.raw())).await;

                                                        if let Some(ref c) = cache {
                                                            let cache_clone = c.clone();
                                                            let node_clone = initial_node.clone();
                                                            let session_data = mmo_storage::cache::SessionData {
                                                                session_id,
                                                                guid: auth_res.guid.raw(),
                                                                node_address: node_clone,
                                                                expires_at: std::time::SystemTime::now()
                                                                    .duration_since(std::time::UNIX_EPOCH)
                                                                    .unwrap_or_default()
                                                                    .as_secs() + 3600,
                                                            };
                                                            tokio::spawn(async move {
                                                                let _ = cache_clone.set_session(&session_data, 3600).await;
                                                            });
                                                        }

                                                        // Memory ordering: register route before releasing authenticated state
                                                        is_auth.store(true, Ordering::Release);

                                                        let resp = ClientPacket::AuthResponse(mmo_schema::client::AuthResponsePacket {
                                                            success: true,
                                                            error_code: mmo_schema::client::auth_error_codes::SUCCESS,
                                                            session_id,
                                                            player_guid: auth_res.guid.raw(),
                                                            assigned_node: initial_node,
                                                            error_message: String::new(),
                                                        });
                                                        let mut lock = active_stream.lock().await;
                                                        if let Some(ref mut s) = *lock {
                                                            let _ = s.write_all(&resp.encode()).await;
                                                        }
                                                    }
                                                    Err(e) => {
                                                        let resp = ClientPacket::AuthResponse(mmo_schema::client::AuthResponsePacket {
                                                            success: false,
                                                            error_code: e.to_error_code(),
                                                            session_id: 0,
                                                            player_guid: 0,
                                                            assigned_node: String::new(),
                                                            error_message: e.to_string(),
                                                        });
                                                        let mut lock = active_stream.lock().await;
                                                        if let Some(ref mut s) = *lock {
                                                            let _ = s.write_all(&resp.encode()).await;
                                                        }
                                                        return;
                                                    }
                                                }
                                            }
                                            _ => {
                                                metrics::counter!("mmo_gateway_auth_failures_total", "reason" => "unauthenticated_action").increment(1);
                                                return;
                                            }
                                        }
                                    }
                                }

                                loop {
                                    match recv.read(&mut chunk).await {
                                        Ok(Some(n)) if n > 0 => {
                                            metrics::counter!("mmo_gateway_bytes_received_total").increment(n as u64);
                                            buf.extend_from_slice(&chunk[..n]);
                                            while let Ok(Some(pkt)) = ClientPacket::decode_frame(&mut buf) {
                                                metrics::counter!("mmo_gateway_packets_received_total", "opcode" => pkt.opcode().as_str())
                                                    .increment(1);
                                                if let ClientPacket::Ping(ref ping) = pkt {
                                                    let pong = ClientPacket::Pong(mmo_schema::client::PongPacket {
                                                        client_timestamp: ping.timestamp,
                                                        server_timestamp: std::time::SystemTime::now()
                                                            .duration_since(std::time::UNIX_EPOCH)
                                                            .unwrap()
                                                            .as_millis() as u64,
                                                    });
                                                    let pong_bytes = pong.encode();
                                                    metrics::counter!("mmo_gateway_packets_sent_total", "opcode" => "Pong").increment(1);
                                                    metrics::counter!("mmo_gateway_bytes_sent_total").increment(pong_bytes.len() as u64);
                                                    let mut lock = active_stream.lock().await;
                                                    if let Some(ref mut send) = *lock {
                                                        let _ = send.write_all(&pong_bytes).await;
                                                    }
                                                    continue;
                                                }

                                                if intercept.handle_packet(session_id, &pkt).await {
                                                    continue;
                                                }

                                                match dispatcher.dispatch_packet(session_id, pkt) {
                                                    Ok(routed) => {
                                                        if let Some(ref tx) = packet_tx {
                                                            let _ = tx.send(routed).await;
                                                        }
                                                    }
                                                    Err(_) => {
                                                        metrics::counter!("mmo_gateway_packets_dropped_total").increment(1);
                                                    }
                                                }
                                            }
                                        }
                                        _ => break,
                                    }
                                }
                            });
                        }
                        Err(_) => break,
                    }
                }
                _ = connection.closed() => {
                    break;
                }
            }
        }

        Ok(())
    }
}

/// Helper to determine if a packet frame is ephemeral (high-frequency movement or sync)
/// suited for unreliable QUIC Datagrams.
pub fn is_ephemeral_packet(bytes: &[u8]) -> bool {
    if bytes.len() >= 4 && bytes[0] == 0x4D && bytes[1] == 0x4D {
        let opcode_raw = u16::from_le_bytes([bytes[2], bytes[3]]);
        if let Ok(opcode) = mmo_schema::client::Opcode::from_u16(opcode_raw) {
            return matches!(
                opcode,
                mmo_schema::client::Opcode::Movement
                    | mmo_schema::client::Opcode::MovementCorrection
                    | mmo_schema::client::Opcode::EntityState
                    | mmo_schema::client::Opcode::EntityDespawn
                    | mmo_schema::client::Opcode::EntityDeltaBatch
                    | mmo_schema::client::Opcode::Ping
                    | mmo_schema::client::Opcode::Pong
            );
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use mmo_core::guid::EntityType;
    use mmo_schema::client::{InteractionPacket, InteractionType, MovementPacket, Vec3};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn test_framing_and_dispatch_mock_client_packets() {
        let router = SessionRouter::new();
        let player_guid = Guid::new(EntityType::Player, 1, 100);
        let session_id = 1;
        let node_addr = "127.0.0.1:50051".to_string();

        router.register_session(session_id, player_guid, node_addr.clone());

        // 1. Build a MovementPacket
        let move_pkt = ClientPacket::Movement(MovementPacket {
            entity_id: player_guid.raw(),
            position: Vec3::new(10.0, 20.0, 30.0),
            velocity: Vec3::new(1.0, 0.0, -1.0),
            yaw: 1.57,
            pitch: 0.25,
            timestamp: 1700000000,
            sequence: 1,
        });

        // 2. Build an InteractionPacket
        let target_guid = Guid::new(EntityType::Npc, 1, 200);
        let interact_pkt = ClientPacket::Interaction(InteractionPacket {
            source_guid: player_guid.raw(),
            target_guid: target_guid.raw(),
            interaction_type: InteractionType::Attack,
            action_id: 42,
            payload: Bytes::from_static(&[1, 2, 3, 4]),
        });

        // 3. Serialize both into wire format bytes
        let move_bytes = move_pkt.encode();
        let interact_bytes = interact_pkt.encode();

        let mut stream_buffer = BytesMut::new();

        // Feed partial bytes of the first packet (packet framer should return empty)
        stream_buffer.extend_from_slice(&move_bytes[..10]);
        let partial_res =
            PacketDispatcher::process_frames_with_router(&router, session_id, &mut stream_buffer)
                .expect("Framing partial bytes should not fail");
        assert!(partial_res.is_empty());
        assert_eq!(stream_buffer.len(), 10);

        // Feed remainder of movement packet and all of interaction packet
        stream_buffer.extend_from_slice(&move_bytes[10..]);
        stream_buffer.extend_from_slice(&interact_bytes);

        let routed =
            PacketDispatcher::process_frames_with_router(&router, session_id, &mut stream_buffer)
                .expect("Should frame and dispatch all complete packets");

        assert_eq!(routed.len(), 2);
        assert!(stream_buffer.is_empty());

        // Verify MovementPacket routing
        assert_eq!(routed[0].session_id, session_id);
        assert_eq!(routed[0].guid, Some(player_guid));
        assert_eq!(routed[0].node_address, "127.0.0.1:50051");
        assert!(routed[0].is_movement());
        assert_eq!(routed[0].packet, move_pkt);

        // Verify InteractionPacket routing
        assert_eq!(routed[1].session_id, session_id);
        assert_eq!(routed[1].guid, Some(player_guid));
        assert_eq!(routed[1].node_address, "127.0.0.1:50051");
        assert!(routed[1].is_interaction());
        assert_eq!(routed[1].packet, interact_pkt);
    }

    #[test]
    fn test_dynamic_route_handoff_seamless_packet_routing() {
        let router = Arc::new(SessionRouter::new());
        let dispatcher = PacketDispatcher::new(router.clone());

        let player_guid = Guid::new(EntityType::Player, 1, 555);
        let session_id = 99;
        let node1 = "127.0.0.1:50051".to_string();
        let node2 = "127.0.0.1:50052".to_string();

        // Step 1: Initial registration to Node 1
        router.register_session(session_id, player_guid, node1.clone());

        // Step 2: Client sends movement packet #1 while in Node 1's domain
        let pkt1 = ClientPacket::Movement(MovementPacket {
            entity_id: player_guid.raw(),
            position: Vec3::new(10.0, 0.0, 10.0),
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            timestamp: 1000,
            sequence: 1,
        });

        let mut buf = BytesMut::from(&pkt1.encode()[..]);
        let routed1 = dispatcher
            .process_frames(session_id, &mut buf)
            .expect("Packet 1 routing failed");
        assert_eq!(routed1.len(), 1);
        assert_eq!(routed1[0].node_address, node1);

        // Step 3: Client sends movement packet #2 still on Node 1
        let pkt2 = ClientPacket::Movement(MovementPacket {
            entity_id: player_guid.raw(),
            position: Vec3::new(15.0, 0.0, 10.0),
            velocity: Vec3::new(5.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            timestamp: 1050,
            sequence: 2,
        });
        buf.extend_from_slice(&pkt2.encode());
        let routed2 = dispatcher
            .process_frames(session_id, &mut buf)
            .expect("Packet 2 routing failed");
        assert_eq!(routed2.len(), 1);
        assert_eq!(routed2[0].node_address, node1);

        // Step 4: Player crosses boundary! Simulation triggers seamless handover to Node 2.
        let updated = router.update_route(player_guid, node2.clone());
        assert!(updated, "Route should exist and be updated");

        // Step 5: Client sends movement packet #3 and #4 immediately without reconnecting
        let pkt3 = ClientPacket::Movement(MovementPacket {
            entity_id: player_guid.raw(),
            position: Vec3::new(20.0, 0.0, 10.0),
            velocity: Vec3::new(5.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            timestamp: 1100,
            sequence: 3,
        });
        let pkt4 = ClientPacket::Movement(MovementPacket {
            entity_id: player_guid.raw(),
            position: Vec3::new(25.0, 0.0, 10.0),
            velocity: Vec3::new(5.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            timestamp: 1150,
            sequence: 4,
        });

        buf.extend_from_slice(&pkt3.encode());
        buf.extend_from_slice(&pkt4.encode());

        let routed_handoff = dispatcher
            .process_frames(session_id, &mut buf)
            .expect("Handoff packets routing failed");

        // Both packets routed seamlessly to Node 2 without packet loss
        assert_eq!(routed_handoff.len(), 2);
        assert_eq!(routed_handoff[0].node_address, node2);
        assert_eq!(routed_handoff[0].packet, pkt3);
        assert_eq!(routed_handoff[1].node_address, node2);
        assert_eq!(routed_handoff[1].packet, pkt4);
    }

    #[test]
    fn test_unrouted_packet_returns_error() {
        let router = Arc::new(SessionRouter::new());
        let dispatcher = PacketDispatcher::new(router);

        let unk_guid = Guid::new(EntityType::Player, 9, 999);
        let pkt = ClientPacket::Movement(MovementPacket {
            entity_id: unk_guid.raw(),
            position: Vec3::ZERO,
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            timestamp: 0,
            sequence: 1,
        });

        let mut buf = BytesMut::from(&pkt.encode()[..]);
        let res = dispatcher.process_frames(12345, &mut buf);
        assert!(res.is_err());
        match res.unwrap_err() {
            GatewayError::NoRouteFound { session_id, guid } => {
                assert_eq!(session_id, 12345);
                assert_eq!(guid, Some(unk_guid));
            }
            other => panic!("Expected NoRouteFound, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_gateway_server_tcp_listener_and_packet_stream() {
        let router = Arc::new(SessionRouter::new());
        let (packet_tx, mut packet_rx) = mpsc::channel(16);

        let server = Arc::new(GatewayServer::with_packet_channel(
            router.clone(),
            packet_tx,
        ));

        // Bind listener on an ephemeral port
        let listener = server.bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let player_guid = Guid::new(EntityType::Player, 1, 777);
        let session_id = server.next_session_id();
        server
            .register_session(session_id, player_guid, "127.0.0.1:50051".into())
            .unwrap();

        // Spawn server accept task for 1 client
        let server_clone = server.clone();
        let server_handle = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            server_clone.handle_connection(socket, session_id).await
        });

        // Client connects
        let mut client_stream = TcpStream::connect(local_addr).await.unwrap();

        let move_pkt = ClientPacket::Movement(MovementPacket {
            entity_id: player_guid.raw(),
            position: Vec3::new(1.0, 2.0, 3.0),
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            timestamp: 100,
            sequence: 1,
        });

        // Write packet across TCP socket
        client_stream.write_all(&move_pkt.encode()).await.unwrap();

        // Receive routed packet from server channel
        let routed = packet_rx.recv().await.expect("Expected routed packet");
        assert_eq!(routed.session_id, session_id);
        assert_eq!(routed.guid, Some(player_guid));
        assert_eq!(routed.node_address, "127.0.0.1:50051");
        assert_eq!(routed.packet, move_pkt);

        // Close connection and wait for server task
        drop(client_stream);
        let _ = server_handle.await;

        // Session removed on disconnect
        assert_eq!(server.get_route_by_session(session_id).unwrap(), None);
    }

    #[tokio::test]
    async fn test_gateway_server_with_cache_backend() {
        use mmo_storage::cache::MemoryCache;

        let router = Arc::new(SessionRouter::new());
        let cache = Arc::new(MemoryCache::new());
        let server = GatewayServer::new(router).with_cache(cache.clone());

        let player_guid = Guid::new(EntityType::Player, 1, 888);
        let session_id = 42;
        let node_addr = "127.0.0.1:50053".to_string();

        // 1. Register with cache
        server
            .register_session_with_cache(session_id, player_guid, node_addr.clone(), 60)
            .await
            .unwrap();

        // Verify route in local router
        assert_eq!(
            server.get_route_by_session(session_id).unwrap(),
            Some(node_addr.clone())
        );

        // Verify route in cache backend
        let cached = cache
            .get_session(session_id)
            .await
            .unwrap()
            .expect("Session in cache");
        assert_eq!(cached.guid, player_guid.raw());
        assert_eq!(cached.node_address, node_addr);

        // 2. Clear local router to simulate separate gateway replica resolving route from cache
        server.router().clear();
        assert_eq!(server.get_route_by_session(session_id).unwrap(), None);

        // Resolving route via resolve_route_or_cache should query cache and populate local router
        let resolved = server.resolve_route_or_cache(session_id).await.unwrap();
        assert_eq!(resolved, Some(node_addr.clone()));
        assert_eq!(
            server.get_route_by_session(session_id).unwrap(),
            Some(node_addr)
        );

        // 3. Remove with cache
        let removed = server.remove_session_with_cache(session_id).await.unwrap();
        assert_eq!(removed, Some(player_guid));
        assert_eq!(server.get_route_by_session(session_id).unwrap(), None);
        assert!(cache.get_session(session_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_gateway_session_sink_registry() {
        let router = Arc::new(SessionRouter::new());
        let server = GatewayServer::new(router);

        let (tx1, mut rx1) = mpsc::channel(16);
        let (tx2, mut _rx2) = mpsc::channel(16);

        server.register_session_sink(1, tx1).unwrap();
        server.register_session_sink(2, tx2).unwrap();

        assert_eq!(server.active_sink_count().unwrap(), 2);
        assert!(server.has_session_sink(1).unwrap());
        assert!(server.has_session_sink(2).unwrap());
        assert!(!server.has_session_sink(3).unwrap());

        // Send to session 1
        let pkt = ClientPacket::EntityDespawn(mmo_schema::client::EntityDespawnPacket {
            entity_id: 12345,
        });
        assert!(server.send_packet_to_session(1, &pkt).await.unwrap());
        assert!(!server.send_packet_to_session(999, &pkt).await.unwrap());

        let received1 = rx1.recv().await.expect("received packet bytes");
        let decoded =
            ClientPacket::decode_frame(&mut bytes::BytesMut::from(received1.as_ref())).unwrap();
        assert_eq!(decoded, Some(pkt.clone()));

        // Broadcast
        let count = server.broadcast_packet(&pkt).await.unwrap();
        assert_eq!(count, 2);

        // Remove sink 1
        server.remove_session_sink(1).unwrap();
        assert_eq!(server.active_sink_count().unwrap(), 1);
        assert!(!server.has_session_sink(1).unwrap());
    }

    #[tokio::test]
    async fn test_gateway_downstream_packet_streaming() {
        let router = Arc::new(SessionRouter::new());
        let (packet_tx, _packet_rx) = mpsc::channel(100);
        let server = GatewayServer::with_packet_channel(router.clone(), packet_tx);

        let listener = server.bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let session_id = server.next_session_id();
        let player_guid = Guid::new(EntityType::Player, 1, 999);
        server
            .register_session(session_id, player_guid, "127.0.0.1:50051".to_string())
            .unwrap();

        let server_clone = Arc::new(server);
        let srv = server_clone.clone();

        let server_task = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let _ = srv.handle_connection(socket, session_id).await;
        });

        let mut client_stream = TcpStream::connect(local_addr).await.unwrap();

        // Wait brief moment for handle_connection to register the sink
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert!(server_clone.has_session_sink(session_id).unwrap());

        // Send an EntityState packet downstream from server to client
        let entity_state = ClientPacket::EntityState(mmo_schema::client::EntityStatePacket {
            entity_id: 8888,
            position: Vec3::new(10.0, 20.0, 30.0),
            velocity: Vec3::new(1.0, 0.0, 0.0),
            yaw: 1.5,
            pitch: 0.0,
            health: 100,
            max_health: 100,
            state_flags: 0,
            timestamp: 5000,
        });

        assert!(server_clone
            .send_packet_to_session(session_id, &entity_state)
            .await
            .unwrap());

        // Client reads the framed packet
        let mut read_buf = BytesMut::with_capacity(1024);
        let n = client_stream.read_buf(&mut read_buf).await.unwrap();
        assert!(n > 0);

        let client_packet = ClientPacket::decode_frame(&mut read_buf).unwrap();
        assert_eq!(client_packet, Some(entity_state));

        // Clean disconnect
        drop(client_stream);
        let _ = server_task.await;

        // Verify sink was unregistered
        assert!(!server_clone.has_session_sink(session_id).unwrap());
    }

    #[test]
    fn test_is_ephemeral_packet() {
        let move_pkt = ClientPacket::Movement(MovementPacket {
            entity_id: 1,
            position: Vec3::ZERO,
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            timestamp: 0,
            sequence: 0,
        });
        assert!(is_ephemeral_packet(&move_pkt.encode()));

        let entity_state_pkt = ClientPacket::EntityState(mmo_schema::client::EntityStatePacket {
            entity_id: 1,
            position: Vec3::ZERO,
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            health: 100,
            max_health: 100,
            state_flags: 0,
            timestamp: 0,
        });
        assert!(is_ephemeral_packet(&entity_state_pkt.encode()));

        let interact_pkt = ClientPacket::Interaction(InteractionPacket {
            source_guid: 1,
            target_guid: 2,
            interaction_type: InteractionType::Attack,
            action_id: 10,
            payload: Bytes::new(),
        });
        assert!(!is_ephemeral_packet(&interact_pkt.encode()));
    }

    #[tokio::test]
    async fn test_gateway_quic_server_datagram_and_stream_handling() {
        use crate::quic::QuicClientSession;

        let router = Arc::new(SessionRouter::new());
        let (packet_tx, mut packet_rx) = mpsc::channel(100);
        let server = Arc::new(GatewayServer::with_packet_channel(
            router.clone(),
            packet_tx,
        ));

        let session_id = 99;
        let player_guid = Guid::new(EntityType::Player, 1, 999);
        let node_addr = "127.0.0.1:50051".to_string();
        server
            .register_session(session_id, player_guid, node_addr.clone())
            .unwrap();

        let quic_endpoint = server
            .bind_quic_self_signed("127.0.0.1:0".parse().unwrap(), &["localhost", "127.0.0.1"])
            .expect("bind quic");
        let local_addr = quic_endpoint.local_addr().unwrap();

        let srv = server.clone();
        let srv_endpoint = quic_endpoint.clone();
        let server_task = tokio::spawn(async move {
            let incoming = srv_endpoint.accept().await.unwrap();
            let conn = incoming.await.unwrap();
            let _ = srv.handle_quic_connection(conn, session_id).await;
        });

        let mut client = QuicClientSession::connect(local_addr, "localhost")
            .await
            .expect("client connect");

        // 1. Inbound Datagram (Movement)
        let move_pkt = ClientPacket::Movement(MovementPacket {
            entity_id: player_guid.raw(),
            position: Vec3::new(10.0, 20.0, 30.0),
            velocity: Vec3::ZERO,
            yaw: 1.0,
            pitch: 0.0,
            timestamp: 1234,
            sequence: 1,
        });
        client.send_datagram(&move_pkt).expect("send datagram");

        let routed_move =
            tokio::time::timeout(tokio::time::Duration::from_secs(2), packet_rx.recv())
                .await
                .expect("timeout waiting for routed movement")
                .expect("routed move pkt");
        assert_eq!(routed_move.session_id, session_id);
        assert_eq!(routed_move.packet, move_pkt);

        // 2. Inbound Stream (Interaction)
        let interact_pkt = ClientPacket::Interaction(InteractionPacket {
            source_guid: player_guid.raw(),
            target_guid: 555,
            interaction_type: InteractionType::Loot,
            action_id: 1,
            payload: Bytes::from_static(&[1, 2, 3]),
        });
        client
            .send_packet_stream(&interact_pkt)
            .await
            .expect("send stream");

        let routed_interact =
            tokio::time::timeout(tokio::time::Duration::from_secs(2), packet_rx.recv())
                .await
                .expect("timeout waiting for routed interaction")
                .expect("routed interact pkt");
        assert_eq!(routed_interact.session_id, session_id);
        assert_eq!(routed_interact.packet, interact_pkt);

        // 3. Bidirectional Stream Ping -> Pong
        let ping_pkt = ClientPacket::Ping(mmo_schema::client::PingPacket { timestamp: 7777 });
        client
            .send_packet_stream(&ping_pkt)
            .await
            .expect("send ping");
        let pong_pkt = tokio::time::timeout(
            tokio::time::Duration::from_secs(2),
            client.recv_packet_stream(),
        )
        .await
        .expect("timeout waiting for pong")
        .expect("pong pkt");
        if let ClientPacket::Pong(pong) = pong_pkt {
            assert_eq!(pong.client_timestamp, 7777);
        } else {
            panic!("Expected Pong packet, got {pong_pkt:?}");
        }

        // 4. Downstream ephemeral broadcast from server via datagram
        let entity_state = ClientPacket::EntityState(mmo_schema::client::EntityStatePacket {
            entity_id: 12345,
            position: Vec3::new(5.0, 6.0, 7.0),
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            health: 80,
            max_health: 100,
            state_flags: 1,
            timestamp: 9000,
        });
        server
            .send_packet_to_session(session_id, &entity_state)
            .await
            .unwrap();

        let received_state =
            tokio::time::timeout(tokio::time::Duration::from_secs(2), client.read_datagram())
                .await
                .expect("timeout waiting for downstream entity state")
                .expect("read datagram");
        assert_eq!(received_state, entity_state);

        // Disconnect
        client.close(0, "normal test close");
        let _ = server_task.await;
        assert!(!server.has_session_sink(session_id).unwrap());
    }

    #[tokio::test]
    async fn test_session_drop_guard_on_task_abort() {
        let router = Arc::new(SessionRouter::new());
        let server = Arc::new(GatewayServer::new(router));

        let listener = server.bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let session_id = 555;
        let player_guid = Guid::new(EntityType::Player, 1, 555);
        let node_addr = "127.0.0.1:50051".to_string();
        server
            .register_session(session_id, player_guid, node_addr)
            .unwrap();

        let srv = server.clone();
        let handle = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let _ = srv.handle_connection(socket, session_id).await;
        });

        let client = TcpStream::connect(local_addr).await.unwrap();

        // Wait brief moment for handle_connection to set up sink
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert!(server.has_session_sink(session_id).unwrap());
        assert!(server.get_route_by_session(session_id).unwrap().is_some());

        // Abort the server task violently
        handle.abort();
        let _ = handle.await;

        // Verify RAII cleanup was triggered despite task abort
        assert!(!server.has_session_sink(session_id).unwrap());
        assert_eq!(server.get_route_by_session(session_id).unwrap(), None);
        drop(client);
    }

    #[tokio::test]
    async fn test_gateway_telemetry_metrics() {
        let telemetry = mmo_core::TelemetryServer::init().unwrap();
        let router = Arc::new(SessionRouter::new());
        let server = GatewayServer::new(router);

        let session_id = 999;
        let (tx, rx) = mpsc::channel(1);
        server.register_session_sink(session_id, tx).unwrap();

        let render_active = telemetry.render_metrics();
        assert!(render_active.contains("mmo_gateway_active_sessions"));

        let ping = ClientPacket::Ping(mmo_schema::client::PingPacket { timestamp: 12345 });
        server
            .send_packet_to_session(session_id, &ping)
            .await
            .unwrap();

        // Saturate buffer and send again to trigger packet drop
        let dropped = server
            .send_packet_to_session(session_id, &ping)
            .await
            .unwrap();
        assert!(!dropped);

        let render_metrics = telemetry.render_metrics();
        assert!(render_metrics.contains("mmo_gateway_packets_sent_total"));
        assert!(render_metrics.contains("mmo_gateway_bytes_sent_total"));
        assert!(render_metrics.contains("mmo_gateway_packets_dropped_total"));

        server.remove_session_sink(session_id).unwrap();
        drop(rx);
    }

    #[tokio::test]
    async fn test_gateway_service_update_route() {
        let router = Arc::new(SessionRouter::new());
        let service = GatewayServiceImpl::new(Arc::clone(&router));

        let guid = Guid::new(mmo_core::guid::EntityType::Player, 1, 100);
        let session_id = 77;
        let initial_node = "127.0.0.1:50051".to_string();
        router.register_session(session_id, guid, initial_node);

        assert_eq!(
            router.get_route_by_guid(guid).as_deref(),
            Some("127.0.0.1:50051")
        );

        let update_req = tonic::Request::new(RouteUpdateRequest {
            guid: guid.raw(),
            session_id,
            node_address: "127.0.0.1:50052".to_string(),
        });

        let resp = service.update_route(update_req).await.unwrap().into_inner();
        assert!(resp.success);

        assert_eq!(
            router.get_route_by_guid(guid).as_deref(),
            Some("127.0.0.1:50052")
        );
        assert_eq!(
            router.get_route_by_session(session_id).as_deref(),
            Some("127.0.0.1:50052")
        );
    }

    #[tokio::test]
    async fn test_gateway_service_update_batch_routes() {
        let router = Arc::new(SessionRouter::new());
        let service = GatewayServiceImpl::new(Arc::clone(&router));

        let guid1 = Guid::new(mmo_core::guid::EntityType::Player, 1, 101);
        let guid2 = Guid::new(mmo_core::guid::EntityType::Player, 1, 102);
        let s1 = 81;
        let s2 = 82;
        let initial_node = "127.0.0.1:50051".to_string();

        router.register_session(s1, guid1, initial_node.clone());
        router.register_session(s2, guid2, initial_node.clone());

        assert_eq!(
            router.get_route_by_guid(guid1).as_deref(),
            Some("127.0.0.1:50051")
        );
        assert_eq!(
            router.get_route_by_guid(guid2).as_deref(),
            Some("127.0.0.1:50051")
        );

        let batch_req = tonic::Request::new(mmo_schema::cluster::BatchRouteUpdateRequest {
            updates: vec![
                RouteUpdateRequest {
                    guid: guid1.raw(),
                    session_id: s1,
                    node_address: "127.0.0.1:50052".to_string(),
                },
                RouteUpdateRequest {
                    guid: guid2.raw(),
                    session_id: s2,
                    node_address: "127.0.0.1:50052".to_string(),
                },
            ],
        });

        let resp = service
            .update_batch_routes(batch_req)
            .await
            .unwrap()
            .into_inner();
        assert!(resp.success);
        assert_eq!(resp.updated_count, 2);

        assert_eq!(
            router.get_route_by_guid(guid1).as_deref(),
            Some("127.0.0.1:50052")
        );
        assert_eq!(
            router.get_route_by_guid(guid2).as_deref(),
            Some("127.0.0.1:50052")
        );
        assert_eq!(
            router.get_route_by_session(s1).as_deref(),
            Some("127.0.0.1:50052")
        );
        assert_eq!(
            router.get_route_by_session(s2).as_deref(),
            Some("127.0.0.1:50052")
        );
    }

    #[tokio::test]
    async fn test_gateway_service_revoke_and_broadcast() {
        let router = Arc::new(SessionRouter::new());
        let sinks = Arc::new(DashMap::new());
        let (dtx, _drx) = mpsc::channel(10);
        let (stx, _srx) = mpsc::channel(10);
        sinks.insert(99, SessionSink::new(dtx, stx));

        let service = GatewayServiceImpl::with_sinks(Arc::clone(&router), Arc::clone(&sinks));
        let guid = Guid::new(mmo_core::guid::EntityType::Player, 1, 999);
        router.register_session(99, guid, "127.0.0.1:50051".to_string());

        assert_eq!(
            router.get_route_by_guid(guid).as_deref(),
            Some("127.0.0.1:50051")
        );
        assert!(sinks.contains_key(&99));

        // 1. Test broadcast route
        let bcast_req = tonic::Request::new(MultiGatewayRouteBroadcastRequest {
            guid: guid.raw(),
            session_id: 99,
            new_node_address: "127.0.0.1:50053".to_string(),
            session_epoch: 2,
        });
        let bcast_resp = service
            .broadcast_route(bcast_req)
            .await
            .unwrap()
            .into_inner();
        assert!(bcast_resp.success);
        assert_eq!(
            router.get_route_by_guid(guid).as_deref(),
            Some("127.0.0.1:50053")
        );

        // 2. Test revoke session
        let revoke_req = tonic::Request::new(RevokeGatewaySessionRequest {
            guid: guid.raw(),
            session_id: 99,
            superseded_by_epoch: 3,
        });
        let revoke_resp = service
            .revoke_gateway_session(revoke_req)
            .await
            .unwrap()
            .into_inner();
        assert!(revoke_resp.success);
        assert_eq!(router.get_route_by_guid(guid), None);
        assert!(!sinks.contains_key(&99));
    }

    #[tokio::test]
    async fn test_slowloris_handshake_timeout_disconnects_idle_client() {
        let router = Arc::new(SessionRouter::new());
        let server =
            Arc::new(GatewayServer::new(router).with_handshake_timeout(Duration::from_millis(50)));
        let listener = server.bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let srv = server.clone();
        let server_task = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            srv.handle_connection(socket, 123).await
        });

        let mut client_stream = TcpStream::connect(local_addr).await.unwrap();

        // Server should timeout and terminate connection because client sends no handshake
        let res = server_task.await.unwrap();
        assert!(res.is_err());

        // Client stream should read EOF
        let mut buf = [0u8; 10];
        let n = client_stream.read(&mut buf).await.unwrap();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn test_bounded_stream_backpressure_drops_when_saturated() {
        let router = Arc::new(SessionRouter::new());
        let server = Arc::new(GatewayServer::new(router));
        let session_id = 456;

        let (dtx, _drx) = mpsc::channel(10);
        let (stx, mut srx) = mpsc::channel(2); // tiny capacity of 2
        server
            .register_session_sink(session_id, SessionSink::new(dtx, stx))
            .unwrap();

        let pkt = ClientPacket::InteractionResult(mmo_schema::client::InteractionResultPacket {
            action_id: 1,
            success: true,
            error_code: 0,
            payload: Vec::new(),
        });

        // First 2 sends succeed and fill the channel
        assert!(server
            .send_packet_to_session(session_id, &pkt)
            .await
            .unwrap());
        assert!(server
            .send_packet_to_session(session_id, &pkt)
            .await
            .unwrap());

        // Third send encounters backpressure and drops packet cleanly (returns false, doesn't panic)
        assert!(!server
            .send_packet_to_session(session_id, &pkt)
            .await
            .unwrap());

        // Drain one packet from the stream receiver
        assert!(srx.recv().await.is_some());

        // Now channel has slot again, next send succeeds
        assert!(server
            .send_packet_to_session(session_id, &pkt)
            .await
            .unwrap());
    }
}
