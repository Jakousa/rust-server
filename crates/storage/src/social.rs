//! Cross-node social and gameplay message bus.
//!
//! Provides topic-based pub/sub broadcasting and point-to-point routing for
//! global chat, party formation, direct whispers, and guild synchronization.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

/// Channel scope for chat messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChatChannel {
    Global,
    Area,
    Guild,
    Party,
    Whisper,
}

/// Chat message payload routed across the cluster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub channel: ChatChannel,
    pub sender_guid: u64,
    pub sender_name: String,
    pub target_guid: Option<u64>,
    pub content: String,
    pub timestamp: u64,
}

/// Party action event for group coordination across simulation nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartyAction {
    Invite {
        party_id: u64,
        inviter_guid: u64,
        invitee_guid: u64,
    },
    Accept {
        party_id: u64,
        member_guid: u64,
    },
    Leave {
        party_id: u64,
        member_guid: u64,
    },
    Disband {
        party_id: u64,
    },
}

/// Guild event for replicating real-time updates across nodes housing members.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GuildSyncAction {
    VaultUpdated {
        guild_id: u64,
        actor_guid: u64,
    },
    MemberJoined {
        guild_id: u64,
        member_guid: u64,
        rank: u8,
    },
    MemberLeft {
        guild_id: u64,
        member_guid: u64,
    },
    RankChanged {
        guild_id: u64,
        member_guid: u64,
        new_rank: u8,
    },
}

/// Container and escrow synchronization action across simulation nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContainerSyncAction {
    Transferred {
        tx_id: String,
        src_container_id: u64,
        dst_container_id: u64,
        item_instance_id: String,
    },
    EscrowUpdated {
        escrow_id: String,
        status: String,
    },
}

/// Universal wrapper envelope dispatched across social bus channels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SocialEnvelope {
    Chat(ChatMessage),
    Party(PartyAction),
    Guild(GuildSyncAction),
    Container(ContainerSyncAction),
}

/// Errors occurring during social bus operations.
#[derive(Debug, thiserror::Error)]
pub enum SocialBusError {
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("Bus communication error: {0}")]
    Channel(String),
}

/// Trait defining the asynchronous cross-node social pub/sub bus.
#[async_trait::async_trait]
pub trait SocialBus: Send + Sync {
    /// Publishes an envelope to a named topic channel.
    async fn publish(&self, channel: &str, envelope: &SocialEnvelope)
        -> Result<(), SocialBusError>;

    /// Subscribes to a named topic channel, returning a broadcast receiver.
    async fn subscribe(
        &self,
        channel: &str,
    ) -> Result<broadcast::Receiver<SocialEnvelope>, SocialBusError>;

    /// Unsubscribes from a named topic channel, freeing resources if no active receivers remain.
    async fn unsubscribe(&self, _channel: &str) -> Result<(), SocialBusError> {
        Ok(())
    }
}

/// In-memory broadcast social bus ideal for standalone testing or single-process topologies.
#[derive(Clone, Default)]
pub struct MemorySocialBus {
    channels: Arc<Mutex<HashMap<String, broadcast::Sender<SocialEnvelope>>>>,
}

impl MemorySocialBus {
    pub const DEFAULT_CAPACITY: usize = 1024;

    pub fn new() -> Self {
        Self {
            channels: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn get_or_create_sender(&self, channel: &str) -> broadcast::Sender<SocialEnvelope> {
        let mut channels = self.channels.lock().unwrap();
        channels
            .entry(channel.to_string())
            .or_insert_with(|| broadcast::channel(Self::DEFAULT_CAPACITY).0)
            .clone()
    }
}

#[async_trait::async_trait]
impl SocialBus for MemorySocialBus {
    async fn publish(
        &self,
        channel: &str,
        envelope: &SocialEnvelope,
    ) -> Result<(), SocialBusError> {
        let sender = self.get_or_create_sender(channel);
        // Ignore SendError if there are currently no active receivers on the channel
        let _ = sender.send(envelope.clone());
        Ok(())
    }

    async fn subscribe(
        &self,
        channel: &str,
    ) -> Result<broadcast::Receiver<SocialEnvelope>, SocialBusError> {
        let sender = self.get_or_create_sender(channel);
        Ok(sender.subscribe())
    }

    async fn unsubscribe(&self, channel: &str) -> Result<(), SocialBusError> {
        let mut channels = self.channels.lock().unwrap();
        if let Some(sender) = channels.get(channel) {
            if sender.receiver_count() == 0 {
                channels.remove(channel);
            }
        }
        Ok(())
    }
}

/// Wire payload envelope exchanged over Redis PubSub to identify message origin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedisBusWireMessage {
    pub origin_node_id: String,
    pub envelope: SocialEnvelope,
}

/// Redis-backed social bus for distributed, multi-process clusters.
pub struct RedisSocialBus {
    client: redis::Client,
    node_id: String,
    memory_fallback: MemorySocialBus,
    publish_conn: Arc<tokio::sync::RwLock<(u64, Option<redis::aio::MultiplexedConnection>)>>,
    subscribed_channels: Arc<Mutex<std::collections::HashSet<String>>>,
    pubsub_sink: Arc<tokio::sync::RwLock<Option<redis::aio::PubSubSink>>>,
    pubsub_task: Arc<Mutex<Option<tokio::task::AbortHandle>>>,
}

impl Drop for RedisSocialBus {
    fn drop(&mut self) {
        if let Some(handle) = self.pubsub_task.lock().unwrap().take() {
            handle.abort();
        }
    }
}

impl RedisSocialBus {
    pub fn new(redis_url: &str) -> Result<Self, SocialBusError> {
        let client =
            redis::Client::open(redis_url).map_err(|e| SocialBusError::Channel(e.to_string()))?;
        let random_id: u64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let node_id = format!("node_{:x}", random_id);
        Ok(Self {
            client,
            node_id,
            memory_fallback: MemorySocialBus::new(),
            publish_conn: Arc::new(tokio::sync::RwLock::new((0, None))),
            subscribed_channels: Arc::new(Mutex::new(std::collections::HashSet::new())),
            pubsub_sink: Arc::new(tokio::sync::RwLock::new(None)),
            pubsub_task: Arc::new(Mutex::new(None)),
        })
    }

    /// Creates a new `RedisSocialBus` with an explicit node ID.
    pub fn with_node_id(
        redis_url: &str,
        node_id: impl Into<String>,
    ) -> Result<Self, SocialBusError> {
        let client =
            redis::Client::open(redis_url).map_err(|e| SocialBusError::Channel(e.to_string()))?;
        Ok(Self {
            client,
            node_id: node_id.into(),
            memory_fallback: MemorySocialBus::new(),
            publish_conn: Arc::new(tokio::sync::RwLock::new((0, None))),
            subscribed_channels: Arc::new(Mutex::new(std::collections::HashSet::new())),
            pubsub_sink: Arc::new(tokio::sync::RwLock::new(None)),
            pubsub_task: Arc::new(Mutex::new(None)),
        })
    }

    /// Accessor for the bus's unique node ID.
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    async fn get_publish_connection(
        &self,
    ) -> Result<(u64, redis::aio::MultiplexedConnection), redis::RedisError> {
        {
            let guard = self.publish_conn.read().await;
            if let Some(ref conn) = guard.1 {
                return Ok((guard.0, conn.clone()));
            }
        }
        let mut guard = self.publish_conn.write().await;
        if let Some(ref conn) = guard.1 {
            return Ok((guard.0, conn.clone()));
        }
        let new_conn = self.client.get_multiplexed_tokio_connection().await?;
        guard.0 = guard.0.wrapping_add(1);
        guard.1 = Some(new_conn.clone());
        Ok((guard.0, new_conn))
    }

    async fn invalidate_publish_connection(&self, failed_gen: u64) {
        let mut guard = self.publish_conn.write().await;
        if guard.0 == failed_gen {
            guard.1 = None;
        }
    }

    fn ensure_pubsub_task(&self) {
        let mut task_guard = self.pubsub_task.lock().unwrap();
        if task_guard.is_some() {
            return;
        }

        let client = self.client.clone();
        let memory_fallback = self.memory_fallback.clone();
        let my_node_id = self.node_id.clone();
        let channels_set = self.subscribed_channels.clone();
        let pubsub_sink_ref = self.pubsub_sink.clone();
        let pubsub_task_ref = self.pubsub_task.clone();

        let handle = tokio::spawn(async move {
            use futures_util::StreamExt;
            let mut retry_delay_ms = 100u64;
            loop {
                let has_channels = !channels_set.lock().unwrap().is_empty();
                if !has_channels {
                    break;
                }

                match client.get_async_pubsub().await {
                    Ok(pubsub) => {
                        let (mut sink, mut stream) = pubsub.split();
                        // Store sink BEFORE snapshotting subscribed_channels so concurrent
                        // subscribe() calls cannot miss registration.
                        *pubsub_sink_ref.write().await = Some(sink.clone());

                        let snapshot: Vec<String> =
                            channels_set.lock().unwrap().iter().cloned().collect();
                        let mut sub_ok = true;
                        for ch in &snapshot {
                            if sink.subscribe(ch).await.is_err() {
                                sub_ok = false;
                                break;
                            }
                        }

                        if sub_ok {
                            retry_delay_ms = 100;
                            while let Some(msg) = stream.next().await {
                                let channel_name = msg.get_channel_name().to_string();
                                let payload: String = match msg.get_payload() {
                                    Ok(p) => p,
                                    Err(_) => continue,
                                };
                                if let Ok(wire) =
                                    serde_json::from_str::<RedisBusWireMessage>(&payload)
                                {
                                    if wire.origin_node_id != my_node_id {
                                        let _ = memory_fallback
                                            .publish(&channel_name, &wire.envelope)
                                            .await;
                                    }
                                } else if let Ok(envelope) =
                                    serde_json::from_str::<SocialEnvelope>(&payload)
                                {
                                    let _ = memory_fallback.publish(&channel_name, &envelope).await;
                                }
                            }
                        }

                        *pubsub_sink_ref.write().await = None;
                    }
                    Err(_) => {
                        tokio::time::sleep(tokio::time::Duration::from_millis(retry_delay_ms))
                            .await;
                        retry_delay_ms = (retry_delay_ms * 2).min(5000);
                    }
                }

                if channels_set.lock().unwrap().is_empty() {
                    break;
                }
            }
            *pubsub_task_ref.lock().unwrap() = None;
        });

        *task_guard = Some(handle.abort_handle());
    }
}

#[async_trait::async_trait]
impl SocialBus for RedisSocialBus {
    async fn publish(
        &self,
        channel: &str,
        envelope: &SocialEnvelope,
    ) -> Result<(), SocialBusError> {
        let wire = RedisBusWireMessage {
            origin_node_id: self.node_id.clone(),
            envelope: envelope.clone(),
        };
        let json = serde_json::to_string(&wire)?;
        if let Ok((gen_id, mut conn)) = self.get_publish_connection().await {
            if redis::AsyncCommands::publish::<_, _, ()>(&mut conn, channel, &json)
                .await
                .is_err()
            {
                self.invalidate_publish_connection(gen_id).await;
                if let Ok((_, mut fresh_conn)) = self.get_publish_connection().await {
                    let _: Result<(), _> =
                        redis::AsyncCommands::publish(&mut fresh_conn, channel, &json).await;
                }
            }
        }
        // Also broadcast into memory fallback for local cluster subscribers
        self.memory_fallback.publish(channel, envelope).await
    }

    async fn subscribe(
        &self,
        channel: &str,
    ) -> Result<broadcast::Receiver<SocialEnvelope>, SocialBusError> {
        let newly_inserted = {
            let mut set = self.subscribed_channels.lock().unwrap();
            set.insert(channel.to_string())
        };

        self.ensure_pubsub_task();

        if newly_inserted {
            let sink_opt = self.pubsub_sink.read().await.clone();
            if let Some(mut sink) = sink_opt {
                let _ = sink.subscribe(channel).await;
            }
        }

        self.memory_fallback.subscribe(channel).await
    }

    async fn unsubscribe(&self, channel: &str) -> Result<(), SocialBusError> {
        let is_empty = {
            let mut set = self.subscribed_channels.lock().unwrap();
            set.remove(channel);
            set.is_empty()
        };

        let sink_opt = self.pubsub_sink.read().await.clone();
        if let Some(mut sink) = sink_opt {
            let _ = sink.unsubscribe(channel).await;
        }

        if is_empty {
            if let Some(abort_handle) = self.pubsub_task.lock().unwrap().take() {
                abort_handle.abort();
            }
            *self.pubsub_sink.write().await = None;
        }

        self.memory_fallback.unsubscribe(channel).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_memory_social_bus_chat_pub_sub() {
        let bus = MemorySocialBus::new();
        let mut rx = bus.subscribe("chat:global").await.unwrap();

        let chat = ChatMessage {
            channel: ChatChannel::Global,
            sender_guid: 100,
            sender_name: "Hero".into(),
            target_guid: None,
            content: "Greetings adventurers!".into(),
            timestamp: 1000,
        };

        bus.publish("chat:global", &SocialEnvelope::Chat(chat.clone()))
            .await
            .unwrap();

        let received = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("timed out waiting for message")
            .unwrap();
        assert_eq!(received, SocialEnvelope::Chat(chat));
    }

    #[tokio::test]
    async fn test_memory_social_bus_whisper_point_to_point() {
        let bus = MemorySocialBus::new();
        let target_guid = 200;
        let whisper_channel = format!("whisper:{}", target_guid);

        let mut rx = bus.subscribe(&whisper_channel).await.unwrap();

        let whisper = ChatMessage {
            channel: ChatChannel::Whisper,
            sender_guid: 100,
            sender_name: "Hero".into(),
            target_guid: Some(target_guid),
            content: "Meet me at the tavern.".into(),
            timestamp: 1005,
        };

        bus.publish(&whisper_channel, &SocialEnvelope::Chat(whisper.clone()))
            .await
            .unwrap();

        let received = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("timed out waiting for message")
            .unwrap();
        assert_eq!(received, SocialEnvelope::Chat(whisper));
    }

    #[tokio::test]
    async fn test_memory_social_bus_unsubscribe() {
        let bus = MemorySocialBus::new();
        let rx = bus.subscribe("chat:temp").await.unwrap();
        assert_eq!(bus.channels.lock().unwrap().len(), 1);
        drop(rx);
        bus.unsubscribe("chat:temp").await.unwrap();
        assert_eq!(bus.channels.lock().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_social_envelopes_and_redis_bus() {
        let party_env = SocialEnvelope::Party(PartyAction::Invite {
            party_id: 1,
            inviter_guid: 10,
            invitee_guid: 20,
        });
        let guild_env = SocialEnvelope::Guild(GuildSyncAction::MemberJoined {
            guild_id: 5,
            member_guid: 10,
            rank: 1,
        });
        let cont_env = SocialEnvelope::Container(ContainerSyncAction::Transferred {
            tx_id: "tx_1".into(),
            src_container_id: 1,
            dst_container_id: 2,
            item_instance_id: "i1".into(),
        });

        for env in [&party_env, &guild_env, &cont_env] {
            let json = serde_json::to_string(env).unwrap();
            let dec: SocialEnvelope = serde_json::from_str(&json).unwrap();
            assert_eq!(*env, dec);
        }

        assert!(RedisSocialBus::new("invalid_url").is_err());
    }

    #[test]
    fn test_redis_wire_message_codec_and_node_id() {
        let envelope = SocialEnvelope::Party(PartyAction::Disband { party_id: 42 });
        let wire = RedisBusWireMessage {
            origin_node_id: "node_alpha".to_string(),
            envelope: envelope.clone(),
        };

        let json = serde_json::to_string(&wire).unwrap();
        let decoded: RedisBusWireMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(wire, decoded);
        assert_eq!(decoded.origin_node_id, "node_alpha");
        assert_eq!(decoded.envelope, envelope);

        // Explicit node_id construction
        let bus = RedisSocialBus::with_node_id("redis://127.0.0.1:6379", "test_node_42").unwrap();
        assert_eq!(bus.node_id(), "test_node_42");
    }

    #[tokio::test]
    async fn test_redis_social_bus_subscribe_offline_fallback() {
        // Pointing to a dummy non-existent port
        let bus = RedisSocialBus::new("redis://127.0.0.1:1").unwrap();
        let mut rx = bus.subscribe("fallback_channel").await.unwrap();

        let env = SocialEnvelope::Guild(GuildSyncAction::MemberLeft {
            guild_id: 100,
            member_guid: 200,
        });

        // Publish should deliver locally via fallback despite Redis being offline
        bus.publish("fallback_channel", &env).await.unwrap();

        let received = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("timeout waiting for local fallback delivery")
            .unwrap();
        assert_eq!(received, env);
    }

    async fn read_resp_command(
        buf: &mut Vec<u8>,
        stream: &mut tokio::net::TcpStream,
    ) -> Option<Vec<Vec<u8>>> {
        use tokio::io::AsyncReadExt;
        loop {
            if let Some((args, consumed)) = parse_resp_array(buf) {
                buf.drain(..consumed);
                return Some(args);
            }
            let mut tmp = [0u8; 2048];
            let n = stream.read(&mut tmp).await.ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&tmp[..n]);
        }
    }

    fn parse_resp_array(buf: &[u8]) -> Option<(Vec<Vec<u8>>, usize)> {
        let crlf = buf.windows(2).position(|w| w == b"\r\n")?;
        if *buf.first()? != b'*' {
            return None;
        }
        let count: usize = std::str::from_utf8(&buf[1..crlf]).ok()?.parse().ok()?;
        let mut pos = crlf + 2;
        let mut args = Vec::with_capacity(count);
        for _ in 0..count {
            if *buf.get(pos)? != b'$' {
                return None;
            }
            let len_end = buf[pos..].windows(2).position(|w| w == b"\r\n")? + pos;
            let len: usize = std::str::from_utf8(&buf[pos + 1..len_end])
                .ok()?
                .parse()
                .ok()?;
            pos = len_end + 2;
            if buf.len() < pos + len + 2 {
                return None;
            }
            args.push(buf[pos..pos + len].to_vec());
            pos += len + 2;
        }
        Some((args, pos))
    }

    fn encode_resp_pubsub_msg(kind: &str, channel: &[u8], count: usize) -> Vec<u8> {
        let mut out = format!(
            "*3\r\n${}\r\n{}\r\n${}\r\n",
            kind.len(),
            kind,
            channel.len()
        )
        .into_bytes();
        out.extend_from_slice(channel);
        out.extend_from_slice(format!("\r\n:{count}\r\n").as_bytes());
        out
    }

    fn encode_resp_message(channel: &[u8], payload: &[u8]) -> Vec<u8> {
        let mut out = format!("*3\r\n$7\r\nmessage\r\n${}\r\n", channel.len()).into_bytes();
        out.extend_from_slice(channel);
        out.extend_from_slice(format!("\r\n${}\r\n", payload.len()).as_bytes());
        out.extend_from_slice(payload);
        out.extend_from_slice(b"\r\n");
        out
    }

    #[tokio::test]
    async fn test_redis_social_bus_reuses_connections_and_multiplexes_pubsub() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::AsyncWriteExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accept_count = Arc::new(AtomicUsize::new(0));
        let subscribed_topics: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let unsubscribed_topics: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let (push_tx, _) = tokio::sync::broadcast::channel::<(String, Vec<u8>)>(32);

        let accept_count_srv = accept_count.clone();
        let subs_srv = subscribed_topics.clone();
        let unsubs_srv = unsubscribed_topics.clone();
        let push_tx_srv = push_tx.clone();
        let server_task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                accept_count_srv.fetch_add(1, Ordering::SeqCst);
                let subs = subs_srv.clone();
                let unsubs = unsubs_srv.clone();
                let mut push_rx = push_tx_srv.subscribe();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut is_pubsub = false;
                    loop {
                        tokio::select! {
                            cmd = read_resp_command(&mut buf, &mut stream) => {
                                let Some(args) = cmd else { break; };
                                let verb = String::from_utf8_lossy(&args[0]).to_ascii_uppercase();
                                match verb.as_str() {
                                    "PUBLISH" => {
                                        if stream.write_all(b":1\r\n").await.is_err() {
                                            break;
                                        }
                                    }
                                    "SUBSCRIBE" => {
                                        is_pubsub = true;
                                        for ch in &args[1..] {
                                            let topic = String::from_utf8_lossy(ch).to_string();
                                            let count = {
                                                let mut g = subs.lock().unwrap();
                                                g.push(topic);
                                                g.len()
                                            };
                                            let frame = encode_resp_pubsub_msg("subscribe", ch, count);
                                            if stream.write_all(&frame).await.is_err() {
                                                return;
                                            }
                                        }
                                    }
                                    "UNSUBSCRIBE" => {
                                        for ch in &args[1..] {
                                            let topic = String::from_utf8_lossy(ch).to_string();
                                            unsubs.lock().unwrap().push(topic);
                                            let frame = encode_resp_pubsub_msg("unsubscribe", ch, 0);
                                            if stream.write_all(&frame).await.is_err() {
                                                return;
                                            }
                                        }
                                    }
                                    "PING" => {
                                        if stream.write_all(b"+PONG\r\n").await.is_err() {
                                            break;
                                        }
                                    }
                                    _ => {
                                        if stream.write_all(b"+OK\r\n").await.is_err() {
                                            break;
                                        }
                                    }
                                }
                            }
                            Ok((ch, payload)) = push_rx.recv(), if is_pubsub => {
                                let frame = encode_resp_message(ch.as_bytes(), &payload);
                                if stream.write_all(&frame).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                });
            }
        });

        let bus = RedisSocialBus::with_node_id(&format!("redis://{addr}"), "gateway_1").unwrap();

        // 1. Multiple topic subscriptions must share ONE single PubSub TCP connection.
        let mut rx_global = bus.subscribe("chat:global").await.unwrap();
        let mut rx_guild = bus.subscribe("chat:guild:1").await.unwrap();
        let mut rx_party = bus.subscribe("chat:party:2").await.unwrap();

        // Wait briefly for async SUBSCRIBE commands to reach the mock server
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if subscribed_topics.lock().unwrap().len() == 3 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("timed out waiting for multiplexed SUBSCRIBE commands");

        assert_eq!(
            accept_count.load(Ordering::SeqCst),
            1,
            "All 3 topic subscriptions must multiplex over a single PubSub TCP connection"
        );

        // 2. Deliver remote messages for two topics over the single PubSub connection.
        let env_global = SocialEnvelope::Guild(GuildSyncAction::MemberJoined {
            guild_id: 1,
            member_guid: 50,
            rank: 2,
        });
        let wire_global = serde_json::to_vec(&RedisBusWireMessage {
            origin_node_id: "remote_node".to_string(),
            envelope: env_global.clone(),
        })
        .unwrap();
        push_tx
            .send(("chat:global".to_string(), wire_global))
            .unwrap();

        let env_party = SocialEnvelope::Party(PartyAction::Disband { party_id: 2 });
        let wire_party = serde_json::to_vec(&RedisBusWireMessage {
            origin_node_id: "remote_node".to_string(),
            envelope: env_party.clone(),
        })
        .unwrap();
        push_tx
            .send(("chat:party:2".to_string(), wire_party))
            .unwrap();

        let got_global = tokio::time::timeout(std::time::Duration::from_secs(2), rx_global.recv())
            .await
            .expect("global recv timeout")
            .unwrap();
        assert_eq!(got_global, env_global);

        let got_party = tokio::time::timeout(std::time::Duration::from_secs(2), rx_party.recv())
            .await
            .expect("party recv timeout")
            .unwrap();
        assert_eq!(got_party, env_party);
        assert!(rx_guild.try_recv().is_err());

        // 3. Multiple publishes must reuse ONE persistent publish TCP connection (total connections = 2).
        for i in 0..5u64 {
            let env = SocialEnvelope::Party(PartyAction::Disband { party_id: i });
            bus.publish("chat:party:2", &env).await.unwrap();
        }
        assert_eq!(
            accept_count.load(Ordering::SeqCst),
            2,
            "5 publishes + 3 topic subscriptions must use at most 2 TCP connections total"
        );

        // 4. Unsubscribe forwards UNSUBSCRIBE over the shared PubSub connection.
        bus.unsubscribe("chat:guild:1").await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if unsubscribed_topics
                    .lock()
                    .unwrap()
                    .contains(&"chat:guild:1".to_string())
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("timed out waiting for UNSUBSCRIBE");
        assert_eq!(accept_count.load(Ordering::SeqCst), 2);

        server_task.abort();
    }
}
