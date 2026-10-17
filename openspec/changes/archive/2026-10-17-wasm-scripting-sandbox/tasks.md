# Implementation Tasks: WASM Scripting Sandbox

## Stage 1 & 2: Review & Alignment
- [x] 1. Draft OpenSpec proposal, specifications, architecture design, and tasks.
- [x] 2. Invoke Pro subagent for rigorous architectural and security review.
- [x] 3. Refine specifications and design based on Pro subagent recommendations.

## Stage 3: Core Primitives & Interfaces (`crates/core`)
- [x] 4. Refactor `HookHandler` and `HookDispatcher` in `crates/core/src/hook.rs`:
  - Add `handle_with_entities` default method to `HookHandler`.
  - Add `dispatch_with_entities` method to `HookDispatcher`.
  - Verification: `cargo test -p mmo-core --lib hook`
- [x] 5. Define genre-agnostic script execution abstractions in `crates/core/src/script.rs`:
  - `ScriptFuelConfig`, `ScriptError`, `ScriptValue`, `FuelMeteredExecutor` trait.
  - Verification: `cargo test -p mmo-core --lib script`
- [x] 6. Export `script` module in `crates/core/src/lib.rs`.
  - Verification: `cargo check -p mmo-core`

## Stage 4: WASM Scripting Engine & Safe Host API (`crates/gameplay`)
- [x] 6. Ensure `wasmtime` dependency configured in `crates/gameplay/Cargo.toml` with minimal necessary features (`runtime`, `cranelift`, `wat`, `std`).
  - Verification: `cargo check -p mmo-gameplay`
- [x] 7. Implement `WasmScriptEngine` in `crates/gameplay/src/scripting/engine.rs`:
  - `wasmtime::Engine` initialization with `consume_fuel(true)`.
  - Bytecode/WAT compilation into `wasmtime::Module`.
  - Thread-safe module registry with atomic swapping.
  - Verification: Unit tests for valid WAT compilation, invalid bytecode rejection.
- [x] 8. Implement safe, bounds-checked Host API in `crates/gameplay/src/scripting/host_api.rs`:
  - Linker bindings for `entity_get_pos`, `entity_set_pos`, `entity_get_attr`, `entity_set_attr`, `entity_distance_sq`, `entity_modify_health`, `dispatch_event`, and `script_log`.
  - Strict guest memory bounds validation preventing out-of-bounds reads/writes.
  - Verification: Host API unit tests with mock guest modules.
- [x] 9. Implement `WasmRuntime` and fuel execution in `crates/gameplay/src/scripting/runtime.rs`:
  - Execution wrapper with fuel quota and fuel consumption accounting.
  - Deterministic termination of infinite loops (`OutOfFuel`).
  - Verification: Test calling normal WASM functions vs runaway `loop {}` modules.
- [x] 10. Implement `ScriptHotReloader` in `crates/gameplay/src/scripting/hot_reload.rs`:
  - Dynamic module reloading by ID without dropping active state.
  - Staged compilation rejecting broken modules without modifying active modules.
  - Verification: Test hot-swapping a module from version A (returning 10) to version B (returning 42).
- [x] 11. Implement `WasmScriptPlugin` in `crates/gameplay/src/scripting/plugin.rs`:
  - Implements `mmo_core::GameplayPlugin`.
  - Registers script hooks into `PluginContext` (events, tick systems).
  - Verification: Integration tests with `PluginManager`.
- [x] 12. Export `scripting` module in `crates/gameplay/src/lib.rs`.
  - Verification: `cargo test -p mmo-gameplay`

## Stage 5: Node Simulation Integration & End-to-End Verification (`crates/node`)
- [x] 13. Integrate `ScriptHotReloader` / script execution coordinator into `TickSimulation` in `crates/node/src/sim.rs`:
  - Provide helper method `reload_script(id, wasm_bytes)` and accessor for script engine.
  - Verification: Unit tests in `sim.rs` verifying script execution and reload during simulation tick.
- [x] 14. Add comprehensive integration and stress tests in `crates/gameplay/tests/wasm_scripting_test.rs`:
  - Runaway loop fuel exhaustion tests under simulation load.
  - Live hot-reloading during simulated combat / ability execution.
  - Host API memory bounds violation rejection tests.
  - Verification: `cargo test --test wasm_scripting_test`

## Stage 6: Quality Gates & Final Verification
- [x] 15. Run full workspace quality gates:
  - Formatting: `cargo fmt --all -- --check`
  - Strict Clippy: `cargo clippy --workspace --all-targets -- -D warnings`
  - Workspace tests: `cargo test --workspace`
- [x] 16. Git commit, push branch, create PR with auto-merge enabled.
