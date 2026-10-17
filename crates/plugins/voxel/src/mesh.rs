//! Boundary face culling and WebGL-ready mesh extraction.

use crate::chunk::PaletteChunk;
use crate::coords::{index_from_local, local_from_index, CHUNK_SIZE, CHUNK_VOXELS};

/// WebGL-compatible mesh buffer representing extracted boundary quads.
/// Each vertex contains 7 floats: `[x, y, z, nx, ny, nz, voxel_id as f32]`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VoxelMesh {
    pub vertices: Vec<f32>,
}

impl VoxelMesh {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(quad_count: usize) -> Self {
        Self {
            vertices: Vec::with_capacity(quad_count * 6 * 7),
        }
    }

    #[inline]
    pub fn vertex_count(&self) -> usize {
        self.vertices.len() / 7
    }

    #[inline]
    pub fn triangle_count(&self) -> usize {
        self.vertex_count() / 3
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }

    pub fn clear(&mut self) {
        self.vertices.clear();
    }
}

/// Boundary slices of adjacent chunks for deadlock-free face culling across boundaries.
#[derive(Debug, Clone, Default)]
pub struct NeighborBoundarySlices {
    /// Neighbor chunk at +X (its x = 0 slice, index = ly + lz * 16)
    pub pos_x: Option<Box<[u16; 256]>>,
    /// Neighbor chunk at -X (its x = 15 slice, index = ly + lz * 16)
    pub neg_x: Option<Box<[u16; 256]>>,
    /// Neighbor chunk at +Y (its y = 0 slice, index = lx + lz * 16)
    pub pos_y: Option<Box<[u16; 256]>>,
    /// Neighbor chunk at -Y (its y = 15 slice, index = lx + lz * 16)
    pub neg_y: Option<Box<[u16; 256]>>,
    /// Neighbor chunk at +Z (its z = 0 slice, index = lx + ly * 16)
    pub pos_z: Option<Box<[u16; 256]>>,
    /// Neighbor chunk at -Z (its z = 15 slice, index = lx + ly * 16)
    pub neg_z: Option<Box<[u16; 256]>>,
}

/// Extracts a 16×16 boundary slice along the X axis (`index = ly + lz * 16`).
pub fn extract_slice_x(chunk: &PaletteChunk, x: usize) -> Box<[u16; 256]> {
    let mut slice = Box::new([0u16; 256]);
    for lz in 0..CHUNK_SIZE {
        for ly in 0..CHUNK_SIZE {
            slice[ly + lz * CHUNK_SIZE] = chunk.get(index_from_local(x, ly, lz));
        }
    }
    slice
}

/// Extracts a 16×16 boundary slice along the Y axis (`index = lx + lz * 16`).
pub fn extract_slice_y(chunk: &PaletteChunk, y: usize) -> Box<[u16; 256]> {
    let mut slice = Box::new([0u16; 256]);
    for lz in 0..CHUNK_SIZE {
        for lx in 0..CHUNK_SIZE {
            slice[lx + lz * CHUNK_SIZE] = chunk.get(index_from_local(lx, y, lz));
        }
    }
    slice
}

/// Extracts a 16×16 boundary slice along the Z axis (`index = lx + ly * 16`).
pub fn extract_slice_z(chunk: &PaletteChunk, z: usize) -> Box<[u16; 256]> {
    let mut slice = Box::new([0u16; 256]);
    for ly in 0..CHUNK_SIZE {
        for lx in 0..CHUNK_SIZE {
            slice[lx + ly * CHUNK_SIZE] = chunk.get(index_from_local(lx, ly, z));
        }
    }
    slice
}

/// Emits two triangles (6 vertices, 42 floats) for a quad face.
#[inline]
fn emit_quad(
    vertices: &mut Vec<f32>,
    v0: (f32, f32, f32),
    v1: (f32, f32, f32),
    v2: (f32, f32, f32),
    v3: (f32, f32, f32),
    norm: (f32, f32, f32),
    voxel_id: f32,
) {
    // Triangle 1: v0, v1, v2
    vertices.extend_from_slice(&[v0.0, v0.1, v0.2, norm.0, norm.1, norm.2, voxel_id]);
    vertices.extend_from_slice(&[v1.0, v1.1, v1.2, norm.0, norm.1, norm.2, voxel_id]);
    vertices.extend_from_slice(&[v2.0, v2.1, v2.2, norm.0, norm.1, norm.2, voxel_id]);

    // Triangle 2: v0, v2, v3
    vertices.extend_from_slice(&[v0.0, v0.1, v0.2, norm.0, norm.1, norm.2, voxel_id]);
    vertices.extend_from_slice(&[v2.0, v2.1, v2.2, norm.0, norm.1, norm.2, voxel_id]);
    vertices.extend_from_slice(&[v3.0, v3.1, v3.2, norm.0, norm.1, norm.2, voxel_id]);
}

/// Generates a culled boundary mesh for a voxel chunk.
///
/// Unloaded neighbor policy: Unloaded neighbor chunks (`None` in `neighbors`) are treated
/// as solid, culling boundary faces to prevent phantom external geometry until the chunk loads.
pub fn generate_chunk_mesh(chunk: &PaletteChunk, neighbors: &NeighborBoundarySlices) -> VoxelMesh {
    if chunk.is_homogeneous() && chunk.get(0) == 0 {
        return VoxelMesh::new();
    }

    generate_mesh_from_raw(&chunk.unpack_all(), neighbors)
}

/// Generates a culled boundary mesh from a raw array of 4,096 voxels.
pub fn generate_mesh_from_raw(
    voxels: &[u16; CHUNK_VOXELS],
    neighbors: &NeighborBoundarySlices,
) -> VoxelMesh {
    let mut mesh = VoxelMesh::with_capacity(256);

    for idx in 0..CHUNK_VOXELS {
        let voxel_id = voxels[idx];
        if voxel_id == 0 {
            continue;
        }

        let (lx, ly, lz) = local_from_index(idx);
        let fx = lx as f32;
        let fy = ly as f32;
        let fz = lz as f32;
        let v_id = voxel_id as f32;

        // 1. +X Face (Right)
        let show_pos_x = if lx + 1 < CHUNK_SIZE {
            voxels[index_from_local(lx + 1, ly, lz)] == 0
        } else {
            // Check +X neighbor slice (x = 0)
            neighbors
                .pos_x
                .as_ref()
                .is_some_and(|s| s[ly + lz * CHUNK_SIZE] == 0)
        };
        if show_pos_x {
            emit_quad(
                &mut mesh.vertices,
                (fx + 1.0, fy, fz),
                (fx + 1.0, fy + 1.0, fz),
                (fx + 1.0, fy + 1.0, fz + 1.0),
                (fx + 1.0, fy, fz + 1.0),
                (1.0, 0.0, 0.0),
                v_id,
            );
        }

        // 2. -X Face (Left)
        let show_neg_x = if lx > 0 {
            voxels[index_from_local(lx - 1, ly, lz)] == 0
        } else {
            neighbors
                .neg_x
                .as_ref()
                .is_some_and(|s| s[ly + lz * CHUNK_SIZE] == 0)
        };
        if show_neg_x {
            emit_quad(
                &mut mesh.vertices,
                (fx, fy, fz + 1.0),
                (fx, fy + 1.0, fz + 1.0),
                (fx, fy + 1.0, fz),
                (fx, fy, fz),
                (-1.0, 0.0, 0.0),
                v_id,
            );
        }

        // 3. +Y Face (Top)
        let show_pos_y = if ly + 1 < CHUNK_SIZE {
            voxels[index_from_local(lx, ly + 1, lz)] == 0
        } else {
            neighbors
                .pos_y
                .as_ref()
                .is_some_and(|s| s[lx + lz * CHUNK_SIZE] == 0)
        };
        if show_pos_y {
            emit_quad(
                &mut mesh.vertices,
                (fx, fy + 1.0, fz),
                (fx, fy + 1.0, fz + 1.0),
                (fx + 1.0, fy + 1.0, fz + 1.0),
                (fx + 1.0, fy + 1.0, fz),
                (0.0, 1.0, 0.0),
                v_id,
            );
        }

        // 4. -Y Face (Bottom)
        let show_neg_y = if ly > 0 {
            voxels[index_from_local(lx, ly - 1, lz)] == 0
        } else {
            neighbors
                .neg_y
                .as_ref()
                .is_some_and(|s| s[lx + lz * CHUNK_SIZE] == 0)
        };
        if show_neg_y {
            emit_quad(
                &mut mesh.vertices,
                (fx, fy, fz + 1.0),
                (fx, fy, fz),
                (fx + 1.0, fy, fz),
                (fx + 1.0, fy, fz + 1.0),
                (0.0, -1.0, 0.0),
                v_id,
            );
        }

        // 5. +Z Face (Front)
        let show_pos_z = if lz + 1 < CHUNK_SIZE {
            voxels[index_from_local(lx, ly, lz + 1)] == 0
        } else {
            neighbors
                .pos_z
                .as_ref()
                .is_some_and(|s| s[lx + ly * CHUNK_SIZE] == 0)
        };
        if show_pos_z {
            emit_quad(
                &mut mesh.vertices,
                (fx, fy, fz + 1.0),
                (fx + 1.0, fy, fz + 1.0),
                (fx + 1.0, fy + 1.0, fz + 1.0),
                (fx, fy + 1.0, fz + 1.0),
                (0.0, 0.0, 1.0),
                v_id,
            );
        }

        // 6. -Z Face (Back)
        let show_neg_z = if lz > 0 {
            voxels[index_from_local(lx, ly, lz - 1)] == 0
        } else {
            neighbors
                .neg_z
                .as_ref()
                .is_some_and(|s| s[lx + ly * CHUNK_SIZE] == 0)
        };
        if show_neg_z {
            emit_quad(
                &mut mesh.vertices,
                (fx + 1.0, fy, fz),
                (fx, fy, fz),
                (fx, fy + 1.0, fz),
                (fx + 1.0, fy + 1.0, fz),
                (0.0, 0.0, -1.0),
                v_id,
            );
        }
    }

    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_air_chunk() {
        let chunk = PaletteChunk::new_empty();
        let neighbors = NeighborBoundarySlices::default();
        let mesh = generate_chunk_mesh(&chunk, &neighbors);
        assert!(mesh.is_empty());
        assert_eq!(mesh.vertex_count(), 0);
    }

    #[test]
    fn test_isolated_solid_block() {
        let mut chunk = PaletteChunk::new_empty();
        chunk.set(index_from_local(7, 7, 7), 1);
        let neighbors = NeighborBoundarySlices::default();
        let mesh = generate_chunk_mesh(&chunk, &neighbors);
        // 1 block = 6 faces = 12 triangles = 36 vertices
        assert_eq!(mesh.vertex_count(), 36);
        assert_eq!(mesh.triangle_count(), 12);
    }

    #[test]
    fn test_2x2x2_solid_cube_culling() {
        let mut chunk = PaletteChunk::new_empty();
        for lx in 5..=6 {
            for ly in 5..=6 {
                for lz in 5..=6 {
                    chunk.set(index_from_local(lx, ly, lz), 1);
                }
            }
        }
        let neighbors = NeighborBoundarySlices::default();
        let mesh = generate_chunk_mesh(&chunk, &neighbors);
        // A 2x2x2 cube has 6 outer sides * 4 block faces each = 24 outward faces
        // 24 faces * 6 vertices = 144 vertices. Internal faces are completely culled!
        assert_eq!(mesh.vertex_count(), 144);
        assert_eq!(mesh.triangle_count(), 48);
    }

    #[test]
    fn test_border_voxel_unloaded_neighbor_policy() {
        let mut chunk = PaletteChunk::new_empty();
        // Place solid block on border at x = 0
        chunk.set(index_from_local(0, 7, 7), 1);

        // When neighbor at -X is unloaded (None), the face toward -X is culled (treated as solid)
        let neighbors = NeighborBoundarySlices::default();
        let mesh = generate_chunk_mesh(&chunk, &neighbors);
        // 5 faces emitted (all except -X) = 30 vertices
        assert_eq!(mesh.vertex_count(), 30);

        // Now neighbor at -X is loaded and contains Air (0)
        let loaded_neighbors = NeighborBoundarySlices {
            neg_x: Some(Box::new([0u16; 256])),
            ..Default::default()
        };
        let mesh_loaded = generate_chunk_mesh(&chunk, &loaded_neighbors);
        // All 6 faces emitted = 36 vertices
        assert_eq!(mesh_loaded.vertex_count(), 36);
    }

    #[test]
    fn test_slice_extraction() {
        let mut chunk = PaletteChunk::new_empty();
        chunk.set(index_from_local(15, 3, 4), 99);

        let slice_x15 = extract_slice_x(&chunk, 15);
        assert_eq!(slice_x15[3 + 4 * 16], 99);
        assert_eq!(slice_x15[0], 0);

        let slice_x0 = extract_slice_x(&chunk, 0);
        assert_eq!(slice_x0[3 + 4 * 16], 0);
    }
}
