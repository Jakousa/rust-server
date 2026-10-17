# Storage Dialect Deduplication and Helper Consolidation Specification

## Purpose
Consolidates duplicated SQLite and PostgreSQL transaction, escrow, marketplace, and repository implementations into unified, dialect-adapted logic so both SQL backends remain strictly in parity.

## Requirements

### Requirement: Unified Dialect-Adapted SQL Execution Across Storage Subsystems
`EscrowCoordinator`, `MarketplaceRepository`, `TransactionCoordinator`, and `StorageRepository` SHALL execute a single canonical business logic definition per operation, translating `?` parameter placeholders and `FOR UPDATE` row-locking clauses via `Database::sql` / `sql_for_dialect` rather than maintaining separate `_postgres` method copies.

#### Scenario: Parity between SQLite and PostgreSQL queries
- **GIVEN** any repository, escrow, marketplace, or 2PC transfer operation
- **WHEN** executed against either `DbBackend::Sqlite` or `DbBackend::Postgres`
- **THEN** both backends SHALL execute the same validation, slot allocation, and state transitions using dialect-translated SQL strings

### Requirement: Shared Time and Container Free-Slot Helpers
Time retrieval (`current_time_ms`, `current_time_secs`) and container free-slot calculation (`compute_free_slots`, `find_first_free_slot`) SHALL be defined once in `crates/storage` and reused across all storage modules without panicking `.unwrap()` calls on `SystemTime`.

#### Scenario: Computing free container slots
- **GIVEN** a container with capacity `5` and occupied slots `{0, 2, 3}`
- **WHEN** `compute_free_slots` and `find_first_free_slot` are called
- **THEN** `find_first_free_slot` SHALL return `Some(1)` and `compute_free_slots` SHALL return `vec![1, 4]`

### Requirement: Propagated Snapshot Persistence Errors
`StorageRepository::save_snapshot` and `StorageRepository::save_snapshot_batch` SHALL propagate SQL execution and transaction commit errors via `Result<(), StorageError>` rather than discarding them with `let _ =`.

#### Scenario: Snapshot batch commit error propagation
- **GIVEN** a database error during `save_snapshot` or `save_snapshot_batch`
- **WHEN** the SQL query or transaction commit fails
- **THEN** the method SHALL return `Err(StorageError::Database(...))` to the caller so the write-behind snapshotter retains dirty entries for retry
