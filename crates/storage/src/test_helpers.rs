//! Test helpers and fixture builders for storage entities.

pub use crate::models::{ContainerRecordBuilder, ItemRecordBuilder};

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
