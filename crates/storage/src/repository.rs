//! Character, Inventory, and Entity repository providing CRUD, inventory management,
//! ACID player-to-player trading, and snapshot persistence operations.

use crate::db::{compute_free_slots, current_time_secs, Database, DbBackend, StorageError};
use crate::models::{
    AccountRecord, CharacterRecord, EntitySnapshot, InventoryRecord, ItemRecord, TradeRecord,
    TradeStatus,
};
use crate::with_dialect_tx;
use sqlx::Row;

macro_rules! parse_character_record {
    ($r:expr) => {{
        let guid_i64: i64 = $r.try_get("guid")?;
        let account_id: String = $r.try_get("account_id")?;
        let name: String = $r.try_get("name")?;
        let level: i32 = $r.try_get("level")?;
        let x: f32 = $r.try_get("pos_x")?;
        let y: f32 = $r.try_get("pos_y")?;
        let z: f32 = $r.try_get("pos_z")?;
        let rotation: f32 = $r.try_get("rotation")?;
        let attrs_str: String = $r.try_get("attributes_json")?;
        let updated_at: i64 = $r.try_get("updated_at")?;
        let attributes: serde_json::Value = serde_json::from_str(&attrs_str)?;

        CharacterRecord {
            guid: guid_i64 as u64,
            account_id,
            name,
            level,
            position: (x, y, z),
            rotation,
            attributes,
            updated_at,
        }
    }};
}

#[macro_export]
macro_rules! parse_item_record {
    ($r:expr) => {{
        let item_instance_id: String = $r.try_get("item_instance_id")?;
        let character_guid: i64 = $r.try_get("character_guid")?;
        let container_id: i64 = $r.try_get("container_id").unwrap_or(character_guid);
        let item_def_id: i32 = $r.try_get("item_def_id")?;
        let slot: i32 = $r.try_get("slot")?;
        let quantity: i32 = $r.try_get("quantity")?;
        let durability: i32 = $r.try_get("durability")?;
        let attrs_str: String = $r.try_get("attributes_json")?;
        let created_at: i64 = $r.try_get("created_at")?;
        let updated_at: i64 = $r.try_get("updated_at")?;
        let attributes: serde_json::Value = serde_json::from_str(&attrs_str)?;

        ItemRecord {
            item_instance_id,
            character_guid: character_guid as u64,
            container_id: if container_id == 0 {
                character_guid as u64
            } else {
                container_id as u64
            },
            item_def_id: item_def_id as u32,
            slot: slot as u32,
            quantity: quantity as u32,
            durability: durability as u32,
            attributes,
            created_at,
            updated_at,
        }
    }};
}

/// Repository for accounts, characters, inventories, items, trades, and entity snapshots.
#[derive(Clone)]
pub struct CharacterRepository {
    db: Database,
}

pub type StorageRepository = CharacterRepository;

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
        let now = current_time_secs();
        let query_str = self
            .db
            .sql("INSERT INTO accounts (account_id, username, created_at) VALUES (?, ?, ?)");

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                sqlx::query(&query_str)
                    .bind(account_id)
                    .bind(username)
                    .bind(now)
                    .execute(pool)
                    .await?;
            }
            DbBackend::Postgres(pool) => {
                sqlx::query(&query_str)
                    .bind(account_id)
                    .bind(username)
                    .bind(now)
                    .execute(pool)
                    .await?;
            }
        }

        Ok(AccountRecord {
            account_id: account_id.to_string(),
            username: username.to_string(),
            created_at: now,
        })
    }

    /// Creates a new character belonging to an account, initializing an empty inventory.
    pub async fn create_character(
        &self,
        account_id: &str,
        guid: u64,
        name: &str,
        pos: (f32, f32, f32),
    ) -> Result<CharacterRecord, StorageError> {
        let now = current_time_secs();
        let empty_json = "{}".to_string();

        let q1 = self.db.sql(
            r#"
            INSERT INTO characters (guid, account_id, name, level, pos_x, pos_y, pos_z, rotation, attributes_json, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        );
        let q2 = self.db.sql(
            r#"
            INSERT INTO inventories (character_guid, capacity, updated_at)
            VALUES (?, ?, ?)
            ON CONFLICT(character_guid) DO NOTHING
            "#,
        );

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                sqlx::query(&q1)
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
                    .execute(pool)
                    .await?;

                sqlx::query(&q2)
                    .bind(guid as i64)
                    .bind(30i32)
                    .bind(now)
                    .execute(pool)
                    .await?;
            }
            DbBackend::Postgres(pool) => {
                sqlx::query(&q1)
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
                    .execute(pool)
                    .await?;

                sqlx::query(&q2)
                    .bind(guid as i64)
                    .bind(30i32)
                    .bind(now)
                    .execute(pool)
                    .await?;
            }
        }

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
        let q = self.db.sql(
            "SELECT guid, account_id, name, level, pos_x, pos_y, pos_z, rotation, attributes_json, updated_at FROM characters WHERE guid = ?",
        );

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                let row = sqlx::query(&q)
                    .bind(guid as i64)
                    .fetch_optional(pool)
                    .await?;

                if let Some(r) = row {
                    Ok(Some(parse_character_record!(r)))
                } else {
                    Ok(None)
                }
            }
            DbBackend::Postgres(pool) => {
                let row = sqlx::query(&q)
                    .bind(guid as i64)
                    .fetch_optional(pool)
                    .await?;

                if let Some(r) = row {
                    Ok(Some(parse_character_record!(r)))
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Updates the persistent coordinates and rotation of a character.
    pub async fn update_character_position(
        &self,
        guid: u64,
        pos: (f32, f32, f32),
        rotation: f32,
    ) -> Result<bool, StorageError> {
        let now = current_time_secs();
        let q = self.db.sql(
            "UPDATE characters SET pos_x = ?, pos_y = ?, pos_z = ?, rotation = ?, updated_at = ? WHERE guid = ?",
        );

        let rows_affected = match self.db.backend() {
            DbBackend::Sqlite(pool) => sqlx::query(&q)
                .bind(pos.0)
                .bind(pos.1)
                .bind(pos.2)
                .bind(rotation)
                .bind(now)
                .bind(guid as i64)
                .execute(pool)
                .await?
                .rows_affected(),
            DbBackend::Postgres(pool) => sqlx::query(&q)
                .bind(pos.0)
                .bind(pos.1)
                .bind(pos.2)
                .bind(rotation)
                .bind(now)
                .bind(guid as i64)
                .execute(pool)
                .await?
                .rows_affected(),
        };

        Ok(rows_affected > 0)
    }

    /// Saves or updates an entity snapshot in the database.
    pub async fn save_snapshot(&self, snapshot: &EntitySnapshot) -> Result<(), StorageError> {
        self.save_snapshot_batch(std::slice::from_ref(snapshot))
            .await?;
        Ok(())
    }

    /// Batch saves a slice of entity snapshots inside a transaction using batched multi-row execution.
    pub async fn save_snapshot_batch(
        &self,
        snapshots: &[EntitySnapshot],
    ) -> Result<usize, StorageError> {
        if snapshots.is_empty() {
            return Ok(0);
        }

        with_dialect_tx!(&self.db, |conn, is_pg| async move {
            let conflict_sql = q!(r#"
                ON CONFLICT(guid) DO UPDATE SET
                    pos_x = excluded.pos_x,
                    pos_y = excluded.pos_y,
                    pos_z = excluded.pos_z,
                    rotation = excluded.rotation,
                    attributes_json = excluded.attributes_json,
                    state_blob = excluded.state_blob,
                    version = excluded.version,
                    timestamp = excluded.timestamp
                WHERE excluded.version >= entity_snapshots.version
                "#);
            for chunk in snapshots.chunks(50) {
                // A multi-row upsert cannot touch the same key twice (PostgreSQL rejects it),
                // so collapse duplicate GUIDs: highest version wins, later entry wins ties.
                let mut latest: std::collections::HashMap<u64, usize> =
                    std::collections::HashMap::with_capacity(chunk.len());
                for (i, snap) in chunk.iter().enumerate() {
                    match latest.get(&snap.guid) {
                        Some(&j) if chunk[j].version > snap.version => {}
                        _ => {
                            latest.insert(snap.guid, i);
                        }
                    }
                }
                let rows: Vec<(&EntitySnapshot, String)> = chunk
                    .iter()
                    .enumerate()
                    .filter(|(i, s)| latest.get(&s.guid) == Some(i))
                    .map(|(_, s)| Ok((s, serde_json::to_string(&s.attributes)?)))
                    .collect::<Result<_, StorageError>>()?;

                let mut qb = sqlx::QueryBuilder::new(
                    "INSERT INTO entity_snapshots (guid, pos_x, pos_y, pos_z, rotation, attributes_json, state_blob, version, timestamp) ",
                );
                qb.push_values(rows.iter(), |mut b, (snap, attrs_str)| {
                    b.push_bind(snap.guid as i64)
                        .push_bind(snap.position.0)
                        .push_bind(snap.position.1)
                        .push_bind(snap.position.2)
                        .push_bind(snap.rotation)
                        .push_bind(attrs_str.as_str())
                        .push_bind(snap.state_blob.as_deref())
                        .push_bind(snap.version as i64)
                        .push_bind(snap.timestamp as i64);
                });
                qb.push(" ");
                qb.push(conflict_sql.as_ref());
                qb.build().execute(&mut *conn).await?;
            }

            Ok(snapshots.len())
        })
    }

    /// Retrieves an entity snapshot by GUID.
    pub async fn get_snapshot(&self, guid: u64) -> Result<Option<EntitySnapshot>, StorageError> {
        let q = self.db.sql(
            "SELECT guid, pos_x, pos_y, pos_z, rotation, attributes_json, state_blob, version, timestamp FROM entity_snapshots WHERE guid = ?",
        );

        let (x, y, z, rotation, attrs_str, state_blob, version, timestamp) = match self.db.backend()
        {
            DbBackend::Sqlite(pool) => {
                let row = sqlx::query(&q)
                    .bind(guid as i64)
                    .fetch_optional(pool)
                    .await?;

                if let Some(r) = row {
                    (
                        r.try_get::<f32, _>("pos_x")?,
                        r.try_get::<f32, _>("pos_y")?,
                        r.try_get::<f32, _>("pos_z")?,
                        r.try_get::<f32, _>("rotation")?,
                        r.try_get::<String, _>("attributes_json")?,
                        r.try_get::<Option<Vec<u8>>, _>("state_blob")?,
                        r.try_get::<i64, _>("version")?,
                        r.try_get::<i64, _>("timestamp")?,
                    )
                } else {
                    return Ok(None);
                }
            }
            DbBackend::Postgres(pool) => {
                let row = sqlx::query(&q)
                    .bind(guid as i64)
                    .fetch_optional(pool)
                    .await?;

                if let Some(r) = row {
                    (
                        r.try_get::<f32, _>("pos_x")?,
                        r.try_get::<f32, _>("pos_y")?,
                        r.try_get::<f32, _>("pos_z")?,
                        r.try_get::<f32, _>("rotation")?,
                        r.try_get::<String, _>("attributes_json")?,
                        r.try_get::<Option<Vec<u8>>, _>("state_blob")?,
                        r.try_get::<i64, _>("version")?,
                        r.try_get::<i64, _>("timestamp")?,
                    )
                } else {
                    return Ok(None);
                }
            }
        };

        let attributes: serde_json::Value = serde_json::from_str(&attrs_str)?;

        Ok(Some(EntitySnapshot {
            guid,
            position: (x, y, z),
            rotation,
            attributes,
            state_blob,
            version: version as u64,
            timestamp: timestamp as u64,
        }))
    }

    /// Alias for `get_snapshot`.
    pub async fn load_snapshot(&self, guid: u64) -> Result<Option<EntitySnapshot>, StorageError> {
        self.get_snapshot(guid).await
    }

    /// Retrieves character inventory metadata.
    pub async fn get_inventory(
        &self,
        character_guid: u64,
    ) -> Result<Option<InventoryRecord>, StorageError> {
        let q = self.db.sql(
            "SELECT character_guid, capacity, updated_at FROM inventories WHERE character_guid = ?",
        );

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                let row = sqlx::query(&q)
                    .bind(character_guid as i64)
                    .fetch_optional(pool)
                    .await?;

                if let Some(r) = row {
                    let guid: i64 = r.try_get("character_guid")?;
                    let cap: i32 = r.try_get("capacity")?;
                    let updated: i64 = r.try_get("updated_at")?;
                    Ok(Some(InventoryRecord {
                        character_guid: guid as u64,
                        capacity: cap as u32,
                        updated_at: updated,
                    }))
                } else {
                    Ok(None)
                }
            }
            DbBackend::Postgres(pool) => {
                let row = sqlx::query(&q)
                    .bind(character_guid as i64)
                    .fetch_optional(pool)
                    .await?;

                if let Some(r) = row {
                    let guid: i64 = r.try_get("character_guid")?;
                    let cap: i32 = r.try_get("capacity")?;
                    let updated: i64 = r.try_get("updated_at")?;
                    Ok(Some(InventoryRecord {
                        character_guid: guid as u64,
                        capacity: cap as u32,
                        updated_at: updated,
                    }))
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Sets inventory capacity for a character.
    pub async fn set_inventory_capacity(
        &self,
        character_guid: u64,
        capacity: u32,
    ) -> Result<(), StorageError> {
        let now = current_time_secs();
        let q = self.db.sql(
            r#"
            INSERT INTO inventories (character_guid, capacity, updated_at)
            VALUES (?, ?, ?)
            ON CONFLICT(character_guid) DO UPDATE SET
                capacity = excluded.capacity,
                updated_at = excluded.updated_at
            "#,
        );

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                sqlx::query(&q)
                    .bind(character_guid as i64)
                    .bind(capacity as i32)
                    .bind(now)
                    .execute(pool)
                    .await?;
            }
            DbBackend::Postgres(pool) => {
                sqlx::query(&q)
                    .bind(character_guid as i64)
                    .bind(capacity as i32)
                    .bind(now)
                    .execute(pool)
                    .await?;
            }
        }

        Ok(())
    }

    /// Adds an item instance to a character's inventory slot.
    pub async fn add_item(&self, item: &ItemRecord) -> Result<(), StorageError> {
        let attrs_str = serde_json::to_string(&item.attributes)?;
        let container_id = if item.container_id == 0 {
            item.character_guid
        } else {
            item.container_id
        };
        let q = self.db.sql(
            r#"
            INSERT INTO items (item_instance_id, character_guid, container_id, item_def_id, slot, quantity, durability, attributes_json, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        );

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                sqlx::query(&q)
                    .bind(&item.item_instance_id)
                    .bind(item.character_guid as i64)
                    .bind(container_id as i64)
                    .bind(item.item_def_id as i32)
                    .bind(item.slot as i32)
                    .bind(item.quantity as i32)
                    .bind(item.durability as i32)
                    .bind(&attrs_str)
                    .bind(item.created_at)
                    .bind(item.updated_at)
                    .execute(pool)
                    .await?;
            }
            DbBackend::Postgres(pool) => {
                sqlx::query(&q)
                    .bind(&item.item_instance_id)
                    .bind(item.character_guid as i64)
                    .bind(container_id as i64)
                    .bind(item.item_def_id as i32)
                    .bind(item.slot as i32)
                    .bind(item.quantity as i32)
                    .bind(item.durability as i32)
                    .bind(&attrs_str)
                    .bind(item.created_at)
                    .bind(item.updated_at)
                    .execute(pool)
                    .await?;
            }
        }

        Ok(())
    }

    /// Retrieves an item by its unique instance ID.
    pub async fn get_item(
        &self,
        item_instance_id: &str,
    ) -> Result<Option<ItemRecord>, StorageError> {
        let q = self.db.sql(
            "SELECT item_instance_id, character_guid, container_id, item_def_id, slot, quantity, durability, attributes_json, created_at, updated_at FROM items WHERE item_instance_id = ?",
        );

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                let row = sqlx::query(&q)
                    .bind(item_instance_id)
                    .fetch_optional(pool)
                    .await?;

                if let Some(r) = row {
                    Ok(Some(parse_item_record!(r)))
                } else {
                    Ok(None)
                }
            }
            DbBackend::Postgres(pool) => {
                let row = sqlx::query(&q)
                    .bind(item_instance_id)
                    .fetch_optional(pool)
                    .await?;

                if let Some(r) = row {
                    Ok(Some(parse_item_record!(r)))
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Retrieves all items belonging to a character, ordered by slot index.
    pub async fn get_character_items(
        &self,
        character_guid: u64,
    ) -> Result<Vec<ItemRecord>, StorageError> {
        let q = self.db.sql(
            "SELECT item_instance_id, character_guid, container_id, item_def_id, slot, quantity, durability, attributes_json, created_at, updated_at FROM items WHERE character_guid = ? ORDER BY slot ASC",
        );

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                let rows = sqlx::query(&q)
                    .bind(character_guid as i64)
                    .fetch_all(pool)
                    .await?;

                let mut items = Vec::with_capacity(rows.len());
                for r in rows {
                    items.push(parse_item_record!(r));
                }
                Ok(items)
            }
            DbBackend::Postgres(pool) => {
                let rows = sqlx::query(&q)
                    .bind(character_guid as i64)
                    .fetch_all(pool)
                    .await?;

                let mut items = Vec::with_capacity(rows.len());
                for r in rows {
                    items.push(parse_item_record!(r));
                }
                Ok(items)
            }
        }
    }

    /// Removes an item instance from inventory.
    pub async fn remove_item(&self, item_instance_id: &str) -> Result<bool, StorageError> {
        let q = self.db.sql("DELETE FROM items WHERE item_instance_id = ?");

        let rows_affected = match self.db.backend() {
            DbBackend::Sqlite(pool) => sqlx::query(&q)
                .bind(item_instance_id)
                .execute(pool)
                .await?
                .rows_affected(),
            DbBackend::Postgres(pool) => sqlx::query(&q)
                .bind(item_instance_id)
                .execute(pool)
                .await?
                .rows_affected(),
        };

        Ok(rows_affected > 0)
    }

    /// Executes an atomic, deadlock-free player-to-player trade of items.
    ///
    /// Locking invariants:
    /// 1. Row locks on `characters` are ALWAYS acquired in strictly increasing numerical order of `guid`
    ///    (`min(sender, recipient)` followed by `max(sender, recipient)`).
    /// 2. For PostgreSQL: Acquires explicit row-level locks via `SELECT guid FROM characters WHERE guid = $1 FOR UPDATE`.
    /// 3. For SQLite: Relies on `BEGIN IMMEDIATE` transaction serialization.
    /// 4. Validates sender owns every item and recipient inventory capacity has enough free slots.
    /// 5. Durably logs the audit trail into `trades` table before committing.
    pub async fn execute_trade(
        &self,
        trade_id: &str,
        sender_guid: u64,
        recipient_guid: u64,
        item_instance_ids: &[String],
    ) -> Result<TradeRecord, StorageError> {
        if sender_guid == recipient_guid {
            return Err(StorageError::TradeError(
                "Sender and recipient must be distinct characters".to_string(),
            ));
        }
        if item_instance_ids.is_empty() {
            return Err(StorageError::TradeError(
                "No items specified for trade".to_string(),
            ));
        }

        let mut unique_items = std::collections::HashSet::with_capacity(item_instance_ids.len());
        for id in item_instance_ids {
            if !unique_items.insert(id) {
                return Err(StorageError::DuplicateItem(id.clone()));
            }
        }

        let (first_guid, second_guid) = (
            sender_guid.min(recipient_guid),
            sender_guid.max(recipient_guid),
        );

        let now = current_time_secs();

        with_dialect_tx!(&self.db, |conn, is_pg| async move {
            // 1. Dialect-specific row locking in ascending numerical order
            let q_c = format!(
                "SELECT guid FROM characters WHERE guid = ?{}",
                lock_suffix!()
            );
            let c1 = sqlx::query(&q!(&q_c))
                .bind(first_guid as i64)
                .fetch_optional(&mut *conn)
                .await?;
            let c2 = sqlx::query(&q!(&q_c))
                .bind(second_guid as i64)
                .fetch_optional(&mut *conn)
                .await?;
            if c1.is_none() || c2.is_none() {
                return Err(StorageError::NotFound(
                    "One or both trade participant characters not found".to_string(),
                ));
            }

            // 2. Fetch recipient inventory capacity
            let inv_row = sqlx::query(&q!(
                "SELECT capacity FROM inventories WHERE character_guid = ?"
            ))
            .bind(recipient_guid as i64)
            .fetch_optional(&mut *conn)
            .await?;
            let capacity = inv_row
                .map(|r| r.try_get::<i32, _>("capacity").unwrap_or(30))
                .unwrap_or(30) as u32;

            // 3. Fetch recipient occupied slots
            let occupied_rows = sqlx::query(&q!("SELECT slot FROM items WHERE character_guid = ?"))
                .bind(recipient_guid as i64)
                .fetch_all(&mut *conn)
                .await?;
            let occupied_slots: std::collections::HashSet<u32> = occupied_rows
                .into_iter()
                .filter_map(|r| r.try_get::<i32, _>("slot").ok().map(|s| s as u32))
                .collect();

            let free_slots = compute_free_slots(&occupied_slots, capacity);

            if free_slots.len() < item_instance_ids.len() {
                return Err(StorageError::InventoryFull(recipient_guid, capacity));
            }

            // 4. Verify sender owns each item
            for item_id in item_instance_ids {
                let item_row = sqlx::query(&q!(
                    "SELECT character_guid FROM items WHERE item_instance_id = ?"
                ))
                .bind(item_id)
                .fetch_optional(&mut *conn)
                .await?;

                match item_row {
                    Some(r) => {
                        let owner: i64 = r.try_get("character_guid")?;
                        if owner as u64 != sender_guid {
                            return Err(StorageError::ItemNotFound(item_id.clone(), sender_guid));
                        }
                    }
                    None => {
                        return Err(StorageError::ItemNotFound(item_id.clone(), sender_guid));
                    }
                }
            }

            // 5. Transfer items to recipient free slots
            for (item_id, target_slot) in item_instance_ids.iter().zip(free_slots.iter()) {
                sqlx::query(&q!(
                    "UPDATE items SET character_guid = ?, container_id = ?, slot = ?, updated_at = ? WHERE item_instance_id = ?"
                ))
                .bind(recipient_guid as i64)
                .bind(recipient_guid as i64)
                .bind(*target_slot as i32)
                .bind(now)
                .bind(item_id)
                .execute(&mut *conn)
                .await?;
            }

            // 6. Record audit trail
            let items_json = serde_json::to_string(item_instance_ids)?;
            sqlx::query(&q!(
                r#"
                INSERT INTO trades (trade_id, sender_guid, recipient_guid, items_json, status, created_at)
                VALUES (?, ?, ?, ?, ?, ?)
                "#
            ))
            .bind(trade_id)
            .bind(sender_guid as i64)
            .bind(recipient_guid as i64)
            .bind(&items_json)
            .bind(TradeStatus::Committed.to_string())
            .bind(now)
            .execute(&mut *conn)
            .await?;

            Ok(TradeRecord {
                trade_id: trade_id.to_string(),
                sender_guid,
                recipient_guid,
                items_json,
                status: TradeStatus::Committed,
                created_at: now,
            })
        })
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
        let updated = repo
            .update_character_position(1001, (15.0, 1.0, 25.0), 1.57)
            .await
            .unwrap();
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
            state_blob: Some(vec![1, 2, 3]),
            version: 1,
            timestamp: 1000,
        };

        let snap2 = EntitySnapshot {
            guid: 5002,
            position: (4.0, 5.0, 6.0),
            rotation: std::f32::consts::PI,
            attributes: serde_json::json!({"mana": 50}),
            state_blob: None,
            version: 1,
            timestamp: 1000,
        };

        let count = repo
            .save_snapshot_batch(&[snap1.clone(), snap2])
            .await
            .unwrap();
        assert_eq!(count, 2);

        let loaded = repo.get_snapshot(5001).await.unwrap().unwrap();
        assert_eq!(loaded.position, (1.0, 2.0, 3.0));
        assert_eq!(loaded.attributes["health"], 100);
        assert_eq!(loaded.state_blob, Some(vec![1, 2, 3]));

        // Update with higher version
        let snap1_v2 = EntitySnapshot {
            guid: 5001,
            position: (10.0, 20.0, 30.0),
            rotation: 1.0,
            attributes: serde_json::json!({"health": 85}),
            state_blob: Some(vec![4, 5, 6]),
            version: 2,
            timestamp: 2000,
        };
        repo.save_snapshot(&snap1_v2).await.unwrap();

        let loaded_v2 = repo.get_snapshot(5001).await.unwrap().unwrap();
        assert_eq!(loaded_v2.position, (10.0, 20.0, 30.0));
        assert_eq!(loaded_v2.version, 2);
        assert_eq!(loaded_v2.state_blob, Some(vec![4, 5, 6]));
    }

    #[tokio::test]
    async fn test_save_snapshot_batch_non_character_entities() {
        let db = Database::connect_memory().await.unwrap();
        let repo = CharacterRepository::new(db);

        // Batch persist snapshots for entity GUIDs that do NOT exist in characters table
        let world_obj_snap = EntitySnapshot {
            guid: 9001,
            position: (100.0, 50.0, -20.0),
            rotation: 0.5,
            attributes: serde_json::json!({"durability": 500}),
            state_blob: Some(vec![10, 20, 30]),
            version: 1,
            timestamp: 5000,
        };

        let count = repo
            .save_snapshot_batch(&[world_obj_snap])
            .await
            .expect("Saving snapshot batch for non-character entities must succeed");
        assert_eq!(count, 1);

        let retrieved = repo.get_snapshot(9001).await.unwrap().unwrap();
        assert_eq!(retrieved.position, (100.0, 50.0, -20.0));
        assert_eq!(retrieved.attributes["durability"], 500);

        // Verify that no row was created in characters table
        let char_lookup = repo.get_character(9001).await.unwrap();
        assert!(char_lookup.is_none());
    }

    #[tokio::test]
    async fn test_inventory_and_item_crud() {
        let db = Database::connect_memory().await.unwrap();
        let repo = CharacterRepository::new(db);

        repo.create_account("acc_inv", "inventory_user")
            .await
            .unwrap();
        repo.create_character("acc_inv", 2001, "Mage", (0.0, 0.0, 0.0))
            .await
            .unwrap();

        // 1. Check default inventory
        let inv = repo.get_inventory(2001).await.unwrap().unwrap();
        assert_eq!(inv.capacity, 30);

        // 2. Set capacity to 40
        repo.set_inventory_capacity(2001, 40).await.unwrap();
        let updated_inv = repo.get_inventory(2001).await.unwrap().unwrap();
        assert_eq!(updated_inv.capacity, 40);

        // 3. Add items
        let item1 = ItemRecord {
            item_instance_id: "item_sword_1".to_string(),
            character_guid: 2001,
            container_id: 2001,
            item_def_id: 101,
            slot: 0,
            quantity: 1,
            durability: 100,
            attributes: serde_json::json!({"attack": 15}),
            created_at: 1000,
            updated_at: 1000,
        };
        repo.add_item(&item1).await.unwrap();

        let item2 = ItemRecord {
            item_instance_id: "item_potion_1".to_string(),
            character_guid: 2001,
            container_id: 2001,
            item_def_id: 202,
            slot: 1,
            quantity: 5,
            durability: 100,
            attributes: serde_json::json!({"heal": 50}),
            created_at: 1000,
            updated_at: 1000,
        };
        repo.add_item(&item2).await.unwrap();

        // 4. Retrieve single item
        let fetched_item = repo.get_item("item_sword_1").await.unwrap().unwrap();
        assert_eq!(fetched_item.item_def_id, 101);
        assert_eq!(fetched_item.attributes["attack"], 15);

        // 5. Retrieve all items for character
        let items = repo.get_character_items(2001).await.unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].slot, 0);
        assert_eq!(items[1].slot, 1);

        // 6. Slot collision rejection
        let colliding_item = ItemRecord {
            item_instance_id: "item_colliding".to_string(),
            character_guid: 2001,
            container_id: 2001,
            item_def_id: 999,
            slot: 0, // already occupied
            quantity: 1,
            durability: 100,
            attributes: serde_json::json!({}),
            created_at: 1000,
            updated_at: 1000,
        };
        assert!(repo.add_item(&colliding_item).await.is_err());

        // 7. Remove item
        let removed = repo.remove_item("item_sword_1").await.unwrap();
        assert!(removed);
        let items_after = repo.get_character_items(2001).await.unwrap();
        assert_eq!(items_after.len(), 1);
    }

    #[tokio::test]
    async fn test_acid_item_trade_success_and_rollback() {
        let db = Database::connect_memory().await.unwrap();
        let repo = CharacterRepository::new(db);

        // Setup accounts and characters
        repo.create_account("acc_trade_a", "trader_a")
            .await
            .unwrap();
        repo.create_account("acc_trade_b", "trader_b")
            .await
            .unwrap();
        repo.create_character("acc_trade_a", 3001, "Alice", (0.0, 0.0, 0.0))
            .await
            .unwrap();
        repo.create_character("acc_trade_b", 3002, "Bob", (0.0, 0.0, 0.0))
            .await
            .unwrap();

        // Give Alice 2 items
        let item1 = ItemRecord {
            item_instance_id: "trade_sword".to_string(),
            character_guid: 3001,
            container_id: 3001,
            item_def_id: 100,
            slot: 0,
            quantity: 1,
            durability: 100,
            attributes: serde_json::json!({}),
            created_at: 1000,
            updated_at: 1000,
        };
        let item2 = ItemRecord {
            item_instance_id: "trade_shield".to_string(),
            character_guid: 3001,
            container_id: 3001,
            item_def_id: 101,
            slot: 1,
            quantity: 1,
            durability: 100,
            attributes: serde_json::json!({}),
            created_at: 1000,
            updated_at: 1000,
        };
        repo.add_item(&item1).await.unwrap();
        repo.add_item(&item2).await.unwrap();

        // 1. Successful trade of trade_sword from Alice (3001) to Bob (3002)
        let trade = repo
            .execute_trade("trade_001", 3001, 3002, &["trade_sword".to_string()])
            .await
            .unwrap();
        assert_eq!(trade.status, TradeStatus::Committed);

        // Verify Bob now owns trade_sword
        let bob_items = repo.get_character_items(3002).await.unwrap();
        assert_eq!(bob_items.len(), 1);
        assert_eq!(bob_items[0].item_instance_id, "trade_sword");
        assert_eq!(bob_items[0].character_guid, 3002);

        // Verify Alice only has trade_shield
        let alice_items = repo.get_character_items(3001).await.unwrap();
        assert_eq!(alice_items.len(), 1);
        assert_eq!(alice_items[0].item_instance_id, "trade_shield");

        // 2. Rollback when recipient inventory has insufficient space
        // Set Bob capacity to 1 (which is now full with trade_sword)
        repo.set_inventory_capacity(3002, 1).await.unwrap();

        let fail_trade = repo
            .execute_trade("trade_002", 3001, 3002, &["trade_shield".to_string()])
            .await;
        assert!(matches!(
            fail_trade,
            Err(StorageError::InventoryFull(3002, 1))
        ));

        // Verify Alice STILL owns trade_shield (no partial loss or dupe)
        let alice_items_after = repo.get_character_items(3001).await.unwrap();
        assert_eq!(alice_items_after.len(), 1);
        assert_eq!(alice_items_after[0].item_instance_id, "trade_shield");

        // 3. Rollback when sender does not own the item
        repo.set_inventory_capacity(3002, 10).await.unwrap();
        let invalid_owner_trade = repo
            .execute_trade("trade_003", 3001, 3002, &["non_existent_item".to_string()])
            .await;
        assert!(matches!(
            invalid_owner_trade,
            Err(StorageError::ItemNotFound(_, 3001))
        ));
    }

    #[tokio::test]
    async fn test_concurrent_reverse_trades_deadlock_freedom() {
        let db = Database::connect_memory().await.unwrap();
        let repo = CharacterRepository::new(db);

        repo.create_account("acc_deadlock_1", "user_d1")
            .await
            .unwrap();
        repo.create_account("acc_deadlock_2", "user_d2")
            .await
            .unwrap();
        repo.create_character("acc_deadlock_1", 4001, "PlayerOne", (0.0, 0.0, 0.0))
            .await
            .unwrap();
        repo.create_character("acc_deadlock_2", 4002, "PlayerTwo", (0.0, 0.0, 0.0))
            .await
            .unwrap();

        // PlayerOne has item1, PlayerTwo has item2
        repo.add_item(&ItemRecord {
            item_instance_id: "p1_ring".to_string(),
            character_guid: 4001,
            container_id: 4001,
            item_def_id: 301,
            slot: 0,
            quantity: 1,
            durability: 100,
            attributes: serde_json::json!({}),
            created_at: 1000,
            updated_at: 1000,
        })
        .await
        .unwrap();

        repo.add_item(&ItemRecord {
            item_instance_id: "p2_amulet".to_string(),
            character_guid: 4002,
            container_id: 4002,
            item_def_id: 302,
            slot: 0,
            quantity: 1,
            durability: 100,
            attributes: serde_json::json!({}),
            created_at: 1000,
            updated_at: 1000,
        })
        .await
        .unwrap();

        // Concurrently run PlayerOne -> PlayerTwo and PlayerTwo -> PlayerOne
        let repo1 = repo.clone();
        let repo2 = repo.clone();

        let t1 = tokio::spawn(async move {
            repo1
                .execute_trade("trade_rev_1", 4001, 4002, &["p1_ring".to_string()])
                .await
        });

        let t2 = tokio::spawn(async move {
            repo2
                .execute_trade("trade_rev_2", 4002, 4001, &["p2_amulet".to_string()])
                .await
        });

        let (res1, res2) = tokio::join!(t1, t2);
        assert!(res1.unwrap().is_ok());
        assert!(res2.unwrap().is_ok());

        // Verify items were exchanged without deadlock or duplication
        let p1_items = repo.get_character_items(4001).await.unwrap();
        let p2_items = repo.get_character_items(4002).await.unwrap();

        assert_eq!(p1_items.len(), 1);
        assert_eq!(p1_items[0].item_instance_id, "p2_amulet");

        assert_eq!(p2_items.len(), 1);
        assert_eq!(p2_items[0].item_instance_id, "p1_ring");
    }

    #[tokio::test]
    async fn test_update_character_position_coordinates() {
        let db = Database::connect_memory().await.unwrap();
        let repo = CharacterRepository::new(db);

        repo.create_account("acc_pos", "pos_user").await.unwrap();
        repo.create_character("acc_pos", 7001, "PosChar", (0.0, 0.0, 0.0))
            .await
            .unwrap();

        // Updating nonexistent character returns false
        let not_found = repo
            .update_character_position(99999, (1.0, 2.0, 3.0), 0.5)
            .await
            .unwrap();
        assert!(!not_found);

        // Test boundary values: negative coordinates, zero, large floats
        let test_coords = [
            ((-100.5, -200.25, -300.75), -std::f32::consts::PI),
            ((0.0, 0.0, 0.0), 0.0),
            ((12345.67, 98765.43, 54321.12), std::f32::consts::TAU),
        ];

        for (pos, rot) in test_coords {
            let ok = repo
                .update_character_position(7001, pos, rot)
                .await
                .unwrap();
            assert!(ok);

            let character = repo.get_character(7001).await.unwrap().unwrap();
            assert_eq!(character.position, pos);
            assert_eq!(character.rotation, rot);
        }
    }

    #[tokio::test]
    async fn test_execute_trade_validation_boundaries() {
        let db = Database::connect_memory().await.unwrap();
        let repo = CharacterRepository::new(db.clone());

        repo.create_account("acc_trade_val", "val_user")
            .await
            .unwrap();
        repo.create_character("acc_trade_val", 8001, "TraderA", (0.0, 0.0, 0.0))
            .await
            .unwrap();
        repo.create_character("acc_trade_val", 8002, "TraderB", (0.0, 0.0, 0.0))
            .await
            .unwrap();

        // 1. Same character ID trade (party_a == party_b) rejected with TradeError
        let same_char_trade = repo
            .execute_trade("trade_self", 8001, 8001, &["item_1".to_string()])
            .await;
        assert!(matches!(same_char_trade, Err(StorageError::TradeError(_))));
        if let Err(StorageError::TradeError(msg)) = same_char_trade {
            assert!(msg.contains("Sender and recipient must be distinct"));
        }

        // 2. Empty items list rejected with TradeError
        let empty_items_trade = repo.execute_trade("trade_empty", 8001, 8002, &[]).await;
        assert!(matches!(
            empty_items_trade,
            Err(StorageError::TradeError(_))
        ));
        if let Err(StorageError::TradeError(msg)) = empty_items_trade {
            assert!(msg.contains("No items specified"));
        }

        // 3. Remove non-existent item returns false
        let removed = repo.remove_item("non_existent_item_guid").await.unwrap();
        assert!(!removed);

        // 4. One participant missing returns NotFound
        let missing_sender = repo
            .execute_trade("trade_missing_s", 99998, 8002, &["item_1".to_string()])
            .await;
        assert!(matches!(missing_sender, Err(StorageError::NotFound(_))));

        let missing_recipient = repo
            .execute_trade("trade_missing_r", 8001, 99999, &["item_1".to_string()])
            .await;
        assert!(matches!(missing_recipient, Err(StorageError::NotFound(_))));

        // 5. Reverse participant GUID ordering (sender > recipient) with missing participant
        let reverse_order_missing = repo
            .execute_trade("trade_rev", 8002, 99999, &["item_1".to_string()])
            .await;
        assert!(matches!(
            reverse_order_missing,
            Err(StorageError::NotFound(_))
        ));

        // 6. Valid reverse trade: sender_guid (8002) > recipient_guid (8001)
        // Set recipient 8001 capacity to exactly 1 to verify free_slots.len() < items.len() exact boundary
        sqlx::query("UPDATE inventories SET capacity = 1 WHERE character_guid = 8001")
            .execute(db.sqlite_pool().unwrap())
            .await
            .unwrap();

        let item_rev = ItemRecord {
            item_instance_id: "trade_item_rev".to_string(),
            character_guid: 8002,
            container_id: 0,
            item_def_id: 99,
            slot: 0,
            quantity: 1,
            durability: 100,
            attributes: serde_json::json!({}),
            created_at: 1000,
            updated_at: 1000,
        };
        repo.add_item(&item_rev).await.unwrap();

        let trade_rev_ok = repo
            .execute_trade(
                "trade_rev_valid",
                8002,
                8001,
                &["trade_item_rev".to_string()],
            )
            .await;
        assert!(trade_rev_ok.is_ok());

        // Now recipient 8001 is completely full (capacity 1, 1 item).
        // Trading another item to 8001 must fail with InventoryFull.
        let item_full = ItemRecord {
            item_instance_id: "trade_item_full".to_string(),
            character_guid: 8002,
            container_id: 0,
            item_def_id: 99,
            slot: 1,
            quantity: 1,
            durability: 100,
            attributes: serde_json::json!({}),
            created_at: 1000,
            updated_at: 1000,
        };
        repo.add_item(&item_full).await.unwrap();

        let trade_full_err = repo
            .execute_trade(
                "trade_full_err",
                8002,
                8001,
                &["trade_item_full".to_string()],
            )
            .await;
        assert!(matches!(
            trade_full_err,
            Err(StorageError::InventoryFull(8001, 1))
        ));
    }

    #[tokio::test]
    async fn test_load_snapshot_alias() {
        let db = Database::connect_memory().await.unwrap();
        let repo = CharacterRepository::new(db);
        let snapshot = EntitySnapshot {
            guid: 9999,
            position: (1.0, 2.0, 3.0),
            rotation: 0.0,
            attributes: serde_json::json!({}),
            state_blob: None,
            version: 1,
            timestamp: 100,
        };
        repo.save_snapshot(&snapshot).await.unwrap();
        let loaded = repo.load_snapshot(9999).await.unwrap();
        assert_eq!(loaded, Some(snapshot));
    }
}
