//! Persistent repository for world objects and spatial sector streaming.

use crate::db::{sql_for_dialect, Database, DbBackend, StorageError};
use crate::models::WorldObjectRecord;
use sqlx::Row;

macro_rules! parse_world_object_record {
    ($r:expr) => {{
        let world_id: i64 = $r.try_get("world_id")?;
        let object_id: i64 = $r.try_get("object_id")?;
        let sector_x: i32 = $r.try_get("sector_x")?;
        let sector_y: i32 = $r.try_get("sector_y")?;
        let sector_z: i32 = $r.try_get("sector_z")?;
        let prefab_id: i32 = $r.try_get("prefab_id")?;
        let pos_x: f32 = $r.try_get("pos_x")?;
        let pos_y: f32 = $r.try_get("pos_y")?;
        let pos_z: f32 = $r.try_get("pos_z")?;
        let rot_x: f32 = $r.try_get("rot_x")?;
        let rot_y: f32 = $r.try_get("rot_y")?;
        let rot_z: f32 = $r.try_get("rot_z")?;
        let rot_w: f32 = $r.try_get("rot_w")?;
        let scale_x: f32 = $r.try_get("scale_x")?;
        let scale_y: f32 = $r.try_get("scale_y")?;
        let scale_z: f32 = $r.try_get("scale_z")?;
        let bounds_min_x: f32 = $r.try_get("bounds_min_x")?;
        let bounds_min_y: f32 = $r.try_get("bounds_min_y")?;
        let bounds_min_z: f32 = $r.try_get("bounds_min_z")?;
        let bounds_max_x: f32 = $r.try_get("bounds_max_x")?;
        let bounds_max_y: f32 = $r.try_get("bounds_max_y")?;
        let bounds_max_z: f32 = $r.try_get("bounds_max_z")?;
        let state_flags: i32 = $r.try_get("state_flags")?;
        let owner_type: String = $r.try_get("owner_type")?;
        let owner_id: String = $r.try_get("owner_id")?;
        let health: Option<i32> = $r.try_get("health")?;
        let max_health: Option<i32> = $r.try_get("max_health")?;
        let properties_json: Option<String> = $r.try_get("properties_json")?;
        let dirty_revision: i64 = $r.try_get("dirty_revision")?;
        let updated_at: i64 = $r.try_get("updated_at")?;

        WorldObjectRecord {
            world_id: world_id as u64,
            object_id: object_id as u64,
            sector_x,
            sector_y,
            sector_z,
            prefab_id: prefab_id as u32,
            pos_x,
            pos_y,
            pos_z,
            rot_x,
            rot_y,
            rot_z,
            rot_w,
            scale_x,
            scale_y,
            scale_z,
            bounds_min_x,
            bounds_min_y,
            bounds_min_z,
            bounds_max_x,
            bounds_max_y,
            bounds_max_z,
            state_flags: state_flags as u32,
            owner_type,
            owner_id,
            health,
            max_health,
            properties_json: properties_json.unwrap_or_else(|| "{}".to_string()),
            dirty_revision: dirty_revision as u64,
            updated_at,
        }
    }};
}

const UPSERT_QUERY: &str = r#"
    INSERT INTO world_objects (
        world_id, object_id, sector_x, sector_y, sector_z, prefab_id,
        pos_x, pos_y, pos_z, rot_x, rot_y, rot_z, rot_w,
        scale_x, scale_y, scale_z,
        bounds_min_x, bounds_min_y, bounds_min_z, bounds_max_x, bounds_max_y, bounds_max_z,
        state_flags, owner_type, owner_id, health, max_health,
        properties_json, dirty_revision, updated_at
    ) VALUES (
        ?, ?, ?, ?, ?, ?,
        ?, ?, ?, ?, ?, ?, ?,
        ?, ?, ?,
        ?, ?, ?, ?, ?, ?,
        ?, ?, ?, ?, ?,
        ?, ?, ?
    )
    ON CONFLICT (world_id, object_id) DO UPDATE SET
        sector_x = excluded.sector_x,
        sector_y = excluded.sector_y,
        sector_z = excluded.sector_z,
        prefab_id = excluded.prefab_id,
        pos_x = excluded.pos_x,
        pos_y = excluded.pos_y,
        pos_z = excluded.pos_z,
        rot_x = excluded.rot_x,
        rot_y = excluded.rot_y,
        rot_z = excluded.rot_z,
        rot_w = excluded.rot_w,
        scale_x = excluded.scale_x,
        scale_y = excluded.scale_y,
        scale_z = excluded.scale_z,
        bounds_min_x = excluded.bounds_min_x,
        bounds_min_y = excluded.bounds_min_y,
        bounds_min_z = excluded.bounds_min_z,
        bounds_max_x = excluded.bounds_max_x,
        bounds_max_y = excluded.bounds_max_y,
        bounds_max_z = excluded.bounds_max_z,
        state_flags = excluded.state_flags,
        owner_type = excluded.owner_type,
        owner_id = excluded.owner_id,
        health = excluded.health,
        max_health = excluded.max_health,
        properties_json = excluded.properties_json,
        dirty_revision = excluded.dirty_revision,
        updated_at = excluded.updated_at
    WHERE excluded.dirty_revision >= world_objects.dirty_revision
"#;

const SELECT_FIELDS: &str = r#"
    world_id, object_id, sector_x, sector_y, sector_z, prefab_id,
    pos_x, pos_y, pos_z, rot_x, rot_y, rot_z, rot_w,
    scale_x, scale_y, scale_z,
    bounds_min_x, bounds_min_y, bounds_min_z, bounds_max_x, bounds_max_y, bounds_max_z,
    state_flags, owner_type, owner_id, health, max_health,
    properties_json, dirty_revision, updated_at
"#;

/// Repository for persistent world objects and spatial sector loading.
#[derive(Clone)]
pub struct WorldObjectRepository {
    db: Database,
}

pub type WorldRepository = WorldObjectRepository;

impl WorldObjectRepository {
    /// Constructs a new `WorldObjectRepository` wrapping the provided database handle.
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// Upserts a single world object.
    pub async fn upsert_world_object(
        &self,
        record: &WorldObjectRecord,
    ) -> Result<(), StorageError> {
        let query_str = sql_for_dialect(UPSERT_QUERY, self.db.is_postgres());

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                Self::bind_upsert_sqlite(sqlx::query(&query_str), record)
                    .execute(pool)
                    .await?;
            }
            DbBackend::Postgres(pool) => {
                Self::bind_upsert_postgres(sqlx::query(&query_str), record)
                    .execute(pool)
                    .await?;
            }
        }

        Ok(())
    }

    /// Atomically upserts a batch of world objects in a transaction.
    pub async fn upsert_world_objects_batch(
        &self,
        records: &[WorldObjectRecord],
    ) -> Result<(), StorageError> {
        if records.is_empty() {
            return Ok(());
        }

        let query_str = sql_for_dialect(UPSERT_QUERY, self.db.is_postgres());

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                let mut tx = pool.begin().await?;
                for record in records {
                    Self::bind_upsert_sqlite(sqlx::query(&query_str), record)
                        .execute(&mut *tx)
                        .await?;
                }
                tx.commit().await?;
            }
            DbBackend::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                for record in records {
                    Self::bind_upsert_postgres(sqlx::query(&query_str), record)
                        .execute(&mut *tx)
                        .await?;
                }
                tx.commit().await?;
            }
        }

        Ok(())
    }

    /// Alias for `upsert_world_object`.
    pub async fn save_world_object(&self, record: &WorldObjectRecord) -> Result<(), StorageError> {
        self.upsert_world_object(record).await
    }

    /// Alias for `upsert_world_objects_batch`.
    pub async fn save_world_objects_batch(
        &self,
        records: &[WorldObjectRecord],
    ) -> Result<(), StorageError> {
        self.upsert_world_objects_batch(records).await
    }

    /// Alias for `get_objects_in_sector`.
    pub async fn load_sector_objects(
        &self,
        world_id: u64,
        sector_x: i32,
        sector_y: i32,
        sector_z: i32,
    ) -> Result<Vec<WorldObjectRecord>, StorageError> {
        self.get_objects_in_sector(world_id, sector_x, sector_y, sector_z)
            .await
    }

    /// Fetches a world object by primary key `(world_id, object_id)`.
    pub async fn get_world_object(
        &self,
        world_id: u64,
        object_id: u64,
    ) -> Result<Option<WorldObjectRecord>, StorageError> {
        let query = format!(
            "SELECT {} FROM world_objects WHERE world_id = ? AND object_id = ?",
            SELECT_FIELDS
        );
        let query_str = sql_for_dialect(&query, self.db.is_postgres());

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                let row = sqlx::query(&query_str)
                    .bind(world_id as i64)
                    .bind(object_id as i64)
                    .fetch_optional(pool)
                    .await?;

                match row {
                    Some(r) => Ok(Some(parse_world_object_record!(r))),
                    None => Ok(None),
                }
            }
            DbBackend::Postgres(pool) => {
                let row = sqlx::query(&query_str)
                    .bind(world_id as i64)
                    .bind(object_id as i64)
                    .fetch_optional(pool)
                    .await?;

                match row {
                    Some(r) => Ok(Some(parse_world_object_record!(r))),
                    None => Ok(None),
                }
            }
        }
    }

    /// Fetches all world objects located within a discrete 3D spatial sector.
    pub async fn get_objects_in_sector(
        &self,
        world_id: u64,
        sector_x: i32,
        sector_y: i32,
        sector_z: i32,
    ) -> Result<Vec<WorldObjectRecord>, StorageError> {
        let query = format!(
            "SELECT {} FROM world_objects WHERE world_id = ? AND sector_x = ? AND sector_y = ? AND sector_z = ?",
            SELECT_FIELDS
        );
        let query_str = sql_for_dialect(&query, self.db.is_postgres());

        match self.db.backend() {
            DbBackend::Sqlite(pool) => {
                let rows = sqlx::query(&query_str)
                    .bind(world_id as i64)
                    .bind(sector_x)
                    .bind(sector_y)
                    .bind(sector_z)
                    .fetch_all(pool)
                    .await?;

                let mut objects = Vec::with_capacity(rows.len());
                for r in rows {
                    objects.push(parse_world_object_record!(r));
                }
                Ok(objects)
            }
            DbBackend::Postgres(pool) => {
                let rows = sqlx::query(&query_str)
                    .bind(world_id as i64)
                    .bind(sector_x)
                    .bind(sector_y)
                    .bind(sector_z)
                    .fetch_all(pool)
                    .await?;

                let mut objects = Vec::with_capacity(rows.len());
                for r in rows {
                    objects.push(parse_world_object_record!(r));
                }
                Ok(objects)
            }
        }
    }

    /// Deletes a world object by its primary key `(world_id, object_id)`.
    pub async fn delete_world_object(
        &self,
        world_id: u64,
        object_id: u64,
    ) -> Result<bool, StorageError> {
        let query = "DELETE FROM world_objects WHERE world_id = ? AND object_id = ?";
        let query_str = sql_for_dialect(query, self.db.is_postgres());

        let rows_affected = match self.db.backend() {
            DbBackend::Sqlite(pool) => sqlx::query(&query_str)
                .bind(world_id as i64)
                .bind(object_id as i64)
                .execute(pool)
                .await?
                .rows_affected(),
            DbBackend::Postgres(pool) => sqlx::query(&query_str)
                .bind(world_id as i64)
                .bind(object_id as i64)
                .execute(pool)
                .await?
                .rows_affected(),
        };

        Ok(rows_affected > 0)
    }

    fn bind_upsert_sqlite<'q>(
        query: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
        r: &'q WorldObjectRecord,
    ) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>> {
        query
            .bind(r.world_id as i64)
            .bind(r.object_id as i64)
            .bind(r.sector_x)
            .bind(r.sector_y)
            .bind(r.sector_z)
            .bind(r.prefab_id as i32)
            .bind(r.pos_x)
            .bind(r.pos_y)
            .bind(r.pos_z)
            .bind(r.rot_x)
            .bind(r.rot_y)
            .bind(r.rot_z)
            .bind(r.rot_w)
            .bind(r.scale_x)
            .bind(r.scale_y)
            .bind(r.scale_z)
            .bind(r.bounds_min_x)
            .bind(r.bounds_min_y)
            .bind(r.bounds_min_z)
            .bind(r.bounds_max_x)
            .bind(r.bounds_max_y)
            .bind(r.bounds_max_z)
            .bind(r.state_flags as i32)
            .bind(&r.owner_type)
            .bind(&r.owner_id)
            .bind(r.health)
            .bind(r.max_health)
            .bind(&r.properties_json)
            .bind(r.dirty_revision as i64)
            .bind(r.updated_at)
    }

    fn bind_upsert_postgres<'q>(
        query: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
        r: &'q WorldObjectRecord,
    ) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
        query
            .bind(r.world_id as i64)
            .bind(r.object_id as i64)
            .bind(r.sector_x)
            .bind(r.sector_y)
            .bind(r.sector_z)
            .bind(r.prefab_id as i32)
            .bind(r.pos_x)
            .bind(r.pos_y)
            .bind(r.pos_z)
            .bind(r.rot_x)
            .bind(r.rot_y)
            .bind(r.rot_z)
            .bind(r.rot_w)
            .bind(r.scale_x)
            .bind(r.scale_y)
            .bind(r.scale_z)
            .bind(r.bounds_min_x)
            .bind(r.bounds_min_y)
            .bind(r.bounds_min_z)
            .bind(r.bounds_max_x)
            .bind(r.bounds_max_y)
            .bind(r.bounds_max_z)
            .bind(r.state_flags as i32)
            .bind(&r.owner_type)
            .bind(&r.owner_id)
            .bind(r.health)
            .bind(r.max_health)
            .bind(&r.properties_json)
            .bind(r.dirty_revision as i64)
            .bind(r.updated_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record(object_id: u64, sx: i32, sy: i32, sz: i32, rev: u64) -> WorldObjectRecord {
        WorldObjectRecord {
            world_id: 1,
            object_id,
            sector_x: sx,
            sector_y: sy,
            sector_z: sz,
            prefab_id: 42,
            pos_x: (sx * 64) as f32 + 10.0,
            pos_y: (sy * 64) as f32 + 10.0,
            pos_z: (sz * 64) as f32 + 10.0,
            rot_x: 0.0,
            rot_y: 0.0,
            rot_z: 0.0,
            rot_w: 1.0,
            scale_x: 1.0,
            scale_y: 1.0,
            scale_z: 1.0,
            bounds_min_x: 0.0,
            bounds_min_y: 0.0,
            bounds_min_z: 0.0,
            bounds_max_x: 2.0,
            bounds_max_y: 2.0,
            bounds_max_z: 2.0,
            state_flags: 3,
            owner_type: "character".to_string(),
            owner_id: "100".to_string(),
            health: Some(100),
            max_health: Some(100),
            properties_json: r#"{"type":"chest"}"#.to_string(),
            dirty_revision: rev,
            updated_at: 1000,
        }
    }

    #[tokio::test]
    async fn test_world_repo_crud_and_sector_lookup() {
        let db = Database::connect_memory().await.unwrap();
        let repo = WorldObjectRepository::new(db);

        // 1. Upsert initial record
        let obj1 = sample_record(101, 0, 0, 0, 1);
        repo.upsert_world_object(&obj1).await.unwrap();

        let fetched = repo
            .get_world_object(1, 101)
            .await
            .unwrap()
            .expect("object exists");
        assert_eq!(fetched.object_id, 101);
        assert_eq!(fetched.dirty_revision, 1);
        assert_eq!(fetched.sector_x, 0);

        // 2. Upsert newer revision
        let mut obj1_v2 = obj1.clone();
        obj1_v2.dirty_revision = 2;
        obj1_v2.pos_x = 15.0;
        repo.upsert_world_object(&obj1_v2).await.unwrap();

        let fetched_v2 = repo.get_world_object(1, 101).await.unwrap().unwrap();
        assert_eq!(fetched_v2.dirty_revision, 2);
        assert!((fetched_v2.pos_x - 15.0).abs() < 1e-4);

        // 3. Stale revision does not overwrite newer
        let mut obj1_stale = obj1.clone();
        obj1_stale.dirty_revision = 1;
        obj1_stale.pos_x = 999.0;
        repo.upsert_world_object(&obj1_stale).await.unwrap();

        let fetched_still_v2 = repo.get_world_object(1, 101).await.unwrap().unwrap();
        assert_eq!(fetched_still_v2.dirty_revision, 2);
        assert!((fetched_still_v2.pos_x - 15.0).abs() < 1e-4);

        // 4. Batch upsert
        let batch = vec![
            sample_record(102, 0, 0, 0, 1),
            sample_record(103, 0, 0, 0, 1),
            sample_record(201, 1, 0, 0, 1),
        ];
        repo.upsert_world_objects_batch(&batch).await.unwrap();

        // 5. Query sector (0, 0, 0) -> Should have 101, 102, 103
        let sector_objs = repo.get_objects_in_sector(1, 0, 0, 0).await.unwrap();
        assert_eq!(sector_objs.len(), 3);
        let ids: Vec<u64> = sector_objs.into_iter().map(|o| o.object_id).collect();
        assert!(ids.contains(&101));
        assert!(ids.contains(&102));
        assert!(ids.contains(&103));

        // 6. Query sector (1, 0, 0) -> Should have 201
        let sector_1 = repo.get_objects_in_sector(1, 1, 0, 0).await.unwrap();
        assert_eq!(sector_1.len(), 1);
        assert_eq!(sector_1[0].object_id, 201);

        // 7. Delete object
        let deleted = repo.delete_world_object(1, 102).await.unwrap();
        assert!(deleted);
        assert!(repo.get_world_object(1, 102).await.unwrap().is_none());

        let deleted_again = repo.delete_world_object(1, 102).await.unwrap();
        assert!(!deleted_again);
    }
}
