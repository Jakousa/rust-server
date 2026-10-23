# Implementation Tasks: Live PostgreSQL Service Container and Dual-Dialect CI Verification (Issue #128)

- [x] 1. Dual-Dialect Storage Test Harness (`crates/storage/src/test_helpers.rs`)
  - [x] 1.1 Implement `connect_test_databases()` and table cleanup helper in `crates/storage/src/test_helpers.rs`, supporting both `sqlite::memory:` and `TEST_POSTGRES_URL` with `REQUIRE_POSTGRES_TEST` enforcement.
  - [x] 1.2 Add unit tests in `crates/storage/src/test_helpers.rs` verifying default SQLite fallback when `TEST_POSTGRES_URL` is unset.

- [x] 2. End-to-End Dual-Dialect Integration Test (`crates/storage/tests/postgres_dialect_integration_test.rs`)
  - [x] 2.1 Create `crates/storage/tests/postgres_dialect_integration_test.rs` exercising `StorageRepository`, `CharacterRepository`, `ContainerRepository`, `TransactionCoordinator`, `EscrowCoordinator`, `MarketplaceRepository`, `WorldRepository` (`save_world_objects_batch`), and `DocumentStore` across all dialects returned by `connect_test_databases()`.
  - [x] 2.2 Run `cargo test -p mmo-storage --test postgres_dialect_integration_test` to verify compilation and execution.

- [x] 3. GitHub Actions CI PostgreSQL Service Container (`.github/workflows/rust.yml`)
  - [x] 3.1 Add `services.postgres` (`postgres:16-alpine`) and `TEST_POSTGRES_URL` + `REQUIRE_POSTGRES_TEST: "1"` environment variables to the `test` job in `.github/workflows/rust.yml`.

- [x] 4. Workspace Quality Gates
  - [x] 4.1 Run `cargo fmt --all -- --check`
  - [x] 4.2 Run `cargo clippy --workspace --all-targets -- -D warnings`
  - [x] 4.3 Run `cargo test --workspace`
