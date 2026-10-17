# Implementation Tasks: Gameplay Trade, Ability, Guild, WASM Host & Plugin Wiring (Issue #103)

- [x] 1. Fix Trade Item Duplication & Currency Settlement (`crates/gameplay/src/{inventory.rs, trade.rs}`)
  - [x] 1.1 Add `#[serde(default)] pub currency: u64` and `remove_item_stack` / `has_item_stack` helpers to `InventoryComponent` in `crates/gameplay/src/inventory.rs`.
  - [x] 1.2 Update `TradeManager::execute_atomic_swap` in `crates/gameplay/src/trade.rs` to verify item ownership and currency balances, remove offered items from giver inventories, validate post-removal slot and weight capacities, add items to recipient inventories, and settle currency.
  - [x] 1.3 Update `test_atomic_trade_lifecycle_and_swap` and add unit tests in `trade.rs` for missing offered item rejection, insufficient currency rejection, and zero item duplication.

- [x] 2. Validate Range & `ActionQueue` Before Deducting Resources/Cooldowns (`crates/gameplay/src/ability.rs`)
  - [x] 2.1 In `execute_or_start_ability`, validate target existence and Euclidean distance `<= ability.range` when `ability.range > 0.0`.
  - [x] 2.2 Check cooldown status, resource balance (`>= cost`), and `ActionQueue` availability (starting the timed action if `cast_time_sec > 0.0`) before deducting resources and starting cooldowns.
  - [x] 2.3 Add unit tests in `crates/gameplay/src/ability.rs` verifying that out-of-range targets and busy `ActionQueue` rejections preserve 100% of caster mana and do not trigger cooldowns.

- [x] 3. Enforce Single-Guild Membership (`crates/gameplay/src/guild.rs`)
  - [x] 3.1 Reject players already in `self.player_to_guild` in `GuildManager::add_member`, add `try_create_guild` and `remove_member`, and ensure `create_guild` cleans up any prior guild membership if called directly.
  - [x] 3.2 Add unit tests in `crates/gameplay/src/guild.rs` verifying rejection of duplicate guild membership and clean member removal.

- [x] 4. Synchronize WASM Host API with `Octree`, `AttributeSet`, and `EventQueue` (`crates/gameplay/src/scripting/`)
  - [x] 4.1 Encapsulate raw pointers on `HostContext` (`host_api.rs`), adding `octree_ptr` and `events_ptr`.
  - [x] 4.2 In `entity_set_pos`, reject non-finite (`NaN`/`Infinity`) coordinates and update `Octree` when `octree_ptr` is non-null.
  - [x] 4.3 In `entity_modify_health`, safely negate `delta`, sync `AttributeSet` `"health"`, and push `EntityDiedEvent` to `EventQueue` on lethal damage.
  - [x] 4.4 Wire `ctx.octree` and `ctx.events` through `ScriptTickSystem::update` (`plugin.rs`) and `WasmRuntime` (`runtime.rs`), and add unit tests in `crates/gameplay/tests/wasm_scripting_test.rs`.

- [x] 5. Wire `EncumbranceHookHandler` & `DamageProcessingSystem` (`crates/gameplay/src/{inventory.rs, combat.rs}`)
  - [x] 5.1 Implement `HookHandler::handle_with_entities` on `EncumbranceHookHandler` and register it in `InventoryPlugin::build`.
  - [x] 5.2 Implement `DamageProcessingSystem` in `crates/gameplay/src/combat.rs` and register it in `CombatPlugin::build`.
  - [x] 5.3 Add unit tests verifying `InventoryPlugin` cancels `Event::BeforeMove` when encumbered and `CombatPlugin` processes queued `DamageEvent`s during a tick.

- [x] 6. Workspace Quality Gates
  - [x] 6.1 Run `cargo fmt --all -- --check`
  - [x] 6.2 Run `cargo clippy --workspace --all-targets -- -D warnings`
  - [x] 6.3 Run `cargo test --workspace`
