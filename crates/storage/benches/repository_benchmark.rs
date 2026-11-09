use criterion::{criterion_group, criterion_main, Criterion};
use mmo_storage::db::Database;
use mmo_storage::models::ItemRecord;
use mmo_storage::repository::CharacterRepository;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use tokio::runtime::Runtime;

static TRADE_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn benchmark(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let db = rt.block_on(async { Database::connect_memory().await.unwrap() });
    let repo = CharacterRepository::new(db);

    rt.block_on(async {
        repo.create_account("acc_1", "user").await.unwrap();
        repo.create_character("acc_1", 1, "Char1", (0.0, 0.0, 0.0))
            .await
            .unwrap();
        repo.create_character("acc_1", 2, "Char2", (0.0, 0.0, 0.0))
            .await
            .unwrap();
    });

    c.bench_function("execute_trade_bulk", |b| {
        b.to_async(&rt).iter_custom(|iters| {
            let repo_clone = repo.clone();
            async move {
                let start = Instant::now();
                for _ in 0..iters {
                    let iter_idx = TRADE_COUNTER.fetch_add(1, Ordering::Relaxed);
                    // Populate items for sender
                    let mut items = Vec::new();
                    // Add items
                    for j in 0..30 {
                        let item_id = format!("item_{}_{}", iter_idx, j);
                        let item_record = ItemRecord {
                            item_instance_id: item_id.clone(),
                            character_guid: 1,
                            container_id: 1,
                            item_def_id: 100,
                            slot: j as u32,
                            quantity: 1,
                            durability: 100,
                            attributes: serde_json::Value::Null,
                            created_at: 0,
                            updated_at: 0,
                        };
                        repo_clone.add_item(&item_record).await.unwrap();
                        items.push(item_id);
                    }

                    let trade_id = format!("trade_{}", iter_idx);
                    repo_clone
                        .execute_trade(&trade_id, 1, 2, &items)
                        .await
                        .unwrap();

                    // Cleanup for next iteration
                    for item in items {
                        repo_clone.remove_item(&item).await.unwrap();
                    }
                }
                start.elapsed()
            }
        });
    });
}

criterion_group!(benches, benchmark);
criterion_main!(benches);
