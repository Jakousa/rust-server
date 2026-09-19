## Context

All entity data and active sessions currently reside strictly in volatile simulation node memory. To support reliable character persistence, account loading, and atomic inter-node entity migrations, a dual-tier persistence system is required: a durable database layer for persistent player records and a low-latency cache for hot session handovers.

## Goals / Non-Goals

**Goals:**
- Provide `crates/storage`, a dedicated crate abstracting relational database persistence and distributed session caching.
- Support runtime SQLx queries compatible with SQLite (for zero-dependency local testing/CI) and PostgreSQL (for production) without requiring live database connections during Docker builds.
- Provide a non-blocking, coalescing write-behind snapshotter so simulation nodes emit database updates without stalling the 20 Hz (50ms) game tick loop.
- Solve component serialization by defining `EntitySnapshot` with typed metadata and a structured serializable attribute/component map.
- Support distributed session presence and migration staging with pluggable Redis and in-memory cache backends.
- Extend `docker-compose.yml` with optional Postgres and Redis services.

**Non-Goals:**
- Real-money transactional banking / payment processing.
- Full object-relational mapping (ORM) with complex inheritance trees; keep queries focused on clean relational schemas.

## Decisions

### Decision 1: Runtime SQLx Queries (No Offline Build Artifacts Required)
- **Chosen**: Use dynamic runtime `sqlx::query(...)` with standard ANSI SQL constructs instead of compile-time `sqlx::query!` macros.
- **Rationale**: Eliminates the requirement for `cargo sqlx prepare` offline metadata or live databases during Docker and CI builds, while allowing identical repository logic to run against SQLite and PostgreSQL.

### Decision 2: Coalescing Dirty Map with Asynchronous Worker Notification
- **Chosen**: The simulation node publishes dirty states into an `Arc<Mutex<HashMap<Guid, EntitySnapshot>>>` and signals a background writer task via `Arc<tokio::sync::Notify>`.
- **Rationale**: Completely avoids backpressure or queue overflow bugs inherent to pure MPSC channels under database lag. Multiple rapid updates for the same entity are naturally coalesced into a single pending write, and the simulation tick loop only performs an instant in-memory lock ($< 1\,\mu\text{s}$).

### Decision 3: Decoupled EntitySnapshot with Serializable Component Map
- **Chosen**: Define `EntitySnapshot` containing core entity fields (`guid`, `position`, `rotation`, `version`, `timestamp`) and a `HashMap<String, serde_json::Value>` for dynamic component attributes.
- **Rationale**: Bypasses the limitation where `Box<dyn Any>` inside `ComponentStore` cannot be directly serialized or cloned, allowing decoupled snapshot generation and wire persistence.

### Decision 4: Pluggable Session Cache (Redis + In-Memory Fallback)
- **Chosen**: Implement `SessionCacheBackend` trait with a Redis client implementation and an in-memory `RwLock<HashMap>` fallback.
- **Rationale**: Provides instant zero-setup execution in unit and integration tests while enabling true multi-host caching in Docker and Kubernetes clusters.

## Risks / Trade-offs

- **[Risk: Stale snapshot overwrite on rapid handovers]** → **Mitigation**: Include monotonic version sequence numbers on entity snapshots; reject database writes with version numbers lower than current DB record.
- **[Risk: High write volume on dirty map]** → **Mitigation**: The background snapshot worker drains all pending snapshots in batches (`dirty_map.drain()`) and uses batch transactions.
