# Implementation Tasks: Direct Unit Test Coverage for Network, Gateway, and Scenario Submodules (Issue #131)

- [x] 1. `mmo-node` Network Submodule Unit Tests (`migration.rs`, `ghost.rs`, `client_pool.rs`)
  - [x] 1.1 Add unit tests to `crates/node/src/network/migration.rs` covering `ingest_migrated_entity` errors, ghost replacement, and single/batch lease rollback paths.
  - [x] 1.2 Add unit tests to `crates/node/src/network/ghost.rs` and `crates/node/src/network/client_pool.rs`.
  - [x] 1.3 Run `cargo test -p mmo-node --lib` to verify all unit tests pass.

- [x] 2. `mmo-gateway` Intercept Handler Unit Tests (`crates/gateway/src/server/intercept.rs`)
  - [x] 2.1 Add unit tests to `crates/gateway/src/server/intercept.rs` covering anti-spoofing, topic routing, session removal, and marketplace error fallbacks.
  - [x] 2.2 Run `cargo test -p mmo-gateway --lib` to verify all unit tests pass.

- [x] 3. `mmo-loadtest` Scenario Manifest Unit Tests (`crates/loadtest/src/scenario.rs`)
  - [x] 3.1 Add unit tests to `crates/loadtest/src/scenario.rs` verifying `ScenarioManifest` and `ScenarioStep` JSON parsing and validation.
  - [x] 3.2 Run `cargo test -p mmo-loadtest` to verify all unit tests pass.

- [x] 4. Workspace Quality Gates
  - [x] 4.1 Run `cargo fmt --all -- --check`
  - [x] 4.2 Run `cargo clippy --workspace --all-targets -- -D warnings`
  - [x] 4.3 Run `cargo test --workspace`
