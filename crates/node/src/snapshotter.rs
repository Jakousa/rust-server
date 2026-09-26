//! Asynchronous, non-blocking write-behind snapshotter for simulation nodes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{watch, Notify};
use tokio::time::sleep;

use mmo_core::guid::Guid;
use mmo_storage::models::EntitySnapshot;
use mmo_storage::repository::CharacterRepository;

/// Default maximum capacity of the dirty snapshot map before new entities are dropped.
pub const DEFAULT_MAX_DIRTY_CAPACITY: usize = 50_000;

/// Default chunk size when writing snapshot batches to the database.
pub const DEFAULT_FLUSH_CHUNK_SIZE: usize = 500;

/// Asynchronous, coalescing write-behind snapshotter.
pub struct WriteBehindSnapshotter {
    dirty_map: Arc<Mutex<HashMap<Guid, EntitySnapshot>>>,
    notify: Arc<Notify>,
    repository: CharacterRepository,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
    max_dirty_capacity: usize,
    chunk_size: usize,
}

impl WriteBehindSnapshotter {
    /// Creates a new `WriteBehindSnapshotter` connected to a `CharacterRepository` with default limits.
    pub fn new(repository: CharacterRepository) -> Self {
        Self::with_limits(
            repository,
            DEFAULT_MAX_DIRTY_CAPACITY,
            DEFAULT_FLUSH_CHUNK_SIZE,
        )
    }

    /// Creates a new `WriteBehindSnapshotter` with explicit dirty queue capacity and chunk size.
    pub fn with_limits(
        repository: CharacterRepository,
        max_dirty_capacity: usize,
        chunk_size: usize,
    ) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        Self {
            dirty_map: Arc::new(Mutex::new(HashMap::new())),
            notify: Arc::new(Notify::new()),
            repository,
            shutdown_tx,
            shutdown_rx,
            max_dirty_capacity,
            chunk_size: chunk_size.max(1),
        }
    }

    /// Marks an entity as dirty by staging its snapshot in the coalescing dirty map.
    ///
    /// This operation only acquires an instantaneous in-memory mutex ($< 1\,\mu\text{s}$)
    /// and immediately signals the background writer without blocking the 20 Hz tick loop.
    /// Returns `true` if staged/coalesced, or `false` if dropped due to dirty queue capacity limits.
    pub fn mark_dirty(&self, guid: Guid, snapshot: EntitySnapshot) -> bool {
        let mut map = self.dirty_map.lock().unwrap();
        if map.len() >= self.max_dirty_capacity && !map.contains_key(&guid) {
            eprintln!(
                "[Snapshotter] Dirty queue capacity exceeded ({}/{}), dropping snapshot for GUID {}",
                map.len(),
                self.max_dirty_capacity,
                guid
            );
            return false;
        }
        map.insert(guid, snapshot);
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

            loop {
                // Wait for either dirty notification, debounce timeout (100ms), or shutdown
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            // Flush any remaining dirty records on shutdown
                            self.flush_batch().await;
                            break;
                        }
                    }
                    _ = self.notify.notified() => {
                        // Debounce briefly to coalesce rapid sequential ticks
                        sleep(Duration::from_millis(50)).await;
                        self.flush_batch().await;
                    }
                }
            }
        })
    }

    /// Drains all currently queued dirty snapshots and batch-saves them to the database in chunks.
    pub async fn flush_batch(&self) -> usize {
        let batch: Vec<EntitySnapshot> = {
            let mut map = self.dirty_map.lock().unwrap();
            map.drain().map(|(_, v)| v).collect()
        };

        if batch.is_empty() {
            return 0;
        }

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
                }
            }
        }
        saved_total
    }

    /// Captures a snapshot of an `Entity` using the `ComponentCodecRegistry` to dynamically serialize attached components.
    pub fn capture_entity_snapshot(
        entity: &mmo_core::Entity,
        codecs: &mmo_core::ComponentCodecRegistry,
        version: u64,
    ) -> EntitySnapshot {
        let attributes = codecs.serialize_all(&entity.components);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        EntitySnapshot {
            guid: entity.guid.0,
            position: entity.position,
            rotation: entity.rotation,
            attributes,
            version,
            timestamp: now,
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
    pub fn restore_entity_components(
        entity: &mut mmo_core::Entity,
        snapshot: &EntitySnapshot,
        codecs: &mmo_core::ComponentCodecRegistry,
    ) -> Result<usize, String> {
        codecs.deserialize_into(&snapshot.attributes, &mut entity.components)
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
                version: 2,
                timestamp: 2000,
            },
        );
        // Map size should still be 1 due to coalescing!
        assert_eq!(snapshotter.dirty_count(), 1);

        // 3. Flush batch
        let flushed = snapshotter.flush_batch().await;
        assert_eq!(flushed, 1);
        assert_eq!(snapshotter.dirty_count(), 0);

        // 4. Verify in DB
        let loaded = repo.get_snapshot(guid.raw()).await.unwrap().unwrap();
        assert_eq!(loaded.position, (5.0, 0.0, 5.0));
        assert_eq!(loaded.version, 2);
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
            version: v,
            timestamp: 1000,
        };

        // Insert first 2
        assert!(snapshotter.mark_dirty(guid1, make_snap(guid1, 1)));
        assert!(snapshotter.mark_dirty(guid2, make_snap(guid2, 1)));
        assert_eq!(snapshotter.dirty_count(), 2);

        // Third entity exceeds capacity and is dropped
        assert!(!snapshotter.mark_dirty(guid3, make_snap(guid3, 1)));
        assert_eq!(snapshotter.dirty_count(), 2);

        // Existing entity (guid1) update should still succeed (coalescing)
        assert!(snapshotter.mark_dirty(guid1, make_snap(guid1, 2)));
        assert_eq!(snapshotter.dirty_count(), 2);
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
}
