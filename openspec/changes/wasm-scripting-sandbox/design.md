# Architecture & Technical Design: WASM Scripting Sandbox

## Architectural Overview

The WASM Scripting Sandbox provides an isolated, deterministic, and hot-reloadable execution layer for gameplay logic. It decouples high-level game rules (quest triggers, ability formulas, NPC dialogues, dynamic event hooks) from native server binaries, adhering to the Prime Directive in `AGENTS.md`.

```
 +-----------------------------------------------------------------------------------+
 |                             mmo-core (Generic Substrate)                         |
 |  - ScriptFuelConfig, ScriptError, ScriptValue, FuelMeteredExecutor                |
 +-----------------------------------------------------------------------------------+
                                          ^
                                          |
 +-----------------------------------------------------------------------------------+
 |                             mmo-gameplay (Scripting)                              |
 |  +-----------------------------------------------------------------------------+  |
 |  | WasmScriptEngine (wasmtime::Engine, Config: fuel=true, sandboxed memory)   |  |
 |  +-----------------------------------------------------------------------------+  |
 |  | Host API Linker: entity_get/set_pos, get/set_attr, distance_sq, log, events  |  |
 |  +-----------------------------------------------------------------------------+  |
 |  | ScriptHotReloader: atomic RwLock<HashMap<String, Arc<wasmtime::Module>>>    |  |
 |  +-----------------------------------------------------------------------------+  |
 |  | WasmScriptPlugin (implements GameplayPlugin): hooks into ticks & events     |  |
 |  +-----------------------------------------------------------------------------+  |
 +-----------------------------------------------------------------------------------+
                                          ^
                                          |
 +-----------------------------------------------------------------------------------+
 |                             mmo-node (Tick & Admin)                               |
 |  - TickSimulation step integration: metered execution per tick                    |
 |  - Hot-reload coordination: reload module by ID from memory/disk                  |
 +-----------------------------------------------------------------------------------+
```

---

## 1. Engine Substrate & Memory Model

### Virtual Machine Runtime
- **Engine**: `wasmtime::Engine` configured with:
  - `Config::consume_fuel(true)`: Enables instruction-based fuel metering.
  - No WASI filesystem, network, or environment access attached to linker.
  - Memory isolation: Every script invocation runs inside a scoped `wasmtime::Store<HostContext>`.

### Memory Model & Bounds Safety
- Guest scripts allocate standard WASM linear memory (`memory`).
- When strings or byte buffers are passed across the FFI boundary:
  - Guest passes `(ptr: u32, len: u32)`.
  - Host validates bounds using strictly checked 64-bit unsigned arithmetic:
    `((ptr as u64) + (len as u64)) <= (guest_memory.data_size(&caller) as u64)`.
    This prevents 32-bit integer overflow/wraparound attacks (e.g. `ptr = 0xFFFFFFFF, len = 2`).
  - If validation fails, host returns an error code (e.g. `-1`) without dereferencing or panicking.
  - All pointer arithmetic uses checked operations.

### Hook Pipeline Entity Context Extension & Fail-Closed Trap Semantics
- In `crates/core/src/hook.rs`:
  - Extend `HookHandler` trait:
    ```rust
    pub trait HookHandler: Send + Sync {
        fn handle(&self, event: &Event, ctx: &mut EventContext) {}
        fn handle_with_entities(
            &self,
            event: &Event,
            ctx: &mut EventContext,
            _entities: &mut EntityStore,
        ) {
            self.handle(event, ctx);
        }
    }
    ```
  - `HookDispatcher::dispatch_with_entities(&self, event: &Event, entities: &mut EntityStore) -> EventContext` passes mutable entity store access down to handlers that need it.
  - Fail-Closed Semantics: If any script executed within a hook (e.g. `BeforeInteract`, `BeforeMove`) traps, runs out of fuel, or panics, the handler immediately marks the context canceled (`ctx.cancel("script_error: ...")`), preventing exploiters from intentionally triggering traps to bypass server-side validation.

---

## 2. Fuel & Execution Limits

### Fuel Metering
- Each script call is allocated an explicit fuel quota:
  - Default: `50_000` units (equivalent to ~50 microseconds of typical computation).
  - Configurable via `ScriptFuelConfig`.
- Before invocation:
  ```rust
  store.set_fuel(fuel_budget)?;
  ```
- If a guest script contains an infinite loop (`loop {}`) or excessive branching, `wasmtime` automatically halts execution when fuel reaches zero.
- Traps are inspected:
  - If fuel exhausted: returns `ScriptError::OutOfFuel`.
  - If other trap (e.g. division by zero, unreachable): returns `ScriptError::Trap(reason)`.
- The simulation tick loop catches all errors gracefully and logs telemetry; node stability is never compromised.

---

## 3. Host API Specification

Host functions are linked into `wasmtime::Linker<HostContext>` under the module namespace `"env"`:

| Function | Signature | Description |
|---|---|---|
| `entity_get_pos` | `(guid: i64, out_x_ptr: i32, out_y_ptr: i32, out_z_ptr: i32) -> i32` | Writes entity 3D position into guest floats; returns 0 on success, -1 on not found/bounds error |
| `entity_set_pos` | `(guid: i64, x: f32, y: f32, z: f32) -> i32` | Updates entity position in host context; returns 0 on success, -1 on invalid guid |
| `entity_get_attr` | `(guid: i64, key_ptr: i32, key_len: i32, out_ptr: i32, out_max_len: i32) -> i32` | Reads string/JSON attribute from entity into guest buffer; returns bytes written or -1 |
| `entity_set_attr` | `(guid: i64, key_ptr: i32, key_len: i32, val_ptr: i32, val_len: i32) -> i32` | Sets entity attribute; returns 0 on success, -1 on error |
| `entity_distance_sq` | `(guid_a: i64, guid_b: i64) -> f32` | Returns squared Euclidean distance between two entities, or -1.0 if not found |
| `entity_modify_health`| `(guid: i64, delta: i32) -> i32` | Modifies entity health component safely, returning new health |
| `dispatch_event` | `(event_type: i32, target_guid: i64, payload_ptr: i32, payload_len: i32) -> i32` | Enqueues a domain event to host event queue |
| `script_log` | `(level: i32, msg_ptr: i32, msg_len: i32) -> i32` | Logs a message via host tracing framework |

---

## 4. Concurrency & Hot-Reloading

### Concurrency & Lock Discipline
- Compiled modules are immutable once compiled (`wasmtime::Module` is thread-safe, `Send + Sync + Clone`).
- Module registry: `Arc<RwLock<HashMap<String, Arc<wasmtime::Module>>>>`.
  - Read access is acquired during script invocation (or snapshot of `Arc<Module>` is cloned under read lock) so execution never holds a write lock.
  - Hot-reload compilation (`wasmtime::Module::new`) happens *before* acquiring the write lock. The write lock is held only for microseconds during the map insertion/atomic pointer swap.
- Host state accesses during execution are scoped to the current tick thread (exclusive borrow `&mut HostContext` or `&mut EntityStore`), preventing race conditions.

### Hot-Reload Pipeline
1. Input: Byte payload (`.wasm`) or Text (`.wat`).
2. Compilation Stage (Staged):
   - Compile module using `wasmtime::Module::new(&engine, &bytes)`.
   - If compilation fails, return error immediately; the existing module remains untouched.
3. Validation Stage:
   - Verify that required guest entrypoints exist (e.g., `on_cast` or `on_tick`).
4. Commit Stage:
   - Insert into registry map `modules.write().insert(module_id.to_string(), Arc::new(new_module))`.
   - Active entity stores and components are unchanged. Subsequent invocations immediately run the new code.

---

## 5. Failure Modes & Edge Cases

| Scenario | Behavior | Mitigation |
|---|---|---|
| Runaway loop / unbounded recursion | Traps when fuel reaches 0 | Returns `ScriptError::OutOfFuel`; tick completes safely |
| Division by zero / unreachable | WASM Trap | Returns `ScriptError::Trap`; logs warning, no panic |
| Malformed WASM bytecode | Compilation fails | Rejected before insertion into registry; previous version retained |
| Guest passes invalid pointer | Memory bounds check fails | Returns `-1` error code to guest; host does not panic or crash |
| Out of memory in guest | Allocation trap | Handled by store memory limits |
| Zero fuel configuration | Instant out of fuel | Guarded with minimum default fuel budget (`min: 1_000`) |
