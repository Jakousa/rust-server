# Implementation Tasks: Spatial A* Pathfinding & AI Obstacle Avoidance

- [ ] 1. Implement Bounded A* Pathfinding & String-Pulling in `AabbCollisionWorld` (`crates/core/src/collision.rs`)
  - [ ] 1.1 Replace the 2-corner fallback in `AabbCollisionWorld::find_path` with bounded A* search (`max_expanded_nodes = 2048`) and line-of-sight string-pulling.
  - [ ] 1.2 Add unit tests in `crates/core/src/collision.rs` verifying pathfinding around single walls, L-shaped barriers, U-shaped traps, and unreachable enclosed targets.

- [ ] 2. Expose `collision_world` on `TickContext` & Wire `FsmBrainSystem` (`crates/core`, `crates/node`, `crates/gameplay`)
  - [ ] 2.1 Add `pub collision_world: Option<&'a dyn crate::collision::CollisionWorld>` to `TickContext<'a>` in `crates/core/src/plugin.rs` and update `TickContext` initializers across `crates/node/src/sim.rs` and tests.
  - [ ] 2.2 Update `FsmBrainSystem::update` (`crates/gameplay/src/ai_brain.rs`) to compute obstacle-avoiding paths via `ctx.collision_world` and call `agent.set_path(...)`.
  - [ ] 2.3 Add integration test in `crates/gameplay/src/ai_brain.rs` verifying an NPC navigates around a wall obstacle to reach a player.

- [ ] 3. Workspace Quality Gates
  - [ ] 3.1 Run `cargo fmt --all -- --check`
  - [ ] 3.2 Run `cargo clippy --workspace --all-targets -- -D warnings`
  - [ ] 3.3 Run `cargo test --workspace`
