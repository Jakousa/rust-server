//! Test helpers and fixture builders for storage entities.

use crate::models::{ContainerRecord, ItemRecord};

/// Fluent builder for constructing [`ContainerRecord`] test fixtures with sensible defaults.
#[derive(Debug, Clone)]
pub struct ContainerRecordBuilder {
    container_id: u64,
    owner_type: String,
    owner_id: String,
    name: String,
    capacity: u32,
    access_policy: String,
    version: u64,
    created_at: i64,
    updated_at: i64,
}

impl ContainerRecordBuilder {
    /// Creates a new builder with default fields and the specified container ID.
    pub fn new(container_id: u64) -> Self {
        Self {
            container_id,
            owner_type: "character".to_string(),
            owner_id: "1001".to_string(),
            name: format!("Container {}", container_id),
            capacity: 10,
            access_policy: "owner_only".to_string(),
            version: 1,
            created_at: 1000,
            updated_at: 1000,
        }
    }

    pub fn owner_type(mut self, owner_type: impl Into<String>) -> Self {
        self.owner_type = owner_type.into();
        self
    }

    pub fn owner_id(mut self, owner_id: impl Into<String>) -> Self {
        self.owner_id = owner_id.into();
        self
    }

    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn capacity(mut self, capacity: u32) -> Self {
        self.capacity = capacity;
        self
    }

    pub fn access_policy(mut self, access_policy: impl Into<String>) -> Self {
        self.access_policy = access_policy.into();
        self
    }

    pub fn version(mut self, version: u64) -> Self {
        self.version = version;
        self
    }

    pub fn created_at(mut self, created_at: i64) -> Self {
        self.created_at = created_at;
        self
    }

    pub fn updated_at(mut self, updated_at: i64) -> Self {
        self.updated_at = updated_at;
        self
    }

    pub fn build(self) -> ContainerRecord {
        ContainerRecord {
            container_id: self.container_id,
            owner_type: self.owner_type,
            owner_id: self.owner_id,
            name: self.name,
            capacity: self.capacity,
            access_policy: self.access_policy,
            version: self.version,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

/// Fluent builder for constructing [`ItemRecord`] test fixtures with sensible defaults.
#[derive(Debug, Clone)]
pub struct ItemRecordBuilder {
    item_instance_id: String,
    character_guid: u64,
    container_id: u64,
    item_def_id: u32,
    slot: u32,
    quantity: u32,
    durability: u32,
    attributes: serde_json::Value,
    created_at: i64,
    updated_at: i64,
}

impl ItemRecordBuilder {
    /// Creates a new builder with default fields and the specified item instance ID.
    pub fn new(item_instance_id: impl Into<String>) -> Self {
        Self {
            item_instance_id: item_instance_id.into(),
            character_guid: 1001,
            container_id: 1,
            item_def_id: 1,
            slot: 0,
            quantity: 1,
            durability: 100,
            attributes: serde_json::json!({}),
            created_at: 1000,
            updated_at: 1000,
        }
    }

    pub fn character_guid(mut self, character_guid: u64) -> Self {
        self.character_guid = character_guid;
        self
    }

    pub fn container_id(mut self, container_id: u64) -> Self {
        self.container_id = container_id;
        self
    }

    pub fn item_def_id(mut self, item_def_id: u32) -> Self {
        self.item_def_id = item_def_id;
        self
    }

    pub fn slot(mut self, slot: u32) -> Self {
        self.slot = slot;
        self
    }

    pub fn quantity(mut self, quantity: u32) -> Self {
        self.quantity = quantity;
        self
    }

    pub fn durability(mut self, durability: u32) -> Self {
        self.durability = durability;
        self
    }

    pub fn attributes(mut self, attributes: serde_json::Value) -> Self {
        self.attributes = attributes;
        self
    }

    pub fn created_at(mut self, created_at: i64) -> Self {
        self.created_at = created_at;
        self
    }

    pub fn updated_at(mut self, updated_at: i64) -> Self {
        self.updated_at = updated_at;
        self
    }

    pub fn build(self) -> ItemRecord {
        ItemRecord {
            item_instance_id: self.item_instance_id,
            character_guid: self.character_guid,
            container_id: self.container_id,
            item_def_id: self.item_def_id,
            slot: self.slot,
            quantity: self.quantity,
            durability: self.durability,
            attributes: self.attributes,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_container_record_builder_defaults() {
        let container = ContainerRecordBuilder::new(500).build();
        assert_eq!(container.container_id, 500);
        assert_eq!(container.owner_type, "character");
        assert_eq!(container.owner_id, "1001");
        assert_eq!(container.name, "Container 500");
        assert_eq!(container.capacity, 10);
        assert_eq!(container.access_policy, "owner_only");
        assert_eq!(container.version, 1);
        assert_eq!(container.created_at, 1000);
        assert_eq!(container.updated_at, 1000);
    }

    #[test]
    fn test_container_record_builder_custom() {
        let container = ContainerRecordBuilder::new(501)
            .owner_type("guild")
            .owner_id("g_99")
            .name("Guild Vault")
            .capacity(100)
            .access_policy("guild_shared")
            .version(3)
            .created_at(2000)
            .updated_at(2050)
            .build();
        assert_eq!(container.container_id, 501);
        assert_eq!(container.owner_type, "guild");
        assert_eq!(container.owner_id, "g_99");
        assert_eq!(container.name, "Guild Vault");
        assert_eq!(container.capacity, 100);
        assert_eq!(container.access_policy, "guild_shared");
        assert_eq!(container.version, 3);
        assert_eq!(container.created_at, 2000);
        assert_eq!(container.updated_at, 2050);
    }

    #[test]
    fn test_item_record_builder_defaults() {
        let item = ItemRecordBuilder::new("item_default").build();
        assert_eq!(item.item_instance_id, "item_default");
        assert_eq!(item.character_guid, 1001);
        assert_eq!(item.container_id, 1);
        assert_eq!(item.item_def_id, 1);
        assert_eq!(item.slot, 0);
        assert_eq!(item.quantity, 1);
        assert_eq!(item.durability, 100);
        assert_eq!(item.attributes, serde_json::json!({}));
        assert_eq!(item.created_at, 1000);
        assert_eq!(item.updated_at, 1000);
    }

    #[test]
    fn test_item_record_builder_custom() {
        let item = ItemRecordBuilder::new("sword_01")
            .character_guid(2002)
            .container_id(7001)
            .item_def_id(42)
            .slot(5)
            .quantity(2)
            .durability(80)
            .attributes(serde_json::json!({"power": 10}))
            .created_at(1100)
            .updated_at(1200)
            .build();
        assert_eq!(item.item_instance_id, "sword_01");
        assert_eq!(item.character_guid, 2002);
        assert_eq!(item.container_id, 7001);
        assert_eq!(item.item_def_id, 42);
        assert_eq!(item.slot, 5);
        assert_eq!(item.quantity, 2);
        assert_eq!(item.durability, 80);
        assert_eq!(item.attributes, serde_json::json!({"power": 10}));
        assert_eq!(item.created_at, 1100);
        assert_eq!(item.updated_at, 1200);
    }
}
