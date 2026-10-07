# Implementation Tasks: Abstract CollisionWorld Trait & Server-Authoritative Spatial Raycasting

## 1. Core CollisionWorld Primitives (`crates/core`)
- [x] 1.1 Define `SpatialQueryFilter`, `RayHit`, `SweepHit`, and `CollisionWorld` trait in `crates/core/src/spatial.rs` (or `collision.rs`).
- [x] 1.2 Implement `EmptyCollisionWorld` and `AabbCollisionWorld` in `crates/core` with raycast, sweep test (including zero-length translation guard and normal extraction), traversability, and obstacle registration.
- [x] 1.3 Add unit tests and proptests for `AabbCollisionWorld` (delegate boilerplate/test cases to local subagent `qwen` where appropriate).

## 2. Server-Authoritative Kinematics & Collision Anti-Cheat (`crates/node`)
- [x] 2.1 Add `KinematicViolationReason::ObstacleCollision` to `crates/node/src/kinematics.rs`.
- [x] 2.2 Add `validate_move_with_collision` to `KinematicValidator` in `crates/node/src/kinematics.rs` ensuring movement across obstacles is rejected and snapped back, respecting `SpatialQueryFilter`.
- [x] 2.3 Integrate `CollisionWorld` with `TickSimulation` in `crates/node/src/sim.rs`:
  - Initialize thread-safe `collision_world: Arc<parking_lot::RwLock<dyn CollisionWorld>>` adhering to lock hierarchy `entity_store -> sector_manager -> collision_world -> octree`.
  - Validate player move packets against `collision_world`.
  - Sync dynamic obstacles on prefab placement (`handle_place_world_object`), destruction (`handle_destroy_world_object`), and sector loading/unloading.
- [x] 2.4 Add unit and integration tests verifying obstacle collision rejection and dynamic obstacle lifecycle in simulation movement.

## 3. Decoupled NavMesh Plugin & Dynamic Carving (`crates/gameplay`)
- [x] 3.1 Implement polygonal navigation structures (`NavMeshPolygon`, `NavMeshWorld`) in `crates/gameplay/src/navmesh.rs` implementing `CollisionWorld`.
- [x] 3.2 Implement dynamic obstacle carving/slicing on `NavMeshWorld` (marking overlapping polygons impassable and updating adjacency).
- [x] 3.3 Implement A* corridor search and Funnel string-pulling algorithm on `NavMeshWorld::find_path`.
- [x] 3.4 Integrate `NavMeshPlugin` into `crates/gameplay` tick system and update `crates/gameplay/src/navigation.rs` for pathfinding steering.
- [x] 3.5 Add comprehensive unit and routing tests verifying pathfinding around static and dynamic obstacles.

## 4. Authoritative Line-of-Sight Ability Verification (`crates/gameplay`)
- [x] 4.1 Implement `validate_ability_line_of_sight` in `crates/gameplay/src/ability.rs` with `.is_finite()` guards and `SpatialQueryFilter { ignore_id: caster_guid, .. }`.
- [x] 4.2 Provide `execute_or_start_ability_with_collision` hook rejecting casts when line-of-sight is blocked by obstacles.
- [x] 4.3 Add unit tests verifying spellcast blocking through obstacles and successful cast on clear LoS.

## 5. Quality Gates & Integration Verification
- [x] 5.1 Run full workspace quality gates (`cargo fmt --all -- --check`).
- [x] 5.2 Run clippy with strict denial of warnings (`cargo clippy --workspace --all-targets -- -D warnings`).
- [x] 5.3 Run full workspace test suite (`cargo test --workspace`).
