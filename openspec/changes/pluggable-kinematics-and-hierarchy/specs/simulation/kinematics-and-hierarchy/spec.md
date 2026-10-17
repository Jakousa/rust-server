# Pluggable Kinematics, Impulse Allowances, and Entity Hierarchy Specification

## Purpose
Enables both discrete ground movement and 6-DoF Newtonian vehicular/space physics, server-authorized impulse exemptions, parent-child relative coordinate frames, and atomic vehicle+passenger partition migration.

## Requirements

### Requirement: Pluggable Motion Integrators and Volumetric Collision Sweep
`KinematicValidator` SHALL support pluggable `MotionIntegrator` strategies (`DiscreteKinematicIntegrator` and `Newtonian6DoFIntegrator`) and invoke `CollisionWorld::sweep_test` using the entity's `collider_half_extents` during movement validation.

#### Scenario: Newtonian 6-DoF acceleration validation
- **GIVEN** an entity configured with `Newtonian6DoFIntegrator` (`max_acceleration = 20.0`, `max_speed = 100.0`)
- **WHEN** the entity accelerates within `20.0 m/s^2` up to `80.0 m/s`
- **THEN** movement validation SHALL succeed and update velocity; if acceleration exceeds `20.0 m/s^2` beyond tolerance, it SHALL return a `KinematicViolation`

### Requirement: Server-Authorized Impulse Allowance
`KinematicValidator::grant_impulse` SHALL register a temporary `ImpulseAllowance` for an entity that increases the allowed displacement and speed budget until `expires_at_tick`, preventing false-positive `SpeedHack` or `TeleportHack` rejections during server-initiated knockbacks or dashes.

#### Scenario: Knockback displacement with active impulse allowance
- **GIVEN** an entity with `max_speed = 7.0` and `max_teleport_distance = 15.0` granted an impulse allowance of `bonus_distance = 25.0` for `5` ticks
- **WHEN** the entity moves `30.0m` within the next tick
- **THEN** `validate_move_with_collision` SHALL accept the movement with zero violation penalty and consume the impulse allowance

### Requirement: Parent-Child Transform Hierarchy and Relative Kinematic Validation
`EntityStore` SHALL support attaching a child entity to a parent entity (`attach_child`, `detach_child`) and propagating parent world transforms (`propagate_hierarchy_transforms`) to all descendants. Movement validation for an attached child SHALL evaluate displacement in the parent's local coordinate frame.

#### Scenario: Passenger walking on a high-speed moving ship
- **GIVEN** Parent Ship S moving at `50.0 m/s` and Child Passenger P attached to S with `max_speed = 7.0 m/s`
- **WHEN** Ship S advances `2.5m` in a 50ms tick and Passenger P walks `0.3m` locally on the deck (`2.8m` total world displacement)
- **THEN** P's movement SHALL be validated against its `0.3m` local displacement (`6.0 m/s <= 7.0 m/s`) and accepted without anti-cheat violation

### Requirement: Atomic Parent-Child Hierarchy Partition Migration
When a parent entity with attached children crosses a spatial partition boundary, the simulation node SHALL group the parent and all transitive children into a single atomic migration batch and SHALL NOT migrate attached children independently of their root parent.

#### Scenario: Ship with attached passengers crosses partition boundary
- **GIVEN** Parent Ship S with 2 attached child passengers P1 and P2 crossing a partition boundary
- **WHEN** boundary migration is evaluated
- **THEN** S, P1, and P2 SHALL be collected together as an atomic migration group
