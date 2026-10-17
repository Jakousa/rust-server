//! WASM execution runtime with deterministic fuel metering and host context injection.

use std::sync::Arc;

use mmo_core::{EntityStore, FuelMeteredExecutor, Guid, ScriptError, ScriptValue};
use wasmtime::{Linker, Store};

use super::engine::WasmScriptEngine;
use super::host_api::{register_host_api, HostContext};

/// Execution runtime managing module instantiations, typed function execution, and fuel budgeting.
pub struct WasmRuntime {
    engine: Arc<WasmScriptEngine>,
    linker: Linker<HostContext>,
}

impl WasmRuntime {
    /// Creates a new `WasmRuntime` bound to the given script engine, initializing the host API linker.
    pub fn new(engine: Arc<WasmScriptEngine>) -> Result<Self, ScriptError> {
        let mut linker = Linker::<HostContext>::new(engine.engine());
        register_host_api(&mut linker)?;
        Ok(WasmRuntime { engine, linker })
    }

    /// Returns a reference to the shared `WasmScriptEngine`.
    pub fn engine(&self) -> &Arc<WasmScriptEngine> {
        &self.engine
    }

    /// Executes an exported function in the specified module with fuel budgeting.
    pub fn execute_function(
        &self,
        module_id: &str,
        func_name: &str,
        args: &[ScriptValue],
        entities: Option<&mut EntityStore>,
        fuel_limit: u64,
    ) -> Result<(Vec<ScriptValue>, u64), ScriptError> {
        let host_ctx = HostContext::new(entities);
        self.execute_function_internal(module_id, func_name, args, host_ctx, fuel_limit)
    }

    /// Executes an exported function with full tick context (entities, octree, and events).
    #[allow(clippy::too_many_arguments)]
    pub fn execute_function_with_tick_context(
        &self,
        module_id: &str,
        func_name: &str,
        args: &[ScriptValue],
        entities: &mut EntityStore,
        octree: &mut mmo_core::Octree,
        events: &mut mmo_core::EventQueue,
        fuel_limit: u64,
    ) -> Result<(Vec<ScriptValue>, u64), ScriptError> {
        let host_ctx = HostContext::with_tick_context(entities, octree, events);
        self.execute_function_internal(module_id, func_name, args, host_ctx, fuel_limit)
    }

    fn execute_function_internal(
        &self,
        module_id: &str,
        func_name: &str,
        args: &[ScriptValue],
        host_ctx: HostContext,
        fuel_limit: u64,
    ) -> Result<(Vec<ScriptValue>, u64), ScriptError> {
        let module = self
            .engine
            .get_module(module_id)
            .ok_or_else(|| ScriptError::ModuleNotFound(module_id.to_string()))?;

        let mut store = Store::new(self.engine.engine(), host_ctx);
        store
            .set_fuel(fuel_limit)
            .map_err(|e| ScriptError::ExecutionError(format!("Failed to set fuel: {e}")))?;

        let instance = self
            .linker
            .instantiate(&mut store, &module)
            .map_err(|e| ScriptError::ExecutionError(format!("Instantiate error: {e}")))?;

        let func = instance
            .get_func(&mut store, func_name)
            .ok_or_else(|| ScriptError::ExportNotFound(func_name.to_string()))?;

        let wasm_args: Vec<wasmtime::Val> = args
            .iter()
            .map(|arg| match arg {
                ScriptValue::I32(v) => wasmtime::Val::I32(*v),
                ScriptValue::I64(v) => wasmtime::Val::I64(*v),
                ScriptValue::F32(v) => wasmtime::Val::F32(v.to_bits()),
                ScriptValue::F64(v) => wasmtime::Val::F64(v.to_bits()),
                _ => wasmtime::Val::I32(0),
            })
            .collect();

        let num_results = func.ty(&store).results().len();
        let mut wasm_results = vec![wasmtime::Val::I32(0); num_results];

        match func.call(&mut store, &wasm_args, &mut wasm_results) {
            Ok(()) => {
                let remaining_fuel = store.get_fuel().unwrap_or(0);
                let fuel_consumed = fuel_limit.saturating_sub(remaining_fuel);

                let script_results = wasm_results
                    .into_iter()
                    .map(|val| match val {
                        wasmtime::Val::I32(v) => ScriptValue::I32(v),
                        wasmtime::Val::I64(v) => ScriptValue::I64(v),
                        wasmtime::Val::F32(b) => ScriptValue::F32(f32::from_bits(b)),
                        wasmtime::Val::F64(b) => ScriptValue::F64(f64::from_bits(b)),
                        _ => ScriptValue::I32(0),
                    })
                    .collect();

                Ok((script_results, fuel_consumed))
            }
            Err(trap) => {
                if let Some(t) = trap.downcast_ref::<wasmtime::Trap>() {
                    if matches!(t, wasmtime::Trap::OutOfFuel) {
                        return Err(ScriptError::OutOfFuel);
                    }
                }
                let err_str = trap.to_string();
                if err_str.to_lowercase().contains("fuel") {
                    Err(ScriptError::OutOfFuel)
                } else {
                    Err(ScriptError::Trap(err_str))
                }
            }
        }
    }

    /// Invokes a gameplay ability cast entrypoint (`on_cast` or specified function name).
    #[allow(clippy::too_many_arguments)]
    pub fn call_ability_cast(
        &self,
        module_id: &str,
        func_name: &str,
        caster: Guid,
        target: Guid,
        ability_id: u32,
        entities: &mut EntityStore,
        fuel: u64,
    ) -> Result<i32, ScriptError> {
        let args = [
            ScriptValue::I64(caster.raw() as i64),
            ScriptValue::I64(target.raw() as i64),
            ScriptValue::I32(ability_id as i32),
        ];

        let (results, _) =
            self.execute_function(module_id, func_name, &args, Some(entities), fuel)?;
        results
            .first()
            .and_then(|v| v.as_i32())
            .ok_or_else(|| ScriptError::ExecutionError("Invalid return type from cast".to_string()))
    }

    /// Invokes a quest event handler script.
    #[allow(clippy::too_many_arguments)]
    pub fn call_quest_event(
        &self,
        module_id: &str,
        func_name: &str,
        player: Guid,
        quest_id: u32,
        event_code: u32,
        entities: &mut EntityStore,
        fuel: u64,
    ) -> Result<i32, ScriptError> {
        let args = [
            ScriptValue::I64(player.raw() as i64),
            ScriptValue::I32(quest_id as i32),
            ScriptValue::I32(event_code as i32),
        ];

        let (results, _) =
            self.execute_function(module_id, func_name, &args, Some(entities), fuel)?;
        results.first().and_then(|v| v.as_i32()).ok_or_else(|| {
            ScriptError::ExecutionError("Invalid return type from quest".to_string())
        })
    }
}

impl FuelMeteredExecutor for WasmRuntime {
    fn execute(
        &self,
        module_id: &str,
        function_name: &str,
        args: &[ScriptValue],
        fuel: u64,
    ) -> Result<(Vec<ScriptValue>, u64), ScriptError> {
        self.execute_function(module_id, function_name, args, None, fuel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normal_calculation() {
        let engine = Arc::new(WasmScriptEngine::new().unwrap());
        let wat = r#"
        (module
            (func (export "add") (param i32 i32) (result i32)
                local.get 0
                local.get 1
                i32.add
            )
        )
        "#;
        let wasm_bytes = wat::parse_str(wat).unwrap();
        engine.load_module("math", &wasm_bytes).unwrap();

        let runtime = WasmRuntime::new(engine).unwrap();
        let (results, fuel_consumed) = runtime
            .execute_function(
                "math",
                "add",
                &[ScriptValue::I32(10), ScriptValue::I32(20)],
                None,
                10_000,
            )
            .unwrap();

        assert_eq!(results, vec![ScriptValue::I32(30)]);
        assert!(fuel_consumed > 0);
    }

    #[test]
    fn test_runaway_loop_out_of_fuel() {
        let engine = Arc::new(WasmScriptEngine::new().unwrap());
        let wat = r#"
        (module
            (func (export "runaway") (result i32)
                (loop (br 0))
                i32.const 1
            )
        )
        "#;
        let wasm_bytes = wat::parse_str(wat).unwrap();
        engine.load_module("runaway_mod", &wasm_bytes).unwrap();

        let runtime = WasmRuntime::new(engine).unwrap();
        let result = runtime.execute_function("runaway_mod", "runaway", &[], None, 5_000);

        assert!(matches!(result, Err(ScriptError::OutOfFuel)));
    }

    #[test]
    fn test_division_by_zero_trap() {
        let engine = Arc::new(WasmScriptEngine::new().unwrap());
        let wat = r#"
        (module
            (func (export "div0") (result i32)
                i32.const 1
                i32.const 0
                i32.div_s
            )
        )
        "#;
        let wasm_bytes = wat::parse_str(wat).unwrap();
        engine.load_module("trap_mod", &wasm_bytes).unwrap();

        let runtime = WasmRuntime::new(engine).unwrap();
        let result = runtime.execute_function("trap_mod", "div0", &[], None, 10_000);

        assert!(matches!(result, Err(ScriptError::Trap(_))));
    }

    #[test]
    fn test_missing_export() {
        let engine = Arc::new(WasmScriptEngine::new().unwrap());
        let wat = r#"(module (func (export "foo") (result i32) i32.const 1))"#;
        let wasm_bytes = wat::parse_str(wat).unwrap();
        engine.load_module("test_mod", &wasm_bytes).unwrap();

        let runtime = WasmRuntime::new(engine).unwrap();
        let result = runtime.execute_function("test_mod", "non_existent", &[], None, 10_000);

        assert!(matches!(result, Err(ScriptError::ExportNotFound(_))));
    }
}
