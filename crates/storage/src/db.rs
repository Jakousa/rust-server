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
}

/// Central database manager backed by SQLite connection pool.
#[derive(Clone)]
pub struct Database {
    pool: Pool<Sqlite>,
}

impl Database {
    /// Connects to a SQLite database using the provided connection string.
    pub async fn connect(url: &str) -> Result<Self, StorageError> {
        let opts = SqliteConnectOptions::from_str(url)?
            .create_if_missing(true)
            .busy_timeout(Duration::from_secs(5));

        let pool = SqlitePoolOptions::new()
            .max_connections(10)
            .connect_with(opts)
            .await?;

        let db = Self { pool };
        db.run_migrations().await?;
        Ok(db)
    }

    /// Creates an in-memory SQLite database for unit tests and local isolation.
    pub async fn connect_memory() -> Result<Self, StorageError> {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")?
            .busy_timeout(Duration::from_secs(5));

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await?;

        let db = Self { pool };
        db.run_migrations().await?;
        Ok(db)
    }

    /// Returns a reference to the underlying SQLx pool.
    pub fn pool(&self) -> &Pool<Sqlite> {
        &self.pool
    }

    /// Executes idempotent database migrations setting up core tables and indexes.
    pub async fn run_migrations(&self) -> Result<(), StorageError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS accounts (
                account_id TEXT PRIMARY KEY,
                username TEXT NOT NULL UNIQUE,
                created_at INTEGER NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS characters (
                guid INTEGER PRIMARY KEY,
                account_id TEXT NOT NULL,
                name TEXT NOT NULL,
                level INTEGER NOT NULL DEFAULT 1,
                pos_x REAL NOT NULL,
                pos_y REAL NOT NULL,
                pos_z REAL NOT NULL,
                rotation REAL NOT NULL DEFAULT 0.0,
                attributes_json TEXT NOT NULL DEFAULT '{}',
                updated_at INTEGER NOT NULL,
                FOREIGN KEY (account_id) REFERENCES accounts(account_id)
            );
            CREATE INDEX IF NOT EXISTS idx_characters_account ON characters(account_id);
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS entity_snapshots (
                guid INTEGER PRIMARY KEY,
                pos_x REAL NOT NULL,
                pos_y REAL NOT NULL,
                pos_z REAL NOT NULL,
                rotation REAL NOT NULL DEFAULT 0.0,
                attributes_json TEXT NOT NULL DEFAULT '{}',
                version INTEGER NOT NULL DEFAULT 1,
                timestamp INTEGER NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

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
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('accounts', 'characters', 'entity_snapshots')"
        )
        .fetch_one(db.pool())
        .await
        .unwrap();

        assert_eq!(count.0, 3);
    }
}
