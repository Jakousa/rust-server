//! Unopinionated, genre-agnostic social messaging and pub/sub event abstractions.

use serde::{Deserialize, Serialize};

use crate::guid::Guid;

/// Generalized channel scope for cross-node and edge routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SocialChannel {
    Global,
    DirectWhisper,
    Group,
    Organization,
    Custom(u16),
}

/// Abstract social event dispatched across edge gateways and simulation nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SocialEvent {
    pub channel: SocialChannel,
    pub sender_guid: Guid,
    pub target_id: Option<u64>,
    pub payload: Vec<u8>,
    pub timestamp: u64,
}

impl SocialEvent {
    pub fn new(
        channel: SocialChannel,
        sender_guid: Guid,
        target_id: Option<u64>,
        payload: Vec<u8>,
        timestamp: u64,
    ) -> Self {
        Self {
            channel,
            sender_guid,
            target_id,
            payload,
            timestamp,
        }
    }

    /// Derives the standard pub/sub routing topic string for this event.
    pub fn topic(&self) -> String {
        match self.channel {
            SocialChannel::Global => "chat:global".to_string(),
            SocialChannel::DirectWhisper => {
                format!("whisper:{}", self.target_id.unwrap_or(0))
            }
            SocialChannel::Group => {
                format!("group:{}", self.target_id.unwrap_or(0))
            }
            SocialChannel::Organization => {
                format!("org:{}", self.target_id.unwrap_or(0))
            }
            SocialChannel::Custom(id) => format!("custom:{}", id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topic_formatting() {
        let guid = Guid::from_raw(12345);
        let global_event = SocialEvent::new(SocialChannel::Global, guid, None, vec![], 0);
        assert_eq!(global_event.topic(), "chat:global");

        let whisper_event =
            SocialEvent::new(SocialChannel::DirectWhisper, guid, Some(67890), vec![], 0);
        assert_eq!(whisper_event.topic(), "whisper:67890");

        let group_event = SocialEvent::new(SocialChannel::Group, guid, Some(67890), vec![], 0);
        assert_eq!(group_event.topic(), "group:67890");

        let org_event = SocialEvent::new(SocialChannel::Organization, guid, Some(67890), vec![], 0);
        assert_eq!(org_event.topic(), "org:67890");

        let custom_event = SocialEvent::new(SocialChannel::Custom(42), guid, None, vec![], 0);
        assert_eq!(custom_event.topic(), "custom:42");
    }

    #[test]
    fn test_serialization_roundtrip() {
        let guid = Guid::from_raw(12345);
        let event = SocialEvent::new(SocialChannel::Global, guid, None, vec![1, 2, 3], 1633072800);

        let serialized = serde_json::to_string(&event).unwrap();
        let deserialized: SocialEvent = serde_json::from_str(&serialized).unwrap();

        assert_eq!(event, deserialized);
    }
}
