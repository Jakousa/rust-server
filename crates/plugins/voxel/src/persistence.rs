//! Asynchronous write-behind persistence and bounded coalescing for modified voxel chunks.

use std::sync::Arc;
use std::time::SystemTime;

use dashmap::DashMap;

use crate::chunk::PaletteChunk;
use crate::codec::encode_rle;
use crate::coords::VoxelChunkCoord;

/// Snapshot of a dirty chunk ready for asynchronous disk persistence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerializedChunkSnapshot {
    pub chunk_coord: VoxelChunkCoord,
    pub revision: u64,
    pub data_blob: Vec<u8>,
    pub updated_at: u64,
}

/// SQL table creation schema compatible with PostgreSQL and SQLite.
pub const VOXEL_CHUNKS_TABLE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS voxel_chunks (
    world_id BIGINT NOT NULL,
    chunk_x INT NOT NULL,
    chunk_y INT NOT NULL,
    chunk_z INT NOT NULL,
    revision BIGINT NOT NULL,
    data_blob BYTEA NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (world_id, chunk_x, chunk_y, chunk_z)
);
"#;

/// Asynchronous write-behind service coalescing dirty chunk mutations to disk.
#[derive(Clone)]
pub struct VoxelWriteBehindService {
    coalescing_map: Arc<DashMap<VoxelChunkCoord, SerializedChunkSnapshot>>,
    max_coalescing_capacity: usize,
}

impl Default for VoxelWriteBehindService {
    fn default() -> Self {
        Self::new(16384)
    }
}

impl VoxelWriteBehindService {
    /// Creates a new `VoxelWriteBehindService` with bounded queue capacity.
    pub fn new(max_capacity: usize) -> Self {
        Self {
            coalescing_map: Arc::new(DashMap::new()),
            max_coalescing_capacity: max_capacity,
        }
    }

    /// Enqueues a chunk snapshot. Rapid successive mutations to the same chunk
    /// overwrite in-place in memory, coalescing into a single future DB write.
    pub fn enqueue(&self, coord: VoxelChunkCoord, chunk: &PaletteChunk) -> bool {
        if self.coalescing_map.len() >= self.max_coalescing_capacity
            && !self.coalescing_map.contains_key(&coord)
        {
            // Bounded capacity protection against OOM
            return false;
        }

        let now_ms = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let data_blob = encode_rle(chunk);
        let snapshot = SerializedChunkSnapshot {
            chunk_coord: coord,
            revision: chunk.revision(),
            data_blob,
            updated_at: now_ms,
        };

        self.coalescing_map.insert(coord, snapshot);
        true
    }

    /// Number of distinct chunks currently pending persistence.
    pub fn pending_count(&self) -> usize {
        self.coalescing_map.len()
    }

    /// Drains all currently queued chunk snapshots for batch persistence.
    pub fn drain_pending(&self) -> Vec<SerializedChunkSnapshot> {
        let mut snapshots = Vec::with_capacity(self.coalescing_map.len());
        let keys: Vec<VoxelChunkCoord> = self.coalescing_map.iter().map(|r| *r.key()).collect();
        for key in keys {
            if let Some((_, snap)) = self.coalescing_map.remove(&key) {
                snapshots.push(snap);
            }
        }
        snapshots
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_behind_coalescing_deduplication() {
        let service = VoxelWriteBehindService::new(100);
        let coord = VoxelChunkCoord::new(1, 2, 3);
        let mut chunk = PaletteChunk::new_empty();

        // Mutation 1
        chunk.set(0, 10);
        assert!(service.enqueue(coord, &chunk));
        assert_eq!(service.pending_count(), 1);

        // Mutation 2 (same chunk, higher revision)
        chunk.set(1, 20);
        assert!(service.enqueue(coord, &chunk));
        // Still exactly 1 entry in queue!
        assert_eq!(service.pending_count(), 1);

        // Drain
        let drained = service.drain_pending();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].revision, 2);
        assert_eq!(service.pending_count(), 0);
    }

    #[test]
    fn test_bounded_capacity_limit() {
        let service = VoxelWriteBehindService::new(2);
        let chunk = PaletteChunk::new_empty();

        assert!(service.enqueue(VoxelChunkCoord::new(1, 0, 0), &chunk));
        assert!(service.enqueue(VoxelChunkCoord::new(2, 0, 0), &chunk));
        // 3rd distinct chunk rejected by bounded capacity
        assert!(!service.enqueue(VoxelChunkCoord::new(3, 0, 0), &chunk));

        // Existing chunk can still be updated
        assert!(service.enqueue(VoxelChunkCoord::new(1, 0, 0), &chunk));
    }
}
