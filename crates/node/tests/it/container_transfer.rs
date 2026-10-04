//! End-to-end integration test for cross-partition container transfers and generic escrow.

use std::sync::Arc;

use mmo_core::BoundingBox3D;
use mmo_node::network::NodeServiceImpl;
use mmo_node::sim::TickSimulation;
use mmo_schema::cluster::node_service_server::NodeService;
use mmo_schema::cluster::{CommitContainerTransferRequest, ReserveContainerSlotRequest};
use mmo_storage::{
    ContainerRecord, ContainerRepository, Database, EscrowCoordinator, EscrowStatus, ItemRecord,
    TransactionCoordinator,
};

#[tokio::test]
async fn test_e2e_cross_partition_container_transfer_and_escrow_lifecycle() {
    let db = Database::connect_memory().await.unwrap();
    let container_repo = ContainerRepository::new(db.clone());
    let coord = Arc::new(TransactionCoordinator::new(db.clone()));
    let escrow_coord = EscrowCoordinator::new(db.clone());

    // 1. Setup Simulation Node 1 (West Partition) and Node 2 (East Partition)
    let bounds_node1 = BoundingBox3D::new((-100.0, -100.0, -100.0), (0.0, 100.0, 100.0));
    let bounds_node2 = BoundingBox3D::new((0.0, -100.0, -100.0), (100.0, 100.0, 100.0));

    let sim_node1 = TickSimulation::new(1, bounds_node1).into_shared();
    let sim_node2 = TickSimulation::new(2, bounds_node2).into_shared();

    let service_node1 =
        NodeServiceImpl::with_coordinator(Arc::clone(&sim_node1), Arc::clone(&coord));
    let service_node2 =
        NodeServiceImpl::with_coordinator(Arc::clone(&sim_node2), Arc::clone(&coord));

    // 2. Setup Containers:
    // C1: Player Personal Vault on Node 1 (Container ID 10001)
    // C2: Guild Stash on Node 2 (Container ID 10002)
    // C_Escrow: Holding Container (Container ID 10003)
    let c1_id = 10001;
    let c2_id = 10002;
    let c_escrow_id = 10003;

    container_repo
        .create_container(&ContainerRecord {
            container_id: c1_id,
            owner_type: "character".to_string(),
            owner_id: "player_alice".to_string(),
            name: "Alice Inventory".to_string(),
            capacity: 20,
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
            owner_id: "guild_knights".to_string(),
            name: "Guild Stash".to_string(),
            capacity: 50,
            access_policy: "owner_only".to_string(),
            version: 1,
            created_at: 1000,
            updated_at: 1000,
        })
        .await
        .unwrap();

    container_repo
        .create_container(&ContainerRecord {
            container_id: c_escrow_id,
            owner_type: "system".to_string(),
            owner_id: "escrow_system".to_string(),
            name: "Escrow Holding".to_string(),
            capacity: 10,
            access_policy: "public_read_write".to_string(),
            version: 1,
            created_at: 1000,
            updated_at: 1000,
        })
        .await
        .unwrap();

    let char_repo = mmo_storage::CharacterRepository::new(db.clone());

    // Alice has a rare sword in C1 slot 0
    let sword_item = ItemRecord {
        item_instance_id: "mythic_sword_e2e".to_string(),
        character_guid: 1001,
        container_id: c1_id,
        item_def_id: 9999,
        slot: 0,
        quantity: 1,
        durability: 100,
        attributes: serde_json::json!({"enchantment": "fire"}),
        created_at: 1000,
        updated_at: 1000,
    };
    char_repo.add_item(&sword_item).await.unwrap();

    // -------------------------------------------------------------
    // PART A: Cross-Node Container Transfer (C1 on Node 1 -> C2 on Node 2)
    // -------------------------------------------------------------
    let tx_id = "tx_cross_partition_001";

    // Step A.1: Node 1 reserves source slot
    let res_src = service_node1
        .reserve_container_slot(tonic::Request::new(ReserveContainerSlotRequest {
            tx_id: tx_id.to_string(),
            container_id: c1_id,
            slot: 0,
            item_instance_id: "mythic_sword_e2e".to_string(),
            reservation_type: "transfer_out".to_string(),
            lease_duration_ms: 5000,
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(
        res_src.success,
        "Source reservation failed: {}",
        res_src.error_message
    );

    // Step A.2: Node 2 reserves destination slot
    let res_dst = service_node2
        .reserve_container_slot(tonic::Request::new(ReserveContainerSlotRequest {
            tx_id: tx_id.to_string(),
            container_id: c2_id,
            slot: 7,
            item_instance_id: String::new(),
            reservation_type: "transfer_in".to_string(),
            lease_duration_ms: 5000,
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(
        res_dst.success,
        "Destination reservation failed: {}",
        res_dst.error_message
    );

    // Step A.3: Commit transfer via Node 2
    let commit_res = service_node2
        .commit_container_transfer(tonic::Request::new(CommitContainerTransferRequest {
            tx_id: tx_id.to_string(),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(
        commit_res.success,
        "Transfer commit failed: {}",
        commit_res.error_message
    );
    assert_eq!(commit_res.src_container_id, c1_id);
    assert_eq!(commit_res.dst_container_id, c2_id);
    assert_eq!(commit_res.item_instance_id, "mythic_sword_e2e");

    // Verify storage state: item is now in C2 slot 7, C1 slot 0 is vacant
    let c1_item = container_repo.get_item_at_slot(c1_id, 0).await.unwrap();
    assert!(c1_item.is_none());

    let c2_item = container_repo
        .get_item_at_slot(c2_id, 7)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(c2_item.item_instance_id, "mythic_sword_e2e");
    assert_eq!(c2_item.container_id, c2_id);
    assert_eq!(c2_item.slot, 7);

    // Verify simulation ticks received notifications via non-blocking channels
    {
        let mut sim2 = sim_node2.write().await;
        sim2.step(0.05);
        // Step drained notifications without blocking
    }

    // -------------------------------------------------------------
    // PART B: Asynchronous Escrow with Hash Secret Verification
    // -------------------------------------------------------------
    // Now deposit the sword from C2 (Guild Stash) into Escrow holding container
    let secret = "correct_trade_passcode_987";
    let escrow_items = vec!["mythic_sword_e2e".to_string()];

    let escrow_rec = escrow_coord
        .create_escrow(
            "escrow_e2e_1",
            c2_id,
            c1_id,
            c_escrow_id,
            &escrow_items,
            secret,
            5,
        )
        .await
        .unwrap();

    assert_eq!(escrow_rec.status, EscrowStatus::Held);

    // Verify item is now in escrow container
    let in_escrow = container_repo
        .get_item("mythic_sword_e2e")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(in_escrow.container_id, c_escrow_id);

    // Test rejection with wrong secret (constant-time comparison)
    let wrong_release = escrow_coord
        .release_escrow("escrow_e2e_1", "wrong_secret_passcode")
        .await;
    assert!(wrong_release.is_err());

    // Release with correct secret -> item moves to beneficiary C1
    let released_ok = escrow_coord
        .release_escrow("escrow_e2e_1", secret)
        .await
        .unwrap();
    assert!(released_ok);

    let settled_escrow = escrow_coord
        .get_escrow("escrow_e2e_1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(settled_escrow.status, EscrowStatus::Settled);

    let alice_item = container_repo
        .get_item("mythic_sword_e2e")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(alice_item.container_id, c1_id);

    // -------------------------------------------------------------
    // PART C: Escrow Timeout Refund
    // -------------------------------------------------------------
    // Create an expiring escrow from C1 to C2 with 0s TTL (immediately expired)
    let expiry_secret = "refund_secret";
    let refund_items = vec!["mythic_sword_e2e".to_string()];

    let expiring_escrow = escrow_coord
        .create_escrow(
            "escrow_e2e_refund",
            c1_id,
            c2_id,
            c_escrow_id,
            &refund_items,
            expiry_secret,
            0, // 0 second TTL - immediately expired
        )
        .await
        .unwrap();
    assert_eq!(expiring_escrow.status, EscrowStatus::Held);

    // Trigger reaper directly without sleep
    let refunded_count = escrow_coord.reap_expired_escrows().await.unwrap();
    assert_eq!(refunded_count, 1);

    // Verify escrow status is Refunded
    let refunded_escrow = escrow_coord
        .get_escrow("escrow_e2e_refund")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refunded_escrow.status, EscrowStatus::Refunded);

    // Verify item was refunded back to depositor C1
    let refunded_item = container_repo
        .get_item("mythic_sword_e2e")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(refunded_item.container_id, c1_id);
}
