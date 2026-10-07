//! Authoritative excavation and deformation validation.

use std::collections::HashMap;

use mmo_core::guid::Guid;

use crate::chunk::PaletteChunk;
use crate::coords::{VoxelChunkCoord, VoxelCoord, CHUNK_SIZE};

/// Domain event emitted when a voxel is modified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoxelChangeEvent {
    pub entity_guid: Guid,
    pub chunk_coord: VoxelChunkCoord,
    pub voxel_coord: VoxelCoord,
    pub local_index: usize,
    pub old_voxel: u16,
    pub new_voxel: u16,
}

impl mmo_core::plugin::DomainEvent for VoxelChangeEvent {}

/// Result of an authoritative voxel modification attempt.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VoxelModifyResult {
    Success {
        old_voxel: u16,
        new_voxel: u16,
    },
    OutOfReach {
        distance: f32,
        max_reach: f32,
    },
    RateLimited {
        elapsed_ms: u64,
        min_interval_ms: u64,
    },
    NoChange,
    ChunkNotLoaded,
}

/// Configuration governing player excavation reach and frequency.
#[derive(Debug, Clone, Copy)]
pub struct ExcavationConfig {
    pub max_reach: f32,
    pub reach_tolerance: f32,
    pub cooldown_ms: u64,
}

impl Default for ExcavationConfig {
    fn default() -> Self {
        Self {
            max_reach: 6.0,
            reach_tolerance: 0.5,
            cooldown_ms: 100,
        }
    }
}

/// Tracks per-entity excavation timestamps to enforce authoritative rate limits.
#[derive(Default)]
pub struct ExcavationRateLimiter {
    last_excavation: HashMap<Guid, u64>,
}

impl ExcavationRateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Checks if the entity is allowed to excavate at `now_ms`.
    /// Updates the recorded timestamp if successful.
    pub fn check_and_update(&mut self, entity: Guid, now_ms: u64, cooldown_ms: u64) -> bool {
        if let Some(&last) = self.last_excavation.get(&entity) {
            if now_ms < last.saturating_add(cooldown_ms) {
                return false;
            }
        }
        self.last_excavation.insert(entity, now_ms);
        true
    }

    pub fn last_time(&self, entity: Guid) -> Option<u64> {
        self.last_excavation.get(&entity).copied()
    }

    pub fn remove_entity(&mut self, entity: Guid) {
        self.last_excavation.remove(&entity);
    }
}

/// Validates player reach, rate limits, and applies the voxel modification.
#[allow(clippy::too_many_arguments)]
pub fn validate_and_apply_modification(
    player_pos: (f32, f32, f32),
    chunk: &mut PaletteChunk,
    idx: usize,
    new_voxel: u16,
    config: &ExcavationConfig,
    rate_limiter: &mut ExcavationRateLimiter,
    entity_guid: Guid,
    _chunk_coord: VoxelChunkCoord,
    voxel_coord: VoxelCoord,
    now_ms: u64,
) -> VoxelModifyResult {
    let target_center = voxel_coord.center_world_pos();
    let dx = player_pos.0 - target_center.0;
    let dy = player_pos.1 - target_center.1;
    let dz = player_pos.2 - target_center.2;
    let dist_sq = dx * dx + dy * dy + dz * dz;

    let allowed_radius = config.max_reach + config.reach_tolerance;
    let allowed_sq = allowed_radius * allowed_radius;

    if dist_sq > allowed_sq {
        return VoxelModifyResult::OutOfReach {
            distance: dist_sq.sqrt(),
            max_reach: config.max_reach,
        };
    }

    if !rate_limiter.check_and_update(entity_guid, now_ms, config.cooldown_ms) {
        let last = rate_limiter.last_time(entity_guid).unwrap_or(0);
        let elapsed = now_ms.saturating_sub(last);
        return VoxelModifyResult::RateLimited {
            elapsed_ms: elapsed,
            min_interval_ms: config.cooldown_ms,
        };
    }

    let old_voxel = chunk.get(idx);
    if chunk.set(idx, new_voxel) {
        VoxelModifyResult::Success {
            old_voxel,
            new_voxel,
        }
    } else {
        VoxelModifyResult::NoChange
    }
}

/// Returns `true` if the local coordinate lies along a chunk boundary.
#[inline]
pub fn is_boundary_voxel(lx: usize, ly: usize, lz: usize) -> bool {
    lx == 0
        || lx + 1 == CHUNK_SIZE
        || ly == 0
        || ly + 1 == CHUNK_SIZE
        || lz == 0
        || lz + 1 == CHUNK_SIZE
}

/// Returns neighbor chunk coordinates affected if a boundary voxel at `(lx, ly, lz)` changes.
pub fn affected_neighbor_chunks(
    chunk: VoxelChunkCoord,
    lx: usize,
    ly: usize,
    lz: usize,
) -> Vec<VoxelChunkCoord> {
    let mut neighbors = Vec::with_capacity(3);
    if lx == 0 {
        neighbors.push(VoxelChunkCoord::new(chunk.x - 1, chunk.y, chunk.z));
    } else if lx + 1 == CHUNK_SIZE {
        neighbors.push(VoxelChunkCoord::new(chunk.x + 1, chunk.y, chunk.z));
    }

    if ly == 0 {
        neighbors.push(VoxelChunkCoord::new(chunk.x, chunk.y - 1, chunk.z));
    } else if ly + 1 == CHUNK_SIZE {
        neighbors.push(VoxelChunkCoord::new(chunk.x, chunk.y + 1, chunk.z));
    }

    if lz == 0 {
        neighbors.push(VoxelChunkCoord::new(chunk.x, chunk.y, chunk.z - 1));
    } else if lz + 1 == CHUNK_SIZE {
        neighbors.push(VoxelChunkCoord::new(chunk.x, chunk.y, chunk.z + 1));
    }

    neighbors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coords::index_from_local;
    use mmo_core::guid::EntityType;

    #[test]
    fn test_within_reach_succeeds() {
        let player_pos = (5.0, 5.0, 5.0);
        let voxel_coord = VoxelCoord::new(5, 5, 5);
        let config = ExcavationConfig::default();
        let mut rate_limiter = ExcavationRateLimiter::new();
        let mut chunk = PaletteChunk::new_homogeneous(1); // solid stone
        let (chunk_coord, lx, ly, lz) = voxel_coord.to_chunk();
        let idx = index_from_local(lx, ly, lz);
        let guid = Guid::new(EntityType::Player, 1, 100);

        let res = validate_and_apply_modification(
            player_pos,
            &mut chunk,
            idx,
            0, // dig to Air
            &config,
            &mut rate_limiter,
            guid,
            chunk_coord,
            voxel_coord,
            1000,
        );

        assert_eq!(
            res,
            VoxelModifyResult::Success {
                old_voxel: 1,
                new_voxel: 0
            }
        );
        assert_eq!(chunk.get(idx), 0);
        assert_eq!(chunk.revision(), 1);
    }

    #[test]
    fn test_out_of_reach_rejected() {
        let player_pos = (0.0, 0.0, 0.0);
        let voxel_coord = VoxelCoord::new(10, 0, 0); // 10.5m away > 6.5m
        let config = ExcavationConfig::default();
        let mut rate_limiter = ExcavationRateLimiter::new();
        let mut chunk = PaletteChunk::new_homogeneous(1);
        let (chunk_coord, lx, ly, lz) = voxel_coord.to_chunk();
        let idx = index_from_local(lx, ly, lz);
        let guid = Guid::new(EntityType::Player, 1, 100);

        let res = validate_and_apply_modification(
            player_pos,
            &mut chunk,
            idx,
            0,
            &config,
            &mut rate_limiter,
            guid,
            chunk_coord,
            voxel_coord,
            1000,
        );

        assert!(matches!(res, VoxelModifyResult::OutOfReach { .. }));
        assert_eq!(chunk.get(idx), 1); // unmodified
    }

    #[test]
    fn test_rate_limiting_enforcement() {
        let player_pos = (0.0, 0.0, 0.0);
        let voxel_coord = VoxelCoord::new(1, 0, 0);
        let config = ExcavationConfig {
            max_reach: 6.0,
            reach_tolerance: 0.5,
            cooldown_ms: 100,
        };
        let mut rate_limiter = ExcavationRateLimiter::new();
        let mut chunk = PaletteChunk::new_homogeneous(1);
        let (chunk_coord, lx, ly, lz) = voxel_coord.to_chunk();
        let idx = index_from_local(lx, ly, lz);
        let guid = Guid::new(EntityType::Player, 1, 100);

        // 1st dig at t=1000 -> OK
        let res1 = validate_and_apply_modification(
            player_pos,
            &mut chunk,
            idx,
            0,
            &config,
            &mut rate_limiter,
            guid,
            chunk_coord,
            voxel_coord,
            1000,
        );
        assert!(matches!(res1, VoxelModifyResult::Success { .. }));

        // 2nd dig at t=1050 (50ms < 100ms) -> RateLimited
        let res2 = validate_and_apply_modification(
            player_pos,
            &mut chunk,
            idx,
            2,
            &config,
            &mut rate_limiter,
            guid,
            chunk_coord,
            voxel_coord,
            1050,
        );
        assert_eq!(
            res2,
            VoxelModifyResult::RateLimited {
                elapsed_ms: 50,
                min_interval_ms: 100
            }
        );

        // 3rd dig at t=1100 (100ms >= 100ms) -> OK
        let res3 = validate_and_apply_modification(
            player_pos,
            &mut chunk,
            idx,
            2,
            &config,
            &mut rate_limiter,
            guid,
            chunk_coord,
            voxel_coord,
            1100,
        );
        assert_eq!(
            res3,
            VoxelModifyResult::Success {
                old_voxel: 0,
                new_voxel: 2
            }
        );
    }

    #[test]
    fn test_boundary_voxel_neighbors() {
        assert!(is_boundary_voxel(0, 5, 5));
        assert!(is_boundary_voxel(15, 5, 5));
        assert!(is_boundary_voxel(5, 0, 5));
        assert!(!is_boundary_voxel(5, 5, 5));

        let c = VoxelChunkCoord::new(10, 20, 30);
        let affected = affected_neighbor_chunks(c, 0, 15, 5);
        assert_eq!(affected.len(), 2);
        assert!(affected.contains(&VoxelChunkCoord::new(9, 20, 30)));
        assert!(affected.contains(&VoxelChunkCoord::new(10, 21, 30)));
    }
}
