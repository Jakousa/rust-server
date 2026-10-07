//! Binary wire protocol and modular packet routing for voxel subsystem.

use crate::coords::{VoxelChunkCoord, VoxelCoord};

/// Assigned modular packet routing ID for the Voxel subsystem.
pub const VOXEL_MODULE_ID: u16 = 0x00A0;

pub const SUB_OPCODE_CHUNK_REQUEST: u16 = 0x0001;
pub const SUB_OPCODE_CHUNK_DATA: u16 = 0x0002;
pub const SUB_OPCODE_DELTA_BATCH: u16 = 0x0003;
pub const SUB_OPCODE_MODIFY_REQUEST: u16 = 0x0004;
pub const SUB_OPCODE_MODIFY_RESPONSE: u16 = 0x0005;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VoxelPacketError {
    #[error("Payload truncated (size {0}, expected at least {1})")]
    PayloadTruncated(usize, usize),
    #[error("Invalid delta count: {0}")]
    InvalidDeltaCount(usize),
    #[error("Unknown sub-opcode: {0}")]
    UnknownSubOpcode(u16),
}

/// Client request to fetch full compressed data for a chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoxelChunkRequestPacket {
    pub chunk: VoxelChunkCoord,
}

impl VoxelChunkRequestPacket {
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(12);
        buf.extend_from_slice(&self.chunk.x.to_le_bytes());
        buf.extend_from_slice(&self.chunk.y.to_le_bytes());
        buf.extend_from_slice(&self.chunk.z.to_le_bytes());
        buf
    }

    pub fn decode(payload: &[u8]) -> Result<Self, VoxelPacketError> {
        if payload.len() < 12 {
            return Err(VoxelPacketError::PayloadTruncated(payload.len(), 12));
        }
        let x = i32::from_le_bytes(payload[0..4].try_into().unwrap());
        let y = i32::from_le_bytes(payload[4..8].try_into().unwrap());
        let z = i32::from_le_bytes(payload[8..12].try_into().unwrap());
        Ok(Self {
            chunk: VoxelChunkCoord::new(x, y, z),
        })
    }
}

/// Server response containing full RLE-compressed chunk data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoxelChunkDataPacket {
    pub chunk: VoxelChunkCoord,
    pub rle_payload: Vec<u8>,
}

impl VoxelChunkDataPacket {
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(12 + self.rle_payload.len());
        buf.extend_from_slice(&self.chunk.x.to_le_bytes());
        buf.extend_from_slice(&self.chunk.y.to_le_bytes());
        buf.extend_from_slice(&self.chunk.z.to_le_bytes());
        buf.extend_from_slice(&self.rle_payload);
        buf
    }

    pub fn decode(payload: &[u8]) -> Result<Self, VoxelPacketError> {
        if payload.len() < 12 {
            return Err(VoxelPacketError::PayloadTruncated(payload.len(), 12));
        }
        let x = i32::from_le_bytes(payload[0..4].try_into().unwrap());
        let y = i32::from_le_bytes(payload[4..8].try_into().unwrap());
        let z = i32::from_le_bytes(payload[8..12].try_into().unwrap());
        Ok(Self {
            chunk: VoxelChunkCoord::new(x, y, z),
            rle_payload: payload[12..].to_vec(),
        })
    }
}

/// Single voxel mutation within a chunk delta batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoxelDeltaEntry {
    pub lx: u8,
    pub ly: u8,
    pub lz: u8,
    pub new_voxel: u16,
}

impl VoxelDeltaEntry {
    pub fn encode(&self) -> [u8; 5] {
        let val_bytes = self.new_voxel.to_le_bytes();
        [self.lx, self.ly, self.lz, val_bytes[0], val_bytes[1]]
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, VoxelPacketError> {
        if bytes.len() < 5 {
            return Err(VoxelPacketError::PayloadTruncated(bytes.len(), 5));
        }
        Ok(Self {
            lx: bytes[0],
            ly: bytes[1],
            lz: bytes[2],
            new_voxel: u16::from_le_bytes([bytes[3], bytes[4]]),
        })
    }
}

/// Compact delta replication packet streaming modified blocks in a chunk to observers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoxelDeltaBatchPacket {
    pub chunk: VoxelChunkCoord,
    pub revision: u64,
    pub deltas: Vec<VoxelDeltaEntry>,
}

impl VoxelDeltaBatchPacket {
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(22 + self.deltas.len() * 5);
        buf.extend_from_slice(&self.chunk.x.to_le_bytes());
        buf.extend_from_slice(&self.chunk.y.to_le_bytes());
        buf.extend_from_slice(&self.chunk.z.to_le_bytes());
        buf.extend_from_slice(&self.revision.to_le_bytes());
        buf.extend_from_slice(&(self.deltas.len() as u16).to_le_bytes());
        for delta in &self.deltas {
            buf.extend_from_slice(&delta.encode());
        }
        buf
    }

    pub fn decode(payload: &[u8]) -> Result<Self, VoxelPacketError> {
        if payload.len() < 22 {
            return Err(VoxelPacketError::PayloadTruncated(payload.len(), 22));
        }
        let x = i32::from_le_bytes(payload[0..4].try_into().unwrap());
        let y = i32::from_le_bytes(payload[4..8].try_into().unwrap());
        let z = i32::from_le_bytes(payload[8..12].try_into().unwrap());
        let revision = u64::from_le_bytes(payload[12..20].try_into().unwrap());
        let delta_count = u16::from_le_bytes(payload[20..22].try_into().unwrap()) as usize;

        let expected_len = 22 + delta_count * 5;
        if payload.len() < expected_len {
            return Err(VoxelPacketError::PayloadTruncated(
                payload.len(),
                expected_len,
            ));
        }

        let mut deltas = Vec::with_capacity(delta_count);
        let mut offset = 22;
        for _ in 0..delta_count {
            deltas.push(VoxelDeltaEntry::decode(&payload[offset..offset + 5])?);
            offset += 5;
        }

        Ok(Self {
            chunk: VoxelChunkCoord::new(x, y, z),
            revision,
            deltas,
        })
    }
}

/// Client request to authoritatively dig or place a voxel block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoxelModifyRequestPacket {
    pub target_voxel: VoxelCoord,
    pub new_voxel: u16,
}

impl VoxelModifyRequestPacket {
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(26);
        buf.extend_from_slice(&self.target_voxel.x.to_le_bytes());
        buf.extend_from_slice(&self.target_voxel.y.to_le_bytes());
        buf.extend_from_slice(&self.target_voxel.z.to_le_bytes());
        buf.extend_from_slice(&self.new_voxel.to_le_bytes());
        buf
    }

    pub fn decode(payload: &[u8]) -> Result<Self, VoxelPacketError> {
        if payload.len() < 26 {
            return Err(VoxelPacketError::PayloadTruncated(payload.len(), 26));
        }
        let x = i64::from_le_bytes(payload[0..8].try_into().unwrap());
        let y = i64::from_le_bytes(payload[8..16].try_into().unwrap());
        let z = i64::from_le_bytes(payload[16..24].try_into().unwrap());
        let new_voxel = u16::from_le_bytes(payload[24..26].try_into().unwrap());
        Ok(Self {
            target_voxel: VoxelCoord::new(x, y, z),
            new_voxel,
        })
    }
}

/// Authoritative server modification response sent back to the initiating client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoxelModifyResponsePacket {
    pub status: u8,
    pub target_voxel: VoxelCoord,
    pub new_voxel: u16,
}

impl VoxelModifyResponsePacket {
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(27);
        buf.push(self.status);
        buf.extend_from_slice(&self.target_voxel.x.to_le_bytes());
        buf.extend_from_slice(&self.target_voxel.y.to_le_bytes());
        buf.extend_from_slice(&self.target_voxel.z.to_le_bytes());
        buf.extend_from_slice(&self.new_voxel.to_le_bytes());
        buf
    }

    pub fn decode(payload: &[u8]) -> Result<Self, VoxelPacketError> {
        if payload.len() < 27 {
            return Err(VoxelPacketError::PayloadTruncated(payload.len(), 27));
        }
        let status = payload[0];
        let x = i64::from_le_bytes(payload[1..9].try_into().unwrap());
        let y = i64::from_le_bytes(payload[9..17].try_into().unwrap());
        let z = i64::from_le_bytes(payload[17..25].try_into().unwrap());
        let new_voxel = u16::from_le_bytes(payload[25..27].try_into().unwrap());
        Ok(Self {
            status,
            target_voxel: VoxelCoord::new(x, y, z),
            new_voxel,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_voxel_chunk_request_roundtrip() {
        let pkt = VoxelChunkRequestPacket {
            chunk: VoxelChunkCoord::new(-10, 5, 42),
        };
        let encoded = pkt.encode();
        assert_eq!(encoded.len(), 12);
        let decoded = VoxelChunkRequestPacket::decode(&encoded).expect("decode");
        assert_eq!(decoded, pkt);
    }

    #[test]
    fn test_voxel_chunk_data_roundtrip() {
        let pkt = VoxelChunkDataPacket {
            chunk: VoxelChunkCoord::new(1, -2, 3),
            rle_payload: vec![1, 2, 3, 4, 5, 6, 7],
        };
        let encoded = pkt.encode();
        let decoded = VoxelChunkDataPacket::decode(&encoded).expect("decode");
        assert_eq!(decoded, pkt);
    }

    #[test]
    fn test_voxel_delta_batch_roundtrip() {
        let pkt = VoxelDeltaBatchPacket {
            chunk: VoxelChunkCoord::new(7, 8, 9),
            revision: 42,
            deltas: vec![
                VoxelDeltaEntry {
                    lx: 0,
                    ly: 1,
                    lz: 2,
                    new_voxel: 100,
                },
                VoxelDeltaEntry {
                    lx: 15,
                    ly: 14,
                    lz: 13,
                    new_voxel: 0,
                },
            ],
        };
        let encoded = pkt.encode();
        assert_eq!(encoded.len(), 22 + 2 * 5);
        let decoded = VoxelDeltaBatchPacket::decode(&encoded).expect("decode");
        assert_eq!(decoded, pkt);
    }

    #[test]
    fn test_voxel_modify_packets_roundtrip() {
        let req = VoxelModifyRequestPacket {
            target_voxel: VoxelCoord::new(-100, 200, -300),
            new_voxel: 5,
        };
        let req_encoded = req.encode();
        assert_eq!(req_encoded.len(), 26);
        let req_decoded = VoxelModifyRequestPacket::decode(&req_encoded).expect("decode");
        assert_eq!(req_decoded, req);

        let resp = VoxelModifyResponsePacket {
            status: 0,
            target_voxel: VoxelCoord::new(-100, 200, -300),
            new_voxel: 5,
        };
        let resp_encoded = resp.encode();
        assert_eq!(resp_encoded.len(), 27);
        let resp_decoded = VoxelModifyResponsePacket::decode(&resp_encoded).expect("decode");
        assert_eq!(resp_decoded, resp);
    }

    #[test]
    fn test_packet_truncation_rejections() {
        let req = VoxelModifyRequestPacket {
            target_voxel: VoxelCoord::new(1, 2, 3),
            new_voxel: 1,
        };
        let encoded = req.encode();
        assert!(VoxelModifyRequestPacket::decode(&encoded[..25]).is_err());
        assert!(VoxelChunkRequestPacket::decode(&encoded[..11]).is_err());
    }
}
