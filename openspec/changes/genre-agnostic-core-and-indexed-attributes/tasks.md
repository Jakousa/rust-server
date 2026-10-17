# Implementation Tasks: Genre-Agnostic Core Decoupling & Indexed Attributes (Issue #96)

- [ ] 1. Add `AttributeId(u8)`, `state_flags`, `ScriptReloadable`, and Generalized Action Primitives to `crates/core`
  - [ ] 1.1 Add `pub state_flags: u32` and `state_flags` constants (`INCAPACITATED`, `MOVEMENT_LOCKED`, `COMBAT_ACTIVE`, `INVULNERABLE`) to `Entity` in `crates/core/src/entity.rs`, and `EntityIncapacitatedEvent` to `crates/core/src/plugin.rs`.
  - [ ] 1.2 Add `AttributeId(pub u8)` and indexed attribute lookup helpers (`canonical_id_for_name`, `get_by_id`, `iter_indexed`) to `AttributeSet` in `crates/core/src/attribute.rs`.
  - [ ] 1.3 Move `CooldownTracker`, `ActionCooldownSystem`, `ActionDefinition`, and `resolve_boundary_sphere_aoe` to `crates/core/src/action.rs` and re-export them from `crates/gameplay/src/ability.rs`.
  - [ ] 1.4 Add `pub trait ScriptReloadable: Send + Sync` to `crates/core/src/script.rs` and implement it for `mmo_gameplay::ScriptHotReloader`.

- [ ] 2. Remove `mmo-gameplay` Dependency from `crates/node` Library
  - [ ] 2.1 Refactor `crates/node/src/aoi.rs` (`extract_entity_attributes`) to read from `AttributeSet`, `state_flags`, and `properties` without importing `mmo_gameplay::HealthComponent`.
  - [ ] 2.2 Refactor `crates/node/src/combat_pipeline.rs` and `crates/node/src/ability_dispatcher.rs` to use `mmo_core::AttributeSet`, `Entity::state_flags`, and `mmo_core::action::{CooldownTracker, ActionDefinition, resolve_boundary_sphere_aoe}`.
  - [ ] 2.3 Refactor `crates/node/src/sim.rs` to use `Arc<dyn mmo_core::script::ScriptReloadable>`, `mmo_core::action::{CooldownTracker, ActionCooldownSystem}`, `Entity::is_incapacitated()`, and `Event::BeforeMove` hooks.
  - [ ] 2.4 Move `mmo-gameplay` from `[dependencies]` to `[dev-dependencies]` in `crates/node/Cargo.toml` and verify `cargo check -p mmo-node --lib`.

- [ ] 3. Generalize `InteractionType` in `crates/schema` & `ContainerOwner` in `crates/core`
  - [ ] 3.1 Add `Custom(u8)` variant to `InteractionType` in `crates/schema/src/client.rs` and add roundtrip unit tests for custom interaction codes `5..=255`.
  - [ ] 3.2 Add `Group(u64)` alias/support to `ContainerOwner` in `crates/core/src/container.rs`.

- [ ] 4. Workspace Quality Gates
  - [ ] 4.1 Run `cargo fmt --all -- --check`
  - [ ] 4.2 Run `cargo clippy --workspace --all-targets -- -D warnings`
  - [ ] 4.3 Run `cargo test --workspace`
