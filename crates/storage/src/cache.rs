//! Distributed and in-memory session cache for player presence and handover staging.

use crate::db::StorageError;
use crate::models::EntitySnapshot;
use async_trait::async_trait;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Active session entry stored in the distributed cache.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionData {
    pub session_id: u64,
    pub guid: u64,
    pub node_address: String,
    pub expires_at: u64,
}

/// Staged entity migration data during boundary handovers between simulation nodes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HandoverData {
    pub guid: u64,
    pub source_node: u32,
    pub target_node: u32,
    pub snapshot: EntitySnapshot,
    pub staged_at: u64,
}

/// Pluggable session and handover cache backend trait.
#[async_trait]
pub trait SessionCacheBackend: Send + Sync {
    /// Stores or refreshes a session entry with TTL in seconds.
    async fn set_session(&self, data: &SessionData, ttl_secs: u64) -> Result<(), StorageError>;

    /// Retrieves an active session by session ID.
    async fn get_session(&self, session_id: u64) -> Result<Option<SessionData>, StorageError>;

    /// Removes a session upon client disconnect.
    async fn remove_session(&self, session_id: u64) -> Result<bool, StorageError>;

    /// Stages serialized entity handover state with TTL.
    async fn stage_handover(&self, data: &HandoverData, ttl_secs: u64) -> Result<(), StorageError>;

    /// Atomically claims and removes a staged handover for an entity.
    async fn claim_handover(&self, guid: u64) -> Result<Option<HandoverData>, StorageError>;
}

/// Thread-safe in-memory cache backend for tests and local execution.
#[derive(Clone, Default)]
pub struct MemoryCache {
    sessions: Arc<RwLock<HashMap<u64, (SessionData, Instant)>>>,
    handovers: Arc<RwLock<HashMap<u64, (HandoverData, Instant)>>>,
}

impl MemoryCache {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SessionCacheBackend for MemoryCache {
    async fn set_session(&self, data: &SessionData, ttl_secs: u64) -> Result<(), StorageError> {
        let expire = Instant::now() + Duration::from_secs(ttl_secs);
        let mut map = self.sessions.write().await;
        map.insert(data.session_id, (data.clone(), expire));
        Ok(())
    }

    async fn get_session(&self, session_id: u64) -> Result<Option<SessionData>, StorageError> {
        let mut map = self.sessions.write().await;
        if let Some((data, expire)) = map.get(&session_id) {
            if Instant::now() <= *expire {
                return Ok(Some(data.clone()));
            }
        }
        map.remove(&session_id);
        Ok(None)
    }

    async fn remove_session(&self, session_id: u64) -> Result<bool, StorageError> {
        let mut map = self.sessions.write().await;
        Ok(map.remove(&session_id).is_some())
    }

    async fn stage_handover(&self, data: &HandoverData, ttl_secs: u64) -> Result<(), StorageError> {
        let expire = Instant::now() + Duration::from_secs(ttl_secs);
        let mut map = self.handovers.write().await;
        map.insert(data.guid, (data.clone(), expire));
        Ok(())
    }

    async fn claim_handover(&self, guid: u64) -> Result<Option<HandoverData>, StorageError> {
        let mut map = self.handovers.write().await;
        if let Some((data, expire)) = map.remove(&guid) {
            if Instant::now() <= expire {
                return Ok(Some(data));
            }
        }
        Ok(None)
    }
}

/// Redis-backed cache backend for multi-container and production deployments.
#[derive(Clone)]
pub struct RedisCache {
    client: redis::Client,
}

impl RedisCache {
    pub fn new(redis_url: &str) -> Result<Self, StorageError> {
        let client = redis::Client::open(redis_url)?;
        Ok(Self { client })
    }
}

#[async_trait]
impl SessionCacheBackend for RedisCache {
    async fn set_session(&self, data: &SessionData, ttl_secs: u64) -> Result<(), StorageError> {
        let mut conn = self.client.get_multiplexed_tokio_connection().await?;
        let key = format!("mmo:session:{}", data.session_id);
        let val = serde_json::to_string(data)?;
        conn.set_ex::<_, _, ()>(key, val, ttl_secs).await?;
        Ok(())
    }

    async fn get_session(&self, session_id: u64) -> Result<Option<SessionData>, StorageError> {
        let mut conn = self.client.get_multiplexed_tokio_connection().await?;
        let key = format!("mmo:session:{}", session_id);
        let val: Option<String> = conn.get(key).await?;
        match val {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    async fn remove_session(&self, session_id: u64) -> Result<bool, StorageError> {
        let mut conn = self.client.get_multiplexed_tokio_connection().await?;
        let key = format!("mmo:session:{}", session_id);
        let deleted: i64 = conn.del(key).await?;
        Ok(deleted > 0)
    }

    async fn stage_handover(&self, data: &HandoverData, ttl_secs: u64) -> Result<(), StorageError> {
        let mut conn = self.client.get_multiplexed_tokio_connection().await?;
        let key = format!("mmo:handover:{}", data.guid);
        let val = serde_json::to_string(data)?;
        conn.set_ex::<_, _, ()>(key, val, ttl_secs).await?;
        Ok(())
    }

    async fn claim_handover(&self, guid: u64) -> Result<Option<HandoverData>, StorageError> {
        let mut conn = self.client.get_multiplexed_tokio_connection().await?;
        let key = format!("mmo:handover:{}", guid);
        let val: Option<String> = conn.get(&key).await?;
        if let Some(s) = val {
            let _: () = conn.del(&key).await?;
            Ok(Some(serde_json::from_str(&s)?))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_memory_cache_session_lifecycle() {
        let cache = MemoryCache::new();

        let session = SessionData {
            session_id: 123,
            guid: 456,
            node_address: "127.0.0.1:50052".to_string(),
            expires_at: 9999,
        };

        cache.set_session(&session, 60).await.unwrap();

        let fetched = cache.get_session(123).await.unwrap().unwrap();
        assert_eq!(fetched.guid, 456);
        assert_eq!(fetched.node_address, "127.0.0.1:50052");

        let removed = cache.remove_session(123).await.unwrap();
        assert!(removed);
        assert!(cache.get_session(123).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_memory_cache_handover_claim() {
        let cache = MemoryCache::new();

        let handover = HandoverData {
            guid: 999,
            source_node: 1,
            target_node: 2,
            snapshot: EntitySnapshot {
                guid: 999,
                position: (10.0, 20.0, 30.0),
                rotation: 0.0,
                attributes: serde_json::json!({}),
                version: 1,
                timestamp: 1000,
            },
            staged_at: 1000,
        };

        cache.stage_handover(&handover, 30).await.unwrap();

        let claimed = cache.claim_handover(999).await.unwrap().unwrap();
        assert_eq!(claimed.source_node, 1);
        assert_eq!(claimed.target_node, 2);

        // Atomic claim should remove entry
        assert!(cache.claim_handover(999).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_memory_cache_expiration_and_not_found() {
        let cache = MemoryCache::new();

        // 1. Non-existent removal
        assert!(!cache.remove_session(9999).await.unwrap());
        assert!(cache.get_session(9999).await.unwrap().is_none());

        // 2. Session expiration (0s TTL)
        let session = SessionData {
            session_id: 888,
            guid: 777,
            node_address: "127.0.0.1:50053".to_string(),
            expires_at: 1000,
        };
        cache.set_session(&session, 0).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
        assert!(cache.get_session(888).await.unwrap().is_none());

        // 3. Handover expiration (0s TTL)
        let handover = HandoverData {
            guid: 555,
            source_node: 1,
            target_node: 2,
            snapshot: EntitySnapshot {
                guid: 555,
                position: (0.0, 0.0, 0.0),
                rotation: 0.0,
                attributes: serde_json::json!({}),
                version: 1,
                timestamp: 1000,
            },
            staged_at: 1000,
        };
        cache.stage_handover(&handover, 0).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
        assert!(cache.claim_handover(555).await.unwrap().is_none());
    }

    #[test]
    fn test_redis_cache_invalid_url() {
        let res = RedisCache::new("not_a_valid_redis_url");
        assert!(res.is_err());
    }
}
