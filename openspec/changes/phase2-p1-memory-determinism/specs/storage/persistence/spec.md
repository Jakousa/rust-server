# Delta Specification: storage/persistence

## MODIFIED Requirements

### Requirement: Asynchronous Entity State Snapshotting
The simulation node SHALL asynchronously persist dirty entity component states and coordinates to the database using a bounded in-memory queue and chunked batch commits without stalling the 20 Hz tick loop.

#### Scenario: Dirty entity write-behind snapshot
- **WHEN** the periodic snapshot interval elapses on a simulation node
- **THEN** the node SHALL dispatch dirty entity states to an asynchronous storage queue and commit them to the database in bounded batch chunks

#### Scenario: Bounded snapshot queue enforcement
- **WHEN** the volume of dirty entity snapshots reaches the configured maximum queue capacity during slow database flushes
- **THEN** the snapshotter SHALL coalesce updates for existing entities and reject or backpressure non-critical entries to prevent unbounded memory growth

### Requirement: Idempotent Schema Migration
The storage layer SHALL execute automated, idempotent database schema migrations establishing tables, primary keys, and indexes upon startup, configuring Write-Ahead Logging (WAL) mode for concurrent read/write stability.

#### Scenario: Database initialization
- **WHEN** the storage service connects to a new or existing SQLite database
- **THEN** it SHALL set `PRAGMA journal_mode=WAL` and `PRAGMA synchronous=NORMAL`, execute pending migrations, and verify table integrity before accepting queries
