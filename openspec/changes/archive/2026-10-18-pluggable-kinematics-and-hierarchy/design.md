# Technical Design: Pluggable Kinematics, Impulse Allowances & Parent-Child Hierarchies

## 1. Parent-Child Hierarchy in `crates/core/src/entity.rs`

### 1.1 `Entity` Fields
Add to `Entity`:
```rust
pub velocity: (f32, f32, f32),
pub collider_half_extents: (f32, f32, f32),
pub parent_guid: Option<Guid>,
pub local_position: (f32, f32, f32),
pub local_rotation: f32,
pub children: Vec<Guid>,
```
Default values in `Entity::new` / `Entity::with_components`:
- `velocity: (0.0, 0.0, 0.0)`
- `collider_half_extents: (0.4, 0.9, 0.4)`
- `parent_guid: None`
- `local_position: (0.0, 0.0, 0.0)`
- `local_rotation: 0.0`
- `children: Vec::new()`

### 1.2 Transform Math & `EntityStore` Hierarchy Methods
- `pub fn local_to_world(parent_pos: (f32, f32, f32), parent_yaw: f32, local_pos: (f32, f32, f32)) -> (f32, f32, f32)`:
  ```rust
  let (sin_y, cos_y) = parent_yaw.sin_cos();
  (
      parent_pos.0 + local_pos.0 * cos_y - local_pos.2 * sin_y,
      parent_pos.1 + local_pos.1,
      parent_pos.2 + local_pos.0 * sin_y + local_pos.2 * cos_y,
  )
  ```
- `pub fn world_to_local(parent_pos: (f32, f32, f32), parent_yaw: f32, world_pos: (f32, f32, f32)) -> (f32, f32, f32)`:
  ```rust
  let dx = world_pos.0 - parent_pos.0;
  let dy = world_pos.1 - parent_pos.1;
  let dz = world_pos.2 - parent_pos.2;
  let (sin_y, cos_y) = (-parent_yaw).sin_cos();
  (
      dx * cos_y - dz * sin_y,
      dy,
      dx * sin_y + dz * cos_y,
  )
  ```
- On `EntityStore`:
  - `pub fn attach_child(&mut self, child_guid: Guid, parent_guid: Guid, local_pos: (f32, f32, f32), local_rot: f32) -> bool`: prevents self-attachment and cycles, detaches from any previous parent, sets `child.parent_guid = Some(parent_guid)`, `child.local_position = local_pos`, `child.local_rotation = local_rot`, adds `child_guid` to `parent.children`, and updates `child.position` and `child.rotation`.
  - `pub fn detach_child(&mut self, child_guid: Guid) -> bool`: removes `child_guid` from parent's `children` and clears `child.parent_guid = None`.
  - `pub fn propagate_hierarchy_transforms(&mut self, root_guid: Guid) -> Vec<(Guid, (f32, f32, f32), (f32, f32, f32))>`: BFS/DFS traversal updating each descendant's `(position, rotation)` from its parent's world transform and returning `Vec<(child_guid, old_world_pos, new_world_pos)>`.
  - `pub fn collect_hierarchy_group(&self, root_guid: Guid) -> Vec<Guid>`: returns `[root_guid]` followed by all transitive children in topological parent-first order.

---

## 2. Pluggable `MotionIntegrator`, Volumetric Sweep & `ImpulseAllowance` (`crates/node/src/kinematics.rs`)

### 2.1 `ImpulseAllowance` & `MotionMode`
Add to `EntityKinematicState`:
```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImpulseAllowance {
    pub bonus_distance: f32,
    pub expires_at_tick: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MotionMode {
    DiscreteGround,
    Newtonian6DoF {
        max_acceleration: f32,
        linear_damping: f32,
    },
}
```
Add fields to `EntityKinematicState`:
- `pub velocity: (f32, f32, f32)`
- `pub collider_half_extents: (f32, f32, f32)`
- `pub motion_mode: MotionMode`
- `pub impulse_allowance: Option<ImpulseAllowance>`

### 2.2 `MotionIntegrator` Trait
```rust
pub trait MotionIntegrator: Send + Sync {
    fn validate_step(
        &self,
        config: &KinematicConfig,
        state: &mut EntityKinematicState,
        proposed_pos: (f32, f32, f32),
        current_tick: u64,
        collision: Option<&dyn CollisionWorld>,
        mover_guid: Option<Guid>,
    ) -> Result<(f32, f32, f32), KinematicViolation>;
}
```
- In `validate_move_with_collision`:
  1. Check if `state.impulse_allowance` is active (`expires_at_tick >= current_tick`). If active, add `allowance.bonus_distance` to `effective_max_teleport` and `allowance.bonus_distance / elapsed_seconds` to `effective_max_speed`, and clear `state.impulse_allowance = None` once consumed.
  2. If `state.motion_mode` is `MotionMode::Newtonian6DoF { max_acceleration, .. }`, validate acceleration `|v_new - state.velocity| / elapsed_seconds <= max_acceleration * config.tolerance_multiplier` alongside `effective_max_speed`.
  3. In collision validation (`if let Some(cw) = collision`), in addition to `cw.is_traversable` and `cw.raycast`, if `state.collider_half_extents.0 > 0.0`, call `cw.sweep_test(Vec3::from_tuple(state.collider_half_extents), start, end, mover_guid)`: if `Some(hit)` with `hit.distance < distance`, reject with `KinematicViolation::WallClipping`.

---

## 3. Hierarchy Movement & Atomic Group Migration in `TickSimulation` (`crates/node/src/sim.rs`)

- When a parent entity moves in `process_client_move_with_seq` / `process_client_move_deterministic`:
  - If `!entity.children.is_empty()`, call `let moved_children = self.entity_store.propagate_hierarchy_transforms(guid);` and for each `(child_guid, old_pos, new_pos)`, update `self.octree` and update `child_k_state.last_position = new_pos` so the parent's motion doesn't count against the child's local displacement!
- When a child entity (`parent_guid = Some(p_guid)`) moves:
  - Validate the child's displacement relative to the parent's movement (because `propagate_hierarchy_transforms` already updated `child_k_state.last_position` by the parent's delta, `distance(child_k_state.last_position, proposed_world_pos)` is **exact local displacement**!), and upon acceptance update `child.local_position = EntityStore::world_to_local(parent_pos, parent_rot, accepted_pos)`.
- In `check_boundary_migration`:
  - Skip any entity with `entity.parent_guid.is_some()`.
  - Provide `TickSimulation::collect_migration_group(&self, root_guid: Guid) -> Vec<Guid>` delegating to `self.entity_store.collect_hierarchy_group(root_guid)`.
