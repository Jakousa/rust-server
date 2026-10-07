//! Sandboxed WebAssembly (WASM) Gameplay Scripting Sandbox.
//!
//! Exposes:
//! - [`WasmScriptEngine`]: Bytecode/WAT compilation, engine configuration, and thread-safe module caching.
//! - [`HostContext`] & [`register_host_api`]: Authoritative Host API with 64-bit bounds-checked memory access.
//! - [`WasmRuntime`]: Fuel-metered execution, safe error mapping, and trap handling.
//! - [`ScriptHotReloader`]: Atomic module swapping without dropping active entity state.
//! - [`WasmScriptPlugin`]: Standalone `GameplayPlugin` integration into ticks and event pipelines.

pub mod engine;
pub mod host_api;
pub mod hot_reload;
pub mod plugin;
pub mod runtime;

pub use engine::WasmScriptEngine;
pub use host_api::{register_host_api, validate_memory_bounds, HostContext};
pub use hot_reload::ScriptHotReloader;
pub use plugin::{ScriptHookHandler, ScriptTickSystem, WasmScriptPlugin};
pub use runtime::WasmRuntime;
