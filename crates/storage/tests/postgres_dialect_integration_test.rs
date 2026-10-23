//! End-to-end dual-dialect integration test suite verifying storage repositories,
//! 2PC container transfers, escrow trading, marketplace transactions, world persistence,
//! and schemaless datastore across SQLite and live PostgreSQL.

use mmo_core::PropertyValue;
use mmo_storage::{
    connect_test_databases, ContainerRecordBuilder, ContainerRepository, DataDocument, DataStore,
    EntitySnapshot, EscrowCoordinator, EscrowStatus, ItemRecordBuilder, ListingStatus,
    MarketplaceRepository, StorageRepository, TransactionCoordinator, WorldObjectRecord,
    WorldRepository,
};
use std::collections::HashMap;

#[tokio::test]
async fn test_dual_dialect_storage_end_to_end() {
    let dbs = connect_test_databases()
        .await
        .expect("Failed to initialize test databases");

    for (dialect, db) in dbs {
        println!(
            "=== Running Dual-Dialect Integration Tests for: {} ===",
            dialect
        );

        // -------------------------------------------------------------
        // 1. Snapshots & Characters
        // -------------------------------------------------------------
        let repo = StorageRepository::new(db.clone());
        let account_id = format!("acc_{}", dialect);
        let username = format!("user_{}", dialect);
        let account = repo
            .create_account(&account_id, &username)
            .await
            .expect("create_account failed");
        assert_eq!(account.username, username);

        let char_guid = if dialect == "postgres" { 2001 } else { 1001 };
        let character = repo
            .create_character(&account.account_id, char_guid, "Hero", (10.0, 20.0, 30.0))
            .await
            .expect("create_character failed");
        assert_eq!(character.guid, char_guid);

        let loaded_char = repo
            .get_character(char_guid)
            .await
            .expect("get_character failed")
            .expect("character not found");
        assert_eq!(loaded_char.name, "Hero");

        // Save & load EntitySnapshot (exercises state_blob, attributes_json, and coordinate columns)
        let snapshot = EntitySnapshot {
            guid: char_guid,
            position: (100.0, 200.0, 300.0),
            rotation: 1.57,
            attributes: serde_json::json!({"health": 100, "mana": 50}),
            state_blob: Some(vec![1, 2, 3, 4, 5, 42]),
            version: 2,
            timestamp: 123456789,
        };
        repo.save_snapshot(&snapshot)
            .await
            .expect("save_snapshot failed");

        let loaded_snap = repo
            .load_snapshot(char_guid)
            .await
            .expect("load_snapshot failed")
            .expect("snapshot not found");
        assert_eq!(loaded_snap.guid, char_guid);
        assert_eq!(loaded_snap.position, (100.0, 200.0, 300.0));
        assert_eq!(loaded_snap.state_blob, Some(vec![1, 2, 3, 4, 5, 42]));

        // Add & fetch items for character
        let item_id = format!("item_sword_{}", dialect);
        let item = ItemRecordBuilder::new(&item_id)
            .character_guid(char_guid)
            .container_id(0)
            .item_def_id(42)
            .slot(0)
            .quantity(1)
            .durability(100)
            .build();
        repo.add_item(&item).await.expect("add_item failed");

        let loaded_item = repo
            .get_item(&item_id)
            .await
            .expect("get_item failed")
            .expect("item not found");
        assert_eq!(loaded_item.item_def_id, 42);

        let char_items = repo
            .get_character_items(char_guid)
            .await
            .expect("get_character_items failed");
        assert_eq!(char_items.len(), 1);

        // -------------------------------------------------------------
        // 2. Containers & 2PC Transfers
        // -------------------------------------------------------------
        let container_repo = ContainerRepository::new(db.clone());
        let coord = TransactionCoordinator::new(db.clone());

        let c1_id = if dialect == "postgres" { 6001 } else { 5001 };
        let c2_id = if dialect == "postgres" { 6002 } else { 5002 };

        let c1 = ContainerRecordBuilder::new(c1_id)
            .owner_id(char_guid.to_string())
            .capacity(10)
            .build();
        let c2 = ContainerRecordBuilder::new(c2_id)
            .owner_id(char_guid.to_string())
            .capacity(10)
            .build();
        container_repo
            .create_container(&c1)
            .await
            .expect("create c1 failed");
        container_repo
            .create_container(&c2)
            .await
            .expect("create c2 failed");

        // Put an item in c1 at slot 0
        let c1_item_id = format!("c1_item_{}", dialect);
        let c1_item = ItemRecordBuilder::new(&c1_item_id)
            .character_guid(char_guid)
            .container_id(c1_id)
            .item_def_id(999)
            .slot(0)
            .build();
        repo.add_item(&c1_item).await.expect("add c1_item failed");

        // Execute transfer from c1 slot 0 -> c2 slot 5
        let tx_id = format!("tx_transfer_{}", dialect);
        let transfer_res = coord
            .execute_transfer(&tx_id, c1_id, 0, c2_id, 5, 5000)
            .await
            .expect("execute_transfer failed");
        assert_eq!(transfer_res.status, "committed");

        // Verify item is now in c2 at slot 5
        let item_in_c2 = container_repo
            .get_item_at_slot(c2_id, 5)
            .await
            .expect("get_item_at_slot failed")
            .expect("item not found in c2");
        assert_eq!(item_in_c2.item_instance_id, c1_item_id);

        // Test prepare and abort_transfer
        let tx_abort_id = format!("tx_abort_{}", dialect);
        coord
            .prepare_transfer(&tx_abort_id, c2_id, 5, c1_id, 2, 5000)
            .await
            .expect("prepare_transfer failed");
        let aborted = coord
            .abort_transfer(&tx_abort_id)
            .await
            .expect("abort_transfer failed");
        assert!(aborted);

        // -------------------------------------------------------------
        // 3. Escrow Trading & Marketplace
        // -------------------------------------------------------------
        let escrow_coord = EscrowCoordinator::new(db.clone());
        let escrow_c_id = if dialect == "postgres" { 6003 } else { 5003 };
        let escrow_container = ContainerRecordBuilder::new(escrow_c_id)
            .owner_id("system")
            .capacity(10)
            .build();
        container_repo
            .create_container(&escrow_container)
            .await
            .expect("create escrow container failed");

        let secret = "super_secret_passphrase";
        let escrow_id = format!("escrow_{}", dialect);

        let escrow_record = escrow_coord
            .create_escrow(
                &escrow_id,
                c2_id,
                c1_id,
                escrow_c_id,
                std::slice::from_ref(&c1_item_id),
                secret,
                60,
            )
            .await
            .expect("create_escrow failed");
        assert_eq!(escrow_record.status, EscrowStatus::Held);

        // Release escrow with secret hash verification
        let released = escrow_coord
            .release_escrow(&escrow_id, secret)
            .await
            .expect("release_escrow failed");
        assert!(released);

        // Verify escrow status is now Settled
        let settled_escrow = escrow_coord
            .get_escrow(&escrow_id)
            .await
            .expect("get_escrow failed")
            .expect("escrow not found");
        assert_eq!(settled_escrow.status, EscrowStatus::Settled);

        // Marketplace Repository
        let market_repo = MarketplaceRepository::new(db.clone());
        let market_escrow_id = if dialect == "postgres" { 6004 } else { 5004 };
        let market_escrow_c = ContainerRecordBuilder::new(market_escrow_id)
            .owner_id("system")
            .capacity(20)
            .build();
        container_repo
            .create_container(&market_escrow_c)
            .await
            .expect("create market escrow container failed");

        // The item is now back in c1 after escrow release (let's find which slot it landed in)
        let c1_items = container_repo
            .get_container_items(c1_id)
            .await
            .expect("get_container_items for c1 failed");
        assert!(!c1_items.is_empty(), "c1 should contain released item");
        let item_slot = c1_items[0].slot;

        let listing = market_repo
            .create_listing(char_guid, c1_id, item_slot, 1500, 3600, market_escrow_id)
            .await
            .expect("create_listing failed");
        assert_eq!(listing.price, 1500);
        assert_eq!(listing.seller_guid, char_guid);

        let queried = market_repo
            .get_listing(&listing.listing_id)
            .await
            .expect("get_listing failed")
            .expect("listing not found");
        assert_eq!(queried.status, ListingStatus::Active);

        let query_list = market_repo
            .query_listings(Some(999), None, None, 0, 10)
            .await
            .expect("query_listings failed");
        assert_eq!(query_list.len(), 1);

        // Cancel listing refunds item back to c1
        let cancelled = market_repo
            .cancel_listing(char_guid, &listing.listing_id, c1_id)
            .await
            .expect("cancel_listing failed");
        assert_eq!(cancelled.status, ListingStatus::Cancelled);

        // -------------------------------------------------------------
        // 4. World Objects (Exercising bind_upsert_postgres & bind_upsert_sqlite)
        // -------------------------------------------------------------
        let world_repo = WorldRepository::new(db.clone());
        let world_id = 1;
        let obj1 = WorldObjectRecord {
            world_id,
            object_id: 101,
            sector_x: 0,
            sector_y: 0,
            sector_z: 0,
            prefab_id: 1,
            pos_x: 10.0,
            pos_y: 20.0,
            pos_z: 30.0,
            rot_x: 0.0,
            rot_y: 0.0,
            rot_z: 0.0,
            rot_w: 1.0,
            scale_x: 1.0,
            scale_y: 1.0,
            scale_z: 1.0,
            bounds_min_x: 9.0,
            bounds_min_y: 19.0,
            bounds_min_z: 29.0,
            bounds_max_x: 11.0,
            bounds_max_y: 21.0,
            bounds_max_z: 31.0,
            state_flags: 0,
            owner_type: "npc".to_string(),
            owner_id: "vendor_1".to_string(),
            health: Some(100),
            max_health: Some(100),
            properties_json: "{\"interaction\":\"trade\"}".to_string(),
            dirty_revision: 1,
            updated_at: 1000,
        };

        let obj2 = WorldObjectRecord {
            world_id,
            object_id: 102,
            sector_x: 0,
            sector_y: 0,
            sector_z: 0,
            prefab_id: 2,
            pos_x: 50.0,
            pos_y: 60.0,
            pos_z: 70.0,
            rot_x: 0.0,
            rot_y: 0.0,
            rot_z: 0.0,
            rot_w: 1.0,
            scale_x: 1.0,
            scale_y: 1.0,
            scale_z: 1.0,
            bounds_min_x: 49.0,
            bounds_min_y: 59.0,
            bounds_min_z: 69.0,
            bounds_max_x: 51.0,
            bounds_max_y: 61.0,
            bounds_max_z: 71.0,
            state_flags: 1,
            owner_type: "static".to_string(),
            owner_id: "rock_1".to_string(),
            health: None,
            max_health: None,
            properties_json: "{}".to_string(),
            dirty_revision: 1,
            updated_at: 1000,
        };

        // Single save
        world_repo
            .save_world_object(&obj1)
            .await
            .expect("save_world_object failed");

        // Batch save (exercises bind_upsert_postgres with all parameters)
        world_repo
            .save_world_objects_batch(&[obj1.clone(), obj2.clone()])
            .await
            .expect("save_world_objects_batch failed");

        // Sector lookup
        let sector_objects = world_repo
            .load_sector_objects(world_id, 0, 0, 0)
            .await
            .expect("load_sector_objects failed");
        assert_eq!(sector_objects.len(), 2);

        // Delete world object
        let deleted = world_repo
            .delete_world_object(world_id, 101)
            .await
            .expect("delete_world_object failed");
        assert!(deleted);

        let remaining = world_repo
            .load_sector_objects(world_id, 0, 0, 0)
            .await
            .expect("load_sector_objects after delete failed");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].object_id, 102);

        // -------------------------------------------------------------
        // 5. Schemaless DocumentStore
        // -------------------------------------------------------------
        let doc_store = DataStore::new();
        let mut props = HashMap::new();
        props.insert(
            "name".to_string(),
            PropertyValue::String("IronOre".to_string()),
        );
        props.insert("tier".to_string(), PropertyValue::Int(2));
        let doc = DataDocument::new("items", "item_42", props.into());

        doc_store.insert_document(doc);
        assert!(doc_store.contains("items", "item_42"));
        let retrieved = doc_store
            .get("items", "item_42")
            .expect("document not found");
        assert_eq!(retrieved.get_str("name"), Some("IronOre"));
        assert_eq!(retrieved.get_i64("tier"), Some(2));

        let removed = doc_store.remove_document("items", "item_42");
        assert!(removed.is_some());
        assert!(!doc_store.contains("items", "item_42"));
    }
}
