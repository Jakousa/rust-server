use criterion::{black_box, criterion_group, criterion_main, Criterion};
use mmo_core::guid::{EntityType, Guid};
use mmo_core::{BoundingBox3D, EntityStore, Octree};
use mmo_node::TickSimulation;

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct PlayerStats {
    pub health: u32,
    pub mana: u32,
}

fn populate_entities(count: usize) -> (EntityStore, Octree, Vec<Guid>) {
    let mut store = EntityStore::new();
    let bounds = BoundingBox3D::new((-500.0, -500.0, -500.0), (500.0, 500.0, 500.0));
    let mut octree = Octree::new(bounds);
    let mut guids = Vec::with_capacity(count);

    for i in 0..count {
        let guid = Guid::new(EntityType::Player, 1, i as u32);
        let x = ((i * 7) % 900) as f32 - 450.0;
        let y = ((i * 13) % 900) as f32 - 450.0;
        let z = ((i * 17) % 900) as f32 - 450.0;
        let pos = (x, y, z);

        let key = store.spawn(guid, pos);
        if let Some(entity) = store.get_mut(key) {
            entity.components.add_component(PlayerStats {
                health: 100,
                mana: 50,
            });
        }
        octree.insert(guid, pos);
        guids.push(guid);
    }

    (store, octree, guids)
}

fn bench_octree_queries(c: &mut Criterion) {
    let (_, octree, _) = populate_entities(1500);

    let mut group = c.benchmark_group("octree_1500_entities");

    group.bench_function("query_radius_50m", |b| {
        b.iter(|| {
            let hits = octree.query_radius(black_box((0.0, 0.0, 0.0)), black_box(50.0));
            black_box(hits);
        });
    });

    group.bench_function("query_range_bounding_box", |b| {
        let query_box = BoundingBox3D::new((-50.0, -50.0, -50.0), (50.0, 50.0, 50.0));
        b.iter(|| {
            let hits = octree.query_range(black_box(&query_box));
            black_box(hits);
        });
    });

    group.bench_function("find_boundary_neighbors", |b| {
        b.iter(|| {
            let neighbors = octree.find_boundary_neighbors(black_box((10.0, 10.0, 10.0)), black_box(5.0));
            black_box(neighbors);
        });
    });

    group.finish();
}

fn bench_slotmap_iterations(c: &mut Criterion) {
    let (mut store, _, _) = populate_entities(1500);

    let mut group = c.benchmark_group("slotmap_1500_entities");

    group.bench_function("iteration_read_positions", |b| {
        b.iter(|| {
            let mut sum_x = 0.0f32;
            for (_, entity) in store.iter() {
                sum_x += entity.position.0;
            }
            black_box(sum_x);
        });
    });

    group.bench_function("iteration_query_components", |b| {
        b.iter(|| {
            let mut total_hp = 0u32;
            for (_, entity) in store.iter() {
                if let Some(stats) = entity.components.get_component::<PlayerStats>() {
                    total_hp += stats.health;
                }
            }
            black_box(total_hp);
        });
    });

    group.bench_function("iteration_mutate_components", |b| {
        b.iter(|| {
            for (_, entity) in store.iter_mut() {
                if let Some(stats) = entity.components.get_component_mut::<PlayerStats>() {
                    stats.health = stats.health.wrapping_add(1);
                }
            }
        });
    });

    group.finish();
}

fn bench_simulation_tick(c: &mut Criterion) {
    let bounds = BoundingBox3D::new((-500.0, -500.0, -500.0), (500.0, 500.0, 500.0));
    let mut sim = TickSimulation::new(1, bounds);

    for i in 0..1000 {
        let guid = Guid::new(EntityType::Player, 1, i as u32);
        let x = ((i * 7) % 900) as f32 - 450.0;
        let y = ((i * 13) % 900) as f32 - 450.0;
        let z = ((i * 17) % 900) as f32 - 450.0;
        sim.spawn_entity(guid, (x, y, z));
    }

    let mut group = c.benchmark_group("simulation_tick_1000_entities");

    group.bench_function("step_tick_20hz", |b| {
        b.iter(|| {
            sim.step_tick();
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_octree_queries,
    bench_slotmap_iterations,
    bench_simulation_tick
);
criterion_main!(benches);
