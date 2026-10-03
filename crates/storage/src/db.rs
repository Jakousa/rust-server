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

/// Central database manager backed by SQLite or PostgreSQL connection pool.
#[derive(Clone)]
pub struct Database {
    backend: DbBackend,
}

impl Database {
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
            r#"
            CREATE TABLE IF NOT EXISTS entity_snapshots (
                guid BIGINT PRIMARY KEY,
                pos_x REAL NOT NULL,
                pos_y REAL NOT NULL,
                pos_z REAL NOT NULL,
                rotation REAL NOT NULL DEFAULT 0.0,
                attributes_json TEXT NOT NULL DEFAULT '{}',
                version BIGINT NOT NULL DEFAULT 1,
                timestamp BIGINT NOT NULL
            );
            "#,
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
}
