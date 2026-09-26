//! End-to-end integration test verifying plugin loading, encumbered movement veto,
//! combat damage & batch regen, atomic trading, and dynamic snapshot persistence recovery.

use mmo_core::guid::EntityType;
use mmo_core::{BoundingBox3D, Guid};
use mmo_gameplay::{
    process_damage_events, CombatPlugin, DamageEvent, HealthComponent, InventoryComponent,
    InventoryPlugin, ItemStack, StatsComponent, TradePlugin,
};
use mmo_node::sim::TickSimulation;
use mmo_node::snapshotter::WriteBehindSnapshotter;
use mmo_storage::repository::CharacterRepository;
use mmo_storage::Database;

#[tokio::test]
async fn test_end_to_end_gameplay_plugins_lifecycle_and_persistence() {
    // 1. Initialize simulation with Octree bounds
    let bounds = BoundingBox3D {
        min: (-1000.0, -1000.0, -1000.0),
        max: (1000.0, 1000.0, 1000.0),
    };
    let mut sim = TickSimulation::new(1, bounds);

    // 2. Register standalone gameplay plugins
    sim.add_plugin(Box::new(InventoryPlugin));
    sim.add_plugin(Box::new(CombatPlugin));
    sim.add_plugin(Box::new(TradePlugin));
    sim.init_plugins();

    // 3. Spawn player entity with dynamic components
    let player_guid = Guid::new(EntityType::Player, 1, 1001);
    assert!(sim.spawn_entity(player_guid, (10.0, 0.0, 10.0)));

    {
        let player = sim.entity_store.get_mut_by_guid(player_guid).unwrap();
        // Bag with capacity 5, max weight 50.0
        let inv = InventoryComponent::new(5, 50.0);
        let health = HealthComponent::new(100.0, 10.0); // 10 HP/sec regen
        let stats = StatsComponent::new(25, 100); // 100 armor = 50% damage mitigation

        player.add_component(inv);
        player.add_component(health);
        player.add_component(stats);
    }

    // 4. Test normal movement passes kinematic and component validation (dist ~0.28m in 0.05s = 5.6m/s < 10m/s)
    let move_res = sim.process_client_move(player_guid, (10.2, 0.0, 10.2), 0.05);
    assert!(move_res.is_ok());
    assert_eq!(move_res.unwrap(), (10.2, 0.0, 10.2));

    // 5. Test encumbrance veto: add overweight item
    {
        let player = sim.entity_store.get_mut_by_guid(player_guid).unwrap();
        let inv = player.get_component_mut::<InventoryComponent>().unwrap();
        let heavy_ore = ItemStack::new(999, "Heavy Mithril Ore", 1, 60.0); // 60.0 > 50.0 max weight
        inv.add_item(heavy_ore).unwrap();
        assert!(inv.is_encumbered());
    }

    // Attempted movement should be vetoed and return authoritative snap-back position
    let encumbered_res = sim.process_client_move(player_guid, (10.4, 0.0, 10.4), 0.05);
    assert!(encumbered_res.is_err());
    assert_eq!(encumbered_res.unwrap_err(), (10.2, 0.0, 10.2));

    // Remove the heavy item so player can move again
    {
        let player = sim.entity_store.get_mut_by_guid(player_guid).unwrap();
        let inv = player.get_component_mut::<InventoryComponent>().unwrap();
        inv.remove_item(0);
        assert!(!inv.is_encumbered());
    }

    // 6. Test combat damage mitigation via event queue
    let enemy_guid = Guid::new(EntityType::Npc, 1, 5001);
    sim.event_queue.push(DamageEvent {
        source: enemy_guid,
        target: player_guid,
        raw_damage: 60.0, // 60 raw damage with 100 armor = 30 effective damage
    });

    let deaths = process_damage_events(&mut sim.event_queue, &mut sim.entity_store);
    assert_eq!(deaths.len(), 0); // Player survived

    let current_hp = sim
        .entity_store
        .get_by_guid(player_guid)
        .unwrap()
        .get_component::<HealthComponent>()
        .unwrap()
        .current_hp;
    assert_eq!(current_hp, 70.0); // 100.0 - 30.0

    // 7. Step simulation ticks: verify batch HealthRegenSystem regenerates health
    // Run 10 ticks = 0.5s * 10 HP/sec = +5.0 HP
    for _ in 0..10 {
        sim.step_tick();
    }

    let regenerated_hp = sim
        .entity_store
        .get_by_guid(player_guid)
        .unwrap()
        .get_component::<HealthComponent>()
        .unwrap()
        .current_hp;
    assert_eq!(regenerated_hp, 75.0);

    // 8. Test persistence and snapshot recovery
    let db = Database::connect_memory().await.unwrap();
    let repo = CharacterRepository::new(db.clone());

    // Capture dynamic snapshot using registered codecs
    let player_entity = sim.entity_store.get_by_guid(player_guid).unwrap();
    let snapshot = WriteBehindSnapshotter::capture_entity_snapshot(player_entity, &sim.codecs, 1);

    // Verify snapshot captured all registered components
    assert!(snapshot.attributes.get("inventory").is_some());
    assert!(snapshot.attributes.get("health").is_some());
    assert!(snapshot.attributes.get("stats").is_some());
    assert_eq!(snapshot.attributes["stats"]["armor"], 100);

    // Save snapshot to database
    repo.save_snapshot(&snapshot).await.unwrap();

    // 9. Simulate node recovery / restart: load from database and restore dynamic components
    let loaded_snapshot = repo.get_snapshot(player_guid.raw()).await.unwrap().unwrap();
    assert_eq!(loaded_snapshot.guid, player_guid.raw());

    // Create a fresh entity without pre-attached components
    let mut restored_player =
        mmo_core::Entity::new(Guid(loaded_snapshot.guid), loaded_snapshot.position);

    let restored_count = WriteBehindSnapshotter::restore_entity_components(
        &mut restored_player,
        &loaded_snapshot,
        &sim.codecs,
    )
    .unwrap();

    assert_eq!(restored_count, 3); // inventory, health, stats
    let restored_health = restored_player.get_component::<HealthComponent>().unwrap();
    assert_eq!(restored_health.current_hp, 75.0);
    let restored_stats = restored_player.get_component::<StatsComponent>().unwrap();
    assert_eq!(restored_stats.armor, 100);
}
