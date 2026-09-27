//! Client-facing protocol packet definitions and binary codecs.
//!
//! Provides compact binary framing and serialization for high-frequency
//! client-gateway and gateway-node communications (movement, entity state sync,
//! and interaction actions).

use bytes::{Buf, BufMut, Bytes, BytesMut};
use thiserror::Error;

use crate::bitpack::{get_varint_u32, get_varint_u64, put_varint_u32, put_varint_u64};
use crate::quantize::{
    dequantize_delta_i16, dequantize_pitch_i8, dequantize_velocity_i16, dequantize_yaw_u8,
    quantize_delta_i16, quantize_pitch_i8, quantize_velocity_i16, quantize_yaw_u8,
};

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
    EntityDeltaBatch = 0x0009,
    ClientAck = 0x000A,
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
            0x0009 => Ok(Opcode::EntityDeltaBatch),
            0x000A => Ok(Opcode::ClientAck),
            other => Err(ClientCodecError::UnknownOpcode(other)),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Opcode::Movement => "Movement",
            Opcode::MovementCorrection => "MovementCorrection",
            Opcode::Interaction => "Interaction",
            Opcode::InteractionResult => "InteractionResult",
            Opcode::EntityState => "EntityState",
            Opcode::EntityDespawn => "EntityDespawn",
            Opcode::Ping => "Ping",
            Opcode::Pong => "Pong",
            Opcode::EntityDeltaBatch => "EntityDeltaBatch",
            Opcode::ClientAck => "ClientAck",
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

/// Field mask flags indicating which entity attributes are present in an `EntityDeltaState`.
pub mod delta_flags {
    /// Full keyframe: absolute position coordinates are sent as 3x f32.
    pub const MASK_KEYFRAME: u16 = 1 << 0;
    /// Quantized position delta X present (i16).
    pub const MASK_DELTA_POS_X: u16 = 1 << 1;
    /// Quantized position delta Y present (i16).
    pub const MASK_DELTA_POS_Y: u16 = 1 << 2;
    /// Quantized position delta Z present (i16).
    pub const MASK_DELTA_POS_Z: u16 = 1 << 3;
    /// Quantized 3D velocity present (3x i16).
    pub const MASK_VELOCITY: u16 = 1 << 4;
    /// Quantized 8-bit yaw present (u8).
    pub const MASK_YAW: u16 = 1 << 5;
    /// Quantized 8-bit pitch present (i8).
    pub const MASK_PITCH: u16 = 1 << 6;
    /// Varint current health present.
    pub const MASK_HEALTH: u16 = 1 << 7;
    /// Varint max health present.
    pub const MASK_MAX_HEALTH: u16 = 1 << 8;
    /// Varint state flags present.
    pub const MASK_STATE_FLAGS: u16 = 1 << 9;
    /// Varint action/animation ID present.
    pub const MASK_ACTION_ID: u16 = 1 << 10;
}

/// A single entity's compressed delta update within an `EntityDeltaBatchPacket`.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityDeltaState {
    pub entity_id: u64,
    pub field_mask: u16,
    pub position: (f32, f32, f32),
    pub velocity: (f32, f32, f32),
    pub yaw: f32,
    pub pitch: f32,
    pub health: u32,
    pub max_health: u32,
    pub state_flags: u32,
    pub action_id: u32,
}

impl Default for EntityDeltaState {
    fn default() -> Self {
        Self {
            entity_id: 0,
            field_mask: 0,
            position: (0.0, 0.0, 0.0),
            velocity: (0.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            health: 0,
            max_health: 0,
            state_flags: 0,
            action_id: 0,
        }
    }
}

impl EntityDeltaState {
    pub fn encode(&self, dst: &mut BytesMut) {
        put_varint_u64(dst, self.entity_id);
        dst.put_u16_le(self.field_mask);

        if self.field_mask & delta_flags::MASK_KEYFRAME != 0 {
            dst.put_f32_le(self.position.0);
            dst.put_f32_le(self.position.1);
            dst.put_f32_le(self.position.2);
        } else {
            if self.field_mask & delta_flags::MASK_DELTA_POS_X != 0 {
                dst.put_i16_le(quantize_delta_i16(self.position.0));
            }
            if self.field_mask & delta_flags::MASK_DELTA_POS_Y != 0 {
                dst.put_i16_le(quantize_delta_i16(self.position.1));
            }
            if self.field_mask & delta_flags::MASK_DELTA_POS_Z != 0 {
                dst.put_i16_le(quantize_delta_i16(self.position.2));
            }
        }

        if self.field_mask & delta_flags::MASK_VELOCITY != 0 {
            dst.put_i16_le(quantize_velocity_i16(self.velocity.0));
            dst.put_i16_le(quantize_velocity_i16(self.velocity.1));
            dst.put_i16_le(quantize_velocity_i16(self.velocity.2));
        }

        if self.field_mask & delta_flags::MASK_YAW != 0 {
            dst.put_u8(quantize_yaw_u8(self.yaw));
        }

        if self.field_mask & delta_flags::MASK_PITCH != 0 {
            dst.put_i8(quantize_pitch_i8(self.pitch));
        }

        if self.field_mask & delta_flags::MASK_HEALTH != 0 {
            put_varint_u32(dst, self.health);
        }

        if self.field_mask & delta_flags::MASK_MAX_HEALTH != 0 {
            put_varint_u32(dst, self.max_health);
        }

        if self.field_mask & delta_flags::MASK_STATE_FLAGS != 0 {
            put_varint_u32(dst, self.state_flags);
        }

        if self.field_mask & delta_flags::MASK_ACTION_ID != 0 {
            put_varint_u32(dst, self.action_id);
        }
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        let entity_id = get_varint_u64(src)?;
        if src.remaining() < 2 {
            return Err(ClientCodecError::BufferTooShort {
                expected: 2,
                got: src.remaining(),
            });
        }
        let field_mask = src.get_u16_le();

        let mut position = (0.0, 0.0, 0.0);
        if field_mask & delta_flags::MASK_KEYFRAME != 0 {
            if src.remaining() < 12 {
                return Err(ClientCodecError::BufferTooShort {
                    expected: 12,
                    got: src.remaining(),
                });
            }
            position = (src.get_f32_le(), src.get_f32_le(), src.get_f32_le());
        } else {
            if field_mask & delta_flags::MASK_DELTA_POS_X != 0 {
                if src.remaining() < 2 {
                    return Err(ClientCodecError::BufferTooShort {
                        expected: 2,
                        got: src.remaining(),
                    });
                }
                position.0 = dequantize_delta_i16(src.get_i16_le());
            }
            if field_mask & delta_flags::MASK_DELTA_POS_Y != 0 {
                if src.remaining() < 2 {
                    return Err(ClientCodecError::BufferTooShort {
                        expected: 2,
                        got: src.remaining(),
                    });
                }
                position.1 = dequantize_delta_i16(src.get_i16_le());
            }
            if field_mask & delta_flags::MASK_DELTA_POS_Z != 0 {
                if src.remaining() < 2 {
                    return Err(ClientCodecError::BufferTooShort {
                        expected: 2,
                        got: src.remaining(),
                    });
                }
                position.2 = dequantize_delta_i16(src.get_i16_le());
            }
        }

        let mut velocity = (0.0, 0.0, 0.0);
        if field_mask & delta_flags::MASK_VELOCITY != 0 {
            if src.remaining() < 6 {
                return Err(ClientCodecError::BufferTooShort {
                    expected: 6,
                    got: src.remaining(),
                });
            }
            velocity = (
                dequantize_velocity_i16(src.get_i16_le()),
                dequantize_velocity_i16(src.get_i16_le()),
                dequantize_velocity_i16(src.get_i16_le()),
            );
        }

        let mut yaw = 0.0;
        if field_mask & delta_flags::MASK_YAW != 0 {
            if !src.has_remaining() {
                return Err(ClientCodecError::BufferTooShort {
                    expected: 1,
                    got: 0,
                });
            }
            yaw = dequantize_yaw_u8(src.get_u8());
        }

        let mut pitch = 0.0;
        if field_mask & delta_flags::MASK_PITCH != 0 {
            if !src.has_remaining() {
                return Err(ClientCodecError::BufferTooShort {
                    expected: 1,
                    got: 0,
                });
            }
            pitch = dequantize_pitch_i8(src.get_i8());
        }

        let mut health = 0;
        if field_mask & delta_flags::MASK_HEALTH != 0 {
            health = get_varint_u32(src)?;
        }

        let mut max_health = 0;
        if field_mask & delta_flags::MASK_MAX_HEALTH != 0 {
            max_health = get_varint_u32(src)?;
        }

        let mut state_flags = 0;
        if field_mask & delta_flags::MASK_STATE_FLAGS != 0 {
            state_flags = get_varint_u32(src)?;
        }

        let mut action_id = 0;
        if field_mask & delta_flags::MASK_ACTION_ID != 0 {
            action_id = get_varint_u32(src)?;
        }

        Ok(Self {
            entity_id,
            field_mask,
            position,
            velocity,
            yaw,
            pitch,
            health,
            max_health,
            state_flags,
            action_id,
        })
    }
}

/// Server broadcast containing batched entity delta states.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityDeltaBatchPacket {
    pub server_tick: u64,
    pub baseline_tick: u64,
    pub despawns: Vec<u64>,
    pub deltas: Vec<EntityDeltaState>,
}

impl EntityDeltaBatchPacket {
    pub fn encode(&self, dst: &mut BytesMut) {
        put_varint_u64(dst, self.server_tick);
        put_varint_u64(dst, self.baseline_tick);
        put_varint_u32(dst, self.despawns.len() as u32);
        for &d in &self.despawns {
            put_varint_u64(dst, d);
        }
        put_varint_u32(dst, self.deltas.len() as u32);
        for delta in &self.deltas {
            delta.encode(dst);
        }
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        let server_tick = get_varint_u64(src)?;
        let baseline_tick = get_varint_u64(src)?;
        let despawn_len = get_varint_u32(src)? as usize;
        let mut despawns = Vec::with_capacity(despawn_len.min(1024));
        for _ in 0..despawn_len {
            despawns.push(get_varint_u64(src)?);
        }
        let delta_len = get_varint_u32(src)? as usize;
        let mut deltas = Vec::with_capacity(delta_len.min(1024));
        for _ in 0..delta_len {
            deltas.push(EntityDeltaState::decode(src)?);
        }
        Ok(Self {
            server_tick,
            baseline_tick,
            despawns,
            deltas,
        })
    }
}

/// Client acknowledgement carrying the latest observed server tick and a 32-tick loss recovery mask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientAckPacket {
    pub session_id: u64,
    pub observed_tick: u64,
    pub ack_mask: u32,
}

impl ClientAckPacket {
    pub fn encode(&self, dst: &mut BytesMut) {
        put_varint_u64(dst, self.session_id);
        put_varint_u64(dst, self.observed_tick);
        dst.put_u32_le(self.ack_mask);
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        let session_id = get_varint_u64(src)?;
        let observed_tick = get_varint_u64(src)?;
        if src.remaining() < 4 {
            return Err(ClientCodecError::BufferTooShort {
                expected: 4,
                got: src.remaining(),
            });
        }
        let ack_mask = src.get_u32_le();
        Ok(Self {
            session_id,
            observed_tick,
            ack_mask,
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
    EntityDeltaBatch(EntityDeltaBatchPacket),
    ClientAck(ClientAckPacket),
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
            ClientPacket::EntityDeltaBatch(_) => Opcode::EntityDeltaBatch,
            ClientPacket::ClientAck(_) => Opcode::ClientAck,
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
            ClientPacket::EntityDeltaBatch(p) => p.encode(&mut payload_buf),
            ClientPacket::ClientAck(p) => p.encode(&mut payload_buf),
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
            Opcode::EntityDeltaBatch => Ok(ClientPacket::EntityDeltaBatch(
                EntityDeltaBatchPacket::decode(&mut payload)?,
            )),
            Opcode::ClientAck => Ok(ClientPacket::ClientAck(ClientAckPacket::decode(
                &mut payload,
            )?)),
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
    fn test_client_ack_packet_roundtrip() {
        let ack = ClientPacket::ClientAck(ClientAckPacket {
            session_id: 12345,
            observed_tick: 500,
            ack_mask: 0b1111_0000_1010_0101,
        });
        let encoded = ack.encode();
        let decoded = ClientPacket::decode(&encoded).expect("Failed to decode ClientAck");
        assert_eq!(ack, decoded);
    }

    #[test]
    fn test_entity_delta_batch_packet_roundtrip() {
        let batch = ClientPacket::EntityDeltaBatch(EntityDeltaBatchPacket {
            server_tick: 100,
            baseline_tick: 95,
            despawns: vec![10, 20, 30],
            deltas: vec![
                EntityDeltaState {
                    entity_id: 101,
                    field_mask: delta_flags::MASK_KEYFRAME
                        | delta_flags::MASK_VELOCITY
                        | delta_flags::MASK_YAW,
                    position: (100.5, 20.25, -50.75),
                    velocity: (1.2, 0.0, -0.5),
                    yaw: std::f32::consts::PI * 0.5,
                    pitch: 0.0,
                    health: 100,
                    max_health: 100,
                    state_flags: 0,
                    action_id: 0,
                },
                EntityDeltaState {
                    entity_id: 102,
                    field_mask: delta_flags::MASK_DELTA_POS_X
                        | delta_flags::MASK_DELTA_POS_Z
                        | delta_flags::MASK_YAW
                        | delta_flags::MASK_HEALTH,
                    position: (0.125, 0.0, -0.25),
                    velocity: (0.0, 0.0, 0.0),
                    yaw: std::f32::consts::PI * 1.5,
                    pitch: 0.0,
                    health: 85,
                    max_health: 100,
                    state_flags: 0,
                    action_id: 0,
                },
            ],
        });

        let encoded = batch.encode();
        let decoded = ClientPacket::decode(&encoded).expect("Failed to decode EntityDeltaBatch");

        match (&batch, &decoded) {
            (
                ClientPacket::EntityDeltaBatch(orig_batch),
                ClientPacket::EntityDeltaBatch(dec_batch),
            ) => {
                assert_eq!(orig_batch.server_tick, dec_batch.server_tick);
                assert_eq!(orig_batch.baseline_tick, dec_batch.baseline_tick);
                assert_eq!(orig_batch.despawns, dec_batch.despawns);
                assert_eq!(orig_batch.deltas.len(), dec_batch.deltas.len());

                // First entity: keyframe
                assert_eq!(dec_batch.deltas[0].entity_id, 101);
                assert_eq!(dec_batch.deltas[0].position, (100.5, 20.25, -50.75));

                // Second entity: delta
                assert_eq!(dec_batch.deltas[1].entity_id, 102);
                assert!((dec_batch.deltas[1].position.0 - 0.125).abs() < 0.008);
                assert!((dec_batch.deltas[1].position.2 - (-0.25)).abs() < 0.008);
                assert_eq!(dec_batch.deltas[1].health, 85);
            }
            _ => panic!("Expected EntityDeltaBatch"),
        }
    }
}
