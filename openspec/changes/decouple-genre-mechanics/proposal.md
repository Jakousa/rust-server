# Proposal: Decouple Remaining Genre-Specific RPG Mechanics from Core Engine Crates

## Problem Statement
Per [`AGENTS.md`](AGENTS.md) Section 1 (*Prime Directive: Unopinionated, Genre-Agnostic MMO Engine*), core engine crates (`crates/core`, `crates/node`, `crates/schema`, `crates/storage`, `crates/gateway`, and `crates/director`) must remain strictly genre-agnostic, unopinionated, and mathematically generalized. All genre-, fantasy-, and design-specific mechanics belong in `crates/gameplay` via `GameplayPlugin` or dedicated modular extension crates (`crates/plugins/*`).

A comprehensive codebase audit (tracked in issue #156) identified four areas of residual coupling:
1. **Hardcoded RPG Combat & Armor Formulas in `crates/node`**: `combat_pipeline.rs` computes a fantasy armor mitigation curve (`armor / (armor + 100.0)` capped at 0.90) and assumes `"health"`, `"max_health"`, and `"armor"` strings. `aoi.rs` hardcodes default 100 values for health in generic replication.
2. **Coupled Snapshot Persistence in `crates/storage`**: `save_snapshot_batch` unconditionally executes `UPDATE characters ...` for every entity snapshot, tying generalized entity state persistence to the RPG `characters` table.
3. **Duplicated Math Types & Hardcoded Genre Attributes**: `mmo-core::spatial::Vec3` and `mmo-schema::client::Vec3` duplicate 3D vector implementations. `mmo-core::attribute` hardcodes fantasy/space-sim names ("mana", "hull", "capacitor", "stamina").
4. **Hardcoded Gateway Interception**: `crates/gateway` embeds marketplace packet handling directly into its edge intercept handler rather than using a generic interceptor interface.

## Scope of Changes
1. **Unify `Vec3` across `mmo-schema` and `mmo-core`**:
   - Enhance `mmo-schema::client::Vec3` with `dot`, `Serialize`, `Deserialize`, and `From`/`Into` `(f32, f32, f32)`.
   - Have `mmo-core` depend on `mmo-schema` (`default-features = false`) and re-export `Vec3` in `mmo_core::spatial`.
2. **Pluggable Attribute Name Registration**:
   - Provide an extensible `AttributeRegistry` in `mmo-core` so plugins (like `CombatPlugin`) register attribute aliases ("health", "mana", "armor", "hull"), while `AttributeId` constants (`PRIMARY_VITALITY`, `SECONDARY_RESOURCE`, `MITIGATION`) represent generalized mathematical substrates.
3. **Pluggable Combat Resolution in `mmo-node`**:
   - Define a `CombatResolver` trait in `mmo-node` (with a mathematically neutral `DefaultCombatResolver` applying raw vitality deltas).
   - Move fantasy armor mitigation curves and RPG health rules into `RpgCombatResolver` in `crates/gameplay::combat`, registered when `CombatPlugin` is installed into `TickSimulation`.
4. **Configurable AoI Attribute Extraction**:
   - Allow `AoiManager` to delegate attribute extraction to an `AoiAttributeExtractor` hook/strategy, with a neutral default reading primary vitality from `AttributeSet` without hardcoded fallback values.
5. **Decoupled Snapshot Persistence in `mmo-storage`**:
   - Remove unconditional `UPDATE characters` query from `save_snapshot_batch`, ensuring `entity_snapshots` functions strictly as a genre-agnostic substrate.
6. **Generic Edge Packet Interception in `mmo-gateway`**:
   - Define `EdgePacketInterceptor` trait in `mmo-gateway`.
   - Decouple `GatewayInterceptHandler` to dispatch to registered interceptors, keeping edge routing unopinionated.

## Non-Goals
- Altering the client binary wire protocol opcodes or breaking FlatBuffers packet schema backwards compatibility.
- Removing RPG mechanics from `crates/gameplay` (gameplay plugins are specifically intended to host genre-specific rules).
