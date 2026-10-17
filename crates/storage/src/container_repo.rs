//! Container storage repository for managing arbitrary storage containers.

use crate::db::{current_time_secs, Database, DbBackend, StorageError};
use crate::models::{ContainerRecord, ItemRecord};
use sqlx::Row;

macro_rules! parse_container_record {
    ($r:expr) => {{
        let container_id: i64 = $r.try_get("container_id")?;
        let owner_type: String = $r.try_get("owner_type")?;
        let owner_id: String = $r.try_get("owner_id")?;
        let name: String = $r.try_get("name")?;
        let capacity: i32 = $r.try_get("capacity")?;
        let access_policy: String = $r.try_get("access_policy")?;
        let version: i64 = $r.try_get("version")?;
        let created_at: i64 = $r.try_get("created_at")?;
        let updated_at: i64 = $r.try_get("updated_at")?;

        ContainerRecord {
            container_id: container_id as u64,
            owner_type,
            owner_id,
            name,
            capacity: capacity as u32,
            access_policy,
            version: version as u64,
            created_at,
            updated_at,
        }
    }};
}

/// Repository for persistent container management.
#[derive(Clone)]
pub struct ContainerRepository {
    db: Database,
}

impl ContainerRepository {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Creates a new container record.
    pub async fn create_container(&self, container: &ContainerRecord) -> Result<(), StorageError> {
        let query = r#"
            INSERT INTO containers (container_id, owner_type, owner_id, name, capacity, access_policy, version, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#;
        let query_str = self.db.sql(query);

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                sqlx::query(&query_str)
                    .bind(container.container_id as i64)
                    .bind(&container.owner_type)
                    .bind(&container.owner_id)
                    .bind(&container.name)
                    .bind(container.capacity as i32)
                    .bind(&container.access_policy)
                    .bind(container.version as i64)
                    .bind(container.created_at)
                    .bind(container.updated_at)
                    .execute(pool)
                    .await?;
            }
            DbBackend::Postgres(pool) => {
                sqlx::query(&query_str)
                    .bind(container.container_id as i64)
                    .bind(&container.owner_type)
                    .bind(&container.owner_id)
                    .bind(&container.name)
                    .bind(container.capacity as i32)
                    .bind(&container.access_policy)
                    .bind(container.version as i64)
                    .bind(container.created_at)
                    .bind(container.updated_at)
                    .execute(pool)
                    .await?;
            }
        }

        Ok(())
    }

    /// Retrieves a container by its unique ID.
    pub async fn get_container(
        &self,
        container_id: u64,
    ) -> Result<Option<ContainerRecord>, StorageError> {
        let query = "SELECT container_id, owner_type, owner_id, name, capacity, access_policy, version, created_at, updated_at FROM containers WHERE container_id = ?";
        let query_str = self.db.sql(query);

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                let row = sqlx::query(&query_str)
                    .bind(container_id as i64)
                    .fetch_optional(pool)
                    .await?;

                match row {
                    Some(r) => Ok(Some(parse_container_record!(r))),
                    None => Ok(None),
                }
            }
            DbBackend::Postgres(pool) => {
                let row = sqlx::query(&query_str)
                    .bind(container_id as i64)
                    .fetch_optional(pool)
                    .await?;

                match row {
                    Some(r) => Ok(Some(parse_container_record!(r))),
                    None => Ok(None),
                }
            }
        }
    }

    /// Retrieves all items residing inside a given container.
    pub async fn get_container_items(
        &self,
        container_id: u64,
    ) -> Result<Vec<ItemRecord>, StorageError> {
        let query = "SELECT item_instance_id, character_guid, container_id, item_def_id, slot, quantity, durability, attributes_json, created_at, updated_at FROM items WHERE container_id = ? ORDER BY slot ASC";
        let query_str = self.db.sql(query);

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                let rows = sqlx::query(&query_str)
                    .bind(container_id as i64)
                    .fetch_all(pool)
                    .await?;

                let mut items = Vec::with_capacity(rows.len());
                for r in rows {
                    items.push(crate::parse_item_record!(r));
                }
                Ok(items)
            }
            DbBackend::Postgres(pool) => {
                let rows = sqlx::query(&query_str)
                    .bind(container_id as i64)
                    .fetch_all(pool)
                    .await?;

                let mut items = Vec::with_capacity(rows.len());
                for r in rows {
                    items.push(crate::parse_item_record!(r));
                }
                Ok(items)
            }
        }
    }

    /// Retrieves the item at a specific slot in a container, if any.
    pub async fn get_item_at_slot(
        &self,
        container_id: u64,
        slot: u32,
    ) -> Result<Option<ItemRecord>, StorageError> {
        let query = "SELECT item_instance_id, character_guid, container_id, item_def_id, slot, quantity, durability, attributes_json, created_at, updated_at FROM items WHERE container_id = ? AND slot = ?";
        let query_str = self.db.sql(query);

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                let row = sqlx::query(&query_str)
                    .bind(container_id as i64)
                    .bind(slot as i32)
                    .fetch_optional(pool)
                    .await?;

                match row {
                    Some(r) => Ok(Some(crate::parse_item_record!(r))),
                    None => Ok(None),
                }
            }
            DbBackend::Postgres(pool) => {
                let row = sqlx::query(&query_str)
                    .bind(container_id as i64)
                    .bind(slot as i32)
                    .fetch_optional(pool)
                    .await?;

                match row {
                    Some(r) => Ok(Some(crate::parse_item_record!(r))),
                    None => Ok(None),
                }
            }
        }
    }

    /// Retrieves an item by its unique instance ID across all containers.
    pub async fn get_item(
        &self,
        item_instance_id: &str,
    ) -> Result<Option<ItemRecord>, StorageError> {
        let query = "SELECT item_instance_id, character_guid, container_id, item_def_id, slot, quantity, durability, attributes_json, created_at, updated_at FROM items WHERE item_instance_id = ?";
        let query_str = self.db.sql(query);

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                let row = sqlx::query(&query_str)
                    .bind(item_instance_id)
                    .fetch_optional(pool)
                    .await?;

                match row {
                    Some(r) => Ok(Some(crate::parse_item_record!(r))),
                    None => Ok(None),
                }
            }
            DbBackend::Postgres(pool) => {
                let row = sqlx::query(&query_str)
                    .bind(item_instance_id)
                    .fetch_optional(pool)
                    .await?;

                match row {
                    Some(r) => Ok(Some(crate::parse_item_record!(r))),
                    None => Ok(None),
                }
            }
        }
    }

    /// Updates container capacity.
    pub async fn set_container_capacity(
        &self,
        container_id: u64,
        new_capacity: u32,
    ) -> Result<(), StorageError> {
        let now = current_time_secs();

        let query = "UPDATE containers SET capacity = ?, updated_at = ? WHERE container_id = ?";
        let query_str = self.db.sql(query);

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                sqlx::query(&query_str)
                    .bind(new_capacity as i32)
                    .bind(now)
                    .bind(container_id as i64)
                    .execute(pool)
                    .await?;
            }
            DbBackend::Postgres(pool) => {
                sqlx::query(&query_str)
                    .bind(new_capacity as i32)
                    .bind(now)
                    .bind(container_id as i64)
                    .execute(pool)
                    .await?;
            }
        }

        Ok(())
    }

    /// Deletes a container and cascades to reservations and escrows.
    pub async fn delete_container(&self, container_id: u64) -> Result<bool, StorageError> {
        let query = "DELETE FROM containers WHERE container_id = ?";
        let query_str = self.db.sql(query);

        let rows_affected = match self.db.backend() {
            DbBackend::Sqlite(pool) => sqlx::query(&query_str)
                .bind(container_id as i64)
                .execute(pool)
                .await?
                .rows_affected(),
            DbBackend::Postgres(pool) => sqlx::query(&query_str)
                .bind(container_id as i64)
                .execute(pool)
                .await?
                .rows_affected(),
        };

        Ok(rows_affected > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::CharacterRepository;

    #[tokio::test]
    async fn test_container_crud_lifecycle() {
        let db = Database::connect_memory().await.unwrap();
        let container_repo = ContainerRepository::new(db.clone());
        let char_repo = CharacterRepository::new(db);

        // 1. Create container
        let container = ContainerRecord {
            container_id: 5001,
            owner_type: "character".to_string(),
            owner_id: "1001".to_string(),
            name: "Personal Vault".to_string(),
            capacity: 20,
            access_policy: "owner_only".to_string(),
            version: 1,
            created_at: 1000,
            updated_at: 1000,
        };
        container_repo.create_container(&container).await.unwrap();

        // 2. Fetch container
        let fetched = container_repo.get_container(5001).await.unwrap().unwrap();
        assert_eq!(fetched.container_id, 5001);
        assert_eq!(fetched.capacity, 20);
        assert_eq!(fetched.name, "Personal Vault");

        // 3. Add items into container
        let item = ItemRecord {
            item_instance_id: "vault_item_1".to_string(),
            character_guid: 1001,
            container_id: 5001,
            item_def_id: 99,
            slot: 0,
            quantity: 10,
            durability: 100,
            attributes: serde_json::json!({}),
            created_at: 1000,
            updated_at: 1000,
        };
        char_repo.add_item(&item).await.unwrap();

        // 4. Retrieve container items
        let items = container_repo.get_container_items(5001).await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].item_instance_id, "vault_item_1");

        let slot_item = container_repo
            .get_item_at_slot(5001, 0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(slot_item.item_instance_id, "vault_item_1");

        let fetched_item = container_repo
            .get_item("vault_item_1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched_item.item_instance_id, "vault_item_1");
        assert!(container_repo
            .get_item("non_existent")
            .await
            .unwrap()
            .is_none());

        let empty_slot = container_repo.get_item_at_slot(5001, 1).await.unwrap();
        assert!(empty_slot.is_none());

        // 5. Update capacity
        container_repo
            .set_container_capacity(5001, 30)
            .await
            .unwrap();
        let updated = container_repo.get_container(5001).await.unwrap().unwrap();
        assert_eq!(updated.capacity, 30);

        // 6. Delete container
        let deleted = container_repo.delete_container(5001).await.unwrap();
        assert!(deleted);
        assert!(container_repo.get_container(5001).await.unwrap().is_none());

        // Deleting non-existent container returns false
        assert!(!container_repo.delete_container(9999).await.unwrap());
    }
}
