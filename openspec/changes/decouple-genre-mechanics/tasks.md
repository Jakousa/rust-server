# Implementation Tasks: Decouple Remaining Genre-Specific RPG Mechanics

## Task 1: Unify Vec3 and Pluggable Attribute Registry
- [x] 1.1 In `crates/schema/src/client.rs`, derive `Serialize, Deserialize` on `Vec3`, and implement `dot(&self, other: &Self) -> f32`, `From<(f32, f32, f32)>`, and `From<Vec3> for (f32, f32, f32)`.
- [x] 1.2 In `crates/schema/Cargo.toml`, add `serde = { version = "1.0", features = ["derive"] }`.
- [x] 1.3 In `crates/core/Cargo.toml`, add `mmo-schema = { path = "../schema", default-features = false }`.
- [x] 1.4 In `crates/core/src/spatial.rs`, re-export `mmo_schema::client::Vec3` as `pub use mmo_schema::client::Vec3;`.
- [x] 1.5 In `crates/core/src/attribute.rs`, implement `AttributeRegistry` with standard mappings and dynamic alias registration, updating `canonical_id_for_name`.
- [x] 1.6 Verify with `cargo test -p mmo-schema && cargo test -p mmo-core`.

## Task 2: Decouple Snapshot Batch Persistence in mmo-storage
- [x] 2.1 In `crates/storage/src/repository.rs`, remove the `char_query` (`UPDATE characters ...`) execution from `save_snapshot_batch`.
- [x] 2.2 Add unit test verifying that `save_snapshot_batch` succeeds for entities not present in the `characters` table.
- [x] 2.3 Update `crates/node/tests/it/persistence.rs` to reflect that `save_snapshot_batch` persists to `entity_snapshots` only and character position is persisted via character updates.
- [x] 2.4 Verify with `cargo test -p mmo-storage`.

## Task 3: Pluggable Combat Resolution in mmo-core, mmo-node, and mmo-gameplay
- [x] 3.1 In `crates/core/src/combat.rs`, define `CombatEffectOutcome` and `CombatResolver` trait, add `combat_resolver` slot to `PluginContext` in `crates/core/src/plugin.rs`, and export from `crates/core`.
- [x] 3.2 In `crates/node/src/combat_pipeline.rs`, implement `DefaultCombatResolver` and update `apply_combat_effect` to delegate to a resolver.
- [x] 3.3 In `crates/node/src/sim.rs`, add `combat_resolver: Arc<dyn CombatResolver>` to `TickSimulation`, provide `set_combat_resolver`, and adopt `ctx.combat_resolver` from `PluginContext` when building plugins.
- [x] 3.4 In `crates/gameplay/src/combat.rs`, implement `RpgCombatResolver` and have `CombatPlugin::build` register it via `ctx.combat_resolver`.
- [x] 3.5 In `crates/node/src/network/service.rs`, update `execute_remote_ability` to delegate to `sim.combat_resolver`.
- [x] 3.6 Verify with `cargo test -p mmo-node && cargo test -p mmo-gameplay`.

## Task 4: Configurable AoI Attribute Extractor in mmo-node
- [x] 4.1 In `crates/node/src/aoi.rs`, define `AoiAttributeExtractor` trait and `DefaultAoiAttributeExtractor`.
- [x] 4.2 In `crates/node/src/aoi.rs`, update `AoiManager` to use the extractor with setter `set_attribute_extractor`.
- [x] 4.3 Verify with `cargo test -p mmo-node --test aoi_tests || cargo test -p mmo-node`.

## Task 5: Generic Edge Packet Interceptor in mmo-gateway
- [x] 5.1 In `crates/gateway/src/server/intercept.rs`, define `EdgePacketInterceptor` trait and `EdgePacketSender` trait.
- [x] 5.2 Implement `EdgePacketInterceptor` for `GatewayMarketplaceService`, ensuring unauthenticated packets receive unauthorized responses and unsolicited `MarketResponse` packets are dropped.
- [x] 5.3 In `crates/gateway/src/server/mod.rs`, add `interceptors: Vec<Arc<dyn EdgePacketInterceptor>>`, `with_interceptor` builder method, and retain `with_marketplace` as a convenience delegator to `with_interceptor`.
- [x] 5.4 Update `GatewayInterceptHandler` to dispatch matching packets through registered `EdgePacketInterceptor`s.
- [x] 5.5 Verify with `cargo test -p mmo-gateway`.

## Task 6: Full Workspace Verification & Quality Gates
- [x] 6.1 Run formatting check: `cargo fmt --all -- --check`.
- [x] 6.2 Run strict linter: `cargo clippy --workspace --all-targets -- -D warnings`.
- [x] 6.3 Run full test suite: `cargo test --workspace`.
