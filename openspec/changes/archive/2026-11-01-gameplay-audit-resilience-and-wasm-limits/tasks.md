# Tasks: Gameplay Systems Resilience, State Consistency, and WASM Limits

## 1. Inventory & Equipment Safety
- [x] 1.1 In `crates/gameplay/src/inventory.rs`, implement transactional rollback in `InventoryPacketHandler` for sub-opcode 1 (equip swap).
- [x] 1.2 In `crates/gameplay/src/inventory.rs`, add sub-opcode 2 (unequip item) with slot bounds checking (`0..=4`), empty equipment slot check, capacity check before unequipping, and zero-loss guarantees.
- [x] 1.3 Add unit tests verifying equip swap on full inventory, failed store rollback, unequip on full inventory, unequip on empty slot, and successful unequip.

## 2. Combat Float & Health Component Hardening
- [x] 2.1 In `crates/gameplay/src/combat.rs`, ensure `HealthComponent::take_damage`, `heal`, `respawn`, and `new` reject/sanitize non-finite (`NaN`, `Inf`) floats. Specifically guard `respawn` against `f32::NAN.clamp(...)` producing `NaN`.
- [x] 2.2 In `crates/gameplay/src/combat.rs`, harden `process_damage_events` and `RpgCombatResolver` against non-finite float propagation to attributes and properties.
- [x] 2.3 Add unit tests verifying `take_damage(f32::NAN)`, `heal(f32::NAN)`, `respawn(f32::NAN)`, and subsequent lethal damage behavior.

## 3. WASM Resource Limiter & Memory Bounds
- [x] 3.1 In `crates/gameplay/src/scripting/host_api.rs`, add `limiter: wasmtime::StoreLimits` with 32MB linear memory cap (`StoreLimitsBuilder::new().memory_size(32 * 1024 * 1024).build()`) to `HostContext`.
- [x] 3.2 In `crates/gameplay/src/scripting/runtime.rs`, bind `store.limiter(|ctx| &mut ctx.limiter)` upon `Store` creation in `WasmRuntime::execute_function_internal`.
- [x] 3.3 Add unit tests executing guest WASM scripts attempting `memory.grow` beyond 32MB, verifying growth rejection and host resilience.

## 4. Trade Bilateral State Reset Hardening
- [x] 4.1 In `crates/gameplay/src/trade.rs`, verify that all validation failure paths in `execute_atomic_swap` (including pre-validation missing component checks) reset `accepted_a`, `accepted_b`, `locked_a`, and `locked_b` to `false`.
- [x] 4.2 In `crates/gameplay/src/village.rs`, verify trade packet handling preserves consistency on failed swap.
- [x] 4.3 Add unit tests confirming both players' locks and acceptances are cleared after swap validation errors and pre-validation missing inventory component errors.

## 5. AI FSM Threat Retargeting
- [x] 5.1 In `crates/gameplay/src/ai_brain.rs`, verify `Chase` and `Attack` states seamlessly switch to `Chase { target: new_target }` when highest threat shifts.
- [x] 5.2 Add unit test `test_fsm_brain_threat_retargeting_preserves_aggro` validating that threat leadership shift updates the target position and transitions to `Chase` without entering `Evade`.

## 6. Workspace Quality Gates & Verification
- [x] 6.1 Run `cargo test -p mmo-gameplay` and ensure all tests pass.
- [x] 6.2 Run `cargo fmt --all -- --check`.
- [x] 6.3 Run `cargo clippy --workspace --all-targets -- -D warnings`.
- [x] 6.4 Run full workspace test suite `cargo test --workspace`.
