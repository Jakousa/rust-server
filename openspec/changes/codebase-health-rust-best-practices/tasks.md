# Tasks: Codebase Health & Idiomatic Rust Best Practices Refactoring

## Phase 1: Domain-Driven Typed Error Enums
- [x] 1.1 In `crates/node/src/network.rs`, introduce `MigrationError` enum with `thiserror`.
- [x] 1.2 Update `execute_entity_migration` and `execute_batch_entity_migration` signatures and call sites to return `Result<_, MigrationError>`.
- [x] 1.3 In `crates/node/src/snapshotter.rs`, update `restore_entity_components` to return `Result<usize, CodecError>` from `codecs.deserialize_into`.
- [x] 1.4 In `crates/core/src/plugin.rs`, define `CodecError` and `PacketHandlerError` with `thiserror`, updating `ComponentCodec` and `ModularPacketHandler`.
- [x] 1.5 Update `crates/gameplay/src/inventory.rs` and other plugin implementations to return `Result<Option<Vec<u8>>, PacketHandlerError>`, mapping domain errors cleanly to `PacketHandlerError::MalformedPayload` / `Custom`.

## Phase 2: Defensive Ingestion & Panic Elimination
- [x] 2.1 In `crates/node/src/network.rs`, replace unsafe `.unwrap()` in `migrate_batch_entities` with safe match pattern returning explicit error responses on missing position.
- [x] 2.2 In `crates/node/src/main.rs`, replace `.unwrap()` in `duration_since(UNIX_EPOCH)` with safe `unwrap_or(0)` fallback.
- [x] 2.3 In `crates/node/src/snapshotter.rs`, replace `.unwrap()` in `capture_entity_snapshot` with safe `unwrap_or(0)` fallback.

## Phase 3: Ergonomic Parameter Structs
- [x] 3.1 In `crates/director/src/coordinator.rs`, define `NodeSmoothingContext` struct.
- [x] 3.2 Refactor `smooth_adjacent_boundaries` to accept `primary: NodeSmoothingContext` and `neighbor: NodeSmoothingContext`, removing `#[allow(clippy::too_many_arguments)]`.
- [x] 3.3 Update unit tests in `crates/director/src/coordinator.rs` to construct `NodeSmoothingContext`.

## Phase 4: Clippy & Modern Rust Idioms
- [x] 4.1 Inlined format arguments across daemon binaries (`crates/node/src/main.rs`, `crates/director/src/main.rs`, `crates/gateway/src/main.rs`).
- [x] 4.2 Use `u32::from(...)` for lossless conversions instead of `as u32`.

## Phase 5: Verification & Quality Gates
- [x] 5.1 Run `cargo test --workspace` to ensure all existing test suites and edge cases pass with typed errors.
- [x] 5.2 Run `cargo clippy --workspace --all-targets -- -D warnings` ensuring 0 warnings.
- [x] 5.3 Run `cargo fmt --all -- --check`.
- [x] 5.4 Commit refactoring to `feat/codebase-health-rust-best-practices`.

