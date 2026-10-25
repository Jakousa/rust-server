# Implementation Tasks: Expand Nightly Mutation Testing Matrix (Issue #130)

- [x] 1. Kill Surviving Mutants in `crates/core/src/collision.rs`
  - [x] 1.1 Run `cargo mutants --package mmo-core --file crates/core/src/collision.rs --jobs 2 -- --lib collision` (or inspect boundary conditions) and add unit tests in `crates/core/src/collision.rs` so zero mutants survive.

- [x] 2. Kill Surviving Mutants in `crates/node/src/kinematics.rs`
  - [x] 2.1 Run `cargo mutants --package mmo-node --file crates/node/src/kinematics.rs --jobs 2 -- --lib kinematics` and add boundary unit tests in `crates/node/src/kinematics.rs` so zero mutants survive.

- [x] 3. Kill Surviving Mutants in `crates/gameplay/src/trade.rs`
  - [x] 3.1 Run `cargo mutants --package mmo-gameplay --file crates/gameplay/src/trade.rs --jobs 2 -- --lib trade` and add boundary unit tests in `crates/gameplay/src/trade.rs` so zero mutants survive.

- [x] 4. Update Nightly Mutation Workflow (`.github/workflows/mutation-tests.yml`)
  - [x] 4.1 Expand the matrix in `.github/workflows/mutation-tests.yml` to include `mmo-core-collision`, `mmo-node-kinematics`, and `mmo-gameplay-trade` alongside `mmo-schema` and `mmo-storage`.

- [x] 5. Workspace Quality Gates
  - [x] 5.1 Run `cargo fmt --all -- --check`
  - [x] 5.2 Run `cargo clippy --workspace --all-targets -- -D warnings`
  - [x] 5.3 Run `cargo test --workspace`
