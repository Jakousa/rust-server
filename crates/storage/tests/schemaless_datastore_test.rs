//! Comprehensive integration tests for schemaless DataStore ingestion,
//! wait-free concurrency, and multi-format asset indexing.

use std::fs;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use mmo_core::{DynamicProperties, Entity, EntityType, Guid, PropertyValue};
use mmo_storage::{DataDocument, DataStore};

#[test]
fn test_datastore_wait_free_concurrency_stress() {
    let store = Arc::new(DataStore::new());

    // Initial population
    for i in 0..100 {
        let mut props = DynamicProperties::new();
        props.set("power", i as i64 * 10);
        props.set("name", format!("Asset_{}", i));
        store.insert_document(DataDocument::new("items", format!("item_{}", i), props));
    }

    let running = Arc::new(AtomicBool::new(true));
    let read_ops = Arc::new(AtomicUsize::new(0));

    // Spawn 8 reader threads simulating 20Hz tick access
    let mut reader_handles = Vec::new();
    for thread_idx in 0..8 {
        let store_clone = Arc::clone(&store);
        let running_clone = Arc::clone(&running);
        let read_ops_clone = Arc::clone(&read_ops);

        reader_handles.push(thread::spawn(move || {
            let mut local_reads = 0;
            while running_clone.load(Ordering::Relaxed) {
                let target_id = format!("item_{}", (local_reads + thread_idx) % 100);
                if let Some(doc) = store_clone.get("items", &target_id) {
                    assert!(doc.get("power").is_some());
                    local_reads += 1;
                }
            }
            read_ops_clone.fetch_add(local_reads, Ordering::Relaxed);
        }));
    }

    // Spawn writer thread performing atomic hot-reloads
    let store_clone = Arc::clone(&store);
    let writer_handle = thread::spawn(move || {
        for reload in 1..=5 {
            thread::sleep(Duration::from_millis(20));
            for i in 0..100 {
                let mut props = DynamicProperties::new();
                props.set("power", (i as i64 * 10) + (reload * 1000));
                props.set("reload_version", reload);
                store_clone.insert_document(DataDocument::new(
                    "items",
                    format!("item_{}", i),
                    props,
                ));
            }
        }
    });

    writer_handle.join().unwrap();
    running.store(false, Ordering::Relaxed);

    for handle in reader_handles {
        handle.join().unwrap();
    }

    let total_reads = read_ops.load(Ordering::Relaxed);
    assert!(
        total_reads > 10_000,
        "Expected high read throughput in wait-free store, got: {}",
        total_reads
    );

    // Verify all items reflect final reload version
    for i in 0..100 {
        let doc = store.get("items", &format!("item_{}", i)).unwrap();
        assert_eq!(doc.get_i64("reload_version"), Some(5));
    }
}

#[test]
fn test_mixed_format_recursive_directory_ingestion() {
    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path();

    let spells_dir = root.join("spells");
    let armor_dir = root.join("armor");
    fs::create_dir_all(&spells_dir).unwrap();
    fs::create_dir_all(&armor_dir).unwrap();

    // 1. JSON file with single spell
    let fireball = r#"{
        "id": "fireball",
        "damage": 120,
        "mana_cost": 30,
        "combat": {
            "element": "fire",
            "aoe_radius": 5.5
        }
    }"#;
    fs::write(spells_dir.join("fireball.json"), fireball).unwrap();

    // 2. MessagePack file with single spell
    let mut frostbolt = serde_json::Map::new();
    frostbolt.insert("id".to_string(), serde_json::json!("frostbolt"));
    frostbolt.insert("damage".to_string(), serde_json::json!(95));
    frostbolt.insert("mana_cost".to_string(), serde_json::json!(25));
    let mut combat_sub = serde_json::Map::new();
    combat_sub.insert("element".to_string(), serde_json::json!("frost"));
    combat_sub.insert("slow_pct".to_string(), serde_json::json!(0.40));
    frostbolt.insert("combat".to_string(), serde_json::Value::Object(combat_sub));

    let mpk_bytes = rmp_serde::to_vec(&serde_json::Value::Object(frostbolt)).unwrap();
    fs::write(spells_dir.join("frostbolt.mpk"), mpk_bytes).unwrap();

    // 3. Array of armor pieces in JSON
    let armors = r#"[
        {"id": "iron_helm", "armor": 15, "weight": 4.0},
        {"id": "plate_chest", "armor": 45, "weight": 18.0}
    ]"#;
    fs::write(armor_dir.join("heavy.JSON"), armors).unwrap();

    let store = DataStore::new();
    let report = store.load_directory(root);
    assert!(report.is_ok());
    assert_eq!(report.loaded, 4);

    // Verify spells
    let fb = store.get("spells", "fireball").unwrap();
    assert_eq!(fb.get_i64("damage"), Some(120));
    assert_eq!(
        fb.get_path("combat.element")
            .and_then(PropertyValue::as_str),
        Some("fire")
    );
    assert_eq!(
        fb.get_path("combat.aoe_radius")
            .and_then(PropertyValue::as_f64),
        Some(5.5)
    );

    let fr = store.get("spells", "frostbolt").unwrap();
    assert_eq!(fr.get_i64("damage"), Some(95));
    assert_eq!(
        fr.get_path("combat.element")
            .and_then(PropertyValue::as_str),
        Some("frost")
    );
    assert_eq!(
        fr.get_path("combat.slow_pct")
            .and_then(PropertyValue::as_f64),
        Some(0.40)
    );

    // Verify armors
    let helm = store.get("armor", "iron_helm").unwrap();
    assert_eq!(helm.get_i64("armor"), Some(15));
    let chest = store.get("armor", "plate_chest").unwrap();
    assert_eq!(chest.get_i64("armor"), Some(45));

    // Test apply to entity
    let mut ent = Entity::new(Guid::new(EntityType::Player, 1, 42), (0.0, 0.0, 0.0));
    store
        .apply_to_entity("armor", "plate_chest", &mut ent)
        .unwrap();
    assert_eq!(ent.get_property_i64("armor"), Some(45));
    assert_eq!(ent.get_property_f64("weight"), Some(18.0));
}
