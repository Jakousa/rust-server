//! GameplayPlugin integration for sandboxed WebAssembly scripting.

use std::sync::Arc;

use mmo_core::{
    EntityStore, Event, EventContext, GameplayPlugin, HookHandler, PluginContext, ScriptError,
    ScriptValue, TickContext, TickSystem,
};

use super::runtime::WasmRuntime;

/// Standalone gameplay plugin registering WASM scripting hooks and tick systems.
pub struct WasmScriptPlugin {
    runtime: Arc<WasmRuntime>,
}

impl WasmScriptPlugin {
    /// Creates a new `WasmScriptPlugin` for the given runtime.
    pub fn new(runtime: Arc<WasmRuntime>) -> Self {
        WasmScriptPlugin { runtime }
    }

    /// Returns a reference to the active `WasmRuntime`.
    pub fn runtime(&self) -> &Arc<WasmRuntime> {
        &self.runtime
    }
}

impl GameplayPlugin for WasmScriptPlugin {
    fn id(&self) -> &'static str {
        "scripting"
    }

    fn build(&self, ctx: &mut PluginContext) {
        ctx.hooks.register_handler(Box::new(ScriptHookHandler {
            runtime: self.runtime.clone(),
        }));
        ctx.tick_systems.push(Box::new(ScriptTickSystem {
            runtime: self.runtime.clone(),
        }));
    }
}

/// Synchronous hook handler dispatching lifecycle events to registered WASM scripts.
pub struct ScriptHookHandler {
    runtime: Arc<WasmRuntime>,
}

impl ScriptHookHandler {
    fn dispatch_event_to_script(
        &self,
        event: &Event,
        ctx: &mut EventContext,
        entities: Option<&mut EntityStore>,
    ) {
        match event {
            Event::BeforeInteract {
                initiator,
                target,
                action_id,
            } if self.runtime.engine().has_module("interact") => {
                let args = [
                    ScriptValue::I64(initiator.raw() as i64),
                    ScriptValue::I64(target.raw() as i64),
                    ScriptValue::I32(*action_id as i32),
                ];
                match self.runtime.execute_function(
                    "interact",
                    "on_before_interact",
                    &args,
                    entities,
                    50_000,
                ) {
                    Ok((results, _)) => {
                        if let Some(code) = results.first().and_then(|v| v.as_i32()) {
                            if code != 0 {
                                ctx.cancel("script_veto");
                            }
                        }
                    }
                    // Fail-Closed: Trap or OutOfFuel automatically vetoes the action
                    Err(ScriptError::OutOfFuel) => {
                        ctx.cancel("script_error: out of fuel");
                    }
                    Err(ScriptError::Trap(e)) => {
                        ctx.cancel(format!("script_error: {e}"));
                    }
                    Err(e) => {
                        ctx.cancel(format!("script_error: {e}"));
                    }
                }
            }
            Event::BeforeMove { entity, from, to } if self.runtime.engine().has_module("move") => {
                let args = [
                    ScriptValue::I64(entity.raw() as i64),
                    ScriptValue::F32(from.0),
                    ScriptValue::F32(from.1),
                    ScriptValue::F32(from.2),
                    ScriptValue::F32(to.0),
                    ScriptValue::F32(to.1),
                    ScriptValue::F32(to.2),
                ];
                match self.runtime.execute_function(
                    "move",
                    "on_before_move",
                    &args,
                    entities,
                    50_000,
                ) {
                    Ok((results, _)) => {
                        if let Some(code) = results.first().and_then(|v| v.as_i32()) {
                            if code != 0 {
                                ctx.cancel("script_veto");
                            }
                        }
                    }
                    Err(ScriptError::OutOfFuel) => {
                        ctx.cancel("script_error: out of fuel");
                    }
                    Err(ScriptError::Trap(e)) => {
                        ctx.cancel(format!("script_error: {e}"));
                    }
                    Err(e) => {
                        ctx.cancel(format!("script_error: {e}"));
                    }
                }
            }
            _ => {}
        }
    }
}

impl HookHandler for ScriptHookHandler {
    fn handle(&self, event: &Event, ctx: &mut EventContext) {
        self.dispatch_event_to_script(event, ctx, None);
    }

    fn handle_with_entities(
        &self,
        event: &Event,
        ctx: &mut EventContext,
        entities: &mut EntityStore,
    ) {
        self.dispatch_event_to_script(event, ctx, Some(entities));
    }
}

/// Periodic batch system executing per-tick world scripts without stalling the simulation tick.
pub struct ScriptTickSystem {
    runtime: Arc<WasmRuntime>,
}

impl TickSystem for ScriptTickSystem {
    fn name(&self) -> &'static str {
        "script_tick_system"
    }

    fn update(&mut self, ctx: &mut TickContext) {
        if self.runtime.engine().has_module("world_tick") {
            let args = [
                ScriptValue::I64(ctx.tick as i64),
                ScriptValue::F32(ctx.dt_sec),
            ];
            if let Err(e) = self.runtime.execute_function_with_tick_context(
                "world_tick",
                "on_world_tick",
                &args,
                ctx.entities,
                ctx.octree,
                ctx.events,
                100_000,
            ) {
                // Log and swallow per-tick script failures to preserve tick deadline
                tracing::warn!("WASM world_tick script execution failure: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::engine::WasmScriptEngine;
    use mmo_core::guid::EntityType;
    use mmo_core::Guid;

    #[test]
    fn test_before_interact_success_and_veto() {
        let engine = Arc::new(WasmScriptEngine::new().unwrap());
        // Script allows interaction when action_id == 1, vetoes (returns 1) otherwise
        let wat = r#"
        (module
            (func (export "on_before_interact") (param i64 i64 i32) (result i32)
                local.get 2
                i32.const 1
                i32.eq
                if (result i32)
                    i32.const 0
                else
                    i32.const 1
                end
            )
        )
        "#;
        let bytes = wat::parse_str(wat).unwrap();
        engine.load_module("interact", &bytes).unwrap();

        let runtime = Arc::new(WasmRuntime::new(engine).unwrap());
        let handler = ScriptHookHandler { runtime };

        let mut entities = EntityStore::new();
        let e1 = Guid::new(EntityType::Player, 1, 10);
        let e2 = Guid::new(EntityType::Npc, 1, 20);

        // Allowed action_id = 1
        let mut ctx1 = EventContext::new();
        handler.handle_with_entities(
            &Event::BeforeInteract {
                initiator: e1,
                target: e2,
                action_id: 1,
            },
            &mut ctx1,
            &mut entities,
        );
        assert!(!ctx1.is_canceled());

        // Vetoed action_id = 2
        let mut ctx2 = EventContext::new();
        handler.handle_with_entities(
            &Event::BeforeInteract {
                initiator: e1,
                target: e2,
                action_id: 2,
            },
            &mut ctx2,
            &mut entities,
        );
        assert!(ctx2.is_canceled());
        assert_eq!(ctx2.cancel_reason(), Some("script_veto"));
    }

    #[test]
    fn test_before_interact_fails_closed_on_trap() {
        let engine = Arc::new(WasmScriptEngine::new().unwrap());
        // Script contains a division by zero trap
        let wat = r#"
        (module
            (func (export "on_before_interact") (param i64 i64 i32) (result i32)
                i32.const 1
                i32.const 0
                i32.div_s
            )
        )
        "#;
        let bytes = wat::parse_str(wat).unwrap();
        engine.load_module("interact", &bytes).unwrap();

        let runtime = Arc::new(WasmRuntime::new(engine).unwrap());
        let handler = ScriptHookHandler { runtime };

        let mut entities = EntityStore::new();
        let e1 = Guid::new(EntityType::Player, 1, 10);
        let e2 = Guid::new(EntityType::Npc, 1, 20);

        let mut ctx = EventContext::new();
        handler.handle_with_entities(
            &Event::BeforeInteract {
                initiator: e1,
                target: e2,
                action_id: 1,
            },
            &mut ctx,
            &mut entities,
        );

        // Verify Fail-Closed: Action MUST be canceled
        assert!(ctx.is_canceled());
        assert!(ctx.cancel_reason().unwrap().starts_with("script_error:"));
    }
}
