//! Authoritative Host API bindings for sandboxed WASM scripts.

use mmo_core::{EntityStore, Guid, ScriptError};
use wasmtime::{Caller, Linker, Memory};

use crate::combat::HealthComponent;

/// Host execution context passed into guest WASM invocations.
pub struct HostContext {
    pub(crate) entities_ptr: *mut EntityStore,
    pub(crate) octree_ptr: *mut mmo_core::Octree,
    pub(crate) events_ptr: *mut mmo_core::EventQueue,
    pub logs: Vec<(i32, String)>,
    pub dispatched_events: Vec<(i32, u64, Vec<u8>)>,
}

impl HostContext {
    /// Creates a host context without entity store access.
    pub fn empty() -> Self {
        Self {
            entities_ptr: std::ptr::null_mut(),
            octree_ptr: std::ptr::null_mut(),
            events_ptr: std::ptr::null_mut(),
            logs: Vec::new(),
            dispatched_events: Vec::new(),
        }
    }

    /// Creates a host context with optional mutable entity store access.
    pub fn new(entities: Option<&mut EntityStore>) -> Self {
        Self {
            entities_ptr: entities.map_or(std::ptr::null_mut(), |e| e as *mut EntityStore),
            octree_ptr: std::ptr::null_mut(),
            events_ptr: std::ptr::null_mut(),
            logs: Vec::new(),
            dispatched_events: Vec::new(),
        }
    }

    /// Creates a host context bound to an active `EntityStore`.
    pub fn with_entities(entities: &mut EntityStore) -> Self {
        Self {
            entities_ptr: entities as *mut EntityStore,
            octree_ptr: std::ptr::null_mut(),
            events_ptr: std::ptr::null_mut(),
            logs: Vec::new(),
            dispatched_events: Vec::new(),
        }
    }

    /// Creates a host context bound to an active tick context (entities, octree, events).
    pub(crate) fn with_tick_context(
        entities: &mut EntityStore,
        octree: &mut mmo_core::Octree,
        events: &mut mmo_core::EventQueue,
    ) -> Self {
        Self {
            entities_ptr: entities as *mut EntityStore,
            octree_ptr: octree as *mut mmo_core::Octree,
            events_ptr: events as *mut mmo_core::EventQueue,
            logs: Vec::new(),
            dispatched_events: Vec::new(),
        }
    }
}

unsafe impl Send for HostContext {}

/// Validates memory bounds using 64-bit unsigned arithmetic to prevent integer wrap-around attacks.
pub fn validate_memory_bounds(
    caller: &mut Caller<'_, HostContext>,
    ptr: i32,
    len: i32,
) -> Option<(Memory, usize, usize)> {
    if ptr < 0 || len < 0 {
        return None;
    }
    let memory = caller
        .get_export("memory")
        .and_then(|export| export.into_memory())?;

    let ptr_u64 = ptr as u64;
    let len_u64 = len as u64;
    let mem_size = memory.data_size(caller) as u64;

    if ptr_u64
        .checked_add(len_u64)
        .is_some_and(|end| end <= mem_size)
    {
        Some((memory, ptr as usize, len as usize))
    } else {
        None
    }
}

/// Registers authoritative Host API functions into the provided Linker under the `"env"` module.
pub fn register_host_api(linker: &mut Linker<HostContext>) -> Result<(), ScriptError> {
    // entity_get_pos: (guid: i64, out_x_ptr: i32, out_y_ptr: i32, out_z_ptr: i32) -> i32
    linker
        .func_wrap(
            "env",
            "entity_get_pos",
            |mut caller: Caller<'_, HostContext>,
             guid: i64,
             out_x_ptr: i32,
             out_y_ptr: i32,
             out_z_ptr: i32|
             -> i32 {
                let entities_ptr = caller.data().entities_ptr;
                if entities_ptr.is_null() {
                    return -1;
                }
                let target_guid = Guid::from_raw(guid as u64);
                let entities = unsafe { &*entities_ptr };
                if let Some(entity) = entities.get_by_guid(target_guid) {
                    let (x, y, z) = entity.position;
                    let (memory, xp, _) = match validate_memory_bounds(&mut caller, out_x_ptr, 4) {
                        Some(v) => v,
                        None => return -1,
                    };
                    if memory.write(&mut caller, xp, &x.to_le_bytes()).is_err() {
                        return -1;
                    }
                    let (memory, yp, _) = match validate_memory_bounds(&mut caller, out_y_ptr, 4) {
                        Some(v) => v,
                        None => return -1,
                    };
                    if memory.write(&mut caller, yp, &y.to_le_bytes()).is_err() {
                        return -1;
                    }
                    let (memory, zp, _) = match validate_memory_bounds(&mut caller, out_z_ptr, 4) {
                        Some(v) => v,
                        None => return -1,
                    };
                    if memory.write(&mut caller, zp, &z.to_le_bytes()).is_err() {
                        return -1;
                    }
                    0
                } else {
                    -1
                }
            },
        )
        .map_err(|e| ScriptError::ExecutionError(e.to_string()))?;

    // entity_set_pos: (guid: i64, x: f32, y: f32, z: f32) -> i32
    linker
        .func_wrap(
            "env",
            "entity_set_pos",
            |caller: Caller<'_, HostContext>, guid: i64, x: f32, y: f32, z: f32| -> i32 {
                if !x.is_finite() || !y.is_finite() || !z.is_finite() {
                    return -1;
                }
                let entities_ptr = caller.data().entities_ptr;
                if entities_ptr.is_null() {
                    return -1;
                }
                let target_guid = Guid::from_raw(guid as u64);
                let entities = unsafe { &mut *entities_ptr };
                if let Some(entity) = entities.get_mut_by_guid(target_guid) {
                    let old_pos = entity.position;
                    entity.position = (x, y, z);

                    let octree_ptr = caller.data().octree_ptr;
                    if !octree_ptr.is_null() {
                        let octree = unsafe { &mut *octree_ptr };
                        if !octree.remove(target_guid, old_pos) {
                            octree.remove_by_guid(target_guid);
                        }
                        octree.insert(target_guid, (x, y, z));
                    }
                    0
                } else {
                    -1
                }
            },
        )
        .map_err(|e| ScriptError::ExecutionError(e.to_string()))?;

    // entity_distance_sq: (guid_a: i64, guid_b: i64) -> f32
    linker
        .func_wrap(
            "env",
            "entity_distance_sq",
            |caller: Caller<'_, HostContext>, guid_a: i64, guid_b: i64| -> f32 {
                let entities_ptr = caller.data().entities_ptr;
                if entities_ptr.is_null() {
                    return -1.0;
                }
                let ga = Guid::from_raw(guid_a as u64);
                let gb = Guid::from_raw(guid_b as u64);
                let entities = unsafe { &*entities_ptr };
                if let (Some(ea), Some(eb)) = (entities.get_by_guid(ga), entities.get_by_guid(gb)) {
                    let dx = ea.position.0 - eb.position.0;
                    let dy = ea.position.1 - eb.position.1;
                    let dz = ea.position.2 - eb.position.2;
                    dx * dx + dy * dy + dz * dz
                } else {
                    -1.0
                }
            },
        )
        .map_err(|e| ScriptError::ExecutionError(e.to_string()))?;

    // entity_modify_health: (guid: i64, delta: i32) -> i32
    linker
        .func_wrap(
            "env",
            "entity_modify_health",
            |caller: Caller<'_, HostContext>, guid: i64, delta: i32| -> i32 {
                let entities_ptr = caller.data().entities_ptr;
                if entities_ptr.is_null() {
                    return -1;
                }
                let target_guid = Guid::from_raw(guid as u64);
                let entities = unsafe { &mut *entities_ptr };
                if let Some(entity) = entities.get_mut_by_guid(target_guid) {
                    if let Some(health) = entity.components.get_component_mut::<HealthComponent>() {
                        let died = if delta < 0 {
                            health.take_damage(-(delta as i64) as f32)
                        } else {
                            health.heal(delta as f32);
                            false
                        };
                        let new_hp = health.current_hp;

                        if let Some(attrs) = entity
                            .components
                            .get_component_mut::<mmo_core::AttributeSet>()
                        {
                            let _ = attrs.set_current("health", new_hp);
                        }

                        if died {
                            let events_ptr = caller.data().events_ptr;
                            if !events_ptr.is_null() {
                                let events = unsafe { &mut *events_ptr };
                                events.push(crate::combat::EntityDiedEvent {
                                    entity: target_guid,
                                    killer: None,
                                });
                            }
                        }

                        new_hp as i32
                    } else {
                        -1
                    }
                } else {
                    -1
                }
            },
        )
        .map_err(|e| ScriptError::ExecutionError(e.to_string()))?;

    // dispatch_event: (event_type: i32, target_guid: i64, payload_ptr: i32, payload_len: i32) -> i32
    linker
        .func_wrap(
            "env",
            "dispatch_event",
            |mut caller: Caller<'_, HostContext>,
             event_type: i32,
             target_guid: i64,
             payload_ptr: i32,
             payload_len: i32|
             -> i32 {
                let payload = if payload_len > 0 {
                    let (memory, p, l) =
                        match validate_memory_bounds(&mut caller, payload_ptr, payload_len) {
                            Some(v) => v,
                            None => return -1,
                        };
                    let mut buf = vec![0u8; l];
                    if memory.read(&caller, p, &mut buf).is_err() {
                        return -1;
                    }
                    buf
                } else {
                    Vec::new()
                };

                caller
                    .data_mut()
                    .dispatched_events
                    .push((event_type, target_guid as u64, payload));
                0
            },
        )
        .map_err(|e| ScriptError::ExecutionError(e.to_string()))?;

    // script_log: (level: i32, msg_ptr: i32, msg_len: i32) -> i32
    linker
        .func_wrap(
            "env",
            "script_log",
            |mut caller: Caller<'_, HostContext>, level: i32, msg_ptr: i32, msg_len: i32| -> i32 {
                if msg_len <= 0 {
                    return -1;
                }
                let (memory, p, l) = match validate_memory_bounds(&mut caller, msg_ptr, msg_len) {
                    Some(v) => v,
                    None => return -1,
                };
                let mut buf = vec![0u8; l];
                if memory.read(&caller, p, &mut buf).is_err() {
                    return -1;
                }
                let msg = String::from_utf8_lossy(&buf).to_string();
                caller.data_mut().logs.push((level, msg));
                0
            },
        )
        .map_err(|e| ScriptError::ExecutionError(e.to_string()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::guid::EntityType;
    use wasmtime::{Engine, Module, Store};

    #[test]
    fn test_validate_memory_bounds_overflow_rejection() {
        let engine = Engine::default();
        let mut linker = Linker::new(&engine);
        linker
            .func_wrap(
                "env",
                "test_bounds",
                |mut caller: Caller<'_, HostContext>, ptr: i32, len: i32| -> i32 {
                    if validate_memory_bounds(&mut caller, ptr, len).is_some() {
                        1
                    } else {
                        0
                    }
                },
            )
            .unwrap();

        let mut store = Store::new(&engine, HostContext::empty());
        let wat = r#"
        (module
            (import "env" "test_bounds" (func $test_bounds (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "check") (param i32 i32) (result i32)
                (call $test_bounds (local.get 0) (local.get 1))
            )
        )
        "#;
        let module = Module::new(&engine, wat).unwrap();
        let instance = linker.instantiate(&mut store, &module).unwrap();
        let check = instance
            .get_typed_func::<(i32, i32), i32>(&mut store, "check")
            .unwrap();

        // Valid inside bounds
        assert_eq!(check.call(&mut store, (0, 100)).unwrap(), 1);

        // Integer overflow attack (0x7FFFFFFF + 10)
        assert_eq!(check.call(&mut store, (i32::MAX, 10)).unwrap(), 0);

        // Negative pointer or length
        assert_eq!(check.call(&mut store, (-1, 10)).unwrap(), 0);
        assert_eq!(check.call(&mut store, (10, -5)).unwrap(), 0);

        // Past memory size (65536)
        assert_eq!(check.call(&mut store, (65530, 20)).unwrap(), 0);
    }

    #[test]
    fn test_host_api_logging_and_events() {
        let engine = Engine::default();
        let mut linker = Linker::new(&engine);
        register_host_api(&mut linker).unwrap();

        let mut store = Store::new(&engine, HostContext::empty());
        let wat = r#"
        (module
            (import "env" "script_log" (func $log (param i32 i32 i32) (result i32)))
            (import "env" "dispatch_event" (func $dispatch (param i32 i64 i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "Hello from WASM")
            (func (export "run")
                (call $log (i32.const 1) (i32.const 0) (i32.const 15))
                drop
                (call $dispatch (i32.const 42) (i64.const 100) (i32.const 0) (i32.const 5))
                drop
            )
        )
        "#;
        let module = Module::new(&engine, wat).unwrap();
        let instance = linker.instantiate(&mut store, &module).unwrap();
        let run = instance
            .get_typed_func::<(), ()>(&mut store, "run")
            .unwrap();
        run.call(&mut store, ()).unwrap();

        let logs = &store.data().logs;
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].0, 1);
        assert_eq!(logs[0].1, "Hello from WASM");

        let events = &store.data().dispatched_events;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, 42);
        assert_eq!(events[0].1, 100);
        assert_eq!(&events[0].2, b"Hello");
    }

    #[test]
    fn test_host_api_entity_position_and_health() {
        let engine = Engine::default();
        let mut linker = Linker::new(&engine);
        register_host_api(&mut linker).unwrap();

        let mut entities = EntityStore::new();
        let guid = Guid::new(EntityType::Player, 1, 1);
        let mut entity = mmo_core::Entity::new(guid, (10.0, 20.0, 30.0));
        entity
            .components
            .add_component(HealthComponent::new(100.0, 1.0));
        entities.insert_entity(entity);

        let mut store = Store::new(&engine, HostContext::with_entities(&mut entities));
        let wat = r#"
        (module
            (import "env" "entity_get_pos" (func $get_pos (param i64 i32 i32 i32) (result i32)))
            (import "env" "entity_set_pos" (func $set_pos (param i64 f32 f32 f32) (result i32)))
            (import "env" "entity_modify_health" (func $mod_hp (param i64 i32) (result i32)))
            (memory (export "memory") 1)
            (func (export "test_pos") (param i64) (result i32)
                (call $get_pos (local.get 0) (i32.const 0) (i32.const 4) (i32.const 8))
                drop
                (call $set_pos (local.get 0) (f32.const 15.0) (f32.const 25.0) (f32.const 35.0))
            )
            (func (export "test_damage") (param i64 i32) (result i32)
                (call $mod_hp (local.get 0) (local.get 1))
            )
        )
        "#;
        let module = Module::new(&engine, wat).unwrap();
        let instance = linker.instantiate(&mut store, &module).unwrap();

        let test_pos = instance
            .get_typed_func::<i64, i32>(&mut store, "test_pos")
            .unwrap();
        let res = test_pos.call(&mut store, guid.raw() as i64).unwrap();
        assert_eq!(res, 0);

        // Read updated position directly from entity
        let updated = entities.get_by_guid(guid).unwrap();
        assert_eq!(updated.position, (15.0, 25.0, 35.0));

        // Damage entity by 25 hp
        let test_damage = instance
            .get_typed_func::<(i64, i32), i32>(&mut store, "test_damage")
            .unwrap();
        let new_hp = test_damage
            .call(&mut store, (guid.raw() as i64, -25))
            .unwrap();
        assert_eq!(new_hp, 75);

        let updated_hp = entities
            .get_by_guid(guid)
            .unwrap()
            .components
            .get_component::<HealthComponent>()
            .unwrap();
        assert_eq!(updated_hp.current_hp, 75.0);
    }
}
