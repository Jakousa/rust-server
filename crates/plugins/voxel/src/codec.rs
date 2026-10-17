//! Run-Length Encoding (RLE) codecs for voxel chunks with decompression bomb defense.

use crate::chunk::PaletteChunk;
use crate::coords::CHUNK_VOXELS;

/// Maximum payload size in bytes (64 KB) to prevent memory exhaustion attacks.
pub const MAX_RLE_PAYLOAD_SIZE: usize = 64 * 1024;
/// Magic header bytes identifying Voxel RLE format v1.
pub const MAGIC_VXR1: [u8; 4] = *b"VXR1";

/// Codec errors for voxel chunk serialization and deserialization.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VoxelCodecError {
    #[error("Payload truncated or too small (size: {0})")]
    PayloadTooSmall(usize),

    #[error("Payload exceeds maximum size ({size} > {max})")]
    PayloadTooLarge { size: usize, max: usize },

    #[error("Invalid magic header: {0:?}")]
    InvalidMagic([u8; 4]),

    #[error("Malformed RLE payload: sum of counts is {sum}, expected {expected}")]
    MalformedRleCount { sum: usize, expected: usize },

    #[error("Premature end of RLE stream reading span {span_index}")]
    UnexpectedEof { span_index: usize },
}

/// Helper consolidating RLE span-encoding loop for both `PaletteChunk` and raw voxel arrays.
fn encode_rle_with<F: Fn(usize) -> u16>(revision: u64, get_voxel: F) -> Vec<u8> {
    let mut spans: Vec<(u16, u16)> = Vec::new();
    let mut current_voxel = get_voxel(0);
    let mut current_count: u16 = 1;

    for i in 1..CHUNK_VOXELS {
        let v = get_voxel(i);
        if v == current_voxel && current_count < u16::MAX {
            current_count += 1;
        } else {
            spans.push((current_count, current_voxel));
            current_voxel = v;
            current_count = 1;
        }
    }
    spans.push((current_count, current_voxel));

    let mut buf = Vec::with_capacity(16 + spans.len() * 4);
    buf.extend_from_slice(&MAGIC_VXR1);
    buf.extend_from_slice(&revision.to_le_bytes());
    buf.extend_from_slice(&(spans.len() as u32).to_le_bytes());

    for (count, voxel_id) in spans {
        buf.extend_from_slice(&count.to_le_bytes());
        buf.extend_from_slice(&voxel_id.to_le_bytes());
    }

    buf
}

/// Serializes a `PaletteChunk` into an RLE compressed byte stream.
pub fn encode_rle(chunk: &PaletteChunk) -> Vec<u8> {
    encode_rle_with(chunk.revision(), |i| chunk.get(i))
}

/// Decodes an RLE byte stream into a `PaletteChunk`, strictly enforcing that total voxels equal 4,096.
pub fn decode_rle(bytes: &[u8]) -> Result<PaletteChunk, VoxelCodecError> {
    let (voxels, revision) = decode_rle_raw(bytes)?;
    let mut chunk = PaletteChunk::from_raw_voxels(&voxels);
    chunk.revision = revision;
    Ok(chunk)
}

/// Serializes raw 4,096 voxels into an RLE compressed byte stream.
pub fn encode_rle_raw(voxels: &[u16; CHUNK_VOXELS], revision: u64) -> Vec<u8> {
    encode_rle_with(revision, |i| voxels[i])
}

/// Decodes an RLE byte stream into raw 4,096 voxels and revision counter.
pub fn decode_rle_raw(bytes: &[u8]) -> Result<(Box<[u16; CHUNK_VOXELS]>, u64), VoxelCodecError> {
    if bytes.len() > MAX_RLE_PAYLOAD_SIZE {
        return Err(VoxelCodecError::PayloadTooLarge {
            size: bytes.len(),
            max: MAX_RLE_PAYLOAD_SIZE,
        });
    }

    if bytes.len() < 16 {
        return Err(VoxelCodecError::PayloadTooSmall(bytes.len()));
    }

    if bytes[0..4] != MAGIC_VXR1 {
        let mut m = [0u8; 4];
        m.copy_from_slice(&bytes[0..4]);
        return Err(VoxelCodecError::InvalidMagic(m));
    }

    let revision = u64::from_le_bytes(
        bytes[4..12]
            .try_into()
            .map_err(|_| VoxelCodecError::PayloadTooSmall(bytes.len()))?,
    );

    let span_count = u32::from_le_bytes(
        bytes[12..16]
            .try_into()
            .map_err(|_| VoxelCodecError::PayloadTooSmall(bytes.len()))?,
    ) as usize;

    let mut voxels = Box::new([0u16; CHUNK_VOXELS]);
    let mut offset = 16;
    let mut total_count = 0;

    for span_idx in 0..span_count {
        if offset + 4 > bytes.len() {
            return Err(VoxelCodecError::UnexpectedEof {
                span_index: span_idx,
            });
        }

        let count = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]) as usize;
        let voxel_id = u16::from_le_bytes([bytes[offset + 2], bytes[offset + 3]]);
        offset += 4;

        if total_count + count > CHUNK_VOXELS {
            return Err(VoxelCodecError::MalformedRleCount {
                sum: total_count + count,
                expected: CHUNK_VOXELS,
            });
        }

        voxels[total_count..total_count + count].fill(voxel_id);
        total_count += count;
    }

    if total_count != CHUNK_VOXELS {
        return Err(VoxelCodecError::MalformedRleCount {
            sum: total_count,
            expected: CHUNK_VOXELS,
        });
    }

    Ok((voxels, revision))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_homogeneous_chunk_roundtrip() {
        let chunk = PaletteChunk::new_homogeneous(42);
        let encoded = encode_rle(&chunk);
        // Header (16 bytes) + 1 span (4 bytes) = 20 bytes!
        assert_eq!(encoded.len(), 20);

        let decoded = decode_rle(&encoded).expect("should decode");
        assert_eq!(decoded.get(0), 42);
        assert_eq!(decoded.get(4095), 42);
        assert_eq!(decoded.revision(), 0);
    }

    #[test]
    fn test_multi_voxel_chunk_roundtrip() {
        let mut chunk = PaletteChunk::new_empty();
        for i in 0..CHUNK_VOXELS {
            chunk.set(i, (i % 5) as u16);
        }
        let encoded = encode_rle(&chunk);
        let decoded = decode_rle(&encoded).expect("should decode");
        assert_eq!(decoded.unpack_all(), chunk.unpack_all());
        assert_eq!(decoded.revision(), chunk.revision());
    }

    #[test]
    fn test_decompression_bomb_rejection() {
        let mut bad_buf = Vec::new();
        bad_buf.extend_from_slice(&MAGIC_VXR1);
        bad_buf.extend_from_slice(&1u64.to_le_bytes());
        bad_buf.extend_from_slice(&1u32.to_le_bytes()); // 1 span
        bad_buf.extend_from_slice(&5000u16.to_le_bytes()); // 5000 > 4096!
        bad_buf.extend_from_slice(&1u16.to_le_bytes());

        let err = decode_rle(&bad_buf).unwrap_err();
        assert_eq!(
            err,
            VoxelCodecError::MalformedRleCount {
                sum: 5000,
                expected: CHUNK_VOXELS,
            }
        );
    }

    #[test]
    fn test_underflow_rejection() {
        let mut bad_buf = Vec::new();
        bad_buf.extend_from_slice(&MAGIC_VXR1);
        bad_buf.extend_from_slice(&1u64.to_le_bytes());
        bad_buf.extend_from_slice(&1u32.to_le_bytes());
        bad_buf.extend_from_slice(&4000u16.to_le_bytes()); // 4000 < 4096!
        bad_buf.extend_from_slice(&1u16.to_le_bytes());

        let err = decode_rle(&bad_buf).unwrap_err();
        assert_eq!(
            err,
            VoxelCodecError::MalformedRleCount {
                sum: 4000,
                expected: CHUNK_VOXELS,
            }
        );
    }

    #[test]
    fn test_invalid_magic_rejection() {
        let mut bad_buf = Vec::new();
        bad_buf.extend_from_slice(b"BAD!");
        bad_buf.extend_from_slice(&1u64.to_le_bytes());
        bad_buf.extend_from_slice(&0u32.to_le_bytes());

        let err = decode_rle(&bad_buf).unwrap_err();
        assert_eq!(err, VoxelCodecError::InvalidMagic(*b"BAD!"));
    }
}
