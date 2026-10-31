//! Concurrent stress and deadlock-freedom verification for distributed containers.

use mmo_storage::{
    ContainerRecordBuilder, ContainerRepository, Database, ItemRecordBuilder, StorageError,
    TransactionCoordinator,
};
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn test_concurrent_reverse_transfers_deadlock_freedom() {
    let db = Database::connect_memory().await.unwrap();
    let container_repo = ContainerRepository::new(db.clone());
    let coord = Arc::new(TransactionCoordinator::new(db.clone()));

    // Create 2 containers: C1 and C2 with capacity 100 each
    let c1_id = 9001;
    let c2_id = 9002;
    container_repo
        .create_container(
            &ContainerRecordBuilder::new(c1_id)
                .owner_type("guild")
                .owner_id("guild_alpha")
                .name("Guild Vault Alpha")
                .capacity(100)
                .build(),
        )
        .await
        .unwrap();

    container_repo
        .create_container(
            &ContainerRecordBuilder::new(c2_id)
                .owner_type("guild")
                .owner_id("guild_beta")
                .name("Guild Vault Beta")
                .capacity(100)
                .build(),
        )
        .await
        .unwrap();

    let char_repo = mmo_storage::CharacterRepository::new(db.clone());

    // Populate C1 with 20 items in slots 0..20
    for i in 0..20 {
        char_repo
            .add_item(
                &ItemRecordBuilder::new(format!("c1_item_{}", i))
                    .character_guid(1001)
                    .container_id(c1_id)
                    .item_def_id(100 + i)
                    .slot(i)
                    .build(),
            )
            .await
            .unwrap();
    }

    // Populate C2 with 20 items in slots 0..20
    for i in 0..20 {
        char_repo
            .add_item(
                &ItemRecordBuilder::new(format!("c2_item_{}", i))
                    .character_guid(1002)
                    .container_id(c2_id)
                    .item_def_id(200 + i)
                    .slot(i)
                    .build(),
            )
            .await
            .unwrap();
    }

    // Verify initial item counts
    let initial_c1_items = container_repo.get_container_items(c1_id).await.unwrap();
    let initial_c2_items = container_repo.get_container_items(c2_id).await.unwrap();
    assert_eq!(initial_c1_items.len(), 20);
    assert_eq!(initial_c2_items.len(), 20);

    // Concurrently execute 50 transfers from C1 -> C2 and 50 transfers from C2 -> C1
    // Wrapped in a strict timeout to guarantee deadlock freedom
    let deadlock_timeout = Duration::from_secs(10);
    let run_test = async {
        let mut handles = Vec::new();

        // 50 tasks transferring C1 slot (i % 20) -> C2 slot (20 + (i % 30))
        for i in 0..50 {
            let coord_clone = Arc::clone(&coord);
            let tx_id = format!("tx_c1_to_c2_{}", i);
            let src_slot = (i % 20) as u32;
            let dst_slot = (20 + (i % 30)) as u32;

            handles.push(tokio::spawn(async move {
                match coord_clone
                    .prepare_transfer(&tx_id, c1_id, src_slot, c2_id, dst_slot, 2000)
                    .await
                {
                    Ok(_) => {
                        let _ = coord_clone.commit_transfer(&tx_id).await;
                    }
                    Err(StorageError::SlotReserved(_, _))
                    | Err(StorageError::SlotOccupied(_, _))
                    | Err(StorageError::ItemNotFound(_, _)) => {
                        // Expected concurrent contention
                    }
                    Err(e) => {
                        panic!("Unexpected transfer error: {:?}", e);
                    }
                }
            }));
        }

        // 50 opposing tasks transferring C2 slot (j % 20) -> C1 slot (20 + (j % 30))
        for j in 0..50 {
            let coord_clone = Arc::clone(&coord);
            let tx_id = format!("tx_c2_to_c1_{}", j);
            let src_slot = (j % 20) as u32;
            let dst_slot = (20 + (j % 30)) as u32;

            handles.push(tokio::spawn(async move {
                match coord_clone
                    .prepare_transfer(&tx_id, c2_id, src_slot, c1_id, dst_slot, 2000)
                    .await
                {
                    Ok(_) => {
                        let _ = coord_clone.commit_transfer(&tx_id).await;
                    }
                    Err(StorageError::SlotReserved(_, _))
                    | Err(StorageError::SlotOccupied(_, _))
                    | Err(StorageError::ItemNotFound(_, _)) => {
                        // Expected concurrent contention
                    }
                    Err(e) => {
                        panic!("Unexpected transfer error: {:?}", e);
                    }
                }
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }
    };

    tokio::time::timeout(deadlock_timeout, run_test)
        .await
        .expect("Deadlock detected during concurrent opposing transfers!");

    // Asset conservation check: total items across both containers MUST be exactly 40
    let final_c1_items = container_repo.get_container_items(c1_id).await.unwrap();
    let final_c2_items = container_repo.get_container_items(c2_id).await.unwrap();
    let total_items = final_c1_items.len() + final_c2_items.len();

    assert_eq!(
        total_items, 40,
        "Asset conservation violation: expected 40 total items, found {}",
        total_items
    );

    // Verify slot integrity: no two items in the same container share a slot
    let mut c1_slots = std::collections::HashSet::new();
    for item in &final_c1_items {
        assert!(
            c1_slots.insert(item.slot),
            "Slot collision in C1 at slot {}",
            item.slot
        );
    }

    let mut c2_slots = std::collections::HashSet::new();
    for item in &final_c2_items {
        assert!(
            c2_slots.insert(item.slot),
            "Slot collision in C2 at slot {}",
            item.slot
        );
    }
}

#[tokio::test]
async fn test_abort_transfer_atomicity_and_audit_record() {
    use sqlx::Row;

    let db = Database::connect_memory().await.unwrap();
    let container_repo = ContainerRepository::new(db.clone());
    let char_repo = mmo_storage::CharacterRepository::new(db.clone());
    let coord = TransactionCoordinator::new(db.clone());

    // Aborting a non-existent tx_id should safely return Ok(false)
    let non_existent = coord.abort_transfer("tx-does-not-exist").await.unwrap();
    assert!(!non_existent);

    // Create 2 containers and 1 item
    let c1_id = 1101;
    let c2_id = 1102;
    container_repo
        .create_container(
            &ContainerRecordBuilder::new(c1_id)
                .owner_type("character")
                .owner_id("1001")
                .capacity(10)
                .build(),
        )
        .await
        .unwrap();

    container_repo
        .create_container(
            &ContainerRecordBuilder::new(c2_id)
                .owner_type("character")
                .owner_id("1002")
                .capacity(10)
                .build(),
        )
        .await
        .unwrap();

    let item = mmo_storage::ItemRecord {
        item_instance_id: "sword-1101".to_string(),
        character_guid: 1001,
        container_id: c1_id,
        item_def_id: 501,
        slot: 0,
        quantity: 1,
        durability: 100,
        attributes: serde_json::json!({}),
        created_at: 1000,
        updated_at: 1000,
    };
    char_repo.add_item(&item).await.unwrap();

    // Prepare transfer
    let tx_id = "tx-abort-test-1";
    let prep = coord
        .prepare_transfer(tx_id, c1_id, 0, c2_id, 0, 5000)
        .await
        .unwrap();
    assert_eq!(prep.container_id, c1_id);

    // Abort transfer
    let aborted = coord.abort_transfer(tx_id).await.unwrap();
    assert!(aborted);

    // Check transaction history in DB
    let query_str = db.sql("SELECT tx_id, src_container_id, dst_container_id, item_instance_id, status FROM container_transactions WHERE tx_id = ?");
    if let Some(pool) = db.sqlite_pool() {
        let row = sqlx::query(&query_str)
            .bind(tx_id)
            .fetch_one(pool)
            .await
            .unwrap();
        let db_status: String = row.try_get("status").unwrap();
        let db_src: i64 = row.try_get("src_container_id").unwrap();
        let db_dst: i64 = row.try_get("dst_container_id").unwrap();
        let db_item: String = row.try_get("item_instance_id").unwrap();
        assert_eq!(db_status, "aborted");
        assert_eq!(db_src as u64, c1_id);
        assert_eq!(db_dst as u64, c2_id);
        assert_eq!(db_item, "sword-1101");
    }
}

#[tokio::test]
async fn test_escrow_and_trade_duplicate_items_and_distinct_containers() {
    let db = Database::connect_memory().await.unwrap();
    let container_repo = ContainerRepository::new(db.clone());
    let char_repo = mmo_storage::CharacterRepository::new(db.clone());
    let escrow_coord = mmo_storage::EscrowCoordinator::new(db.clone());
    let market_repo = mmo_storage::MarketplaceRepository::new(db.clone());

    let c_dep = 2001;
    let c_esc = 2002;

    container_repo
        .create_container(
            &ContainerRecordBuilder::new(c_dep)
                .owner_type("character")
                .owner_id("1001")
                .capacity(10)
                .build(),
        )
        .await
        .unwrap();

    container_repo
        .create_container(
            &ContainerRecordBuilder::new(c_esc)
                .owner_type("escrow")
                .owner_id("escrow_system")
                .capacity(10)
                .build(),
        )
        .await
        .unwrap();

    let item = mmo_storage::ItemRecord {
        item_instance_id: "gem-1".to_string(),
        character_guid: 1001,
        container_id: c_dep,
        item_def_id: 99,
        slot: 0,
        quantity: 1,
        durability: 100,
        attributes: serde_json::json!({}),
        created_at: 1000,
        updated_at: 1000,
    };
    char_repo.add_item(&item).await.unwrap();

    // 1. Escrow with identical depositor and escrow container should fail
    let err_same_container = escrow_coord
        .create_escrow(
            "escrow-same",
            c_dep,
            2003,
            c_dep,
            &["gem-1".to_string()],
            "secret",
            60,
        )
        .await;
    assert!(matches!(
        err_same_container,
        Err(StorageError::InvalidParameter(_))
    ));

    // 2. Escrow with duplicate item IDs should fail
    let err_dup_items = escrow_coord
        .create_escrow(
            "escrow-dup",
            c_dep,
            2003,
            c_esc,
            &["gem-1".to_string(), "gem-1".to_string()],
            "secret",
            60,
        )
        .await;
    assert!(matches!(err_dup_items, Err(StorageError::DuplicateItem(_))));

    // 3. Marketplace listing with identical source and escrow container should fail
    let err_market_same = market_repo
        .create_listing(1001, c_dep, 0, 100, 60, c_dep)
        .await;
    assert!(matches!(
        err_market_same,
        Err(StorageError::InvalidParameter(_))
    ));

    // 4. Character trade with duplicate item IDs should fail
    char_repo.create_account("acc1", "user1").await.unwrap();

    char_repo.create_account("acc2", "user2").await.unwrap();

    char_repo
        .create_character("acc1", 1001, "Sender", (0.0, 0.0, 0.0))
        .await
        .unwrap();

    char_repo
        .create_character("acc2", 1002, "Recipient", (0.0, 0.0, 0.0))
        .await
        .unwrap();

    let err_trade_dup = char_repo
        .execute_trade(
            "trade-1",
            1001,
            1002,
            &["gem-1".to_string(), "gem-1".to_string()],
        )
        .await;
    assert!(matches!(err_trade_dup, Err(StorageError::DuplicateItem(_))));
}
