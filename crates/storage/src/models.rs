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

/// Item instance record stored in a container or character's inventory slot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemRecord {
    pub item_instance_id: String,
    pub character_guid: u64,
    pub container_id: u64,
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

/// Generic storage container persistent record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContainerRecord {
    pub container_id: u64,
    pub owner_type: String,
    pub owner_id: String,
    pub name: String,
    pub capacity: u32,
    pub access_policy: String,
    pub version: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Temporary slot reservation record for two-phase transfers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReservationRecord {
    pub reservation_id: String,
    pub tx_id: String,
    pub container_id: u64,
    pub slot: u32,
    pub item_instance_id: Option<String>,
    pub reservation_type: String,
    pub expires_at_epoch_ms: i64,
    pub created_at_epoch_ms: i64,
}

/// Escrow status for asynchronous conditional transactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EscrowStatus {
    Held,
    Settled,
    Refunded,
}

impl std::fmt::Display for EscrowStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EscrowStatus::Held => write!(f, "held"),
            EscrowStatus::Settled => write!(f, "settled"),
            EscrowStatus::Refunded => write!(f, "refunded"),
        }
    }
}

impl std::str::FromStr for EscrowStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "held" => Ok(EscrowStatus::Held),
            "settled" => Ok(EscrowStatus::Settled),
            "refunded" => Ok(EscrowStatus::Refunded),
            other => Err(format!("Unknown escrow status: {}", other)),
        }
    }
}

/// Escrow holding record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EscrowRecord {
    pub escrow_id: String,
    pub depositor_container_id: u64,
    pub beneficiary_container_id: u64,
    pub escrow_container_id: u64,
    pub status: EscrowStatus,
    pub secret_hash: String,
    pub expires_at_epoch_ms: i64,
    pub created_at_epoch_ms: i64,
    pub updated_at_epoch_ms: i64,
}

/// Audit log record for completed or aborted container transfers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContainerTransactionRecord {
    pub tx_id: String,
    pub src_container_id: u64,
    pub dst_container_id: u64,
    pub item_instance_id: String,
    pub status: String,
    pub timestamp_epoch_ms: i64,
}
