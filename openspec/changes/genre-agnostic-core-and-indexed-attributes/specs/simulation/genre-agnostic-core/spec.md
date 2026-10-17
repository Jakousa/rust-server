# Genre-Agnostic Core Decoupling and Indexed Attribute Specification

## Purpose
Enforces the `AGENTS.md` Prime Directive by removing all compile-time dependencies from `mmo-node` to `mmo-gameplay`, introducing `AttributeId(u8)` indexed attribute access in `mmo-core`, and allowing extensible interaction codes in `mmo-schema`.

## Requirements

### Requirement: Zero `mmo-gameplay` Library Dependency in `mmo-node`
The `mmo-node` library crate SHALL NOT depend on `mmo-gameplay` in `[dependencies]`. All simulation tick processing, movement validation, AoI attribute extraction, cross-partition action dispatch, and WASM script hot-reloading in `mmo-node` SHALL operate exclusively on generalized `mmo-core` and `mmo-schema` primitives.

#### Scenario: Compiling mmo-node without mmo-gameplay
- **GIVEN** `crates/node/Cargo.toml` with `mmo-gameplay` removed from `[dependencies]`
- **WHEN** `cargo check -p mmo-node --lib` is executed
- **THEN** `mmo-node` SHALL compile with zero errors and zero warnings

### Requirement: Deterministic `AttributeId(u8)` Indexing in `AttributeSet`
`AttributeSet` SHALL map attributes to compact `AttributeId(u8)` identifiers (with canonical slots `1 = PrimaryVitality`, `2 = MaxPrimaryVitality`, `3 = SecondaryResource`, `4 = MaxSecondaryResource`, `5 = MoveSpeedScale`, `6 = Mitigation`, and `16..=255` for custom attributes) and support $O(1)$ ID-based lookup and delta extraction.

#### Scenario: AoI attribute extraction from generic AttributeSet
- **GIVEN** an entity with `AttributeSet` containing `"health"` (`75.0`/`100.0`) and `"energy"` (`40.0`/`50.0`) and no `mmo-gameplay` components
- **WHEN** `AoiManager::extract_entity_attributes` extracts wire deltas for the entity
- **THEN** it SHALL populate `health = 75`, `max_health = 100`, and emit deterministic `ScalarAttributeDelta` records using their `AttributeId`s

### Requirement: Extensible Wire `InteractionType`
`InteractionType::from_u8` and `InteractionType::as_u8` in `crates/schema/src/client.rs` SHALL support `InteractionType::Custom(u8)` for any interaction verb ID in `5..=255` without returning a wire decode error.

#### Scenario: Roundtrip encoding of custom genre interaction verb
- **GIVEN** an `InteractionPacket` with `interaction_type = InteractionType::Custom(42)` (e.g., warp drive activation or block excavation)
- **WHEN** the packet is encoded and decoded via `ClientPacket`
- **THEN** decoding SHALL succeed and yield `InteractionType::Custom(42)`
