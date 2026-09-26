# Technical Design: Production Distributed Storage (PostgreSQL & Account/Inventory)

## Architecture Overview

```mermaid
flowchart TD
    subgraph K8sCluster["Kubernetes Cluster (mmo namespace)"]
        subgraph SimTier["Simulation Tier"]
            Node0["mmo-node-0"]
            Node1["mmo-node-1"]
            Gateway["mmo-gateway"]
        end
        subgraph StorageTier["Persistence Tier"]
            PG["PostgreSQL (postgres:16-alpine)\nService: postgres.mmo:5432"]
            Redis["Redis (redis:7-alpine)\nService: redis.mmo:6379"]
        end
    end

    Node0 -->|"sqlx PgPool (ACID / Snapshots)"| PG
    Node1 -->|"sqlx PgPool (ACID / Snapshots)"| PG
    Gateway -->|"sqlx PgPool (Auth / Characters)"| PG
    Gateway -->|"Redis Session Cache & Social Bus"| Redis
    Node0 -->|"Redis Social Bus"| Redis
    Node1 -->|"Redis Social Bus"| Redis
```

---

## 1. Unified Multi-Backend Database Abstraction

To support both hermetic in-memory / local SQLite testing and distributed PostgreSQL production deployment, `crates/storage` will encapsulate backend dispatch inside `Database`:

```rust
#[derive(Clone)]
pub enum DbBackend {
    Sqlite(sqlx::Pool<sqlx::Sqlite>),
    Postgres(sqlx::Pool<sqlx::Postgres>),
}

#[derive(Clone)]
pub struct Database {
    backend: DbBackend,
}
```

### Protocol Detection
- If URI starts with `postgres://` or `postgresql://`:
  - Initialize `PgPoolOptions::new().min_connections(2).max_connections(20).acquire_timeout(Duration::from_secs(5))`
  - Execute PostgreSQL migration batch.
- If URI starts with `sqlite:` or `sqlite::memory:`:
  - Initialize `SqlitePoolOptions::new().max_connections(10).busy_timeout(Duration::from_secs(5))`
  - Set pragmas `journal_mode = WAL`, `synchronous = NORMAL`.
  - Execute SQLite migration batch.

### Query Compatibility & Dialect Strategy
- **Parameter Placeholders**:
  - `sqlx` expects `?` for SQLite, but strictly requires `$1, $2, ...` for PostgreSQL.
  - To prevent dialect bugs and code duplication, `crates/storage` provides a query compilation helper:
    `sql_for_dialect(query_template: &str, backend: &DbBackend) -> String`
    which automatically maps unquoted `?` placeholders to `$1, $2, $3...` when targeting PostgreSQL, or retains `?` when targeting SQLite.
- **Dialect-Specific Row Locking**:
  - *PostgreSQL*: Executes `SELECT guid FROM characters WHERE guid = $1 FOR UPDATE` on `first_guid` followed by `second_guid` within a transaction.
  - *SQLite*: Executes `BEGIN IMMEDIATE` to obtain a reserved lock on the database file, skipping `FOR UPDATE` (which SQLite rejects with a syntax error).
- **Batch Insertion Optimization**:
  - When saving entity snapshots in `save_snapshot_batch`, multi-row batch inserts will use `sqlx::QueryBuilder` to generate a single round-trip multi-row statement (`INSERT INTO entity_snapshots (...) VALUES (...), (...)... ON CONFLICT (...) DO UPDATE`) rather than running 500 individual queries in a loop, avoiding connection pool starvation over the network.
- Standard ANSI types (`BIGINT`, `INTEGER`, `REAL`, `TEXT`) are natively supported across both engines.

---

## 2. Relational Schema & Tables

### Tables & Relationships
1. **`accounts`**:
   - `account_id TEXT PRIMARY KEY`
   - `username TEXT NOT NULL UNIQUE`
   - `status TEXT NOT NULL DEFAULT 'active'`
   - `created_at BIGINT NOT NULL`
   - `updated_at BIGINT NOT NULL`

2. **`characters`**:
   - `guid BIGINT PRIMARY KEY`
   - `account_id TEXT NOT NULL REFERENCES accounts(account_id) ON DELETE CASCADE`
   - `name TEXT NOT NULL`
   - `level INTEGER NOT NULL DEFAULT 1`
   - `experience BIGINT NOT NULL DEFAULT 0`
   - `pos_x REAL NOT NULL`
   - `pos_y REAL NOT NULL`
   - `pos_z REAL NOT NULL`
   - `rotation REAL NOT NULL DEFAULT 0.0`
   - `attributes_json TEXT NOT NULL DEFAULT '{}'`
   - `updated_at BIGINT NOT NULL`
   - Index: `idx_characters_account ON characters(account_id)`

3. **`inventories`**:
   - `character_guid BIGINT PRIMARY KEY REFERENCES characters(guid) ON DELETE CASCADE`
   - `capacity INTEGER NOT NULL DEFAULT 30`
   - `updated_at BIGINT NOT NULL`

4. **`items`**:
   - `item_instance_id TEXT PRIMARY KEY`
   - `character_guid BIGINT NOT NULL REFERENCES characters(guid) ON DELETE CASCADE`
   - `item_def_id INTEGER NOT NULL`
   - `slot INTEGER NOT NULL`
   - `quantity INTEGER NOT NULL DEFAULT 1`
   - `durability INTEGER NOT NULL DEFAULT 100`
   - `attributes_json TEXT NOT NULL DEFAULT '{}'`
   - `created_at BIGINT NOT NULL`
   - `updated_at BIGINT NOT NULL`
   - Constraint: `UNIQUE(character_guid, slot)`
   - Index: `idx_items_character_slot ON items(character_guid, slot)`

5. **`trades`** (Audit Trail):
   - `trade_id TEXT PRIMARY KEY`
   - `sender_guid BIGINT NOT NULL`
   - `recipient_guid BIGINT NOT NULL`
   - `items_json TEXT NOT NULL`
   - `status TEXT NOT NULL` (committed / rolled_back)
   - `created_at BIGINT NOT NULL`
   - Index: `idx_trades_sender ON trades(sender_guid)`
   - Index: `idx_trades_recipient ON trades(recipient_guid)`

---

## 3. Deadlock-Free ACID Trading Protocol

```mermaid
sequenceDiagram
    autonumber
    participant ClientA as Sender (Char A)
    participant ClientB as Recipient (Char B)
    participant Repo as InventoryRepository
    participant DB as Postgres / SQLite

    Note over Repo,DB: Lock Ordering: min(A, B) -> max(A, B)
    ClientA->>Repo: execute_trade(sender=A, recipient=B, item_ids)
    Repo->>DB: BEGIN TRANSACTION
    Repo->>DB: Lock sender & recipient rows (sorted order)
    Repo->>DB: Validate sender owns items & recipient has free slots
    alt Validation Failed (no slot or missing item)
        Repo->>DB: Record audit log (status = rolled_back)
        Repo->>DB: ROLLBACK TRANSACTION
        Repo-->>ClientA: Error (InsufficientSpace / MissingItem)
    else Validation Succeeded
        Repo->>DB: Move item rows (update character_guid & slot)
        Repo->>DB: Record audit log (status = committed)
        Repo->>DB: COMMIT TRANSACTION
        Repo-->>ClientA: Success (TradeReceipt)
        Repo-->>ClientB: Success (TradeReceipt)
    end
```

### Deadlock Elimination
Circular wait condition:
- Thread 1: Wants locks on Char 100 then Char 200.
- Thread 2: Wants locks on Char 200 then Char 100.
- Result without ordering: Deadlock.
- **Rule**: All transactions requiring locks on multiple characters MUST sort GUIDs:
  `let (first_guid, second_guid) = if guid_a < guid_b { (guid_a, guid_b) } else { (guid_b, guid_a) };`
  Lock `first_guid`, then lock `second_guid`.

---

## 4. Kubernetes Architecture

- `k8s/base/postgres.yaml`:
  - Deployment with `postgres:16-alpine`.
  - Secret `postgres-secret` with `POSTGRES_DB=mmo_db`, `POSTGRES_USER=mmo`, `POSTGRES_PASSWORD=mmo_secret`.
  - Service `postgres` on port `5432`.
  - Health probes: `pg_isready -U mmo -d mmo_db` for liveness and readiness.
- In `k8s/base/node-statefulset.yaml`:
  - Set `DATABASE_URL: postgres://mmo:mmo_secret@postgres.mmo.svc.cluster.local:5432/mmo_db`.
- In `k8s/base/gateway.yaml`:
  - Set `DATABASE_URL` pointing to PostgreSQL cluster service.
