//! Discrete spatial coordinates and sector mapping for voxel chunks.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Number of voxels along each axis of a chunk.
pub const CHUNK_SIZE: usize = 16;
/// Total number of voxels per chunk (16x16x16 = 4096).
pub const CHUNK_VOXELS: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE;
/// Uniform dimension of a single voxel in meters.
pub const VOXEL_SCALE: f32 = 1.0;
/// Number of 16m chunks along each axis of a 64m sector (64 / 16 = 4).
pub const CHUNKS_PER_SECTOR_AXIS: i32 = 4;

/// Discrete 3D voxel coordinate in world space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VoxelCoord {
    pub x: i64,
    pub y: i64,
    pub z: i64,
}

impl VoxelCoord {
    pub const fn new(x: i64, y: i64, z: i64) -> Self {
        Self { x, y, z }
    }

    /// Computes the voxel coordinate enclosing the given floating-point world position.
    pub fn from_world_pos(pos: (f32, f32, f32)) -> Self {
        Self {
            x: (pos.0 / VOXEL_SCALE).floor() as i64,
            y: (pos.1 / VOXEL_SCALE).floor() as i64,
            z: (pos.2 / VOXEL_SCALE).floor() as i64,
        }
    }

    /// Center position of this voxel in world space.
    pub fn center_world_pos(&self) -> (f32, f32, f32) {
        (
            (self.x as f32 + 0.5) * VOXEL_SCALE,
            (self.y as f32 + 0.5) * VOXEL_SCALE,
            (self.z as f32 + 0.5) * VOXEL_SCALE,
        )
    }

    /// Converts this world voxel coordinate into enclosing chunk coordinate and local coordinates `[0, 15]`.
    /// Guaranteed positive local indices across negative world coordinates via Euclidean math.
    pub fn to_chunk(&self) -> (VoxelChunkCoord, usize, usize, usize) {
        let cx = self.x.div_euclid(CHUNK_SIZE as i64) as i32;
        let cy = self.y.div_euclid(CHUNK_SIZE as i64) as i32;
        let cz = self.z.div_euclid(CHUNK_SIZE as i64) as i32;
        let lx = self.x.rem_euclid(CHUNK_SIZE as i64) as usize;
        let ly = self.y.rem_euclid(CHUNK_SIZE as i64) as usize;
        let lz = self.z.rem_euclid(CHUNK_SIZE as i64) as usize;
        (VoxelChunkCoord::new(cx, cy, cz), lx, ly, lz)
    }

    /// Converts this world voxel coordinate into enclosing chunk coordinate and flat local index `[0, 4095]`.
    pub fn to_chunk_and_index(&self) -> (VoxelChunkCoord, usize) {
        let (chunk, lx, ly, lz) = self.to_chunk();
        (chunk, index_from_local(lx, ly, lz))
    }
}

impl fmt::Display for VoxelCoord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "VoxelCoord({}, {}, {})", self.x, self.y, self.z)
    }
}

/// Discrete 3D chunk coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct VoxelChunkCoord {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl VoxelChunkCoord {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// Computes the chunk coordinate enclosing the given floating-point world position.
    pub fn from_world_pos(pos: (f32, f32, f32)) -> Self {
        Self {
            x: (pos.0 / (CHUNK_SIZE as f32 * VOXEL_SCALE)).floor() as i32,
            y: (pos.1 / (CHUNK_SIZE as f32 * VOXEL_SCALE)).floor() as i32,
            z: (pos.2 / (CHUNK_SIZE as f32 * VOXEL_SCALE)).floor() as i32,
        }
    }

    /// Minimum corner world position of this chunk.
    pub fn min_world_pos(&self) -> (f32, f32, f32) {
        (
            self.x as f32 * CHUNK_SIZE as f32 * VOXEL_SCALE,
            self.y as f32 * CHUNK_SIZE as f32 * VOXEL_SCALE,
            self.z as f32 * CHUNK_SIZE as f32 * VOXEL_SCALE,
        )
    }

    /// Converts this chunk coordinate into the enclosing 64m `SectorCoord3D`.
    pub fn to_sector(&self) -> mmo_core::world::SectorCoord3D {
        mmo_core::world::SectorCoord3D::new(
            self.x.div_euclid(CHUNKS_PER_SECTOR_AXIS),
            self.y.div_euclid(CHUNKS_PER_SECTOR_AXIS),
            self.z.div_euclid(CHUNKS_PER_SECTOR_AXIS),
        )
    }

    /// Converts a local coordinate within this chunk back to world `VoxelCoord`.
    pub fn voxel_coord(&self, lx: usize, ly: usize, lz: usize) -> VoxelCoord {
        VoxelCoord {
            x: self.x as i64 * CHUNK_SIZE as i64 + lx as i64,
            y: self.y as i64 * CHUNK_SIZE as i64 + ly as i64,
            z: self.z as i64 * CHUNK_SIZE as i64 + lz as i64,
        }
    }

    /// Returns the 26 immediate neighbor chunks (excluding self).
    pub fn neighborhood(&self) -> Vec<VoxelChunkCoord> {
        let mut neighbors = Vec::with_capacity(26);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if dx == 0 && dy == 0 && dz == 0 {
                        continue;
                    }
                    neighbors.push(VoxelChunkCoord::new(self.x + dx, self.y + dy, self.z + dz));
                }
            }
        }
        neighbors
    }

    /// Returns the 6 direct cardinal neighbor chunks (along X, Y, Z faces).
    pub fn cardinal_neighbors(&self) -> [VoxelChunkCoord; 6] {
        [
            VoxelChunkCoord::new(self.x + 1, self.y, self.z),
            VoxelChunkCoord::new(self.x - 1, self.y, self.z),
            VoxelChunkCoord::new(self.x, self.y + 1, self.z),
            VoxelChunkCoord::new(self.x, self.y - 1, self.z),
            VoxelChunkCoord::new(self.x, self.y, self.z + 1),
            VoxelChunkCoord::new(self.x, self.y, self.z - 1),
        ]
    }
}

impl fmt::Display for VoxelChunkCoord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "VoxelChunkCoord({}, {}, {})", self.x, self.y, self.z)
    }
}

/// Converts local chunk coordinates `(lx, ly, lz)` into a flat array index `0..4095`.
#[inline(always)]
pub const fn index_from_local(lx: usize, ly: usize, lz: usize) -> usize {
    lx + ly * CHUNK_SIZE + lz * CHUNK_SIZE * CHUNK_SIZE
}

/// Converts flat array index `0..4095` back into local chunk coordinates `(lx, ly, lz)`.
#[inline(always)]
pub const fn local_from_index(idx: usize) -> (usize, usize, usize) {
    (
        idx % CHUNK_SIZE,
        (idx / CHUNK_SIZE) % CHUNK_SIZE,
        idx / (CHUNK_SIZE * CHUNK_SIZE),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_voxel_coord_roundtrip() {
        let pos = (-17.0, -5.0, -65.0);
        let coord = VoxelCoord::from_world_pos(pos);
        assert_eq!(coord, VoxelCoord::new(-17, -5, -65));
        assert_eq!(coord.center_world_pos(), (-16.5, -4.5, -64.5));

        let (chunk, lx, ly, lz) = coord.to_chunk();
        assert_eq!(chunk, VoxelChunkCoord::new(-2, -1, -5));
        assert_eq!(lx, 15);
        assert_eq!(ly, 11);
        assert_eq!(lz, 15);

        let reconstructed = chunk.voxel_coord(lx, ly, lz);
        assert_eq!(reconstructed, coord);
    }

    #[test]
    fn test_chunk_coord_roundtrip() {
        let pos = (-17.0, -5.0, -65.0);
        let coord = VoxelChunkCoord::from_world_pos(pos);
        assert_eq!(coord, VoxelChunkCoord::new(-2, -1, -5));
        assert_eq!(coord.min_world_pos(), (-32.0, -16.0, -80.0));
    }

    #[test]
    fn test_index_roundtrip() {
        for idx in 0..CHUNK_VOXELS {
            let (lx, ly, lz) = local_from_index(idx);
            assert!(lx < CHUNK_SIZE && ly < CHUNK_SIZE && lz < CHUNK_SIZE);
            assert_eq!(index_from_local(lx, ly, lz), idx);
        }
    }

    #[test]
    fn test_sector_conversion() {
        let chunk = VoxelChunkCoord::new(4, 4, 4);
        let sector = chunk.to_sector();
        assert_eq!(sector, mmo_core::world::SectorCoord3D::new(1, 1, 1));

        let neg_chunk = VoxelChunkCoord::new(-1, -4, -5);
        let neg_sector = neg_chunk.to_sector();
        assert_eq!(neg_sector, mmo_core::world::SectorCoord3D::new(-1, -1, -2));
    }

    #[test]
    fn test_neighborhood() {
        let chunk = VoxelChunkCoord::new(0, 0, 0);
        let neighborhood = chunk.neighborhood();
        assert_eq!(neighborhood.len(), 26);
        for neighbor in neighborhood {
            assert!(neighbor.x.abs() <= 1 && neighbor.y.abs() <= 1 && neighbor.z.abs() <= 1);
        }

        let cardinals = chunk.cardinal_neighbors();
        assert_eq!(cardinals.len(), 6);
    }
}
