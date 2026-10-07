//! WASM Scripting Engine managing compilation and module caching via `wasmtime`.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use mmo_core::ScriptError;
use wasmtime::{Engine, Module};

/// Core WebAssembly script engine managing the underlying runtime and compiled module cache.
pub struct WasmScriptEngine {
    engine: Engine,
    modules: Arc<RwLock<HashMap<String, Arc<Module>>>>,
}

impl Default for WasmScriptEngine {
    fn default() -> Self {
        Self::new().expect("Failed to initialize default WasmScriptEngine")
    }
}

impl WasmScriptEngine {
    /// Creates a new WASM scripting engine with fuel metering enabled.
    pub fn new() -> Result<Self, ScriptError> {
        let mut config = wasmtime::Config::new();
        config.consume_fuel(true);
        let engine = Engine::new(&config)
            .map_err(|e| ScriptError::CompilationError(format!("Engine init failed: {e}")))?;
        let modules = Arc::new(RwLock::new(HashMap::new()));
        Ok(WasmScriptEngine { engine, modules })
    }

    /// Returns a reference to the underlying `wasmtime::Engine`.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Compiles raw WASM bytecode or WAT text format into a `wasmtime::Module`.
    pub fn compile_module(&self, wasm_bytes: &[u8]) -> Result<Module, ScriptError> {
        Module::new(&self.engine, wasm_bytes)
            .map_err(|e| ScriptError::CompilationError(e.to_string()))
    }

    /// Staged compilation and atomic registration of a compiled module.
    ///
    /// The module is compiled first before acquiring the write lock; if compilation
    /// fails, existing modules remain intact.
    pub fn load_module(&self, module_id: &str, wasm_bytes: &[u8]) -> Result<(), ScriptError> {
        let module = self.compile_module(wasm_bytes)?;
        let mut modules = self
            .modules
            .write()
            .map_err(|e| ScriptError::ExecutionError(format!("Lock poisoned: {e}")))?;
        modules.insert(module_id.to_string(), Arc::new(module));
        Ok(())
    }

    /// Retrieves an `Arc<Module>` by module identifier.
    pub fn get_module(&self, module_id: &str) -> Option<Arc<Module>> {
        let modules = self.modules.read().ok()?;
        modules.get(module_id).cloned()
    }

    /// Removes a registered module by identifier.
    pub fn remove_module(&self, module_id: &str) -> bool {
        if let Ok(mut modules) = self.modules.write() {
            modules.remove(module_id).is_some()
        } else {
            false
        }
    }

    /// Returns `true` if a module with the given ID is registered.
    pub fn has_module(&self, module_id: &str) -> bool {
        if let Ok(modules) = self.modules.read() {
            modules.contains_key(module_id)
        } else {
            false
        }
    }

    /// Returns a list of all currently registered module IDs.
    pub fn module_ids(&self) -> Vec<String> {
        if let Ok(modules) = self.modules.read() {
            modules.keys().cloned().collect()
        } else {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_new() {
        let engine = WasmScriptEngine::new();
        assert!(engine.is_ok());
    }

    #[test]
    fn test_compile_and_load_module() {
        let engine = WasmScriptEngine::new().unwrap();
        let wat = r#"(module (func (export "test") (result i32) i32.const 42))"#;
        let wasm_bytes = wat::parse_str(wat).unwrap();
        let module_id = "test_module";
        assert!(engine.load_module(module_id, &wasm_bytes).is_ok());
        assert!(engine.has_module(module_id));
    }

    #[test]
    fn test_get_module() {
        let engine = WasmScriptEngine::new().unwrap();
        let wat = r#"(module (func (export "test") (result i32) i32.const 42))"#;
        let wasm_bytes = wat::parse_str(wat).unwrap();
        let module_id = "test_module";
        engine.load_module(module_id, &wasm_bytes).unwrap();
        let module = engine.get_module(module_id);
        assert!(module.is_some());
    }

    #[test]
    fn test_reject_malformed_wasm() {
        let engine = WasmScriptEngine::new().unwrap();
        let malformed_wasm = b"invalid wasm bytecode content";
        let module_id = "malformed_module";
        let result = engine.load_module(module_id, malformed_wasm);
        assert!(result.is_err());
    }

    #[test]
    fn test_remove_module() {
        let engine = WasmScriptEngine::new().unwrap();
        let wat = r#"(module (func (export "test") (result i32) i32.const 42))"#;
        let wasm_bytes = wat::parse_str(wat).unwrap();
        let module_id = "test_module";
        engine.load_module(module_id, &wasm_bytes).unwrap();
        assert!(engine.remove_module(module_id));
        assert!(!engine.has_module(module_id));
    }

    #[test]
    fn test_module_ids() {
        let engine = WasmScriptEngine::new().unwrap();
        let wat = r#"(module (func (export "test") (result i32) i32.const 42))"#;
        let wasm_bytes = wat::parse_str(wat).unwrap();
        let module_id1 = "module1";
        let module_id2 = "module2";
        engine.load_module(module_id1, &wasm_bytes).unwrap();
        engine.load_module(module_id2, &wasm_bytes).unwrap();
        let ids = engine.module_ids();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&module_id1.to_string()));
        assert!(ids.contains(&module_id2.to_string()));
    }
}
