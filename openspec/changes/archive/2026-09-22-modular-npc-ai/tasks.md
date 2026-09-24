## 1. Navigation & Kinematic Steering (`crates/gameplay/src/navigation.rs`)

- [x] 1.1 Implement `NavAgentComponent`, `WaypointComponent`, and `LeashComponent` data models
- [x] 1.2 Implement `NavSteeringSystem` driving kinematic velocity toward target waypoints with arrival tolerance and speed checks
- [x] 1.3 Implement `NpcMovementPlugin` registering components and tick systems, and add unit tests verifying waypoint traversal and leash detection

## 2. Entity-Driven Spawners & Command Buffer (`crates/gameplay/src/spawner.rs`)

- [x] 2.1 Implement `SpawnerComponent`, `SpawnTemplate`, and `SpawnCommandBuffer` for deferred structural mutations
- [x] 2.2 Implement `SpawnerSystem` evaluating cooldowns, active spawn counts, and pushing entity creation commands
- [x] 2.3 Implement `SpawnerPlugin` and add unit tests verifying population limits, respawn timers, and clean structural flushing

## 3. Sensory Perception & Spatial Scanning (`crates/gameplay/src/perception.rs`)

- [x] 3.1 Implement `PerceptionComponent` and `HostileSpottedEvent` data structures
- [x] 3.2 Implement `PerceptionScanSystem` executing periodic `Octree3D` sphere queries to detect nearby hostile targets
- [x] 3.3 Implement `PerceptionPlugin` and add unit tests verifying range detection and event emission

## 4. Threat & Aggro Management (`crates/gameplay/src/threat.rs`)

- [x] 4.1 Implement `ThreatTableComponent` with fixed-capacity entries and target selection thresholds
- [x] 4.2 Implement event listeners for `HostileSpottedEvent`, `DamageEvent`, and reset triggers
- [x] 4.3 Implement `ThreatPlugin` and add unit tests verifying damage threat accumulation, target switching, and combat drop

## 5. Modular Finite State Machine Brain (`crates/gameplay/src/ai_brain.rs`)

- [x] 5.1 Implement `FsmBrainComponent` with states (`Idle`, `Patrol`, `Chase`, `Attack`, `Evade`)
- [x] 5.2 Implement `FsmBrainSystem` driving state transitions based on navigation arrival, threat targets, and attack cooldowns
- [x] 5.3 Implement `MonsterAiPlugin` and add unit tests verifying state transitions from idle to chase, attack, and evade

## 6. End-to-End Simulation Integration & Verification

- [x] 6.1 Register new AI plugins in `crates/gameplay/src/lib.rs` and dynamic component codecs
- [x] 6.2 Create end-to-end simulation test (`crates/node/tests/npc_ai_e2e.rs`) verifying:
  - Spawner instantiates mob
  - Mob patrols waypoints
  - Player approaches, triggers perception and threat, mob enters chase
  - Mob closes distance and attacks player
  - Player flees past leash radius, triggering leash reset and threat wipe
- [x] 6.3 Run full workspace tests `cargo test --workspace` to ensure zero regressions
