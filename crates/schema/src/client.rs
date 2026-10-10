//! Client-facing protocol packet definitions and binary codecs.
//!
//! Provides compact binary framing and serialization for high-frequency
//! client-gateway and gateway-node communications (movement, entity state sync,
//! and interaction actions).

use bytes::{Buf, BufMut, Bytes, BytesMut};
use std::collections::HashMap;
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
    WorldObjectSpawn = 0x000B,
    WorldObjectDelta = 0x000C,
    WorldObjectDespawn = 0x000D,
    WorldObjectResync = 0x000E,
    PlaceObject = 0x000F,
    InteractObject = 0x0010,
    AuthRequest = 0x0011,
    AuthResponse = 0x0012,
    SocialMessage = 0x0013,
    SocialSubscribe = 0x0014,
    SocialUnsubscribe = 0x0015,
    MarketListRequest = 0x0016,
    MarketBuyRequest = 0x0017,
    MarketCancelRequest = 0x0018,
    MarketQueryRequest = 0x0019,
    MarketResponse = 0x001A,
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
            0x000B => Ok(Opcode::WorldObjectSpawn),
            0x000C => Ok(Opcode::WorldObjectDelta),
            0x000D => Ok(Opcode::WorldObjectDespawn),
            0x000E => Ok(Opcode::WorldObjectResync),
            0x000F => Ok(Opcode::PlaceObject),
            0x0010 => Ok(Opcode::InteractObject),
            0x0011 => Ok(Opcode::AuthRequest),
            0x0012 => Ok(Opcode::AuthResponse),
            0x0013 => Ok(Opcode::SocialMessage),
            0x0014 => Ok(Opcode::SocialSubscribe),
            0x0015 => Ok(Opcode::SocialUnsubscribe),
            0x0016 => Ok(Opcode::MarketListRequest),
            0x0017 => Ok(Opcode::MarketBuyRequest),
            0x0018 => Ok(Opcode::MarketCancelRequest),
            0x0019 => Ok(Opcode::MarketQueryRequest),
            0x001A => Ok(Opcode::MarketResponse),
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
            Opcode::WorldObjectSpawn => "WorldObjectSpawn",
            Opcode::WorldObjectDelta => "WorldObjectDelta",
            Opcode::WorldObjectDespawn => "WorldObjectDespawn",
            Opcode::WorldObjectResync => "WorldObjectResync",
            Opcode::PlaceObject => "PlaceObject",
            Opcode::InteractObject => "InteractObject",
            Opcode::AuthRequest => "AuthRequest",
            Opcode::AuthResponse => "AuthResponse",
            Opcode::SocialMessage => "SocialMessage",
            Opcode::SocialSubscribe => "SocialSubscribe",
            Opcode::SocialUnsubscribe => "SocialUnsubscribe",
            Opcode::MarketListRequest => "MarketListRequest",
            Opcode::MarketBuyRequest => "MarketBuyRequest",
            Opcode::MarketCancelRequest => "MarketCancelRequest",
            Opcode::MarketQueryRequest => "MarketQueryRequest",
            Opcode::MarketResponse => "MarketResponse",
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

    pub fn is_finite(&self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    pub fn length_squared(&self) -> f32 {
        self.x * self.x + self.y * self.y + self.z * self.z
    }

    pub fn length(&self) -> f32 {
        self.length_squared().sqrt()
    }

    pub fn normalized(&self) -> Option<Self> {
        let len = self.length();
        if len > 1e-6 {
            Some(Self {
                x: self.x / len,
                y: self.y / len,
                z: self.z / len,
            })
        } else {
            None
        }
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

/// Specific reason category for a server-originated movement correction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum MovementCorrectionReason {
    KinematicViolation = 0,
    Reconciliation = 1,
    Veto = 2,
    OutOfBounds = 3,
    Custom(u8),
}

impl From<u8> for MovementCorrectionReason {
    fn from(val: u8) -> Self {
        match val {
            0 => MovementCorrectionReason::KinematicViolation,
            1 => MovementCorrectionReason::Reconciliation,
            2 => MovementCorrectionReason::Veto,
            3 => MovementCorrectionReason::OutOfBounds,
            custom => MovementCorrectionReason::Custom(custom),
        }
    }
}

impl From<MovementCorrectionReason> for u8 {
    fn from(r: MovementCorrectionReason) -> Self {
        match r {
            MovementCorrectionReason::KinematicViolation => 0,
            MovementCorrectionReason::Reconciliation => 1,
            MovementCorrectionReason::Veto => 2,
            MovementCorrectionReason::OutOfBounds => 3,
            MovementCorrectionReason::Custom(val) => val,
        }
    }
}

/// Server-originated movement snap-back correction (anti-cheat / kinematic violation / reconciliation).
#[derive(Debug, Clone, PartialEq)]
pub struct MovementCorrectionPacket {
    pub entity_id: u64,
    pub corrected_position: Vec3,
    pub corrected_velocity: Vec3,
    pub sequence: u32,
    pub reason: u8,
}

impl MovementCorrectionPacket {
    pub fn reason_enum(&self) -> MovementCorrectionReason {
        MovementCorrectionReason::from(self.reason)
    }

    pub fn with_reason(mut self, reason: MovementCorrectionReason) -> Self {
        self.reason = reason.into();
        self
    }

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

/// Client-originated lag-compensated interaction/raycast payload embedded in `InteractionPacket.payload`.
#[derive(Debug, Clone, PartialEq)]
pub struct LagCompensationPayload {
    /// Server-synchronized target timestamp (adjusted via client ping/clock offset estimation, in milliseconds).
    pub client_timestamp: u64,
    /// Origin of the interaction ray in 3D world space.
    pub origin: Vec3,
    /// Direction vector of the interaction ray (normalized on query execution).
    pub direction: Vec3,
    /// Maximum raycast range in world units.
    pub max_distance: f32,
    /// Client-side input sequence number at query invocation.
    pub sequence: u32,
}

impl LagCompensationPayload {
    /// Fixed binary serialization size in bytes (8 + 12 + 12 + 4 + 4 = 40 bytes).
    pub const SIZE: usize = 40;

    pub fn encode(&self, dst: &mut BytesMut) {
        dst.put_u64_le(self.client_timestamp);
        self.origin.encode(dst);
        self.direction.encode(dst);
        dst.put_f32_le(self.max_distance);
        dst.put_u32_le(self.sequence);
    }

    pub fn encode_to_bytes(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(Self::SIZE);
        self.encode(&mut buf);
        buf.freeze()
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        if src.remaining() < Self::SIZE {
            return Err(ClientCodecError::BufferTooShort {
                expected: Self::SIZE,
                got: src.remaining(),
            });
        }
        let client_timestamp = src.get_u64_le();
        let origin = Vec3::decode(src)?;
        let direction = Vec3::decode(src)?;
        let max_distance = src.get_f32_le();
        let sequence = src.get_u32_le();

        if !origin.is_finite() || !direction.is_finite() {
            return Err(ClientCodecError::InvalidData(
                "Origin and direction must contain finite floats".to_string(),
            ));
        }

        if !max_distance.is_finite() || max_distance <= 0.0 {
            return Err(ClientCodecError::InvalidData(
                "max_distance must be finite and positive".to_string(),
            ));
        }

        if direction.length() < 1e-6 {
            return Err(ClientCodecError::InvalidData(
                "Direction vector is degenerate (near zero)".to_string(),
            ));
        }

        Ok(Self {
            client_timestamp,
            origin,
            direction,
            max_distance,
            sequence,
        })
    }

    pub fn normalized_direction(&self) -> Option<Vec3> {
        self.direction.normalized()
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
    pub const MASK_KEYFRAME: u16 = 1;
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
    /// Generic scalar attributes present (varint count followed by attribute ID and value pairs).
    pub const MASK_ATTRIBUTES: u16 = 1 << 11;
}

/// Unopinionated generic scalar attribute delta identified by numeric attribute ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScalarAttributeDelta {
    pub attribute_id: u16,
    pub value: u32,
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
    pub attributes: Vec<ScalarAttributeDelta>,
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
            attributes: Vec::new(),
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

        if self.field_mask & delta_flags::MASK_ATTRIBUTES != 0 {
            put_varint_u32(dst, self.attributes.len() as u32);
            for attr in &self.attributes {
                put_varint_u32(dst, attr.attribute_id as u32);
                put_varint_u32(dst, attr.value);
            }
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

        let mut attributes = Vec::new();
        if field_mask & delta_flags::MASK_ATTRIBUTES != 0 {
            let attr_len = get_varint_u32(src)? as usize;
            if attr_len > 256 {
                return Err(ClientCodecError::InvalidData(format!(
                    "Too many scalar attributes in delta: {}",
                    attr_len
                )));
            }
            attributes.reserve_exact(attr_len);
            for _ in 0..attr_len {
                let attr_id = get_varint_u32(src)?;
                if attr_id > u16::MAX as u32 {
                    return Err(ClientCodecError::InvalidData(
                        "Attribute ID exceeds u16::MAX".to_string(),
                    ));
                }
                let val = get_varint_u32(src)?;
                attributes.push(ScalarAttributeDelta {
                    attribute_id: attr_id as u16,
                    value: val,
                });
            }
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
            attributes,
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

fn put_string(dst: &mut BytesMut, s: &str) {
    put_varint_u32(dst, s.len() as u32);
    dst.extend_from_slice(s.as_bytes());
}

fn get_string(src: &mut impl Buf) -> Result<String, ClientCodecError> {
    let len = get_varint_u32(src)? as usize;
    if src.remaining() < len {
        return Err(ClientCodecError::BufferTooShort {
            expected: len,
            got: src.remaining(),
        });
    }
    let mut bytes = vec![0u8; len];
    src.copy_to_slice(&mut bytes);
    String::from_utf8(bytes).map_err(|e| ClientCodecError::InvalidData(e.to_string()))
}

fn put_property_map(dst: &mut BytesMut, map: &HashMap<String, String>) {
    put_varint_u32(dst, map.len() as u32);
    for (k, v) in map {
        put_string(dst, k);
        put_string(dst, v);
    }
}

fn get_property_map(src: &mut impl Buf) -> Result<HashMap<String, String>, ClientCodecError> {
    let count = get_varint_u32(src)? as usize;
    let mut map = HashMap::with_capacity(count.min(1024));
    for _ in 0..count {
        let k = get_string(src)?;
        let v = get_string(src)?;
        map.insert(k, v);
    }
    Ok(map)
}

/// Server broadcast replicating a spawned world object or constructed prefab.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldObjectSpawnPacket {
    pub object_id: u64,
    pub prefab_id: u32,
    pub pos: Vec3,
    pub rot: (f32, f32, f32, f32),
    pub scale: Vec3,
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    pub state_flags: u32,
    pub owner_type: u8,
    pub owner_id: String,
    pub health: Option<(u32, u32)>,
    pub properties: HashMap<String, String>,
    pub revision: u64,
}

impl WorldObjectSpawnPacket {
    pub fn encode(&self, dst: &mut BytesMut) {
        put_varint_u64(dst, self.object_id);
        put_varint_u32(dst, self.prefab_id);
        dst.put_f32_le(self.pos.x);
        dst.put_f32_le(self.pos.y);
        dst.put_f32_le(self.pos.z);
        dst.put_f32_le(self.rot.0);
        dst.put_f32_le(self.rot.1);
        dst.put_f32_le(self.rot.2);
        dst.put_f32_le(self.rot.3);
        dst.put_f32_le(self.scale.x);
        dst.put_f32_le(self.scale.y);
        dst.put_f32_le(self.scale.z);
        dst.put_f32_le(self.bounds_min.x);
        dst.put_f32_le(self.bounds_min.y);
        dst.put_f32_le(self.bounds_min.z);
        dst.put_f32_le(self.bounds_max.x);
        dst.put_f32_le(self.bounds_max.y);
        dst.put_f32_le(self.bounds_max.z);
        put_varint_u32(dst, self.state_flags);
        dst.put_u8(self.owner_type);
        put_string(dst, &self.owner_id);
        if let Some((cur, max)) = self.health {
            dst.put_u8(1);
            put_varint_u32(dst, cur);
            put_varint_u32(dst, max);
        } else {
            dst.put_u8(0);
        }
        put_property_map(dst, &self.properties);
        put_varint_u64(dst, self.revision);
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        let object_id = get_varint_u64(src)?;
        let prefab_id = get_varint_u32(src)?;
        if src.remaining() < 16 * 4 {
            return Err(ClientCodecError::BufferTooShort {
                expected: 16 * 4,
                got: src.remaining(),
            });
        }
        let pos = Vec3 {
            x: src.get_f32_le(),
            y: src.get_f32_le(),
            z: src.get_f32_le(),
        };
        let rot = (
            src.get_f32_le(),
            src.get_f32_le(),
            src.get_f32_le(),
            src.get_f32_le(),
        );
        let scale = Vec3 {
            x: src.get_f32_le(),
            y: src.get_f32_le(),
            z: src.get_f32_le(),
        };
        let bounds_min = Vec3 {
            x: src.get_f32_le(),
            y: src.get_f32_le(),
            z: src.get_f32_le(),
        };
        let bounds_max = Vec3 {
            x: src.get_f32_le(),
            y: src.get_f32_le(),
            z: src.get_f32_le(),
        };
        let state_flags = get_varint_u32(src)?;
        if src.remaining() < 1 {
            return Err(ClientCodecError::BufferTooShort {
                expected: 1,
                got: src.remaining(),
            });
        }
        let owner_type = src.get_u8();
        let owner_id = get_string(src)?;
        if src.remaining() < 1 {
            return Err(ClientCodecError::BufferTooShort {
                expected: 1,
                got: src.remaining(),
            });
        }
        let has_health = src.get_u8();
        let health = if has_health != 0 {
            let cur = get_varint_u32(src)?;
            let max = get_varint_u32(src)?;
            Some((cur, max))
        } else {
            None
        };
        let properties = get_property_map(src)?;
        let revision = get_varint_u64(src)?;

        Ok(Self {
            object_id,
            prefab_id,
            pos,
            rot,
            scale,
            bounds_min,
            bounds_max,
            state_flags,
            owner_type,
            owner_id,
            health,
            properties,
            revision,
        })
    }
}

/// Server broadcast communicating delta state changes for a world object.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldObjectDeltaPacket {
    pub object_id: u64,
    pub revision: u64,
    pub state_flags: Option<u32>,
    pub health: Option<(u32, u32)>,
    pub properties: HashMap<String, String>,
}

impl WorldObjectDeltaPacket {
    pub fn encode(&self, dst: &mut BytesMut) {
        put_varint_u64(dst, self.object_id);
        put_varint_u64(dst, self.revision);
        if let Some(flags) = self.state_flags {
            dst.put_u8(1);
            put_varint_u32(dst, flags);
        } else {
            dst.put_u8(0);
        }
        if let Some((cur, max)) = self.health {
            dst.put_u8(1);
            put_varint_u32(dst, cur);
            put_varint_u32(dst, max);
        } else {
            dst.put_u8(0);
        }
        put_property_map(dst, &self.properties);
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        let object_id = get_varint_u64(src)?;
        let revision = get_varint_u64(src)?;
        if src.remaining() < 1 {
            return Err(ClientCodecError::BufferTooShort {
                expected: 1,
                got: src.remaining(),
            });
        }
        let has_flags = src.get_u8();
        let state_flags = if has_flags != 0 {
            Some(get_varint_u32(src)?)
        } else {
            None
        };
        if src.remaining() < 1 {
            return Err(ClientCodecError::BufferTooShort {
                expected: 1,
                got: src.remaining(),
            });
        }
        let has_health = src.get_u8();
        let health = if has_health != 0 {
            let cur = get_varint_u32(src)?;
            let max = get_varint_u32(src)?;
            Some((cur, max))
        } else {
            None
        };
        let properties = get_property_map(src)?;

        Ok(Self {
            object_id,
            revision,
            state_flags,
            health,
            properties,
        })
    }
}

/// Server broadcast communicating despawn or destruction of a world object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorldObjectDespawnPacket {
    pub object_id: u64,
}

impl WorldObjectDespawnPacket {
    pub fn encode(&self, dst: &mut BytesMut) {
        put_varint_u64(dst, self.object_id);
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        let object_id = get_varint_u64(src)?;
        Ok(Self { object_id })
    }
}

/// Client request to fetch the complete current state of a world object after detected revision gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorldObjectResyncPacket {
    pub object_id: u64,
}

impl WorldObjectResyncPacket {
    pub fn encode(&self, dst: &mut BytesMut) {
        put_varint_u64(dst, self.object_id);
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        let object_id = get_varint_u64(src)?;
        Ok(Self { object_id })
    }
}

/// Client request to authoritatively place a world object or constructible prefab.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaceObjectPacket {
    pub prefab_id: u32,
    pub pos: Vec3,
    pub rot: (f32, f32, f32, f32),
    pub scale: Vec3,
    pub properties: HashMap<String, String>,
}

impl PlaceObjectPacket {
    pub fn encode(&self, dst: &mut BytesMut) {
        put_varint_u32(dst, self.prefab_id);
        dst.put_f32_le(self.pos.x);
        dst.put_f32_le(self.pos.y);
        dst.put_f32_le(self.pos.z);
        dst.put_f32_le(self.rot.0);
        dst.put_f32_le(self.rot.1);
        dst.put_f32_le(self.rot.2);
        dst.put_f32_le(self.rot.3);
        dst.put_f32_le(self.scale.x);
        dst.put_f32_le(self.scale.y);
        dst.put_f32_le(self.scale.z);
        put_property_map(dst, &self.properties);
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        let prefab_id = get_varint_u32(src)?;
        if src.remaining() < 10 * 4 {
            return Err(ClientCodecError::BufferTooShort {
                expected: 10 * 4,
                got: src.remaining(),
            });
        }
        let pos = Vec3 {
            x: src.get_f32_le(),
            y: src.get_f32_le(),
            z: src.get_f32_le(),
        };
        let rot = (
            src.get_f32_le(),
            src.get_f32_le(),
            src.get_f32_le(),
            src.get_f32_le(),
        );
        let scale = Vec3 {
            x: src.get_f32_le(),
            y: src.get_f32_le(),
            z: src.get_f32_le(),
        };
        let properties = get_property_map(src)?;

        Ok(Self {
            prefab_id,
            pos,
            rot,
            scale,
            properties,
        })
    }
}

/// Client request to interact with an active world object (e.g. open door, mine node, loot chest).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InteractObjectPacket {
    pub object_id: u64,
    pub action_type: u32,
}

impl InteractObjectPacket {
    pub fn encode(&self, dst: &mut BytesMut) {
        put_varint_u64(dst, self.object_id);
        put_varint_u32(dst, self.action_type);
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        let object_id = get_varint_u64(src)?;
        let action_type = get_varint_u32(src)?;
        Ok(Self {
            object_id,
            action_type,
        })
    }
}

/// Standard authentication error codes returned in `AuthResponsePacket`.
pub mod auth_error_codes {
    pub const SUCCESS: u32 = 0;
    pub const INVALID_TOKEN: u32 = 1;
    pub const EXPIRED_TOKEN: u32 = 2;
    pub const INVALID_SIGNATURE: u32 = 3;
    pub const INVALID_CLAIMS: u32 = 4;
    pub const VERSION_MISMATCH: u32 = 5;
    pub const TIMEOUT: u32 = 6;
    pub const RATE_LIMITED: u32 = 7;
    pub const CLUSTER_UNAVAILABLE: u32 = 8;
    pub const IDENTITY_MISMATCH: u32 = 9;
}

/// Client-originated authentication request containing a cryptographic token (e.g. JWT or HandoverTicket).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthRequestPacket {
    pub token: String,
    pub client_version: u32,
}

impl AuthRequestPacket {
    pub fn encode(&self, dst: &mut BytesMut) {
        let token_bytes = self.token.as_bytes();
        dst.put_u16_le(token_bytes.len() as u16);
        dst.put_slice(token_bytes);
        dst.put_u32_le(self.client_version);
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        if src.remaining() < 2 {
            return Err(ClientCodecError::BufferTooShort {
                expected: 2,
                got: src.remaining(),
            });
        }
        let token_len = src.get_u16_le() as usize;
        if src.remaining() < token_len + 4 {
            return Err(ClientCodecError::BufferTooShort {
                expected: token_len + 4,
                got: src.remaining(),
            });
        }
        let token_bytes = src.copy_to_bytes(token_len);
        let token = String::from_utf8(token_bytes.to_vec())
            .map_err(|e| ClientCodecError::InvalidData(e.to_string()))?;
        let client_version = src.get_u32_le();
        Ok(Self {
            token,
            client_version,
        })
    }
}

/// Server-originated authentication response granting or denying session admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthResponsePacket {
    pub success: bool,
    pub error_code: u32,
    pub session_id: u64,
    pub player_guid: u64,
    pub assigned_node: String,
    pub error_message: String,
}

impl AuthResponsePacket {
    pub fn encode(&self, dst: &mut BytesMut) {
        dst.put_u8(if self.success { 1 } else { 0 });
        dst.put_u32_le(self.error_code);
        dst.put_u64_le(self.session_id);
        dst.put_u64_le(self.player_guid);
        let node_bytes = self.assigned_node.as_bytes();
        dst.put_u16_le(node_bytes.len() as u16);
        dst.put_slice(node_bytes);
        let msg_bytes = self.error_message.as_bytes();
        dst.put_u16_le(msg_bytes.len() as u16);
        dst.put_slice(msg_bytes);
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        // 1 (success) + 4 (error_code) + 8 (session_id) + 8 (player_guid) + 2 (node_len) + 2 (msg_len) = 25 bytes min
        const MIN_SIZE: usize = 25;
        if src.remaining() < MIN_SIZE {
            return Err(ClientCodecError::BufferTooShort {
                expected: MIN_SIZE,
                got: src.remaining(),
            });
        }
        let success = src.get_u8() != 0;
        let error_code = src.get_u32_le();
        let session_id = src.get_u64_le();
        let player_guid = src.get_u64_le();

        let node_len = src.get_u16_le() as usize;
        if src.remaining() < node_len {
            return Err(ClientCodecError::BufferTooShort {
                expected: node_len,
                got: src.remaining(),
            });
        }
        let node_bytes = src.copy_to_bytes(node_len);
        let assigned_node = String::from_utf8(node_bytes.to_vec())
            .map_err(|e| ClientCodecError::InvalidData(e.to_string()))?;

        if src.remaining() < 2 {
            return Err(ClientCodecError::BufferTooShort {
                expected: 2,
                got: src.remaining(),
            });
        }
        let msg_len = src.get_u16_le() as usize;
        if src.remaining() < msg_len {
            return Err(ClientCodecError::BufferTooShort {
                expected: msg_len,
                got: src.remaining(),
            });
        }
        let msg_bytes = src.copy_to_bytes(msg_len);
        let error_message = String::from_utf8(msg_bytes.to_vec())
            .map_err(|e| ClientCodecError::InvalidData(e.to_string()))?;

        Ok(Self {
            success,
            error_code,
            session_id,
            player_guid,
            assigned_node,
            error_message,
        })
    }
}

/// Social message packet for global chat, party chat, guild chat, and direct whispers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocialMessagePacket {
    pub channel_type: u8,
    pub target_id: u64,
    pub sender_guid: u64,
    pub sender_name: String,
    pub content: String,
    pub timestamp: u64,
}

impl SocialMessagePacket {
    pub fn encode(&self, dst: &mut BytesMut) {
        dst.put_u8(self.channel_type);
        dst.put_u64_le(self.target_id);
        dst.put_u64_le(self.sender_guid);
        let name_bytes = self.sender_name.as_bytes();
        dst.put_u16_le(name_bytes.len() as u16);
        dst.put_slice(name_bytes);
        let content_bytes = self.content.as_bytes();
        dst.put_u16_le(content_bytes.len() as u16);
        dst.put_slice(content_bytes);
        dst.put_u64_le(self.timestamp);
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        const MIN_SIZE: usize = 29;
        if src.remaining() < MIN_SIZE {
            return Err(ClientCodecError::BufferTooShort {
                expected: MIN_SIZE,
                got: src.remaining(),
            });
        }
        let channel_type = src.get_u8();
        let target_id = src.get_u64_le();
        let sender_guid = src.get_u64_le();
        let name_len = src.get_u16_le() as usize;
        if src.remaining() < name_len {
            return Err(ClientCodecError::BufferTooShort {
                expected: name_len,
                got: src.remaining(),
            });
        }
        let name_bytes = src.copy_to_bytes(name_len);
        let sender_name = String::from_utf8(name_bytes.to_vec())
            .map_err(|e| ClientCodecError::InvalidData(e.to_string()))?;

        if src.remaining() < 2 {
            return Err(ClientCodecError::BufferTooShort {
                expected: 2,
                got: src.remaining(),
            });
        }
        let content_len = src.get_u16_le() as usize;
        if src.remaining() < content_len + 8 {
            return Err(ClientCodecError::BufferTooShort {
                expected: content_len + 8,
                got: src.remaining(),
            });
        }
        let content_bytes = src.copy_to_bytes(content_len);
        let content = String::from_utf8(content_bytes.to_vec())
            .map_err(|e| ClientCodecError::InvalidData(e.to_string()))?;
        let timestamp = src.get_u64_le();

        Ok(Self {
            channel_type,
            target_id,
            sender_guid,
            sender_name,
            content,
            timestamp,
        })
    }
}

/// Request to subscribe to a social pub/sub topic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocialSubscribePacket {
    pub channel_type: u8,
    pub target_id: u64,
    pub channel_name: String,
}

impl SocialSubscribePacket {
    pub fn encode(&self, dst: &mut BytesMut) {
        dst.put_u8(self.channel_type);
        dst.put_u64_le(self.target_id);
        let name_bytes = self.channel_name.as_bytes();
        dst.put_u16_le(name_bytes.len() as u16);
        dst.put_slice(name_bytes);
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        if src.remaining() < 11 {
            return Err(ClientCodecError::BufferTooShort {
                expected: 11,
                got: src.remaining(),
            });
        }
        let channel_type = src.get_u8();
        let target_id = src.get_u64_le();
        let name_len = src.get_u16_le() as usize;
        if src.remaining() < name_len {
            return Err(ClientCodecError::BufferTooShort {
                expected: name_len,
                got: src.remaining(),
            });
        }
        let name_bytes = src.copy_to_bytes(name_len);
        let channel_name = String::from_utf8(name_bytes.to_vec())
            .map_err(|e| ClientCodecError::InvalidData(e.to_string()))?;
        Ok(Self {
            channel_type,
            target_id,
            channel_name,
        })
    }
}

/// Request to unsubscribe from a social pub/sub topic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocialUnsubscribePacket {
    pub channel_type: u8,
    pub target_id: u64,
    pub channel_name: String,
}

impl SocialUnsubscribePacket {
    pub fn encode(&self, dst: &mut BytesMut) {
        dst.put_u8(self.channel_type);
        dst.put_u64_le(self.target_id);
        let name_bytes = self.channel_name.as_bytes();
        dst.put_u16_le(name_bytes.len() as u16);
        dst.put_slice(name_bytes);
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        if src.remaining() < 11 {
            return Err(ClientCodecError::BufferTooShort {
                expected: 11,
                got: src.remaining(),
            });
        }
        let channel_type = src.get_u8();
        let target_id = src.get_u64_le();
        let name_len = src.get_u16_le() as usize;
        if src.remaining() < name_len {
            return Err(ClientCodecError::BufferTooShort {
                expected: name_len,
                got: src.remaining(),
            });
        }
        let name_bytes = src.copy_to_bytes(name_len);
        let channel_name = String::from_utf8(name_bytes.to_vec())
            .map_err(|e| ClientCodecError::InvalidData(e.to_string()))?;
        Ok(Self {
            channel_type,
            target_id,
            channel_name,
        })
    }
}

/// Standard marketplace error codes.
pub mod market_error_codes {
    pub const SUCCESS: u32 = 0;
    pub const LISTING_NOT_FOUND: u32 = 1;
    pub const UNAUTHORIZED: u32 = 2;
    pub const INSUFFICIENT_FUNDS: u32 = 3;
    pub const CONTAINER_FULL: u32 = 4;
    pub const ITEM_NOT_FOUND: u32 = 5;
    pub const ALREADY_SOLD_OR_CANCELLED: u32 = 6;
    pub const INTERNAL_ERROR: u32 = 7;
    pub const INVALID_PARAMETERS: u32 = 8;
}

/// Wire representation of a marketplace listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketListingWire {
    pub listing_id: String,
    pub seller_guid: u64,
    pub item_instance_id: String,
    pub item_def_id: u32,
    pub price: u64,
    pub created_at: u64,
    pub expires_at: u64,
}

impl MarketListingWire {
    pub fn encode(&self, dst: &mut BytesMut) {
        let lid_bytes = self.listing_id.as_bytes();
        dst.put_u16_le(lid_bytes.len() as u16);
        dst.put_slice(lid_bytes);
        dst.put_u64_le(self.seller_guid);
        let iid_bytes = self.item_instance_id.as_bytes();
        dst.put_u16_le(iid_bytes.len() as u16);
        dst.put_slice(iid_bytes);
        dst.put_u32_le(self.item_def_id);
        dst.put_u64_le(self.price);
        dst.put_u64_le(self.created_at);
        dst.put_u64_le(self.expires_at);
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        if src.remaining() < 2 {
            return Err(ClientCodecError::BufferTooShort {
                expected: 2,
                got: src.remaining(),
            });
        }
        let lid_len = src.get_u16_le() as usize;
        if src.remaining() < lid_len + 8 + 2 {
            return Err(ClientCodecError::BufferTooShort {
                expected: lid_len + 10,
                got: src.remaining(),
            });
        }
        let lid_bytes = src.copy_to_bytes(lid_len);
        let listing_id = String::from_utf8(lid_bytes.to_vec())
            .map_err(|e| ClientCodecError::InvalidData(e.to_string()))?;
        let seller_guid = src.get_u64_le();
        let iid_len = src.get_u16_le() as usize;
        if src.remaining() < iid_len + 4 + 8 + 8 + 8 {
            return Err(ClientCodecError::BufferTooShort {
                expected: iid_len + 28,
                got: src.remaining(),
            });
        }
        let iid_bytes = src.copy_to_bytes(iid_len);
        let item_instance_id = String::from_utf8(iid_bytes.to_vec())
            .map_err(|e| ClientCodecError::InvalidData(e.to_string()))?;
        let item_def_id = src.get_u32_le();
        let price = src.get_u64_le();
        let created_at = src.get_u64_le();
        let expires_at = src.get_u64_le();

        Ok(Self {
            listing_id,
            seller_guid,
            item_instance_id,
            item_def_id,
            price,
            created_at,
            expires_at,
        })
    }
}

/// Request to create a new market listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketListRequestPacket {
    pub item_instance_id: String,
    pub src_container_id: u64,
    pub src_slot: u32,
    pub price: u64,
    pub duration_secs: u64,
}

impl MarketListRequestPacket {
    pub fn encode(&self, dst: &mut BytesMut) {
        let iid_bytes = self.item_instance_id.as_bytes();
        dst.put_u16_le(iid_bytes.len() as u16);
        dst.put_slice(iid_bytes);
        dst.put_u64_le(self.src_container_id);
        dst.put_u32_le(self.src_slot);
        dst.put_u64_le(self.price);
        dst.put_u64_le(self.duration_secs);
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        if src.remaining() < 2 {
            return Err(ClientCodecError::BufferTooShort {
                expected: 2,
                got: src.remaining(),
            });
        }
        let iid_len = src.get_u16_le() as usize;
        if src.remaining() < iid_len + 28 {
            return Err(ClientCodecError::BufferTooShort {
                expected: iid_len + 28,
                got: src.remaining(),
            });
        }
        let iid_bytes = src.copy_to_bytes(iid_len);
        let item_instance_id = String::from_utf8(iid_bytes.to_vec())
            .map_err(|e| ClientCodecError::InvalidData(e.to_string()))?;
        let src_container_id = src.get_u64_le();
        let src_slot = src.get_u32_le();
        let price = src.get_u64_le();
        let duration_secs = src.get_u64_le();

        Ok(Self {
            item_instance_id,
            src_container_id,
            src_slot,
            price,
            duration_secs,
        })
    }
}

/// Request to purchase an active market listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketBuyRequestPacket {
    pub listing_id: String,
    pub buyer_container_id: u64,
    pub payment_container_id: u64,
    pub price: u64,
}

impl MarketBuyRequestPacket {
    pub fn encode(&self, dst: &mut BytesMut) {
        let lid_bytes = self.listing_id.as_bytes();
        dst.put_u16_le(lid_bytes.len() as u16);
        dst.put_slice(lid_bytes);
        dst.put_u64_le(self.buyer_container_id);
        dst.put_u64_le(self.payment_container_id);
        dst.put_u64_le(self.price);
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        if src.remaining() < 2 {
            return Err(ClientCodecError::BufferTooShort {
                expected: 2,
                got: src.remaining(),
            });
        }
        let lid_len = src.get_u16_le() as usize;
        if src.remaining() < lid_len + 24 {
            return Err(ClientCodecError::BufferTooShort {
                expected: lid_len + 24,
                got: src.remaining(),
            });
        }
        let lid_bytes = src.copy_to_bytes(lid_len);
        let listing_id = String::from_utf8(lid_bytes.to_vec())
            .map_err(|e| ClientCodecError::InvalidData(e.to_string()))?;
        let buyer_container_id = src.get_u64_le();
        let payment_container_id = src.get_u64_le();
        let price = src.get_u64_le();

        Ok(Self {
            listing_id,
            buyer_container_id,
            payment_container_id,
            price,
        })
    }
}

/// Request to cancel a market listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketCancelRequestPacket {
    pub listing_id: String,
    pub dst_container_id: u64,
}

impl MarketCancelRequestPacket {
    pub fn encode(&self, dst: &mut BytesMut) {
        let lid_bytes = self.listing_id.as_bytes();
        dst.put_u16_le(lid_bytes.len() as u16);
        dst.put_slice(lid_bytes);
        dst.put_u64_le(self.dst_container_id);
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        if src.remaining() < 2 {
            return Err(ClientCodecError::BufferTooShort {
                expected: 2,
                got: src.remaining(),
            });
        }
        let lid_len = src.get_u16_le() as usize;
        if src.remaining() < lid_len + 8 {
            return Err(ClientCodecError::BufferTooShort {
                expected: lid_len + 8,
                got: src.remaining(),
            });
        }
        let lid_bytes = src.copy_to_bytes(lid_len);
        let listing_id = String::from_utf8(lid_bytes.to_vec())
            .map_err(|e| ClientCodecError::InvalidData(e.to_string()))?;
        let dst_container_id = src.get_u64_le();

        Ok(Self {
            listing_id,
            dst_container_id,
        })
    }
}

/// Request to query market listings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketQueryRequestPacket {
    pub item_def_id: u32,
    pub min_price: u64,
    pub max_price: u64,
    pub offset: u32,
    pub limit: u32,
}

impl MarketQueryRequestPacket {
    pub fn encode(&self, dst: &mut BytesMut) {
        dst.put_u32_le(self.item_def_id);
        dst.put_u64_le(self.min_price);
        dst.put_u64_le(self.max_price);
        dst.put_u32_le(self.offset);
        dst.put_u32_le(self.limit);
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        if src.remaining() < 28 {
            return Err(ClientCodecError::BufferTooShort {
                expected: 28,
                got: src.remaining(),
            });
        }
        let item_def_id = src.get_u32_le();
        let min_price = src.get_u64_le();
        let max_price = src.get_u64_le();
        let offset = src.get_u32_le();
        let limit = src.get_u32_le();

        Ok(Self {
            item_def_id,
            min_price,
            max_price,
            offset,
            limit,
        })
    }
}

/// Server response for marketplace actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketResponsePacket {
    pub request_type: u8,
    pub success: bool,
    pub error_code: u32,
    pub error_message: String,
    pub listings: Vec<MarketListingWire>,
}

impl MarketResponsePacket {
    pub fn encode(&self, dst: &mut BytesMut) {
        dst.put_u8(self.request_type);
        dst.put_u8(if self.success { 1 } else { 0 });
        dst.put_u32_le(self.error_code);
        let msg_bytes = self.error_message.as_bytes();
        dst.put_u16_le(msg_bytes.len() as u16);
        dst.put_slice(msg_bytes);
        dst.put_u16_le(self.listings.len() as u16);
        for item in &self.listings {
            item.encode(dst);
        }
    }

    pub fn decode(src: &mut impl Buf) -> Result<Self, ClientCodecError> {
        if src.remaining() < 10 {
            return Err(ClientCodecError::BufferTooShort {
                expected: 10,
                got: src.remaining(),
            });
        }
        let request_type = src.get_u8();
        let success = src.get_u8() != 0;
        let error_code = src.get_u32_le();
        let msg_len = src.get_u16_le() as usize;
        if src.remaining() < msg_len + 2 {
            return Err(ClientCodecError::BufferTooShort {
                expected: msg_len + 2,
                got: src.remaining(),
            });
        }
        let msg_bytes = src.copy_to_bytes(msg_len);
        let error_message = String::from_utf8(msg_bytes.to_vec())
            .map_err(|e| ClientCodecError::InvalidData(e.to_string()))?;

        let count = src.get_u16_le() as usize;
        let mut listings = Vec::with_capacity(count);
        for _ in 0..count {
            listings.push(MarketListingWire::decode(src)?);
        }

        Ok(Self {
            request_type,
            success,
            error_code,
            error_message,
            listings,
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
    WorldObjectSpawn(WorldObjectSpawnPacket),
    WorldObjectDelta(WorldObjectDeltaPacket),
    WorldObjectDespawn(WorldObjectDespawnPacket),
    WorldObjectResync(WorldObjectResyncPacket),
    PlaceObject(PlaceObjectPacket),
    InteractObject(InteractObjectPacket),
    AuthRequest(AuthRequestPacket),
    AuthResponse(AuthResponsePacket),
    SocialMessage(SocialMessagePacket),
    SocialSubscribe(SocialSubscribePacket),
    SocialUnsubscribe(SocialUnsubscribePacket),
    MarketListRequest(MarketListRequestPacket),
    MarketBuyRequest(MarketBuyRequestPacket),
    MarketCancelRequest(MarketCancelRequestPacket),
    MarketQueryRequest(MarketQueryRequestPacket),
    MarketResponse(MarketResponsePacket),
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
            ClientPacket::WorldObjectSpawn(_) => Opcode::WorldObjectSpawn,
            ClientPacket::WorldObjectDelta(_) => Opcode::WorldObjectDelta,
            ClientPacket::WorldObjectDespawn(_) => Opcode::WorldObjectDespawn,
            ClientPacket::WorldObjectResync(_) => Opcode::WorldObjectResync,
            ClientPacket::PlaceObject(_) => Opcode::PlaceObject,
            ClientPacket::InteractObject(_) => Opcode::InteractObject,
            ClientPacket::AuthRequest(_) => Opcode::AuthRequest,
            ClientPacket::AuthResponse(_) => Opcode::AuthResponse,
            ClientPacket::SocialMessage(_) => Opcode::SocialMessage,
            ClientPacket::SocialSubscribe(_) => Opcode::SocialSubscribe,
            ClientPacket::SocialUnsubscribe(_) => Opcode::SocialUnsubscribe,
            ClientPacket::MarketListRequest(_) => Opcode::MarketListRequest,
            ClientPacket::MarketBuyRequest(_) => Opcode::MarketBuyRequest,
            ClientPacket::MarketCancelRequest(_) => Opcode::MarketCancelRequest,
            ClientPacket::MarketQueryRequest(_) => Opcode::MarketQueryRequest,
            ClientPacket::MarketResponse(_) => Opcode::MarketResponse,
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
            ClientPacket::WorldObjectSpawn(p) => p.encode(&mut payload_buf),
            ClientPacket::WorldObjectDelta(p) => p.encode(&mut payload_buf),
            ClientPacket::WorldObjectDespawn(p) => p.encode(&mut payload_buf),
            ClientPacket::WorldObjectResync(p) => p.encode(&mut payload_buf),
            ClientPacket::PlaceObject(p) => p.encode(&mut payload_buf),
            ClientPacket::InteractObject(p) => p.encode(&mut payload_buf),
            ClientPacket::AuthRequest(p) => p.encode(&mut payload_buf),
            ClientPacket::AuthResponse(p) => p.encode(&mut payload_buf),
            ClientPacket::SocialMessage(p) => p.encode(&mut payload_buf),
            ClientPacket::SocialSubscribe(p) => p.encode(&mut payload_buf),
            ClientPacket::SocialUnsubscribe(p) => p.encode(&mut payload_buf),
            ClientPacket::MarketListRequest(p) => p.encode(&mut payload_buf),
            ClientPacket::MarketBuyRequest(p) => p.encode(&mut payload_buf),
            ClientPacket::MarketCancelRequest(p) => p.encode(&mut payload_buf),
            ClientPacket::MarketQueryRequest(p) => p.encode(&mut payload_buf),
            ClientPacket::MarketResponse(p) => p.encode(&mut payload_buf),
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
            Opcode::WorldObjectSpawn => Ok(ClientPacket::WorldObjectSpawn(
                WorldObjectSpawnPacket::decode(&mut payload)?,
            )),
            Opcode::WorldObjectDelta => Ok(ClientPacket::WorldObjectDelta(
                WorldObjectDeltaPacket::decode(&mut payload)?,
            )),
            Opcode::WorldObjectDespawn => Ok(ClientPacket::WorldObjectDespawn(
                WorldObjectDespawnPacket::decode(&mut payload)?,
            )),
            Opcode::WorldObjectResync => Ok(ClientPacket::WorldObjectResync(
                WorldObjectResyncPacket::decode(&mut payload)?,
            )),
            Opcode::PlaceObject => Ok(ClientPacket::PlaceObject(PlaceObjectPacket::decode(
                &mut payload,
            )?)),
            Opcode::InteractObject => Ok(ClientPacket::InteractObject(
                InteractObjectPacket::decode(&mut payload)?,
            )),
            Opcode::AuthRequest => Ok(ClientPacket::AuthRequest(AuthRequestPacket::decode(
                &mut payload,
            )?)),
            Opcode::AuthResponse => Ok(ClientPacket::AuthResponse(AuthResponsePacket::decode(
                &mut payload,
            )?)),
            Opcode::SocialMessage => Ok(ClientPacket::SocialMessage(SocialMessagePacket::decode(
                &mut payload,
            )?)),
            Opcode::SocialSubscribe => Ok(ClientPacket::SocialSubscribe(
                SocialSubscribePacket::decode(&mut payload)?,
            )),
            Opcode::SocialUnsubscribe => Ok(ClientPacket::SocialUnsubscribe(
                SocialUnsubscribePacket::decode(&mut payload)?,
            )),
            Opcode::MarketListRequest => Ok(ClientPacket::MarketListRequest(
                MarketListRequestPacket::decode(&mut payload)?,
            )),
            Opcode::MarketBuyRequest => Ok(ClientPacket::MarketBuyRequest(
                MarketBuyRequestPacket::decode(&mut payload)?,
            )),
            Opcode::MarketCancelRequest => Ok(ClientPacket::MarketCancelRequest(
                MarketCancelRequestPacket::decode(&mut payload)?,
            )),
            Opcode::MarketQueryRequest => Ok(ClientPacket::MarketQueryRequest(
                MarketQueryRequestPacket::decode(&mut payload)?,
            )),
            Opcode::MarketResponse => Ok(ClientPacket::MarketResponse(
                MarketResponsePacket::decode(&mut payload)?,
            )),
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
                    attributes: Vec::new(),
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
                    attributes: Vec::new(),
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

    #[test]
    fn test_opcode_as_str() {
        assert_eq!(Opcode::Movement.as_str(), "Movement");
        assert_eq!(Opcode::MovementCorrection.as_str(), "MovementCorrection");
        assert_eq!(Opcode::Interaction.as_str(), "Interaction");
        assert_eq!(Opcode::InteractionResult.as_str(), "InteractionResult");
        assert_eq!(Opcode::EntityState.as_str(), "EntityState");
        assert_eq!(Opcode::EntityDespawn.as_str(), "EntityDespawn");
        assert_eq!(Opcode::Ping.as_str(), "Ping");
        assert_eq!(Opcode::Pong.as_str(), "Pong");
        assert_eq!(Opcode::EntityDeltaBatch.as_str(), "EntityDeltaBatch");
        assert_eq!(Opcode::ClientAck.as_str(), "ClientAck");
    }

    #[test]
    fn test_opcode_from_u16() {
        assert_eq!(Opcode::from_u16(0x0001).unwrap(), Opcode::Movement);
        assert_eq!(
            Opcode::from_u16(0x0002).unwrap(),
            Opcode::MovementCorrection
        );
        assert_eq!(Opcode::from_u16(0x0003).unwrap(), Opcode::Interaction);
        assert_eq!(Opcode::from_u16(0x0004).unwrap(), Opcode::InteractionResult);
        assert_eq!(Opcode::from_u16(0x0005).unwrap(), Opcode::EntityState);
        assert_eq!(Opcode::from_u16(0x0006).unwrap(), Opcode::EntityDespawn);
        assert_eq!(Opcode::from_u16(0x0007).unwrap(), Opcode::Ping);
        assert_eq!(Opcode::from_u16(0x0008).unwrap(), Opcode::Pong);
        assert_eq!(Opcode::from_u16(0x0009).unwrap(), Opcode::EntityDeltaBatch);
        assert_eq!(Opcode::from_u16(0x000A).unwrap(), Opcode::ClientAck);
        assert_eq!(
            Opcode::from_u16(0xFFFF).unwrap_err(),
            ClientCodecError::UnknownOpcode(0xFFFF)
        );
    }

    #[test]
    fn test_vec3_distance_and_distance_squared() {
        let p0 = Vec3::new(0.0, 0.0, 0.0);
        let p1 = Vec3::new(3.0, 4.0, 0.0);
        assert_eq!(p0.distance_squared(&p1), 25.0);
        assert_eq!(p0.distance(&p1), 5.0);
        assert_eq!(p1.distance_squared(&p0), 25.0);
        assert_eq!(p1.distance(&p0), 5.0);

        // Identical points
        let p_same = Vec3::new(10.5, -20.25, 30.125);
        assert_eq!(p_same.distance_squared(&p_same), 0.0);
        assert_eq!(p_same.distance(&p_same), 0.0);
        assert_eq!(Vec3::ZERO.distance_squared(&Vec3::ZERO), 0.0);
        assert_eq!(Vec3::ZERO.distance(&Vec3::ZERO), 0.0);

        // Negative coordinates
        let p_neg = Vec3::new(-3.0, -4.0, 0.0);
        assert_eq!(p_neg.distance_squared(&p0), 25.0);
        assert_eq!(p_neg.distance(&p0), 5.0);

        // 3D vector with non-zero z: dx=2, dy=3, dz=6 -> 4 + 9 + 36 = 49 -> dist = 7
        let a = Vec3::new(1.0, 2.0, 3.0);
        let b = Vec3::new(3.0, 5.0, 9.0);
        assert_eq!(a.distance_squared(&b), 49.0);
        assert_eq!(a.distance(&b), 7.0);
        assert_eq!(b.distance_squared(&a), 49.0);
        assert_eq!(b.distance(&a), 7.0);

        // Single-axis differences to kill coordinate swap mutants
        let x_only = Vec3::new(5.0, 0.0, 0.0);
        assert_eq!(x_only.distance_squared(&p0), 25.0);
        assert_eq!(x_only.distance(&p0), 5.0);

        let y_only = Vec3::new(0.0, 5.0, 0.0);
        assert_eq!(y_only.distance_squared(&p0), 25.0);
        assert_eq!(y_only.distance(&p0), 5.0);

        let z_only = Vec3::new(0.0, 0.0, 5.0);
        assert_eq!(z_only.distance_squared(&p0), 25.0);
        assert_eq!(z_only.distance(&p0), 5.0);
    }

    #[test]
    fn test_packet_decode_buffer_truncation() {
        // Vec3 decode truncation (11 bytes < 12)
        let mut buf_vec3 = &vec![0u8; 11][..];
        assert_eq!(
            Vec3::decode(&mut buf_vec3),
            Err(ClientCodecError::BufferTooShort {
                expected: 12,
                got: 11,
            })
        );

        // MovementPacket::decode (51 bytes < 52)
        let mut buf_mv = &vec![0u8; 51][..];
        assert_eq!(
            MovementPacket::decode(&mut buf_mv),
            Err(ClientCodecError::BufferTooShort {
                expected: 52,
                got: 51,
            })
        );

        // MovementCorrectionPacket::decode (36 bytes < 37)
        let mut buf_mc = &vec![0u8; 36][..];
        assert_eq!(
            MovementCorrectionPacket::decode(&mut buf_mc),
            Err(ClientCodecError::BufferTooShort {
                expected: 37,
                got: 36,
            })
        );

        // InteractionPacket::decode minimum size (24 bytes and 16 bytes < 25)
        let mut buf_it24 = &vec![0u8; 24][..];
        assert_eq!(
            InteractionPacket::decode(&mut buf_it24),
            Err(ClientCodecError::BufferTooShort {
                expected: 25,
                got: 24,
            })
        );
        let mut buf_it16 = &vec![0u8; 16][..];
        assert_eq!(
            InteractionPacket::decode(&mut buf_it16),
            Err(ClientCodecError::BufferTooShort {
                expected: 25,
                got: 16,
            })
        );

        // InteractionPacket::decode payload length truncation
        let mut it_payload_buf = BytesMut::new();
        it_payload_buf.put_u64_le(10); // source_guid
        it_payload_buf.put_u64_le(20); // target_guid
        it_payload_buf.put_u8(1); // interaction_type
        it_payload_buf.put_u32_le(100); // action_id
        it_payload_buf.put_u32_le(10); // payload_len = 10
        it_payload_buf.put_slice(&[1, 2, 3]); // only 3 bytes remaining
        let mut it_slice = &it_payload_buf[..];
        assert_eq!(
            InteractionPacket::decode(&mut it_slice),
            Err(ClientCodecError::BufferTooShort {
                expected: 10,
                got: 3,
            })
        );

        // InteractionPacket::decode with exactly MIN_SIZE (25 bytes, payload_len = 0) succeeds
        let mut it_exact = BytesMut::new();
        it_exact.put_u64_le(10);
        it_exact.put_u64_le(20);
        it_exact.put_u8(1);
        it_exact.put_u32_le(100);
        it_exact.put_u32_le(0); // payload_len = 0
        let mut it_exact_slice = &it_exact[..];
        let decoded_it = InteractionPacket::decode(&mut it_exact_slice).unwrap();
        assert_eq!(decoded_it.source_guid, 10);
        assert!(decoded_it.payload.is_empty());

        // InteractionResultPacket::decode minimum size (12 bytes and 6 bytes < 13)
        let mut buf_ir12 = &vec![0u8; 12][..];
        assert_eq!(
            InteractionResultPacket::decode(&mut buf_ir12),
            Err(ClientCodecError::BufferTooShort {
                expected: 13,
                got: 12,
            })
        );
        let mut buf_ir6 = &vec![0u8; 6][..];
        assert_eq!(
            InteractionResultPacket::decode(&mut buf_ir6),
            Err(ClientCodecError::BufferTooShort {
                expected: 13,
                got: 6,
            })
        );

        // InteractionResultPacket::decode payload length truncation
        let mut ir_payload_buf = BytesMut::new();
        ir_payload_buf.put_u32_le(100); // action_id
        ir_payload_buf.put_u8(1); // success
        ir_payload_buf.put_u32_le(0); // error_code
        ir_payload_buf.put_u32_le(8); // payload_len = 8
        ir_payload_buf.put_slice(&[1, 2]); // only 2 bytes remaining
        let mut ir_slice = &ir_payload_buf[..];
        assert_eq!(
            InteractionResultPacket::decode(&mut ir_slice),
            Err(ClientCodecError::BufferTooShort {
                expected: 8,
                got: 2,
            })
        );

        // InteractionResultPacket::decode with exactly MIN_SIZE (13 bytes, payload_len = 0) succeeds
        let mut ir_exact = BytesMut::new();
        ir_exact.put_u32_le(100);
        ir_exact.put_u8(1);
        ir_exact.put_u32_le(0);
        ir_exact.put_u32_le(0); // payload_len = 0
        let mut ir_exact_slice = &ir_exact[..];
        let decoded_ir = InteractionResultPacket::decode(&mut ir_exact_slice).unwrap();
        assert_eq!(decoded_ir.action_id, 100);
        assert!(decoded_ir.payload.is_empty());

        // EntityStatePacket::decode (59 bytes and 56 bytes < 60)
        let mut buf_es59 = &vec![0u8; 59][..];
        assert_eq!(
            EntityStatePacket::decode(&mut buf_es59),
            Err(ClientCodecError::BufferTooShort {
                expected: 60,
                got: 59,
            })
        );
        let mut buf_es56 = &vec![0u8; 56][..];
        assert_eq!(
            EntityStatePacket::decode(&mut buf_es56),
            Err(ClientCodecError::BufferTooShort {
                expected: 60,
                got: 56,
            })
        );

        // EntityDespawnPacket::decode (7 bytes < 8)
        let mut buf_ed = &vec![0u8; 7][..];
        assert_eq!(
            EntityDespawnPacket::decode(&mut buf_ed),
            Err(ClientCodecError::BufferTooShort {
                expected: 8,
                got: 7,
            })
        );

        // PingPacket::decode (7 bytes < 8)
        let mut buf_ping = &vec![0u8; 7][..];
        assert_eq!(
            PingPacket::decode(&mut buf_ping),
            Err(ClientCodecError::BufferTooShort {
                expected: 8,
                got: 7,
            })
        );

        // PongPacket::decode (15 bytes < 16)
        let mut buf_pong = &vec![0u8; 15][..];
        assert_eq!(
            PongPacket::decode(&mut buf_pong),
            Err(ClientCodecError::BufferTooShort {
                expected: 16,
                got: 15,
            })
        );

        // ClientAckPacket::decode (varints valid, but ack_mask truncated to 3 bytes < 4)
        let mut buf_ack = BytesMut::new();
        put_varint_u64(&mut buf_ack, 100); // session_id
        put_varint_u64(&mut buf_ack, 200); // observed_tick
        buf_ack.put_slice(&[1, 2, 3]); // only 3 bytes instead of 4
        let mut ack_slice = &buf_ack[..];
        assert_eq!(
            ClientAckPacket::decode(&mut ack_slice),
            Err(ClientCodecError::BufferTooShort {
                expected: 4,
                got: 3,
            })
        );

        // ClientAckPacket::decode with empty buffer (fails at first varint)
        let mut buf_ack_empty = &[][..];
        assert_eq!(
            ClientAckPacket::decode(&mut buf_ack_empty),
            Err(ClientCodecError::BufferTooShort {
                expected: 1,
                got: 0,
            })
        );
    }

    #[test]
    fn test_delta_flags_constants() {
        assert_eq!(delta_flags::MASK_KEYFRAME, 1);
        assert_eq!(delta_flags::MASK_DELTA_POS_X, 2);
        assert_eq!(delta_flags::MASK_DELTA_POS_Y, 4);
        assert_eq!(delta_flags::MASK_DELTA_POS_Z, 8);
        assert_eq!(delta_flags::MASK_VELOCITY, 16);
        assert_eq!(delta_flags::MASK_YAW, 32);
        assert_eq!(delta_flags::MASK_PITCH, 64);
        assert_eq!(delta_flags::MASK_HEALTH, 128);
        assert_eq!(delta_flags::MASK_MAX_HEALTH, 256);
        assert_eq!(delta_flags::MASK_STATE_FLAGS, 512);
        assert_eq!(delta_flags::MASK_ACTION_ID, 1024);
    }

    #[test]
    fn test_entity_delta_state_individual_fields_roundtrip() {
        // MASK_KEYFRAME
        let s_kf = EntityDeltaState {
            entity_id: 42,
            field_mask: delta_flags::MASK_KEYFRAME,
            position: (12.5, -34.75, 56.125),
            ..Default::default()
        };
        let mut buf = BytesMut::new();
        s_kf.encode(&mut buf);
        let dec_kf = EntityDeltaState::decode(&mut buf).unwrap();
        assert_eq!(dec_kf.entity_id, 42);
        assert_eq!(dec_kf.field_mask, delta_flags::MASK_KEYFRAME);
        assert_eq!(dec_kf.position, (12.5, -34.75, 56.125));

        // MASK_DELTA_POS_X
        let s_dx = EntityDeltaState {
            entity_id: 1,
            field_mask: delta_flags::MASK_DELTA_POS_X,
            position: (1.5, 0.0, 0.0),
            ..Default::default()
        };
        let mut buf = BytesMut::new();
        s_dx.encode(&mut buf);
        let dec_dx = EntityDeltaState::decode(&mut buf).unwrap();
        assert_eq!(dec_dx.entity_id, 1);
        assert_eq!(dec_dx.field_mask, delta_flags::MASK_DELTA_POS_X);
        assert!((dec_dx.position.0 - 1.5).abs() < 0.008);
        assert_eq!(dec_dx.position.1, 0.0);
        assert_eq!(dec_dx.position.2, 0.0);

        // MASK_DELTA_POS_Y
        let s_dy = EntityDeltaState {
            entity_id: 2,
            field_mask: delta_flags::MASK_DELTA_POS_Y,
            position: (0.0, -2.25, 0.0),
            ..Default::default()
        };
        let mut buf = BytesMut::new();
        s_dy.encode(&mut buf);
        let dec_dy = EntityDeltaState::decode(&mut buf).unwrap();
        assert_eq!(dec_dy.entity_id, 2);
        assert_eq!(dec_dy.field_mask, delta_flags::MASK_DELTA_POS_Y);
        assert_eq!(dec_dy.position.0, 0.0);
        assert!((dec_dy.position.1 - (-2.25)).abs() < 0.008);
        assert_eq!(dec_dy.position.2, 0.0);

        // MASK_DELTA_POS_Z
        let s_dz = EntityDeltaState {
            entity_id: 3,
            field_mask: delta_flags::MASK_DELTA_POS_Z,
            position: (0.0, 0.0, 3.75),
            ..Default::default()
        };
        let mut buf = BytesMut::new();
        s_dz.encode(&mut buf);
        let dec_dz = EntityDeltaState::decode(&mut buf).unwrap();
        assert_eq!(dec_dz.entity_id, 3);
        assert_eq!(dec_dz.field_mask, delta_flags::MASK_DELTA_POS_Z);
        assert_eq!(dec_dz.position.0, 0.0);
        assert_eq!(dec_dz.position.1, 0.0);
        assert!((dec_dz.position.2 - 3.75).abs() < 0.008);

        // MASK_VELOCITY
        let s_vel = EntityDeltaState {
            entity_id: 4,
            field_mask: delta_flags::MASK_VELOCITY,
            velocity: (1.5, -2.5, 3.5),
            ..Default::default()
        };
        let mut buf = BytesMut::new();
        s_vel.encode(&mut buf);
        let dec_vel = EntityDeltaState::decode(&mut buf).unwrap();
        assert_eq!(dec_vel.entity_id, 4);
        assert_eq!(dec_vel.field_mask, delta_flags::MASK_VELOCITY);
        assert!((dec_vel.velocity.0 - 1.5).abs() < 0.01);
        assert!((dec_vel.velocity.1 - (-2.5)).abs() < 0.01);
        assert!((dec_vel.velocity.2 - 3.5).abs() < 0.01);

        // MASK_YAW
        let s_yaw = EntityDeltaState {
            entity_id: 5,
            field_mask: delta_flags::MASK_YAW,
            yaw: 1.57,
            ..Default::default()
        };
        let mut buf = BytesMut::new();
        s_yaw.encode(&mut buf);
        let dec_yaw = EntityDeltaState::decode(&mut buf).unwrap();
        assert_eq!(dec_yaw.entity_id, 5);
        assert_eq!(dec_yaw.field_mask, delta_flags::MASK_YAW);
        assert!((dec_yaw.yaw - 1.57).abs() < 0.03);

        // MASK_PITCH
        let s_pitch = EntityDeltaState {
            entity_id: 6,
            field_mask: delta_flags::MASK_PITCH,
            pitch: -0.75,
            ..Default::default()
        };
        let mut buf = BytesMut::new();
        s_pitch.encode(&mut buf);
        let dec_pitch = EntityDeltaState::decode(&mut buf).unwrap();
        assert_eq!(dec_pitch.entity_id, 6);
        assert_eq!(dec_pitch.field_mask, delta_flags::MASK_PITCH);
        assert!((dec_pitch.pitch - (-0.75)).abs() < 0.03);

        // MASK_HEALTH
        let s_hp = EntityDeltaState {
            entity_id: 7,
            field_mask: delta_flags::MASK_HEALTH,
            health: 250,
            ..Default::default()
        };
        let mut buf = BytesMut::new();
        s_hp.encode(&mut buf);
        let dec_hp = EntityDeltaState::decode(&mut buf).unwrap();
        assert_eq!(dec_hp.entity_id, 7);
        assert_eq!(dec_hp.field_mask, delta_flags::MASK_HEALTH);
        assert_eq!(dec_hp.health, 250);

        // MASK_MAX_HEALTH
        let s_max_hp = EntityDeltaState {
            entity_id: 8,
            field_mask: delta_flags::MASK_MAX_HEALTH,
            max_health: 500,
            ..Default::default()
        };
        let mut buf = BytesMut::new();
        s_max_hp.encode(&mut buf);
        let dec_max_hp = EntityDeltaState::decode(&mut buf).unwrap();
        assert_eq!(dec_max_hp.entity_id, 8);
        assert_eq!(dec_max_hp.field_mask, delta_flags::MASK_MAX_HEALTH);
        assert_eq!(dec_max_hp.max_health, 500);

        // MASK_STATE_FLAGS
        let s_sf = EntityDeltaState {
            entity_id: 9,
            field_mask: delta_flags::MASK_STATE_FLAGS,
            state_flags: 0xABCD_1234,
            ..Default::default()
        };
        let mut buf = BytesMut::new();
        s_sf.encode(&mut buf);
        let dec_sf = EntityDeltaState::decode(&mut buf).unwrap();
        assert_eq!(dec_sf.entity_id, 9);
        assert_eq!(dec_sf.field_mask, delta_flags::MASK_STATE_FLAGS);
        assert_eq!(dec_sf.state_flags, 0xABCD_1234);

        // MASK_ACTION_ID
        let s_act = EntityDeltaState {
            entity_id: 10,
            field_mask: delta_flags::MASK_ACTION_ID,
            action_id: 7777,
            ..Default::default()
        };
        let mut buf = BytesMut::new();
        s_act.encode(&mut buf);
        let dec_act = EntityDeltaState::decode(&mut buf).unwrap();
        assert_eq!(dec_act.entity_id, 10);
        assert_eq!(dec_act.field_mask, delta_flags::MASK_ACTION_ID);
        assert_eq!(dec_act.action_id, 7777);

        // MASK_ATTRIBUTES
        let s_attr = EntityDeltaState {
            entity_id: 11,
            field_mask: delta_flags::MASK_ATTRIBUTES,
            attributes: vec![
                ScalarAttributeDelta {
                    attribute_id: 1,
                    value: 100,
                },
                ScalarAttributeDelta {
                    attribute_id: 5,
                    value: 5000,
                },
            ],
            ..Default::default()
        };
        let mut buf = BytesMut::new();
        s_attr.encode(&mut buf);
        let dec_attr = EntityDeltaState::decode(&mut buf).unwrap();
        assert_eq!(dec_attr.entity_id, 11);
        assert_eq!(dec_attr.field_mask, delta_flags::MASK_ATTRIBUTES);
        assert_eq!(dec_attr.attributes.len(), 2);
        assert_eq!(dec_attr.attributes[0].attribute_id, 1);
        assert_eq!(dec_attr.attributes[0].value, 100);
        assert_eq!(dec_attr.attributes[1].attribute_id, 5);
        assert_eq!(dec_attr.attributes[1].value, 5000);
    }

    #[test]
    fn test_entity_delta_state_combined_fields_roundtrip() {
        let all_delta_flags = delta_flags::MASK_DELTA_POS_X
            | delta_flags::MASK_DELTA_POS_Y
            | delta_flags::MASK_DELTA_POS_Z
            | delta_flags::MASK_VELOCITY
            | delta_flags::MASK_YAW
            | delta_flags::MASK_PITCH
            | delta_flags::MASK_HEALTH
            | delta_flags::MASK_MAX_HEALTH
            | delta_flags::MASK_STATE_FLAGS
            | delta_flags::MASK_ACTION_ID
            | delta_flags::MASK_ATTRIBUTES;

        let state = EntityDeltaState {
            entity_id: 999,
            field_mask: all_delta_flags,
            position: (1.25, -2.5, 3.75),
            velocity: (-1.0, 2.0, -3.0),
            yaw: 1.0,
            pitch: -0.5,
            health: 80,
            max_health: 120,
            state_flags: 0xF00F,
            action_id: 42,
            attributes: vec![
                ScalarAttributeDelta {
                    attribute_id: 10,
                    value: 420,
                },
                ScalarAttributeDelta {
                    attribute_id: 20,
                    value: 840,
                },
            ],
        };

        let mut buf = BytesMut::new();
        state.encode(&mut buf);
        let decoded = EntityDeltaState::decode(&mut buf).unwrap();

        assert_eq!(decoded.entity_id, 999);
        assert_eq!(decoded.field_mask, all_delta_flags);
        assert!((decoded.position.0 - 1.25).abs() < 0.008);
        assert!((decoded.position.1 - (-2.5)).abs() < 0.008);
        assert!((decoded.position.2 - 3.75).abs() < 0.008);
        assert!((decoded.velocity.0 - (-1.0)).abs() < 0.01);
        assert!((decoded.velocity.1 - 2.0).abs() < 0.01);
        assert!((decoded.velocity.2 - (-3.0)).abs() < 0.01);
        assert!((decoded.yaw - 1.0).abs() < 0.03);
        assert!((decoded.pitch - (-0.5)).abs() < 0.03);
        assert_eq!(decoded.health, 80);
        assert_eq!(decoded.max_health, 120);
        assert_eq!(decoded.state_flags, 0xF00F);
        assert_eq!(decoded.action_id, 42);
        assert_eq!(decoded.attributes.len(), 2);
        assert_eq!(decoded.attributes[0].attribute_id, 10);
        assert_eq!(decoded.attributes[0].value, 420);
        assert_eq!(decoded.attributes[1].attribute_id, 20);
        assert_eq!(decoded.attributes[1].value, 840);
    }

    #[test]
    fn test_entity_delta_state_decode_truncations() {
        // Field mask truncated (< 2 bytes)
        let mut buf = BytesMut::new();
        put_varint_u64(&mut buf, 1);
        buf.put_u8(0); // 1 byte instead of 2
        let mut slice = &buf[..];
        assert_eq!(
            EntityDeltaState::decode(&mut slice),
            Err(ClientCodecError::BufferTooShort {
                expected: 2,
                got: 1,
            })
        );

        // Keyframe position truncated (< 12 bytes)
        let mut buf = BytesMut::new();
        put_varint_u64(&mut buf, 1);
        buf.put_u16_le(delta_flags::MASK_KEYFRAME);
        buf.put_slice(&[0u8; 11]); // 11 bytes instead of 12
        let mut slice = &buf[..];
        assert_eq!(
            EntityDeltaState::decode(&mut slice),
            Err(ClientCodecError::BufferTooShort {
                expected: 12,
                got: 11,
            })
        );

        // Delta pos X truncated (< 2 bytes)
        let mut buf = BytesMut::new();
        put_varint_u64(&mut buf, 1);
        buf.put_u16_le(delta_flags::MASK_DELTA_POS_X);
        buf.put_u8(0); // 1 byte instead of 2
        let mut slice = &buf[..];
        assert_eq!(
            EntityDeltaState::decode(&mut slice),
            Err(ClientCodecError::BufferTooShort {
                expected: 2,
                got: 1,
            })
        );

        // Delta pos Y truncated (< 2 bytes)
        let mut buf = BytesMut::new();
        put_varint_u64(&mut buf, 1);
        buf.put_u16_le(delta_flags::MASK_DELTA_POS_Y);
        buf.put_u8(0); // 1 byte instead of 2
        let mut slice = &buf[..];
        assert_eq!(
            EntityDeltaState::decode(&mut slice),
            Err(ClientCodecError::BufferTooShort {
                expected: 2,
                got: 1,
            })
        );

        // Delta pos Z truncated (< 2 bytes)
        let mut buf = BytesMut::new();
        put_varint_u64(&mut buf, 1);
        buf.put_u16_le(delta_flags::MASK_DELTA_POS_Z);
        buf.put_u8(0); // 1 byte instead of 2
        let mut slice = &buf[..];
        assert_eq!(
            EntityDeltaState::decode(&mut slice),
            Err(ClientCodecError::BufferTooShort {
                expected: 2,
                got: 1,
            })
        );

        // Velocity truncated (< 6 bytes)
        let mut buf = BytesMut::new();
        put_varint_u64(&mut buf, 1);
        buf.put_u16_le(delta_flags::MASK_VELOCITY);
        buf.put_slice(&[0u8; 5]); // 5 bytes instead of 6
        let mut slice = &buf[..];
        assert_eq!(
            EntityDeltaState::decode(&mut slice),
            Err(ClientCodecError::BufferTooShort {
                expected: 6,
                got: 5,
            })
        );

        // Yaw truncated (0 bytes)
        let mut buf = BytesMut::new();
        put_varint_u64(&mut buf, 1);
        buf.put_u16_le(delta_flags::MASK_YAW);
        let mut slice = &buf[..];
        assert_eq!(
            EntityDeltaState::decode(&mut slice),
            Err(ClientCodecError::BufferTooShort {
                expected: 1,
                got: 0,
            })
        );

        // Pitch truncated (0 bytes)
        let mut buf = BytesMut::new();
        put_varint_u64(&mut buf, 1);
        buf.put_u16_le(delta_flags::MASK_PITCH);
        let mut slice = &buf[..];
        assert_eq!(
            EntityDeltaState::decode(&mut slice),
            Err(ClientCodecError::BufferTooShort {
                expected: 1,
                got: 0,
            })
        );

        // Attributes count exceeds 256
        let mut buf = BytesMut::new();
        put_varint_u64(&mut buf, 1);
        buf.put_u16_le(delta_flags::MASK_ATTRIBUTES);
        put_varint_u32(&mut buf, 257); // > 256
        let mut slice = &buf[..];
        assert!(matches!(
            EntityDeltaState::decode(&mut slice),
            Err(ClientCodecError::InvalidData(_))
        ));

        // Attribute ID exceeds u16::MAX
        let mut buf = BytesMut::new();
        put_varint_u64(&mut buf, 1);
        buf.put_u16_le(delta_flags::MASK_ATTRIBUTES);
        put_varint_u32(&mut buf, 1);
        put_varint_u32(&mut buf, 70000); // > u16::MAX
        put_varint_u32(&mut buf, 100);
        let mut slice = &buf[..];
        assert!(matches!(
            EntityDeltaState::decode(&mut slice),
            Err(ClientCodecError::InvalidData(_))
        ));

        // Attributes list truncated
        let mut buf = BytesMut::new();
        put_varint_u64(&mut buf, 1);
        buf.put_u16_le(delta_flags::MASK_ATTRIBUTES);
        put_varint_u32(&mut buf, 2); // claims 2 attributes
        put_varint_u32(&mut buf, 1);
        put_varint_u32(&mut buf, 100); // only 1 attribute provided
        let mut slice = &buf[..];
        assert!(matches!(
            EntityDeltaState::decode(&mut slice),
            Err(ClientCodecError::BufferTooShort { .. })
        ));
    }

    #[test]
    fn test_client_packet_framing_decode_bytes_and_frame() {
        // decode_bytes with 7 bytes (< HEADER_SIZE 8)
        let bytes_short = Bytes::copy_from_slice(&[0u8; 7]);
        assert_eq!(
            ClientPacket::decode_bytes(bytes_short),
            Err(ClientCodecError::BufferTooShort {
                expected: 8,
                got: 7,
            })
        );

        // decode_bytes with payload_len > remaining -> LengthMismatch
        let mut buf_mismatch = BytesMut::new();
        buf_mismatch.put_u16_le(PACKET_MAGIC);
        buf_mismatch.put_u16_le(Opcode::Ping as u16);
        buf_mismatch.put_u32_le(25); // claims 25 bytes payload
        buf_mismatch.put_slice(&[0u8; 5]); // only 5 bytes provided
        assert_eq!(
            ClientPacket::decode_bytes(buf_mismatch.freeze()),
            Err(ClientCodecError::LengthMismatch {
                expected: 25,
                got: 5,
            })
        );

        // decode_frame with 7 bytes (< HEADER_SIZE 8) -> Ok(None)
        let mut frame_short = BytesMut::from(&[0u8; 7][..]);
        assert_eq!(ClientPacket::decode_frame(&mut frame_short).unwrap(), None);

        // decode_frame with 8 bytes header claiming 10 bytes payload -> Ok(None)
        let mut frame_hdr = BytesMut::new();
        frame_hdr.put_u16_le(PACKET_MAGIC);
        frame_hdr.put_u16_le(Opcode::Ping as u16);
        frame_hdr.put_u32_le(10); // payload_len = 10 -> total_frame_size = 18
        assert_eq!(ClientPacket::decode_frame(&mut frame_hdr).unwrap(), None);
        assert_eq!(frame_hdr.len(), 8); // Buffer must not be consumed

        // decode_bytes with exactly HEADER_SIZE 8 bytes (kills < replaced with <= on HEADER_SIZE check)
        let mut buf_exact_hdr = BytesMut::new();
        buf_exact_hdr.put_u16_le(PACKET_MAGIC);
        buf_exact_hdr.put_u16_le(Opcode::Ping as u16);
        buf_exact_hdr.put_u32_le(0); // claims 0 payload
        assert_eq!(
            ClientPacket::decode_bytes(buf_exact_hdr.freeze()),
            Err(ClientCodecError::BufferTooShort {
                expected: 8,
                got: 0,
            })
        );

        // decode_frame with exactly HEADER_SIZE 8 bytes and invalid magic (kills < replaced with <= on HEADER_SIZE check)
        let mut frame_bad_magic = BytesMut::new();
        frame_bad_magic.put_u16_le(0x9999); // bad magic
        frame_bad_magic.put_u16_le(Opcode::Ping as u16);
        frame_bad_magic.put_u32_le(0);
        assert_eq!(
            ClientPacket::decode_frame(&mut frame_bad_magic),
            Err(ClientCodecError::InvalidMagic {
                expected: PACKET_MAGIC,
                got: 0x9999,
            })
        );
    }

    #[test]
    fn test_interaction_type_roundtrip() {
        for (val, expected) in [
            (0, InteractionType::Talk),
            (1, InteractionType::Attack),
            (2, InteractionType::UseItem),
            (3, InteractionType::Loot),
            (4, InteractionType::Trade),
            (99, InteractionType::Custom(99)),
        ] {
            assert_eq!(InteractionType::from(val), expected);
            assert_eq!(u8::from(expected), val);
        }
    }

    #[test]
    fn test_client_packet_opcode_all_variants() {
        let packets = [
            (
                ClientPacket::Movement(MovementPacket {
                    entity_id: 1,
                    position: Vec3::ZERO,
                    velocity: Vec3::ZERO,
                    yaw: 0.0,
                    pitch: 0.0,
                    timestamp: 0,
                    sequence: 0,
                }),
                Opcode::Movement,
            ),
            (
                ClientPacket::MovementCorrection(MovementCorrectionPacket {
                    entity_id: 1,
                    corrected_position: Vec3::ZERO,
                    corrected_velocity: Vec3::ZERO,
                    sequence: 0,
                    reason: 0,
                }),
                Opcode::MovementCorrection,
            ),
            (
                ClientPacket::Interaction(InteractionPacket {
                    source_guid: 1,
                    target_guid: 2,
                    interaction_type: InteractionType::Talk,
                    action_id: 1,
                    payload: Bytes::new(),
                }),
                Opcode::Interaction,
            ),
            (
                ClientPacket::InteractionResult(InteractionResultPacket {
                    action_id: 1,
                    success: true,
                    error_code: 0,
                    payload: vec![],
                }),
                Opcode::InteractionResult,
            ),
            (
                ClientPacket::EntityState(EntityStatePacket {
                    entity_id: 1,
                    position: Vec3::ZERO,
                    velocity: Vec3::ZERO,
                    yaw: 0.0,
                    pitch: 0.0,
                    health: 100,
                    max_health: 100,
                    state_flags: 0,
                    timestamp: 0,
                }),
                Opcode::EntityState,
            ),
            (
                ClientPacket::EntityDespawn(EntityDespawnPacket { entity_id: 1 }),
                Opcode::EntityDespawn,
            ),
            (
                ClientPacket::Ping(PingPacket { timestamp: 1 }),
                Opcode::Ping,
            ),
            (
                ClientPacket::Pong(PongPacket {
                    client_timestamp: 1,
                    server_timestamp: 2,
                }),
                Opcode::Pong,
            ),
            (
                ClientPacket::EntityDeltaBatch(EntityDeltaBatchPacket {
                    server_tick: 1,
                    baseline_tick: 0,
                    despawns: vec![],
                    deltas: vec![],
                }),
                Opcode::EntityDeltaBatch,
            ),
            (
                ClientPacket::ClientAck(ClientAckPacket {
                    session_id: 1,
                    observed_tick: 1,
                    ack_mask: 0,
                }),
                Opcode::ClientAck,
            ),
            (
                ClientPacket::WorldObjectSpawn(WorldObjectSpawnPacket {
                    object_id: 100,
                    prefab_id: 1,
                    pos: Vec3::ZERO,
                    rot: (0.0, 0.0, 0.0, 1.0),
                    scale: Vec3 {
                        x: 1.0,
                        y: 1.0,
                        z: 1.0,
                    },
                    bounds_min: Vec3::ZERO,
                    bounds_max: Vec3::ZERO,
                    state_flags: 1,
                    owner_type: 0,
                    owner_id: "owner".into(),
                    health: Some((100, 100)),
                    properties: HashMap::new(),
                    revision: 1,
                }),
                Opcode::WorldObjectSpawn,
            ),
            (
                ClientPacket::WorldObjectDelta(WorldObjectDeltaPacket {
                    object_id: 100,
                    revision: 2,
                    state_flags: Some(2),
                    health: None,
                    properties: HashMap::new(),
                }),
                Opcode::WorldObjectDelta,
            ),
            (
                ClientPacket::WorldObjectDespawn(WorldObjectDespawnPacket { object_id: 100 }),
                Opcode::WorldObjectDespawn,
            ),
            (
                ClientPacket::WorldObjectResync(WorldObjectResyncPacket { object_id: 100 }),
                Opcode::WorldObjectResync,
            ),
            (
                ClientPacket::PlaceObject(PlaceObjectPacket {
                    prefab_id: 1,
                    pos: Vec3::ZERO,
                    rot: (0.0, 0.0, 0.0, 1.0),
                    scale: Vec3 {
                        x: 1.0,
                        y: 1.0,
                        z: 1.0,
                    },
                    properties: HashMap::new(),
                }),
                Opcode::PlaceObject,
            ),
            (
                ClientPacket::InteractObject(InteractObjectPacket {
                    object_id: 100,
                    action_type: 1,
                }),
                Opcode::InteractObject,
            ),
            (
                ClientPacket::AuthRequest(AuthRequestPacket {
                    token: "jwt.token.here".to_string(),
                    client_version: 1,
                }),
                Opcode::AuthRequest,
            ),
            (
                ClientPacket::AuthResponse(AuthResponsePacket {
                    success: true,
                    error_code: 0,
                    session_id: 1234,
                    player_guid: 5678,
                    assigned_node: "127.0.0.1:50051".to_string(),
                    error_message: String::new(),
                }),
                Opcode::AuthResponse,
            ),
            (
                ClientPacket::SocialMessage(SocialMessagePacket {
                    channel_type: 0,
                    target_id: 0,
                    sender_guid: 100,
                    sender_name: "Player".into(),
                    content: "Hello".into(),
                    timestamp: 1000,
                }),
                Opcode::SocialMessage,
            ),
            (
                ClientPacket::SocialSubscribe(SocialSubscribePacket {
                    channel_type: 2,
                    target_id: 123,
                    channel_name: "chat:guild:123".into(),
                }),
                Opcode::SocialSubscribe,
            ),
            (
                ClientPacket::SocialUnsubscribe(SocialUnsubscribePacket {
                    channel_type: 2,
                    target_id: 123,
                    channel_name: "chat:guild:123".into(),
                }),
                Opcode::SocialUnsubscribe,
            ),
            (
                ClientPacket::MarketListRequest(MarketListRequestPacket {
                    item_instance_id: "item-1".into(),
                    src_container_id: 10,
                    src_slot: 1,
                    price: 500,
                    duration_secs: 3600,
                }),
                Opcode::MarketListRequest,
            ),
            (
                ClientPacket::MarketBuyRequest(MarketBuyRequestPacket {
                    listing_id: "list-1".into(),
                    buyer_container_id: 20,
                    payment_container_id: 21,
                    price: 500,
                }),
                Opcode::MarketBuyRequest,
            ),
            (
                ClientPacket::MarketCancelRequest(MarketCancelRequestPacket {
                    listing_id: "list-1".into(),
                    dst_container_id: 10,
                }),
                Opcode::MarketCancelRequest,
            ),
            (
                ClientPacket::MarketQueryRequest(MarketQueryRequestPacket {
                    item_def_id: 42,
                    min_price: 100,
                    max_price: 1000,
                    offset: 0,
                    limit: 20,
                }),
                Opcode::MarketQueryRequest,
            ),
            (
                ClientPacket::MarketResponse(MarketResponsePacket {
                    request_type: 1,
                    success: true,
                    error_code: 0,
                    error_message: String::new(),
                    listings: vec![],
                }),
                Opcode::MarketResponse,
            ),
        ];

        for (pkt, expected_opcode) in packets {
            assert_eq!(pkt.opcode(), expected_opcode);
        }
    }

    #[test]
    fn test_world_object_spawn_roundtrip() {
        let mut props = HashMap::new();
        props.insert("name".to_string(), "Iron Door".to_string());
        props.insert("tier".to_string(), "2".to_string());

        let pkt = WorldObjectSpawnPacket {
            object_id: 9999,
            prefab_id: 42,
            pos: Vec3 {
                x: 12.5,
                y: 0.0,
                z: -34.5,
            },
            rot: (0.0, 0.5, 0.0, 0.5),
            scale: Vec3 {
                x: 1.0,
                y: 2.0,
                z: 1.0,
            },
            bounds_min: Vec3 {
                x: 11.5,
                y: 0.0,
                z: -35.5,
            },
            bounds_max: Vec3 {
                x: 13.5,
                y: 2.0,
                z: -33.5,
            },
            state_flags: 3,
            owner_type: 1,
            owner_id: "guild_123".to_string(),
            health: Some((250, 500)),
            properties: props,
            revision: 7,
        };

        let client_pkt = ClientPacket::WorldObjectSpawn(pkt.clone());
        let encoded = client_pkt.encode();
        let decoded = ClientPacket::decode(&encoded).expect("decode failed");

        if let ClientPacket::WorldObjectSpawn(decoded_pkt) = decoded {
            assert_eq!(decoded_pkt, pkt);
        } else {
            panic!("Decoded into wrong packet variant");
        }
    }

    #[test]
    fn test_world_object_delta_roundtrip() {
        let mut props = HashMap::new();
        props.insert("durability".to_string(), "85".to_string());

        let pkt = WorldObjectDeltaPacket {
            object_id: 54321,
            revision: 12,
            state_flags: Some(7),
            health: Some((85, 100)),
            properties: props,
        };

        let client_pkt = ClientPacket::WorldObjectDelta(pkt.clone());
        let encoded = client_pkt.encode();
        let decoded = ClientPacket::decode(&encoded).expect("decode failed");

        if let ClientPacket::WorldObjectDelta(decoded_pkt) = decoded {
            assert_eq!(decoded_pkt, pkt);
        } else {
            panic!("Decoded into wrong packet variant");
        }
    }

    #[test]
    fn test_world_object_despawn_and_resync_roundtrip() {
        let despawn = WorldObjectDespawnPacket { object_id: 777 };
        let encoded_despawn = ClientPacket::WorldObjectDespawn(despawn).encode();
        let decoded = ClientPacket::decode(&encoded_despawn).expect("decode failed");
        assert_eq!(decoded, ClientPacket::WorldObjectDespawn(despawn));

        let resync = WorldObjectResyncPacket { object_id: 888 };
        let encoded_resync = ClientPacket::WorldObjectResync(resync).encode();
        let decoded = ClientPacket::decode(&encoded_resync).expect("decode failed");
        assert_eq!(decoded, ClientPacket::WorldObjectResync(resync));
    }

    #[test]
    fn test_place_and_interact_object_roundtrip() {
        let mut props = HashMap::new();
        props.insert("material".to_string(), "stone".to_string());

        let place = PlaceObjectPacket {
            prefab_id: 5,
            pos: Vec3 {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            },
            rot: (0.0, 0.0, 0.0, 1.0),
            scale: Vec3 {
                x: 1.0,
                y: 1.0,
                z: 1.0,
            },
            properties: props,
        };
        let encoded_place = ClientPacket::PlaceObject(place.clone()).encode();
        let decoded_place = ClientPacket::decode(&encoded_place).expect("decode failed");
        assert_eq!(decoded_place, ClientPacket::PlaceObject(place));

        let interact = InteractObjectPacket {
            object_id: 999,
            action_type: 2,
        };
        let encoded_interact = ClientPacket::InteractObject(interact).encode();
        let decoded_interact = ClientPacket::decode(&encoded_interact).expect("decode failed");
        assert_eq!(decoded_interact, ClientPacket::InteractObject(interact));
    }

    #[test]
    fn test_world_object_packets_truncated_buffer_rejection() {
        let mut buf = BytesMut::new();
        put_varint_u64(&mut buf, 100);
        put_varint_u32(&mut buf, 1);
        // Truncate before floats
        assert!(WorldObjectSpawnPacket::decode(&mut buf.freeze()).is_err());

        let mut buf = BytesMut::new();
        put_varint_u64(&mut buf, 100);
        // Truncate before revision
        assert!(WorldObjectDeltaPacket::decode(&mut buf.freeze()).is_err());

        let mut buf = BytesMut::new();
        put_varint_u32(&mut buf, 1);
        // Truncate before position
        assert!(PlaceObjectPacket::decode(&mut buf.freeze()).is_err());
    }

    #[test]
    fn test_world_object_spawn_decoder_exact_boundaries() {
        // 1. Buffer length check: src.remaining() < 16 * 4 (64 bytes)
        // Header: object_id (1 byte varint), prefab_id (1 byte varint)
        let mut base = BytesMut::new();
        put_varint_u64(&mut base, 42);
        put_varint_u32(&mut base, 7);

        // Exactly 63 bytes remaining after header
        let mut buf_63 = base.clone();
        buf_63.put_slice(&[0u8; 63]);
        let err_63 = WorldObjectSpawnPacket::decode(&mut buf_63.freeze())
            .expect_err("should fail when remaining < 64");
        assert_eq!(
            err_63,
            ClientCodecError::BufferTooShort {
                expected: 64,
                got: 63
            }
        );

        // Exactly 64 bytes for pos/rot/scale/bounds, followed by state_flags
        let mut base_64 = base.clone();
        base_64.put_slice(&[0u8; 64]); // pos, rot, scale, bounds_min, bounds_max
        put_varint_u32(&mut base_64, 0); // state_flags

        // Test exact 0 bytes remaining before owner_type (< 1 check)
        let buf_no_owner_type = base_64.clone();
        let err_owner_type = WorldObjectSpawnPacket::decode(&mut buf_no_owner_type.freeze())
            .expect_err("should fail with 0 bytes before owner_type");
        assert_eq!(
            err_owner_type,
            ClientCodecError::BufferTooShort {
                expected: 1,
                got: 0
            }
        );

        // Provide owner_type and empty owner_id string
        let mut base_with_owner = base_64.clone();
        base_with_owner.put_u8(1); // owner_type
        put_string(&mut base_with_owner, ""); // owner_id (empty string varint len 0)

        // Test exact 0 bytes remaining before has_health (< 1 check)
        let buf_no_health = base_with_owner.clone();
        let err_health = WorldObjectSpawnPacket::decode(&mut buf_no_health.freeze())
            .expect_err("should fail with 0 bytes before has_health");
        assert_eq!(
            err_health,
            ClientCodecError::BufferTooShort {
                expected: 1,
                got: 0
            }
        );

        // Complete minimal valid packet: has_health = 0, empty properties, revision = 100
        let mut valid_buf = base_with_owner.clone();
        valid_buf.put_u8(0); // has_health = 0
        put_property_map(&mut valid_buf, &HashMap::new()); // empty props (0 count)
        put_varint_u64(&mut valid_buf, 100); // revision

        let decoded = WorldObjectSpawnPacket::decode(&mut valid_buf.freeze())
            .expect("should decode minimal valid packet");
        assert_eq!(decoded.object_id, 42);
        assert_eq!(decoded.prefab_id, 7);
        assert_eq!(decoded.owner_type, 1);
        assert_eq!(decoded.owner_id, "");
        assert_eq!(decoded.health, None);
        assert_eq!(decoded.revision, 100);
    }

    #[test]
    fn test_world_object_delta_decoder_exact_boundaries() {
        let mut base = BytesMut::new();
        put_varint_u64(&mut base, 55); // object_id
        put_varint_u64(&mut base, 12); // revision

        // 0 bytes remaining before has_flags (< 1 check)
        let buf_no_flags = base.clone();
        let err_flags = WorldObjectDeltaPacket::decode(&mut buf_no_flags.freeze())
            .expect_err("should fail with 0 bytes before has_flags");
        assert_eq!(
            err_flags,
            ClientCodecError::BufferTooShort {
                expected: 1,
                got: 0
            }
        );

        // Provide has_flags = 0
        let mut base_with_flags = base.clone();
        base_with_flags.put_u8(0);

        // 0 bytes remaining before has_health (< 1 check)
        let buf_no_health = base_with_flags.clone();
        let err_health = WorldObjectDeltaPacket::decode(&mut buf_no_health.freeze())
            .expect_err("should fail with 0 bytes before has_health");
        assert_eq!(
            err_health,
            ClientCodecError::BufferTooShort {
                expected: 1,
                got: 0
            }
        );

        // Provide has_health = 0 and empty properties map
        let mut valid_buf = base_with_flags.clone();
        valid_buf.put_u8(0); // has_health = 0
        put_property_map(&mut valid_buf, &HashMap::new());

        let decoded = WorldObjectDeltaPacket::decode(&mut valid_buf.freeze())
            .expect("should decode valid delta");
        assert_eq!(decoded.object_id, 55);
        assert_eq!(decoded.revision, 12);
        assert_eq!(decoded.state_flags, None);
        assert_eq!(decoded.health, None);
    }

    #[test]
    fn test_place_object_packet_decoder_boundary() {
        let mut base = BytesMut::new();
        put_varint_u32(&mut base, 10); // prefab_id

        // Exactly 39 bytes remaining (< 40 bytes)
        let mut buf_39 = base.clone();
        buf_39.put_slice(&[0u8; 39]);
        let err_39 =
            PlaceObjectPacket::decode(&mut buf_39.freeze()).expect_err("should fail with 39 bytes");
        assert_eq!(
            err_39,
            ClientCodecError::BufferTooShort {
                expected: 40,
                got: 39
            }
        );

        // Exactly 40 bytes + empty properties map succeeds
        let mut buf_40 = base.clone();
        buf_40.put_slice(&[0u8; 40]);
        put_property_map(&mut buf_40, &HashMap::new());

        let decoded = PlaceObjectPacket::decode(&mut buf_40.freeze())
            .expect("should decode with 40 bytes + props");
        assert_eq!(decoded.prefab_id, 10);
    }

    #[test]
    fn test_auth_request_packet_exact_boundaries() {
        // Test token length 0 with 4 bytes client_version (2 bytes for token_len + 4 bytes version)
        let mut zero_len_buf = BytesMut::new();
        zero_len_buf.put_u16_le(0); // token_len = 0
        zero_len_buf.put_u32_le(101); // client_version = 101
        let decoded_zero = AuthRequestPacket::decode(&mut zero_len_buf.freeze())
            .expect("token_len = 0 should succeed");
        assert_eq!(decoded_zero.token, "");
        assert_eq!(decoded_zero.client_version, 101);

        // Test token length 5 ("hello"): token_len + 4 = 9 bytes total payload
        // Exact payload length token_len + 4 succeeds
        let mut exact_buf = BytesMut::new();
        exact_buf.put_u16_le(5);
        exact_buf.put_slice(b"hello");
        exact_buf.put_u32_le(202);
        let decoded_exact = AuthRequestPacket::decode(&mut exact_buf.freeze())
            .expect("exact length should succeed");
        assert_eq!(decoded_exact.token, "hello");
        assert_eq!(decoded_exact.client_version, 202);

        // token_len + 3 bytes (1 byte short of client_version) fails BufferTooShort
        let mut short_buf = BytesMut::new();
        short_buf.put_u16_le(5);
        short_buf.put_slice(b"hello");
        short_buf.put_slice(&[1, 2, 3]); // only 3 bytes for client_version instead of 4
        let err_short = AuthRequestPacket::decode(&mut short_buf.freeze())
            .expect_err("token_len + 3 should fail BufferTooShort");
        assert_eq!(
            err_short,
            ClientCodecError::BufferTooShort {
                expected: 5 + 4,
                got: 5 + 3
            }
        );
    }

    #[test]
    fn test_auth_response_packet_exact_boundaries() {
        // MIN_SIZE is 25 bytes:
        // 1 (success) + 4 (error_code) + 8 (session_id) + 8 (player_guid) + 2 (node_len) + 2 (msg_len)
        let mut base_25 = BytesMut::new();
        base_25.put_u8(1); // success
        base_25.put_u32_le(0); // error_code
        base_25.put_u64_le(100); // session_id
        base_25.put_u64_le(200); // player_guid

        // Test assigned_node boundary: node_len = 4 ("node")
        let mut node_short = base_25.clone();
        node_short.put_u16_le(4);
        node_short.put_slice(b"nod"); // only 3 bytes
        let err_node = AuthResponsePacket::decode(&mut node_short.freeze())
            .expect_err("assigned_node length mismatch should fail");
        assert_eq!(
            err_node,
            ClientCodecError::BufferTooShort {
                expected: 4,
                got: 3
            }
        );

        // Test error_message boundary:
        let mut with_node = base_25.clone();
        with_node.put_u16_le(4);
        with_node.put_slice(b"node");

        // Exactly 1 byte remaining after node (needs 2 for msg_len u16)
        let mut short_msg_len = with_node.clone();
        short_msg_len.put_u8(0);
        let err_msg_len = AuthResponsePacket::decode(&mut short_msg_len.freeze())
            .expect_err("1 byte remaining for msg_len should fail");
        assert_eq!(
            err_msg_len,
            ClientCodecError::BufferTooShort {
                expected: 2,
                got: 1
            }
        );

        // msg_len = 5 ("error"), but only 4 bytes remaining
        let mut msg_short = with_node.clone();
        msg_short.put_u16_le(5);
        msg_short.put_slice(b"erro"); // only 4 bytes
        let err_msg = AuthResponsePacket::decode(&mut msg_short.freeze())
            .expect_err("error_message length mismatch should fail");
        assert_eq!(
            err_msg,
            ClientCodecError::BufferTooShort {
                expected: 5,
                got: 4
            }
        );

        // Exact match succeeds
        let mut exact_res = with_node.clone();
        exact_res.put_u16_le(5);
        exact_res.put_slice(b"error");
        let decoded_res = AuthResponsePacket::decode(&mut exact_res.freeze())
            .expect("exact lengths should succeed");
        assert_eq!(decoded_res.assigned_node, "node");
        assert_eq!(decoded_res.error_message, "error");
    }

    #[test]
    fn test_auth_packets_roundtrip() {
        let auth_req = AuthRequestPacket {
            token: "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.e30.t-IDcSemACt8x4iTMCda8Yhe3iZaWbvV5XKSTbuAn0M".to_string(),
            client_version: 42,
        };
        let packet_req = ClientPacket::AuthRequest(auth_req.clone());
        let encoded_req = packet_req.encode();
        let decoded_req = ClientPacket::decode(&encoded_req).expect("decode auth req");
        assert_eq!(decoded_req, packet_req);

        let auth_res_success = AuthResponsePacket {
            success: true,
            error_code: auth_error_codes::SUCCESS,
            session_id: 1001,
            player_guid: 2002,
            assigned_node: "127.0.0.1:50051".to_string(),
            error_message: String::new(),
        };
        let packet_res = ClientPacket::AuthResponse(auth_res_success.clone());
        let encoded_res = packet_res.encode();
        let decoded_res = ClientPacket::decode(&encoded_res).expect("decode auth res");
        assert_eq!(decoded_res, packet_res);

        let auth_res_fail = AuthResponsePacket {
            success: false,
            error_code: auth_error_codes::EXPIRED_TOKEN,
            session_id: 0,
            player_guid: 0,
            assigned_node: String::new(),
            error_message: "Token has expired".to_string(),
        };
        let packet_fail = ClientPacket::AuthResponse(auth_res_fail.clone());
        let encoded_fail = packet_fail.encode();
        let decoded_fail = ClientPacket::decode(&encoded_fail).expect("decode auth fail");
        assert_eq!(decoded_fail, packet_fail);
    }

    #[test]
    fn test_auth_packets_truncated_buffer_rejection() {
        let mut buf = BytesMut::new();
        buf.put_u16_le(10); // claims 10 bytes of token
        buf.put_slice(b"short"); // only 5 bytes
        assert!(AuthRequestPacket::decode(&mut buf.freeze()).is_err());

        let mut buf2 = BytesMut::new();
        buf2.put_u8(1); // success
        buf2.put_u32_le(0); // error code
        buf2.put_u64_le(1); // session_id
                            // Truncated before player_guid
        assert!(AuthResponsePacket::decode(&mut buf2.freeze()).is_err());
    }

    #[test]
    fn test_social_packets_roundtrip() {
        let msg = SocialMessagePacket {
            channel_type: 4,
            target_id: 200,
            sender_guid: 100,
            sender_name: "Alice".into(),
            content: "Whisper text".into(),
            timestamp: 12345,
        };
        let pkt = ClientPacket::SocialMessage(msg);
        let encoded = pkt.encode();
        let decoded = ClientPacket::decode(&encoded).expect("decode SocialMessage");
        assert_eq!(decoded, pkt);

        let sub = SocialSubscribePacket {
            channel_type: 2,
            target_id: 50,
            channel_name: "chat:guild:50".into(),
        };
        let pkt = ClientPacket::SocialSubscribe(sub);
        let encoded = pkt.encode();
        let decoded = ClientPacket::decode(&encoded).expect("decode SocialSubscribe");
        assert_eq!(decoded, pkt);

        let unsub = SocialUnsubscribePacket {
            channel_type: 2,
            target_id: 50,
            channel_name: "chat:guild:50".into(),
        };
        let pkt = ClientPacket::SocialUnsubscribe(unsub);
        let encoded = pkt.encode();
        let decoded = ClientPacket::decode(&encoded).expect("decode SocialUnsubscribe");
        assert_eq!(decoded, pkt);
    }

    #[test]
    fn test_social_packets_truncated_buffer_rejection() {
        let mut buf = BytesMut::new();
        buf.put_u8(0); // channel_type
        buf.put_u64_le(0); // target_id
        buf.put_u64_le(100); // sender_guid
        buf.put_u16_le(10); // claims 10 bytes name
        buf.put_slice(b"short"); // only 5 bytes
        assert!(SocialMessagePacket::decode(&mut buf.freeze()).is_err());

        let mut buf2 = BytesMut::new();
        buf2.put_u8(0);
        buf2.put_u64_le(1);
        buf2.put_u16_le(8); // claims 8 bytes channel_name
        buf2.put_slice(b"abc"); // only 3 bytes
        assert!(SocialSubscribePacket::decode(&mut buf2.freeze()).is_err());
    }

    #[test]
    fn test_market_packets_roundtrip() {
        let list_req = MarketListRequestPacket {
            item_instance_id: "item-xyz".into(),
            src_container_id: 101,
            src_slot: 3,
            price: 750,
            duration_secs: 7200,
        };
        let pkt = ClientPacket::MarketListRequest(list_req);
        let encoded = pkt.encode();
        let decoded = ClientPacket::decode(&encoded).expect("decode MarketListRequest");
        assert_eq!(decoded, pkt);

        let buy_req = MarketBuyRequestPacket {
            listing_id: "listing-42".into(),
            buyer_container_id: 102,
            payment_container_id: 103,
            price: 750,
        };
        let pkt = ClientPacket::MarketBuyRequest(buy_req);
        let encoded = pkt.encode();
        let decoded = ClientPacket::decode(&encoded).expect("decode MarketBuyRequest");
        assert_eq!(decoded, pkt);

        let cancel_req = MarketCancelRequestPacket {
            listing_id: "listing-42".into(),
            dst_container_id: 101,
        };
        let pkt = ClientPacket::MarketCancelRequest(cancel_req);
        let encoded = pkt.encode();
        let decoded = ClientPacket::decode(&encoded).expect("decode MarketCancelRequest");
        assert_eq!(decoded, pkt);

        let query_req = MarketQueryRequestPacket {
            item_def_id: 99,
            min_price: 100,
            max_price: 1000,
            offset: 10,
            limit: 25,
        };
        let pkt = ClientPacket::MarketQueryRequest(query_req);
        let encoded = pkt.encode();
        let decoded = ClientPacket::decode(&encoded).expect("decode MarketQueryRequest");
        assert_eq!(decoded, pkt);

        let wire_listing = MarketListingWire {
            listing_id: "listing-42".into(),
            seller_guid: 1001,
            item_instance_id: "item-xyz".into(),
            item_def_id: 99,
            price: 750,
            created_at: 10000,
            expires_at: 17200,
        };
        let resp = MarketResponsePacket {
            request_type: 4,
            success: true,
            error_code: market_error_codes::SUCCESS,
            error_message: "OK".into(),
            listings: vec![wire_listing],
        };
        let pkt = ClientPacket::MarketResponse(resp);
        let encoded = pkt.encode();
        let decoded = ClientPacket::decode(&encoded).expect("decode MarketResponse");
        assert_eq!(decoded, pkt);
    }

    #[test]
    fn test_market_packets_truncated_buffer_rejection() {
        let mut buf = BytesMut::new();
        buf.put_u16_le(20); // claims 20 bytes listing_id
        buf.put_slice(b"short");
        assert!(MarketBuyRequestPacket::decode(&mut buf.freeze()).is_err());

        let mut buf2 = BytesMut::new();
        buf2.put_u8(1);
        buf2.put_u8(1);
        buf2.put_u32_le(0);
        buf2.put_u16_le(2);
        buf2.put_slice(b"OK");
        buf2.put_u16_le(5); // claims 5 listings
                            // No listing bytes follow
        assert!(MarketResponsePacket::decode(&mut buf2.freeze()).is_err());
    }

    #[test]
    fn test_vec3_normalized_exact_boundary() {
        // len > 1e-6 returns Some; len <= 1e-6 returns None
        let at_boundary = Vec3::new(1e-6, 0.0, 0.0);
        assert_eq!(at_boundary.length(), 1e-6);
        assert!(at_boundary.normalized().is_none());

        let just_above = Vec3::new(1.0001e-6, 0.0, 0.0);
        assert!(just_above.normalized().is_some());

        // Verify exact component division (3-4-0 and 2-3-6 vectors)
        let v = Vec3::new(3.0, 4.0, 0.0);
        let n = v.normalized().unwrap();
        assert!((n.x - 0.6).abs() < 1e-6);
        assert!((n.y - 0.8).abs() < 1e-6);
        assert!((n.z - 0.0).abs() < 1e-6);

        let v3 = Vec3::new(2.0, 3.0, 6.0);
        let n3 = v3.normalized().unwrap();
        assert!((n3.x - (2.0 / 7.0)).abs() < 1e-6);
        assert!((n3.y - (3.0 / 7.0)).abs() < 1e-6);
        assert!((n3.z - (6.0 / 7.0)).abs() < 1e-6);
    }

    #[test]
    fn test_lag_compensation_payload_unit() {
        let payload = LagCompensationPayload {
            client_timestamp: 123456789,
            origin: Vec3::new(10.0, 20.0, 30.0),
            direction: Vec3::new(0.0, 1.0, 0.0),
            max_distance: 50.0,
            sequence: 42,
        };
        let bytes = payload.encode_to_bytes();
        assert_eq!(bytes.len(), LagCompensationPayload::SIZE);
        let mut slice = bytes.clone();
        let decoded = LagCompensationPayload::decode(&mut slice).expect("valid decode");
        assert_eq!(decoded.client_timestamp, payload.client_timestamp);
        assert_eq!(decoded.origin, payload.origin);
        assert_eq!(decoded.direction, payload.direction);
        assert_eq!(decoded.max_distance, payload.max_distance);
        assert_eq!(decoded.sequence, payload.sequence);
        assert_eq!(
            decoded.normalized_direction(),
            Some(Vec3::new(0.0, 1.0, 0.0))
        );

        // Truncated buffer
        let mut short = bytes.slice(0..LagCompensationPayload::SIZE - 1);
        let err = LagCompensationPayload::decode(&mut short).unwrap_err();
        assert_eq!(
            err,
            ClientCodecError::BufferTooShort {
                expected: LagCompensationPayload::SIZE,
                got: LagCompensationPayload::SIZE - 1,
            }
        );

        // Direction length exactly 1e-6: direction.length() < 1e-6 is false, so it succeeds.
        // If mutated to <= 1e-6, it would fail.
        let at_limit = LagCompensationPayload {
            client_timestamp: 100,
            origin: Vec3::new(0.0, 0.0, 0.0),
            direction: Vec3::new(1e-6, 0.0, 0.0),
            max_distance: 10.0,
            sequence: 1,
        };
        let at_limit_bytes = at_limit.encode_to_bytes();
        let mut at_limit_slice = at_limit_bytes.clone();
        assert!(LagCompensationPayload::decode(&mut at_limit_slice).is_ok());

        // Below 1e-6 fails
        let below_limit = LagCompensationPayload {
            client_timestamp: 100,
            origin: Vec3::new(0.0, 0.0, 0.0),
            direction: Vec3::new(0.5e-6, 0.0, 0.0),
            max_distance: 10.0,
            sequence: 1,
        };
        let mut below_slice = below_limit.encode_to_bytes();
        assert!(LagCompensationPayload::decode(&mut below_slice).is_err());

        // Non-finite origin with finite direction
        let mut nan_origin = payload.clone();
        nan_origin.origin = Vec3::new(f32::NAN, 0.0, 0.0);
        assert!(LagCompensationPayload::decode(&mut nan_origin.encode_to_bytes()).is_err());

        // Finite origin with non-finite direction
        let mut nan_dir = payload.clone();
        nan_dir.direction = Vec3::new(0.0, f32::NAN, 0.0);
        assert!(LagCompensationPayload::decode(&mut nan_dir.encode_to_bytes()).is_err());

        // Non-positive max_distance with finite values
        let mut zero_dist = payload.clone();
        zero_dist.max_distance = 0.0;
        assert!(LagCompensationPayload::decode(&mut zero_dist.encode_to_bytes()).is_err());

        let mut neg_dist = payload.clone();
        neg_dist.max_distance = -5.0;
        assert!(LagCompensationPayload::decode(&mut neg_dist.encode_to_bytes()).is_err());

        // NaN max_distance
        let mut nan_dist = payload.clone();
        nan_dist.max_distance = f32::NAN;
        assert!(LagCompensationPayload::decode(&mut nan_dist.encode_to_bytes()).is_err());
    }

    #[test]
    fn test_entity_delta_state_attribute_boundaries() {
        // Line 947: attr_len > 256. attr_len == 256 succeeds, attr_len == 257 fails.
        let mut buf = BytesMut::new();
        put_varint_u64(&mut buf, 1);
        buf.put_u16_le(delta_flags::MASK_ATTRIBUTES);
        put_varint_u32(&mut buf, 256);
        for i in 0..256 {
            put_varint_u32(&mut buf, i);
            put_varint_u32(&mut buf, 10);
        }
        let decoded =
            EntityDeltaState::decode(&mut buf.freeze()).expect("256 attributes should succeed");
        assert_eq!(decoded.attributes.len(), 256);

        let mut buf_over = BytesMut::new();
        put_varint_u64(&mut buf_over, 1);
        buf_over.put_u16_le(delta_flags::MASK_ATTRIBUTES);
        put_varint_u32(&mut buf_over, 257);
        for i in 0..257 {
            put_varint_u32(&mut buf_over, i);
            put_varint_u32(&mut buf_over, 10);
        }
        assert!(EntityDeltaState::decode(&mut buf_over.freeze()).is_err());

        // Line 956: attr_id > u16::MAX as u32. attr_id == u16::MAX as u32 succeeds, u16::MAX + 1 fails.
        let mut buf_max = BytesMut::new();
        put_varint_u64(&mut buf_max, 1);
        buf_max.put_u16_le(delta_flags::MASK_ATTRIBUTES);
        put_varint_u32(&mut buf_max, 1);
        put_varint_u32(&mut buf_max, u16::MAX as u32);
        put_varint_u32(&mut buf_max, 42);
        let decoded_max = EntityDeltaState::decode(&mut buf_max.freeze())
            .expect("attr_id == u16::MAX should succeed");
        assert_eq!(decoded_max.attributes[0].attribute_id, u16::MAX);

        let mut buf_exceed = BytesMut::new();
        put_varint_u64(&mut buf_exceed, 1);
        buf_exceed.put_u16_le(delta_flags::MASK_ATTRIBUTES);
        put_varint_u32(&mut buf_exceed, 1);
        put_varint_u32(&mut buf_exceed, u16::MAX as u32 + 1);
        put_varint_u32(&mut buf_exceed, 42);
        assert!(EntityDeltaState::decode(&mut buf_exceed.freeze()).is_err());
    }

    #[test]
    fn test_world_object_spawn_and_delta_exact_boundaries() {
        // Line 1155: src.remaining() < 16 * 4 (64)
        let mut buf_64 = BytesMut::new();
        put_varint_u64(&mut buf_64, 100); // object_id
        put_varint_u32(&mut buf_64, 1); // prefab_id
        for _ in 0..16 {
            buf_64.put_f32_le(0.0);
        }
        // At line 1155, remaining is 64. Passes line 1155.
        // At line 1187 state_flags fails or state_flags is missing:
        let err = WorldObjectSpawnPacket::decode(&mut buf_64.clone().freeze()).unwrap_err();
        assert_eq!(
            err,
            ClientCodecError::BufferTooShort {
                expected: 1,
                got: 0
            }
        );

        // Add state_flags (varint 0) + 1 byte owner_type + owner_id (0) + 1 byte has_health (0) + properties (0) + revision (1)
        let mut buf_valid = buf_64.clone();
        put_varint_u32(&mut buf_valid, 0); // state_flags
        buf_valid.put_u8(1); // owner_type (line 1188 check)
        put_string(&mut buf_valid, ""); // owner_id
        buf_valid.put_u8(0); // has_health (line 1196 check)
        put_varint_u32(&mut buf_valid, 0); // properties len
        put_varint_u64(&mut buf_valid, 1); // revision
        let decoded =
            WorldObjectSpawnPacket::decode(&mut buf_valid.freeze()).expect("valid decode");
        assert_eq!(decoded.object_id, 100);

        // WorldObjectDeltaPacket line 1276: src.remaining() < 1 for has_health
        let mut buf_delta = BytesMut::new();
        put_varint_u64(&mut buf_delta, 100);
        put_varint_u64(&mut buf_delta, 1); // revision
        buf_delta.put_u8(0); // has_flags = 0
        buf_delta.put_u8(0); // has_health = 0
        buf_delta.put_u8(0); // has_position = 0
        buf_delta.put_u8(0); // has_rotation = 0
        buf_delta.put_u8(0); // has_scale = 0
        buf_delta.put_u8(0); // has_bounds = 0
        buf_delta.put_u8(0); // has_owner = 0
        buf_delta.put_u8(0); // has_properties = 0
        let decoded_delta =
            WorldObjectDeltaPacket::decode(&mut buf_delta.freeze()).expect("valid delta");
        assert_eq!(decoded_delta.object_id, 100);

        // PlaceObjectPacket line 1364: src.remaining() < 10 * 4 (40)
        let mut buf_place = BytesMut::new();
        put_varint_u32(&mut buf_place, 5); // prefab_id
        for _ in 0..10 {
            buf_place.put_f32_le(0.0);
        }
        put_varint_u32(&mut buf_place, 0); // properties len
        let decoded_place =
            PlaceObjectPacket::decode(&mut buf_place.freeze()).expect("valid place object");
        assert_eq!(decoded_place.prefab_id, 5);

        // AuthRequestPacket line 1451: src.remaining() < 2
        let mut buf_auth_req = BytesMut::new();
        buf_auth_req.put_u16_le(0); // token_len = 0
                                    // 0 bytes follow, requires token_len + 4 = 4
        let err_auth_req = AuthRequestPacket::decode(&mut buf_auth_req.freeze()).unwrap_err();
        assert_eq!(
            err_auth_req,
            ClientCodecError::BufferTooShort {
                expected: 4,
                got: 0
            }
        );

        // AuthResponsePacket line 1503: src.remaining() < 25 (exact MIN_SIZE 25)
        let mut buf_auth_res = BytesMut::new();
        buf_auth_res.put_u8(1); // success
        buf_auth_res.put_u32_le(0); // error_code
        buf_auth_res.put_u64_le(10); // session_id
        buf_auth_res.put_u64_le(20); // player_guid
        buf_auth_res.put_u16_le(0); // node_len = 0
        buf_auth_res.put_u16_le(0); // msg_len = 0
        assert_eq!(buf_auth_res.len(), 25);
        let decoded_auth_res =
            AuthResponsePacket::decode(&mut buf_auth_res.freeze()).expect("valid auth res");
        assert_eq!(decoded_auth_res.session_id, 10);
    }

    #[test]
    fn test_social_packets_exact_boundaries() {
        // Line 1580: SocialMessagePacket MIN_SIZE = 29
        let mut buf_msg = BytesMut::new();
        buf_msg.put_u8(1); // channel_type
        buf_msg.put_u64_le(10); // target_id
        buf_msg.put_u64_le(20); // sender_guid
        buf_msg.put_u16_le(0); // name_len = 0
        buf_msg.put_u16_le(0); // content_len = 0
        buf_msg.put_u64_le(999); // timestamp
        assert_eq!(buf_msg.len(), 29);
        let decoded_msg =
            SocialMessagePacket::decode(&mut buf_msg.clone().freeze()).expect("exact 29 bytes");
        assert_eq!(decoded_msg.sender_guid, 20);

        // Buffer with 28 bytes fails line 1580
        let err_28 =
            SocialMessagePacket::decode(&mut buf_msg.clone().freeze().slice(0..28)).unwrap_err();
        assert_eq!(
            err_28,
            ClientCodecError::BufferTooShort {
                expected: 29,
                got: 28
            }
        );

        // Line 1590: src.remaining() < name_len (requires >= 29 bytes for line 1580)
        let mut buf_name = BytesMut::new();
        buf_name.put_u8(1);
        buf_name.put_u64_le(10);
        buf_name.put_u64_le(20);
        buf_name.put_u16_le(10); // name_len = 10
        buf_name.put_slice(b"0123456789"); // exactly 10 bytes name (total 29 bytes)
        let err_name = SocialMessagePacket::decode(&mut buf_name.freeze()).unwrap_err();
        assert_eq!(
            err_name,
            ClientCodecError::BufferTooShort {
                expected: 2,
                got: 0
            }
        );

        let mut buf_name_short = BytesMut::new();
        buf_name_short.put_u8(1);
        buf_name_short.put_u64_le(10);
        buf_name_short.put_u64_le(20);
        buf_name_short.put_u16_le(11); // name_len = 11
        buf_name_short.put_slice(b"0123456789"); // 10 bytes name (total 29 bytes)
        let err_name_short = SocialMessagePacket::decode(&mut buf_name_short.freeze()).unwrap_err();
        assert_eq!(
            err_name_short,
            ClientCodecError::BufferTooShort {
                expected: 11,
                got: 10
            }
        );

        // Line 1600: src.remaining() < 2 (requires >= 29 bytes for line 1580, exactly 2 bytes remaining at line 1600)
        let mut buf_content_len = BytesMut::new();
        buf_content_len.put_u8(1);
        buf_content_len.put_u64_le(10);
        buf_content_len.put_u64_le(20);
        buf_content_len.put_u16_le(10); // name_len = 10
        buf_content_len.put_slice(b"0123456789"); // 10 bytes name (total 29 bytes)
        buf_content_len.put_u16_le(0); // content_len = 0 (exactly 2 bytes remaining, total 31 bytes)
        let err_content_len =
            SocialMessagePacket::decode(&mut buf_content_len.freeze()).unwrap_err();
        assert_eq!(
            err_content_len,
            ClientCodecError::BufferTooShort {
                expected: 8,
                got: 0
            }
        );

        // Lines 1607 & 1609: src.remaining() < content_len + 8
        let mut buf_content = BytesMut::new();
        buf_content.put_u8(1);
        buf_content.put_u64_le(10);
        buf_content.put_u64_le(20);
        buf_content.put_u16_le(0);
        buf_content.put_u16_le(3); // content_len = 3
        buf_content.put_slice(b"abc"); // 3 bytes
        buf_content.put_slice(&[0u8; 7]); // only 7 bytes timestamp (needs 8)
                                          // Remaining is 3 + 7 = 10 bytes. Expected: 3 + 8 = 11.
        let err_content = SocialMessagePacket::decode(&mut buf_content.freeze()).unwrap_err();
        assert_eq!(
            err_content,
            ClientCodecError::BufferTooShort {
                expected: 11,
                got: 10
            }
        );

        // SocialSubscribePacket line 1647: MIN_SIZE = 11
        let mut buf_sub = BytesMut::new();
        buf_sub.put_u8(1);
        buf_sub.put_u64_le(10);
        buf_sub.put_u16_le(0); // name_len = 0
        assert_eq!(buf_sub.len(), 11);
        let decoded_sub =
            SocialSubscribePacket::decode(&mut buf_sub.clone().freeze()).expect("exact 11 bytes");
        assert_eq!(decoded_sub.channel_name, "");
        let err_sub_10 =
            SocialSubscribePacket::decode(&mut buf_sub.clone().freeze().slice(0..10)).unwrap_err();
        assert_eq!(
            err_sub_10,
            ClientCodecError::BufferTooShort {
                expected: 11,
                got: 10
            }
        );

        // SocialSubscribePacket line 1656: src.remaining() < name_len
        let mut buf_sub_name = BytesMut::new();
        buf_sub_name.put_u8(1);
        buf_sub_name.put_u64_le(10);
        buf_sub_name.put_u16_le(4);
        buf_sub_name.put_slice(b"tes"); // only 3 bytes
        let err_sub_name = SocialSubscribePacket::decode(&mut buf_sub_name.freeze()).unwrap_err();
        assert_eq!(
            err_sub_name,
            ClientCodecError::BufferTooShort {
                expected: 4,
                got: 3
            }
        );

        // SocialUnsubscribePacket lines 1691 & 1700
        let mut buf_unsub = BytesMut::new();
        buf_unsub.put_u8(1);
        buf_unsub.put_u64_le(10);
        buf_unsub.put_u16_le(0);
        assert_eq!(buf_unsub.len(), 11);
        let decoded_unsub = SocialUnsubscribePacket::decode(&mut buf_unsub.clone().freeze())
            .expect("exact 11 bytes");
        assert_eq!(decoded_unsub.channel_name, "");
        let err_unsub_10 =
            SocialUnsubscribePacket::decode(&mut buf_unsub.clone().freeze().slice(0..10))
                .unwrap_err();
        assert_eq!(
            err_unsub_10,
            ClientCodecError::BufferTooShort {
                expected: 11,
                got: 10
            }
        );

        let mut buf_unsub_name = BytesMut::new();
        buf_unsub_name.put_u8(1);
        buf_unsub_name.put_u64_le(10);
        buf_unsub_name.put_u16_le(4);
        buf_unsub_name.put_slice(b"tes");
        let err_unsub_name =
            SocialUnsubscribePacket::decode(&mut buf_unsub_name.freeze()).unwrap_err();
        assert_eq!(
            err_unsub_name,
            ClientCodecError::BufferTooShort {
                expected: 4,
                got: 3
            }
        );
    }

    #[test]
    fn test_market_packets_exact_boundaries() {
        // MarketListingWire line 1758: src.remaining() < 2
        let mut buf_wire = BytesMut::new();
        buf_wire.put_u16_le(0); // lid_len = 0
                                // Remaining: 0. Expected: lid_len + 10 = 10
        let err_wire = MarketListingWire::decode(&mut buf_wire.freeze()).unwrap_err();
        assert_eq!(
            err_wire,
            ClientCodecError::BufferTooShort {
                expected: 10,
                got: 0
            }
        );

        // MarketListingWire line 1765 & 1767: lid_len + 10
        let mut buf_wire2 = BytesMut::new();
        buf_wire2.put_u16_le(4); // lid_len = 4
        buf_wire2.put_slice(b"list");
        buf_wire2.put_u64_le(100); // seller_guid (8 bytes)
        buf_wire2.put_u8(0); // only 1 byte for iid_len
                             // Remaining: 13 bytes. Expected: 4 + 10 = 14
        let err_wire2 = MarketListingWire::decode(&mut buf_wire2.freeze()).unwrap_err();
        assert_eq!(
            err_wire2,
            ClientCodecError::BufferTooShort {
                expected: 14,
                got: 13
            }
        );

        // MarketListingWire line 1765 exact boundary (< vs <=)
        // With remaining == lid_len + 10 (0 + 10 = 10):
        // '<' is false, so it consumes lid, seller_guid, and iid_len, failing at line 1776 with BufferTooShort { expected: 28, got: 0 }.
        // '<=' is true, so it prematurely fails at line 1765 with BufferTooShort { expected: 10, got: 10 }.
        let mut buf_wire_exact_boundary = BytesMut::new();
        buf_wire_exact_boundary.put_u16_le(0); // lid_len = 0
        buf_wire_exact_boundary.put_u64_le(100); // seller_guid (8 bytes)
        buf_wire_exact_boundary.put_u16_le(0); // iid_len = 0 (2 bytes)
        let err_wire_boundary =
            MarketListingWire::decode(&mut buf_wire_exact_boundary.freeze()).unwrap_err();
        assert_eq!(
            err_wire_boundary,
            ClientCodecError::BufferTooShort {
                expected: 28,
                got: 0
            }
        );

        // MarketListingWire line 1776 & 1778: iid_len + 28
        let mut buf_wire3 = BytesMut::new();
        buf_wire3.put_u16_le(0); // lid_len = 0
        buf_wire3.put_u64_le(100); // seller_guid
        buf_wire3.put_u16_le(2); // iid_len = 2
        buf_wire3.put_slice(b"it");
        buf_wire3.put_slice(&[0u8; 27]); // 27 bytes (1 byte short of 28)
                                         // Remaining: 2 + 27 = 29. Expected: 2 + 28 = 30.
        let err_wire3 = MarketListingWire::decode(&mut buf_wire3.freeze()).unwrap_err();
        assert_eq!(
            err_wire3,
            ClientCodecError::BufferTooShort {
                expected: 30,
                got: 29
            }
        );

        // Exact wire listing decode
        let mut buf_wire_exact = BytesMut::new();
        buf_wire_exact.put_u16_le(0);
        buf_wire_exact.put_u64_le(100);
        buf_wire_exact.put_u16_le(0);
        buf_wire_exact.put_u32_le(1);
        buf_wire_exact.put_u64_le(10);
        buf_wire_exact.put_u64_le(1000);
        buf_wire_exact.put_u64_le(2000);
        let decoded_wire =
            MarketListingWire::decode(&mut buf_wire_exact.freeze()).expect("valid wire listing");
        assert_eq!(decoded_wire.seller_guid, 100);

        // MarketListRequestPacket lines 1824 & 1831: iid_len + 28
        let mut buf_list = BytesMut::new();
        buf_list.put_u16_le(0); // iid_len = 0
        let err_list = MarketListRequestPacket::decode(&mut buf_list.freeze()).unwrap_err();
        assert_eq!(
            err_list,
            ClientCodecError::BufferTooShort {
                expected: 28,
                got: 0
            }
        );

        let mut buf_list2 = BytesMut::new();
        buf_list2.put_u16_le(2);
        buf_list2.put_slice(b"it");
        buf_list2.put_slice(&[0u8; 27]); // 29 bytes. Expected: 2 + 28 = 30
        let err_list2 = MarketListRequestPacket::decode(&mut buf_list2.freeze()).unwrap_err();
        assert_eq!(
            err_list2,
            ClientCodecError::BufferTooShort {
                expected: 30,
                got: 29
            }
        );

        let mut buf_list_exact = BytesMut::new();
        buf_list_exact.put_u16_le(0);
        buf_list_exact.put_u64_le(1);
        buf_list_exact.put_u32_le(0);
        buf_list_exact.put_u64_le(50);
        buf_list_exact.put_u64_le(3600);
        let decoded_list =
            MarketListRequestPacket::decode(&mut buf_list_exact.freeze()).expect("valid list req");
        assert_eq!(decoded_list.price, 50);

        // MarketBuyRequestPacket lines 1875 & 1882: lid_len + 24
        let mut buf_buy = BytesMut::new();
        buf_buy.put_u16_le(0); // lid_len = 0
        let err_buy = MarketBuyRequestPacket::decode(&mut buf_buy.freeze()).unwrap_err();
        assert_eq!(
            err_buy,
            ClientCodecError::BufferTooShort {
                expected: 24,
                got: 0
            }
        );

        let mut buf_buy2 = BytesMut::new();
        buf_buy2.put_u16_le(3);
        buf_buy2.put_slice(b"lid");
        buf_buy2.put_slice(&[0u8; 23]); // 26 bytes. Expected: 3 + 24 = 27
        let err_buy2 = MarketBuyRequestPacket::decode(&mut buf_buy2.freeze()).unwrap_err();
        assert_eq!(
            err_buy2,
            ClientCodecError::BufferTooShort {
                expected: 27,
                got: 26
            }
        );

        let mut buf_buy_exact = BytesMut::new();
        buf_buy_exact.put_u16_le(0);
        buf_buy_exact.put_u64_le(10);
        buf_buy_exact.put_u64_le(20);
        buf_buy_exact.put_u64_le(100);
        let decoded_buy =
            MarketBuyRequestPacket::decode(&mut buf_buy_exact.freeze()).expect("valid buy req");
        assert_eq!(decoded_buy.price, 100);

        // MarketCancelRequestPacket lines 1920 & 1927: lid_len + 8
        let mut buf_cancel = BytesMut::new();
        buf_cancel.put_u16_le(0);
        let err_cancel = MarketCancelRequestPacket::decode(&mut buf_cancel.freeze()).unwrap_err();
        assert_eq!(
            err_cancel,
            ClientCodecError::BufferTooShort {
                expected: 8,
                got: 0
            }
        );

        let mut buf_cancel2 = BytesMut::new();
        buf_cancel2.put_u16_le(2);
        buf_cancel2.put_slice(b"id");
        buf_cancel2.put_slice(&[0u8; 7]); // 9 bytes. Expected: 2 + 8 = 10
        let err_cancel2 = MarketCancelRequestPacket::decode(&mut buf_cancel2.freeze()).unwrap_err();
        assert_eq!(
            err_cancel2,
            ClientCodecError::BufferTooShort {
                expected: 10,
                got: 9
            }
        );

        let mut buf_cancel_exact = BytesMut::new();
        buf_cancel_exact.put_u16_le(0);
        buf_cancel_exact.put_u64_le(5);
        let decoded_cancel = MarketCancelRequestPacket::decode(&mut buf_cancel_exact.freeze())
            .expect("valid cancel req");
        assert_eq!(decoded_cancel.dst_container_id, 5);

        // MarketQueryRequestPacket line 1965: 28 bytes
        let mut buf_query = BytesMut::new();
        buf_query.put_slice(&[0u8; 27]);
        let err_query = MarketQueryRequestPacket::decode(&mut buf_query.freeze()).unwrap_err();
        assert_eq!(
            err_query,
            ClientCodecError::BufferTooShort {
                expected: 28,
                got: 27
            }
        );

        let mut buf_query_exact = BytesMut::new();
        buf_query_exact.put_u32_le(10);
        buf_query_exact.put_u64_le(0);
        buf_query_exact.put_u64_le(500);
        buf_query_exact.put_u32_le(0);
        buf_query_exact.put_u32_le(20);
        assert_eq!(buf_query_exact.len(), 28);
        let decoded_query = MarketQueryRequestPacket::decode(&mut buf_query_exact.freeze())
            .expect("valid query req");
        assert_eq!(decoded_query.limit, 20);

        // MarketResponsePacket lines 2012 & 2022
        let mut buf_resp = BytesMut::new();
        buf_resp.put_slice(&[0u8; 9]);
        let err_resp = MarketResponsePacket::decode(&mut buf_resp.freeze()).unwrap_err();
        assert_eq!(
            err_resp,
            ClientCodecError::BufferTooShort {
                expected: 10,
                got: 9
            }
        );

        let mut buf_resp_exact = BytesMut::new();
        buf_resp_exact.put_u8(1); // request_type
        buf_resp_exact.put_u8(1); // success
        buf_resp_exact.put_u32_le(0); // error_code
        buf_resp_exact.put_u16_le(0); // msg_len = 0
        buf_resp_exact.put_u16_le(0); // listings = 0
        assert_eq!(buf_resp_exact.len(), 10);
        let decoded_resp =
            MarketResponsePacket::decode(&mut buf_resp_exact.freeze()).expect("valid resp");
        assert_eq!(decoded_resp.request_type, 1);

        let mut buf_resp_msg = BytesMut::new();
        buf_resp_msg.put_u8(1);
        buf_resp_msg.put_u8(1);
        buf_resp_msg.put_u32_le(0);
        buf_resp_msg.put_u16_le(3); // msg_len = 3
        buf_resp_msg.put_slice(b"err"); // 3 bytes
        buf_resp_msg.put_u8(0); // only 1 byte for listings count
                                // Remaining: 3 + 1 = 4. Expected: 3 + 2 = 5
        let err_resp_msg = MarketResponsePacket::decode(&mut buf_resp_msg.freeze()).unwrap_err();
        assert_eq!(
            err_resp_msg,
            ClientCodecError::BufferTooShort {
                expected: 5,
                got: 4
            }
        );

        // msg_len = 1 with 2 bytes remaining (msg_len + 2: 1 + 2 = 3. If mutated to *, 1 * 2 = 2)
        let mut buf_resp_msg_one = BytesMut::new();
        buf_resp_msg_one.put_u8(1);
        buf_resp_msg_one.put_u8(1);
        buf_resp_msg_one.put_u32_le(0);
        buf_resp_msg_one.put_u16_le(1); // msg_len = 1
        buf_resp_msg_one.put_u8(b'a'); // 1 byte for message
        buf_resp_msg_one.put_u8(0); // only 1 byte for listings count (needs 2)
                                    // Total buffer is 10 bytes. After reading header (8 bytes), 2 bytes remain.
                                    // Expected is msg_len + 2 = 3, got 2.
        let err_resp_msg_one =
            MarketResponsePacket::decode(&mut buf_resp_msg_one.freeze()).unwrap_err();
        assert_eq!(
            err_resp_msg_one,
            ClientCodecError::BufferTooShort {
                expected: 3,
                got: 2
            }
        );
    }
}
