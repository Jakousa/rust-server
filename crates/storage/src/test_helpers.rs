//! Test helpers and fixture builders for storage entities.

use crate::db::{Database, StorageError};
pub use crate::models::{ContainerRecordBuilder, ItemRecordBuilder};

/// Truncates all tables in PostgreSQL to ensure test isolation.
pub async fn reset_postgres_tables(db: &Database) -> Result<(), StorageError> {
    if let Some(pool) = db.pg_pool() {
        sqlx::query(
            r#"
            TRUNCATE TABLE
                market_listings,
                trades,
                container_transactions,
                container_reservations,
                escrows,
                items,
                inventories,
                containers,
                entity_snapshots,
                characters,
                accounts,
                world_objects
            CASCADE;
            "#,
        )
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Connects to an in-memory SQLite database and, if `TEST_POSTGRES_URL` is configured,
/// connects to PostgreSQL, runs migrations, and truncates all tables for a clean test run.
///
/// If `REQUIRE_POSTGRES_TEST` is set to `"1"` or `"true"`, panics if `TEST_POSTGRES_URL` is missing.
pub async fn connect_test_databases() -> Result<Vec<(&'static str, Database)>, StorageError> {
    let require_pg = std::env::var("REQUIRE_POSTGRES_TEST")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let pg_url = std::env::var("TEST_POSTGRES_URL").ok();
    connect_test_databases_impl(pg_url, require_pg).await
}

/// Internal helper parameterized by configuration for hermetic unit testing.
pub async fn connect_test_databases_impl(
    pg_url: Option<String>,
    require_pg: bool,
) -> Result<Vec<(&'static str, Database)>, StorageError> {
    let sqlite_db = Database::connect_memory().await?;
    let mut dbs = vec![("sqlite", sqlite_db)];

    match pg_url {
        Some(url) if !url.trim().is_empty() => {
            let pg_db = Database::connect(&url).await?;
            pg_db.run_migrations().await?;
            reset_postgres_tables(&pg_db).await?;
            dbs.push(("postgres", pg_db));
        }
        _ if require_pg => {
            panic!("REQUIRE_POSTGRES_TEST=1 is set, but TEST_POSTGRES_URL is missing or empty");
        }
        _ => {}
    }

    Ok(dbs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_container_record_builder_defaults() {
        let container = ContainerRecordBuilder::new(500).build();
        assert_eq!(container.container_id, 500);
        assert_eq!(container.owner_type, "character");
        assert_eq!(container.owner_id, "1001");
        assert_eq!(container.name, "Container 500");
        assert_eq!(container.capacity, 10);
        assert_eq!(container.access_policy, "owner_only");
        assert_eq!(container.version, 1);
        assert_eq!(container.created_at, 1000);
        assert_eq!(container.updated_at, 1000);
    }

    #[test]
    fn test_container_record_builder_custom() {
        let container = ContainerRecordBuilder::new(501)
            .owner_type("guild")
            .owner_id("g_99")
            .name("Guild Vault")
            .capacity(100)
            .access_policy("guild_shared")
            .version(3)
            .created_at(2000)
            .updated_at(2050)
            .build();
        assert_eq!(container.container_id, 501);
        assert_eq!(container.owner_type, "guild");
        assert_eq!(container.owner_id, "g_99");
        assert_eq!(container.name, "Guild Vault");
        assert_eq!(container.capacity, 100);
        assert_eq!(container.access_policy, "guild_shared");
        assert_eq!(container.version, 3);
        assert_eq!(container.created_at, 2000);
        assert_eq!(container.updated_at, 2050);
    }

    #[test]
    fn test_item_record_builder_defaults() {
        let item = ItemRecordBuilder::new("item_default").build();
        assert_eq!(item.item_instance_id, "item_default");
        assert_eq!(item.character_guid, 1001);
        assert_eq!(item.container_id, 1);
        assert_eq!(item.item_def_id, 1);
        assert_eq!(item.slot, 0);
        assert_eq!(item.quantity, 1);
        assert_eq!(item.durability, 100);
        assert_eq!(item.attributes, serde_json::json!({}));
        assert_eq!(item.created_at, 1000);
        assert_eq!(item.updated_at, 1000);
    }

    #[test]
    fn test_item_record_builder_custom() {
        let item = ItemRecordBuilder::new("sword_01")
            .character_guid(2002)
            .container_id(7001)
            .item_def_id(42)
            .slot(5)
            .quantity(2)
            .durability(80)
            .attributes(serde_json::json!({"power": 10}))
            .created_at(1100)
            .updated_at(1200)
            .build();
        assert_eq!(item.item_instance_id, "sword_01");
        assert_eq!(item.character_guid, 2002);
        assert_eq!(item.container_id, 7001);
        assert_eq!(item.item_def_id, 42);
        assert_eq!(item.slot, 5);
        assert_eq!(item.quantity, 2);
        assert_eq!(item.durability, 80);
        assert_eq!(item.attributes, serde_json::json!({"power": 10}));
        assert_eq!(item.created_at, 1100);
        assert_eq!(item.updated_at, 1200);
    }

    #[tokio::test]
    async fn test_connect_test_databases_fallback_sqlite() {
        let dbs = connect_test_databases_impl(None, false).await.unwrap();
        assert_eq!(dbs.len(), 1);
        assert_eq!(dbs[0].0, "sqlite");
        assert!(dbs[0].1.ping().await.is_ok());
    }

    #[tokio::test]
    #[should_panic(
        expected = "REQUIRE_POSTGRES_TEST=1 is set, but TEST_POSTGRES_URL is missing or empty"
    )]
    async fn test_connect_test_databases_require_pg_missing_panics() {
        let _ = connect_test_databases_impl(None, true).await;
    }

    #[tokio::test]
    #[should_panic(
        expected = "REQUIRE_POSTGRES_TEST=1 is set, but TEST_POSTGRES_URL is missing or empty"
    )]
    async fn test_connect_test_databases_require_pg_empty_url_panics() {
        let _ = connect_test_databases_impl(Some("   ".to_string()), true).await;
    }
}
