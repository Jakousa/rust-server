## Why

Modern MMO engines require autonomous non-player entities (NPCs, creatures, ambient wildlife, vendors), but hardcoding "monsters" or "aggro" into the core engine violates domain-agnostic architecture and limits server flexibility. Different MMO genres demand completely different AI paradigms: social virtual worlds require waypoint walking and dialogue with zero combat; survival games require prey-fleeing behaviors; and classic fantasy RPGs require threat/hate tables, aggro radii, and ability rotations.

This change introduces a fully decoupled, ECS-native NPC and AI subsystem within `crates/gameplay` that provides modular components for navigation steering, entity-driven spawners, perception, threat accounting, and finite state machines without coupling to `crates/core` or compromising the 20 Hz tick simulation budget.

## What Changes

- **Navigation Steering (`NpcMovementPlugin`)**:
  - `NavAgentComponent` and `WaypointComponent` for linear kinematic waypoint following and directional steering.
  - `NavSteeringSystem` driving entity movement along paths with arrival tolerance and speed validation.
  - `LeashComponent` and leashing logic to bound NPC wander distances and trigger reset behavior.
- **Entity-Driven Spawner System (`SpawnerPlugin`)**:
  - `SpawnerComponent` attached to static world entities managing template IDs, active spawn limits, spawn radii, and respawn cooldown timers.
  - Deferred structural command buffer ensuring entity creations/destructions occur at tick boundaries without violating ECS aliasing rules.
- **Sensory Perception (`PerceptionPlugin`)**:
  - `PerceptionComponent` defining sight radius, field-of-view, and faction affinities.
  - Periodic spatial scanning via `Octree3D` emitting `HostileSpottedEvent` when targets enter detection ranges.
- **Threat & Aggro System (`ThreatPlugin`)**:
  - `ThreatTableComponent` maintaining threat scores across target entities.
  - Decoupled listeners for `HostileSpottedEvent`, `DamageEvent`, and healing events to update threat scores without direct component mutation coupling.
- **Combat & Ambient AI Brains (`MonsterAiPlugin`)**:
  - `FsmBrainComponent` executing classic MMO behavior states (`Idle`, `Patrol`, `Chase`, `Attack`, `Evade/ReturnHome`).
  - Seamless integration with `CombatPlugin` for attack actions and `NpcMovementPlugin` for pursuit.

## Capabilities

### New Capabilities
- `gameplay/npc-ai`: Autonomous non-player entity systems encompassing decoupled kinematic navigation, entity-driven spawners, sensory perception scanning, threat/aggro accounting, and modular finite state machine execution.

### Modified Capabilities
<!-- None: existing core plugin and content plugin contracts remain unchanged -->

## Impact

- **`crates/gameplay`**: Adds new modules `navigation.rs`, `spawner.rs`, `perception.rs`, `threat.rs`, `ai_brain.rs`.
- **`crates/core`**: Unchanged; reuses existing `EntityStore`, `Octree3D`, `EventQueue`, `HookDispatcher`, and `TickSystem`.
- **`crates/node`**: End-to-end integration tests verify NPC spawning, waypoint patrol, aggro detection, combat engagement, and leash reset within `TickSimulation`.
