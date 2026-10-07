//! Client-side WebAssembly voxel chunk decompression and mesh extraction.

use wasm_bindgen::prelude::*;

const CHUNK_SIZE: usize = 16;
const CHUNK_VOXELS: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE; // 4096
const MAGIC_VXR1: [u8; 4] = *b"VXR1";
const MAX_RLE_PAYLOAD_SIZE: usize = 64 * 1024;

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

/// Generates culled face boundary mesh vertices.
fn generate_culled_mesh(voxels: &[u16; CHUNK_VOXELS]) -> Vec<f32> {
    let mut vertices = Vec::with_capacity(1024);

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
                if lx + 1 == CHUNK_SIZE || voxels[index_from_local(lx + 1, ly, lz)] == 0 {
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
                if lx == 0 || voxels[index_from_local(lx - 1, ly, lz)] == 0 {
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
                if ly + 1 == CHUNK_SIZE || voxels[index_from_local(lx, ly + 1, lz)] == 0 {
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
                if ly == 0 || voxels[index_from_local(lx, ly - 1, lz)] == 0 {
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
                if lz + 1 == CHUNK_SIZE || voxels[index_from_local(lx, ly, lz + 1)] == 0 {
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
                if lz == 0 || voxels[index_from_local(lx, ly, lz - 1)] == 0 {
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
}
