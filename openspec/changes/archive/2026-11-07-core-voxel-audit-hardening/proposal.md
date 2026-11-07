# Proposal: Core Primitives & Voxel Engine Audit Hardening

## Problem Statement
During a comprehensive security, concurrency, and memory audit of `crates/core`, `crates/node`, and `crates/plugins/voxel` (GitHub Issue #167), critical vulnerabilities and concurrency bottlenecks were identified across core primitives and the voxel engine:

1. **Unbounded EventQueue Memory Leak**:
   - `ActionTickSystem::update` (`crates/core/src/action.rs`) emits an `ActionProgressEvent` on every simulation tick for every entity channeling or executing a timed action.
   - `VoxelTickSystem::update` (`crates/plugins/voxel/src/plugin.rs`) drains and pushes `VoxelChangeEvent`s into `ctx.events`.
   - In `TickSimulation::step` (`crates/node/src/sim.rs`), no consumer drains these ephemeral progress/change events. While `EventQueue::retain_recent(1024)` caps event counts per type, thousands of obsolete boxed event objects linger indefinitely, causing avoidable heap footprint and memory leaks in long-running simulation loops.
   - **Remediation**: Introduce an explicit ephemeral event lifecycle (`register_ephemeral`, `clear_ephemeral`) in `EventQueue`, an `end_of_tick` hook in `TickSystem`, and clear ephemeral per-tick events at tick boundaries in `TickSimulation::step`.

2. **Remote Panic DoS via Unvalidated Voxel Wire Coordinates & Unchecked Palette Indexing**:
   - `VoxelDeltaEntry::decode` previously permitted coordinates $\ge 16$, which maps to index $\ge 4096$, triggering an assertion panic in `PaletteChunk::set` and crashing the server node.
   - While wire coordinate checks exist in `net.rs`, `PaletteChunk::get` and `PaletteChunk::set` (`crates/plugins/voxel/src/chunk.rs`) still contain panicking `assert!(idx < CHUNK_VOXELS)`.
   - **Remediation**: Replace panicking assertions in `PaletteChunk::get` and `PaletteChunk::set` with safe bounds validation (returning default 0 or `false` on out-of-bounds indices), providing defense-in-depth against remote crash DoS.

3. **Infinite Loop & OOM in Entity Hierarchy Transform Propagation**:
   - `EntityStore::propagate_hierarchy_transforms` (`crates/core/src/entity.rs`) previously lacked a cycle-detection guard during BFS traversal. If entity parents/children formed a circular reference, the loop ran infinitely until memory exhaustion.
   - **Remediation**: Verify cycle guards using `seen: HashSet<Guid>` and ensure complete edge-case test coverage for cyclic hierarchies.

4. **Global Lock Contention in Voxel Modification**:
   - `VoxelWorld::modify_voxel` (`crates/plugins/voxel/src/world.rs`) acquires and holds the global `rate_limiter` Mutex while awaiting `chunk_arc.write()` and executing palette repacking.
   - This eliminates chunk-level concurrency: any player modifying a voxel in one chunk blocks all players modifying voxels in entirely different chunks across the world.
   - **Remediation**: Decompose modification into reach validation, rate-limiter check under a scoped Mutex that is dropped immediately, and isolated chunk write-lock acquisition.

5. **SectorSpatialIndex Multi-Sector Query Blindspot**:
   - `SectorSpatialIndex` (`crates/core/src/world.rs`) indexes `WorldObject` instances solely by their position origin (`transform.position`).
   - Objects with extents spanning across sector boundaries are missed by AABB queries in adjacent sectors because `query_aabb` only scans sectors spanning the query's bounding box without accounting for object bounds spilling over from adjacent sectors.
   - **Remediation**: Track maximum object extent in `SectorSpatialIndex` and expand query bounds by `max_extent` when calculating candidate sectors.

6. **Potential Panics in Attribute Value and Action Progress Clamping**:
   - `f32::clamp` panics if `min > max` or either bound or the value is `NaN`.
   - While `safe_clamp` was introduced for `AttributeValue::new` and `modify_current`, `AttributeSet::get_current_by_id` and `TimedAction::progress_fraction` still use bare `.clamp(...)` or unchecked division.
   - **Remediation**: Ensure all attribute and action progress calculations use `safe_clamp` or non-panicking bounds enforcement.

## Proposed Solution
- Core Primitives:
  - Add ephemeral event tracking to `EventQueue` (`mmo_core::plugin::EventQueue`) with `register_ephemeral::<E>()`, `clear_ephemeral()`, and `clear_type::<E>()`.
  - Add `end_of_tick(&mut self, ctx: &mut TickContext)` to `TickSystem`.
  - Register `ActionProgressEvent` as ephemeral in core and `VoxelChangeEvent` in the voxel plugin.
  - In `TickSimulation::step`, invoke `end_of_tick` and `clear_ephemeral` at the tick boundary.
  - In `SectorSpatialIndex`, track object extents and expand candidate sectors in `query_aabb` by `max_extent`.
  - In `crates/core/src/attribute.rs` and `crates/core/src/action.rs`, replace remaining raw `clamp` calls with `safe_clamp` and finite progress fractions.
- Voxel Engine:
  - In `crates/plugins/voxel/src/chunk.rs`, replace panicking `assert!(idx < CHUNK_VOXELS)` with defensive bounds handling.
  - In `crates/plugins/voxel/src/world.rs` and `modify.rs`, drop the rate-limiter lock before acquiring `chunk_arc.write()`.

## Out of Scope
- Redesigning the spatial sector grid into an dynamic R-tree or BVH (the uniform sector index with max-extent expansion is sufficient and retains $O(1)$ sector lookups).
- Modifying client-side prediction pipelines or WASM rendering logic.
