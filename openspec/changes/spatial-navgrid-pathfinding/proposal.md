# Spatial 3D/2.5D A* Pathfinding Substrate & Obstacle-Avoiding NPC AI Proposal

## Context & Problem Statement
In a server-authoritative MMO, autonomous NPCs, creatures, drones, and companions must navigate around walls, terrain obstacles, and placed prefabs without walking through geometry or getting stuck on corners:

1. **Single-Corner Heuristic vs True A* Pathfinding in `AabbCollisionWorld` (`crates/core/src/collision.rs:410-455`)**:
   - Currently, `AabbCollisionWorld::find_path(start, end)` only checks if the direct line-of-sight ray hits an obstacle, and if so, tries just two offset corners (`wp1`, `wp2`) of that single obstacle.
   - It fails on multi-obstacle rooms, L-shaped corridors, U-shaped barriers, or staggered walls.

2. **`TickContext` Lacks `CollisionWorld` Reference (`crates/core/src/plugin.rs:280-286`)**:
   - `TickContext<'a>` exposes `entities`, `octree`, `events`, `tick_number`, and `dt_sec`, but does not pass `collision_world: Option<&'a dyn CollisionWorld>`.
   - Consequently, `FsmBrainSystem` (`crates/gameplay/src/ai_brain.rs:181-216`) sets `agent.set_target(dest)` (straight-line movement) instead of computing an obstacle-avoiding waypoint path via `CollisionWorld::find_path` and `agent.set_path(waypoints)`.

## Proposed Solution

1. **Bounded Lattice A* Pathfinding with Line-of-Sight String Pulling (`crates/core/src/collision.rs`)**:
   - Replace the 2-corner heuristic in `AabbCollisionWorld::find_path(start, end)` with a deterministic, bounded 3D/2.5D A* search (`max_expanded_nodes = 2048`, `step_size = 1.0`, clearance margin `0.4`) with Octile/Euclidean heuristic and greedy line-of-sight string-pulling (path smoothing) so straight segments are collapsed to minimal corner waypoints.

2. **Expose `collision_world` on `TickContext<'a>` (`crates/core/src/plugin.rs` & `crates/node/src/sim.rs`)**:
   - Add `pub collision_world: Option<&'a dyn crate::collision::CollisionWorld>` to `TickContext<'a>` and pass `Some(&**collision_guard)` in `TickSimulation::step`.

3. **Wire `FsmBrainSystem` to `CollisionWorld::find_path` (`crates/gameplay/src/ai_brain.rs`)**:
   - In `FsmBrainSystem::update`, when an NPC enters or updates `Chase` or `Evade` target destination, query `ctx.collision_world.and_then(|cw| cw.find_path(start, dest))` and populate `agent.set_path(waypoints)` when an obstacle-avoiding multi-waypoint path is returned.

## Scope & Non-Goals
- **In Scope**: `crates/core/src/{collision.rs, plugin.rs}`, `crates/node/src/sim.rs`, and `crates/gameplay/src/ai_brain.rs`.
- **Non-Goals**: Hardcoding genre-specific NPC behaviors into `crates/core`.
