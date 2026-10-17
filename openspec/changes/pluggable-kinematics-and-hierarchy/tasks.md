# Implementation Tasks: Pluggable Kinematics, Impulse Allowances & Entity Hierarchies

- [ ] 1. Parent-Child Transform Hierarchy in `crates/core/src/entity.rs`
  - [ ] 1.1 Add `velocity`, `collider_half_extents`, `parent_guid`, `local_position`, `local_rotation`, and `children` to `Entity` in `crates/core/src/entity.rs`.
  - [ ] 1.2 Implement `local_to_world`, `world_to_local`, `attach_child`, `detach_child`, `propagate_hierarchy_transforms`, and `collect_hierarchy_group` on `EntityStore`.
  - [ ] 1.3 Add unit tests in `crates/core/src/entity.rs` verifying multi-child attachment, rotation-aware transform propagation, cycle prevention, and group collection.

- [ ] 2. Pluggable `MotionIntegrator`, `Newtonian6DoF`, Volumetric Sweep & `ImpulseAllowance` (`crates/node/src/kinematics.rs`)
  - [ ] 2.1 Add `ImpulseAllowance`, `MotionMode` (`DiscreteGround`, `Newtonian6DoF`), and `MotionIntegrator` trait to `crates/node/src/kinematics.rs`.
  - [ ] 2.2 Implement `grant_impulse` on `KinematicValidator` and honour unexpired `ImpulseAllowance` in `validate_move_with_collision`.
  - [ ] 2.3 Wire volumetric `CollisionWorld::sweep_test` using `state.collider_half_extents` into `validate_move_with_collision`.
  - [ ] 2.4 Add unit tests in `crates/node/src/kinematics.rs` covering `ImpulseAllowance` knockback exemption, `Newtonian6DoF` acceleration limits, and volumetric `sweep_test` wall-clipping rejection.

- [ ] 3. Hierarchy Transform Propagation & Atomic Migration Grouping in `TickSimulation` (`crates/node/src/sim.rs`)
  - [ ] 3.1 In `TickSimulation` movement processing, propagate parent movement to attached children (`propagate_hierarchy_transforms`), updating `Octree` and child `EntityKinematicState::last_position`, and update `child.local_position` when a child moves on a parent platform.
  - [ ] 3.2 Update boundary migration checks in `TickSimulation` so child entities (`parent_guid.is_some()`) do not migrate independently and root parents collect their full hierarchy group (`collect_migration_group`).
  - [ ] 3.3 Add unit tests in `crates/node/src/sim.rs` verifying a passenger walking on a fast-moving ship passes kinematic validation and migrates as an atomic group with the ship.

- [ ] 4. Workspace Quality Gates
  - [ ] 4.1 Run `cargo fmt --all -- --check`
  - [ ] 4.2 Run `cargo clippy --workspace --all-targets -- -D warnings`
  - [ ] 4.3 Run `cargo test --workspace`
