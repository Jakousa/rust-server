# Pluggable Motion Integrators, Server Impulse Allowances & Parent-Child Entity Hierarchies Proposal

## Context & Problem Statement
To support the full spectrum of popular MMO genres—from Tab-Target RPGs (*WoW*, *FFXIV*) and Shooters (*Destiny*) to Space Simulations (*EVE*, *Star Citizen*) and Vehicular Sandboxes (*Sea of Thieves*, *ArcheAge*)—the engine must resolve four fundamental kinematic and spatial hierarchy gaps:

1. **Single Hardcoded Linear Speed Validator vs Pluggable Motion Models (`crates/node/src/kinematics.rs`)**:
   - `KinematicValidator` currently assumes a single scalar ground speed check (`distance / dt <= max_speed`) and only tests point traversability + center raycast (`collision.raycast` and `collision.is_traversable`), never invoking volumetric `collision.sweep_test(half_extents, start, end)`.
   - Space, flight, and vehicular MMOs require a **Newtonian 6-DoF Motion Model** (tracking velocity, acceleration bounds, and linear damping), while ground RPGs/Shooters require a **Discrete Kinematic Model** with volumetric AABB sweep tests.

2. **Server-Initiated Knockbacks, Dashes, and Teleports Trigger Anti-Cheat (`crates/node/src/kinematics.rs`)**:
   - When a server gameplay plugin applies a knockback, dash, or jump-pad impulse to an entity, the entity's sudden displacement exceeds `max_speed * dt` or `max_teleport_distance`, causing `KinematicValidator` to penalize the player with a `SpeedHack` or `TeleportHack` violation.

3. **Flat World Coordinates vs Parent-Child Relative Coordinate Frames ("The Moving Ship Problem", `crates/core/src/entity.rs`)**:
   - `Entity` only stores absolute world coordinates `(x, y, z)`. When players stand on a moving ship, airship, or multi-crew vehicle:
     - The ship's high velocity causes passengers' world-space displacements to trigger speed-hack rejections unless validated in the parent ship's local coordinate frame.
     - Moving the parent ship does not automatically transform and update attached passengers in the `Octree`.

4. **Independent Boundary Migration Splits Vehicles from Passengers (`crates/node/src/sim.rs:1168` & `network.rs`)**:
   - When a vehicle with 5 mounted passengers crosses an Octree partition boundary, independent per-entity boundary checks can migrate the vehicle on one tick and leave passengers behind or migrate them separately.

## Proposed Solution

1. **Pluggable `MotionIntegrator` Trait & Volumetric AABB Sweep (`crates/node/src/kinematics.rs`)**:
   - Define `MotionIntegrator` trait with built-in implementations:
     - `DiscreteKinematicIntegrator`: Ground/directional movement validation with volumetric `CollisionWorld::sweep_test` when `collider_half_extents` is non-zero.
     - `Newtonian6DoFIntegrator`: Inertial velocity/acceleration validation (`|dv| / dt <= max_acceleration`, `|v| <= max_speed`, linear damping) for space/vehicular entities.

2. **Server-Authorized `ImpulseAllowance` on `EntityKinematicState`**:
   - Add `ImpulseAllowance { pub max_bonus_distance: f32, pub expires_at_tick: u64 }` and `KinematicValidator::grant_impulse(guid, bonus_distance, duration_ticks, current_tick)` so server-authorized dashes, knockbacks, and teleports expand the allowed displacement window without false-positive violations.

3. **Parent-Child Transform Hierarchy in `Entity` & `EntityStore` (`crates/core/src/entity.rs`)**:
   - Add `pub parent_guid: Option<Guid>`, `pub local_position: (f32, f32, f32)`, `pub local_rotation: f32`, and `pub children: Vec<Guid>` to `Entity`.
   - Add `attach_child`, `detach_child`, and `propagate_hierarchy_transforms(parent_guid)` on `EntityStore` (computing rotated world positions `(px + lx * cos(prot) - lz * sin(prot), py + ly, pz + lx * sin(prot) + lz * cos(prot))` for all descendants and returning updated `(Guid, world_pos)` pairs so `TickSimulation` updates `Octree` and child `EntityKinematicState::last_position`).
   - Validate child movement packets in parent-relative local space so a ship moving at `100 m/s` never triggers anti-cheat on a passenger walking at `5 m/s` on deck.

4. **Atomic Hierarchy Group Migration (`crates/node/src/sim.rs`)**:
   - Child entities (`parent_guid.is_some()`) never trigger independent boundary migration; when a root parent crosses a partition boundary, `collect_hierarchy_group(parent_guid)` gathers the parent and all transitive children so they migrate together in a single batch (`execute_batch_entity_migration`).

## Scope & Non-Goals
- **In Scope**: `crates/core/src/entity.rs`, `crates/node/src/{kinematics.rs, sim.rs, network.rs}`.
- **Non-Goals**: Hardcoding specific vehicle types or spaceship classes in `core` or `node`.
