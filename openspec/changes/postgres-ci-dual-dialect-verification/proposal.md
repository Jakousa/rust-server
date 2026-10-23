# Live PostgreSQL Service Container and Dual-Dialect CI Integration Verification Proposal

## Context & Problem Statement
Although `crates/storage` supports both SQLite (`sqlite::memory:`) and PostgreSQL (`postgres://`) via `Database`, `with_dialect_tx!`, and `Database::sql`, all automated tests in `.github/workflows/rust.yml` currently run exclusively against in-memory SQLite. As a result:
1. PostgreSQL-specific parameter binding (`$1, $2, ...`), `ON CONFLICT` upserts (`bind_upsert_postgres` in `crates/storage/src/world_repo.rs`), `JSONB` casting, and `FOR UPDATE` row-level locking semantics are not exercised against a live PostgreSQL instance during PR CI runs.
2. `crates/storage/src/db.rs` and `crates/storage/src/test_helpers.rs` lack a reusable dual-dialect test helper that connects to `TEST_POSTGRES_URL` when configured and isolates schema/table state per test.

## Proposed Solution
1. **Dual-Dialect Test Harness (`crates/storage/src/test_helpers.rs` & `crates/storage/src/db.rs`)**:
   - Add an async helper `connect_test_databases() -> Vec<(&'static str, Database)>` (or `connect_postgres_test_db() -> Option<Database>`) in `crates/storage/src/test_helpers.rs` that always returns an initialized `sqlite::memory:` `Database` and, when `std::env::var("TEST_POSTGRES_URL")` is set and non-empty, connects to PostgreSQL, runs migrations (`db.migrate().await`), and truncates test tables cleanly so integration tests can verify identical behavior across both dialects.
2. **PostgreSQL Dual-Dialect Integration Test Suite (`crates/storage/tests/postgres_dialect_integration_test.rs`)**:
   - Add an integration test suite exercising:
     - `CharacterRepository` & `StorageRepository` (`save_snapshot`, `load_snapshot`, `save_character`, `add_item`)
     - `ContainerRepository` & `TransactionCoordinator` (`execute_transfer` with `FOR UPDATE` locking and `abort_transfer`)
     - `EscrowCoordinator` & `MarketplaceRepository` (`execute_trade`, `create_listing`, `purchase_listing`, `cancel_listing`)
     - `WorldRepository` (`save_world_object`, `save_world_objects_batch` exercising `bind_upsert_postgres`, `save_terrain_chunk`, `load_sector_objects`)
     - `DocumentStore` (`put_document`, `cas_document`, `delete_document`)
     against every database returned by `connect_test_databases()`.
   - When `REQUIRE_POSTGRES_TEST=1` is set in the environment (as in CI), assert that `TEST_POSTGRES_URL` was present and tested so CI cannot silently skip the PostgreSQL dialect pass.
3. **GitHub Actions PostgreSQL Service Container (`.github/workflows/rust.yml`)**:
   - Add a `services.postgres` container (`postgres:16-alpine`, port `5432`, health-checked via `pg_isready`) to the `test` job in `.github/workflows/rust.yml` and pass `TEST_POSTGRES_URL: postgres://mmo:mmo_test_password@localhost:5432/mmo_test` and `REQUIRE_POSTGRES_TEST: "1"`.

## Scope & Non-Goals
- **In Scope**: `crates/storage/src/test_helpers.rs`, `crates/storage/tests/postgres_dialect_integration_test.rs`, and `.github/workflows/rust.yml`.
- **Non-Goals**: Changing production SQL schemas or removing hermetic `sqlite::memory:` default behavior when `TEST_POSTGRES_URL` is unset locally.
