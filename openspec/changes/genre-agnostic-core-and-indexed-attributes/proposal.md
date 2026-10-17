# Enforce Prime Directive: Genre-Agnostic Core Decoupling & Indexed Attribute Substrate Proposal

## Context & Problem Statement
Per `AGENTS.md` Section 1 (**Prime Directive: Unopinionated, Genre-Agnostic MMO Engine**), `crates/core`, `crates/node`, `crates/schema`, `crates/storage`, `crates/gateway`, and `crates/director` must remain strictly genre-agnostic, unopinionated, and mathematically generalized, with all game-specific mechanics isolated in `crates/gameplay` or modular plugins via `GameplayPlugin`.

Currently, several core crates violate this boundary (Issue #96):

1. **Direct `mmo-gameplay` Dependency in `crates/node` (`crates/node/Cargo.toml:15`)**:
   - `mmo-node` directly depends on `mmo-gameplay` in `[dependencies]` and imports concrete RPG types (`HealthComponent`, `StatsComponent`, `EntityDiedEvent`, `CooldownTracker`, `AbilityDefinition`, `AbilityTickSystem`, `ScriptHotReloader`, `check_encumbrance`, `resolve_boundary_sphere_aoe`) across `sim.rs`, `network.rs`, `ability_dispatcher.rs`, `combat_pipeline.rs`, and `aoi.rs`.
   - `TickSimulation::with_kinematics` (`sim.rs:104, 109`) hardcodes `mmo_gameplay::CooldownTracker` and `mmo_gameplay::AbilityTickSystem`.
   - `process_client_move_with_seq` (`sim.rs:1011-1020`) and `process_client_move_deterministic` (`sim.rs:1129-1138`) hardcode `mmo_gameplay::check_encumbrance` and `HealthComponent::is_dead` instead of using core state flags and `Event::BeforeMove` hooks.
   - `AoiManager::extract_entity_attributes` (`aoi.rs:305-348`) falls back to `mmo_gameplay::combat::HealthComponent`.

2. **String-Only Attribute Lookups vs Compact Indexed `AttributeId` (Clash 5)**:
   - `AttributeSet` (`crates/core/src/attribute.rs`) only indexes attributes by `HashMap<String, AttributeValue>`, forcing string allocations and linear sorting in `AoiManager::extract_entity_attributes` while wire codecs use `u16` attribute IDs (`ScalarAttributeDelta`).

3. **Genre Leakage in `crates/core` and `crates/schema`**:
   - `crates/core/src/script.rs` lacks a `ScriptReloadable` trait, forcing `TickSimulation` to store `Option<Arc<mmo_gameplay::ScriptHotReloader>>`.
   - `crates/core/src/container.rs:40-45` uses `ContainerOwner::Guild` instead of a generalized `Group`/`Organization` alias.
   - `crates/schema/src/client.rs:224-257` restricts `InteractionType` to 5 hardcoded RPG verbs (`Talk`, `Attack`, `UseItem`, `Loot`, `Trade`), rejecting any `u8 > 4` at the wire decoder level instead of supporting extensible `Custom(u8)` interaction verbs.

## Proposed Solution

1. **Move Generalized Action & Cooldown Primitives to `crates/core` and Sever `mmo-gameplay` from `mmo-node`**:
   - Move `CooldownTracker`, `ActionCooldownSystem` (formerly `AbilityTickSystem`), and a generalized `ActionDefinition` + `resolve_boundary_sphere_targets` into `crates/core/src/action.rs` (re-exporting them in `mmo-gameplay` for backward compatibility).
   - Add `pub trait ScriptReloadable: Send + Sync` to `crates/core/src/script.rs`, implement it for `mmo_gameplay::ScriptHotReloader`, and store `Option<Arc<dyn ScriptReloadable>>` in `TickSimulation`.
   - Add `pub state_flags: u32` (with bit constants `state_flags::INCAPACITATED = 1 << 0`, `state_flags::MOVEMENT_LOCKED = 1 << 1`) to `mmo_core::Entity` and check `entity.state_flags` + `Event::BeforeMove` hooks in `TickSimulation` instead of importing `mmo_gameplay::HealthComponent` or `check_encumbrance`.
   - Refactor `crates/node/src/combat_pipeline.rs` and `ability_dispatcher.rs` to operate on `mmo_core::AttributeSet`, `mmo_core::Entity::state_flags`, and `mmo_core::action::CooldownTracker`, and remove `mmo-gameplay` from `crates/node/Cargo.toml` `[dependencies]` (keeping it only in `[dev-dependencies]` for integration tests).

2. **Fast `AttributeId(u8)` Indexed Access in `AttributeSet` (`crates/core/src/attribute.rs`)**:
   - Introduce `AttributeId(pub u8)` with well-known constants (`PRIMARY_VITALITY = 1`, `MAX_PRIMARY_VITALITY = 2`, `SECONDARY_RESOURCE = 3`, `MAX_SECONDARY_RESOURCE = 4`, `MOVE_SPEED_SCALE = 5`, `MITIGATION = 6`) and deterministic ID mapping on `AttributeSet` (`get_by_id`, `set_current_by_id`, `iter_indexed`) so `AoiManager::extract_entity_attributes` extracts `ScalarAttributeDelta`s without `mmo-gameplay` imports.

3. **Extensible `InteractionType::Custom(u8)` in `crates/schema/src/client.rs`**:
   - Add `Custom(u8)` to `InteractionType` (`crates/schema/src/client.rs:224-257`) so any `u8` interaction code (`5..=255`) roundtrips losslessly on the wire for non-RPG genres (space mining, vehicle boarding, hacking, building).

## Scope & Non-Goals
- **In Scope**: `crates/core/src/{action.rs, attribute.rs, entity.rs, script.rs, container.rs}`, `crates/schema/src/client.rs`, `crates/node/Cargo.toml`, `crates/node/src/{sim.rs, aoi.rs, combat_pipeline.rs, ability_dispatcher.rs, network.rs}`, and `crates/gameplay/src/{ability.rs, combat.rs, inventory.rs}`.
- **Non-Goals**: Removing `mmo-gameplay` plugins themselves (they remain first-class reference plugins in `crates/gameplay`).
