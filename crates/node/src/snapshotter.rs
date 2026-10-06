//! Asynchronous, non-blocking write-behind snapshotter for simulation nodes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, watch, Notify};
use tokio::time::sleep;

use mmo_core::guid::Guid;
use mmo_storage::models::EntitySnapshot;
use mmo_storage::repository::CharacterRepository;

/// Default maximum capacity of the dirty snapshot map before non-critical entities are dropped.
pub const DEFAULT_MAX_DIRTY_CAPACITY: usize = 50_000;

/// Default chunk size when writing snapshot batches to the database.
pub const DEFAULT_FLUSH_CHUNK_SIZE: usize = 500;

/// Default bounded MPSC channel capacity between simulation tick loop and snapshot worker.
pub const DEFAULT_SNAPSHOT_CHANNEL_CAPACITY: usize = 100_000;

/// Default debounce duration to coalesce rapid sequential ticks per entity.
pub const DEFAULT_DEBOUNCE_DURATION: Duration = Duration::from_millis(50);

/// Decoupled snapshot payload sent through the lock-free MPSC channel.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotPayload {
    pub guid: Guid,
    pub position: (f32, f32, f32),
    pub rotation: f32,
    pub attributes: serde_json::Value,
    pub state_blob: Option<Vec<u8>>,
    pub version: u64,
    pub timestamp: u64,
}

impl From<SnapshotPayload> for EntitySnapshot {
    fn from(p: SnapshotPayload) -> Self {
        Self {
            guid: p.guid.raw(),
            position: p.position,
            rotation: p.rotation,
            attributes: p.attributes,
            state_blob: p.state_blob,
            version: p.version,
            timestamp: p.timestamp,
        }
    }
}

/// Asynchronous, coalescing write-behind snapshotter.
pub struct WriteBehindSnapshotter {
    dirty_map: Arc<Mutex<HashMap<Guid, EntitySnapshot>>>,
    notify: Arc<Notify>,
    repository: CharacterRepository,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
    channel_tx: mpsc::Sender<SnapshotPayload>,
    channel_rx: Arc<Mutex<Option<mpsc::Receiver<SnapshotPayload>>>>,
    max_dirty_capacity: usize,
    chunk_size: usize,
    debounce_duration: Duration,
}

impl WriteBehindSnapshotter {
    /// Creates a new `WriteBehindSnapshotter` connected to a `CharacterRepository` with default limits.
    pub fn new(repository: CharacterRepository) -> Self {
        Self::with_full_config(
            repository,
            DEFAULT_MAX_DIRTY_CAPACITY,
            DEFAULT_FLUSH_CHUNK_SIZE,
            DEFAULT_SNAPSHOT_CHANNEL_CAPACITY,
            DEFAULT_DEBOUNCE_DURATION,
        )
    }

    /// Creates a new `WriteBehindSnapshotter` with explicit dirty queue capacity and chunk size.
    pub fn with_limits(
        repository: CharacterRepository,
        max_dirty_capacity: usize,
        chunk_size: usize,
    ) -> Self {
        Self::with_full_config(
            repository,
            max_dirty_capacity,
            chunk_size,
            DEFAULT_SNAPSHOT_CHANNEL_CAPACITY,
            DEFAULT_DEBOUNCE_DURATION,
        )
    }

    /// Creates a new `WriteBehindSnapshotter` with full configuration options.
    pub fn with_full_config(
        repository: CharacterRepository,
        max_dirty_capacity: usize,
        chunk_size: usize,
        channel_capacity: usize,
        debounce_duration: Duration,
    ) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (channel_tx, channel_rx) = mpsc::channel(channel_capacity.max(1));
        Self {
            dirty_map: Arc::new(Mutex::new(HashMap::new())),
            notify: Arc::new(Notify::new()),
            repository,
            shutdown_tx,
            shutdown_rx,
            channel_tx,
            channel_rx: Arc::new(Mutex::new(Some(channel_rx))),
            max_dirty_capacity,
            chunk_size: chunk_size.max(1),
            debounce_duration,
        }
    }

    /// Returns a cloned sender for pushing snapshots into the bounded non-blocking channel.
    pub fn sender(&self) -> mpsc::Sender<SnapshotPayload> {
        self.channel_tx.clone()
    }

    /// Enqueues a snapshot payload into the lock-free bounded MPSC channel.
    ///
    /// This method is non-blocking and executes in nanoseconds during the 20 Hz tick loop.
    /// Returns `true` if enqueued, or `false` if dropped due to channel capacity limits.
    pub fn enqueue_snapshot(&self, payload: SnapshotPayload) -> bool {
        match self.channel_tx.try_send(payload) {
            Ok(()) => {
                self.notify.notify_one();
                true
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                metrics::counter!("mmo_node_snapshot_dropped_channel_full").increment(1);
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }

    /// Marks an entity as dirty by staging its snapshot in the coalescing dirty map.
    ///
    /// This operation acquires an instantaneous in-memory mutex ($< 1\,\mu\text{s}$)
    /// and ensures monotonic version ordering (`snapshot.version >= existing.version`).
    /// Returns `true` if staged/coalesced, or `false` if dropped or superseded.
    pub fn mark_dirty(&self, guid: Guid, snapshot: EntitySnapshot) -> bool {
        let mut map = self.dirty_map.lock().unwrap();
        if let Some(existing) = map.get_mut(&guid) {
            if snapshot.version >= existing.version {
                *existing = snapshot;
                self.notify.notify_one();
                return true;
            }
            return false;
        }

        // Capacity bound check with priority retention for initial entity creations
        if map.len() >= self.max_dirty_capacity {
            if snapshot.version <= 1 {
                map.insert(guid, snapshot);
                metrics::gauge!("mmo_node_snapshot_dirty_queue").set(map.len() as f64);
                self.notify.notify_one();
                return true;
            }
            metrics::counter!("mmo_node_snapshot_dropped_capacity").increment(1);
            eprintln!(
                "[Snapshotter] Dirty queue capacity exceeded ({}/{}), dropping snapshot for GUID {}",
                map.len(),
                self.max_dirty_capacity,
                guid
            );
            return false;
        }

        map.insert(guid, snapshot);
        metrics::gauge!("mmo_node_snapshot_dirty_queue").set(map.len() as f64);
        self.notify.notify_one();
        true
    }

    /// Returns the number of currently queued dirty entities.
    pub fn dirty_count(&self) -> usize {
        self.dirty_map.lock().unwrap().len()
    }

    /// Returns the configured maximum dirty queue capacity.
    pub fn max_dirty_capacity(&self) -> usize {
        self.max_dirty_capacity
    }

    /// Signals the background writer task to stop.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
        self.notify.notify_one();
    }

    /// Spawns the background worker task that drains dirty entities and batch-writes to the DB.
    pub fn start_worker(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut shutdown_rx = self.shutdown_rx.clone();
            let mut rx = self.channel_rx.lock().unwrap().take();
            let debounce = self.debounce_duration;

            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            // Drain any remaining items from the channel before exit
                            if let Some(ref mut channel) = rx {
                                while let Ok(payload) = channel.try_recv() {
                                    let guid = payload.guid;
                                    let snapshot = EntitySnapshot::from(payload);
                                    self.mark_dirty(guid, snapshot);
                                }
                            }
                            self.flush_batch().await;
                            break;
                        }
                    }
                    maybe_payload = async {
                        if let Some(ref mut channel) = rx {
                            channel.recv().await
                        } else {
                            std::future::pending::<Option<SnapshotPayload>>().await
                        }
                    } => {
                        match maybe_payload {
                            Some(payload) => {
                                let guid = payload.guid;
                                let snapshot = EntitySnapshot::from(payload);
                                self.mark_dirty(guid, snapshot);

                                // Drain any immediately available payloads in batch
                                if let Some(ref mut channel) = rx {
                                    while let Ok(p) = channel.try_recv() {
                                        let g = p.guid;
                                        let s = EntitySnapshot::from(p);
                                        self.mark_dirty(g, s);
                                    }
                                }

                                // Debounce briefly to coalesce rapid consecutive ticks
                                sleep(debounce).await;

                                // Drain any items that arrived during debounce
                                if let Some(ref mut channel) = rx {
                                    while let Ok(p) = channel.try_recv() {
                                        let g = p.guid;
                                        let s = EntitySnapshot::from(p);
                                        self.mark_dirty(g, s);
                                    }
                                }

                                self.flush_batch().await;
                            }
                            None => {
                                // Channel closed
                                self.flush_batch().await;
                                break;
                            }
                        }
                    }
                    _ = self.notify.notified() => {
                        // Debounce briefly to coalesce rapid sequential ticks
                        sleep(debounce).await;
                        self.flush_batch().await;
                    }
                }
            }
        })
    }

    /// Drains all currently queued dirty snapshots using $O(1)$ buffer swapping and batch-saves them to the database.
    pub async fn flush_batch(&self) -> usize {
        let batch: Vec<EntitySnapshot> = {
            let mut map = self.dirty_map.lock().unwrap();
            let swapped = std::mem::take(&mut *map);
            metrics::gauge!("mmo_node_snapshot_dirty_queue").set(0.0);
            swapped.into_values().collect()
        };

        if batch.is_empty() {
            return 0;
        }

        let start = std::time::Instant::now();
        let mut saved_total = 0;
        for chunk in batch.chunks(self.chunk_size) {
            match self.repository.save_snapshot_batch(chunk).await {
                Ok(saved) => {
                    saved_total += saved;
                }
                Err(e) => {
                    eprintln!(
                        "[Snapshotter] Failed to save entity snapshot chunk of {}: {}",
                        chunk.len(),
                        e
                    );
                    metrics::counter!("mmo_node_snapshot_flush_errors_total").increment(1);
                    // Pro Review mitigation: Merge failed records back into dirty_map so they are retried on the next flush cycle
                    let mut map = self.dirty_map.lock().unwrap();
                    for snapshot in chunk {
                        let guid = Guid::from_raw(snapshot.guid);
                        if let Some(existing) = map.get_mut(&guid) {
                            if snapshot.version >= existing.version {
                                *existing = snapshot.clone();
                            }
                        } else if map.len() < self.max_dirty_capacity {
                            map.insert(guid, snapshot.clone());
                        }
                    }
                    metrics::gauge!("mmo_node_snapshot_dirty_queue").set(map.len() as f64);
                }
            }
        }
        let elapsed_sec = start.elapsed().as_secs_f64();
        metrics::histogram!("mmo_node_snapshot_flush_duration_seconds").record(elapsed_sec);
        metrics::counter!("mmo_node_snapshot_flushed_total").increment(saved_total as u64);
        saved_total
    }

    /// Captures a snapshot of an `Entity` using the `ComponentCodecRegistry` to dynamically serialize attached components
    /// into both attributes JSON and an opaque MessagePack binary blob.
    pub fn capture_entity_snapshot(
        entity: &mmo_core::Entity,
        codecs: &mmo_core::ComponentCodecRegistry,
        version: u64,
    ) -> EntitySnapshot {
        let attributes = codecs.serialize_all(&entity.components);
        let state_blob = codecs.serialize_all_binary(&entity.components).ok();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        EntitySnapshot {
            guid: entity.guid.0,
            position: entity.position,
            rotation: entity.rotation,
            attributes,
            state_blob,
            version,
            timestamp: now,
        }
    }

    /// Captures a lightweight `SnapshotPayload` for non-blocking MPSC channel transmission.
    pub fn capture_entity_payload(
        entity: &mmo_core::Entity,
        codecs: &mmo_core::ComponentCodecRegistry,
        version: u64,
    ) -> SnapshotPayload {
        let snapshot = Self::capture_entity_snapshot(entity, codecs, version);
        SnapshotPayload {
            guid: entity.guid,
            position: snapshot.position,
            rotation: snapshot.rotation,
            attributes: snapshot.attributes,
            state_blob: snapshot.state_blob,
            version: snapshot.version,
            timestamp: snapshot.timestamp,
        }
    }

    /// Marks an entity as dirty by dynamically extracting its components using the `ComponentCodecRegistry`.
    pub fn mark_entity_dirty(
        &self,
        entity: &mmo_core::Entity,
        codecs: &mmo_core::ComponentCodecRegistry,
        version: u64,
    ) {
        let snapshot = Self::capture_entity_snapshot(entity, codecs, version);
        self.mark_dirty(entity.guid, snapshot);
    }

    /// Restores dynamic components from an `EntitySnapshot` into an `Entity`.
    /// Prefers the opaque binary `state_blob` if present, falling back to JSON `attributes`.
    pub fn restore_entity_components(
        entity: &mut mmo_core::Entity,
        snapshot: &EntitySnapshot,
        codecs: &mmo_core::ComponentCodecRegistry,
    ) -> Result<usize, mmo_core::CodecError> {
        if let Some(ref blob) = snapshot.state_blob {
            codecs.deserialize_binary_into(blob, &mut entity.components)
        } else {
            codecs.deserialize_into(&snapshot.attributes, &mut entity.components)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::guid::EntityType;
    use mmo_storage::Database;

    #[tokio::test]
    async fn test_write_behind_snapshotter_coalescing_and_flush() {
        let db = Database::connect_memory().await.unwrap();
        let repo = CharacterRepository::new(db);
        let snapshotter = Arc::new(WriteBehindSnapshotter::new(repo.clone()));

        let guid = Guid::new(EntityType::Player, 1, 100);

        // 1. Mark dirty version 1
        snapshotter.mark_dirty(
            guid,
            EntitySnapshot {
                guid: guid.raw(),
                position: (1.0, 0.0, 1.0),
                rotation: 0.0,
                attributes: serde_json::json!({"hp": 100}),
                state_blob: Some(vec![1, 2, 3]),
                version: 1,
                timestamp: 1000,
            },
        );
        assert_eq!(snapshotter.dirty_count(), 1);

        // 2. Coalescing: mark dirty version 2 before worker flushes
        snapshotter.mark_dirty(
            guid,
            EntitySnapshot {
                guid: guid.raw(),
                position: (5.0, 0.0, 5.0),
                rotation: 1.0,
                attributes: serde_json::json!({"hp": 90}),
                state_blob: Some(vec![4, 5, 6]),
                version: 2,
                timestamp: 2000,
            },
        );
        // Map size should still be 1 due to coalescing!
        assert_eq!(snapshotter.dirty_count(), 1);

        // Stale version (1 <= 2) should not overwrite version 2
        assert!(!snapshotter.mark_dirty(
            guid,
            EntitySnapshot {
                guid: guid.raw(),
                position: (0.0, 0.0, 0.0),
                rotation: 0.0,
                attributes: serde_json::json!({"hp": 10}),
                state_blob: None,
                version: 1,
                timestamp: 500,
            },
        ));

        // 3. Flush batch
        let flushed = snapshotter.flush_batch().await;
        assert_eq!(flushed, 1);
        assert_eq!(snapshotter.dirty_count(), 0);

        // 4. Verify in DB
        let loaded = repo.get_snapshot(guid.raw()).await.unwrap().unwrap();
        assert_eq!(loaded.position, (5.0, 0.0, 5.0));
        assert_eq!(loaded.version, 2);
        assert_eq!(loaded.state_blob, Some(vec![4, 5, 6]));
    }

    #[tokio::test]
    async fn test_write_behind_snapshotter_bounded_capacity_and_coalescing() {
        let db = Database::connect_memory().await.unwrap();
        let repo = CharacterRepository::new(db);
        // Capacity = 2
        let snapshotter = WriteBehindSnapshotter::with_limits(repo, 2, 500);

        let guid1 = Guid::new(EntityType::Player, 1, 101);
        let guid2 = Guid::new(EntityType::Player, 1, 102);
        let guid3 = Guid::new(EntityType::Player, 1, 103);

        let make_snap = |guid: Guid, v: u64| EntitySnapshot {
            guid: guid.raw(),
            position: (0.0, 0.0, 0.0),
            rotation: 0.0,
            attributes: serde_json::json!({}),
            state_blob: None,
            version: v,
            timestamp: 1000,
        };

        // Insert first 2
        assert!(snapshotter.mark_dirty(guid1, make_snap(guid1, 1)));
        assert!(snapshotter.mark_dirty(guid2, make_snap(guid2, 1)));
        assert_eq!(snapshotter.dirty_count(), 2);

        // Existing entity (guid1) update should still succeed (coalescing)
        assert!(snapshotter.mark_dirty(guid1, make_snap(guid1, 2)));
        assert_eq!(snapshotter.dirty_count(), 2);

        // Third entity with version > 1 exceeds capacity and is dropped
        assert!(!snapshotter.mark_dirty(guid3, make_snap(guid3, 2)));
        assert_eq!(snapshotter.dirty_count(), 2);
    }

    #[tokio::test]
    async fn test_write_behind_snapshotter_mpsc_channel_pipeline() {
        let db = Database::connect_memory().await.unwrap();
        let repo = CharacterRepository::new(db.clone());
        let snapshotter = Arc::new(WriteBehindSnapshotter::with_full_config(
            repo.clone(),
            100,
            10,
            50,
            Duration::from_millis(20),
        ));

        let guid = Guid::new(EntityType::Player, 1, 201);
        let payload = SnapshotPayload {
            guid,
            position: (10.0, 20.0, 30.0),
            rotation: 1.57,
            attributes: serde_json::json!({"level": 42}),
            state_blob: Some(vec![10, 20, 30]),
            version: 1,
            timestamp: 1000,
        };

        // Enqueue via non-blocking channel
        assert!(snapshotter.enqueue_snapshot(payload));

        let handle = snapshotter.clone().start_worker();

        // Allow worker to debounce and flush
        sleep(Duration::from_millis(80)).await;

        let loaded = repo.get_snapshot(guid.raw()).await.unwrap().unwrap();
        assert_eq!(loaded.position, (10.0, 20.0, 30.0));
        assert_eq!(loaded.state_blob, Some(vec![10, 20, 30]));

        snapshotter.shutdown();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_write_behind_snapshotter_chunked_flush() {
        let db = Database::connect_memory().await.unwrap();
        let repo = CharacterRepository::new(db);
        // Capacity = 10, Chunk size = 2
        let snapshotter = WriteBehindSnapshotter::with_limits(repo.clone(), 10, 2);

        for id in 1..=5 {
            let guid = Guid::new(EntityType::Player, 1, id);
            snapshotter.mark_dirty(
                guid,
                EntitySnapshot {
                    guid: guid.raw(),
                    position: (id as f32, 0.0, 0.0),
                    rotation: 0.0,
                    attributes: serde_json::json!({}),
                    state_blob: None,
                    version: 1,
                    timestamp: 1000,
                },
            );
        }

        assert_eq!(snapshotter.dirty_count(), 5);
        let flushed = snapshotter.flush_batch().await;
        assert_eq!(flushed, 5);
        assert_eq!(snapshotter.dirty_count(), 0);

        // Verify all 5 were written to database across the chunks
        for id in 1..=5 {
            let guid = Guid::new(EntityType::Player, 1, id);
            let snap = repo.get_snapshot(guid.raw()).await.unwrap();
            assert!(snap.is_some());
        }
    }

    #[tokio::test]
    async fn test_snapshotter_telemetry_metrics() {
        let server = mmo_core::TelemetryServer::init().unwrap();
        let db = Database::connect_memory().await.unwrap();
        let repo = CharacterRepository::new(db);
        let snapshotter = WriteBehindSnapshotter::with_limits(repo, 10, 5);

        let guid = Guid::new(EntityType::Player, 1, 42);
        snapshotter.mark_dirty(
            guid,
            EntitySnapshot {
                guid: guid.raw(),
                position: (1.0, 2.0, 3.0),
                rotation: 0.0,
                attributes: serde_json::json!({}),
                state_blob: None,
                version: 1,
                timestamp: 1000,
            },
        );

        let render_before = server.render_metrics();
        assert!(render_before.contains("mmo_node_snapshot_dirty_queue"));

        snapshotter.flush_batch().await;
        let render_after = server.render_metrics();
        assert!(render_after.contains("mmo_node_snapshot_flush_duration_seconds"));
    }

    #[tokio::test]
    async fn test_write_behind_snapshotter_retry_on_db_failure() {
        let db = Database::connect_memory().await.unwrap();
        let repo = CharacterRepository::new(db.clone());
        let snapshotter = WriteBehindSnapshotter::new(repo);

        let guid = Guid::new(EntityType::Player, 1, 999);
        let snap = EntitySnapshot {
            guid: guid.raw(),
            position: (1.0, 2.0, 3.0),
            rotation: 0.0,
            attributes: serde_json::json!({}),
            state_blob: Some(vec![1, 2, 3]),
            version: 1,
            timestamp: 1000,
        };

        snapshotter.mark_dirty(guid, snap);
        assert_eq!(snapshotter.dirty_count(), 1);

        // Close the database pool to simulate database outage
        db.sqlite_pool().unwrap().close().await;

        // Flush batch should fail and merge failed records back into dirty_map
        let saved = snapshotter.flush_batch().await;
        assert_eq!(saved, 0);
        assert_eq!(
            snapshotter.dirty_count(),
            1,
            "Failed records must be retained in dirty_map for retry"
        );
    }

    #[tokio::test]
    async fn test_write_behind_snapshotter_channel_drop_when_full() {
        let db = Database::connect_memory().await.unwrap();
        let repo = CharacterRepository::new(db);
        // Bounded channel with capacity = 2
        let snapshotter =
            WriteBehindSnapshotter::with_full_config(repo, 100, 10, 2, Duration::from_millis(50));

        let make_payload = |id: u32| SnapshotPayload {
            guid: Guid::new(EntityType::Player, 1, id),
            position: (0.0, 0.0, 0.0),
            rotation: 0.0,
            attributes: serde_json::json!({}),
            state_blob: None,
            version: 1,
            timestamp: 1000,
        };

        assert!(snapshotter.enqueue_snapshot(make_payload(1)));
        assert!(snapshotter.enqueue_snapshot(make_payload(2)));
        // 3rd payload exceeds channel capacity (2) and is dropped non-blockingly
        assert!(!snapshotter.enqueue_snapshot(make_payload(3)));
    }

    #[tokio::test]
    async fn test_tick_simulation_periodic_snapshot_dispatch() {
        let bounds = mmo_core::BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
        let mut sim = crate::sim::TickSimulation::new(1, bounds);

        let (tx, mut rx) = mpsc::channel(10);
        sim.set_snapshot_sender(tx);
        sim.set_snapshot_interval_ticks(2);

        let guid = Guid::new(EntityType::Player, 1, 555);
        assert!(sim.spawn_entity(guid, (5.0, 10.0, 15.0)));

        // Tick 1: interval not reached (1 % 2 != 0)
        sim.step_tick();
        assert!(rx.try_recv().is_err());

        // Tick 2: interval reached (2 % 2 == 0)
        sim.step_tick();
        let payload = rx
            .try_recv()
            .expect("Payload should be dispatched on tick 2");
        assert_eq!(payload.guid, guid);
        assert_eq!(payload.position, (5.0, 10.0, 15.0));
        assert_eq!(payload.version, 2);
    }
}
