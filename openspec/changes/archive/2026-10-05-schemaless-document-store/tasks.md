# Tasks: Schemaless Document Store for Data-Driven Assets

## Phase 1: Core Primitives (`crates/core`)
- [x] 1.1 Implement `PropertyValue` enum in `crates/core/src/property.rs` with `Null`, `Bool`, `Int`, `Float`, `String`, `Array`, `Object`, numeric coercion, and conversions (`From<serde_json::Value>`, primitives).
- [x] 1.2 Implement `DynamicProperties` struct with defensive dotted-path navigation (`get_path`), typed getters, merging, and JSON serialization.
- [x] 1.3 Integrate `DynamicProperties` field into `Entity` in `crates/core/src/entity.rs` with convenience property methods (`get_property`, `set_property`, etc.) and export in `crates/core/src/lib.rs`.
- [x] 1.4 Write comprehensive unit tests in `crates/core` testing scalar conversion, dotted lookup, property merging, and `Entity` integration.

## Phase 2: Schemaless Storage Engine (`crates/storage`)
- [x] 2.1 Update `crates/storage/Cargo.toml` with `rmp-serde = "1.3"`, `arc-swap = "1.7"`, and `mmo-core = { path = "../core" }`.
- [x] 2.2 Implement `DataDocument`, `DataStoreError`, and `LoadReport` in `crates/storage/src/datastore.rs`.
- [x] 2.3 Implement `DataStore` in-memory structure using `ArcSwap` wait-free snapshot storage and collection-isolated maps.
- [x] 2.4 Implement single-file format ingestion supporting case-insensitive `.json` and `.mpk`/`.msgpack`, multi-document arrays, and key-value maps.
- [x] 2.5 Implement recursive directory ingestion with collection inference and non-aborting error accumulation (`LoadReport`).
- [x] 2.6 Implement `DataStore::apply_to_entity` to attach document properties to an `mmo_core::Entity`.
- [x] 2.7 Export `DataStore`, `DataDocument`, `DataStoreError`, and `LoadReport` in `crates/storage/src/lib.rs` and write exhaustive unit tests for ingestion, query, error handling, and wait-free concurrency.

## Phase 3: Gameplay Decoupling & Content Verification (`crates/gameplay`)
- [x] 3.1 Adapt or extend `crates/gameplay` systems (combat/stats) to interpret dynamic properties from `entity.properties`.
- [x] 3.2 Add helper or integration support in `crates/gameplay` for spawning/configuring entities from `DataStore` document definitions.
- [x] 3.3 Create end-to-end integration tests proving:
  - Startup ingestion of JSON and MessagePack assets.
  - Entity property attachment from dynamic definitions.
  - Runtime balance/content updates without Rust backend recompilation.

## Phase 4: Quality Gates & Verification
- [x] 4.1 Run `cargo fmt --all -- --check`.
- [x] 4.2 Run `cargo clippy --workspace --all-targets -- -D warnings`.
- [x] 4.3 Run `cargo test --workspace`.
