//! End-to-end simulation integration test for modular NPC & AI systems.
//!
//! Verifies the complete autonomous NPC lifecycle on the 20 Hz simulation loop:
//! 1. `SpawnerPlugin` instantiates a monster from template with all components.
//! 2. Mob traverses waypoints in `Patrol` state.
//! 3. Player enters sensory range -> `PerceptionScanSystem` emits `HostileSpottedEvent`.
//! 4. `ThreatSystem` adds threat -> `FsmBrainSystem` transitions mob from `Patrol` to `Chase`.
//! 5. Mob pursues player into combat range and executes attacks via `DamageEvent`.
//! 6. Player flees past leash radius -> `LeashComponent` flags leash-out.
//! 7. Threat table is wiped, FSM transitions to `Evade`, and mob navigates back to spawn origin.

use mmo_core::guid::EntityType;
use mmo_core::{BoundingBox3D, Guid};
use mmo_gameplay::{
    CombatPlugin, DamageEvent, FsmBrainComponent, FsmState, HealthComponent,
    LeashComponent, MonsterAiPlugin, NpcMovementPlugin, PerceptionComponent,
    PerceptionPlugin, SpawnTemplate, SpawnerComponent, SpawnerPlugin, ThreatPlugin,
    ThreatTableComponent, WaypointComponent, WaypointLoopMode,
};
use mmo_node::sim::TickSimulation;

#[tokio::test]
async fn test_end_to_end_npc_ai_simulation_lifecycle() {
    // 1. Initialize simulation with 3D Octree
    let bounds = BoundingBox3D::new((-500.0, -500.0, -500.0), (500.0, 500.0, 500.0));
    let mut sim = TickSimulation::new(1, bounds);

    // 2. Configure Goblin template
    let goblin_template = SpawnTemplate {
        template_id: 101,
        name: "Forest Goblin".to_string(),
        max_health: 80.0,
        speed: 8.0, // 8 units/second
        attack_power: 15,
        armor: 10,
        leash_radius: 20.0, // 20m leash boundary
    };

    // 3. Register standalone gameplay & AI plugins
    sim.add_plugin(Box::new(CombatPlugin));
    sim.add_plugin(Box::new(NpcMovementPlugin));
    sim.add_plugin(Box::new(SpawnerPlugin::new(vec![goblin_template])));
    sim.add_plugin(Box::new(PerceptionPlugin));
    sim.add_plugin(Box::new(ThreatPlugin));
    sim.add_plugin(Box::new(MonsterAiPlugin));
    sim.init_plugins();

    // 4. Spawn a spawner entity at (0, 0, 0)
    let spawner_guid = Guid::new(EntityType::Npc, 1, 100);
    assert!(sim.spawn_entity(spawner_guid, (0.0, 0.0, 0.0)));

    let spawner_comp = SpawnerComponent::new(101, 1, 10.0, 0.0); // max 1 spawn, ready immediately
    sim.entity_store
        .get_mut_by_guid(spawner_guid)
        .unwrap()
        .add_component(spawner_comp);

    // 5. Tick simulation -> SpawnerSystem should auto-instantiate the Goblin mob
    sim.step_tick();

    let spawner_ent = sim.entity_store.get_by_guid(spawner_guid).unwrap();
    let spawned_guids = spawner_ent
        .get_component::<SpawnerComponent>()
        .unwrap()
        .spawned_entities
        .clone();
    assert_eq!(spawned_guids.len(), 1);
    let mob_guid = spawned_guids[0];

    // Configure the spawned mob with waypoints, perception, threat table, and FSM brain
    {
        let mob_ent = sim.entity_store.get_mut_by_guid(mob_guid).unwrap();

        // Waypoints: (0, 0, 0) -> (10, 0, 0) -> (0, 0, 0)
        let waypoints = WaypointComponent::new(
            vec![(0.0, 0.0, 0.0), (10.0, 0.0, 0.0)],
            WaypointLoopMode::PingPong,
        );
        let perception = PerceptionComponent::new(12.0, 2, vec![1]); // Hostile to faction 1 (players) within 12m
        let threat_table = ThreatTableComponent::default_combat();
        let fsm_brain = FsmBrainComponent::new(
            FsmState::Patrol,
            2.0,  // 2m melee attack range
            0.5,  // 0.5s attack cooldown
            1.0,
            20.0, // 20 damage per attack
        );

        mob_ent.add_component(waypoints);
        mob_ent.add_component(perception);
        mob_ent.add_component(threat_table);
        mob_ent.add_component(fsm_brain);
    }

    // 6. Step several ticks -> Mob should patrol toward (10, 0, 0)
    for _ in 0..10 {
        sim.step_tick(); // 10 * 50ms = 0.5s. Speed 8.0 * 0.5 = 4.0 units
    }

    let mob_ent = sim.entity_store.get_by_guid(mob_guid).unwrap();
    assert!(mob_ent.position.0 > 0.0);
    assert_eq!(
        mob_ent.get_component::<FsmBrainComponent>().unwrap().current_state,
        FsmState::Patrol
    );

    // 7. Spawn Player entity at (6.0, 0.0, 0.0) -> Within perception radius (12m)
    let player_guid = Guid::new(EntityType::Player, 1, 500);
    assert!(sim.spawn_entity(player_guid, (6.0, 0.0, 0.0)));
    {
        let player_ent = sim.entity_store.get_mut_by_guid(player_guid).unwrap();
        player_ent.add_component(HealthComponent::new(100.0, 0.0));
        player_ent.add_component(PerceptionComponent::new(20.0, 1, vec![2])); // Player faction 1
    }

    // 8. Step simulation -> Perception detects player, emits HostileSpottedEvent, Threat adds aggro, FSM enters Chase
    sim.step_tick();

    let mob_ent = sim.entity_store.get_by_guid(mob_guid).unwrap();
    assert_eq!(
        mob_ent.get_component::<FsmBrainComponent>().unwrap().current_state,
        FsmState::Chase { target: player_guid }
    );

    // 9. Step simulation until mob closes into 2.0m melee range and attacks
    for _ in 0..15 {
        sim.step_tick();
    }

    let mob_ent = sim.entity_store.get_by_guid(mob_guid).unwrap();
    assert!(matches!(
        mob_ent.get_component::<FsmBrainComponent>().unwrap().current_state,
        FsmState::Attack { target, .. } if target == player_guid
    ));

    // Verify DamageEvent was emitted by the attack state
    let damage_events = sim.event_queue.drain::<DamageEvent>();
    assert!(!damage_events.is_empty());
    assert_eq!(damage_events[0].source, mob_guid);
    assert_eq!(damage_events[0].target, player_guid);
    assert_eq!(damage_events[0].raw_damage, 20.0);

    // 10. Player flees past leash radius (spawn origin is at (0, 0, 0), leash radius is 20m)
    {
        let player_ent = sim.entity_store.get_mut_by_guid(player_guid).unwrap();
        player_ent.position = (100.0, 0.0, 0.0);
        sim.octree.remove(player_guid, (6.0, 0.0, 0.0));
        sim.octree.insert(player_guid, (100.0, 0.0, 0.0));

        let mob_ent = sim.entity_store.get_mut_by_guid(mob_guid).unwrap();
        let old_mob_pos = mob_ent.position;
        mob_ent.position = (25.0, 0.0, 0.0);
        sim.octree.remove(mob_guid, old_mob_pos);
        sim.octree.insert(mob_guid, (25.0, 0.0, 0.0));
    }

    // Next tick: NavSteeringSystem detects position > 20m leash -> flags is_leashed_out
    // ThreatSystem clears threat table
    // FsmBrainSystem transitions to Evade { return_position: (0, 0, 0) }
    sim.step_tick();

    let mob_ent = sim.entity_store.get_by_guid(mob_guid).unwrap();
    assert!(mob_ent.get_component::<LeashComponent>().unwrap().is_leashed_out);
    assert_eq!(
        mob_ent.get_component::<ThreatTableComponent>().unwrap().current_target,
        None
    );
    assert!(matches!(
        mob_ent.get_component::<FsmBrainComponent>().unwrap().current_state,
        FsmState::Evade { .. }
    ));

    // 11. Run simulation steps until mob returns to spawn origin (0, 0, 0) and enters Idle
    for _ in 0..80 {
        sim.step_tick();
    }

    let mob_ent = sim.entity_store.get_by_guid(mob_guid).unwrap();
    assert!(mob_ent.position.0 <= 1.0);
    assert!(!mob_ent.get_component::<LeashComponent>().unwrap().is_leashed_out);
    assert!(matches!(
        mob_ent.get_component::<FsmBrainComponent>().unwrap().current_state,
        FsmState::Idle { .. }
    ));
}
