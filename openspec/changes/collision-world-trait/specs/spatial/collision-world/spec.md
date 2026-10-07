# Specification: Abstract CollisionWorld Trait & Server-Authoritative Spatial Raycasting

## Purpose
Provides an unopinionated, genre-agnostic spatial collision and navigation interface in core, enabling authoritative anti-cheat trajectory validation, dynamic obstacle carving, line-of-sight raycasts, and decoupled polygonal navigation.

## Requirements

### Requirement: Abstract CollisionWorld Trait
The system SHALL define a genre-agnostic `CollisionWorld` trait in `crates/core` providing methods for raycasting, sweep testing, traversability testing, dynamic obstacle registration, and pathfinding with `SpatialQueryFilter` support to prevent self-intersection.

#### Scenario: Raycast hit detection with filter
- **GIVEN** a `CollisionWorld` containing an obstacle with known bounds
- **WHEN** a ray is cast from an origin through the obstacle with a filter ignoring the shooter's own GUID
- **THEN** the system SHALL return a `RayHit` containing the impact point, hit normal, distance, and obstacle entity identifier without self-intersection

#### Scenario: Raycast miss
- **GIVEN** a `CollisionWorld` with unobstructed space between origin and target
- **WHEN** a ray is cast along an unobstructed trajectory
- **THEN** the system SHALL return `None`

#### Scenario: Sweep test collision detection
- **GIVEN** an entity AABB at a start position
- **WHEN** a sweep test is evaluated along a non-zero translation vector towards an obstacle
- **THEN** the system SHALL return a `SweepHit` indicating the earliest hit fraction `[0.0, 1.0]` before collision

#### Scenario: Zero-length translation sweep early-out
- **GIVEN** an entity AABB
- **WHEN** a sweep test is evaluated with a translation vector of zero length
- **THEN** the system SHALL immediately return `None` without division by zero or NaN normals

### Requirement: Kinematics Validator Anti-Cheat Collision Verification
The `KinematicsValidator` SHALL validate proposed movement trajectories against the `CollisionWorld` without hardcoding specific geometry representations.

#### Scenario: Movement through solid obstacle rejected
- **GIVEN** a player moving from `current_pos` to `new_pos`
- **WHEN** the trajectory between `current_pos` and `new_pos` intersects solid collision geometry in `CollisionWorld`
- **THEN** `KinematicsValidator` SHALL reject the movement with an `ObstacleCollision` violation and snap the player back to `current_pos`

#### Scenario: Clear movement permitted
- **GIVEN** a player moving along an unobstructed path within speed limits
- **WHEN** the trajectory does not intersect any obstacles in `CollisionWorld`
- **THEN** `KinematicsValidator` SHALL permit the movement

### Requirement: Dynamic Obstacle Carving & Registration
The system SHALL support dynamic registration and unregistration of obstacles in `CollisionWorld` following strict lock hierarchy `entity_store -> sector_manager -> collision_world -> octree`.

#### Scenario: Prefab placement registers obstacle
- **GIVEN** an active simulation node and a `CollisionWorld`
- **WHEN** a constructible prefab or world object is placed
- **THEN** its bounding box SHALL be registered as a dynamic obstacle in `CollisionWorld`

#### Scenario: Prefab destruction unregisters obstacle
- **GIVEN** a registered obstacle in `CollisionWorld`
- **WHEN** the associated world object is destroyed or despawned
- **THEN** the obstacle SHALL be unregistered from `CollisionWorld`, restoring traversability

### Requirement: Decoupled Polygonal Navigation Mesh Plugin
The system SHALL provide a polygonal navigation mesh plugin in `crates/gameplay` that implements `CollisionWorld` and supports routing around static geometry and dynamic obstacles.

#### Scenario: Pathfinding around obstacle
- **GIVEN** a `NavMeshWorld` with a polygonal walkable surface and an obstacle blocking the direct line between start and goal
- **WHEN** `find_path(start, goal)` is requested
- **THEN** the system SHALL compute a collision-free waypoint corridor routing around the obstacle

#### Scenario: Dynamic obstacle carving alters paths
- **GIVEN** a previously open polygon corridor
- **WHEN** a dynamic obstacle is registered across the corridor
- **THEN** subsequent pathfinding requests SHALL re-route around the carved region

### Requirement: Authoritative Line-of-Sight Ability Verification
The gameplay system SHALL authoritatively verify line-of-sight with strict finite-coordinate checks before executing targeted abilities or spells.

#### Scenario: Ability cast through wall rejected
- **GIVEN** a caster and target separated by an obstacle blocking line of sight
- **WHEN** the caster attempts to execute a targeted ability on the target
- **THEN** the system SHALL reject the ability execution due to blocked line of sight

#### Scenario: Ability cast with non-finite coordinates rejected safely
- **GIVEN** a cast request with NaN or Infinity coordinates
- **WHEN** line of sight validation is executed
- **THEN** the system SHALL reject the request with an error without panicking
