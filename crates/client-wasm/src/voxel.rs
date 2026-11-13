//! Client-side WebAssembly voxel chunk decompression and mesh extraction.

use std::collections::HashMap;

use wasm_bindgen::prelude::*;

const CHUNK_SIZE: usize = 16;
const CHUNK_VOXELS: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE; // 4096
const MAGIC_VXR1: [u8; 4] = *b"VXR1";
const MAX_RLE_PAYLOAD_SIZE: usize = 64 * 1024;

/// Boundary slices of adjacent chunks for inter-chunk face culling.
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

/// Extracts a 16x16 boundary slice along the X axis (`index = ly + lz * 16`).
pub fn extract_slice_x(voxels: &[u16; CHUNK_VOXELS], x: usize) -> Box<[u16; 256]> {
    let mut slice = Box::new([0u16; 256]);
    for lz in 0..CHUNK_SIZE {
        for ly in 0..CHUNK_SIZE {
            slice[ly + lz * CHUNK_SIZE] = voxels[index_from_local(x, ly, lz)];
        }
    }
    slice
}

/// Extracts a 16x16 boundary slice along the Y axis (`index = lx + lz * 16`).
pub fn extract_slice_y(voxels: &[u16; CHUNK_VOXELS], y: usize) -> Box<[u16; 256]> {
    let mut slice = Box::new([0u16; 256]);
    for lz in 0..CHUNK_SIZE {
        for lx in 0..CHUNK_SIZE {
            slice[lx + lz * CHUNK_SIZE] = voxels[index_from_local(lx, y, lz)];
        }
    }
    slice
}

/// Extracts a 16x16 boundary slice along the Z axis (`index = lx + ly * 16`).
pub fn extract_slice_z(voxels: &[u16; CHUNK_VOXELS], z: usize) -> Box<[u16; 256]> {
    let mut slice = Box::new([0u16; 256]);
    for ly in 0..CHUNK_SIZE {
        for lx in 0..CHUNK_SIZE {
            slice[lx + ly * CHUNK_SIZE] = voxels[index_from_local(lx, ly, z)];
        }
    }
    slice
}

struct StoredChunk {
    voxels: Box<[u16; CHUNK_VOXELS]>,
    revision: u64,
}

/// Client-side cache of decoded voxel chunks supporting neighbor-aware boundary face culling.
#[wasm_bindgen]
#[derive(Default)]
pub struct VoxelChunkStore {
    chunks: HashMap<(i32, i32, i32), StoredChunk>,
}

#[wasm_bindgen]
impl VoxelChunkStore {
    /// Creates an empty `VoxelChunkStore`.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Decodes an RLE-compressed chunk payload and stores it at chunk coordinates `(cx, cy, cz)`.
    /// Returns the decoded chunk revision.
    pub fn insert_chunk_rle(
        &mut self,
        cx: i32,
        cy: i32,
        cz: i32,
        data: &[u8],
    ) -> Result<u64, String> {
        let (voxels, revision) = decode_rle_voxels(data)?;
        self.chunks
            .insert((cx, cy, cz), StoredChunk { voxels, revision });
        Ok(revision)
    }

    /// Removes a chunk at `(cx, cy, cz)`. Returns `true` if a chunk was present.
    pub fn remove_chunk(&mut self, cx: i32, cy: i32, cz: i32) -> bool {
        self.chunks.remove(&(cx, cy, cz)).is_some()
    }

    /// Returns the number of loaded chunks in the store.
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Clears all loaded chunks from the store.
    pub fn clear(&mut self) {
        self.chunks.clear();
    }

    /// Generates a culled boundary mesh for the chunk at `(cx, cy, cz)` using adjacent loaded neighbors.
    ///
    /// Matches `crates/plugins/voxel/src/mesh.rs` unloaded neighbor policy: unloaded neighbors are
    /// treated as solid so phantom external walls are not emitted until adjacent chunks load.
    pub fn build_chunk_mesh(&self, cx: i32, cy: i32, cz: i32) -> Option<VoxelChunkMesh> {
        let chunk = self.chunks.get(&(cx, cy, cz))?;
        let neighbors = self.extract_neighbor_slices(cx, cy, cz);
        let vertices = generate_culled_mesh_with_neighbors(&chunk.voxels, &neighbors);
        Some(VoxelChunkMesh {
            vertices,
            revision: chunk.revision,
        })
    }

    /// Generates raw WebGL vertex floats for the chunk at `(cx, cy, cz)` using neighbor culling.
    pub fn build_chunk_mesh_vertices(&self, cx: i32, cy: i32, cz: i32) -> Option<Vec<f32>> {
        self.build_chunk_mesh(cx, cy, cz).map(|mesh| mesh.vertices)
    }
}

impl VoxelChunkStore {
    /// Extracts boundary slices from all loaded cardinal neighbors around `(cx, cy, cz)`.
    pub fn extract_neighbor_slices(&self, cx: i32, cy: i32, cz: i32) -> NeighborBoundarySlices {
        let mut slices = NeighborBoundarySlices::default();

        if let Some(nx) = cx
            .checked_add(1)
            .and_then(|x| self.chunks.get(&(x, cy, cz)))
        {
            slices.pos_x = Some(extract_slice_x(&nx.voxels, 0));
        }
        if let Some(nx) = cx
            .checked_sub(1)
            .and_then(|x| self.chunks.get(&(x, cy, cz)))
        {
            slices.neg_x = Some(extract_slice_x(&nx.voxels, CHUNK_SIZE - 1));
        }
        if let Some(ny) = cy
            .checked_add(1)
            .and_then(|y| self.chunks.get(&(cx, y, cz)))
        {
            slices.pos_y = Some(extract_slice_y(&ny.voxels, 0));
        }
        if let Some(ny) = cy
            .checked_sub(1)
            .and_then(|y| self.chunks.get(&(cx, y, cz)))
        {
            slices.neg_y = Some(extract_slice_y(&ny.voxels, CHUNK_SIZE - 1));
        }
        if let Some(nz) = cz
            .checked_add(1)
            .and_then(|z| self.chunks.get(&(cx, cy, z)))
        {
            slices.pos_z = Some(extract_slice_z(&nz.voxels, 0));
        }
        if let Some(nz) = cz
            .checked_sub(1)
            .and_then(|z| self.chunks.get(&(cx, cy, z)))
        {
            slices.neg_z = Some(extract_slice_z(&nz.voxels, CHUNK_SIZE - 1));
        }

        slices
    }
}

/// Result structure for client-side decoded voxel meshes.
#[wasm_bindgen]
pub struct VoxelChunkMesh {
    vertices: Vec<f32>,
    revision: u64,
}

#[wasm_bindgen]
impl VoxelChunkMesh {
    /// Returns a flat `Float32Array` compatible buffer of vertex attributes.
    /// Memory layout per vertex (7 floats): `[x, y, z, nx, ny, nz, voxel_id]`
    pub fn get_vertices(&self) -> Vec<f32> {
        self.vertices.clone()
    }

    /// Number of vertices in the mesh buffer.
    pub fn vertex_count(&self) -> usize {
        self.vertices.len() / 7
    }

    /// Number of triangles in the mesh buffer.
    pub fn triangle_count(&self) -> usize {
        self.vertex_count() / 3
    }

    /// Chunk revision number.
    pub fn revision(&self) -> u64 {
        self.revision
    }
}

/// Decodes an RLE-compressed voxel chunk payload into a WebGL-compatible flat vertex buffer.
///
/// Memory layout per vertex (7 floats):
/// `[x, y, z, nx, ny, nz, voxel_id as f32]`
#[wasm_bindgen]
pub fn decode_voxel_chunk_mesh(data: &[u8]) -> Result<Vec<f32>, String> {
    let (voxels, _revision) = decode_rle_voxels(data)?;
    Ok(generate_culled_mesh(&voxels))
}

/// Decodes RLE payload into `VoxelChunkMesh` instance.
#[wasm_bindgen]
pub fn decode_voxel_chunk(data: &[u8]) -> Result<VoxelChunkMesh, String> {
    let (voxels, revision) = decode_rle_voxels(data)?;
    let vertices = generate_culled_mesh(&voxels);
    Ok(VoxelChunkMesh { vertices, revision })
}

/// Decompresses raw RLE payload into a 4,096 voxel array and revision counter.
fn decode_rle_voxels(bytes: &[u8]) -> Result<(Box<[u16; CHUNK_VOXELS]>, u64), String> {
    if bytes.len() > MAX_RLE_PAYLOAD_SIZE {
        return Err(format!(
            "Payload exceeds max allowed size ({} > {})",
            bytes.len(),
            MAX_RLE_PAYLOAD_SIZE
        ));
    }
    if bytes.len() < 16 {
        return Err(format!("Payload too small ({})", bytes.len()));
    }

    if bytes[0..4] != MAGIC_VXR1 {
        return Err("Invalid magic header for voxel chunk".to_string());
    }

    let revision = u64::from_le_bytes(
        bytes[4..12]
            .try_into()
            .map_err(|_| "Failed reading revision".to_string())?,
    );
    let span_count = u32::from_le_bytes(
        bytes[12..16]
            .try_into()
            .map_err(|_| "Failed reading span count".to_string())?,
    ) as usize;

    let mut voxels = Box::new([0u16; CHUNK_VOXELS]);
    let mut offset = 16;
    let mut total_count = 0;

    for _ in 0..span_count {
        if offset + 4 > bytes.len() {
            return Err("Unexpected EOF in RLE stream".to_string());
        }

        let count = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]) as usize;
        let voxel_id = u16::from_le_bytes([bytes[offset + 2], bytes[offset + 3]]);
        offset += 4;

        if total_count + count > CHUNK_VOXELS {
            return Err(format!(
                "Decompression count overflow ({} > {})",
                total_count + count,
                CHUNK_VOXELS
            ));
        }

        voxels[total_count..total_count + count].fill(voxel_id);
        total_count += count;
    }

    if total_count != CHUNK_VOXELS {
        return Err(format!(
            "Decompression count underflow ({} != {})",
            total_count, CHUNK_VOXELS
        ));
    }

    Ok((voxels, revision))
}

#[inline(always)]
fn index_from_local(lx: usize, ly: usize, lz: usize) -> usize {
    lx + ly * CHUNK_SIZE + lz * CHUNK_SIZE * CHUNK_SIZE
}

#[inline(always)]
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

#[inline(always)]
fn show_boundary_face(slice: Option<&[u16; 256]>, idx: usize, unloaded_is_air: bool) -> bool {
    match slice {
        Some(s) => s[idx] == 0,
        None => unloaded_is_air,
    }
}

/// Generates culled face boundary mesh vertices for an isolated chunk (no neighbor context).
fn generate_culled_mesh(voxels: &[u16; CHUNK_VOXELS]) -> Vec<f32> {
    generate_culled_mesh_inner(voxels, None)
}

/// Generates culled face boundary mesh vertices using adjacent chunk boundary slices.
///
/// Matches `crates/plugins/voxel/src/mesh.rs`: unloaded neighbors (`None`) are treated as solid.
pub fn generate_culled_mesh_with_neighbors(
    voxels: &[u16; CHUNK_VOXELS],
    neighbors: &NeighborBoundarySlices,
) -> Vec<f32> {
    generate_culled_mesh_inner(voxels, Some(neighbors))
}

fn generate_culled_mesh_inner(
    voxels: &[u16; CHUNK_VOXELS],
    neighbors: Option<&NeighborBoundarySlices>,
) -> Vec<f32> {
    let mut vertices = Vec::with_capacity(1024);
    let unloaded_is_air = neighbors.is_none();

    for lz in 0..CHUNK_SIZE {
        for ly in 0..CHUNK_SIZE {
            for lx in 0..CHUNK_SIZE {
                let idx = index_from_local(lx, ly, lz);
                let voxel_id = voxels[idx];
                if voxel_id == 0 {
                    continue;
                }

                let fx = lx as f32;
                let fy = ly as f32;
                let fz = lz as f32;
                let v_id = voxel_id as f32;

                // 1. +X
                let show_pos_x = if lx + 1 < CHUNK_SIZE {
                    voxels[index_from_local(lx + 1, ly, lz)] == 0
                } else {
                    show_boundary_face(
                        neighbors.and_then(|n| n.pos_x.as_deref()),
                        ly + lz * CHUNK_SIZE,
                        unloaded_is_air,
                    )
                };
                if show_pos_x {
                    emit_quad(
                        &mut vertices,
                        (fx + 1.0, fy, fz),
                        (fx + 1.0, fy + 1.0, fz),
                        (fx + 1.0, fy + 1.0, fz + 1.0),
                        (fx + 1.0, fy, fz + 1.0),
                        (1.0, 0.0, 0.0),
                        v_id,
                    );
                }

                // 2. -X
                let show_neg_x = if lx > 0 {
                    voxels[index_from_local(lx - 1, ly, lz)] == 0
                } else {
                    show_boundary_face(
                        neighbors.and_then(|n| n.neg_x.as_deref()),
                        ly + lz * CHUNK_SIZE,
                        unloaded_is_air,
                    )
                };
                if show_neg_x {
                    emit_quad(
                        &mut vertices,
                        (fx, fy, fz + 1.0),
                        (fx, fy + 1.0, fz + 1.0),
                        (fx, fy + 1.0, fz),
                        (fx, fy, fz),
                        (-1.0, 0.0, 0.0),
                        v_id,
                    );
                }

                // 3. +Y
                let show_pos_y = if ly + 1 < CHUNK_SIZE {
                    voxels[index_from_local(lx, ly + 1, lz)] == 0
                } else {
                    show_boundary_face(
                        neighbors.and_then(|n| n.pos_y.as_deref()),
                        lx + lz * CHUNK_SIZE,
                        unloaded_is_air,
                    )
                };
                if show_pos_y {
                    emit_quad(
                        &mut vertices,
                        (fx, fy + 1.0, fz),
                        (fx, fy + 1.0, fz + 1.0),
                        (fx + 1.0, fy + 1.0, fz + 1.0),
                        (fx + 1.0, fy + 1.0, fz),
                        (0.0, 1.0, 0.0),
                        v_id,
                    );
                }

                // 4. -Y
                let show_neg_y = if ly > 0 {
                    voxels[index_from_local(lx, ly - 1, lz)] == 0
                } else {
                    show_boundary_face(
                        neighbors.and_then(|n| n.neg_y.as_deref()),
                        lx + lz * CHUNK_SIZE,
                        unloaded_is_air,
                    )
                };
                if show_neg_y {
                    emit_quad(
                        &mut vertices,
                        (fx, fy, fz + 1.0),
                        (fx, fy, fz),
                        (fx + 1.0, fy, fz),
                        (fx + 1.0, fy, fz + 1.0),
                        (0.0, -1.0, 0.0),
                        v_id,
                    );
                }

                // 5. +Z
                let show_pos_z = if lz + 1 < CHUNK_SIZE {
                    voxels[index_from_local(lx, ly, lz + 1)] == 0
                } else {
                    show_boundary_face(
                        neighbors.and_then(|n| n.pos_z.as_deref()),
                        lx + ly * CHUNK_SIZE,
                        unloaded_is_air,
                    )
                };
                if show_pos_z {
                    emit_quad(
                        &mut vertices,
                        (fx, fy, fz + 1.0),
                        (fx + 1.0, fy, fz + 1.0),
                        (fx + 1.0, fy + 1.0, fz + 1.0),
                        (fx, fy + 1.0, fz + 1.0),
                        (0.0, 0.0, 1.0),
                        v_id,
                    );
                }

                // 6. -Z
                let show_neg_z = if lz > 0 {
                    voxels[index_from_local(lx, ly, lz - 1)] == 0
                } else {
                    show_boundary_face(
                        neighbors.and_then(|n| n.neg_z.as_deref()),
                        lx + ly * CHUNK_SIZE,
                        unloaded_is_air,
                    )
                };
                if show_neg_z {
                    emit_quad(
                        &mut vertices,
                        (fx + 1.0, fy, fz),
                        (fx, fy, fz),
                        (fx, fy + 1.0, fz),
                        (fx + 1.0, fy + 1.0, fz),
                        (0.0, 0.0, -1.0),
                        v_id,
                    );
                }
            }
        }
    }

    vertices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_wasm_empty_chunk_mesh() {
        let mut rle = Vec::new();
        rle.extend_from_slice(&MAGIC_VXR1);
        rle.extend_from_slice(&1u64.to_le_bytes()); // revision 1
        rle.extend_from_slice(&1u32.to_le_bytes()); // 1 span
        rle.extend_from_slice(&4096u16.to_le_bytes()); // 4096 air voxels
        rle.extend_from_slice(&0u16.to_le_bytes());

        let vertices = decode_voxel_chunk_mesh(&rle).expect("decode mesh");
        assert_eq!(vertices.len(), 0);

        let mesh = decode_voxel_chunk(&rle).expect("decode mesh obj");
        assert_eq!(mesh.vertex_count(), 0);
        assert_eq!(mesh.revision(), 1);
    }

    #[test]
    fn test_client_wasm_solid_block_mesh() {
        let mut rle = Vec::new();
        rle.extend_from_slice(&MAGIC_VXR1);
        rle.extend_from_slice(&2u64.to_le_bytes());
        rle.extend_from_slice(&3u32.to_le_bytes()); // 3 spans
                                                    // 1 solid block at index 100
        rle.extend_from_slice(&100u16.to_le_bytes());
        rle.extend_from_slice(&0u16.to_le_bytes());
        rle.extend_from_slice(&1u16.to_le_bytes());
        rle.extend_from_slice(&42u16.to_le_bytes());
        rle.extend_from_slice(&((4096 - 101) as u16).to_le_bytes());
        rle.extend_from_slice(&0u16.to_le_bytes());

        let mesh = decode_voxel_chunk(&rle).expect("decode mesh");
        // 1 solid block = 6 faces * 6 vertices = 36 vertices
        assert_eq!(mesh.vertex_count(), 36);
        assert_eq!(mesh.triangle_count(), 12);
        assert_eq!(mesh.revision(), 2);
    }

    fn encode_rle_from_voxels(voxels: &[u16; CHUNK_VOXELS], revision: u64) -> Vec<u8> {
        let mut spans: Vec<(u16, u16)> = Vec::new();
        let mut current = voxels[0];
        let mut count: u16 = 0;
        for &v in voxels.iter() {
            if v == current {
                count += 1;
            } else {
                spans.push((count, current));
                current = v;
                count = 1;
            }
        }
        spans.push((count, current));

        let mut rle = Vec::new();
        rle.extend_from_slice(&MAGIC_VXR1);
        rle.extend_from_slice(&revision.to_le_bytes());
        rle.extend_from_slice(&(spans.len() as u32).to_le_bytes());
        for (c, id) in spans {
            rle.extend_from_slice(&c.to_le_bytes());
            rle.extend_from_slice(&id.to_le_bytes());
        }
        rle
    }

    #[test]
    fn test_voxel_chunk_store_boundary_face_culling_with_neighbors() {
        let mut store = VoxelChunkStore::new();

        // Chunk (0, 0, 0): single solid block at x=15, y=7, z=7 (on the +X boundary)
        let mut voxels_a = [0u16; CHUNK_VOXELS];
        voxels_a[index_from_local(15, 7, 7)] = 1;
        let rle_a = encode_rle_from_voxels(&voxels_a, 1);
        store.insert_chunk_rle(0, 0, 0, &rle_a).unwrap();

        // When neighbor (1, 0, 0) is unloaded, unloaded neighbor policy treats it as solid -> +X face culled (5 faces = 30 vertices)
        let mesh_unloaded = store.build_chunk_mesh(0, 0, 0).unwrap();
        assert_eq!(mesh_unloaded.vertex_count(), 30);

        // Load neighbor (1, 0, 0) as empty Air -> +X face exposed (6 faces = 36 vertices)
        let voxels_air = [0u16; CHUNK_VOXELS];
        let rle_air = encode_rle_from_voxels(&voxels_air, 1);
        store.insert_chunk_rle(1, 0, 0, &rle_air).unwrap();
        let mesh_air_neighbor = store.build_chunk_mesh(0, 0, 0).unwrap();
        assert_eq!(mesh_air_neighbor.vertex_count(), 36);

        // Replace neighbor (1, 0, 0) with a touching solid block at x=0, y=7, z=7 -> shared face culled on both chunks
        let mut voxels_b = [0u16; CHUNK_VOXELS];
        voxels_b[index_from_local(0, 7, 7)] = 2;
        let rle_b = encode_rle_from_voxels(&voxels_b, 2);
        store.insert_chunk_rle(1, 0, 0, &rle_b).unwrap();

        let mesh_a_culled = store.build_chunk_mesh(0, 0, 0).unwrap();
        let mesh_b_culled = store.build_chunk_mesh(1, 0, 0).unwrap();
        assert_eq!(mesh_a_culled.vertex_count(), 30);
        assert_eq!(mesh_b_culled.vertex_count(), 30);
    }

    #[test]
    fn test_generate_culled_mesh_with_y_and_z_neighbors() {
        // Place solid block on +Y and -Z borders: (4, 15, 0)
        let mut voxels = [0u16; CHUNK_VOXELS];
        voxels[index_from_local(4, 15, 0)] = 3;

        // Loaded all-air neighbors on all 6 sides -> all 6 faces emitted (36 vertices)
        let all_air = NeighborBoundarySlices {
            pos_x: Some(Box::new([0u16; 256])),
            neg_x: Some(Box::new([0u16; 256])),
            pos_y: Some(Box::new([0u16; 256])),
            neg_y: Some(Box::new([0u16; 256])),
            pos_z: Some(Box::new([0u16; 256])),
            neg_z: Some(Box::new([0u16; 256])),
        };
        assert_eq!(
            generate_culled_mesh_with_neighbors(&voxels, &all_air).len() / 7,
            36
        );

        // Put solid voxels touching +Y (lx=4, lz=0) and -Z (lx=4, ly=15) -> 2 faces culled -> 4 faces = 24 vertices
        let mut pos_y_slice = Box::new([0u16; 256]);
        pos_y_slice[4] = 9; // lx + lz * 16 = 4 + 0 * 16
        let mut neg_z_slice = Box::new([0u16; 256]);
        neg_z_slice[4 + 15 * 16] = 9; // lx + ly * 16

        let touching = NeighborBoundarySlices {
            pos_y: Some(pos_y_slice),
            neg_z: Some(neg_z_slice),
            ..all_air
        };
        assert_eq!(
            generate_culled_mesh_with_neighbors(&voxels, &touching).len() / 7,
            24
        );
    }
}
