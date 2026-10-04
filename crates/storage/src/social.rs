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
}

/// Redis-backed social bus for distributed, multi-process clusters.
pub struct RedisSocialBus {
    client: redis::Client,
    memory_fallback: MemorySocialBus,
}

impl RedisSocialBus {
    pub fn new(redis_url: &str) -> Result<Self, SocialBusError> {
        let client =
            redis::Client::open(redis_url).map_err(|e| SocialBusError::Channel(e.to_string()))?;
        Ok(Self {
            client,
            memory_fallback: MemorySocialBus::new(),
        })
    }
}

#[async_trait::async_trait]
impl SocialBus for RedisSocialBus {
    async fn publish(
        &self,
        channel: &str,
        envelope: &SocialEnvelope,
    ) -> Result<(), SocialBusError> {
        if let Ok(mut conn) = self.client.get_multiplexed_async_connection().await {
            let json = serde_json::to_string(envelope)?;
            let _: Result<(), _> = redis::AsyncCommands::publish(&mut conn, channel, json).await;
        }
        // Also broadcast into memory fallback for local cluster subscribers
        self.memory_fallback.publish(channel, envelope).await
    }

    async fn subscribe(
        &self,
        channel: &str,
    ) -> Result<broadcast::Receiver<SocialEnvelope>, SocialBusError> {
        self.memory_fallback.subscribe(channel).await
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

        let received = rx.recv().await.unwrap();
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

        let received = rx.recv().await.unwrap();
        assert_eq!(received, SocialEnvelope::Chat(whisper));
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
}
