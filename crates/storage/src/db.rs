//! Database connection pool and schema migrations.

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Pool, Sqlite};
use std::str::FromStr;
use std::time::Duration;
use thiserror::Error;

/// Storage errors.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("Database error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("Redis cache error: {0}")]
    Redis(#[from] redis::RedisError),

    #[error("Entity not found: {0}")]
    NotFound(String),

    #[error("Version conflict: record version {current} >= incoming {incoming}")]
    VersionConflict { current: u64, incoming: u64 },

    #[error("Unsupported database backend: {0}")]
    UnsupportedBackend(String),

    #[error("Inventory is full for character {0} (capacity {1})")]
    InventoryFull(u64, u32),

    #[error("Item {0} not found in inventory of character {1}")]
    ItemNotFound(String, u64),

    #[error("Slot collision: slot {0} is already occupied on character {1}")]
    SlotCollision(u32, u64),

    #[error("Insufficient quantity: requested {requested}, available {available}")]
    InsufficientQuantity { requested: u32, available: u32 },

    #[error("Trade failed: {0}")]
    TradeError(String),

    #[error("Container {0} not found")]
    ContainerNotFound(u64),

    #[error("Container {0} is full (capacity {1})")]
    ContainerFull(u64, u32),

    #[error("Slot {0} in container {1} is already occupied")]
    SlotOccupied(u32, u64),

    #[error("Slot {0} in container {1} is currently reserved")]
    SlotReserved(u32, u64),

    #[error("Transaction {0} expired or was aborted")]
    ReservationExpiredOrAborted(String),

    #[error("Escrow {0} not found")]
    EscrowNotFound(String),

    #[error("Escrow {0} secret mismatch")]
    EscrowSecretMismatch(String),

    #[error("Escrow {0} has expired")]
    EscrowExpired(String),

    #[error("Escrow {0} is already settled or refunded")]
    EscrowAlreadySettled(String),

    #[error("Unauthorized access to container {0}")]
    UnauthorizedContainerAccess(u64),

    #[error("Listing not found: {0}")]
    ListingNotFound(String),

    #[error("Listing {0} is not active (status: {1})")]
    ListingNotActive(String, String),

    #[error("Unauthorized marketplace action on listing {0}")]
    UnauthorizedMarketAction(String),
}

/// Database backend variant.
#[derive(Clone)]
pub enum DbBackend {
    Sqlite(sqlx::Pool<Sqlite>),
    Postgres(sqlx::Pool<sqlx::Postgres>),
}

/// Helper function to adapt standard ANSI SQL queries with `?` parameters
/// to PostgreSQL's `$1, $2, ...` positional parameter requirements.
pub fn sql_for_dialect(query: &str, is_postgres: bool) -> String {
    if !is_postgres {
        return query.to_string();
    }
    let mut result = String::with_capacity(query.len() + 16);
    let mut param_idx = 1;
    let mut in_single_quote = false;
    for c in query.chars() {
        if c == '\'' {
            in_single_quote = !in_single_quote;
            result.push(c);
        } else if c == '?' && !in_single_quote {
            result.push('$');
            result.push_str(&param_idx.to_string());
            param_idx += 1;
        } else {
            result.push(c);
        }
    }
    result
}

/// Current UTC timestamp in milliseconds since Unix epoch without panicking.
pub fn current_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Current UTC timestamp in seconds since Unix epoch without panicking.
pub fn current_time_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Computes all free slots in ascending order for a container of given capacity.
pub fn compute_free_slots(occupied: &std::collections::HashSet<u32>, capacity: u32) -> Vec<u32> {
    (0..capacity).filter(|s| !occupied.contains(s)).collect()
}

/// Finds the first free slot index in ascending numerical order for a container of given capacity.
pub fn find_first_free_slot(
    occupied: &std::collections::HashSet<u32>,
    capacity: u32,
) -> Option<u32> {
    (0..capacity).find(|s| !occupied.contains(s))
}

/// Macro for executing dialect-adapted transactional blocks across SQLite and PostgreSQL.
#[macro_export]
macro_rules! with_dialect_tx {
    ($db:expr, |$conn:ident, $is_pg:ident| async move $body:block) => {
        $crate::with_dialect_tx!($db, |$conn, $is_pg| $body)
    };
    ($db:expr, |$conn:ident, $is_pg:ident| async $body:block) => {
        $crate::with_dialect_tx!($db, |$conn, $is_pg| $body)
    };
    ($db:expr, |$conn:ident| async move $body:block) => {
        $crate::with_dialect_tx!($db, |$conn, _is_pg| $body)
    };
    ($db:expr, |$conn:ident| async $body:block) => {
        $crate::with_dialect_tx!($db, |$conn, _is_pg| $body)
    };
    ($db:expr, |$conn:ident| $body:block) => {
        $crate::with_dialect_tx!($db, |$conn, _is_pg| $body)
    };
    ($db:expr, |$conn:ident, $is_pg:ident| $body:block) => {
        match $db.backend() {
            $crate::db::DbBackend::Sqlite(pool) => {
                let mut conn_holder = pool.acquire().await?;
                sqlx::query("BEGIN IMMEDIATE")
                    .execute(&mut *conn_holder)
                    .await?;
                let res: Result<_, $crate::db::StorageError> = {
                    let $conn = &mut *conn_holder;
                    #[allow(unused_variables)]
                    let $is_pg = false;
                    #[allow(unused_macros)]
                    macro_rules! q {
                        ($sql:expr) => {
                            $crate::db::sql_for_dialect($sql, false)
                        };
                    }
                    #[allow(unused_macros)]
                    macro_rules! lock_suffix {
                        () => {
                            ""
                        };
                    }
                    async { $body }.await
                };
                match res {
                    Ok(val) => {
                        sqlx::query("COMMIT").execute(&mut *conn_holder).await?;
                        Ok(val)
                    }
                    Err(e) => {
                        let _ = sqlx::query("ROLLBACK").execute(&mut *conn_holder).await;
                        Err(e)
                    }
                }
            }
            $crate::db::DbBackend::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let res: Result<_, $crate::db::StorageError> = {
                    let $conn = &mut *tx;
                    #[allow(unused_variables)]
                    let $is_pg = true;
                    #[allow(unused_macros)]
                    macro_rules! q {
                        ($sql:expr) => {
                            $crate::db::sql_for_dialect($sql, true)
                        };
                    }
                    #[allow(unused_macros)]
                    macro_rules! lock_suffix {
                        () => {
                            " FOR UPDATE"
                        };
                    }
                    async { $body }.await
                };
                match res {
                    Ok(val) => {
                        tx.commit().await?;
                        Ok(val)
                    }
                    Err(e) => Err(e),
                }
            }
        }
    };
}

/// Central database manager backed by SQLite or PostgreSQL connection pool.
#[derive(Clone)]
pub struct Database {
    backend: DbBackend,
}

impl Database {
    /// Adapts the given SQL statement with `?` parameter placeholders to the active backend dialect.
    pub fn sql(&self, query: &str) -> String {
        sql_for_dialect(query, self.is_postgres())
    }

    /// Suffix for locking rows during transactions (` FOR UPDATE` on PostgreSQL, empty on SQLite).
    pub fn for_update_suffix(&self) -> &'static str {
        if self.is_postgres() {
            " FOR UPDATE"
        } else {
            ""
        }
    }
    /// Connects to a database using the provided connection string.
    /// Auto-detects dialect:
    /// - `postgres://` or `postgresql://` -> PostgreSQL pool
    /// - `sqlite:` or local path -> SQLite pool
    pub async fn connect(url: &str) -> Result<Self, StorageError> {
        if url.starts_with("postgres://") || url.starts_with("postgresql://") {
            let pool = sqlx::postgres::PgPoolOptions::new()
                .min_connections(2)
                .max_connections(20)
                .acquire_timeout(Duration::from_secs(5))
                .idle_timeout(Duration::from_secs(600))
                .connect(url)
                .await?;

            let db = Self {
                backend: DbBackend::Postgres(pool),
            };
            db.run_migrations().await?;
            Ok(db)
        } else if url.starts_with("sqlite:") || url.contains(".db") || url.contains(":memory:") {
            let mut opts = SqliteConnectOptions::from_str(url)?
                .create_if_missing(true)
                .busy_timeout(Duration::from_secs(5));

            if !url.contains(":memory:") {
                opts = opts
                    .pragma("journal_mode", "WAL")
                    .pragma("synchronous", "NORMAL");
            }

            let pool = SqlitePoolOptions::new()
                .max_connections(10)
                .connect_with(opts)
                .await?;

            let db = Self {
                backend: DbBackend::Sqlite(pool),
            };
            db.run_migrations().await?;
            Ok(db)
        } else {
            Err(StorageError::UnsupportedBackend(url.to_string()))
        }
    }

    /// Creates an in-memory SQLite database for unit tests and local isolation.
    pub async fn connect_memory() -> Result<Self, StorageError> {
        let opts =
            SqliteConnectOptions::from_str("sqlite::memory:")?.busy_timeout(Duration::from_secs(5));

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await?;

        let db = Self {
            backend: DbBackend::Sqlite(pool),
        };
        db.run_migrations().await?;
        Ok(db)
    }

    /// Returns a reference to the active `DbBackend`.
    pub fn backend(&self) -> &DbBackend {
        &self.backend
    }

    /// Returns whether the database backend is PostgreSQL.
    pub fn is_postgres(&self) -> bool {
        matches!(self.backend, DbBackend::Postgres(_))
    }

    /// Returns a reference to the underlying SQLite pool if active.
    pub fn sqlite_pool(&self) -> Option<&Pool<Sqlite>> {
        match &self.backend {
            DbBackend::Sqlite(p) => Some(p),
            _ => None,
        }
    }

    /// Returns a reference to the underlying PostgreSQL pool if active.
    pub fn pg_pool(&self) -> Option<&Pool<sqlx::Postgres>> {
        match &self.backend {
            DbBackend::Postgres(p) => Some(p),
            _ => None,
        }
    }

    /// Backward compatibility accessor for SQLite pool (used by legacy tests).
    pub fn pool(&self) -> &Pool<Sqlite> {
        self.sqlite_pool()
            .expect("Database backend is not SQLite; use db.backend() or pg_pool()")
    }

    /// Liveness / readiness health check probe executing `SELECT 1`.
    pub async fn ping(&self) -> Result<(), StorageError> {
        match &self.backend {
            DbBackend::Sqlite(pool) => {
                sqlx::query("SELECT 1").execute(pool).await?;
            }
            DbBackend::Postgres(pool) => {
                sqlx::query("SELECT 1").execute(pool).await?;
            }
        }
        Ok(())
    }

    /// Executes idempotent database migrations setting up core tables and indexes.
    pub async fn run_migrations(&self) -> Result<(), StorageError> {
        let blob_type = if self.is_postgres() { "BYTEA" } else { "BLOB" };
        let create_entity_snapshots = format!(
            r#"
            CREATE TABLE IF NOT EXISTS entity_snapshots (
                guid BIGINT PRIMARY KEY,
                pos_x REAL NOT NULL,
                pos_y REAL NOT NULL,
                pos_z REAL NOT NULL,
                rotation REAL NOT NULL DEFAULT 0.0,
                attributes_json TEXT NOT NULL DEFAULT '{{}}',
                state_blob {},
                version BIGINT NOT NULL DEFAULT 1,
                timestamp BIGINT NOT NULL
            );
            "#,
            blob_type
        );

        let statements = [
            r#"
            CREATE TABLE IF NOT EXISTS accounts (
                account_id TEXT PRIMARY KEY,
                username TEXT NOT NULL UNIQUE,
                created_at BIGINT NOT NULL
            );
            "#,
            r#"
            CREATE TABLE IF NOT EXISTS characters (
                guid BIGINT PRIMARY KEY,
                account_id TEXT NOT NULL,
                name TEXT NOT NULL,
                level INTEGER NOT NULL DEFAULT 1,
                pos_x REAL NOT NULL,
                pos_y REAL NOT NULL,
                pos_z REAL NOT NULL,
                rotation REAL NOT NULL DEFAULT 0.0,
                attributes_json TEXT NOT NULL DEFAULT '{}',
                updated_at BIGINT NOT NULL,
                FOREIGN KEY (account_id) REFERENCES accounts(account_id) ON DELETE CASCADE
            );
            "#,
            r#"
            CREATE INDEX IF NOT EXISTS idx_characters_account ON characters(account_id);
            "#,
            create_entity_snapshots.as_str(),
            r#"
            CREATE TABLE IF NOT EXISTS inventories (
                character_guid BIGINT PRIMARY KEY,
                capacity INTEGER NOT NULL DEFAULT 30,
                updated_at BIGINT NOT NULL,
                FOREIGN KEY (character_guid) REFERENCES characters(guid) ON DELETE CASCADE
            );
            "#,
            r#"
            CREATE TABLE IF NOT EXISTS items (
                item_instance_id TEXT PRIMARY KEY,
                character_guid BIGINT NOT NULL DEFAULT 0,
                container_id BIGINT NOT NULL DEFAULT 0,
                item_def_id INTEGER NOT NULL,
                slot INTEGER NOT NULL,
                quantity INTEGER NOT NULL DEFAULT 1,
                durability INTEGER NOT NULL DEFAULT 100,
                attributes_json TEXT NOT NULL DEFAULT '{}',
                created_at BIGINT NOT NULL,
                updated_at BIGINT NOT NULL,
                CONSTRAINT uq_container_slot UNIQUE(container_id, slot)
            );
            "#,
            r#"
            CREATE INDEX IF NOT EXISTS idx_items_character_guid ON items(character_guid);
            "#,
            r#"
            CREATE INDEX IF NOT EXISTS idx_items_container_slot ON items(container_id, slot);
            "#,
            r#"
            CREATE TABLE IF NOT EXISTS trades (
                trade_id TEXT PRIMARY KEY,
                sender_guid BIGINT NOT NULL,
                recipient_guid BIGINT NOT NULL,
                items_json TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at BIGINT NOT NULL
            );
            "#,
            r#"
            CREATE INDEX IF NOT EXISTS idx_trades_sender ON trades(sender_guid);
            "#,
            r#"
            CREATE INDEX IF NOT EXISTS idx_trades_recipient ON trades(recipient_guid);
            "#,
            r#"
            CREATE TABLE IF NOT EXISTS containers (
                container_id BIGINT PRIMARY KEY,
                owner_type TEXT NOT NULL,
                owner_id TEXT NOT NULL,
                name TEXT NOT NULL,
                capacity INTEGER NOT NULL,
                access_policy TEXT NOT NULL,
                version BIGINT NOT NULL DEFAULT 1,
                created_at BIGINT NOT NULL,
                updated_at BIGINT NOT NULL
            );
            "#,
            r#"
            CREATE INDEX IF NOT EXISTS idx_containers_owner ON containers (owner_type, owner_id);
            "#,
            r#"
            CREATE TABLE IF NOT EXISTS container_reservations (
                reservation_id TEXT PRIMARY KEY,
                tx_id TEXT NOT NULL,
                container_id BIGINT NOT NULL,
                slot INTEGER NOT NULL,
                item_instance_id TEXT,
                reservation_type TEXT NOT NULL,
                expires_at_epoch_ms BIGINT NOT NULL,
                created_at_epoch_ms BIGINT NOT NULL,
                FOREIGN KEY (container_id) REFERENCES containers(container_id) ON DELETE CASCADE,
                CONSTRAINT uq_container_slot_res UNIQUE(container_id, slot)
            );
            "#,
            r#"
            CREATE INDEX IF NOT EXISTS idx_reservations_tx ON container_reservations(tx_id);
            "#,
            r#"
            CREATE INDEX IF NOT EXISTS idx_reservations_expiry ON container_reservations(expires_at_epoch_ms);
            "#,
            r#"
            CREATE TABLE IF NOT EXISTS escrows (
                escrow_id TEXT PRIMARY KEY,
                depositor_container_id BIGINT NOT NULL,
                beneficiary_container_id BIGINT NOT NULL,
                escrow_container_id BIGINT NOT NULL,
                status TEXT NOT NULL,
                secret_hash TEXT NOT NULL,
                expires_at_epoch_ms BIGINT NOT NULL,
                created_at_epoch_ms BIGINT NOT NULL,
                updated_at_epoch_ms BIGINT NOT NULL,
                FOREIGN KEY (depositor_container_id) REFERENCES containers(container_id) ON DELETE CASCADE,
                FOREIGN KEY (beneficiary_container_id) REFERENCES containers(container_id) ON DELETE CASCADE,
                FOREIGN KEY (escrow_container_id) REFERENCES containers(container_id) ON DELETE CASCADE
            );
            "#,
            r#"
            CREATE INDEX IF NOT EXISTS idx_escrows_status_expiry ON escrows(status, expires_at_epoch_ms);
            "#,
            r#"
            CREATE TABLE IF NOT EXISTS container_transactions (
                tx_id TEXT PRIMARY KEY,
                src_container_id BIGINT NOT NULL,
                dst_container_id BIGINT NOT NULL,
                item_instance_id TEXT NOT NULL,
                status TEXT NOT NULL,
                timestamp_epoch_ms BIGINT NOT NULL
            );
            "#,
            r#"
            CREATE INDEX IF NOT EXISTS idx_container_tx_time ON container_transactions(timestamp_epoch_ms);
            "#,
            r#"
            CREATE TABLE IF NOT EXISTS world_objects (
                world_id BIGINT NOT NULL,
                object_id BIGINT NOT NULL,
                sector_x INT NOT NULL,
                sector_y INT NOT NULL,
                sector_z INT NOT NULL,
                prefab_id INT NOT NULL,
                pos_x REAL NOT NULL,
                pos_y REAL NOT NULL,
                pos_z REAL NOT NULL,
                rot_x REAL NOT NULL,
                rot_y REAL NOT NULL,
                rot_z REAL NOT NULL,
                rot_w REAL NOT NULL,
                scale_x REAL NOT NULL DEFAULT 1.0,
                scale_y REAL NOT NULL DEFAULT 1.0,
                scale_z REAL NOT NULL DEFAULT 1.0,
                bounds_min_x REAL NOT NULL,
                bounds_min_y REAL NOT NULL,
                bounds_min_z REAL NOT NULL,
                bounds_max_x REAL NOT NULL,
                bounds_max_y REAL NOT NULL,
                bounds_max_z REAL NOT NULL,
                state_flags INT NOT NULL DEFAULT 0,
                owner_type TEXT NOT NULL,
                owner_id TEXT NOT NULL,
                health INT,
                max_health INT,
                properties_json TEXT,
                dirty_revision BIGINT NOT NULL,
                updated_at BIGINT NOT NULL,
                PRIMARY KEY (world_id, object_id)
            );
            "#,
            r#"
            CREATE INDEX IF NOT EXISTS idx_world_objects_sector ON world_objects (world_id, sector_x, sector_y, sector_z);
            "#,
            r#"
            CREATE INDEX IF NOT EXISTS idx_world_objects_updated ON world_objects (world_id, updated_at);
            "#,
            r#"
            CREATE TABLE IF NOT EXISTS market_listings (
                listing_id TEXT PRIMARY KEY,
                seller_guid BIGINT NOT NULL,
                seller_container_id BIGINT NOT NULL,
                item_instance_id TEXT NOT NULL,
                item_def_id INTEGER NOT NULL,
                price BIGINT NOT NULL,
                escrow_container_id BIGINT NOT NULL,
                escrow_slot INTEGER NOT NULL,
                status TEXT NOT NULL,
                created_at_epoch_ms BIGINT NOT NULL,
                expires_at_epoch_ms BIGINT NOT NULL,
                updated_at_epoch_ms BIGINT NOT NULL
            );
            "#,
            r#"
            CREATE INDEX IF NOT EXISTS idx_market_status_expiry ON market_listings (status, expires_at_epoch_ms);
            "#,
            r#"
            CREATE INDEX IF NOT EXISTS idx_market_item_def ON market_listings (item_def_id, status);
            "#,
            r#"
            CREATE INDEX IF NOT EXISTS idx_market_seller ON market_listings (seller_guid, status);
            "#,
        ];

        match &self.backend {
            DbBackend::Sqlite(pool) => {
                for stmt in statements {
                    sqlx::query(stmt).execute(pool).await?;
                }
            }
            DbBackend::Postgres(pool) => {
                for stmt in statements {
                    sqlx::query(stmt).execute(pool).await?;
                }
            }
        }

        self.ensure_items_container_column().await?;
        self.ensure_entity_snapshots_blob_column().await?;

        Ok(())
    }

    /// Idempotently verifies and adds `state_blob` column on the `entity_snapshots` table.
    async fn ensure_entity_snapshots_blob_column(&self) -> Result<(), StorageError> {
        use sqlx::Row;
        match &self.backend {
            DbBackend::Postgres(pool) => {
                sqlx::query(
                    "ALTER TABLE entity_snapshots ADD COLUMN IF NOT EXISTS state_blob BYTEA",
                )
                .execute(pool)
                .await?;
            }
            DbBackend::Sqlite(pool) => {
                let cols = sqlx::query("PRAGMA table_info(entity_snapshots)")
                    .fetch_all(pool)
                    .await?;
                let has_col = cols.iter().any(|r| {
                    r.try_get::<String, _>("name")
                        .map(|n| n == "state_blob")
                        .unwrap_or(false)
                });
                if !has_col {
                    sqlx::query("ALTER TABLE entity_snapshots ADD COLUMN state_blob BLOB")
                        .execute(pool)
                        .await?;
                }
            }
        }
        Ok(())
    }

    /// Idempotently verifies and backfills `container_id` on the `items` table.
    async fn ensure_items_container_column(&self) -> Result<(), StorageError> {
        use sqlx::Row;
        match &self.backend {
            DbBackend::Postgres(pool) => {
                sqlx::query(
                    "ALTER TABLE items ADD COLUMN IF NOT EXISTS container_id BIGINT DEFAULT 0",
                )
                .execute(pool)
                .await?;
                sqlx::query(
                    "UPDATE items SET container_id = character_guid WHERE container_id = 0",
                )
                .execute(pool)
                .await?;
            }
            DbBackend::Sqlite(pool) => {
                let cols = sqlx::query("PRAGMA table_info(items)")
                    .fetch_all(pool)
                    .await?;
                let has_col = cols.iter().any(|r| {
                    r.try_get::<String, _>("name")
                        .map(|n| n == "container_id")
                        .unwrap_or(false)
                });
                if !has_col {
                    sqlx::query("ALTER TABLE items ADD COLUMN container_id BIGINT DEFAULT 0")
                        .execute(pool)
                        .await?;
                    sqlx::query(
                        "UPDATE items SET container_id = character_guid WHERE container_id = 0",
                    )
                    .execute(pool)
                    .await?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_database_initialization_and_migrations() {
        let db = Database::connect_memory().await.unwrap();

        // Verify tables exist by querying sqlite_master
        let count: (i64,) = sqlx::query_as(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('accounts', 'characters', 'entity_snapshots', 'inventories', 'items', 'trades')"
        )
        .fetch_one(db.pool())
        .await
        .unwrap();

        assert_eq!(count.0, 6);
    }

    #[tokio::test]
    async fn test_database_ping() {
        let db = Database::connect_memory().await.unwrap();
        assert!(db.ping().await.is_ok());
    }

    #[test]
    fn test_sql_for_dialect_translation() {
        let query = "INSERT INTO items (id, name, desc) VALUES (?, ?, 'what?');";
        assert_eq!(sql_for_dialect(query, false), query);
        assert_eq!(
            sql_for_dialect(query, true),
            "INSERT INTO items (id, name, desc) VALUES ($1, $2, 'what?');"
        );
    }

    #[tokio::test]
    async fn test_database_wal_mode() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test_wal.db");
        let url = format!("sqlite://{}", db_path.display());

        let db = Database::connect(&url).await.unwrap();

        let row: (String,) = sqlx::query_as("PRAGMA journal_mode")
            .fetch_one(db.pool())
            .await
            .unwrap();

        assert_eq!(row.0.to_lowercase(), "wal");
    }

    #[test]
    fn test_compute_free_slots_and_find_first() {
        let mut occupied = std::collections::HashSet::new();
        occupied.insert(0);
        occupied.insert(2);
        occupied.insert(3);

        let free = compute_free_slots(&occupied, 5);
        assert_eq!(free, vec![1, 4]);

        assert_eq!(find_first_free_slot(&occupied, 5), Some(1));

        occupied.insert(1);
        occupied.insert(4);
        assert_eq!(compute_free_slots(&occupied, 5), Vec::<u32>::new());
        assert_eq!(find_first_free_slot(&occupied, 5), None);
    }

    #[tokio::test]
    async fn test_database_sql_and_suffix() {
        let db = Database::connect_memory().await.unwrap();
        assert_eq!(
            db.sql("SELECT * FROM items WHERE a = ? AND b = ?"),
            "SELECT * FROM items WHERE a = ? AND b = ?"
        );
        assert_eq!(db.for_update_suffix(), "");

        let pg_pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://localhost/test")
            .unwrap();
        let pg_db = Database {
            backend: DbBackend::Postgres(pg_pool),
        };
        assert!(pg_db.is_postgres());
        assert_eq!(pg_db.for_update_suffix(), " FOR UPDATE");
    }
}
