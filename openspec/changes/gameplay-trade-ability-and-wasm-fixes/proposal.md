# Fix Trade Item Duplication, Ability Pre-Checks, Guild Roster Validation, WASM Host Sync & Plugin Wiring Proposal

## Context & Problem Statement
Auditing `crates/gameplay` (Issue #103) revealed five correctness and integration bugs across trading, ability execution, guild roster management, WASM host bindings, and plugin registration:

1. **Item Duplication & Unsettled Currency in `TradeManager::execute_atomic_swap` (`crates/gameplay/src/trade.rs:209-268`)**:
   - `execute_atomic_swap` adds `session.offer_a.items` to `inv2` and `session.offer_b.items` to `inv1` without verifying that `player_a` and `player_b` actually possess the offered items, without removing those items from `inv1` and `inv2`, and without checking or transferring `offer_a.currency` and `offer_b.currency`.

2. **Resource & Cooldown Deducted Before Range and Busy Checks in `execute_or_start_ability` (`crates/gameplay/src/ability.rs:100-159, 211-233`)**:
   - `execute_or_start_ability` deducts mana/energy (`lines 119-129`) and triggers cooldowns (`lines 132-134`) **before** verifying that `ActionQueue::start_action(..., QueuePolicy::RejectIfBusy, ...)` (`lines 145-148`) succeeds. If the caster is already casting an action, the cast is rejected, but mana and cooldowns are already lost.
   - Neither `execute_or_start_ability` nor `execute_or_start_ability_with_collision` checks that the target is within `ability.range`.

3. **Inconsistent Guild Roster State (`crates/gameplay/src/guild.rs:130-160`)**:
   - Calling `GuildManager::create_guild` or `GuildManager::add_member` for a player already in a guild overwrites `self.player_to_guild.insert(player, guild_id)` without rejecting the action, leaving the player dangling in the previous guild's `members` map.

4. **Spatial Octree Desynchronization, Missing Death Events, and Raw Pointer Exposure in WASM Host API (`crates/gameplay/src/scripting/host_api.rs`)**:
   - `entity_set_pos` (`lines 123-143`) mutates `entity.position` without rejecting `NaN`/`Infinity` coordinates or updating `Octree`.
   - `entity_modify_health` (`lines 170-198`) mutates `HealthComponent` without syncing `AttributeSet` `"health"` or emitting `EntityDiedEvent` when health reaches `0`.
   - `HostContext` (`lines 9-44`) exposes public raw pointer fields (`pub entities_ptr: *mut EntityStore`).

5. **Unwired `EncumbranceHookHandler` and `process_damage_events` (`crates/gameplay/src/inventory.rs:118-125, 203-217` & `combat.rs:124-139`)**:
   - `EncumbranceHookHandler::handle` and `InventoryPlugin::build`'s `Event::BeforeMove` closure have empty bodies instead of checking `check_encumbrance` and calling `ctx.cancel("encumbered")`.
   - `CombatPlugin::build` only registers `HealthRegenSystem` and never registers a `TickSystem` to drain and process `DamageEvent`s via `process_damage_events`.

## Proposed Solution

1. **True Atomic Swap & Currency Settlement in `TradeManager`**:
   - Add `#[serde(default)] pub currency: u64` to `InventoryComponent`.
   - In `TradeManager::execute_atomic_swap`, verify both inventories contain the offered items and sufficient currency (`inv.currency >= offer.currency`), check post-swap slot and weight capacities (`current_weight - outgoing_weight + incoming_weight <= max_weight`), remove offered items from giver inventories, add them to recipient inventories, and settle currency balances atomically.

2. **Range & `ActionQueue` Pre-Validation in `ability.rs`**:
   - Validate target existence and Euclidean distance `<= ability.range` (when `ability.range > 0.0`) before mutating caster state.
   - Verify cooldown availability, sufficient resource balance (`get_current >= cost`), and `ActionQueue` availability (starting the timed action if `cast_time_sec > 0.0` or checking `!queue.is_busy()`) **before** deducting resources and starting cooldowns.

3. **Strict Single-Guild Membership in `GuildManager`**:
   - Reject players already in `self.player_to_guild` in `GuildManager::add_member` and `GuildManager::create_guild` (or `try_create_guild`), and add `remove_member` cleanup.

4. **Safe, Synchronized WASM Host Bindings**:
   - Validate `x.is_finite() && y.is_finite() && z.is_finite()` in `entity_set_pos` and update `Octree` when `octree_ptr` is bound in `HostContext`.
   - In `entity_modify_health`, safely negate `delta`, synchronize `AttributeSet` `"health"`, and emit `EntityDiedEvent` into `EventQueue` on death.
   - Encapsulate raw pointers on `HostContext` as private fields and bind `ctx.octree` and `ctx.events` in `ScriptTickSystem::update`.

5. **Wire `EncumbranceHookHandler` and `DamageProcessingSystem`**:
   - Implement `HookHandler::handle_with_entities` on `EncumbranceHookHandler` to cancel `Event::BeforeMove` when `check_encumbrance(&ent.components)` is true, and register it in `InventoryPlugin::build`.
   - Define `DamageProcessingSystem` invoking `process_damage_events(ctx.events, ctx.entities)` and register it in `CombatPlugin::build`.

## Scope & Non-Goals
- **In Scope**: `crates/gameplay/src/{trade.rs, inventory.rs, ability.rs, guild.rs, combat.rs, lib.rs}` and `crates/gameplay/src/scripting/{host_api.rs, runtime.rs, plugin.rs}`.
- **Non-Goals**: Modifying core engine crates (`core`, `node`, `schema`, `storage`, `gateway`, `director`).
