## 1. Storage Crate & Relational Persistence (`crates/storage`)

- [x] 1.1 Initialize `crates/storage` in Cargo workspace with dependencies (`sqlx`, `tokio`, `serde`, `serde_json`, `redis`, `thiserror`), and verify with `cargo check -p mmo-storage`
- [x] 1.2 Implement database connection manager with automated schema migrations (`accounts`, `characters`, `entity_snapshots`) using runtime SQLx queries, and verify with SQLite database initialization tests
- [x] 1.3 Implement `CharacterRepository` and `EntitySnapshot` data models providing character creation, lookup, and component updates, and verify via unit tests
- [x] 1.4 Implement `SessionCacheBackend` trait with `MemoryCache` and `RedisCache` implementations for session presence and handover staging, and verify via unit tests

## 2. Cluster Integration & Verification

- [x] 2.1 Implement `WriteBehindSnapshotter` in `crates/node` using a coalescing dirty map (`Arc<Mutex<HashMap<Guid, EntitySnapshot>>>`) and background worker with `tokio::sync::Notify` to persist states without stalling the 20 Hz tick, and verify via unit tests
- [x] 2.2 Wire session presence and route cache lookups in `crates/gateway` using `SessionCacheBackend`, and verify with session routing tests
- [x] 2.3 Update `docker-compose.yml` to include optional `postgres` and `redis` services wired into cluster environment variables, and verify configuration via `docker compose config`
- [x] 2.4 Run end-to-end integration test verifying character creation, continuous movement snapshotting, and state reload upon simulation restart
