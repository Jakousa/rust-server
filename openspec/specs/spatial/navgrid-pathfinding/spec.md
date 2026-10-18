# Spatial A* Pathfinding and AI Obstacle Avoidance Specification

## Purpose
Provides a generalized, bounded A* pathfinding and line-of-sight string-pulling algorithm on `AabbCollisionWorld` and exposes `CollisionWorld` in `TickContext` so AI plugins navigate around multi-obstacle geometry.

## Requirements

### Requirement: Bounded A* Pathfinding with Line-of-Sight Smoothing
`AabbCollisionWorld::find_path(start, end)` SHALL compute an obstacle-free waypoint sequence from `start` to `end` using bounded A* lattice search (`max_expanded_nodes <= 2048`) followed by line-of-sight string-pulling so that every consecutive segment `(waypoints[i], waypoints[i + 1])` is unobstructed.

#### Scenario: Navigating around a U-shaped or multi-wall obstacle
- **GIVEN** an `AabbCollisionWorld` with multiple AABB walls blocking the direct line and single-corner bypass between `start = (0.0, 0.0, 0.0)` and `end = (10.0, 0.0, 0.0)`
- **WHEN** `find_path(start, end)` is called
- **THEN** it SHALL return `Some(waypoints)` starting at `start` and ending at `end` where every segment is raycast-clear of all obstacles

### Requirement: CollisionWorld Access in TickContext and AI Path Following
`TickContext` SHALL provide `collision_world: Option<&'a dyn CollisionWorld>`, and `FsmBrainSystem::update` SHALL compute and assign multi-waypoint paths via `NavAgentComponent::set_path` when chasing or evading around obstacles.

#### Scenario: NPC chases player around a wall
- **GIVEN** an NPC at `(0.0, 0.0, 0.0)` chasing a player at `(10.0, 0.0, 0.0)` separated by a solid wall at `x = 5.0, z in -4.0..4.0`
- **WHEN** `FsmBrainSystem::update` and `NavFollowSystem::update` execute across ticks
- **THEN** the NPC SHALL follow waypoints around the wall edge (`|z| > 4.0`) rather than walking through the wall
