# Implementation Tasks: Track 3 Production Distributed Storage

## Task List

- [x] 1. **Storage Crate Dependencies & Multi-Backend Abstraction**
  - [x] 1.1 Update `crates/storage/Cargo.toml` with `postgres` feature on `sqlx` (`runtime-tokio-rustls`, `sqlite`, `postgres`, `macros`).
  - [x] 1.2 Refactor `crates/storage/src/db.rs` to introduce `DbBackend` enum (`Sqlite(sqlx::SqlitePool)`, `Postgres(sqlx::PgPool)`).
  - [x] 1.3 Implement query placeholder translation utility `sql_for_dialect(query, backend)` converting `?` to `$1, $2, ...` for PostgreSQL.
  - [x] 1.4 Implement `Database::connect(url)` protocol dispatcher detecting `postgres://`, `postgresql://`, and `sqlite:`.
  - [x] 1.5 Implement `Database::ping(&self)` health check probe for connection pool liveness.
  - [x] 1.6 Verify backward compatibility with existing SQLite tests via `cargo test -p mmo-storage`.

- [x] 2. **Relational Schema Migrations & Data Models**
  - [x] 2.1 Update `crates/storage/src/models.rs` with `InventoryRecord`, `ItemRecord`, `TradeRecord`, and `TradeStatus`.
  - [x] 2.2 Implement cross-dialect migrations in `crates/storage/src/db.rs` setting up `accounts`, `characters`, `entity_snapshots`, `inventories`, `items`, and `trades`.
  - [x] 2.3 Add unit tests verifying schema creation, primary keys, and foreign keys on SQLite.

- [x] 3. **Inventory & Deadlock-Free ACID Trading Repository**
  - [x] 3.1 Implement inventory management methods on `CharacterRepository` / `InventoryRepository`: `get_inventory`, `set_inventory_capacity`, `add_item`, `remove_item`, `get_item`.
  - [x] 3.2 Implement `execute_trade` protocol enforcing sorted GUID lock acquisition (`min(guid_a, guid_b)` then `max(guid_a, guid_b)`), dialect-specific locking (`FOR UPDATE` for Postgres, `BEGIN IMMEDIATE` for SQLite), free slot validation, and trade audit logging.
  - [x] 3.3 Optimize `save_snapshot_batch` to use batched statement generation, avoiding sequential single-row round-trips over the network.
  - [x] 3.4 Add unit tests for inventory operations, slot collision prevention, and full rollback when recipient lacks free slots.
  - [x] 3.5 Add concurrency race condition test simulating simultaneous opposite trades (`A -> B` and `B -> A`) to verify deadlock freedom.

- [x] 4. **Kubernetes PostgreSQL Manifests & Deployment Configuration**
  - [x] 4.1 Create `k8s/base/postgres.yaml` with Deployment, Service, Secret, and Health Probes (`pg_isready`).
  - [x] 4.2 Update `k8s/base/kustomization.yaml` to include `postgres.yaml`.
  - [x] 4.3 Update `k8s/base/node-statefulset.yaml` and `k8s/base/gateway.yaml` with `DATABASE_URL` referencing PostgreSQL service.

- [x] 5. **Quality Gates & Cluster Deployment Verification**
  - [x] 5.1 Run `cargo clippy --workspace --all-targets -- -D warnings`.
  - [x] 5.2 Run `cargo test --workspace`.
  - [x] 5.3 Deploy manifests to local `k3d` cluster (`./scripts/cluster.sh deploy`) and verify PostgreSQL pod readiness.
  - [x] 5.4 Check node logs to ensure successful connection to distributed PostgreSQL.
  - [x] 5.5 Commit and push changes to `origin/main`.
