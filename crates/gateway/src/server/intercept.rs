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
                // If active subscribers for the topic already dropped to 0 while awaiting subscription, don't spawn
                if self.subscription_manager.get_subscribers(topic).is_empty() {
                    return;
                }
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
                if let Some(old_handle) = self
                    .active_bus_subscriptions
                    .insert(topic.to_string(), handle)
                {
                    old_handle.abort();
                }
            }
        }
    }

    /// Cleanly deregisters all subscriptions for `session_id`, terminating idle topic listeners.
    pub fn remove_session(&self, session_id: u64) {
        let removed = self.subscription_manager.remove_session(session_id);
        for topic in removed {
            if self.subscription_manager.get_subscribers(&topic).is_empty() {
                if let Some((_, handle)) = self.active_bus_subscriptions.remove(&topic) {
                    handle.abort();
                }
                if let Some(ref bus) = self.social_bus {
                    let bus = bus.clone();
                    let topic_clone = topic.clone();
                    if let Ok(rt) = tokio::runtime::Handle::try_current() {
                        rt.spawn(async move {
                            let _ = bus.unsubscribe(&topic_clone).await;
                        });
                    }
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
                // Anti-Spoofing: Must be an authenticated session and sender_guid must match bound_guid.
                let bound_guid = match self.router.get_guid_by_session(session_id) {
                    Some(guid) => guid,
                    None => {
                        metrics::counter!("mmo_gateway_spoofed_social_packets_dropped_total")
                            .increment(1);
                        return true;
                    }
                };
                if msg.sender_guid != bound_guid.raw() {
                    metrics::counter!("mmo_gateway_spoofed_social_packets_dropped_total")
                        .increment(1);
                    return true;
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
                // Authorization: Must be an authenticated session to subscribe to social topics.
                let bound_guid = match self.router.get_guid_by_session(session_id) {
                    Some(guid) => guid,
                    None => {
                        metrics::counter!("mmo_gateway_spoofed_social_packets_dropped_total")
                            .increment(1);
                        return true;
                    }
                };

                let topic = if !sub.channel_name.is_empty() {
                    // Prevent whisper impersonation or eavesdropping via custom channel names
                    if sub.channel_name.starts_with("whisper:") {
                        let expected = format!("whisper:{}", bound_guid.raw());
                        if sub.channel_name != expected {
                            metrics::counter!("mmo_gateway_spoofed_social_packets_dropped_total")
                                .increment(1);
                            return true;
                        }
                    }
                    sub.channel_name.clone()
                } else {
                    match sub.channel_type {
                        0 => "chat:global".to_string(),
                        1 => {
                            // Whisper subscription is strictly restricted to the session's own GUID
                            if sub.target_id != bound_guid.raw() {
                                metrics::counter!(
                                    "mmo_gateway_spoofed_social_packets_dropped_total"
                                )
                                .increment(1);
                                return true;
                            }
                            format!("whisper:{}", bound_guid.raw())
                        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::guid::Guid;
    use mmo_schema::client::{
        market_error_codes, MarketBuyRequestPacket, MarketCancelRequestPacket,
        MarketListRequestPacket, MarketQueryRequestPacket, MarketResponsePacket,
        SocialMessagePacket, SocialSubscribePacket, SocialUnsubscribePacket,
    };
    use tokio::sync::mpsc;

    #[allow(clippy::type_complexity)]
    fn create_test_handler() -> (
        GatewayInterceptHandler,
        Arc<SessionRouter>,
        Arc<GatewaySubscriptionManager>,
        Arc<DashMap<u64, SessionSink>>,
    ) {
        let router = Arc::new(SessionRouter::new());
        let subscription_manager = Arc::new(GatewaySubscriptionManager::new());
        let session_sinks = Arc::new(DashMap::new());
        let active_bus_subscriptions = Arc::new(DashMap::new());

        let handler = GatewayInterceptHandler::new(
            router.clone(),
            subscription_manager.clone(),
            None,
            None,
            session_sinks.clone(),
            active_bus_subscriptions,
        );

        (handler, router, subscription_manager, session_sinks)
    }

    #[tokio::test]
    async fn test_social_message_anti_spoofing_drops_mismatched_sender() {
        let (handler, router, sub_mgr, sinks) = create_test_handler();
        let s1 = 1u64;
        let s2 = 2u64;
        let bound_guid = Guid::from_raw(100);
        router.register_session(s1, bound_guid, "127.0.0.1:50051".to_string());

        // Subscribe session 2 to chat:global
        sub_mgr.subscribe(s2, "chat:global".to_string());
        let (stx2, mut srx2) = mpsc::channel(16);
        let (dtx2, _drx2) = mpsc::channel(16);
        sinks.insert(s2, SessionSink::new(dtx2, stx2));

        // 1. Spoofed sender_guid = 200 (bound is 100) -> dropped
        let spoofed_packet = ClientPacket::SocialMessage(SocialMessagePacket {
            channel_type: 0,
            target_id: 0,
            sender_guid: 200,
            sender_name: "Imposter".to_string(),
            content: "Spoofed message".to_string(),
            timestamp: 1000,
        });
        assert!(handler.handle_packet(s1, &spoofed_packet).await);
        assert!(
            srx2.try_recv().is_err(),
            "spoofed packet must not be delivered"
        );

        // 2. Legitimate sender_guid = 100 -> delivered
        let legit_packet = ClientPacket::SocialMessage(SocialMessagePacket {
            channel_type: 0,
            target_id: 0,
            sender_guid: 100,
            sender_name: "RealPlayer".to_string(),
            content: "Hello friends".to_string(),
            timestamp: 1001,
        });
        assert!(handler.handle_packet(s1, &legit_packet).await);
        assert!(
            srx2.try_recv().is_ok(),
            "legit packet must be delivered to subscriber"
        );
    }

    #[tokio::test]
    async fn test_social_message_unbound_session_dropped() {
        let (handler, _router, sub_mgr, sinks) = create_test_handler();
        let unauthenticated_session = 999u64;
        let s2 = 2u64;

        sub_mgr.subscribe(s2, "chat:global".to_string());
        let (stx2, mut srx2) = mpsc::channel(16);
        let (dtx2, _drx2) = mpsc::channel(16);
        sinks.insert(s2, SessionSink::new(dtx2, stx2));

        // Unbound session sends message claiming sender_guid = 100
        let msg_pkt = ClientPacket::SocialMessage(SocialMessagePacket {
            channel_type: 0,
            target_id: 0,
            sender_guid: 100,
            sender_name: "Anonymous".to_string(),
            content: "Spoofed global message".to_string(),
            timestamp: 1000,
        });
        assert!(
            handler
                .handle_packet(unauthenticated_session, &msg_pkt)
                .await
        );
        assert!(
            srx2.try_recv().is_err(),
            "unbound session message must be dropped"
        );
    }

    #[tokio::test]
    async fn test_social_subscribe_unauthorized_whisper_dropped() {
        let (handler, router, sub_mgr, _sinks) = create_test_handler();
        let s1 = 101u64;
        let my_guid = Guid::from_raw(500);
        let victim_guid = 600u64;
        router.register_session(s1, my_guid, "127.0.0.1:50051".to_string());

        // 1. Attempt to eavesdrop via channel_type = 1 and victim's target_id
        let eavesdrop_type1 = ClientPacket::SocialSubscribe(SocialSubscribePacket {
            channel_type: 1,
            channel_name: String::new(),
            target_id: victim_guid,
        });
        handler.handle_packet(s1, &eavesdrop_type1).await;
        assert!(
            !sub_mgr.get_subscribers("whisper:600").contains(&s1),
            "cannot subscribe to victim's whisper channel via channel_type=1"
        );

        // 2. Attempt to eavesdrop via channel_name = "whisper:600"
        let eavesdrop_named = ClientPacket::SocialSubscribe(SocialSubscribePacket {
            channel_type: 0,
            channel_name: "whisper:600".to_string(),
            target_id: 0,
        });
        handler.handle_packet(s1, &eavesdrop_named).await;
        assert!(
            !sub_mgr.get_subscribers("whisper:600").contains(&s1),
            "cannot subscribe to victim's whisper channel via channel_name"
        );

        // 3. Subscribing to own whisper succeeds
        let own_whisper = ClientPacket::SocialSubscribe(SocialSubscribePacket {
            channel_type: 1,
            channel_name: String::new(),
            target_id: 500,
        });
        handler.handle_packet(s1, &own_whisper).await;
        assert!(
            sub_mgr.get_subscribers("whisper:500").contains(&s1),
            "subscribing to own whisper channel must succeed"
        );
    }

    #[tokio::test]
    async fn test_social_subscribe_unsubscribe_and_remove_session() {
        let (handler, router, sub_mgr, _sinks) = create_test_handler();
        let s1 = 10u64;
        router.register_session(s1, Guid::from_raw(55), "127.0.0.1:50051".to_string());

        // Subscribe all standard channel types and custom name
        handler
            .handle_packet(
                s1,
                &ClientPacket::SocialSubscribe(SocialSubscribePacket {
                    channel_type: 0,
                    channel_name: String::new(),
                    target_id: 0,
                }),
            )
            .await;
        assert!(sub_mgr.get_subscribers("chat:global").contains(&s1));

        handler
            .handle_packet(
                s1,
                &ClientPacket::SocialSubscribe(SocialSubscribePacket {
                    channel_type: 1,
                    channel_name: String::new(),
                    target_id: 55,
                }),
            )
            .await;
        assert!(sub_mgr.get_subscribers("whisper:55").contains(&s1));

        handler
            .handle_packet(
                s1,
                &ClientPacket::SocialSubscribe(SocialSubscribePacket {
                    channel_type: 2,
                    channel_name: String::new(),
                    target_id: 77,
                }),
            )
            .await;
        assert!(sub_mgr.get_subscribers("chat:party:77").contains(&s1));

        handler
            .handle_packet(
                s1,
                &ClientPacket::SocialSubscribe(SocialSubscribePacket {
                    channel_type: 3,
                    channel_name: String::new(),
                    target_id: 88,
                }),
            )
            .await;
        assert!(sub_mgr.get_subscribers("chat:guild:88").contains(&s1));

        handler
            .handle_packet(
                s1,
                &ClientPacket::SocialSubscribe(SocialSubscribePacket {
                    channel_type: 4,
                    channel_name: String::new(),
                    target_id: 0,
                }),
            )
            .await;
        assert!(sub_mgr.get_subscribers("chat:area").contains(&s1));

        handler
            .handle_packet(
                s1,
                &ClientPacket::SocialSubscribe(SocialSubscribePacket {
                    channel_type: 99,
                    channel_name: "trade:lfg".to_string(),
                    target_id: 0,
                }),
            )
            .await;
        assert!(sub_mgr.get_subscribers("trade:lfg").contains(&s1));

        // Unsubscribe custom name
        handler
            .handle_packet(
                s1,
                &ClientPacket::SocialUnsubscribe(SocialUnsubscribePacket {
                    channel_type: 0,
                    channel_name: "trade:lfg".to_string(),
                    target_id: 0,
                }),
            )
            .await;
        assert!(!sub_mgr.get_subscribers("trade:lfg").contains(&s1));

        // remove_session cleans up remaining subscriptions
        handler.remove_session(s1);
        assert!(sub_mgr.get_subscribers("whisper:55").is_empty());
    }

    #[tokio::test]
    async fn test_market_requests_unauthenticated_and_unavailable_errors() {
        let (handler, _router, _sub_mgr, sinks) = create_test_handler();
        let s1 = 42u64;
        let (stx, mut srx) = mpsc::channel(16);
        let (dtx, _drx) = mpsc::channel(16);
        sinks.insert(s1, SessionSink::new(dtx, stx));

        // 1. Unauthenticated MarketListRequest -> UNAUTHORIZED
        let list_pkt = ClientPacket::MarketListRequest(MarketListRequestPacket {
            item_instance_id: "inst_1".to_string(),
            src_container_id: 10,
            src_slot: 0,
            price: 100,
            duration_secs: 3600,
        });
        assert!(handler.handle_packet(s1, &list_pkt).await);
        let bytes = srx.try_recv().expect("response sent");
        let resp = ClientPacket::decode(&bytes).expect("decodable");
        match resp {
            ClientPacket::MarketResponse(m) => {
                assert_eq!(m.request_type, 1);
                assert!(!m.success);
                assert_eq!(m.error_code, market_error_codes::UNAUTHORIZED);
            }
            other => panic!("expected MarketResponse, got {other:?}"),
        }

        // 2. Unauthenticated MarketBuyRequest -> UNAUTHORIZED
        let buy_pkt = ClientPacket::MarketBuyRequest(MarketBuyRequestPacket {
            listing_id: "list_1".to_string(),
            buyer_container_id: 10,
            payment_container_id: 11,
            price: 100,
        });
        assert!(handler.handle_packet(s1, &buy_pkt).await);
        let bytes = srx.try_recv().expect("response sent");
        let resp = ClientPacket::decode(&bytes).expect("decodable");
        match resp {
            ClientPacket::MarketResponse(m) => {
                assert_eq!(m.request_type, 2);
                assert_eq!(m.error_code, market_error_codes::UNAUTHORIZED);
            }
            other => panic!("expected MarketResponse, got {other:?}"),
        }

        // 3. Unauthenticated MarketCancelRequest -> UNAUTHORIZED
        let cancel_pkt = ClientPacket::MarketCancelRequest(MarketCancelRequestPacket {
            listing_id: "list_1".to_string(),
            dst_container_id: 10,
        });
        assert!(handler.handle_packet(s1, &cancel_pkt).await);
        let bytes = srx.try_recv().expect("response sent");
        let resp = ClientPacket::decode(&bytes).expect("decodable");
        match resp {
            ClientPacket::MarketResponse(m) => {
                assert_eq!(m.request_type, 3);
                assert_eq!(m.error_code, market_error_codes::UNAUTHORIZED);
            }
            other => panic!("expected MarketResponse, got {other:?}"),
        }

        // 4. MarketQueryRequest with service None -> INTERNAL_ERROR
        let query_pkt = ClientPacket::MarketQueryRequest(MarketQueryRequestPacket {
            item_def_id: 1,
            min_price: 10,
            max_price: 1000,
            offset: 0,
            limit: 10,
        });
        assert!(handler.handle_packet(s1, &query_pkt).await);
        let bytes = srx.try_recv().expect("response sent");
        let resp = ClientPacket::decode(&bytes).expect("decodable");
        match resp {
            ClientPacket::MarketResponse(m) => {
                assert_eq!(m.request_type, 4);
                assert_eq!(m.error_code, market_error_codes::INTERNAL_ERROR);
            }
            other => panic!("expected MarketResponse, got {other:?}"),
        }

        // 5. Unsolicited MarketResponse from client intercepted & dropped
        let unsolicited = ClientPacket::MarketResponse(MarketResponsePacket {
            request_type: 1,
            success: true,
            error_code: 0,
            error_message: String::new(),
            listings: vec![],
        });
        assert!(handler.handle_packet(s1, &unsolicited).await);
        assert!(srx.try_recv().is_err());
    }
}
