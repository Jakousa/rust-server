//! Character and Entity repository providing CRUD and snapshot persistence operations.

use crate::db::{Database, StorageError};
use crate::models::{AccountRecord, CharacterRecord, EntitySnapshot};
use sqlx::Row;

/// Repository for accounts, characters, and entity snapshots.
#[derive(Clone)]
pub struct CharacterRepository {
    db: Database,
}

impl CharacterRepository {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Creates a new player account.
    pub async fn create_account(
        &self,
        account_id: &str,
        username: &str,
    ) -> Result<AccountRecord, StorageError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        sqlx::query("INSERT INTO accounts (account_id, username, created_at) VALUES (?, ?, ?)")
            .bind(account_id)
            .bind(username)
            .bind(now)
            .execute(self.db.pool())
            .await?;

        Ok(AccountRecord {
            account_id: account_id.to_string(),
            username: username.to_string(),
            created_at: now,
        })
    }

    /// Creates a new character belonging to an account.
    pub async fn create_character(
        &self,
        account_id: &str,
        guid: u64,
        name: &str,
        pos: (f32, f32, f32),
    ) -> Result<CharacterRecord, StorageError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let empty_json = "{}".to_string();

        sqlx::query(
            r#"
            INSERT INTO characters (guid, account_id, name, level, pos_x, pos_y, pos_z, rotation, attributes_json, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#
        )
        .bind(guid as i64)
        .bind(account_id)
        .bind(name)
        .bind(1i32)
        .bind(pos.0)
        .bind(pos.1)
        .bind(pos.2)
        .bind(0.0f32)
        .bind(&empty_json)
        .bind(now)
        .execute(self.db.pool())
        .await?;

        Ok(CharacterRecord {
            guid,
            account_id: account_id.to_string(),
            name: name.to_string(),
            level: 1,
            position: pos,
            rotation: 0.0,
            attributes: serde_json::json!({}),
            updated_at: now,
        })
    }

    /// Loads a character record by GUID.
    pub async fn get_character(&self, guid: u64) -> Result<Option<CharacterRecord>, StorageError> {
        let row = sqlx::query(
            "SELECT guid, account_id, name, level, pos_x, pos_y, pos_z, rotation, attributes_json, updated_at FROM characters WHERE guid = ?"
        )
        .bind(guid as i64)
        .fetch_optional(self.db.pool())
        .await?;

        if let Some(r) = row {
            let guid_i64: i64 = r.try_get("guid")?;
            let account_id: String = r.try_get("account_id")?;
            let name: String = r.try_get("name")?;
            let level: i32 = r.try_get("level")?;
            let x: f32 = r.try_get("pos_x")?;
            let y: f32 = r.try_get("pos_y")?;
            let z: f32 = r.try_get("pos_z")?;
            let rotation: f32 = r.try_get("rotation")?;
            let attrs_str: String = r.try_get("attributes_json")?;
            let updated_at: i64 = r.try_get("updated_at")?;

            let attributes: serde_json::Value = serde_json::from_str(&attrs_str)?;

            Ok(Some(CharacterRecord {
                guid: guid_i64 as u64,
                account_id,
                name,
                level,
                position: (x, y, z),
                rotation,
                attributes,
                updated_at,
            }))
        } else {
            Ok(None)
        }
    }

    /// Updates the persistent coordinates and rotation of a character.
    pub async fn update_character_position(
        &self,
        guid: u64,
        pos: (f32, f32, f32),
        rotation: f32,
    ) -> Result<bool, StorageError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let result = sqlx::query(
            "UPDATE characters SET pos_x = ?, pos_y = ?, pos_z = ?, rotation = ?, updated_at = ? WHERE guid = ?"
        )
        .bind(pos.0)
        .bind(pos.1)
        .bind(pos.2)
        .bind(rotation)
        .bind(now)
        .bind(guid as i64)
        .execute(self.db.pool())
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Saves or updates an entity snapshot in the database.
    pub async fn save_snapshot(&self, snapshot: &EntitySnapshot) -> Result<(), StorageError> {
        let attrs_str = serde_json::to_string(&snapshot.attributes)?;

        sqlx::query(
            r#"
            INSERT INTO entity_snapshots (guid, pos_x, pos_y, pos_z, rotation, attributes_json, version, timestamp)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(guid) DO UPDATE SET
                pos_x = excluded.pos_x,
                pos_y = excluded.pos_y,
                pos_z = excluded.pos_z,
                rotation = excluded.rotation,
                attributes_json = excluded.attributes_json,
                version = excluded.version,
                timestamp = excluded.timestamp
            WHERE excluded.version >= entity_snapshots.version
            "#
        )
        .bind(snapshot.guid as i64)
        .bind(snapshot.position.0)
        .bind(snapshot.position.1)
        .bind(snapshot.position.2)
        .bind(snapshot.rotation)
        .bind(&attrs_str)
        .bind(snapshot.version as i64)
        .bind(snapshot.timestamp as i64)
        .execute(self.db.pool())
        .await?;

        // Synchronize coordinates to characters table if this entity is a player character
        let _ = sqlx::query(
            r#"
            UPDATE characters
            SET pos_x = ?, pos_y = ?, pos_z = ?, rotation = ?, updated_at = ?
            WHERE guid = ?
            "#
        )
        .bind(snapshot.position.0)
        .bind(snapshot.position.1)
        .bind(snapshot.position.2)
        .bind(snapshot.rotation)
        .bind(snapshot.timestamp as i64)
        .bind(snapshot.guid as i64)
        .execute(self.db.pool())
        .await;

        Ok(())
    }

    /// Batch saves a slice of entity snapshots inside a single transaction.
    pub async fn save_snapshot_batch(&self, snapshots: &[EntitySnapshot]) -> Result<usize, StorageError> {
        if snapshots.is_empty() {
            return Ok(0);
        }

        let mut tx = self.db.pool().begin().await?;

        for snap in snapshots {
            let attrs_str = serde_json::to_string(&snap.attributes)?;
            sqlx::query(
                r#"
                INSERT INTO entity_snapshots (guid, pos_x, pos_y, pos_z, rotation, attributes_json, version, timestamp)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(guid) DO UPDATE SET
                    pos_x = excluded.pos_x,
                    pos_y = excluded.pos_y,
                    pos_z = excluded.pos_z,
                    rotation = excluded.rotation,
                    attributes_json = excluded.attributes_json,
                    version = excluded.version,
                    timestamp = excluded.timestamp
                WHERE excluded.version >= entity_snapshots.version
                "#
            )
            .bind(snap.guid as i64)
            .bind(snap.position.0)
            .bind(snap.position.1)
            .bind(snap.position.2)
            .bind(snap.rotation)
            .bind(&attrs_str)
            .bind(snap.version as i64)
            .bind(snap.timestamp as i64)
            .execute(&mut *tx)
            .await?;

            // Also synchronize position to characters table if this entity is a player character
            sqlx::query(
                r#"
                UPDATE characters
                SET pos_x = ?, pos_y = ?, pos_z = ?, rotation = ?, updated_at = ?
                WHERE guid = ?
                "#
            )
            .bind(snap.position.0)
            .bind(snap.position.1)
            .bind(snap.position.2)
            .bind(snap.rotation)
            .bind(snap.timestamp as i64)
            .bind(snap.guid as i64)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(snapshots.len())
    }

    /// Retrieves an entity snapshot by GUID.
    pub async fn get_snapshot(&self, guid: u64) -> Result<Option<EntitySnapshot>, StorageError> {
        let row = sqlx::query(
            "SELECT guid, pos_x, pos_y, pos_z, rotation, attributes_json, version, timestamp FROM entity_snapshots WHERE guid = ?"
        )
        .bind(guid as i64)
        .fetch_optional(self.db.pool())
        .await?;

        if let Some(r) = row {
            let guid_i64: i64 = r.try_get("guid")?;
            let x: f32 = r.try_get("pos_x")?;
            let y: f32 = r.try_get("pos_y")?;
            let z: f32 = r.try_get("pos_z")?;
            let rotation: f32 = r.try_get("rotation")?;
            let attrs_str: String = r.try_get("attributes_json")?;
            let version: i64 = r.try_get("version")?;
            let timestamp: i64 = r.try_get("timestamp")?;

            let attributes: serde_json::Value = serde_json::from_str(&attrs_str)?;

            Ok(Some(EntitySnapshot {
                guid: guid_i64 as u64,
                position: (x, y, z),
                rotation,
                attributes,
                version: version as u64,
                timestamp: timestamp as u64,
            }))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_character_crud_lifecycle() {
        let db = Database::connect_memory().await.unwrap();
        let repo = CharacterRepository::new(db);

        // 1. Create account
        let acc = repo.create_account("acc_1", "hero_user").await.unwrap();
        assert_eq!(acc.account_id, "acc_1");

        // 2. Create character
        let char_record = repo
            .create_character("acc_1", 1001, "Paladin", (10.0, 0.0, 20.0))
            .await
            .unwrap();
        assert_eq!(char_record.name, "Paladin");

        // 3. Retrieve character
        let fetched = repo.get_character(1001).await.unwrap().unwrap();
        assert_eq!(fetched.position, (10.0, 0.0, 20.0));

        // 4. Update coordinates
        let updated = repo.update_character_position(1001, (15.0, 1.0, 25.0), 1.57).await.unwrap();
        assert!(updated);

        let refetched = repo.get_character(1001).await.unwrap().unwrap();
        assert_eq!(refetched.position, (15.0, 1.0, 25.0));
        assert_eq!(refetched.rotation, 1.57);
    }

    #[tokio::test]
    async fn test_entity_snapshot_persistence_and_batch() {
        let db = Database::connect_memory().await.unwrap();
        let repo = CharacterRepository::new(db);

        let snap1 = EntitySnapshot {
            guid: 5001,
            position: (1.0, 2.0, 3.0),
            rotation: 0.0,
            attributes: serde_json::json!({"health": 100}),
            version: 1,
            timestamp: 1000,
        };

        let snap2 = EntitySnapshot {
            guid: 5002,
            position: (4.0, 5.0, 6.0),
            rotation: 3.14,
            attributes: serde_json::json!({"mana": 50}),
            version: 1,
            timestamp: 1000,
        };

        let count = repo.save_snapshot_batch(&[snap1.clone(), snap2]).await.unwrap();
        assert_eq!(count, 2);

        let loaded = repo.get_snapshot(5001).await.unwrap().unwrap();
        assert_eq!(loaded.position, (1.0, 2.0, 3.0));
        assert_eq!(loaded.attributes["health"], 100);

        // Update with higher version
        let snap1_v2 = EntitySnapshot {
            guid: 5001,
            position: (10.0, 20.0, 30.0),
            rotation: 1.0,
            attributes: serde_json::json!({"health": 85}),
            version: 2,
            timestamp: 2000,
        };
        repo.save_snapshot(&snap1_v2).await.unwrap();

        let loaded_v2 = repo.get_snapshot(5001).await.unwrap().unwrap();
        assert_eq!(loaded_v2.position, (10.0, 20.0, 30.0));
        assert_eq!(loaded_v2.version, 2);
    }
}
