# Implementation Tasks: Fix CI Paths Filter & Harden Mutation Testing

## Phase 1: CI Workflow Modernization
- [x] 1. Update `.github/workflows/rust.yml` to remove top-level `paths-ignore` and introduce `filter` job with `dorny/paths-filter@v3`.
- [x] 2. Update job condition guards in `rust.yml` (`lint`, `test`, `mutants`) to execute conditionally based on `needs.filter.outputs.code`.
- [x] 3. Update `verify` (`Lint, Test & Verify`) job to implement the airtight verification script (validating filter success, no cancelled/failed jobs, mandatory success when code is modified).

## Phase 2: Schema Test Hardening (Zero Surviving Mutants)
- [x] 4. Add unit test assertions in `crates/schema/src/client.rs` for `From<MovementCorrectionReason> for u8` across all variants (`KinematicViolation`, `Reconciliation`, `Veto`, `OutOfBounds`, `Custom`).
- [x] 5. Add exact buffer boundary unit tests in `crates/schema/src/client.rs` for `WorldObjectSpawnPacket::decode`, `WorldObjectDeltaPacket::decode`, `PlaceObjectPacket::decode`, and `AuthResponsePacket::decode`.
- [x] 6. Run `cargo mutants --package mmo-schema --file crates/schema/src/client.rs` locally to verify 0 missed mutants.

## Phase 3: Storage Isolation & Mutants Configuration
- [x] 7. Refactor unreachable PostgreSQL match arms in `transaction_coord.rs`, `escrow_coord.rs`, `marketplace.rs`, and `repository.rs` into private `*_postgres` helper methods.
- [x] 8. Update `.cargo/mutants.toml` with regex exclusions for `'.*_postgres\b'` and `'Database::ping'`.
- [x] 9. Add unit tests in `crates/storage/src/datastore.rs` for `DataDocument::get`, `get_path`, `LoadReport::is_ok`, and `DataStore::load_directory`.
- [x] 10. Add unit tests in `transaction_coord.rs`, `escrow_coord.rs`, and `marketplace.rs` for lease duration and slot capacity boundary conditions.
- [x] 11. Run `cargo mutants --package mmo-storage` on touched files/functions to verify 0 missed mutants.

## Phase 4: Verification & PR Creation
- [x] 12. Run workspace quality gates: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- [x] 13. Commit changes, push branch `feat/fix-ci-paths-filter-and-mutation-tests`, create PR, and enable auto-merge.
