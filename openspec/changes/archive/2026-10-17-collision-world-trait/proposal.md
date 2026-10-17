# Proposal: Abstract CollisionWorld Trait & Server-Authoritative Spatial Raycasting

## Problem Statement

With the introduction of persistent world objects, constructible player structures, and dynamic prefabs, the current NPC AI and pathfinding subsystem relies on basic linear kinematic steering with leashing. Dynamic virtual worlds require authoritative spatial navigation capable of routing around geometry and obstacles. Furthermore, players could theoretically walk or cast abilities through solid structures without authoritative spatial raycasting.

However, to strictly adhere to the Prime Directive (`AGENTS.md`)—keeping the core engine genre-agnostic, unopinionated, and mathematically generalized (without hardcoding 2D grid, 3D voxel, or specific polygon NavMesh formats in core)—the core engine cannot assume a specific concrete spatial representation.

## Proposed Solution

1. **Abstract `CollisionWorld` Trait in `crates/core`**:
   - Define a mathematically unopinionated, genre-agnostic `CollisionWorld` trait exposing standardized spatial interfaces: `raycast()`, `sweep_test()`, `is_point_traversable()`, `register_obstacle()`, `unregister_obstacle()`, and `find_path()`.
   - Provide concrete default implementations in core (`EmptyCollisionWorld` and `AabbCollisionWorld`) to maintain zero game-specific assumptions.

2. **Kinematic Anti-Cheat with Collision Checking in `crates/node`**:
   - Extend `KinematicValidator` in `crates/node/src/kinematics.rs` to validate movement against any `CollisionWorld` instance without assuming underlying geometry.
   - Integrate collision validation into `TickSimulation`, rejecting moves through solid obstacles and snapping cheaters back to their last valid position.

3. **Dynamic Obstacle Carving & Slicing in `crates/node`**:
   - Synchronize dynamic obstacles with `CollisionWorld` on prefab placement, destruction, and sector loading/unloading.

4. **Decoupled Polygonal Navigation Mesh Plugin in `crates/gameplay`**:
   - Implement a modular `NavMeshWorld` / `NavMeshPlugin` in `crates/gameplay/src/navmesh.rs` supporting polygon meshes, dynamic obstacle carving (slicing / masking polygon connectivity), raycasting, and A* pathfinding with funnel string-pulling.
   - Expose navigation query interfaces to `GameplayPlugin` and NPC AI systems in `crates/gameplay/src/navigation.rs`.

5. **Authoritative Line-of-Sight Ability Verification in `crates/gameplay`**:
   - Provide line-of-sight raycasting hooks for combat and spellcasting in `crates/gameplay/src/ability.rs`, preventing players and NPCs from casting spells or firing through obstacles.

## Scope & Non-Goals

### In Scope
- Core `CollisionWorld` trait and collision hit types (`RayHit`, `SweepHit`) in `crates/core`.
- `AabbCollisionWorld` reference collision substrate in `crates/core`.
- `KinematicValidator` extension to enforce non-penetration anti-cheat via `CollisionWorld`.
- Sector and prefab placement hook synchronization for dynamic obstacles in `crates/node`.
- Decoupled `NavMeshWorld` and `NavMeshPlugin` in `crates/gameplay` with obstacle carving and pathfinding.
- Line-of-sight verification hooks in `crates/gameplay/src/ability.rs`.

### Non-Goals
- Core engine reliance on heavy C++ Recast bindings (using native, zero-dependency, memory-safe Rust polygon navigation structures).
- Client-side predictive physics engines (server remains authoritative).
- Physics simulation of ragdolls or soft bodies (pure kinematic and navigation collision bounds).
