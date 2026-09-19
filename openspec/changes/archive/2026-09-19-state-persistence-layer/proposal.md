## Why

Currently, all entity data (positions, components, sessions) resides purely in volatile simulation worker memory; server restarts or node crashes cause state loss. We need a dual-tier persistence architecture consisting of hot in-memory session caching (for fast entity migration and gateway presence) and durable database storage (for player accounts, character stats, and periodic asynchronous snapshotting).

## What Changes

- Create `crates/storage`: a dedicated storage crate providing database persistence abstractions and session caching.
- Implement durable relational persistence using `sqlx` (SQLite for local/test and PostgreSQL compatible for production) with migration scripts for `accounts`, `characters`, `entity_snapshots`, and `inventories`.
- Implement asynchronous write-behind snapshotter on `crates/node` that commits dirty entity components to the storage tier at configurable intervals (e.g., every 30 seconds or on graceful logout).
- Implement hot session caching interface (Redis client with in-memory fallback for testing) for fast node handover state transfer and gateway routing cache.
- Update `docker-compose.yml` with optional `postgres` and `redis` service dependencies.

## Capabilities

### New Capabilities
- `storage/persistence`: Manages durable entity and character state persistence using relational storage, asynchronous snapshotting, and transaction-safe schema migrations.
- `storage/session-cache`: Manages low-latency distributed session presence and hot entity state handoffs between simulation nodes.

### Modified Capabilities
<!-- None: core server specifications remain normative and unchanged. -->

## Impact

- New workspace crate: `crates/storage`.
- Integration into `crates/node` (snapshotting lifecycle hooks) and `crates/gateway` (presence & session validation).
- New dependencies: `sqlx` (with runtime-tokio-rustls, sqlite/postgres), `redis`, `serde`, `serde_json`.
- `docker-compose.yml` additions for database and cache services.
