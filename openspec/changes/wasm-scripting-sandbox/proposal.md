# Proposal: Sandboxed WASM Scripting Engine with Fuel Metering & Hot-Reload

## Why

In accordance with the Prime Directive (`AGENTS.md`), game-specific mechanics belong in decoupled plugins or extension layers rather than core server binaries. Currently, modifying quest conditions, spell calculations, encounter scripts, or dynamic world events requires editing Rust source code, recompiling crates, and restarting simulation nodes. This increases operational friction, prevents live balance tuning, and risks server downtime during live operations.

To enable live gameplay customization while strictly maintaining stability and security, we introduce a lightweight, sandboxed WebAssembly (WASM) scripting runtime (`wasmtime`). Crucially, this scripting engine must enforce strict execution fuel/gas metering, prevent runaway CPU loops from violating the 20 Hz (50ms) tick deadline, isolate scripts from arbitrary host filesystem/network syscalls, expose safe typed Host APIs for entity query and mutation, and support atomic hot-reloading of modules at tick boundaries without losing active simulation state.

## What Changes

1. **Core Metered Execution Substrate (`crates/core`)**:
   - Establish genre-agnostic script execution primitives: `ScriptFuelConfig`, `ScriptError` (accounting for fuel exhaustion, memory traps, and missing symbols), `ScriptValue`, and metered script execution interfaces.
   - Core crates remain strictly genre-agnostic, with no game-specific assumptions.

2. **WASM Scripting Engine & Safe Host API (`crates/gameplay`)**:
   - Integrate `wasmtime` with fuel consumption enabled (`Config::consume_fuel(true)`), sandboxed memory without raw OS/filesystem/socket access.
   - Implement `WasmScriptEngine` supporting compilation and management of WASM modules (from `.wasm` binary or `.wat` text format).
   - Implement typed, bounds-checked Host API bindings (`host_api.rs`) enabling scripts to:
     - Query and update entity positions (`entity_get_position`, `entity_set_position`).
     - Query and mutate entity attributes / health / stats (`entity_get_attribute`, `entity_set_attribute`, `entity_modify_health`).
     - Calculate spatial distances (`entity_distance_sq`).
     - Dispatch events into the simulation event queue (`dispatch_event`).
     - Safe logging (`script_log`).
   - Implement deterministic fuel exhaustion handling: any infinite loop or runaway script trap immediately aborts the guest execution without panicking or blocking the simulation tick loop.
   - Implement `ScriptHotReloader` enabling atomic replacement of compiled modules in memory or from asset directories without restarting the simulation node or dropping entity state.
   - Implement `WasmScriptPlugin` implementing `mmo_core::GameplayPlugin` to hook into ticks, lifecycle events, and ability/quest dispatches.

3. **Node Simulation Integration (`crates/node`)**:
   - Integrate the script coordinator into `TickSimulation`, allowing runtime reloading of script modules via RPC / command, and executing script hooks safely during tick updates.

## Capabilities

### New Capabilities
- `gameplay/scripting/engine`: WASM compilation, module registry, and deterministic fuel metering.
- `gameplay/scripting/host_api`: Sandboxed, typed host bindings for entity inspection, mutation, events, and logging with memory safety guards.
- `gameplay/scripting/hot_reload`: Atomic module swapping at tick boundaries without simulation interruption.
- `gameplay/scripting/plugin`: Integration of WASM scripts into the `GameplayPlugin` lifecycle.

### Modified Capabilities
- `core/script`: Generalized abstractions for metered execution and script lifecycle.
- `node/sim`: Simulation hook for hot-reload coordination.

## Impact

- **Zero Core Assumptions**: Core crates stay completely unopinionated and genre-agnostic.
- **Safety & Isolation**: Untrusted or buggy scripts cannot crash the process, freeze the tick loop, or access host files/network.
- **Hot-Reloading**: Ability triggers, quest checks, and world events can be modified and reloaded live without dropping players or restarting processes.
- **High Performance**: WASM execution via JIT/leaf compilation completes in microseconds, bounded strictly by fuel limits.
