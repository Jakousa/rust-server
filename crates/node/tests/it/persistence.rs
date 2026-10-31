//! End-to-end integration test for persistent state and recovery across simulation restarts.
//!
//! Validates:
//! 1. Account and character creation in durable storage (SQLite)
//! 2. Continuous player movement and asynchronous write-behind snapshotting
//! 3. Simulation crash / restart recovery: restoring entity state in a fresh simulation instance

use mmo_core::guid::{EntityType, Guid};
use mmo_core::BoundingBox3D;
use mmo_node::sim::TickSimulation;
use mmo_node::snapshotter::WriteBehindSnapshotter;
use mmo_storage::db::Database;
use mmo_storage::models::EntitySnapshot;
use mmo_storage::repository::CharacterRepository;
use serde_json::json;

#[tokio::test]
async fn test_end_to_end_persistence_and_simulation_recovery() {
    // 1. Initialize durable database with automatic schema migrations
    let db = Database::connect_memory()
        .await
        .expect("Failed to initialize in-memory SQLite database");
    let repo = CharacterRepository::new(db);

    // 2. Account and Character creation
    let account = repo
        .create_account("acc_001", "valiant_hero")
        .await
        .expect("Failed to create account");
    assert_eq!(account.account_id, "acc_001");
    assert_eq!(account.username, "valiant_hero");

    let player_guid = Guid::new(EntityType::Player, 1, 55001);
    let character = repo
        .create_character(
            &account.account_id,
            player_guid.raw(),
            "ValiantKnight",
            (0.0, 0.0, 0.0),
        )
        .await
        .expect("Failed to create character");
    assert_eq!(character.guid, player_guid.raw());
    assert_eq!(character.name, "ValiantKnight");
    assert_eq!(character.position, (0.0, 0.0, 0.0));

    // 3. Start Node 1 simulation and WriteBehindSnapshotter
    let bounds = BoundingBox3D::new((-500.0, -500.0, -500.0), (500.0, 500.0, 500.0));
    let mut node1_sim = TickSimulation::new(1, bounds);
    let snapshotter = WriteBehindSnapshotter::new(repo.clone());

    // Spawn player in Node 1 simulation
    let entity_key = node1_sim.entity_store.spawn(player_guid, (0.0, 0.0, 0.0));
    assert_eq!(node1_sim.entity_store.count(), 1);

    // 4. Simulate continuous player movement across 5 ticks with write-behind snapshotting
    let mut current_pos = (0.0, 0.0, 0.0);
    let mut current_yaw = 0.0;

    for tick in 1..=5 {
        // Move entity by 2.0 units per tick along the X axis
        current_pos.0 += 2.0;
        current_pos.1 = 10.0;
        current_pos.2 = 5.0;
        current_yaw += 0.1;

        // Update entity position inside the simulation store
        if let Some(entity) = node1_sim.entity_store.get_mut(entity_key) {
            entity.position = current_pos;
            entity.rotation = current_yaw;
        }

        // Stage snapshot to non-blocking write-behind queue
        let snapshot = EntitySnapshot {
            guid: player_guid.raw(),
            position: current_pos,
            rotation: current_yaw,
            attributes: json!({
                "level": 15,
                "health": 100 - tick,
                "mana": 50 + tick
            }),
            state_blob: Some(vec![1, 2, 3, tick as u8]),
            version: tick,
            timestamp: 1000 + tick,
        };
        snapshotter.mark_dirty(player_guid, snapshot);

        // Simulation ticks forward at 20 Hz without blocking on DB
        node1_sim.step_tick();
    }

    // Flush pending snapshots to durable storage
    let flushed = snapshotter.flush_batch().await;
    assert_eq!(flushed, 1); // 1 entity coalesced from 5 rapid ticks

    // 5. Verify snapshot in persistent database
    let persisted_snapshot = repo
        .get_snapshot(player_guid.raw())
        .await
        .expect("Query snapshot failed")
        .expect("Snapshot should exist in database");

    assert_eq!(persisted_snapshot.guid, player_guid.raw());
    assert_eq!(persisted_snapshot.position, (10.0, 10.0, 5.0));
    assert!((persisted_snapshot.rotation - 0.5).abs() < 1e-5);
    assert_eq!(persisted_snapshot.version, 5);
    assert_eq!(persisted_snapshot.attributes["level"], json!(15));
    assert_eq!(persisted_snapshot.attributes["health"], json!(95));
    assert_eq!(persisted_snapshot.state_blob, Some(vec![1, 2, 3, 5]));

    // Verify entity_snapshots is the authoritative runtime persistence layer
    // (characters table is decoupled and updated via character-specific workflows)
    let loaded_character = repo
        .get_character(player_guid.raw())
        .await
        .expect("Query character failed")
        .expect("Character record should exist");
    assert_eq!(loaded_character.name, "ValiantKnight");

    // 6. Simulate Node 1 crash / restart: drop node 1 simulation entirely
    drop(node1_sim);
    drop(snapshotter);

    // 7. Spin up a brand new simulation instance (restarted node)
    let mut restarted_node_sim = TickSimulation::new(1, bounds);
    assert_eq!(restarted_node_sim.entity_store.count(), 0);

    // Recover player state from durable repository
    let recovered_snapshot = repo
        .get_snapshot(player_guid.raw())
        .await
        .expect("Query snapshot on restart failed")
        .expect("Snapshot must persist across crash");

    let recovered_guid = Guid::from_raw(recovered_snapshot.guid);
    let recovered_key = restarted_node_sim
        .entity_store
        .spawn(recovered_guid, recovered_snapshot.position);

    // Verify entity was restored into simulation entity store
    assert_eq!(restarted_node_sim.entity_store.count(), 1);
    let recovered_entity = restarted_node_sim
        .entity_store
        .get(recovered_key)
        .expect("Entity should exist in restarted store");
    assert_eq!(recovered_entity.guid, player_guid);
    assert_eq!(recovered_entity.position, (10.0, 10.0, 5.0));

    // Verify coordinates and attributes match pre-crash state
    assert_eq!(recovered_snapshot.position, (10.0, 10.0, 5.0));
    assert_eq!(recovered_snapshot.version, 5);
    assert_eq!(recovered_snapshot.attributes["health"], json!(95));

    // Simulation continues ticking normally with recovered entity
    restarted_node_sim.step_tick();
    assert_eq!(restarted_node_sim.tick_counter, 1);
    assert_eq!(restarted_node_sim.entity_store.count(), 1);
}
