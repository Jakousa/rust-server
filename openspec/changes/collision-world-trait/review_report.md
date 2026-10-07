# Architectural & Code Review: collision-world-trait

## Summary of Findings
The proposal correctly identifies the need for an abstract, unopinionated `CollisionWorld` trait to keep the core engine genre-agnostic (adhering to `AGENTS.md`). The separation of generalized raycasting/sweeps in `crates/core` from polygonal NavMesh implementation in `crates/gameplay` is excellent. However, there are critical gaps in lock ordering definition, floating-point safety (NaN/Inf), raycast self-intersection, and task granularity.

## Critical Architectural Assessment

### 1. Concurrency, Race Conditions & Deadlocks
- **Lock Ordering Omission:** The design specifies the lock order as `1. entity_store 2. collision_world 3. octree`. However, `SectorManager` (which handles sector loading/unloading) is not placed in this hierarchy. Since dynamic obstacles are registered/unregistered on sector load/unload, if `SectorManager` holds locks while calling `collision_world` methods, there is a severe deadlock risk if another thread acquires `collision_world` and then queries `SectorManager`. The exact lock tier of `SectorManager` must be explicitly defined in the proposal.
- **RwLock Contention:** Using `Arc<RwLock<dyn CollisionWorld>>` on `TickSimulation` is mostly fine for reads, but dynamic carving (write lock) during sector loads or bulk prefab placements could cause massive latency spikes on the main simulation tick. Consider lock-free concurrent structures or batching collision world updates at the start/end of the tick.

### 2. Security, Anti-Cheat & Malformed Input Handling
- **NaN/Inf Handling in Line-of-Sight:** In `validate_ability_line_of_sight` (`design.md`), if `caster_pos` or `target_pos` contain `NaN` or `Infinity`, `dir.length()` will be `NaN`. The condition `dist < 1e-4` evaluates to `false` for `NaN`, leading to `.normalized().unwrap()`, which will panic and crash the server. Explicit `.is_finite()` checks must be added.
- **Raycast Self-Intersection:** The `raycast()` interface does not accept an ignored entity ID or an origin offset. When a caster fires a ray from `caster_pos` (which is typically inside their own bounding box or NavMesh cylinder), the raycast will immediately hit the caster's own obstacle geometry. The `CollisionWorld::raycast` signature needs a way to filter or ignore the caster (e.g., `ignore_entity: Option<Guid>`) or rely on a distinct collision layer mask.
- **Speed Cap & Tolerance Coordination:** `KinematicValidator` snaps cheaters back to `current_pos`. This is correct, but floating-point errors in `sweep_test` can cause legitimate players skimming walls to be rejected. A small penetration tolerance or slide-along-wall (sliding vector) response should be considered instead of a hard reject and snap-back for minor collisions.

### 3. Edge Cases & Blackout Scenarios
- **Unloaded Sectors:** If a player moves or casts into an unloaded sector, `CollisionWorld` will lack obstacles there. This allows players to cheat by moving through walls that haven't streamed in yet. The spatial system must treat unloaded sector boundaries as opaque/impassable, or `KinematicValidator` must reject movement into unpopulated sector bounds.
- **Zero-Length Translations:** `sweep_test` with a zero-length vector could cause divide-by-zero or `NaN` normals. The implementation must explicitly early-out on `translation.length_squared() < EPSILON`.
- **Degenerate Polygons in NavMesh Carving:** Dynamic polygon carving (slicing) is notoriously prone to floating-point robustness issues. Coplanar vertices or collinear edges during AABB intersection can yield degenerate polygons, crashing the pathfinder.

### 4. Task Granularity & Verification Feasibility
- **NavMesh Implementation is Too Coarse:** Tasks 3.1, 3.2, and 3.3 propose implementing a complete custom Polygon NavMesh with dynamic boolean carving, A* heuristic search, and Funnel string-pulling. This is a massive, highly complex undertaking that merits its own dedicated OpenSpec change. It is too large for a single PR alongside the core trait refactor.

## Concrete Recommendations / Adjustments

1. **Update Lock Hierarchy:** Explicitly place `SectorManager` in the lock hierarchy in `design.md` (e.g., `1. entity_store 2. sector_manager 3. collision_world 4. octree`) to prevent sector-streaming deadlocks.
2. **Add `.is_finite()` Guards:** Ensure all gameplay hooks (like `validate_ability_line_of_sight`) validate that input vectors are finite before performing vector math.
3. **Enhance Trait Signatures:**
   - Update `CollisionWorld::raycast` to take `QueryFilter` or `ignore_id: Option<Guid>` to prevent self-intersection.
   - Update `CollisionWorld::sweep_test` to similarly accept an ignore list.
4. **Handle Unloaded Space:** Require `CollisionWorld` or `KinematicValidator` to reject traversal into unloaded spatial domains.
5. **Split the NavMesh Task:** Move the implementation of the decoupled `NavMeshWorld` (Task 3) into a separate, subsequent OpenSpec. For this PR, implement the `CollisionWorld` trait, `AabbCollisionWorld`, and the kinematic validation hooks.

## Final Verdict
**APPROVED WITH MINOR ADJUSTMENTS**
