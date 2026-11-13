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

const MAX_TTL_SECS: u64 = 2_592_000; // 30 days

#[async_trait]
impl SessionCacheBackend for MemoryCache {
    async fn set_session(&self, data: &SessionData, ttl_secs: u64) -> Result<(), StorageError> {
        let safe_ttl = ttl_secs.min(MAX_TTL_SECS);
        let expire = Instant::now() + Duration::from_secs(safe_ttl);
        let mut map = self.sessions.write().await;
        map.insert(data.session_id, (data.clone(), expire));
        Ok(())
    }

    async fn get_session(&self, session_id: u64) -> Result<Option<SessionData>, StorageError> {
        // Fast path: shared read lock for hits and misses.
        {
            let map = self.sessions.read().await;
            match map.get(&session_id) {
                None => return Ok(None),
                Some((data, expire)) if Instant::now() <= *expire => {
                    return Ok(Some(data.clone()));
                }
                Some(_) => {}
            }
        }

        // Slow path: evict the expired entry, re-verifying under the write lock
        // in case it was refreshed between the two lock acquisitions.
        let mut map = self.sessions.write().await;
        match map.get(&session_id) {
            Some((data, expire)) if Instant::now() <= *expire => Ok(Some(data.clone())),
            Some(_) => {
                map.remove(&session_id);
                Ok(None)
            }
            None => Ok(None),
        }
    }

    async fn remove_session(&self, session_id: u64) -> Result<bool, StorageError> {
        let mut map = self.sessions.write().await;
        Ok(map.remove(&session_id).is_some())
    }

    async fn stage_handover(&self, data: &HandoverData, ttl_secs: u64) -> Result<(), StorageError> {
        let safe_ttl = ttl_secs.min(MAX_TTL_SECS);
        let expire = Instant::now() + Duration::from_secs(safe_ttl);
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
    conn: Arc<RwLock<(u64, Option<redis::aio::MultiplexedConnection>)>>,
}

impl RedisCache {
    pub fn new(redis_url: &str) -> Result<Self, StorageError> {
        let client = redis::Client::open(redis_url)?;
        Ok(Self {
            client,
            conn: Arc::new(RwLock::new((0, None))),
        })
    }

    async fn get_connection(
        &self,
    ) -> Result<(u64, redis::aio::MultiplexedConnection), StorageError> {
        {
            let guard = self.conn.read().await;
            if let Some(ref conn) = guard.1 {
                return Ok((guard.0, conn.clone()));
            }
        }
        let mut guard = self.conn.write().await;
        if let Some(ref conn) = guard.1 {
            return Ok((guard.0, conn.clone()));
        }
        let new_conn = self.client.get_multiplexed_tokio_connection().await?;
        guard.0 = guard.0.wrapping_add(1);
        guard.1 = Some(new_conn.clone());
        Ok((guard.0, new_conn))
    }

    async fn invalidate_connection(&self, failed_gen: u64) {
        let mut guard = self.conn.write().await;
        if guard.0 == failed_gen {
            guard.1 = None;
        }
    }
}

fn is_transient_connection_error(err: &redis::RedisError) -> bool {
    err.is_io_error()
        || err.is_connection_dropped()
        || err.is_unrecoverable_error()
        || err.is_timeout()
        || err.is_connection_refusal()
}

macro_rules! with_redis_conn {
    ($self:expr, $conn:ident => $body:expr) => {{
        let (gen_id, mut $conn) = $self.get_connection().await?;
        match $body {
            Ok(val) => Ok(val),
            Err(err) if is_transient_connection_error(&err) => {
                $self.invalidate_connection(gen_id).await;
                let (_new_gen, mut $conn) = $self.get_connection().await?;
                $body.map_err(StorageError::from)
            }
            Err(err) => Err(StorageError::from(err)),
        }
    }};
}

#[async_trait]
impl SessionCacheBackend for RedisCache {
    async fn set_session(&self, data: &SessionData, ttl_secs: u64) -> Result<(), StorageError> {
        let key = format!("mmo:session:{}", data.session_id);
        let val = serde_json::to_string(data)?;
        with_redis_conn!(self, conn => {
            conn.set_ex::<_, _, ()>(&key, &val, ttl_secs).await
        })
    }

    async fn get_session(&self, session_id: u64) -> Result<Option<SessionData>, StorageError> {
        let key = format!("mmo:session:{}", session_id);
        let val: Option<String> = with_redis_conn!(self, conn => {
            conn.get(&key).await
        })?;
        match val {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    async fn remove_session(&self, session_id: u64) -> Result<bool, StorageError> {
        let key = format!("mmo:session:{}", session_id);
        let deleted: i64 = with_redis_conn!(self, conn => {
            conn.del(&key).await
        })?;
        Ok(deleted > 0)
    }

    async fn stage_handover(&self, data: &HandoverData, ttl_secs: u64) -> Result<(), StorageError> {
        let key = format!("mmo:handover:{}", data.guid);
        let val = serde_json::to_string(data)?;
        with_redis_conn!(self, conn => {
            conn.set_ex::<_, _, ()>(&key, &val, ttl_secs).await
        })
    }

    async fn claim_handover(&self, guid: u64) -> Result<Option<HandoverData>, StorageError> {
        let key = format!("mmo:handover:{}", guid);
        let script = redis::Script::new(
            r#"
            local val = redis.call('GET', KEYS[1])
            if val then
                redis.call('DEL', KEYS[1])
            end
            return val
            "#,
        );
        let val: Option<String> = with_redis_conn!(self, conn => {
            script.key(&key).invoke_async(&mut conn).await
        })?;
        match val {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
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
                state_blob: None,
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
    async fn test_concurrent_handover_claims_mutual_exclusion() {
        let cache = std::sync::Arc::new(MemoryCache::new());

        let handover = HandoverData {
            guid: 777,
            source_node: 1,
            target_node: 2,
            snapshot: EntitySnapshot {
                guid: 777,
                position: (1.0, 2.0, 3.0),
                rotation: 0.0,
                attributes: serde_json::json!({}),
                state_blob: None,
                version: 1,
                timestamp: 1000,
            },
            staged_at: 1000,
        };

        cache.stage_handover(&handover, 30).await.unwrap();

        // Spawn 10 concurrent tasks competing to claim the same handover
        let mut handles = Vec::new();
        for _ in 0..10 {
            let c = cache.clone();
            handles.push(tokio::spawn(
                async move { c.claim_handover(777).await.unwrap() },
            ));
        }

        let mut successes = 0;
        for handle in handles {
            if let Some(claimed) = handle.await.unwrap() {
                assert_eq!(claimed.guid, 777);
                successes += 1;
            }
        }

        // Exactly one task must successfully claim the handover
        assert_eq!(successes, 1);
        assert!(cache.claim_handover(777).await.unwrap().is_none());
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
                state_blob: None,
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

    #[tokio::test]
    async fn test_redis_cache_backend_error_handling() {
        let cache = RedisCache::new("redis://127.0.0.1:1").unwrap();

        let session = SessionData {
            session_id: 123,
            guid: 456,
            node_address: "127.0.0.1:50052".to_string(),
            expires_at: 9999,
        };
        assert!(cache.set_session(&session, 60).await.is_err());
        assert!(cache.get_session(123).await.is_err());
        assert!(cache.remove_session(123).await.is_err());

        let handover = HandoverData {
            guid: 999,
            source_node: 1,
            target_node: 2,
            snapshot: EntitySnapshot {
                guid: 999,
                position: (10.0, 20.0, 30.0),
                rotation: 0.0,
                attributes: serde_json::json!({}),
                state_blob: None,
                version: 1,
                timestamp: 1000,
            },
            staged_at: 1000,
        };
        assert!(cache.stage_handover(&handover, 30).await.is_err());
        assert!(cache.claim_handover(999).await.is_err());
    }

    #[tokio::test]
    async fn test_memory_cache_extreme_ttl_does_not_overflow() {
        let cache = MemoryCache::new();

        let session = SessionData {
            session_id: 111,
            guid: 222,
            node_address: "127.0.0.1:50051".to_string(),
            expires_at: 1000,
        };

        // Extreme TTL u64::MAX must not panic on Instant overflow
        assert!(cache.set_session(&session, u64::MAX).await.is_ok());
        let fetched = cache.get_session(111).await.unwrap();
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().guid, 222);

        let handover = HandoverData {
            guid: 333,
            source_node: 1,
            target_node: 2,
            snapshot: EntitySnapshot {
                guid: 333,
                position: (0.0, 0.0, 0.0),
                rotation: 0.0,
                attributes: serde_json::json!({}),
                state_blob: None,
                version: 1,
                timestamp: 1000,
            },
            staged_at: 1000,
        };
        assert!(cache.stage_handover(&handover, u64::MAX).await.is_ok());
        let claimed = cache.claim_handover(333).await.unwrap();
        assert!(claimed.is_some());
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

    #[tokio::test]
    async fn test_redis_cache_reuses_persistent_connection_and_reconnects_on_drop() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use tokio::io::AsyncWriteExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accept_count = Arc::new(AtomicUsize::new(0));
        let drop_next_set = Arc::new(AtomicBool::new(false));
        let store: Arc<std::sync::Mutex<HashMap<Vec<u8>, Vec<u8>>>> =
            Arc::new(std::sync::Mutex::new(HashMap::new()));

        let accept_count_srv = accept_count.clone();
        let drop_flag_srv = drop_next_set.clone();
        let store_srv = store.clone();
        let server_task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                accept_count_srv.fetch_add(1, Ordering::SeqCst);
                let drop_flag = drop_flag_srv.clone();
                let store = store_srv.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    while let Some(args) = read_resp_command(&mut buf, &mut stream).await {
                        let cmd = String::from_utf8_lossy(&args[0]).to_ascii_uppercase();
                        let reply: Vec<u8> = match cmd.as_str() {
                            "PING" => b"+PONG\r\n".to_vec(),
                            "SET" if args.len() >= 3 => {
                                if drop_flag.swap(false, Ordering::SeqCst) {
                                    // Simulate sudden TCP drop to force client reconnect
                                    return;
                                }
                                store
                                    .lock()
                                    .unwrap()
                                    .insert(args[1].clone(), args[2].clone());
                                b"+OK\r\n".to_vec()
                            }
                            "SETEX" if args.len() >= 4 => {
                                if drop_flag.swap(false, Ordering::SeqCst) {
                                    return;
                                }
                                store
                                    .lock()
                                    .unwrap()
                                    .insert(args[1].clone(), args[3].clone());
                                b"+OK\r\n".to_vec()
                            }
                            "GET" if args.len() >= 2 => {
                                let val = store.lock().unwrap().get(&args[1]).cloned();
                                match val {
                                    Some(v) => {
                                        let mut r = format!("${}\r\n", v.len()).into_bytes();
                                        r.extend_from_slice(&v);
                                        r.extend_from_slice(b"\r\n");
                                        r
                                    }
                                    None => b"$-1\r\n".to_vec(),
                                }
                            }
                            "DEL" if args.len() >= 2 => {
                                let removed = store.lock().unwrap().remove(&args[1]).is_some();
                                format!(":{}\r\n", usize::from(removed)).into_bytes()
                            }
                            "EVALSHA" | "EVAL" if args.len() >= 4 => {
                                let val = store.lock().unwrap().remove(&args[3]);
                                match val {
                                    Some(v) => {
                                        let mut r = format!("${}\r\n", v.len()).into_bytes();
                                        r.extend_from_slice(&v);
                                        r.extend_from_slice(b"\r\n");
                                        r
                                    }
                                    None => b"$-1\r\n".to_vec(),
                                }
                            }
                            _ => b"+OK\r\n".to_vec(),
                        };
                        if stream.write_all(&reply).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });

        let cache = RedisCache::new(&format!("redis://{addr}")).unwrap();
        let session = SessionData {
            session_id: 42,
            guid: 777,
            node_address: "127.0.0.1:50051".to_string(),
            expires_at: 9999,
        };

        // Multiple sequential operations must reuse the exact same single TCP connection.
        cache.set_session(&session, 60).await.unwrap();
        let fetched = cache
            .get_session(42)
            .await
            .unwrap()
            .expect("session cached");
        assert_eq!(fetched.guid, 777);
        assert!(cache.remove_session(42).await.unwrap());
        assert!(cache.get_session(42).await.unwrap().is_none());

        let handover = HandoverData {
            guid: 777,
            source_node: 1,
            target_node: 2,
            snapshot: EntitySnapshot {
                guid: 777,
                position: (1.0, 2.0, 3.0),
                rotation: 0.5,
                attributes: serde_json::json!({}),
                state_blob: None,
                version: 1,
                timestamp: 1000,
            },
            staged_at: 1000,
        };
        cache.stage_handover(&handover, 30).await.unwrap();
        let claimed = cache.claim_handover(777).await.unwrap().expect("handover");
        assert_eq!(claimed.target_node, 2);

        assert_eq!(
            accept_count.load(Ordering::SeqCst),
            1,
            "RedisCache must reuse a single multiplexed TCP connection across operations"
        );

        // Trigger a TCP connection drop on the next SET, verifying automatic reconnect.
        drop_next_set.store(true, Ordering::SeqCst);
        cache.set_session(&session, 60).await.unwrap();
        let fetched_after = cache
            .get_session(42)
            .await
            .unwrap()
            .expect("session after reconnect");
        assert_eq!(fetched_after.guid, 777);
        assert_eq!(
            accept_count.load(Ordering::SeqCst),
            2,
            "RedisCache should reconnect once after connection failure and reuse the new connection"
        );

        server_task.abort();
    }
}
