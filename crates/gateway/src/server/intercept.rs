use bytes::Bytes;
use dashmap::DashMap;
use mmo_schema::client::ClientPacket;
use std::sync::Arc;

use super::sink::SessionSink;
use crate::marketplace::GatewayMarketplaceService;
use crate::router::SessionRouter;
use crate::social::GatewaySubscriptionManager;

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
