use criterion::{black_box, criterion_group, criterion_main, Criterion};
use first_rust_project::world::World;

fn benchmark_world_update(c: &mut Criterion) {
    let mut world = World::new();

    c.bench_function("world_update", |b| b.iter(|| {
        world.update();
    }));
}

criterion_group!(benches, benchmark_world_update);
criterion_main!(benches);