//! Concurrent stress and deadlock-freedom verification for distributed containers.

use mmo_storage::{
    ContainerRecord, ContainerRepository, Database, ItemRecord, StorageError,
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
        .create_container(&ContainerRecord {
            container_id: c1_id,
            owner_type: "guild".to_string(),
            owner_id: "guild_alpha".to_string(),
            name: "Guild Vault Alpha".to_string(),
            capacity: 100,
            access_policy: "owner_only".to_string(),
            version: 1,
            created_at: 1000,
            updated_at: 1000,
        })
        .await
        .unwrap();

    container_repo
        .create_container(&ContainerRecord {
            container_id: c2_id,
            owner_type: "guild".to_string(),
            owner_id: "guild_beta".to_string(),
            name: "Guild Vault Beta".to_string(),
            capacity: 100,
            access_policy: "owner_only".to_string(),
            version: 1,
            created_at: 1000,
            updated_at: 1000,
        })
        .await
        .unwrap();

    let char_repo = mmo_storage::CharacterRepository::new(db.clone());

    // Populate C1 with 20 items in slots 0..20
    for i in 0..20 {
        char_repo
            .add_item(&ItemRecord {
                item_instance_id: format!("c1_item_{}", i),
                character_guid: 1001,
                container_id: c1_id,
                item_def_id: 100 + i,
                slot: i,
                quantity: 1,
                durability: 100,
                attributes: serde_json::json!({}),
                created_at: 1000,
                updated_at: 1000,
            })
            .await
            .unwrap();
    }

    // Populate C2 with 20 items in slots 0..20
    for i in 0..20 {
        char_repo
            .add_item(&ItemRecord {
                item_instance_id: format!("c2_item_{}", i),
                character_guid: 1002,
                container_id: c2_id,
                item_def_id: 200 + i,
                slot: i,
                quantity: 1,
                durability: 100,
                attributes: serde_json::json!({}),
                created_at: 1000,
                updated_at: 1000,
            })
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
