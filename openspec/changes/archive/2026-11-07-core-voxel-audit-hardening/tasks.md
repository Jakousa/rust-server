# Implementation Tasks: Core Primitives & Voxel Engine Audit Hardening

- [x] 1. EventQueue Ephemeral Lifecycle & End-of-Tick Cleanup (`crates/core/src/plugin.rs`, `action.rs`, `crates/plugins/voxel/src/plugin.rs`, `crates/node/src/sim.rs`)
  - [x] 1.1 In `crates/core/src/plugin.rs`, add `ephemeral_types: HashSet<TypeId>` to `EventQueue` along with `register_ephemeral::<E>()`, `is_ephemeral::<E>()`, `clear_ephemeral()`, and `clear_type::<E>()`.
  - [x] 1.2 In `crates/core/src/plugin.rs`, add `fn end_of_tick(&mut self, _ctx: &mut TickContext) {}` to `TickSystem` trait.
  - [x] 1.3 In `crates/core/src/action.rs` and `crates/node/src/sim.rs`, register `ActionProgressEvent` as ephemeral in `EventQueue` during `ActionTickSystem::update` and `TickSimulation::with_kinematics`.
  - [x] 1.4 In `crates/plugins/voxel/src/plugin.rs`, register `VoxelChangeEvent` as ephemeral during `VoxelTerrainPlugin::build`.
  - [x] 1.5 In `crates/node/src/sim.rs`, invoke `end_of_tick` for all tick systems and call `event_queue.clear_ephemeral()` at the end of each tick in `TickSimulation::step`.
  - [x] 1.6 Add unit tests for `EventQueue` ephemeral lifecycle in `crates/core/src/plugin.rs` and verify `sim.rs` tick event clearing.

- [x] 2. Multi-Sector Spatial Index Completeness (`crates/core/src/world.rs`)
  - [x] 2.1 Add `max_extent(&self) -> f32` to `WorldObject` computing the maximum distance from `transform.position` to any bounding box edge, returning `0.0` if any coordinate is non-finite.
  - [x] 2.2 In `SectorSpatialIndex`, add `object_extents: HashMap<Guid, f32>` and `max_extent: f32`.
  - [x] 2.3 Update `insert`, `remove`, and `clear` to maintain `max_extent` dynamically, including recomputing `max_extent` when an existing largest object shrinks or is removed.
  - [x] 2.4 In `query_aabb`, reject non-finite query bounds and expand query bounds by `max_extent` before computing `min_sector` and `max_sector`.
  - [x] 2.5 Add unit tests in `crates/core/src/world.rs` verifying multi-sector query coverage for cross-boundary objects and `max_extent` recomputation on object removal and shrinking.

- [x] 3. Voxel Concurrency Optimization & Defensive Palette Bounds (`crates/plugins/voxel/src/modify.rs`, `world.rs`, `chunk.rs`)
  - [x] 3.1 In `crates/plugins/voxel/src/chunk.rs`, replace panicking `assert!(idx < CHUNK_VOXELS)` with non-panicking bounds checking in `PaletteChunk::get` (return 0) and `set` (return false), using `self.data.get(word_idx)` / `get_mut(word_idx)`.
  - [x] 3.2 In `crates/plugins/voxel/src/modify.rs`, break down modification validation into `validate_reach`, `validate_rate_limit`, and `apply_modification`.
  - [x] 3.3 In `crates/plugins/voxel/src/world.rs`, scope the rate limiter lock and drop it immediately before acquiring `chunk_arc.write()`.
  - [x] 3.4 Add concurrency and boundary tests verifying non-blocking multi-chunk modifications and safe out-of-bounds index handling.

- [x] 4. Numeric Safety & Clamping Hardening (`crates/core/src/attribute.rs`, `crates/core/src/action.rs`)
  - [x] 4.1 In `crates/core/src/attribute.rs`, update `get_current_by_id` to use `safe_clamp`.
  - [x] 4.2 In `crates/core/src/action.rs`, update `progress` (and add `progress_fraction` alias) to safely handle non-finite or non-positive durations with `safe_clamp`.
  - [x] 4.3 Add unit tests verifying `safe_clamp` usage with `NaN` and inverted bounds in `attribute.rs` and `action.rs`.

- [x] 5. Quality Gates & PR Auto-Merge
  - [x] 5.1 Run `cargo fmt --all -- --check`
  - [x] 5.2 Run `cargo clippy --workspace --all-targets -- -D warnings`
  - [x] 5.3 Run `cargo test --workspace`
  - [ ] 5.4 Push branch `fix/issue-167-core-voxel-audit` and create PR with auto-merge enabled.
