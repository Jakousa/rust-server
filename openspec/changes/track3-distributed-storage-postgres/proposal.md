# Change Proposal: Production Distributed Storage (PostgreSQL & Account/Inventory)

## Context & Motivation
In Phase 1–3 and Track 2, the MMO server developed an elastic dynamic spatial partitioning cluster with live two-phase entity migration and dynamic route invalidation. Persistence was initially backed by node-local SQLite with write-behind snapshotting.

While SQLite is optimal for fast, hermetic, zero-dependency unit and integration testing, production deployments requiring multi-node horizontal scaling, high concurrency, and cross-node consistency require an enterprise-grade distributed relational database:
1. **Multi-Node Concurrency**: In a multi-node cluster, multiple simulation nodes (`mmo-node-0`, `mmo-node-1`, etc.) concurrently read and persist character snapshots and inventory state. Local SQLite cannot be concurrently accessed across independent Kubernetes pods without network filesystem corruption.
2. **ACID Item Trading & Anti-Duplication**: Player-to-player item trades and bank deposits span multiple entity inventories. Transactions must be strictly serializable/atomic with robust deadlock prevention, preventing item duplication (dupe exploits) during node crashes or network partitions.
3. **Flexible Storage Deployment**: The storage engine must be pluggable, seamlessly accepting a PostgreSQL connection string in Kubernetes or cloud environments (e.g. Amazon RDS, Google Cloud SQL, CockroachDB) while retaining SQLite support for in-memory and local test runs.

## Scope of Change
1. **Multi-Backend Database Engine (`crates/storage`)**:
   - Upgrade `crates/storage` dependencies to enable PostgreSQL alongside SQLite via `sqlx`.
   - Provide an unified `Database` abstraction that auto-detects dialect (`postgres://` / `postgresql://` vs `sqlite://` / `sqlite::memory:`).
   - Implement connection pool sizing, idle timeouts, busy timeouts, and health check probes (`db.ping()`).
2. **Relational Schema & Migrations**:
   - Automated idempotent migrations for both PostgreSQL and SQLite.
   - Relational tables: `accounts`, `characters`, `inventories`, `items`, `trades`.
   - Indexes and foreign keys enforcing referential integrity.
3. **Inventory & ACID Trading Repository**:
   - Character inventory management: capacity, item slots, stack quantities, item metadata/attributes.
   - Atomic multi-item player-to-player trade protocol with strict GUID lock ordering (`min(guid_a, guid_b)` -> `max(guid_a, guid_b)`) to eliminate deadlocks.
   - Audit trail in `trades` table recording transactions.
4. **Kubernetes Cluster Integration**:
   - Add PostgreSQL instance manifest (`k8s/base/postgres.yaml`) with persistent storage and credentials.
   - Expose ClusterIP service `postgres.mmo:5432`.
   - Wire `DATABASE_URL` into simulation node manifests (`k8s/base/node-statefulset.yaml`).
5. **Quality Gates & Verification**:
   - Concurrent item trade race condition tests.
   - Reconnection and ping health tests.
   - Zero clippy warnings, complete test pass, container build, and Kubernetes cluster validation.

## Non-Goals
- Replacing Redis caching for ephemeral session routing or pub/sub social chat.
- Distributed consensus for spatial simulation (handled by Director and Raft/gRPC).
- Full ORM adoption (we use SQLx parameterized queries for maximal query optimization and control).
