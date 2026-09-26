//! 64-bit Structured GUID (TrinityCore-style ObjectGuid).
//!
//! Bit layout (64 bits total):
//! - `[63..48]` (16 bits): EntityType (Player, NPC, DynamicObject, WorldItem, etc.)
//! - `[47..32]` (16 bits): Cluster / Node ID
//! - `[31..0]`  (32 bits): Monotonic Sequence Counter

use std::fmt;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[repr(u16)]
pub enum EntityType {
    Unknown = 0,
    Player = 1,
    Npc = 2,
    DynamicObject = 3,
    WorldItem = 4,
}

impl From<u16> for EntityType {
    fn from(val: u16) -> Self {
        match val {
            1 => EntityType::Player,
            2 => EntityType::Npc,
            3 => EntityType::DynamicObject,
            4 => EntityType::WorldItem,
            _ => EntityType::Unknown,
        }
    }
}

#[derive(
    Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct Guid(pub u64);

impl Guid {
    pub const TYPE_SHIFT: u32 = 48;
    pub const NODE_SHIFT: u32 = 32;
    pub const SEQ_MASK: u64 = 0xFFFF_FFFF;
    pub const NODE_MASK: u64 = 0xFFFF;
    pub const TYPE_MASK: u64 = 0xFFFF;

    pub fn new(entity_type: EntityType, node_id: u16, sequence: u32) -> Self {
        let t = (entity_type as u64) << Self::TYPE_SHIFT;
        let n = (node_id as u64) << Self::NODE_SHIFT;
        let s = sequence as u64;
        Self(t | n | s)
    }

    pub fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub fn raw(&self) -> u64 {
        self.0
    }

    pub fn entity_type(&self) -> EntityType {
        let t = ((self.0 >> Self::TYPE_SHIFT) & Self::TYPE_MASK) as u16;
        EntityType::from(t)
    }

    pub fn node_id(&self) -> u16 {
        ((self.0 >> Self::NODE_SHIFT) & Self::NODE_MASK) as u16
    }

    pub fn sequence(&self) -> u32 {
        (self.0 & Self::SEQ_MASK) as u32
    }

    pub fn is_player(&self) -> bool {
        self.entity_type() == EntityType::Player
    }
}

impl fmt::Debug for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Guid({:?}:Node{}:Seq{}, raw={:#018x})",
            self.entity_type(),
            self.node_id(),
            self.sequence(),
            self.0
        )
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?}-{}-{}",
            self.entity_type(),
            self.node_id(),
            self.sequence()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_guid_pack_and_unpack() {
        let guid = Guid::new(EntityType::Player, 42, 100_000);

        assert_eq!(guid.entity_type(), EntityType::Player);
        assert_eq!(guid.node_id(), 42);
        assert_eq!(guid.sequence(), 100_000);
        assert!(guid.is_player());
    }

    #[test]
    fn test_guid_max_bounds() {
        let guid = Guid::new(EntityType::WorldItem, u16::MAX, u32::MAX);

        assert_eq!(guid.entity_type(), EntityType::WorldItem);
        assert_eq!(guid.node_id(), u16::MAX);
        assert_eq!(guid.sequence(), u32::MAX);
    }

    #[test]
    fn test_guid_raw_roundtrip() {
        let raw = 0x0001_002A_0001_86A0; // Player, Node 42, Seq 100_000
        let guid = Guid::from_raw(raw);

        assert_eq!(guid.raw(), raw);
        assert_eq!(guid.entity_type(), EntityType::Player);
        assert_eq!(guid.node_id(), 42);
        assert_eq!(guid.sequence(), 100_000);
    }
}
