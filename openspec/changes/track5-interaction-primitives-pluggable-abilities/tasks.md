# Implementation Tasks: Track 5 - Interaction Primitives & Pluggable Abilities

- [ ] **Task 1: Core Timed Action Primitives (`crates/core`)**
  - Implement `TimedAction`, `ActionQueue`, `ActionInterruptFlags`, and action lifecycle events (`ActionStartedEvent`, `ActionProgressEvent`, `ActionCompletedEvent`, `ActionInterruptedEvent`).
  - Implement `ActionTickSystem` updating action elapsed time during tick simulation.
  - Implement `ComponentCodec` for `ActionQueue`.
  - Add unit tests for action queueing, progression, completion, and interruption.

- [ ] **Task 2: Core Dynamic Attribute & Modifier System (`crates/core`)**
  - Implement `AttributeSet`, `AttributeValue`, `ModifierType`, `TimedModifier`, and `AuraExpiredEvent`.
  - Implement deterministic calculation formula: $(\text{Base} + \sum \text{Flat}) \times (1.0 + \sum \text{Percent})$ with bounds clamping.
  - Implement `AttributeTickSystem` for passive attribute regeneration, decay, and timed modifier expiration.
  - Implement `ComponentCodec` for `AttributeSet`.
  - Add unit tests for scalar clamping, flat/percent modifier stacking, order-independent removal, and regen determinism.

- [ ] **Task 3: Authoritative 3D Spatial Target Queries (`crates/core`)**
  - Implement `SpatialTargetQuery` geometric query methods against `Octree`: Point-to-Point, Raycast / Line-of-Sight (against authoritative Octree AABB bounding volumes), Cone / Cleave, and Sphere / Radial AoE.
  - Add angle/facing math helpers and bounding-volume collision queries.
  - Add unit tests for range filtering, facing angles, and radial gathering.

- [ ] **Task 4: Simulation Tick & Kinematic Interruption Integration (`crates/node`)**
  - Integrate `ActionTickSystem` and `AttributeTickSystem` into the 20 Hz simulation loop (`sim.rs`) following the strict phase ordering (Movement $\to$ Actions $\to$ Attributes $\to$ Plugin Event Draining).
  - Wire kinematic movement validation: moving with non-zero displacement synchronously aborts active actions before position commit when `cancel_on_movement` is enabled.
  - Wire damage events: suffering damage aborts active actions when `cancel_on_damage` is enabled.
  - Add integration tests verifying interruption guarantees under simulated movement.

- [ ] **Task 5: Pluggable Ability & Upgraded Combat Systems (`crates/gameplay`)**
  - Implement `AbilityPlugin` with `CooldownTracker`, GCD tracking, and resource validation.
  - Refactor `CombatPlugin` to consume `AttributeSet` for health/armor/resources and `TimedAction` for cast bars.
  - Wire ability completion to damage events and `ThreatPlugin` aggro generation.
  - Add end-to-end gameplay plugin tests.

- [ ] **Task 6: Verification, CI Quality Gates & PR Workflow**
  - Format workspace: `cargo fmt --all -- --check`.
  - Lint workspace: `cargo clippy --workspace --all-targets -- -D warnings`.
  - Run full test suite: `cargo test --workspace`.
  - Push branch and create PR via `gh pr create` with auto-merge enabled.
