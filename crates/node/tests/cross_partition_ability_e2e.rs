//! End-to-end integration tests for Cross-Partition Interaction & Boundary Ability Dispatch.
//!
//! Spawns two live simulation nodes (Node 1 and Node 2) connected via Tonic gRPC,
//! verifying:
//! 1. Targeted remote ability dispatch against replicated boundary ghosts.
//! 2. Boundary-crossing Area-of-Effect cleaves hitting local and remote entities concurrently.
//! 3. Active migration lease veto (`TARGET_MIGRATING`) and non-blocking caster compensation.
//! 4. Authoritative range re-validation ($1.2\times$ tolerance) veto (`OUT_OF_RANGE`) and refund.

use std::sync::Arc;
use std::time::Duration;

use mmo_core::guid::{EntityType, Guid};
use mmo_core::{ActionInterruptFlags, AttributeSet, BoundingBox3D, Vec3};
use mmo_gameplay::ability::{AbilityDefinition, CooldownTracker};
use mmo_gameplay::combat::HealthComponent;
use mmo_node::ability_dispatcher::{
    cast_boundary_sphere_aoe, cast_cross_partition_ability, CastOutcome, CrossNodeDispatcher,
};
use mmo_node::network::{GhostRecord, NodeServiceImpl};
use mmo_node::TickSimulation;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

async fn create_node(
    node_id: u16,
    bounds: BoundingBox3D,
) -> (
    Arc<tokio::sync::RwLock<TickSimulation>>,
    std::net::SocketAddr,
) {
    let sim = Arc::new(tokio::sync::RwLock::new(TickSimulation::new(
        node_id, bounds,
    )));
    let service = NodeServiceImpl::new(Arc::clone(&sim));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        Server::builder()
            .add_service(service.into_server())
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    (sim, addr)
}

#[tokio::test]
async fn test_e2e_cross_partition_targeted_ability() {
    // Node 1: [0..50, 0..100, 0..100]
    let (node1_sim, _node1_addr) =
        create_node(1, BoundingBox3D::new((0.0, 0.0, 0.0), (50.0, 100.0, 100.0))).await;
    // Node 2: [50..100, 0..100, 0..100]
    let (node2_sim, node2_addr) = create_node(
        2,
        BoundingBox3D::new((50.0, 0.0, 0.0), (100.0, 100.0, 100.0)),
    )
    .await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    let caster = Guid::new(EntityType::Player, 1, 101);
    let target = Guid::new(EntityType::Player, 2, 202);

    // Setup Caster on Node 1 at (45, 10, 10)
    {
        let mut sim1 = node1_sim.write().await;
        sim1.spawn_entity(caster, (45.0, 10.0, 10.0));
        let ent = sim1.entity_store.get_mut_by_guid(caster).unwrap();
        let mut attrs = AttributeSet::new();
        attrs.add_attribute("mana", 100.0, 0.0, 100.0, 0.0);
        ent.add_component(attrs);
        ent.add_component(CooldownTracker::new());

        // Replicate Ghost of target from Node 2
        sim1.ghost_store.insert_or_update(GhostRecord {
            guid: target,
            position: (55.0, 10.0, 10.0),
            velocity: (0.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            component_data: Vec::new(),
            last_tick: 1,
            source_node_id: 2,
        });
    }

    // Setup Target on Node 2 at (55, 10, 10) with 100 HP
    {
        let mut sim2 = node2_sim.write().await;
        sim2.spawn_entity(target, (55.0, 10.0, 10.0));
        let ent = sim2.entity_store.get_mut_by_guid(target).unwrap();
        ent.add_component(HealthComponent::new(100.0, 0.0));
    }

    let mut dispatcher = CrossNodeDispatcher::new(1);
    dispatcher.add_peer(2, format!("http://{}", node2_addr));

    let fireball = AbilityDefinition {
        id: 11,
        name: "Fireball".into(),
        cast_time_sec: 0.0,
        cooldown_sec: 5.0,
        gcd_sec: 1.5,
        range: 20.0,
        resource_cost: Some(("mana".into(), 25.0)),
        interrupt_flags: ActionInterruptFlags::default(),
    };

    // Cast ability
    let outcome = {
        let mut sim1 = node1_sim.write().await;
        cast_cross_partition_ability(
            &mut sim1,
            &dispatcher,
            caster,
            &fireball,
            target,
            40, // 40 damage
            0,
        )
    };

    assert_eq!(
        outcome,
        Ok(CastOutcome::RemoteDispatched { target_node_id: 2 })
    );

    // Caster state on Node 1 should immediately reflect mana cost and cooldown
    {
        let sim1 = node1_sim.read().await;
        let ent = sim1.entity_store.get_by_guid(caster).unwrap();
        let mana = ent
            .get_component::<AttributeSet>()
            .unwrap()
            .get_current("mana")
            .unwrap();
        assert_eq!(mana, 75.0);

        let cd = ent.get_component::<CooldownTracker>().unwrap();
        assert!(cd.is_on_cooldown(11));
    }

    // Wait for gRPC execution to complete on Node 2
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Verify target health on Node 2 was decremented
    {
        let sim2 = node2_sim.read().await;
        let ent = sim2.entity_store.get_by_guid(target).unwrap();
        let health = ent.get_component::<HealthComponent>().unwrap();
        assert_eq!(health.current_hp, 60.0);
    }
}

#[tokio::test]
async fn test_e2e_cross_partition_boundary_aoe() {
    let (node1_sim, _node1_addr) =
        create_node(1, BoundingBox3D::new((0.0, 0.0, 0.0), (50.0, 100.0, 100.0))).await;
    let (node2_sim, node2_addr) = create_node(
        2,
        BoundingBox3D::new((50.0, 0.0, 0.0), (100.0, 100.0, 100.0)),
    )
    .await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    let caster = Guid::new(EntityType::Player, 1, 301);
    let local_enemy = Guid::new(EntityType::Npc, 1, 302);
    let remote_enemy = Guid::new(EntityType::Player, 2, 401);

    // Setup Caster at (48, 20, 20) and Local Enemy at (46, 20, 20)
    {
        let mut sim1 = node1_sim.write().await;
        sim1.spawn_entity(caster, (48.0, 20.0, 20.0));
        let ent = sim1.entity_store.get_mut_by_guid(caster).unwrap();
        let mut attrs = AttributeSet::new();
        attrs.add_attribute("energy", 100.0, 0.0, 100.0, 0.0);
        ent.add_component(attrs);
        ent.add_component(CooldownTracker::new());

        sim1.spawn_entity(local_enemy, (46.0, 20.0, 20.0));
        let ent_enemy = sim1.entity_store.get_mut_by_guid(local_enemy).unwrap();
        ent_enemy.add_component(HealthComponent::new(100.0, 0.0));

        // Ghost of remote enemy at (52, 20, 20)
        sim1.ghost_store.insert_or_update(GhostRecord {
            guid: remote_enemy,
            position: (52.0, 20.0, 20.0),
            velocity: (0.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            component_data: Vec::new(),
            last_tick: 1,
            source_node_id: 2,
        });
    }

    // Setup Remote Enemy on Node 2 at (52, 20, 20)
    {
        let mut sim2 = node2_sim.write().await;
        sim2.spawn_entity(remote_enemy, (52.0, 20.0, 20.0));
        let ent = sim2.entity_store.get_mut_by_guid(remote_enemy).unwrap();
        ent.add_component(HealthComponent::new(100.0, 0.0));
    }

    let mut dispatcher = CrossNodeDispatcher::new(1);
    dispatcher.add_peer(2, format!("http://{}", node2_addr));

    let whirlwind = AbilityDefinition {
        id: 12,
        name: "Whirlwind".into(),
        cast_time_sec: 0.0,
        cooldown_sec: 4.0,
        gcd_sec: 1.5,
        range: 10.0,
        resource_cost: Some(("energy".into(), 30.0)),
        interrupt_flags: ActionInterruptFlags::default(),
    };

    let outcome = {
        let mut sim1 = node1_sim.write().await;
        cast_boundary_sphere_aoe(
            &mut sim1,
            &dispatcher,
            caster,
            &whirlwind,
            Vec3::new(48.0, 20.0, 20.0),
            10.0,
            5,
            35,
            0,
        )
        .expect("boundary aoe executed")
    };

    assert_eq!(outcome.local_hit_count, 1);
    assert_eq!(outcome.remote_dispatched_nodes, vec![2]);

    // Verify local enemy took damage immediately
    {
        let sim1 = node1_sim.read().await;
        let ent = sim1.entity_store.get_by_guid(local_enemy).unwrap();
        let hp = ent.get_component::<HealthComponent>().unwrap().current_hp;
        assert_eq!(hp, 65.0);
    }

    // Wait for remote gRPC batch to commit on Node 2
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Verify remote enemy took damage on Node 2
    {
        let sim2 = node2_sim.read().await;
        let ent = sim2.entity_store.get_by_guid(remote_enemy).unwrap();
        let hp = ent.get_component::<HealthComponent>().unwrap().current_hp;
        assert_eq!(hp, 65.0);
    }
}

#[tokio::test]
async fn test_e2e_migration_lease_veto_and_compensation() {
    let (node1_sim, _node1_addr) =
        create_node(1, BoundingBox3D::new((0.0, 0.0, 0.0), (50.0, 100.0, 100.0))).await;
    let (node2_sim, node2_addr) = create_node(
        2,
        BoundingBox3D::new((50.0, 0.0, 0.0), (100.0, 100.0, 100.0)),
    )
    .await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    let caster = Guid::new(EntityType::Player, 1, 501);
    let target = Guid::new(EntityType::Player, 2, 601);

    // Setup Caster on Node 1
    {
        let mut sim1 = node1_sim.write().await;
        sim1.spawn_entity(caster, (45.0, 10.0, 10.0));
        let ent = sim1.entity_store.get_mut_by_guid(caster).unwrap();
        let mut attrs = AttributeSet::new();
        attrs.add_attribute("mana", 100.0, 0.0, 100.0, 0.0);
        ent.add_component(attrs);
        ent.add_component(CooldownTracker::new());

        sim1.ghost_store.insert_or_update(GhostRecord {
            guid: target,
            position: (55.0, 10.0, 10.0),
            velocity: (0.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            component_data: Vec::new(),
            last_tick: 1,
            source_node_id: 2,
        });
    }

    // Setup Target on Node 2 and freeze with migration lease
    {
        let mut sim2 = node2_sim.write().await;
        sim2.spawn_entity(target, (55.0, 10.0, 10.0));
        let ent = sim2.entity_store.get_mut_by_guid(target).unwrap();
        ent.add_component(HealthComponent::new(100.0, 0.0));
        assert!(sim2.acquire_migration_lease(target));
    }

    let mut dispatcher = CrossNodeDispatcher::new(1);
    dispatcher.add_peer(2, format!("http://{}", node2_addr));

    let spell = AbilityDefinition {
        id: 13,
        name: "Arcane Blast".into(),
        cast_time_sec: 0.0,
        cooldown_sec: 6.0,
        gcd_sec: 1.5,
        range: 20.0,
        resource_cost: Some(("mana".into(), 40.0)),
        interrupt_flags: ActionInterruptFlags::default(),
    };

    // Cast spell
    {
        let mut sim1 = node1_sim.write().await;
        let _ = cast_cross_partition_ability(&mut sim1, &dispatcher, caster, &spell, target, 50, 0);
    }

    // Proactive deduction occurred
    {
        let sim1 = node1_sim.read().await;
        let ent = sim1.entity_store.get_by_guid(caster).unwrap();
        let mana = ent
            .get_component::<AttributeSet>()
            .unwrap()
            .get_current("mana")
            .unwrap();
        assert_eq!(mana, 60.0);
    }

    // Wait for RPC rejection to arrive and push compensation
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Run simulation tick on Node 1 to drain compensation
    {
        let mut sim1 = node1_sim.write().await;
        sim1.step(0.05);

        let ent = sim1.entity_store.get_by_guid(caster).unwrap();
        let mana = ent
            .get_component::<AttributeSet>()
            .unwrap()
            .get_current("mana")
            .unwrap();
        assert_eq!(mana, 100.0); // Full refund!

        let cd = ent.get_component::<CooldownTracker>().unwrap();
        assert!(!cd.is_on_cooldown(13)); // Cooldown reset!
    }

    // Target on Node 2 remained unscathed at 100 HP
    {
        let sim2 = node2_sim.read().await;
        let ent = sim2.entity_store.get_by_guid(target).unwrap();
        let hp = ent.get_component::<HealthComponent>().unwrap().current_hp;
        assert_eq!(hp, 100.0);
    }
}

#[tokio::test]
async fn test_e2e_out_of_range_revalidation_veto_and_compensation() {
    let (node1_sim, _node1_addr) =
        create_node(1, BoundingBox3D::new((0.0, 0.0, 0.0), (50.0, 100.0, 100.0))).await;
    let (node2_sim, node2_addr) = create_node(
        2,
        BoundingBox3D::new((50.0, 0.0, 0.0), (100.0, 100.0, 100.0)),
    )
    .await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    let caster = Guid::new(EntityType::Player, 1, 701);
    let target = Guid::new(EntityType::Player, 2, 801);

    // Caster at (45, 10, 10) on Node 1
    {
        let mut sim1 = node1_sim.write().await;
        sim1.spawn_entity(caster, (45.0, 10.0, 10.0));
        let ent = sim1.entity_store.get_mut_by_guid(caster).unwrap();
        let mut attrs = AttributeSet::new();
        attrs.add_attribute("mana", 100.0, 0.0, 100.0, 0.0);
        ent.add_component(attrs);
        ent.add_component(CooldownTracker::new());

        // Stale ghost record on Node 1 believes target is at (50, 10, 10) (dist 5.0 <= 10.0)
        sim1.ghost_store.insert_or_update(GhostRecord {
            guid: target,
            position: (50.0, 10.0, 10.0),
            velocity: (0.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            component_data: Vec::new(),
            last_tick: 1,
            source_node_id: 2,
        });
    }

    // But Target on Node 2 has actually moved far away to (75, 10, 10) (actual dist 30.0 > 10.0 * 1.2 = 12.0)
    {
        let mut sim2 = node2_sim.write().await;
        sim2.spawn_entity(target, (75.0, 10.0, 10.0));
        let ent = sim2.entity_store.get_mut_by_guid(target).unwrap();
        ent.add_component(HealthComponent::new(100.0, 0.0));
    }

    let mut dispatcher = CrossNodeDispatcher::new(1);
    dispatcher.add_peer(2, format!("http://{}", node2_addr));

    let melee_strike = AbilityDefinition {
        id: 14,
        name: "Melee Strike".into(),
        cast_time_sec: 0.0,
        cooldown_sec: 3.0,
        gcd_sec: 1.5,
        range: 10.0,
        resource_cost: Some(("mana".into(), 20.0)),
        interrupt_flags: ActionInterruptFlags::default(),
    };

    // Cast spell
    {
        let mut sim1 = node1_sim.write().await;
        let _ = cast_cross_partition_ability(
            &mut sim1,
            &dispatcher,
            caster,
            &melee_strike,
            target,
            40,
            0,
        );
    }

    // Wait for target node to reject due to out-of-range authoritative re-validation
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Node 1 drains compensation
    {
        let mut sim1 = node1_sim.write().await;
        sim1.step(0.05);

        let ent = sim1.entity_store.get_by_guid(caster).unwrap();
        let mana = ent
            .get_component::<AttributeSet>()
            .unwrap()
            .get_current("mana")
            .unwrap();
        assert_eq!(mana, 100.0); // Refunded!

        let cd = ent.get_component::<CooldownTracker>().unwrap();
        assert!(!cd.is_on_cooldown(14)); // Cooldown reset!
    }

    // Target on Node 2 remained unscathed
    {
        let sim2 = node2_sim.read().await;
        let ent = sim2.entity_store.get_by_guid(target).unwrap();
        let hp = ent.get_component::<HealthComponent>().unwrap().current_hp;
        assert_eq!(hp, 100.0);
    }
}
