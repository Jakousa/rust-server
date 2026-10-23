# Technical Design: Live PostgreSQL Service Container and Dual-Dialect CI Verification

## 1. Dual-Dialect Test Harness (`crates/storage/src/test_helpers.rs`)

Add an async test helper in `crates/storage/src/test_helpers.rs`:
```rust
use crate::db::{Database, StorageError};

/// Connects to an in-memory SQLite database and, if `TEST_POSTGRES_URL` is configured,
/// connects to PostgreSQL, runs migrations, and truncates all tables for a clean test run.
///
/// If `REQUIRE_POSTGRES_TEST` is set to `"1"` or `"true"`, panics if `TEST_POSTGRES_URL` is missing.
pub async fn connect_test_databases() -> Result<Vec<(&'static str, Database)>, StorageError> {
    let sqlite_db = Database::connect_memory().await?;
    let mut dbs = vec![("sqlite", sqlite_db)];

    let require_pg = std::env::var("REQUIRE_POSTGRES_TEST")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    match std::env::var("TEST_POSTGRES_URL") {
        Ok(url) if !url.trim().is_empty() => {
            let pg_db = Database::connect(&url).await?;
            pg_db.migrate().await?;
            reset_postgres_tables(&pg_db).await?;
            dbs.push(("postgres", pg_db));
        }
        _ if require_pg => {
            panic!("REQUIRE_POSTGRES_TEST=1 is set, but TEST_POSTGRES_URL is missing or empty");
        }
        _ => {}
    }

    Ok(dbs)
}
```
Where `reset_postgres_tables(db: &Database)` executes:
```sql
TRUNCATE TABLE
    marketplace_listings,
    escrow_locks,
    container_transfer_Smallest, -- actual table names from db.rs migrations:
    transfer_logs,
    container_items,
    containers,
    character_items,
    characters,
    entity_snapshots,
    world_objects,
    terrain_chunks,
    kv_documents
RESTART IDENTITY CASCADE;
```
*(Verify exact table names against `crates/storage/src/db.rs` migration DDL statements and use individual `DELETE FROM <table>` statements inside a single transaction or `TRUNCATE` across the exact migrated tables so foreign keys are respected).*

---

## 2. Dual-Dialect Integration Suite (`crates/storage/tests/postgres_dialect_integration_test.rs`)

Create `crates/storage/tests/postgres_dialect_integration_test.rs` with a single sequential test entrypoint `#[tokio::test] async fn test_dual_dialect_storage_end_to_end()` (using a single test function avoids concurrent table truncation races across parallel nextest processes on the shared `mmo_test` database):
1. Iterate `(dialect_name, db)` in `connect_test_databases().await.unwrap()`.
2. **Snapshots & Characters**:
   - Save and load `EntitySnapshot` via `StorageRepository`.
   - Create `CharacterRecord` and `ItemRecord` via `CharacterRepository`.
3. **Containers & 2PC Transfers**:
   - Create source and destination containers via `ContainerRepository`, populate items, execute `TransactionCoordinator::execute_transfer`, and verify item ownership/container movement and `abort_transfer`.
4. **Escrow Trading & Marketplace**:
   - Execute an atomic player-to-player swap via `EscrowCoordinator`.
   - Create, query, purchase, and cancel listings via `MarketplaceRepository`.
5. **World Objects & Terrain Chunks**:
   - Exercise `WorldRepository::save_world_object`, `save_world_objects_batch` (which hits `bind_upsert_postgres` on Postgres), `load_sector_objects`, `delete_world_object`, and `save_terrain_chunk` / `load_terrain_chunk`.
6. **Schemaless Document Store**:
   - Exercise `DocumentStore::put_document`, `get_document`, `cas_document`, and `delete_document`.

---

## 3. GitHub Actions Workflow Update (`.github/workflows/rust.yml`)

In `.github/workflows/rust.yml` under `jobs.test`:
```yaml
    services:
      postgres:
        image: postgres:16-alpine
        env:
          POSTGRES_USER: mmo
          POSTGRES_PASSWORD: mmo_test_password
          POSTGRES_DB: mmo_test
        ports:
          - 5432:5432
        options: >-
          --health-cmd pg_isready
          --health-interval 5s
          --health-timeout 5s
          --health-retries 10
```
And on the `Run Tests with Code Coverage (cargo-llvm-cov)` step:
```yaml
      - name: Run Tests with Code Coverage (cargo-llvm-cov)
        env:
          TEST_POSTGRES_URL: postgres://mmo:mmo_test_password@localhost:5432/mmo_test
          REQUIRE_POSTGRES_TEST: "1"
        run: cargo llvm-cov nextest --workspace --all-targets --ignore-filename-regex 'main\.rs' --fail-under-lines 85 --lcov --output-path lcov.info
```
