//! Concurrent voxel world managing active chunks and authoritative modifications.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use dashmap::{DashMap, DashSet};
use mmo_core::guid::Guid;

use crate::chunk::PaletteChunk;
use crate::coords::{index_from_local, VoxelChunkCoord, VoxelCoord};
use crate::mesh::{
    extract_slice_x, extract_slice_y, extract_slice_z, generate_chunk_mesh, NeighborBoundarySlices,
    VoxelMesh,
};
use crate::modify::{
    affected_neighbor_chunks, is_boundary_voxel, validate_and_apply_modification, ExcavationConfig,
    ExcavationRateLimiter, VoxelChangeEvent, VoxelModifyResult,
};

/// High-concurrency spatial container managing voxel chunks.
#[derive(Clone)]
pub struct VoxelWorld {
    chunks: Arc<DashMap<VoxelChunkCoord, Arc<RwLock<PaletteChunk>>>>,
    dirty_chunks: Arc<DashSet<VoxelChunkCoord>>,
    rate_limiter: Arc<Mutex<ExcavationRateLimiter>>,
    config: ExcavationConfig,
    global_revision: Arc<AtomicU64>,
}

impl Default for VoxelWorld {
    fn default() -> Self {
        Self::new(ExcavationConfig::default())
    }
}

impl VoxelWorld {
    /// Creates a new `VoxelWorld` with the given excavation configuration.
    pub fn new(config: ExcavationConfig) -> Self {
        Self {
            chunks: Arc::new(DashMap::new()),
            dirty_chunks: Arc::new(DashSet::new()),
            rate_limiter: Arc::new(Mutex::new(ExcavationRateLimiter::new())),
            config,
            global_revision: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Configuration parameters for player excavation.
    pub fn config(&self) -> &ExcavationConfig {
        &self.config
    }

    /// Inserts or replaces a chunk at the given chunk coordinate.
    pub fn insert_chunk(&self, coord: VoxelChunkCoord, chunk: PaletteChunk) {
        let chunk_arc = Arc::new(RwLock::new(chunk));
        self.chunks.insert(coord, chunk_arc);
        self.dirty_chunks.insert(coord);

        // When a new chunk loads, flag adjacent loaded neighbor chunks dirty so their
        // border quads are properly exposed or culled.
        for neighbor in coord.cardinal_neighbors() {
            if self.chunks.contains_key(&neighbor) {
                self.dirty_chunks.insert(neighbor);
            }
        }

        self.global_revision.fetch_add(1, Ordering::Relaxed);
    }

    /// Retrieves an Arc reference to the chunk at `coord` if loaded.
    pub fn get_chunk(&self, coord: VoxelChunkCoord) -> Option<Arc<RwLock<PaletteChunk>>> {
        self.chunks.get(&coord).map(|r| r.value().clone())
    }

    /// Removes and unloads a chunk.
    pub fn remove_chunk(&self, coord: VoxelChunkCoord) -> Option<PaletteChunk> {
        self.dirty_chunks.remove(&coord);
        let (_, chunk_arc) = self.chunks.remove(&coord)?;
        let chunk = match Arc::try_unwrap(chunk_arc) {
            Ok(rw) => rw.into_inner().unwrap_or_else(|e| e.into_inner()),
            Err(shared) => shared.read().unwrap().clone(),
        };

        // Flag adjacent neighbors dirty to regenerate boundaries
        for neighbor in coord.cardinal_neighbors() {
            if self.chunks.contains_key(&neighbor) {
                self.dirty_chunks.insert(neighbor);
            }
        }

        Some(chunk)
    }

    /// Total number of currently loaded chunks.
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Returns `true` if the chunk is currently loaded.
    pub fn is_chunk_loaded(&self, coord: VoxelChunkCoord) -> bool {
        self.chunks.contains_key(&coord)
    }

    /// Authoritatively modifies a voxel block.
    pub fn modify_voxel(
        &self,
        entity: Guid,
        target: VoxelCoord,
        new_voxel: u16,
        player_pos: (f32, f32, f32),
        now_ms: u64,
    ) -> (VoxelModifyResult, Option<VoxelChangeEvent>) {
        let (chunk_coord, lx, ly, lz) = target.to_chunk();
        let chunk_arc = match self.get_chunk(chunk_coord) {
            Some(c) => c,
            None => return (VoxelModifyResult::ChunkNotLoaded, None),
        };

        let idx = index_from_local(lx, ly, lz);
        let mut rate_limiter = self.rate_limiter.lock().unwrap();
        let mut chunk = match chunk_arc.write() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };

        let result = validate_and_apply_modification(
            player_pos,
            &mut chunk,
            idx,
            new_voxel,
            &self.config,
            &mut rate_limiter,
            entity,
            chunk_coord,
            target,
            now_ms,
        );

        match result {
            VoxelModifyResult::Success {
                old_voxel,
                new_voxel,
            } => {
                self.dirty_chunks.insert(chunk_coord);
                if is_boundary_voxel(lx, ly, lz) {
                    for neighbor in affected_neighbor_chunks(chunk_coord, lx, ly, lz) {
                        if self.chunks.contains_key(&neighbor) {
                            self.dirty_chunks.insert(neighbor);
                        }
                    }
                }
                self.global_revision.fetch_add(1, Ordering::Relaxed);

                (
                    result,
                    Some(VoxelChangeEvent {
                        entity_guid: entity,
                        chunk_coord,
                        voxel_coord: target,
                        local_index: idx,
                        old_voxel,
                        new_voxel,
                    }),
                )
            }
            _ => (result, None),
        }
    }

    /// Extracts isolated boundary slices from adjacent neighbor chunks.
    ///
    /// Locks are acquired and dropped immediately per neighbor to prevent lock-inversion deadlocks.
    pub fn extract_neighbor_slices(&self, center: VoxelChunkCoord) -> NeighborBoundarySlices {
        let mut slices = NeighborBoundarySlices::default();

        // +X neighbor (x + 1): sample its x = 0 slice
        if let Some(c) = self.get_chunk(VoxelChunkCoord::new(center.x + 1, center.y, center.z)) {
            let chunk = c.read().unwrap();
            slices.pos_x = Some(extract_slice_x(&chunk, 0));
        }

        // -X neighbor (x - 1): sample its x = 15 slice
        if let Some(c) = self.get_chunk(VoxelChunkCoord::new(center.x - 1, center.y, center.z)) {
            let chunk = c.read().unwrap();
            slices.neg_x = Some(extract_slice_x(&chunk, 15));
        }

        // +Y neighbor (y + 1): sample its y = 0 slice
        if let Some(c) = self.get_chunk(VoxelChunkCoord::new(center.x, center.y + 1, center.z)) {
            let chunk = c.read().unwrap();
            slices.pos_y = Some(extract_slice_y(&chunk, 0));
        }

        // -Y neighbor (y - 1): sample its y = 15 slice
        if let Some(c) = self.get_chunk(VoxelChunkCoord::new(center.x, center.y - 1, center.z)) {
            let chunk = c.read().unwrap();
            slices.neg_y = Some(extract_slice_y(&chunk, 15));
        }

        // +Z neighbor (z + 1): sample its z = 0 slice
        if let Some(c) = self.get_chunk(VoxelChunkCoord::new(center.x, center.y, center.z + 1)) {
            let chunk = c.read().unwrap();
            slices.pos_z = Some(extract_slice_z(&chunk, 0));
        }

        // -Z neighbor (z - 1): sample its z = 15 slice
        if let Some(c) = self.get_chunk(VoxelChunkCoord::new(center.x, center.y, center.z - 1)) {
            let chunk = c.read().unwrap();
            slices.neg_z = Some(extract_slice_z(&chunk, 15));
        }

        slices
    }

    /// Generates a culled boundary mesh for the specified chunk.
    pub fn build_mesh(&self, coord: VoxelChunkCoord) -> Option<VoxelMesh> {
        let chunk_arc = self.get_chunk(coord)?;
        let chunk_snapshot = {
            let lock = chunk_arc.read().unwrap();
            lock.clone()
        };

        let neighbors = self.extract_neighbor_slices(coord);
        Some(generate_chunk_mesh(&chunk_snapshot, &neighbors))
    }

    /// Drains all currently queued dirty chunk coordinates for batch processing.
    pub fn drain_dirty_chunks(&self) -> Vec<VoxelChunkCoord> {
        let mut coords = Vec::new();
        let keys: Vec<VoxelChunkCoord> = self.dirty_chunks.iter().map(|r| *r).collect();
        for key in keys {
            if self.dirty_chunks.remove(&key).is_some() {
                coords.push(key);
            }
        }
        coords
    }

    /// Monotonically increasing world mutation revision.
    pub fn global_revision(&self) -> u64 {
        self.global_revision.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::guid::EntityType;

    #[test]
    fn test_voxel_world_insert_and_modify() {
        let world = VoxelWorld::default();
        let coord = VoxelChunkCoord::new(0, 0, 0);
        let chunk = PaletteChunk::new_homogeneous(1); // solid stone
        world.insert_chunk(coord, chunk);
        assert_eq!(world.chunk_count(), 1);

        let entity = Guid::new(EntityType::Player, 1, 42);
        let target = VoxelCoord::new(5, 5, 5);
        let player_pos = (5.0, 5.0, 5.0);

        let (res, event) = world.modify_voxel(entity, target, 0, player_pos, 1000);
        assert!(matches!(res, VoxelModifyResult::Success { .. }));
        assert!(event.is_some());
        let ev = event.unwrap();
        assert_eq!(ev.old_voxel, 1);
        assert_eq!(ev.new_voxel, 0);

        let dirty = world.drain_dirty_chunks();
        assert!(dirty.contains(&coord));
    }

    #[test]
    fn test_voxel_world_unloaded_chunk() {
        let world = VoxelWorld::default();
        let entity = Guid::new(EntityType::Player, 1, 42);
        let target = VoxelCoord::new(100, 100, 100);

        let (res, event) = world.modify_voxel(entity, target, 0, (100.0, 100.0, 100.0), 1000);
        assert_eq!(res, VoxelModifyResult::ChunkNotLoaded);
        assert!(event.is_none());
    }

    #[test]
    fn test_voxel_world_mesh_extraction() {
        let world = VoxelWorld::default();
        let coord = VoxelChunkCoord::new(0, 0, 0);
        let mut chunk = PaletteChunk::new_empty();
        chunk.set(index_from_local(7, 7, 7), 1);
        world.insert_chunk(coord, chunk);

        let mesh = world.build_mesh(coord).expect("mesh");
        assert_eq!(mesh.vertex_count(), 36);
    }
}
