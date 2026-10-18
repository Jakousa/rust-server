//! Asynchronous write-behind snapshotter for persistent world objects.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{watch, Notify};
use tokio::time::sleep;

use mmo_core::container::ContainerOwner;
use mmo_core::guid::Guid;
use mmo_core::octree::BoundingBox3D;
use mmo_core::world::{ObjectHealth, Transform3D, WorldObject};
use mmo_storage::models::WorldObjectRecord;
use mmo_storage::world_repo::WorldObjectRepository;

/// Converts a domain `WorldObject` into a persistence `WorldObjectRecord`.
pub fn world_object_to_record(world_id: u64, obj: &WorldObject) -> WorldObjectRecord {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    WorldObjectRecord {
        world_id,
        object_id: obj.object_id.0,
        sector_x: obj.sector().x,
        sector_y: obj.sector().y,
        sector_z: obj.sector().z,
        prefab_id: obj.prefab_id,
        pos_x: obj.transform.position.0,
        pos_y: obj.transform.position.1,
        pos_z: obj.transform.position.2,
        rot_x: obj.transform.rotation.0,
        rot_y: obj.transform.rotation.1,
        rot_z: obj.transform.rotation.2,
        rot_w: obj.transform.rotation.3,
        scale_x: obj.transform.scale.0,
        scale_y: obj.transform.scale.1,
        scale_z: obj.transform.scale.2,
        bounds_min_x: obj.bounds.min.0,
        bounds_min_y: obj.bounds.min.1,
        bounds_min_z: obj.bounds.min.2,
        bounds_max_x: obj.bounds.max.0,
        bounds_max_y: obj.bounds.max.1,
        bounds_max_z: obj.bounds.max.2,
        state_flags: obj.state_flags,
        owner_type: obj.owner.owner_type().to_string(),
        owner_id: obj.owner.owner_id(),
        health: obj.health.map(|h| h.current as i32),
        max_health: obj.health.map(|h| h.max as i32),
        properties_json: serde_json::to_string(&obj.properties)
            .unwrap_or_else(|_| "{}".to_string()),
        dirty_revision: obj.dirty_revision,
        updated_at: now,
    }
}

/// Reconstructs a domain `WorldObject` from a persistence `WorldObjectRecord`.
pub fn record_to_world_object(r: &WorldObjectRecord) -> Option<WorldObject> {
    let owner = ContainerOwner::from_parts(&r.owner_type, &r.owner_id).ok()?;
    let properties: HashMap<String, String> =
        serde_json::from_str(&r.properties_json).unwrap_or_default();
    let health = match (r.health, r.max_health) {
        (Some(curr), Some(max)) => Some(ObjectHealth::new(curr.max(0) as u32, max.max(0) as u32)),
        _ => None,
    };

    Some(WorldObject {
        object_id: Guid(r.object_id),
        prefab_id: r.prefab_id,
        transform: Transform3D::new(
            (r.pos_x, r.pos_y, r.pos_z),
            (r.rot_x, r.rot_y, r.rot_z, r.rot_w),
            (r.scale_x, r.scale_y, r.scale_z),
        ),
        bounds: BoundingBox3D::new(
            (r.bounds_min_x, r.bounds_min_y, r.bounds_min_z),
            (r.bounds_max_x, r.bounds_max_y, r.bounds_max_z),
        ),
        state_flags: r.state_flags,
        phase_mask: 1,
        owner,
        health,
        properties,
        dirty_revision: r.dirty_revision,
    })
}

/// Default dirty capacity for world object writes.
pub const DEFAULT_MAX_WORLD_DIRTY_CAPACITY: usize = 20_000;

/// Default batch chunk size for flushing world objects to storage.
pub const DEFAULT_WORLD_FLUSH_CHUNK_SIZE: usize = 250;

/// Non-blocking, coalescing write-behind snapshotter for world objects.
pub struct WorldObjectWriteBehindSnapshotter {
    dirty_map: Arc<Mutex<HashMap<Guid, WorldObjectRecord>>>,
    notify: Arc<Notify>,
    repository: WorldObjectRepository,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
    max_dirty_capacity: usize,
    chunk_size: usize,
}

impl WorldObjectWriteBehindSnapshotter {
    /// Constructs a new snapshotter wrapping the given repository.
    pub fn new(repository: WorldObjectRepository) -> Self {
        Self::with_limits(
            repository,
            DEFAULT_MAX_WORLD_DIRTY_CAPACITY,
            DEFAULT_WORLD_FLUSH_CHUNK_SIZE,
        )
    }

    /// Constructs a new snapshotter with explicit capacity and batch chunk limits.
    pub fn with_limits(
        repository: WorldObjectRepository,
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

    /// Stages a dirty world object record in the write-behind map.
    ///
    /// Holding the mutex only takes a quick hash map insert ($< 1\,\mu\text{s}$)
    /// and immediately notifies the background flusher.
    pub fn mark_dirty(&self, record: WorldObjectRecord) -> bool {
        let guid = Guid(record.object_id);
        let mut map = self.dirty_map.lock().unwrap();
        if map.len() >= self.max_dirty_capacity && !map.contains_key(&guid) {
            eprintln!(
                "[WorldSnapshotter] Dirty queue capacity exceeded ({}/{}), dropping world object {}",
                map.len(),
                self.max_dirty_capacity,
                guid
            );
            return false;
        }

        // Coalesce: only overwrite if new dirty revision is >= current staged revision
        if let Some(existing) = map.get_mut(&guid) {
            if record.dirty_revision >= existing.dirty_revision {
                *existing = record;
            }
        } else {
            map.insert(guid, record);
        }

        metrics::gauge!("mmo_node_world_snapshot_dirty_queue").set(map.len() as f64);
        self.notify.notify_one();
        true
    }

    /// Stages a domain `WorldObject` as dirty for a given world ID.
    pub fn mark_object_dirty(&self, world_id: u64, obj: &WorldObject) -> bool {
        let record = world_object_to_record(world_id, obj);
        self.mark_dirty(record)
    }

    /// Returns the number of currently queued dirty world objects.
    pub fn dirty_count(&self) -> usize {
        self.dirty_map.lock().unwrap().len()
    }

    /// Signals the background worker to stop.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
        self.notify.notify_one();
    }

    /// Spawns the background flush worker task.
    pub fn start_worker(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut shutdown_rx = self.shutdown_rx.clone();

            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            self.flush_batch().await;
                            break;
                        }
                    }
                    _ = self.notify.notified() => {
                        sleep(Duration::from_millis(50)).await;
                        self.flush_batch().await;
                    }
                }
            }
        })
    }

    /// Drains all currently queued dirty world objects using lock-free buffer swapping (`std::mem::take`)
    /// and flushes them to the repository in batches.
    pub async fn flush_batch(&self) -> usize {
        // Fast buffer swap: holding lock for nanoseconds
        let batch: Vec<WorldObjectRecord> = {
            let mut map = self.dirty_map.lock().unwrap();
            let swapped = std::mem::take(&mut *map);
            metrics::gauge!("mmo_node_world_snapshot_dirty_queue").set(0.0);
            swapped.into_values().collect()
        };

        if batch.is_empty() {
            return 0;
        }

        let start = std::time::Instant::now();
        let mut saved_total = 0;
        for chunk in batch.chunks(self.chunk_size) {
            match self.repository.upsert_world_objects_batch(chunk).await {
                Ok(()) => {
                    saved_total += chunk.len();
                }
                Err(e) => {
                    eprintln!(
                        "[WorldSnapshotter] Failed to flush batch of {} world objects: {}",
                        chunk.len(),
                        e
                    );
                    // Pro Review mitigation: Merge failed records back into dirty_map so they are retried on the next flush cycle
                    let mut map = self.dirty_map.lock().unwrap();
                    for record in chunk {
                        let guid = Guid(record.object_id);
                        if let Some(existing) = map.get_mut(&guid) {
                            if record.dirty_revision > existing.dirty_revision {
                                *existing = record.clone();
                            }
                        } else if map.len() < self.max_dirty_capacity {
                            map.insert(guid, record.clone());
                        }
                    }
                    metrics::gauge!("mmo_node_world_snapshot_dirty_queue").set(map.len() as f64);
                }
            }
        }

        let elapsed_sec = start.elapsed().as_secs_f64();
        metrics::histogram!("mmo_node_world_snapshot_flush_duration_seconds").record(elapsed_sec);
        saved_total
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_storage::db::Database;

    fn make_test_object(id: u64, pos: (f32, f32, f32), rev: u64) -> WorldObject {
        let mut obj = WorldObject::new(
            Guid(id),
            1,
            Transform3D::new(pos, (0.0, 0.0, 0.0, 1.0), (1.0, 1.0, 1.0)),
            BoundingBox3D::new((-1.0, -1.0, -1.0), (1.0, 1.0, 1.0)),
            ContainerOwner::Character(100),
        );
        obj.dirty_revision = rev;
        obj
    }

    #[tokio::test]
    async fn test_world_object_record_roundtrip() {
        let obj = make_test_object(42, (10.0, 20.0, 30.0), 3);
        let record = world_object_to_record(1, &obj);
        assert_eq!(record.object_id, 42);
        assert_eq!(record.dirty_revision, 3);

        let reconstructed = record_to_world_object(&record).unwrap();
        assert_eq!(reconstructed.object_id, obj.object_id);
        assert_eq!(reconstructed.dirty_revision, obj.dirty_revision);
        assert_eq!(reconstructed.transform.position, obj.transform.position);
    }

    #[tokio::test]
    async fn test_world_snapshotter_coalescing_and_flush() {
        let db = Database::connect_memory().await.unwrap();
        let repo = WorldObjectRepository::new(db);
        let snapshotter = Arc::new(WorldObjectWriteBehindSnapshotter::new(repo.clone()));

        // Stage revision 1
        let obj1 = make_test_object(101, (5.0, 5.0, 5.0), 1);
        assert!(snapshotter.mark_object_dirty(1, &obj1));
        assert_eq!(snapshotter.dirty_count(), 1);

        // Coalesce with revision 2
        let obj1_v2 = make_test_object(101, (10.0, 5.0, 5.0), 2);
        assert!(snapshotter.mark_object_dirty(1, &obj1_v2));
        assert_eq!(snapshotter.dirty_count(), 1); // Still 1 due to coalescing

        // Add object 102
        let obj2 = make_test_object(102, (20.0, 0.0, 0.0), 1);
        assert!(snapshotter.mark_object_dirty(1, &obj2));
        assert_eq!(snapshotter.dirty_count(), 2);

        // Flush batch
        let flushed = snapshotter.flush_batch().await;
        assert_eq!(flushed, 2);
        assert_eq!(snapshotter.dirty_count(), 0);

        // Verify in DB
        let fetched1 = repo.get_world_object(1, 101).await.unwrap().unwrap();
        assert_eq!(fetched1.dirty_revision, 2);
        assert!((fetched1.pos_x - 10.0).abs() < 1e-4);

        let fetched2 = repo.get_world_object(1, 102).await.unwrap().unwrap();
        assert_eq!(fetched2.dirty_revision, 1);
    }

    #[tokio::test]
    async fn test_world_snapshotter_retry_on_flush_failure() {
        let db = Database::connect_memory().await.unwrap();
        let repo = WorldObjectRepository::new(db.clone());
        let snapshotter = Arc::new(WorldObjectWriteBehindSnapshotter::new(repo.clone()));

        let obj = make_test_object(999, (1.0, 2.0, 3.0), 1);
        snapshotter.mark_object_dirty(1, &obj);
        assert_eq!(snapshotter.dirty_count(), 1);

        // Close the database pool to simulate database blackout / failure
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
}
