# Multi-Backend Storage Engine Specification

## Purpose
Specifies the unified database connection layer in `crates/storage`, capable of transparently connecting to either PostgreSQL or SQLite with health monitoring and connection pooling.

## Requirements

### Requirement: Protocol Auto-Detection and Connection Pooling
The `Database` abstraction MUST inspect the connection string scheme and initialize the appropriate SQLx connection pool:
- `postgres://` or `postgresql://`: Configures `sqlx::PgPool` with configurable connection bounds (min 2, max 20, acquire timeout 5s).
- `sqlite:` or `sqlite::memory:`: Configures `sqlx::SqlitePool` with WAL journal mode (for file-based SQLite) and busy timeout of 5 seconds.

#### Scenario: Connecting to PostgreSQL
- **GIVEN** a valid PostgreSQL connection URI `postgres://user:pass@localhost:5432/mmo_db`
- **WHEN** `Database::connect(url)` is invoked
- **THEN** it successfully connects to the PostgreSQL server, runs migrations, and returns a healthy `Database` instance.

#### Scenario: Connecting to In-Memory SQLite for Hermetic Testing
- **GIVEN** a call to `Database::connect_memory()`
- **WHEN** the pool is initialized
- **THEN** it connects to an in-memory SQLite database, runs migrations, and isolates tests without disk side effects.

### Requirement: Database Health Check (Liveness / Readiness)
The `Database` abstraction MUST provide a `ping()` method executing a lightweight query (`SELECT 1`) to verify connectivity.

#### Scenario: Database ping succeeds
- **GIVEN** an active, healthy database connection
- **WHEN** `db.ping().await` is called
- **THEN** it returns `Ok(())` within 500ms.

#### Scenario: Database ping fails on disconnected server
- **GIVEN** an unreachable or dropped database connection
- **WHEN** `db.ping().await` is called
- **THEN** it returns a `StorageError::Sqlx` error without panicking or hanging indefinitely.
