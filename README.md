# Distributed Rust MMO Server Engine

A high-performance, distributed, server-authoritative MMO game engine written in Rust.

## Architecture

- **`crates/core`**: 64-bit structured GUIDs, generational arena entity storage (`SlotMap` + `TypeMap`), Sparse 3D Octree with boundary neighbor discovery, synchronous veto hooks (`HookDispatcher`), open typed event queue (`EventQueue`), dynamic component serialization (`ComponentCodecRegistry`), and modular packet dispatch.
- **`crates/schema`**: FlatBuffers wire protocol framing and gRPC/Tonic definitions for internal cluster coordination (`ClusterDirector`, `ClusterNode`).
- **`crates/node`**: 20 Hz fixed-step authoritative simulation loop, kinematic movement anti-cheat validation, ghost boundary replication, and non-blocking write-behind snapshotting.
- **`crates/director`**: Central coordinator managing node registration, heartbeat liveness, and dynamic Sparse Octree slice balancing.
- **`crates/gateway`**: Edge connection termination, session routing, dynamic route handover across octant boundaries, and session cache lookups.
- **`crates/storage`**: Relational PostgreSQL/SQLite migrations (`accounts`, `characters`, `entity_snapshots`), distributed session caching (in-memory & Redis), and cluster-wide `SocialBus` (in-memory & Redis Pub/Sub) for whispers, chat channels, party and guild event sync.
- **`crates/gameplay`**: Standalone, decoupled gameplay plugins:
  - `InventoryPlugin`: Containers, slot constraints, weight limits, equipment slots, encumbrance veto.
  - `CombatPlugin`: Health pools, armor mitigation, batch health regen tick system, damage processing.
  - `TradePlugin`: Two-player trade sessions, distance-based abortion, atomic disjoint inventory swap.
  - `LootPlugin`: Probability-weighted drop tables, loot containers, corpse looting.
  - `QuestPlugin`: Quest definitions, objective tracking (kills, items), reward distribution.
  - `GuildPlugin`: Roster hierarchy (Member, Officer, Leader), shared transactional guild vault bank.
  - `PartyPlugin`: Multi-member parties, cross-node invite/accept lifecycle, leader controls.
  - `NpcMovementPlugin`: Kinematic steering, waypoints, arrival tolerance, leashing boundary checks.
  - `SpawnerPlugin`: Static spawner entities, respawn timers, population limits, deferred structural creation buffer.
  - `PerceptionPlugin`: 3D Octree sensory scanning, faction hostility filtering, `HostileSpottedEvent`.
  - `ThreatPlugin`: Bounded threat tables, 110% melee target switching, damage/healing threat accounting, leash wipe.
  - `MonsterAiPlugin`: Concrete ECS finite state machine (`Idle`, `Patrol`, `Chase`, `Attack`, `Evade`).
- **`crates/loadtest`**: Headless virtual bot swarm with 3D kinematic trajectory steering and HDR histogram latency percentiles.

---

## Roadmap

### Completed Milestones
- [x] **Core Primitives & Simulation Foundation**: 64-bit GUIDs, generational arena entity storage, Sparse 3D Octree with boundary neighbor discovery, and 20 Hz tick simulation.
- [x] **Cluster Infrastructure & Load Testing**: gRPC Cluster Director dynamic boundary balancing, Edge Gateway packet routing and dynamic handoff, headless virtual bot load test harness.
- [x] **Zero-Downtime Persistence**: Write-behind coalescing snapshotter with SQLite/PostgreSQL storage repositories and Redis session caching.
- [x] **Decoupled Gameplay Plugin Architecture**: Standalone plugin lifecycle, open typed event queue with veto hooks, dynamic component serialization codec registry, and modular packet routing.
- [x] **Classic MMO Content Plugins**: Reference implementations for Inventory/Equipment, Combat/Stats/HealthRegen, Atomic Two-Player Trade, Loot Tables & Containers, Quests & Objectives, Guild Rosters & Vaults, and Parties.
- [x] **Cross-Node Gameplay & Social Bus**: Cluster message broker (`SocialBus` with Memory and Redis backends) bridging whispers, global chat channels, party state sync, and guild vault updates across simulation nodes and edge gateways.
- [x] **Autonomous NPC & Monster AI**: Completely decoupled ECS-native systems for kinematic navigation, entity-driven spawners, sensory perception scanning, threat/hate tables, and modular FSM behaviors.
- [x] **Client AoI Packet Broadcasting**: Real-time position and state replication (`EntityState` and `EntityDespawn` packets) pushed from simulation nodes through the Edge Gateway to connected clients within the 3D Octree visibility radius with dynamic density throttling.

### Current Focus & Upcoming Milestones
- [ ] **Native QUIC UDP Transport**: Upgrade Edge Gateway from framed TCP to native multiplexed QUIC (`quinn`) streams with 0-RTT reconnection and packet loss resilience.
- [ ] **Playable Visual Client Frontend**: Interactive client built in Bevy or WebAssembly/WebGL to explore the world, interact with entities, equip gear, and fight monsters.