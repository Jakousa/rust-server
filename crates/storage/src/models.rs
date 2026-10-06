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
    #[serde(default)]
    pub state_blob: Option<Vec<u8>>,
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

impl ItemRecord {
    /// Returns a new [`ItemRecordBuilder`] initialized with the given instance ID.
    pub fn builder(item_instance_id: impl Into<String>) -> crate::test_helpers::ItemRecordBuilder {
        crate::test_helpers::ItemRecordBuilder::new(item_instance_id)
    }
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

impl ContainerRecord {
    /// Returns a new [`ContainerRecordBuilder`] initialized with the given container ID.
    pub fn builder(container_id: u64) -> crate::test_helpers::ContainerRecordBuilder {
        crate::test_helpers::ContainerRecordBuilder::new(container_id)
    }
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

/// Persistent record representing a dynamic, interactive, or static world object in a sector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldObjectRecord {
    pub world_id: u64,
    pub object_id: u64,
    pub sector_x: i32,
    pub sector_y: i32,
    pub sector_z: i32,
    pub prefab_id: u32,
    pub pos_x: f32,
    pub pos_y: f32,
    pub pos_z: f32,
    pub rot_x: f32,
    pub rot_y: f32,
    pub rot_z: f32,
    pub rot_w: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub scale_z: f32,
    pub bounds_min_x: f32,
    pub bounds_min_y: f32,
    pub bounds_min_z: f32,
    pub bounds_max_x: f32,
    pub bounds_max_y: f32,
    pub bounds_max_z: f32,
    pub state_flags: u32,
    pub owner_type: String,
    pub owner_id: String,
    pub health: Option<i32>,
    pub max_health: Option<i32>,
    pub properties_json: String,
    pub dirty_revision: u64,
    pub updated_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_trade_status_display_and_from_str() {
        assert_eq!(TradeStatus::Pending.to_string(), "pending");
        assert_eq!(TradeStatus::Committed.to_string(), "committed");
        assert_eq!(TradeStatus::RolledBack.to_string(), "rolled_back");

        assert_eq!(
            TradeStatus::from_str("pending").unwrap(),
            TradeStatus::Pending
        );
        assert_eq!(
            TradeStatus::from_str("committed").unwrap(),
            TradeStatus::Committed
        );
        assert_eq!(
            TradeStatus::from_str("rolled_back").unwrap(),
            TradeStatus::RolledBack
        );
        assert!(TradeStatus::from_str("invalid").is_err());

        let json = serde_json::to_string(&TradeStatus::Pending).unwrap();
        let decoded: TradeStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, TradeStatus::Pending);
    }

    #[test]
    fn test_escrow_status_display_and_from_str() {
        assert_eq!(EscrowStatus::Held.to_string(), "held");
        assert_eq!(EscrowStatus::Settled.to_string(), "settled");
        assert_eq!(EscrowStatus::Refunded.to_string(), "refunded");

        assert_eq!(EscrowStatus::from_str("held").unwrap(), EscrowStatus::Held);
        assert_eq!(
            EscrowStatus::from_str("settled").unwrap(),
            EscrowStatus::Settled
        );
        assert_eq!(
            EscrowStatus::from_str("refunded").unwrap(),
            EscrowStatus::Refunded
        );
        assert!(EscrowStatus::from_str("unknown").is_err());

        let json = serde_json::to_string(&EscrowStatus::Held).unwrap();
        let decoded: EscrowStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, EscrowStatus::Held);
    }

    #[test]
    fn test_records_serialization_and_equality() {
        let account = AccountRecord {
            account_id: "acc_1".to_string(),
            username: "tester".to_string(),
            created_at: 1000,
        };
        let acc_json = serde_json::to_string(&account).unwrap();
        let acc_dec: AccountRecord = serde_json::from_str(&acc_json).unwrap();
        assert_eq!(account, acc_dec);
        assert_eq!(account.clone(), acc_dec);
        assert!(!format!("{:?}", account).is_empty());

        let character = CharacterRecord {
            guid: 10,
            account_id: "acc_1".to_string(),
            name: "Warrior".to_string(),
            level: 1,
            position: (1.0, 2.0, 3.0),
            rotation: 1.5,
            attributes: serde_json::json!({"hp": 100}),
            updated_at: 1000,
        };
        let char_json = serde_json::to_string(&character).unwrap();
        let char_dec: CharacterRecord = serde_json::from_str(&char_json).unwrap();
        assert_eq!(character, char_dec);
        assert_eq!(character.clone(), char_dec);
        assert!(!format!("{:?}", character).is_empty());

        let snapshot = EntitySnapshot {
            guid: 10,
            position: (1.0, 2.0, 3.0),
            rotation: 1.5,
            attributes: serde_json::json!({}),
            state_blob: Some(vec![1, 2, 3]),
            version: 2,
            timestamp: 2000,
        };
        let snap_json = serde_json::to_string(&snapshot).unwrap();
        let snap_dec: EntitySnapshot = serde_json::from_str(&snap_json).unwrap();
        assert_eq!(snapshot, snap_dec);
        assert_eq!(snapshot.clone(), snap_dec);
        assert!(!format!("{:?}", snapshot).is_empty());

        let inv = InventoryRecord {
            character_guid: 10,
            capacity: 20,
            updated_at: 1000,
        };
        let inv_json = serde_json::to_string(&inv).unwrap();
        let inv_dec: InventoryRecord = serde_json::from_str(&inv_json).unwrap();
        assert_eq!(inv, inv_dec);
        assert_eq!(inv.clone(), inv_dec);
        assert!(!format!("{:?}", inv).is_empty());

        let item = ItemRecord {
            item_instance_id: "item_1".to_string(),
            character_guid: 10,
            container_id: 100,
            item_def_id: 5,
            slot: 0,
            quantity: 1,
            durability: 100,
            attributes: serde_json::json!({}),
            created_at: 1000,
            updated_at: 1000,
        };
        let item_json = serde_json::to_string(&item).unwrap();
        let item_dec: ItemRecord = serde_json::from_str(&item_json).unwrap();
        assert_eq!(item, item_dec);
        assert_eq!(item.clone(), item_dec);
        assert!(!format!("{:?}", item).is_empty());

        let trade = TradeRecord {
            trade_id: "tx_1".to_string(),
            sender_guid: 10,
            recipient_guid: 20,
            items_json: "[]".to_string(),
            status: TradeStatus::Committed,
            created_at: 1000,
        };
        let trade_json = serde_json::to_string(&trade).unwrap();
        let trade_dec: TradeRecord = serde_json::from_str(&trade_json).unwrap();
        assert_eq!(trade, trade_dec);
        assert_eq!(trade.clone(), trade_dec);
        assert!(!format!("{:?}", trade).is_empty());

        let container = ContainerRecord {
            container_id: 100,
            owner_type: "character".to_string(),
            owner_id: "10".to_string(),
            name: "Bags".to_string(),
            capacity: 20,
            access_policy: "owner_only".to_string(),
            version: 1,
            created_at: 1000,
            updated_at: 1000,
        };
        let cont_json = serde_json::to_string(&container).unwrap();
        let cont_dec: ContainerRecord = serde_json::from_str(&cont_json).unwrap();
        assert_eq!(container, cont_dec);
        assert_eq!(container.clone(), cont_dec);
        assert!(!format!("{:?}", container).is_empty());

        let res = ReservationRecord {
            reservation_id: "res_1".to_string(),
            tx_id: "tx_1".to_string(),
            container_id: 100,
            slot: 0,
            item_instance_id: Some("item_1".to_string()),
            reservation_type: "transfer".to_string(),
            expires_at_epoch_ms: 2000,
            created_at_epoch_ms: 1000,
        };
        let res_json = serde_json::to_string(&res).unwrap();
        let res_dec: ReservationRecord = serde_json::from_str(&res_json).unwrap();
        assert_eq!(res, res_dec);
        assert_eq!(res.clone(), res_dec);
        assert!(!format!("{:?}", res).is_empty());

        let escrow = EscrowRecord {
            escrow_id: "esc_1".to_string(),
            depositor_container_id: 100,
            beneficiary_container_id: 200,
            escrow_container_id: 300,
            status: EscrowStatus::Held,
            secret_hash: "hash".to_string(),
            expires_at_epoch_ms: 5000,
            created_at_epoch_ms: 1000,
            updated_at_epoch_ms: 1000,
        };
        let esc_json = serde_json::to_string(&escrow).unwrap();
        let esc_dec: EscrowRecord = serde_json::from_str(&esc_json).unwrap();
        assert_eq!(escrow, esc_dec);
        assert_eq!(escrow.clone(), esc_dec);
        assert!(!format!("{:?}", escrow).is_empty());

        let tx = ContainerTransactionRecord {
            tx_id: "tx_1".to_string(),
            src_container_id: 100,
            dst_container_id: 200,
            item_instance_id: "item_1".to_string(),
            status: "committed".to_string(),
            timestamp_epoch_ms: 1000,
        };
        let tx_json = serde_json::to_string(&tx).unwrap();
        let tx_dec: ContainerTransactionRecord = serde_json::from_str(&tx_json).unwrap();
        assert_eq!(tx, tx_dec);
        assert_eq!(tx.clone(), tx_dec);
        assert!(!format!("{:?}", tx).is_empty());

        let wo = WorldObjectRecord {
            world_id: 1,
            object_id: 42,
            sector_x: 2,
            sector_y: -1,
            sector_z: 3,
            prefab_id: 101,
            pos_x: 128.5,
            pos_y: -60.0,
            pos_z: 195.0,
            rot_x: 0.0,
            rot_y: 0.0,
            rot_z: 0.0,
            rot_w: 1.0,
            scale_x: 1.0,
            scale_y: 1.0,
            scale_z: 1.0,
            bounds_min_x: 127.0,
            bounds_min_y: -61.0,
            bounds_min_z: 194.0,
            bounds_max_x: 130.0,
            bounds_max_y: -59.0,
            bounds_max_z: 196.0,
            state_flags: 3,
            owner_type: "character".to_string(),
            owner_id: "1001".to_string(),
            health: Some(100),
            max_health: Some(100),
            properties_json: "{\"mat\":\"stone\"}".to_string(),
            dirty_revision: 5,
            updated_at: 1700000000,
        };
        let wo_json = serde_json::to_string(&wo).unwrap();
        let wo_dec: WorldObjectRecord = serde_json::from_str(&wo_json).unwrap();
        assert_eq!(wo, wo_dec);
        assert_eq!(wo.clone(), wo_dec);
        assert!(!format!("{:?}", wo).is_empty());
    }
}
