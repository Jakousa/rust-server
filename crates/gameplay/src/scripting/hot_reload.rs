//! Atomic script hot-reload coordinator supporting live module replacement.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use mmo_core::ScriptError;

use super::engine::WasmScriptEngine;
use super::runtime::WasmRuntime;

/// Coordinator managing atomic hot-reloading of WebAssembly modules at runtime.
pub struct ScriptHotReloader {
    runtime: Arc<WasmRuntime>,
}

impl ScriptHotReloader {
    /// Creates a new `ScriptHotReloader` wrapping the specified WASM runtime.
    pub fn new(runtime: Arc<WasmRuntime>) -> Self {
        ScriptHotReloader { runtime }
    }

    /// Returns a reference to the active `WasmRuntime`.
    pub fn runtime(&self) -> &Arc<WasmRuntime> {
        &self.runtime
    }

    /// Returns a reference to the underlying `WasmScriptEngine`.
    pub fn engine(&self) -> &Arc<WasmScriptEngine> {
        self.runtime.engine()
    }

    /// Atomically compiles and reloads a WASM module by ID.
    ///
    /// If compilation of `wasm_bytes` fails, the existing module in the registry
    /// remains active and unmodified.
    pub fn reload_module(&self, module_id: &str, wasm_bytes: &[u8]) -> Result<(), ScriptError> {
        self.engine().load_module(module_id, wasm_bytes)
    }

    /// Reads a WASM binary or WAT file from disk and reloads it into the registry.
    pub fn reload_from_file<P: AsRef<Path>>(
        &self,
        module_id: &str,
        path: P,
    ) -> Result<(), ScriptError> {
        let bytes = fs::read(path).map_err(|e| ScriptError::Io(e.to_string()))?;
        self.reload_module(module_id, &bytes)
    }

    /// Unloads a module from the registry.
    pub fn unload_module(&self, module_id: &str) -> bool {
        self.engine().remove_module(module_id)
    }

    /// Returns a list of all currently loaded module IDs.
    pub fn loaded_modules(&self) -> Vec<String> {
        self.engine().module_ids()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::ScriptValue;

    #[test]
    fn test_hot_swap_module() {
        let engine = Arc::new(WasmScriptEngine::new().unwrap());
        let runtime = Arc::new(WasmRuntime::new(engine).unwrap());
        let hot_reloader = ScriptHotReloader::new(runtime.clone());

        // Version 1 of calculator: calc() -> 10
        let wat_v1 = r#"(module (func (export "calc") (result i32) i32.const 10))"#;
        let bytes_v1 = wat::parse_str(wat_v1).unwrap();
        hot_reloader.reload_module("calc", &bytes_v1).unwrap();

        let (res_v1, _) = runtime
            .execute_function("calc", "calc", &[], None, 10_000)
            .unwrap();
        assert_eq!(res_v1, vec![ScriptValue::I32(10)]);

        // Version 2 of calculator: calc() -> 42
        let wat_v2 = r#"(module (func (export "calc") (result i32) i32.const 42))"#;
        let bytes_v2 = wat::parse_str(wat_v2).unwrap();
        hot_reloader.reload_module("calc", &bytes_v2).unwrap();

        let (res_v2, _) = runtime
            .execute_function("calc", "calc", &[], None, 10_000)
            .unwrap();
        assert_eq!(res_v2, vec![ScriptValue::I32(42)]);
    }

    #[test]
    fn test_staged_compilation_rejects_broken_without_unloading() {
        let engine = Arc::new(WasmScriptEngine::new().unwrap());
        let runtime = Arc::new(WasmRuntime::new(engine).unwrap());
        let hot_reloader = ScriptHotReloader::new(runtime.clone());

        // Load valid version
        let wat_v1 = r#"(module (func (export "calc") (result i32) i32.const 10))"#;
        let bytes_v1 = wat::parse_str(wat_v1).unwrap();
        hot_reloader.reload_module("calc", &bytes_v1).unwrap();

        // Attempt to hot reload broken bytes
        let broken_bytes = b"not valid wasm bytecode";
        let res = hot_reloader.reload_module("calc", broken_bytes);
        assert!(res.is_err());

        // Verify active module is still V1 (returning 10)
        let (still_v1, _) = runtime
            .execute_function("calc", "calc", &[], None, 10_000)
            .unwrap();
        assert_eq!(still_v1, vec![ScriptValue::I32(10)]);
    }
}
