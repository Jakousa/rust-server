//! Client-facing protocol packet definitions and binary codecs.
//!
//! Provides compact binary framing and serialization for high-frequency
//! client-gateway and gateway-node communications (movement, entity state sync,
//! and interaction actions).

use bytes::{Buf, BufMut, Bytes, BytesMut};
use thiserror::Error;

/// Protocol magic header: 2 bytes `0x4D, 0x4D` ("MM" for MMO).
pub const PACKET_MAGIC: u16 = 0x4D4D;

/// Minimum header size in bytes: magic (2) + opcode (2) + payload_len (4) = 8 bytes.
pub const HEADER_SIZE: usize = 8;

/// Errors that can occur during packet framing, encoding, or decoding.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ClientCodecError {
    #[error("Packet buffer too short: expected at least {expected} bytes, got {got}")]
    BufferTooShort { expected: usize, got: usize },

    #[error("Invalid packet magic: expected {expected:#06x}, got {got:#06x}")]
    InvalidMagic { expected: u16, got: u16 },

    #[error("Unknown opcode: {0:#06x}")]
    UnknownOpcode(u16),

    #[error("Payload length mismatch: expected {expected} bytes, got {got}")]
    LengthMismatch { expected: usize, got: usize },

    #[error("Invalid packet data: {0}")]
    InvalidData(String),
}

/// Operation codes distinguishing packet types on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum Opcode {
    Movement = 0x0001,
    MovementCorrection = 0x0002,
    Interaction = 0x0003,
    InteractionResult = 0x0004,
    EntityState = 0x0005,
    EntityDespawn = 0x0006,
    Ping = 0x0007,
    Pong = 0x0008,
}

impl Opcode {
    pub fn from_u16(val: u16) -> Result<Self, ClientCodecError> {
        match val {
            0x0001 => Ok(Opcode::Movement),
            0x0002 => Ok(Opcode::MovementCorrection),
            0x0003 => Ok(Opcode::Interaction),
            0x0004 => Ok(Opcode::InteractionResult),
            0x0005 => Ok(Opcode::EntityState),
            0x0006 => Ok(Opcode::EntityDespawn),
            0x0007 => Ok(Opcode::Ping),
            0x0008 => Ok(Opcode::Pong),
            other => Err(ClientCodecError::UnknownOpcode(other)),
        }
    }
}

/// 3D Vector for coordinates and velocities.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn distance_squared(&self, other: &Self) -> f32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        let dz = self.z - other.z;
        dx * dx + dy * dy + dz * dz
    }

    pub fn distance(&self, other: &Self) -> f32 {
        self.distance_squared(other).sqrt()
    }

    pub fn encode(&self, dst: &mut BytesMut) {
        dst.put_f32_le(self.x);
        dst.put_f32_le(self.y);
        dst.put_f32_le(self.z);
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        if src.remaining() < 12 {
            return Err(ClientCodecError::BufferTooShort {
                expected: 12,
                got: src.remaining(),
            });
        }
        Ok(Self {
            x: src.get_f32_le(),
            y: src.get_f32_le(),
            z: src.get_f32_le(),
        })
    }
}

/// Interaction categories supported across entities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum InteractionType {
    Talk = 0,
    Attack = 1,
    UseItem = 2,
    Loot = 3,
    Trade = 4,
    Custom(u8),
}

impl From<u8> for InteractionType {
    fn from(val: u8) -> Self {
        match val {
            0 => InteractionType::Talk,
            1 => InteractionType::Attack,
            2 => InteractionType::UseItem,
            3 => InteractionType::Loot,
            4 => InteractionType::Trade,
            custom => InteractionType::Custom(custom),
        }
    }
}

impl From<InteractionType> for u8 {
    fn from(it: InteractionType) -> Self {
        match it {
            InteractionType::Talk => 0,
            InteractionType::Attack => 1,
            InteractionType::UseItem => 2,
            InteractionType::Loot => 3,
            InteractionType::Trade => 4,
            InteractionType::Custom(val) => val,
        }
    }
}

/// Client-originated movement packet.
#[derive(Debug, Clone, PartialEq)]
pub struct MovementPacket {
    pub entity_id: u64,
    pub position: Vec3,
    pub velocity: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub timestamp: u64,
    pub sequence: u32,
}

impl MovementPacket {
    pub fn encode(&self, dst: &mut BytesMut) {
        dst.put_u64_le(self.entity_id);
        self.position.encode(dst);
        self.velocity.encode(dst);
        dst.put_f32_le(self.yaw);
        dst.put_f32_le(self.pitch);
        dst.put_u64_le(self.timestamp);
        dst.put_u32_le(self.sequence);
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        // 8 (entity_id) + 12 (pos) + 12 (vel) + 4 (yaw) + 4 (pitch) + 8 (timestamp) + 4 (sequence) = 52 bytes
        const SIZE: usize = 52;
        if src.remaining() < SIZE {
            return Err(ClientCodecError::BufferTooShort {
                expected: SIZE,
                got: src.remaining(),
            });
        }
        let entity_id = src.get_u64_le();
        let position = Vec3::decode(src)?;
        let velocity = Vec3::decode(src)?;
        let yaw = src.get_f32_le();
        let pitch = src.get_f32_le();
        let timestamp = src.get_u64_le();
        let sequence = src.get_u32_le();

        Ok(Self {
            entity_id,
            position,
            velocity,
            yaw,
            pitch,
            timestamp,
            sequence,
        })
    }
}

/// Server-originated movement snap-back correction (anti-cheat / kinematic violation).
#[derive(Debug, Clone, PartialEq)]
pub struct MovementCorrectionPacket {
    pub entity_id: u64,
    pub corrected_position: Vec3,
    pub corrected_velocity: Vec3,
    pub sequence: u32,
    pub reason: u8,
}

impl MovementCorrectionPacket {
    pub fn encode(&self, dst: &mut BytesMut) {
        dst.put_u64_le(self.entity_id);
        self.corrected_position.encode(dst);
        self.corrected_velocity.encode(dst);
        dst.put_u32_le(self.sequence);
        dst.put_u8(self.reason);
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        // 8 + 12 + 12 + 4 + 1 = 37 bytes
        const SIZE: usize = 37;
        if src.remaining() < SIZE {
            return Err(ClientCodecError::BufferTooShort {
                expected: SIZE,
                got: src.remaining(),
            });
        }
        let entity_id = src.get_u64_le();
        let corrected_position = Vec3::decode(src)?;
        let corrected_velocity = Vec3::decode(src)?;
        let sequence = src.get_u32_le();
        let reason = src.get_u8();

        Ok(Self {
            entity_id,
            corrected_position,
            corrected_velocity,
            sequence,
            reason,
        })
    }
}

/// Client-originated interaction request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionPacket {
    pub source_guid: u64,
    pub target_guid: u64,
    pub interaction_type: InteractionType,
    pub action_id: u32,
    pub payload: Bytes,
}

impl InteractionPacket {
    pub fn encode(&self, dst: &mut BytesMut) {
        dst.put_u64_le(self.source_guid);
        dst.put_u64_le(self.target_guid);
        dst.put_u8(self.interaction_type.into());
        dst.put_u32_le(self.action_id);
        dst.put_u32_le(self.payload.len() as u32);
        dst.put_slice(&self.payload);
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        // 8 + 8 + 1 + 4 + 4 = 25 bytes minimum
        const MIN_SIZE: usize = 25;
        if src.remaining() < MIN_SIZE {
            return Err(ClientCodecError::BufferTooShort {
                expected: MIN_SIZE,
                got: src.remaining(),
            });
        }
        let source_guid = src.get_u64_le();
        let target_guid = src.get_u64_le();
        let interaction_type = InteractionType::from(src.get_u8());
        let action_id = src.get_u32_le();
        let payload_len = src.get_u32_le() as usize;

        if src.remaining() < payload_len {
            return Err(ClientCodecError::BufferTooShort {
                expected: payload_len,
                got: src.remaining(),
            });
        }
        let payload = src.copy_to_bytes(payload_len);

        Ok(Self {
            source_guid,
            target_guid,
            interaction_type,
            action_id,
            payload,
        })
    }
}

/// Server response to an interaction request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionResultPacket {
    pub action_id: u32,
    pub success: bool,
    pub error_code: u32,
    pub payload: Vec<u8>,
}

impl InteractionResultPacket {
    pub fn encode(&self, dst: &mut BytesMut) {
        dst.put_u32_le(self.action_id);
        dst.put_u8(if self.success { 1 } else { 0 });
        dst.put_u32_le(self.error_code);
        dst.put_u32_le(self.payload.len() as u32);
        dst.put_slice(&self.payload);
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        // 4 + 1 + 4 + 4 = 13 bytes minimum
        const MIN_SIZE: usize = 13;
        if src.remaining() < MIN_SIZE {
            return Err(ClientCodecError::BufferTooShort {
                expected: MIN_SIZE,
                got: src.remaining(),
            });
        }
        let action_id = src.get_u32_le();
        let success = src.get_u8() != 0;
        let error_code = src.get_u32_le();
        let payload_len = src.get_u32_le() as usize;

        if src.remaining() < payload_len {
            return Err(ClientCodecError::BufferTooShort {
                expected: payload_len,
                got: src.remaining(),
            });
        }
        let mut payload = vec![0u8; payload_len];
        src.copy_to_slice(&mut payload);

        Ok(Self {
            action_id,
            success,
            error_code,
            payload,
        })
    }
}

/// Server broadcast of authoritative entity state.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityStatePacket {
    pub entity_id: u64,
    pub position: Vec3,
    pub velocity: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub health: u32,
    pub max_health: u32,
    pub state_flags: u32,
    pub timestamp: u64,
}

impl EntityStatePacket {
    pub fn encode(&self, dst: &mut BytesMut) {
        dst.put_u64_le(self.entity_id);
        self.position.encode(dst);
        self.velocity.encode(dst);
        dst.put_f32_le(self.yaw);
        dst.put_f32_le(self.pitch);
        dst.put_u32_le(self.health);
        dst.put_u32_le(self.max_health);
        dst.put_u32_le(self.state_flags);
        dst.put_u64_le(self.timestamp);
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        // 8 + 12 + 12 + 4 + 4 + 4 + 4 + 4 + 8 = 60 bytes
        const SIZE: usize = 60;
        if src.remaining() < SIZE {
            return Err(ClientCodecError::BufferTooShort {
                expected: SIZE,
                got: src.remaining(),
            });
        }
        let entity_id = src.get_u64_le();
        let position = Vec3::decode(src)?;
        let velocity = Vec3::decode(src)?;
        let yaw = src.get_f32_le();
        let pitch = src.get_f32_le();
        let health = src.get_u32_le();
        let max_health = src.get_u32_le();
        let state_flags = src.get_u32_le();
        let timestamp = src.get_u64_le();

        Ok(Self {
            entity_id,
            position,
            velocity,
            yaw,
            pitch,
            health,
            max_health,
            state_flags,
            timestamp,
        })
    }
}

/// Server notification when an entity despawns or leaves Area of Interest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityDespawnPacket {
    pub entity_id: u64,
}

impl EntityDespawnPacket {
    pub fn encode(&self, dst: &mut BytesMut) {
        dst.put_u64_le(self.entity_id);
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        if src.remaining() < 8 {
            return Err(ClientCodecError::BufferTooShort {
                expected: 8,
                got: src.remaining(),
            });
        }
        Ok(Self {
            entity_id: src.get_u64_le(),
        })
    }
}

/// Client heartbeat ping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PingPacket {
    pub timestamp: u64,
}

impl PingPacket {
    pub fn encode(&self, dst: &mut BytesMut) {
        dst.put_u64_le(self.timestamp);
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        if src.remaining() < 8 {
            return Err(ClientCodecError::BufferTooShort {
                expected: 8,
                got: src.remaining(),
            });
        }
        Ok(Self {
            timestamp: src.get_u64_le(),
        })
    }
}

/// Server heartbeat pong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PongPacket {
    pub client_timestamp: u64,
    pub server_timestamp: u64,
}

impl PongPacket {
    pub fn encode(&self, dst: &mut BytesMut) {
        dst.put_u64_le(self.client_timestamp);
        dst.put_u64_le(self.server_timestamp);
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        if src.remaining() < 16 {
            return Err(ClientCodecError::BufferTooShort {
                expected: 16,
                got: src.remaining(),
            });
        }
        Ok(Self {
            client_timestamp: src.get_u64_le(),
            server_timestamp: src.get_u64_le(),
        })
    }
}

/// Top-level client packet enum encompassing all protocol messages.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientPacket {
    Movement(MovementPacket),
    MovementCorrection(MovementCorrectionPacket),
    Interaction(InteractionPacket),
    InteractionResult(InteractionResultPacket),
    EntityState(EntityStatePacket),
    EntityDespawn(EntityDespawnPacket),
    Ping(PingPacket),
    Pong(PongPacket),
}

impl ClientPacket {
    pub fn opcode(&self) -> Opcode {
        match self {
            ClientPacket::Movement(_) => Opcode::Movement,
            ClientPacket::MovementCorrection(_) => Opcode::MovementCorrection,
            ClientPacket::Interaction(_) => Opcode::Interaction,
            ClientPacket::InteractionResult(_) => Opcode::InteractionResult,
            ClientPacket::EntityState(_) => Opcode::EntityState,
            ClientPacket::EntityDespawn(_) => Opcode::EntityDespawn,
            ClientPacket::Ping(_) => Opcode::Ping,
            ClientPacket::Pong(_) => Opcode::Pong,
        }
    }

    /// Serializes the packet into a framed binary byte vector:
    /// `[MAGIC (2B) | OPCODE (2B) | PAYLOAD_LEN (4B) | PAYLOAD (NB)]`
    pub fn encode(&self) -> Vec<u8> {
        let mut payload_buf = BytesMut::new();
        match self {
            ClientPacket::Movement(p) => p.encode(&mut payload_buf),
            ClientPacket::MovementCorrection(p) => p.encode(&mut payload_buf),
            ClientPacket::Interaction(p) => p.encode(&mut payload_buf),
            ClientPacket::InteractionResult(p) => p.encode(&mut payload_buf),
            ClientPacket::EntityState(p) => p.encode(&mut payload_buf),
            ClientPacket::EntityDespawn(p) => p.encode(&mut payload_buf),
            ClientPacket::Ping(p) => p.encode(&mut payload_buf),
            ClientPacket::Pong(p) => p.encode(&mut payload_buf),
        }

        let payload_len = payload_buf.len() as u32;
        let mut frame = BytesMut::with_capacity(HEADER_SIZE + payload_buf.len());
        frame.put_u16_le(PACKET_MAGIC);
        frame.put_u16_le(self.opcode() as u16);
        frame.put_u32_le(payload_len);
        frame.extend_from_slice(&payload_buf);

        frame.to_vec()
    }

    /// Deserializes a single framed packet from an owned `Bytes` buffer without payload copying.
    pub fn decode_bytes(mut bytes: Bytes) -> Result<Self, ClientCodecError> {
        if bytes.len() < HEADER_SIZE {
            return Err(ClientCodecError::BufferTooShort {
                expected: HEADER_SIZE,
                got: bytes.len(),
            });
        }

        let magic = bytes.get_u16_le();
        if magic != PACKET_MAGIC {
            return Err(ClientCodecError::InvalidMagic {
                expected: PACKET_MAGIC,
                got: magic,
            });
        }

        let opcode_val = bytes.get_u16_le();
        let opcode = Opcode::from_u16(opcode_val)?;
        let payload_len = bytes.get_u32_le() as usize;

        if bytes.remaining() < payload_len {
            return Err(ClientCodecError::LengthMismatch {
                expected: payload_len,
                got: bytes.remaining(),
            });
        }

        let mut payload = bytes.split_to(payload_len);
        match opcode {
            Opcode::Movement => Ok(ClientPacket::Movement(MovementPacket::decode(
                &mut payload,
            )?)),
            Opcode::MovementCorrection => Ok(ClientPacket::MovementCorrection(
                MovementCorrectionPacket::decode(&mut payload)?,
            )),
            Opcode::Interaction => Ok(ClientPacket::Interaction(InteractionPacket::decode(
                &mut payload,
            )?)),
            Opcode::InteractionResult => Ok(ClientPacket::InteractionResult(
                InteractionResultPacket::decode(&mut payload)?,
            )),
            Opcode::EntityState => Ok(ClientPacket::EntityState(EntityStatePacket::decode(
                &mut payload,
            )?)),
            Opcode::EntityDespawn => Ok(ClientPacket::EntityDespawn(EntityDespawnPacket::decode(
                &mut payload,
            )?)),
            Opcode::Ping => Ok(ClientPacket::Ping(PingPacket::decode(&mut payload)?)),
            Opcode::Pong => Ok(ClientPacket::Pong(PongPacket::decode(&mut payload)?)),
        }
    }

    /// Deserializes a single framed packet from a slice.
    pub fn decode(bytes: &[u8]) -> Result<Self, ClientCodecError> {
        Self::decode_bytes(Bytes::copy_from_slice(bytes))
    }

    /// Streaming frame decoder for parsing incoming packets from a `BytesMut` buffer.
    ///
    /// Consumes the bytes if a full frame is present, or returns `Ok(None)` if
    /// more bytes are needed to complete the frame.
    pub fn decode_frame(buf: &mut BytesMut) -> Result<Option<Self>, ClientCodecError> {
        if buf.len() < HEADER_SIZE {
            return Ok(None);
        }

        let magic = u16::from_le_bytes([buf[0], buf[1]]);
        if magic != PACKET_MAGIC {
            return Err(ClientCodecError::InvalidMagic {
                expected: PACKET_MAGIC,
                got: magic,
            });
        }

        let payload_len = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
        let total_frame_size = HEADER_SIZE + payload_len;

        if buf.len() < total_frame_size {
            return Ok(None);
        }

        let frame_bytes = buf.split_to(total_frame_size).freeze();
        let packet = Self::decode_bytes(frame_bytes)?;
        Ok(Some(packet))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vec3_serialization() {
        let v = Vec3::new(12.5, -45.25, 100.0);
        let mut buf = BytesMut::new();
        v.encode(&mut buf);
        assert_eq!(buf.len(), 12);

        let decoded = Vec3::decode(&mut buf).unwrap();
        assert_eq!(v, decoded);
    }

    #[test]
    fn test_movement_packet_roundtrip() {
        let pkt = ClientPacket::Movement(MovementPacket {
            entity_id: 0x1234_5678_9ABC_DEF0,
            position: Vec3::new(10.0, 20.0, 30.0),
            velocity: Vec3::new(1.0, 0.0, -1.0),
            yaw: 1.57,
            pitch: 0.25,
            timestamp: 1700000000,
            sequence: 42,
        });

        let encoded = pkt.encode();
        let decoded = ClientPacket::decode(&encoded).expect("Failed to decode movement packet");
        assert_eq!(pkt, decoded);
    }

    #[test]
    fn test_movement_correction_packet_roundtrip() {
        let pkt = ClientPacket::MovementCorrection(MovementCorrectionPacket {
            entity_id: 1001,
            corrected_position: Vec3::new(0.0, 5.0, 0.0),
            corrected_velocity: Vec3::ZERO,
            sequence: 15,
            reason: 2,
        });

        let encoded = pkt.encode();
        let decoded = ClientPacket::decode(&encoded).expect("Failed to decode correction packet");
        assert_eq!(pkt, decoded);
    }

    #[test]
    fn test_interaction_packet_roundtrip() {
        let pkt = ClientPacket::Interaction(InteractionPacket {
            source_guid: 100,
            target_guid: 200,
            interaction_type: InteractionType::Attack,
            action_id: 99,
            payload: Bytes::from_static(&[1, 2, 3, 4, 5]),
        });

        let encoded = pkt.encode();
        let decoded = ClientPacket::decode(&encoded).expect("Failed to decode interaction packet");
        assert_eq!(pkt, decoded);
    }

    #[test]
    fn test_interaction_result_packet_roundtrip() {
        let pkt = ClientPacket::InteractionResult(InteractionResultPacket {
            action_id: 99,
            success: true,
            error_code: 0,
            payload: vec![10, 20, 30],
        });

        let encoded = pkt.encode();
        let decoded = ClientPacket::decode(&encoded).expect("Failed to decode result packet");
        assert_eq!(pkt, decoded);
    }

    #[test]
    fn test_entity_state_packet_roundtrip() {
        let pkt = ClientPacket::EntityState(EntityStatePacket {
            entity_id: 555,
            position: Vec3::new(1.0, 2.0, 3.0),
            velocity: Vec3::new(0.1, 0.2, 0.3),
            yaw: std::f32::consts::PI,
            pitch: -0.5,
            health: 100,
            max_health: 100,
            state_flags: 0b0001,
            timestamp: 99999,
        });

        let encoded = pkt.encode();
        let decoded = ClientPacket::decode(&encoded).expect("Failed to decode entity state packet");
        assert_eq!(pkt, decoded);
    }

    #[test]
    fn test_entity_despawn_packet_roundtrip() {
        let pkt = ClientPacket::EntityDespawn(EntityDespawnPacket { entity_id: 777 });
        let encoded = pkt.encode();
        let decoded = ClientPacket::decode(&encoded).expect("Failed to decode despawn packet");
        assert_eq!(pkt, decoded);
    }

    #[test]
    fn test_ping_pong_packet_roundtrip() {
        let ping = ClientPacket::Ping(PingPacket { timestamp: 12345 });
        let encoded_ping = ping.encode();
        let decoded_ping = ClientPacket::decode(&encoded_ping).unwrap();
        assert_eq!(ping, decoded_ping);

        let pong = ClientPacket::Pong(PongPacket {
            client_timestamp: 12345,
            server_timestamp: 12350,
        });
        let encoded_pong = pong.encode();
        let decoded_pong = ClientPacket::decode(&encoded_pong).unwrap();
        assert_eq!(pong, decoded_pong);
    }

    #[test]
    fn test_streaming_decode_frame() {
        let pkt1 = ClientPacket::Ping(PingPacket { timestamp: 111 });
        let pkt2 = ClientPacket::EntityDespawn(EntityDespawnPacket { entity_id: 222 });

        let mut stream_buffer = BytesMut::new();
        stream_buffer.extend_from_slice(&pkt1.encode());
        stream_buffer.extend_from_slice(&pkt2.encode());

        // Decode first packet
        let res1 = ClientPacket::decode_frame(&mut stream_buffer).unwrap();
        assert_eq!(res1, Some(pkt1));

        // Decode second packet
        let res2 = ClientPacket::decode_frame(&mut stream_buffer).unwrap();
        assert_eq!(res2, Some(pkt2));

        // No more complete frames
        let res3 = ClientPacket::decode_frame(&mut stream_buffer).unwrap();
        assert_eq!(res3, None);
        assert!(stream_buffer.is_empty());
    }

    #[test]
    fn test_partial_frame_streaming() {
        let pkt = ClientPacket::Ping(PingPacket { timestamp: 999 });
        let bytes = pkt.encode();

        let mut buf = BytesMut::new();
        // Feed partial header
        buf.extend_from_slice(&bytes[..4]);
        assert_eq!(ClientPacket::decode_frame(&mut buf).unwrap(), None);

        // Feed rest of header and part of payload
        buf.extend_from_slice(&bytes[4..10]);
        assert_eq!(ClientPacket::decode_frame(&mut buf).unwrap(), None);

        // Feed remaining bytes
        buf.extend_from_slice(&bytes[10..]);
        assert_eq!(ClientPacket::decode_frame(&mut buf).unwrap(), Some(pkt));
        assert!(buf.is_empty());
    }

    #[test]
    fn test_invalid_magic() {
        let mut buf = BytesMut::new();
        buf.put_u16_le(0x1234); // invalid magic
        buf.put_u16_le(Opcode::Ping as u16);
        buf.put_u32_le(8);
        buf.put_u64_le(100);

        let err = ClientPacket::decode(&buf).unwrap_err();
        match err {
            ClientCodecError::InvalidMagic { expected, got } => {
                assert_eq!(expected, PACKET_MAGIC);
                assert_eq!(got, 0x1234);
            }
            _ => panic!("Expected InvalidMagic error"),
        }
    }

    #[test]
    fn test_unknown_opcode() {
        let mut buf = BytesMut::new();
        buf.put_u16_le(PACKET_MAGIC);
        buf.put_u16_le(0x9999); // unknown opcode
        buf.put_u32_le(0);

        let err = ClientPacket::decode(&buf).unwrap_err();
        match err {
            ClientCodecError::UnknownOpcode(0x9999) => {}
            _ => panic!("Expected UnknownOpcode error"),
        }
    }
}
