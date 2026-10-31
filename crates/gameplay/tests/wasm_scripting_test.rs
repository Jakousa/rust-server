//! Comprehensive integration and stress tests for the WASM Scripting Sandbox.

use std::sync::Arc;

use mmo_core::guid::EntityType;
use mmo_core::{
    AttributeSet, BoundingBox3D, ComponentCodecRegistry, Entity, EntityStore, Event, EventQueue,
    Guid, HookDispatcher, ModularPacketRouter, Octree, PluginManager, ScriptError, ScriptValue,
};
use mmo_gameplay::{
    combat::EntityDiedEvent, HealthComponent, ScriptHotReloader, WasmRuntime, WasmScriptEngine,
    WasmScriptPlugin,
};

#[test]
fn test_fuel_exhaustion_stress_termination() {
    let engine = Arc::new(WasmScriptEngine::new().expect("Failed to create engine"));
    let runtime = Arc::new(WasmRuntime::new(engine.clone()).expect("Failed to create runtime"));

    // Normal calculation module
    let normal_wat = r#"
    (module
        (func (export "compute") (param i32 i32) (result i32)
            local.get 0
            local.get 1
            i32.add
        )
    )
    "#;
    let normal_bytes = wat::parse_str(normal_wat).unwrap();
    engine.load_module("normal", &normal_bytes).unwrap();

    // Runaway infinite loop module
    let loop_wat = r#"
    (module
        (func (export "runaway") (result i32)
            (loop (br 0))
            i32.const 1
        )
    )
    "#;
    let loop_bytes = wat::parse_str(loop_wat).unwrap();
    engine.load_module("runaway", &loop_bytes).unwrap();

    // Normal call succeeds
    let (res, consumed) = runtime
        .execute_function(
            "normal",
            "compute",
            &[ScriptValue::I32(10), ScriptValue::I32(20)],
            None,
            10_000,
        )
        .unwrap();
    assert_eq!(res, vec![ScriptValue::I32(30)]);
    assert!(consumed > 0);

    // Stress test: Invoke runaway loop 50 times in rapid succession
    for _ in 0..50 {
        let err = runtime.execute_function("runaway", "runaway", &[], None, 2_000);
        assert!(matches!(err, Err(ScriptError::OutOfFuel)));
    }
}

#[test]
fn test_hot_reloading_during_ability_execution() {
    let engine = Arc::new(WasmScriptEngine::new().unwrap());
    let runtime = Arc::new(WasmRuntime::new(engine.clone()).unwrap());
    let reloader = ScriptHotReloader::new(runtime.clone());

    let mut entities = EntityStore::new();
    let caster = Guid::new(EntityType::Player, 1, 10);
    let target = Guid::new(EntityType::Npc, 1, 20);

    let mut target_ent = Entity::new(target, (0.0, 0.0, 0.0));
    target_ent
        .components
        .add_component(HealthComponent::new(100.0, 1.0));
    entities.insert_entity(target_ent);

    // Ability V1: deals 10 damage via Host API entity_modify_health
    let v1_wat = r#"
    (module
        (import "env" "entity_modify_health" (func $mod_hp (param i64 i32) (result i32)))
        (func (export "on_cast") (param i64 i64 i32) (result i32)
            (call $mod_hp (local.get 1) (i32.const -10))
        )
    )
    "#;
    let v1_bytes = wat::parse_str(v1_wat).unwrap();
    reloader.reload_module("fireball", &v1_bytes).unwrap();

    let hp_after_v1 = runtime
        .call_ability_cast(
            "fireball",
            "on_cast",
            caster,
            target,
            101,
            &mut entities,
            50_000,
        )
        .unwrap();
    assert_eq!(hp_after_v1, 90);

    let ent = entities.get_by_guid(target).unwrap();
    let hp_comp = ent.components.get_component::<HealthComponent>().unwrap();
    assert_eq!(hp_comp.current_hp, 90.0);

    // Hot-reload Ability V2: upgraded to deal 35 damage
    let v2_wat = r#"
    (module
        (import "env" "entity_modify_health" (func $mod_hp (param i64 i32) (result i32)))
        (func (export "on_cast") (param i64 i64 i32) (result i32)
            (call $mod_hp (local.get 1) (i32.const -35))
        )
    )
    "#;
    let v2_bytes = wat::parse_str(v2_wat).unwrap();
    reloader.reload_module("fireball", &v2_bytes).unwrap();

    let hp_after_v2 = runtime
        .call_ability_cast(
            "fireball",
            "on_cast",
            caster,
            target,
            101,
            &mut entities,
            50_000,
        )
        .unwrap();
    assert_eq!(hp_after_v2, 55);

    let ent2 = entities.get_by_guid(target).unwrap();
    let hp_comp2 = ent2.components.get_component::<HealthComponent>().unwrap();
    assert_eq!(hp_comp2.current_hp, 55.0);
}

#[test]
fn test_host_api_memory_bounds_violation_safety() {
    let engine = Arc::new(WasmScriptEngine::new().unwrap());
    let runtime = Arc::new(WasmRuntime::new(engine.clone()).unwrap());

    // Script maliciously passes invalid pointers:
    // 1. Negative pointer -1
    // 2. Integer overflow pointer 0x7FFFFFFF (i32::MAX)
    // 3. Pointer beyond linear memory 100,000
    let exploit_wat = r#"
    (module
        (import "env" "entity_get_pos" (func $get_pos (param i64 i32 i32 i32) (result i32)))
        (import "env" "script_log" (func $log (param i32 i32 i32) (result i32)))
        (memory (export "memory") 1)
        (func (export "test_negative") (result i32)
            (call $get_pos (i64.const 1) (i32.const -1) (i32.const 0) (i32.const 0))
        )
        (func (export "test_overflow") (result i32)
            (call $get_pos (i64.const 1) (i32.const 2147483647) (i32.const 0) (i32.const 0))
        )
        (func (export "test_out_of_bounds") (result i32)
            (call $get_pos (i64.const 1) (i32.const 100000) (i32.const 0) (i32.const 0))
        )
        (func (export "test_log_bounds") (result i32)
            (call $log (i32.const 1) (i32.const 2147483647) (i32.const 10))
        )
    )
    "#;
    let exploit_bytes = wat::parse_str(exploit_wat).unwrap();
    engine.load_module("exploit", &exploit_bytes).unwrap();

    let mut entities = EntityStore::new();
    let p = Guid::new(EntityType::Player, 1, 1);
    entities.insert_entity(Entity::new(p, (10.0, 10.0, 10.0)));

    let (res_neg, _) = runtime
        .execute_function("exploit", "test_negative", &[], Some(&mut entities), 10_000)
        .unwrap();
    assert_eq!(res_neg, vec![ScriptValue::I32(-1)]);

    let (res_over, _) = runtime
        .execute_function("exploit", "test_overflow", &[], Some(&mut entities), 10_000)
        .unwrap();
    assert_eq!(res_over, vec![ScriptValue::I32(-1)]);

    let (res_oob, _) = runtime
        .execute_function(
            "exploit",
            "test_out_of_bounds",
            &[],
            Some(&mut entities),
            10_000,
        )
        .unwrap();
    assert_eq!(res_oob, vec![ScriptValue::I32(-1)]);

    let (res_log, _) = runtime
        .execute_function(
            "exploit",
            "test_log_bounds",
            &[],
            Some(&mut entities),
            10_000,
        )
        .unwrap();
    assert_eq!(res_log, vec![ScriptValue::I32(-1)]);
}

#[test]
fn test_plugin_veto_and_fail_closed_pipeline() {
    let engine = Arc::new(WasmScriptEngine::new().unwrap());
    let runtime = Arc::new(WasmRuntime::new(engine.clone()).unwrap());
    let reloader = ScriptHotReloader::new(runtime.clone());

    let mut hooks = HookDispatcher::new();
    let mut codecs = ComponentCodecRegistry::new();
    let mut event_queue = EventQueue::new();
    let mut packets = ModularPacketRouter::new();
    let mut tick_systems = Vec::new();
    let mut combat_resolver = None;

    let mut plugin_mgr = PluginManager::new();
    plugin_mgr.add_plugin(Box::new(WasmScriptPlugin::new(runtime)));
    plugin_mgr.build_all(
        &mut hooks,
        &mut codecs,
        &mut event_queue,
        &mut packets,
        &mut tick_systems,
        &mut combat_resolver,
    );

    let mut entities = EntityStore::new();
    let p1 = Guid::new(EntityType::Player, 1, 1);
    let p2 = Guid::new(EntityType::Npc, 1, 2);

    // 1. Script allows (returns 0)
    let allow_wat = r#"
    (module
        (func (export "on_before_interact") (param i64 i64 i32) (result i32)
            i32.const 0
        )
    )
    "#;
    reloader
        .reload_module("interact", &wat::parse_str(allow_wat).unwrap())
        .unwrap();

    let ctx1 = hooks.dispatch_with_entities(
        &Event::BeforeInteract {
            initiator: p1,
            target: p2,
            action_id: 1,
        },
        &mut entities,
    );
    assert!(!ctx1.is_canceled());

    // 2. Script vetoes (returns 1)
    let veto_wat = r#"
    (module
        (func (export "on_before_interact") (param i64 i64 i32) (result i32)
            i32.const 1
        )
    )
    "#;
    reloader
        .reload_module("interact", &wat::parse_str(veto_wat).unwrap())
        .unwrap();

    let ctx2 = hooks.dispatch_with_entities(
        &Event::BeforeInteract {
            initiator: p1,
            target: p2,
            action_id: 1,
        },
        &mut entities,
    );
    assert!(ctx2.is_canceled());
    assert_eq!(ctx2.cancel_reason(), Some("script_veto"));

    // 3. Script traps (div 0) -> Fails closed
    let trap_wat = r#"
    (module
        (func (export "on_before_interact") (param i64 i64 i32) (result i32)
            i32.const 1
            i32.const 0
            i32.div_s
        )
    )
    "#;
    reloader
        .reload_module("interact", &wat::parse_str(trap_wat).unwrap())
        .unwrap();

    let ctx3 = hooks.dispatch_with_entities(
        &Event::BeforeInteract {
            initiator: p1,
            target: p2,
            action_id: 1,
        },
        &mut entities,
    );
    assert!(ctx3.is_canceled());
    assert!(ctx3.cancel_reason().unwrap().starts_with("script_error:"));

    // 4. Script out of fuel -> Fails closed
    let fuel_wat = r#"
    (module
        (func (export "on_before_interact") (param i64 i64 i32) (result i32)
            (loop (br 0))
            i32.const 0
        )
    )
    "#;
    reloader
        .reload_module("interact", &wat::parse_str(fuel_wat).unwrap())
        .unwrap();

    let ctx4 = hooks.dispatch_with_entities(
        &Event::BeforeInteract {
            initiator: p1,
            target: p2,
            action_id: 1,
        },
        &mut entities,
    );
    assert!(ctx4.is_canceled());
    assert_eq!(ctx4.cancel_reason(), Some("script_error: out of fuel"));
}

#[test]
fn test_wasm_entity_set_pos_updates_octree_and_rejects_nan() {
    let engine = Arc::new(WasmScriptEngine::new().unwrap());
    let runtime = Arc::new(WasmRuntime::new(engine.clone()).unwrap());

    let mut entities = EntityStore::new();
    let guid = Guid::new(EntityType::Player, 1, 999);
    let key = entities.spawn(guid, (10.0, 10.0, 10.0));

    let mut octree = Octree::new(BoundingBox3D::new(
        (-100.0, -100.0, -100.0),
        (100.0, 100.0, 100.0),
    ));
    octree.insert(guid, (10.0, 10.0, 10.0));

    let mut events = EventQueue::new();

    let wat = r#"
    (module
        (import "env" "entity_set_pos" (func $set_pos (param i64 f32 f32 f32) (result i32)))
        (func (export "teleport") (param i64 f32 f32 f32) (result i32)
            (call $set_pos (local.get 0) (local.get 1) (local.get 2) (local.get 3))
        )
    )
    "#;
    engine
        .load_module("movement", &wat::parse_str(wat).unwrap())
        .unwrap();

    // 1. NaN coordinate rejection
    let (res_nan, _) = runtime
        .execute_function_with_tick_context(
            "movement",
            "teleport",
            &[
                ScriptValue::I64(guid.raw() as i64),
                ScriptValue::F32(f32::NAN),
                ScriptValue::F32(20.0),
                ScriptValue::F32(30.0),
            ],
            &mut entities,
            &mut octree,
            &mut events,
            50_000,
        )
        .unwrap();
    assert_eq!(res_nan, vec![ScriptValue::I32(-1)]);
    assert_eq!(entities.get(key).unwrap().position, (10.0, 10.0, 10.0));

    // 2. Valid move updates entity position and synchronizes Octree
    let (res_ok, _) = runtime
        .execute_function_with_tick_context(
            "movement",
            "teleport",
            &[
                ScriptValue::I64(guid.raw() as i64),
                ScriptValue::F32(50.0),
                ScriptValue::F32(50.0),
                ScriptValue::F32(50.0),
            ],
            &mut entities,
            &mut octree,
            &mut events,
            50_000,
        )
        .unwrap();
    assert_eq!(res_ok, vec![ScriptValue::I32(0)]);
    assert_eq!(entities.get(key).unwrap().position, (50.0, 50.0, 50.0));

    // Old position in octree is empty, new position contains guid
    assert!(octree.query_radius((10.0, 10.0, 10.0), 1.0).is_empty());
    let found = octree.query_radius((50.0, 50.0, 50.0), 1.0);
    assert_eq!(found, vec![guid]);
}

#[test]
fn test_wasm_entity_modify_health_syncs_attributes_and_emits_death() {
    let engine = Arc::new(WasmScriptEngine::new().unwrap());
    let runtime = Arc::new(WasmRuntime::new(engine.clone()).unwrap());

    let mut entities = EntityStore::new();
    let guid = Guid::new(EntityType::Player, 1, 888);
    let key = entities.spawn(guid, (0.0, 0.0, 0.0));

    let mut attrs = AttributeSet::new();
    attrs.add_attribute("health", 100.0, 0.0, 100.0, 0.0);

    let ent = entities.get_mut(key).unwrap();
    ent.add_component(HealthComponent::new(100.0, 0.0));
    ent.add_component(attrs);

    let mut octree = Octree::new(BoundingBox3D::new(
        (-100.0, -100.0, -100.0),
        (100.0, 100.0, 100.0),
    ));
    let mut events = EventQueue::new();

    let wat = r#"
    (module
        (import "env" "entity_modify_health" (func $mod_hp (param i64 i32) (result i32)))
        (func (export "apply_damage") (param i64 i32) (result i32)
            (call $mod_hp (local.get 0) (local.get 1))
        )
    )
    "#;
    engine
        .load_module("combat_mod", &wat::parse_str(wat).unwrap())
        .unwrap();

    // 1. Partial damage
    let (res_dmg, _) = runtime
        .execute_function_with_tick_context(
            "combat_mod",
            "apply_damage",
            &[ScriptValue::I64(guid.raw() as i64), ScriptValue::I32(-40)],
            &mut entities,
            &mut octree,
            &mut events,
            50_000,
        )
        .unwrap();
    assert_eq!(res_dmg, vec![ScriptValue::I32(60)]);

    let ent = entities.get(key).unwrap();
    assert_eq!(
        ent.get_component::<HealthComponent>().unwrap().current_hp,
        60.0
    );
    assert_eq!(
        ent.get_component::<AttributeSet>()
            .unwrap()
            .get_current("health")
            .unwrap(),
        60.0
    );
    assert!(events.is_empty());

    // 2. Lethal damage -> health 0, AttributeSet 0, emits EntityDiedEvent
    let (res_lethal, _) = runtime
        .execute_function_with_tick_context(
            "combat_mod",
            "apply_damage",
            &[ScriptValue::I64(guid.raw() as i64), ScriptValue::I32(-100)],
            &mut entities,
            &mut octree,
            &mut events,
            50_000,
        )
        .unwrap();
    assert_eq!(res_lethal, vec![ScriptValue::I32(0)]);

    let ent = entities.get(key).unwrap();
    assert_eq!(
        ent.get_component::<HealthComponent>().unwrap().current_hp,
        0.0
    );
    assert_eq!(
        ent.get_component::<AttributeSet>()
            .unwrap()
            .get_current("health")
            .unwrap(),
        0.0
    );

    let death_events = events.drain::<EntityDiedEvent>();
    assert_eq!(death_events.len(), 1);
    assert_eq!(death_events[0].entity, guid);
    assert_eq!(death_events[0].killer, None);
}
