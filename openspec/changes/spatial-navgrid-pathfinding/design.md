# Technical Design: Spatial A* Pathfinding & AI Obstacle Avoidance

## 1. Bounded A* Search with String-Pulling in `AabbCollisionWorld::find_path` (`crates/core/src/collision.rs:410-455`)

### 1.1 Fast Paths
1. Check `if !self.is_traversable(start) || !self.is_traversable(end) { return None; }`.
2. Let `diff = end - start; let dist = diff.length();`. If `dist < 1e-4`, return `Some(vec![start])`.
3. If `self.raycast(start, diff.normalize(), dist, None).is_none()`, return `Some(vec![start, end])`.

### 1.2 Discretized 3D/2.5D A* Lattice Search
- Grid origin anchored at `start` with `STEP: f32 = 1.0` and agent clearance radius `CLEARANCE: f32 = 0.4` (`half_extents = Vec3::new(0.4, 0.4, 0.4)`).
- Node coordinate `(ix, iy, iz): (i32, i32, i32)` maps to world point `pos = Vec3::new(start.x + ix as f32 * STEP, start.y + iy as f32 * STEP, start.z + iz as f32 * STEP)`.
- Goal cell `goal_cell = ((end.x - start.x).round() as i32, (end.y - start.y).round() as i32, (end.z - start.z).round() as i32)`.
- Priority queue `BinaryHeap<Reverse<(OrderedF32, (i32, i32, i32))>>` with `g_score: HashMap<(i32, i32, i32), f32>`, `came_from: HashMap<(i32, i32, i32), (i32, i32, i32)>`, and `MAX_EXPANDED: usize = 2048`.
- Neighbors: 8 planar XZ directions `(dx, 0, dz)` (cost `1.0` for cardinal, `1.4142` for diagonal) plus vertical `(0, dy, 0)` when `(end.y - start.y).abs() > 0.5`.
- Transition validation from `curr_pos` to `next_pos`:
  - `self.is_traversable(next_pos)` and `self.sweep_test(Vec3::new(CLEARANCE, CLEARANCE, CLEARANCE), curr_pos, next_pos, None).is_none()`.
- When a node is within `<= 1.5 * STEP` of `end` and has clear sweep/raycast to `end`, reconstruct the raw waypoint chain `[start, ..., end]`.

### 1.3 Line-of-Sight String Pulling (Path Smoothing)
- Given raw path `raw: Vec<Vec3>`:
  ```rust
  let mut smoothed = vec![raw[0]];
  let mut current_idx = 0;
  while current_idx + 1 < raw.len() {
      let mut furthest_idx = current_idx + 1;
      for candidate_idx in ((current_idx + 2)..raw.len()).rev() {
          let seg = raw[candidate_idx] - raw[current_idx];
          let len = seg.length();
          if len > 1e-4
              && self.sweep_test(Vec3::new(CLEARANCE, CLEARANCE, CLEARANCE), raw[current_idx], raw[candidate_idx], None).is_none()
          {
              furthest_idx = candidate_idx;
              break;
          }
      }
      smoothed.push(raw[furthest_idx]);
      current_idx = furthest_idx;
  }
  ```

---

## 2. Expose `collision_world` on `TickContext<'a>` (`crates/core/src/plugin.rs` & `crates/node/src/sim.rs`)

- In `crates/core/src/plugin.rs:280-286`:
  ```rust
  pub struct TickContext<'a> {
      pub tick_number: u64,
      pub dt_sec: f32,
      pub entities: &'a mut EntityStore,
      pub octree: &'a mut Octree,
      pub events: &'a mut EventQueue,
      pub collision_world: Option<&'a dyn crate::collision::CollisionWorld>,
  }
  ```
- In `crates/node/src/sim.rs:620-626`, pass `collision_world: Some(&**collision_guard)` into `TickContext`.

---

## 3. Wire `FsmBrainSystem` to `CollisionWorld::find_path` (`crates/gameplay/src/ai_brain.rs:211-218`)

- When `set_nav_target = Some(Some(dest))` in `FsmBrainSystem::update`:
  - Read `entity_pos = entity.position;`.
  - If `let Some(cw) = ctx.collision_world`:
    - Call `cw.find_path(Vec3::new(entity_pos.0, entity_pos.1, entity_pos.2), Vec3::new(dest.0, dest.1, dest.2))`.
    - If `Some(waypoints)` is returned with `waypoints.len() > 1`:
      - Convert `waypoints[1..]` to `Vec<(f32, f32, f32)>` and call `agent.set_path(tuple_waypoints)`!
    - Else call `agent.set_target(dest)`.
