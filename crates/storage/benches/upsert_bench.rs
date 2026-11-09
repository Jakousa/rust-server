use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use mmo_storage::db::{Database, DbBackend};
use mmo_storage::models::WorldObjectRecord;
use mmo_storage::world_repo::WorldObjectRepository;
use sqlx::Acquire;

fn sample_record(object_id: u64, idx: u64) -> WorldObjectRecord {
    WorldObjectRecord {
        world_id: 1,
        object_id,
        sector_x: 0,
        sector_y: 0,
        sector_z: 0,
        prefab_id: 42,
        pos_x: (idx as f32) * 10.0,
        pos_y: 0.0,
        pos_z: 0.0,
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
        dirty_revision: 1,
        updated_at: 1000,
    }
}

async fn n_plus_one_insert(db: &Database, records: &[WorldObjectRecord]) {
    let mut conn = match db.backend() {
        DbBackend::Sqlite(pool) => pool.acquire().await.unwrap(),
        _ => panic!("Expected sqlite"),
    };
    let mut tx = conn.begin().await.unwrap();
    let query_str = "
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
    ";

    for r in records {
        sqlx::query(query_str)
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
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
}

fn bench_upserts(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let batch_size = 1000;

    let records: Vec<WorldObjectRecord> = (0..batch_size)
        .map(|i| sample_record(1000 + i as u64, i as u64))
        .collect();

    let mut group = c.benchmark_group("world_objects_upsert");

    group.bench_function("n_plus_one_upsert", |b| {
        b.to_async(&runtime).iter_batched(
            || {
                std::thread::spawn(|| {
                    let rt = tokio::runtime::Runtime::new().unwrap();
                    rt.block_on(async { Database::connect_memory().await.unwrap() })
                })
                .join()
                .unwrap()
            },
            |db| {
                let recs = records.clone();
                async move {
                    n_plus_one_insert(&db, &recs).await;
                }
            },
            BatchSize::SmallInput,
        );
    });

    group.bench_function("bulk_push_values", |b| {
        b.to_async(&runtime).iter_batched(
            || {
                std::thread::spawn(|| {
                    let rt = tokio::runtime::Runtime::new().unwrap();
                    rt.block_on(async {
                        let db = Database::connect_memory().await.unwrap();

                        WorldObjectRepository::new(db)
                    })
                })
                .join()
                .unwrap()
            },
            |repo| {
                let recs = records.clone();
                async move {
                    repo.upsert_world_objects_batch(&recs).await.unwrap();
                }
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

criterion_group!(benches, bench_upserts);
criterion_main!(benches);
