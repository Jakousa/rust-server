# Implementation Tasks: Expand Nightly Mutation Testing Matrix (Issue #130)

- [ ] 1. Kill Surviving Mutants in `crates/core/src/collision.rs`
  - [ ] 1.1 Run `cargo mutants --package mmo-core --file crates/core/src/collision.rs --jobs 2 -- --lib collision` (or inspect boundary conditions) and add unit tests in `crates/core/src/collision.rs` so zero mutants survive.

- [ ] 2. Kill Surviving Mutants in `crates/node/src/kinematics.rs`
  - [ ] 2.1 Run `cargo mutants --package mmo-node --file crates/node/src/kinematics.rs --jobs 2 -- --lib kinematics` and add boundary unit tests in `crates/node/src/kinematics.rs` so zero mutants survive.

- [ ] 3. Kill Surviving Mutants in `crates/gameplay/src/trade.rs`
  - [ ] 3.1 Run `cargo mutants --package mmo-gameplay --file crates/gameplay/src/trade.rs --jobs 2 -- --lib trade` and add boundary unit tests in `crates/gameplay/src/trade.rs` so zero mutants survive.

- [ ] 4. Update Nightly Mutation Workflow (`.github/workflows/mutation-tests.yml`)
  - [ ] 4.1 Expand the matrix in `.github/workflows/mutation-tests.yml` to include `mmo-core-collision`, `mmo-node-kinematics`, and `mmo-gameplay-trade` alongside `mmo-schema` and `mmo-storage`.

- [ ] 5. Workspace Quality Gates
  - [ ] 5.1 Run `cargo fmt --all -- --check`
  - [ ] 5.2 Run `cargo clippy --workspace --all-targets -- -D warnings`
  - [ ] 5.3 Run `cargo test --workspace`
