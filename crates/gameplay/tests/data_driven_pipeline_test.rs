//! End-to-end integration test proving data-driven gameplay pipelines
//! without backend Rust code recompilation.

use std::fs;

use mmo_core::{
    BoundingBox3D, EntityStore, EntityType, EventQueue, Guid, Octree, TickContext, TickSystem,
};
use mmo_gameplay::{
    process_damage_events, DamageEvent, HealthComponent, SpawnTemplate, SpawnerComponent,
    SpawnerSystem, StatsComponent,
};
use mmo_storage::DataStore;

#[test]
fn test_data_driven_content_pipeline_and_hot_reload() {
    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path();

    let monsters_dir = root.join("monsters");
    let items_dir = root.join("items");
    fs::create_dir_all(&monsters_dir).unwrap();
    fs::create_dir_all(&items_dir).unwrap();

    // 1. Initial Designer Asset Definitions (v1)
    let goblin_v1_json = r#"{
        "id": "goblin_scout",
        "name": "Goblin Scout",
        "max_health": 80.0,
        "speed": 7.0,
        "attack_power": 15,
        "armor": 4,
        "leash_radius": 15.0,
        "element": "poison",
        "bonus_damage": 5.0
    }"#;
    let goblin_path = monsters_dir.join("goblin_scout.json");
    fs::write(&goblin_path, goblin_v1_json).unwrap();

    let shield_json = r#"{
        "id": "buckler",
        "name": "Iron Buckler",
        "damage_taken_multiplier": 0.60,
        "damage_reduction": 3.0
    }"#;
    fs::write(items_dir.join("buckler.json"), shield_json).unwrap();

    // 2. Node Startup: Ingest all schemaless assets into DataStore
    let data_store = DataStore::new();
    let report = data_store.load_directory(root);
    assert!(report.is_ok());
    assert_eq!(report.loaded, 2);

    // 3. Spawner System pulls template dynamically from DataStore
    let goblin_doc = data_store
        .get("monsters", "goblin_scout")
        .expect("goblin_scout must exist in DataStore");
    let spawner_template = SpawnTemplate::from_properties(101, &goblin_doc.properties);
    assert_eq!(spawner_template.max_health, 80.0);
    assert_eq!(spawner_template.attack_power, 15);

    let mut spawner_system = SpawnerSystem::new();
    spawner_system.register_template(spawner_template);

    // 4. Simulate Spawner Tick and Entity Instantiation
    let mut entities = EntityStore::new();
    let mut octree = Octree::new(BoundingBox3D::new(
        (-100.0, -100.0, -100.0),
        (100.0, 100.0, 100.0),
    ));
    let mut events = EventQueue::new();

    let spawner_guid = Guid::new(EntityType::Npc, 1, 1);
    let spawner_key = entities.spawn(spawner_guid, (0.0, 0.0, 0.0));
    entities
        .get_mut(spawner_key)
        .unwrap()
        .add_component(SpawnerComponent::new(101, 1, 10.0, 2.0));

    {
        let mut ctx = TickContext {
            entities: &mut entities,
            octree: &mut octree,
            events: &mut events,
            dt_sec: 0.1,
            tick: 1,
            collision_world: None,
        };
        spawner_system.update(&mut ctx);
    }

    // Verify spawned goblin has dynamic properties attached from JSON
    let spawned_goblin = entities
        .iter()
        .find(|(k, _)| *k != spawner_key)
        .map(|(_, e)| e)
        .expect("Goblin must be spawned");
    let goblin_guid = spawned_goblin.guid;
    assert_eq!(
        spawned_goblin
            .get_component::<HealthComponent>()
            .unwrap()
            .max_hp,
        80.0
    );
    assert_eq!(
        spawned_goblin
            .get_component::<StatsComponent>()
            .unwrap()
            .attack_power,
        15
    );
    assert_eq!(spawned_goblin.get_property_str("element"), Some("poison"));
    assert_eq!(spawned_goblin.get_property_f64("bonus_damage"), Some(5.0));

    // 5. Create Defender entity and equip Iron Buckler from DataStore
    let defender_guid = Guid::new(EntityType::Player, 1, 42);
    let def_key = entities.spawn(defender_guid, (1.0, 0.0, 0.0));
    let defender = entities.get_mut(def_key).unwrap();
    defender.add_component(HealthComponent::new(100.0, 0.0));
    data_store
        .apply_to_entity("items", "buckler", defender)
        .expect("Buckler applied to defender");

    assert_eq!(
        defender.get_property_f64("damage_taken_multiplier"),
        Some(0.60)
    );
    assert_eq!(defender.get_property_f64("damage_reduction"), Some(3.0));

    // 6. Combat resolution using dynamic properties
    // Goblin raw attack = 20.0 + bonus_damage(5.0) = 25.0
    // Mitigated by buckler: 25.0 * 0.60 = 15.0 - 3.0 = 12.0 damage
    events.push(DamageEvent {
        source: goblin_guid,
        target: defender_guid,
        raw_damage: 20.0,
    });

    process_damage_events(&mut events, &mut entities);

    let def_health = entities
        .get_by_guid(defender_guid)
        .unwrap()
        .get_component::<HealthComponent>()
        .unwrap()
        .current_hp;
    assert_eq!(def_health, 88.0); // 100.0 - 12.0 = 88.0

    // 7. CONTENT PATCH / HOT RELOAD (without backend recompilation!)
    // Designer edits goblin_scout.json to buff max_health to 120 and bonus_damage to 15
    let goblin_v2_json = r#"{
        "id": "goblin_scout",
        "name": "Enraged Goblin Scout",
        "max_health": 120.0,
        "speed": 8.5,
        "attack_power": 30,
        "armor": 8,
        "leash_radius": 25.0,
        "element": "chaos",
        "bonus_damage": 15.0
    }"#;
    fs::write(&goblin_path, goblin_v2_json).unwrap();

    // Reload modified file into DataStore
    let reloaded = data_store
        .load_file(&goblin_path)
        .expect("File reloads cleanly");
    assert_eq!(reloaded, 1);

    // Verify updated asset in DataStore
    let updated_doc = data_store.get("monsters", "goblin_scout").unwrap();
    assert_eq!(updated_doc.get_f64("max_health"), Some(120.0));
    assert_eq!(updated_doc.get_str("element"), Some("chaos"));
    assert_eq!(updated_doc.get_f64("bonus_damage"), Some(15.0));

    // Register updated template into Spawner
    let updated_template = SpawnTemplate::from_properties(101, &updated_doc.properties);
    assert_eq!(updated_template.max_health, 120.0);
    spawner_system.register_template(updated_template);

    // Despawn old goblin and trigger respawn
    entities.despawn_by_guid(goblin_guid);
    entities
        .get_mut(spawner_key)
        .unwrap()
        .get_component_mut::<SpawnerComponent>()
        .unwrap()
        .current_cooldown_secs = 0.0;

    {
        let mut ctx = TickContext {
            entities: &mut entities,
            octree: &mut octree,
            events: &mut events,
            dt_sec: 0.1,
            tick: 2,
            collision_world: None,
        };
        spawner_system.update(&mut ctx);
    }

    // New spawned entity immediately reflects the newly patched JSON balance values!
    let new_goblin = entities
        .iter()
        .find(|(k, _)| *k != spawner_key && *k != def_key)
        .map(|(_, e)| e)
        .expect("New goblin must be spawned");
    assert_eq!(
        new_goblin
            .get_component::<HealthComponent>()
            .unwrap()
            .max_hp,
        120.0
    );
    assert_eq!(
        new_goblin
            .get_component::<StatsComponent>()
            .unwrap()
            .attack_power,
        30
    );
    assert_eq!(new_goblin.get_property_str("element"), Some("chaos"));
    assert_eq!(new_goblin.get_property_f64("bonus_damage"), Some(15.0));
}
