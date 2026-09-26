//! Data models for persistence tier.

use serde::{Deserialize, Serialize};

/// Account persistent record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountRecord {
    pub account_id: String,
    pub username: String,
    pub created_at: i64,
}

/// Character persistent record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CharacterRecord {
    pub guid: u64,
    pub account_id: String,
    pub name: String,
    pub level: i32,
    pub position: (f32, f32, f32),
    pub rotation: f32,
    pub attributes: serde_json::Value,
    pub updated_at: i64,
}

/// Decoupled entity snapshot representing in-memory state captured for persistence or handover.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntitySnapshot {
    pub guid: u64,
    pub position: (f32, f32, f32),
    pub rotation: f32,
    pub attributes: serde_json::Value,
    pub version: u64,
    pub timestamp: u64,
}

/// Inventory record representing character inventory metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InventoryRecord {
    pub character_guid: u64,
    pub capacity: u32,
    pub updated_at: i64,
}

/// Item instance record stored in a character's inventory slot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemRecord {
    pub item_instance_id: String,
    pub character_guid: u64,
    pub item_def_id: u32,
    pub slot: u32,
    pub quantity: u32,
    pub durability: u32,
    pub attributes: serde_json::Value,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Trade status for transactional audit logging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TradeStatus {
    Pending,
    Committed,
    RolledBack,
}

impl std::fmt::Display for TradeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TradeStatus::Pending => write!(f, "pending"),
            TradeStatus::Committed => write!(f, "committed"),
            TradeStatus::RolledBack => write!(f, "rolled_back"),
        }
    }
}

impl std::str::FromStr for TradeStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(TradeStatus::Pending),
            "committed" => Ok(TradeStatus::Committed),
            "rolled_back" => Ok(TradeStatus::RolledBack),
            other => Err(format!("Unknown trade status: {}", other)),
        }
    }
}

/// Trade audit log record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TradeRecord {
    pub trade_id: String,
    pub sender_guid: u64,
    pub recipient_guid: u64,
    pub items_json: String,
    pub status: TradeStatus,
    pub created_at: i64,
}
