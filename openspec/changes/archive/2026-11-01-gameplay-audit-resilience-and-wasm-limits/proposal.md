# Proposal: Gameplay Systems Hardening, State Resilience, and WASM Memory Limits

## Motivation & Problem Statement
A comprehensive production audit across Partition 5 (`crates/gameplay`) identified critical vulnerabilities, data-loss failure modes, and DoS risks:

1. **[Critical] Silent Item Deletion During Equipment Swap on Full Inventory (`crates/gameplay/src/inventory.rs`)**:
   Swapping an equipped item into a full inventory drops the previous item if `inv.add_item(old)` fails, silently destroying the item forever and leaving the inventory in an inconsistent state. Unequip also lacks transactional rollback guarantees and capacity pre-checks.

2. **[Critical] Non-Finite (NaN) Damage and Healing Inducing Zombie/Invulnerable Entities (`crates/gameplay/src/combat.rs`)**:
   If an incoming raw or mitigated damage amount is `NaN`, `current_hp` evaluates to `NaN`. Because comparisons like `NaN <= 0.0` evaluate to `false`, `is_dead` remains `false`, making the damaged entity immortal. Non-finite values in healing, respawn initial HP, and combat resolver mitigations similarly poison health states.

3. **[Critical] Missing WASM ResourceLimiter Inducing OOM Panics (`crates/gameplay/src/scripting/`)**:
   `WasmScriptEngine` and `ScriptRuntime` enable fuel consumption but lack linear memory bounds and a `ResourceLimiter`. Untrusted or buggy guest scripts can execute `memory.grow` up to 4GB of RAM, triggering host node OOM termination and crashing game shards.

4. **[High] Stuck Bilateral Acceptance & Locks on Partial Trade Failure (`crates/gameplay/src/trade.rs`, `village.rs`)**:
   When `execute_atomic_swap` fails validation (e.g. insufficient inventory capacity, encumbrance violation, or missing inventory components), bilateral acceptance and lock flags could remain `true`. If inventory conditions change subsequently, a player can finalize the trade without reciprocal re-verification and consent.

5. **[High] AI FSM Evades to Spawn on Threat Target Switch (`crates/gameplay/src/ai_brain.rs`)**:
   When highest threat switches from the current target to another player, the mob FSM drops aggro and evades all the way back to spawn rather than seamlessly switching target and continuing the pursuit.

## Proposed Changes & User Impact
- **Transactional Equipment Swap & Unequip**: Ensure atomic swaps where no item can be destroyed; if storing the replaced or unequipped item fails, the state is rolled back cleanly. Add explicit unequip packet handling.
- **Strict Non-Finite Sanitization in Combat**: Reject non-finite (`NaN`, `Inf`, `-Inf`) values in `take_damage`, `heal`, `respawn`, `new`, `process_damage_events`, and `RpgCombatResolver`.
- **Authoritative WASM Resource Limiter**: Integrate `wasmtime::ResourceLimiter` / `StoreLimits` into `HostContext` and enforce a strict 32MB linear memory cap on all guest scripts, preventing OOM DoS attacks.
- **Bilateral Trade Acceptance & Lock Invalidation**: Ensure that any atomic swap validation or precondition failure unconditionally resets `accepted_a`, `accepted_b`, `locked_a`, and `locked_b` to `false`.
- **Smooth AI Threat Retargeting**: Transition directly to `FsmState::Chase { target: new_target }` when threat leadership shifts, keeping combat responsive.

## Non-Goals
- Changing wire packet formats or breaking client compatibility.
- Introducing fantasy-specific or genre-specific rules into core crates.
- Replacing the existing `wasmtime` engine or `StoreLimits` mechanism with heavy OS cgroups.
