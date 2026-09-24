# Modular NPC & AI Specification

## Purpose

Defines autonomous non-player entity systems encompassing decoupled kinematic navigation, entity-driven spawners, sensory perception scanning, threat/aggro accounting, and modular finite state machine execution.

## Requirements

### Requirement: Kinematic NPC Navigation and Leashing
The system SHALL provide modular navigation components and systems to steer entities toward target positions or along predefined waypoints, enforcing speed limits and leashing boundaries.

#### Scenario: Waypoint following and arrival
- **WHEN** an NPC entity with a `WaypointComponent` reaches within the arrival distance of its active waypoint
- **THEN** the navigation system SHALL advance the waypoint index according to the path loop mode and steer toward the next node

#### Scenario: Exceeding maximum leash distance
- **WHEN** an NPC entity travels beyond its configured maximum leash radius from its spawn origin
- **THEN** the system SHALL trigger an evasion state, clearing threat targets and navigating the entity back to its spawn position

### Requirement: Entity-Driven Spawner Lifecycle
The system SHALL support spawner entities that periodically evaluate active population counts and spawn configured entity templates without mutating entity collections mid-tick.

#### Scenario: Spawner queues entity creation
- **WHEN** a spawner entity detects that its active spawn count is below maximum and its respawn timer has elapsed
- **THEN** the spawner system SHALL queue an entity instantiation into a deferred command buffer to be flushed at the tick boundary

#### Scenario: Tracking spawned entity death
- **WHEN** an entity spawned by a spawner dies or despawns
- **THEN** the spawner SHALL decrement its active spawn count and restart its respawn cooldown timer

### Requirement: Sensory Perception Scanning
The system SHALL provide perception scanning allowing entities to detect hostile, neutral, or friendly entities in their sensory range using spatial queries.

#### Scenario: Hostile target enters perception radius
- **WHEN** a hostile entity enters the configured perception radius and line-of-sight of an observing entity
- **THEN** the perception system SHALL emit a `HostileSpottedEvent` with the observer and detected entity GUIDs

### Requirement: Threat and Aggro Table Management
The system SHALL maintain dynamic threat tables for combat entities, accumulating threat from perception proximity, damage dealt, and support actions.

#### Scenario: Threat accumulation from damage
- **WHEN** an entity receives a `DamageEvent` from an attacker
- **THEN** the threat system SHALL add threat proportional to the damage amount and update the primary target if the attacker exceeds the current target's threat threshold

#### Scenario: Threat table reset on evasion
- **WHEN** an entity enters an evasion or reset state
- **THEN** the threat system SHALL clear all entries in the entity's threat table and drop combat engagement

### Requirement: Modular Finite State Machine Brain
The system SHALL support state-driven AI brains that transition entities through behavioral states including idle, patrol, chase, attack, and evade.

#### Scenario: Transition from patrol to chase on hostile detection
- **WHEN** an entity in `Patrol` state receives a hostile target from its threat table
- **THEN** the FSM brain SHALL transition the entity to `Chase` state and set the navigation target toward the hostile entity's position

#### Scenario: Transition from chase to attack in combat range
- **WHEN** an entity in `Chase` state closes within melee or spell attack range of its current target
- **THEN** the FSM brain SHALL transition to `Attack` state and emit combat actions in accordance with attack cooldowns
