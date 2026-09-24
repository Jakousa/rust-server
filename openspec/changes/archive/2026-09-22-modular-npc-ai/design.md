## Context

The engine currently executes a 20 Hz fixed-step simulation in `crates/node` driven by `TickSimulation`, which runs registered `TickSystem`s from plugins and processes kinematic player movements. `crates/gameplay` contains reference plugins for inventory, combat, trade, loot, quests, guilds, and parties. Non-player entities (NPCs) require autonomous behavioral steering, spawning lifecycles, sensory detection, threat management, and combat decision-making without coupling `crates/core` to game-specific genre concepts.

## Goals / Non-Goals

**Goals:**
- Provide standalone, decoupled plugins in `crates/gameplay`:
  1. `NpcMovementPlugin`: Kinematic steering, waypoints, arrival detection, leashing bounds.
  2. `SpawnerPlugin`: Static spawner entities, respawn timers, deferred structural creation buffer.
  3. `PerceptionPlugin`: Spatial detection via `Octree3D`, line-of-sight checks, `HostileSpottedEvent`.
  4. `ThreatPlugin`: Threat/hate tables, aggro thresholds, damage/heal threat multipliers.
  5. `MonsterAiPlugin`: Concrete ECS state machine (`Idle`, `Patrol`, `Chase`, `Attack`, `Evade`).
- Preserve strict zero-allocation per-tick performance in combat and steering.
- Ensure structural mutations (spawning/despawning entities) never violate Rust ECS aliasing rules.

**Non-Goals:**
- Full 3D polygonal NavMesh / Recast pathfinding (deferred to a future spatial geometry milestone; linear kinematic steering with leashing is sufficient for current open-world bounds).
- Complex hierarchical Behavior Tree authoring tools or visual editors.
- Hardcoding monster definitions into `crates/core` or `crates/node`.

## Decisions

### 1. ECS-Native Concrete Components over Dynamic Trait Objects
- **Decision**: Model AI brains as concrete ECS data components (`FsmBrainComponent`, `WaypointComponent`) rather than `Box<dyn AiBrain>` trait objects.
- **Rationale**: Iterating over dynamic trait objects incurs heap dereferencing and vtable calls that thrash the CPU L1/L2 cache when ticking thousands of entities at 20 Hz. Concrete components allow cache-coherent linear iteration in specialized tick systems (`fsm_brain_system`, `nav_steering_system`).
- **Alternative Considered**: `Box<dyn AiBrain>`: Rejected due to cache locality penalties and awkward borrow-checking semantics when passing simulation context across trait boundaries.

### 2. Event-Decoupled Sensory Perception and Threat Accounting
- **Decision**: Split perception scanning and threat table tracking into separate plugins communicating strictly through `EventQueue` (`HostileSpottedEvent`, `DamageEvent`).
- **Rationale**: If `PerceptionSystem` directly mutated another entity's `ThreatTableComponent`, it would require simultaneous mutable borrows across distinct entities in `EntityStore`, triggering Rust borrow checker conflicts. Emitting events into the deferred `EventQueue` decouples the borrow lifetimes cleanly. Furthermore, non-combat games (e.g. social or economic simulations) can use `PerceptionPlugin` or `NpcMovementPlugin` without pulling in `ThreatPlugin`.
- **Alternative Considered**: Monolithic `MonsterAggroComponent`: Rejected because it tightly bakes combat assumptions into general spatial perception.

### 3. Entity-Driven Spawners with Deferred Structural Command Buffers
- **Decision**: Model spawners as regular entities with `SpawnerComponent` placed in the `Octree3D`. Instantiations are pushed to a `SpawnCommandBuffer` and applied at the tick boundary.
- **Rationale**: Spawners benefit from existing spatial indexing, zone division, and persistence. Spawning new entities directly while iterating `EntityStore` during a tick causes an aliasing panic. Deferring insertions via a command buffer is the standard, safe pattern in high-performance ECS architectures.
- **Alternative Considered**: Central node-wide spawner list: Rejected because it does not integrate with spatial queries or entity lifecycle hooks.

### 4. Kinematic Steering with Leash Sphere Evasion
- **Decision**: Implement linear vector steering with arrival tolerance in `NavSteeringSystem`, paired with a `LeashComponent` sphere centered at the NPC's spawn point.
- **Rationale**: Works natively with `KinematicValidator` in `crates/node`, requires zero external mesh dependencies, and enforces classic MMO "leash & reset" mechanics (if mob chases target > `max_leash_radius`, it clears threat, becomes invulnerable, and runs back to spawn).

## Risks / Trade-offs

- **[Risk] Obstacle Pathing**: Without a polygonal NavMesh, NPCs can get stuck on concave terrain obstacles.
  - *Mitigation*: The leash mechanism automatically breaks stuck entities out of chase/patrol after a timeout or distance limit, resetting them to their spawn origin. NavMesh integration can be layered on later as a pluggable path provider.
- **[Risk] Allocation in Threat Tables**: Dynamic combat encounters could allocate memory when inserting new targets into `HashMap<Guid, f32>`.
  - *Mitigation*: Use pre-allocated storage or bounded collections (e.g. up to 16 combatants per mob) to eliminate heap allocations during combat ticks.
