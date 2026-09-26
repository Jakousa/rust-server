# Distributed Rust MMO Server Platform

A distributed, server-authoritative, spatial simulation engine and modular gameplay platform written in Rust.

---

## Core Philosophy & Design Principles

This project is not merely a single-game server; it is a **genre-agnostic virtual world simulation platform**. It solves the hardest problems in distributed systems—spatial partitioning, authoritative synchronization, zero-duplication entity migration, state persistence, and low-latency edge ingress—while leaving gameplay rules entirely to modular plugins.

Whether the world being simulated is a fantasy tab-target RPG, a space trading simulation, a physics-driven survival sandbox, or a social virtual world, the engine substrate remains identical.

```
┌────────────────────────────────────────────────────────────────────────┐
│                        GAMEPLAY MODULES / PLUGINS                      │
│   e.g. Combat, Threat Tables, Spells, Inventory, Quests, Guilds, AI    │
│   (Implements GameplayPlugin, dynamic components, deferred events)    │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │ registers codecs, hooks, systems
┌───────────────────────────────────▼────────────────────────────────────┐
│                       ENGINE SIMULATION SUBSTRATE                      │
│                                                                        │
│  1. Spatial Substrate (3D Octree) 3. State & Persistence               │
│     • Dynamic domain balancing       • Dynamic ECS ComponentStore      │
│     • Area of Interest (AoI)         • Async Write-Behind DB (PG/SQL)  │
│     • 2-Phase Live Entity Migration  • Atomic Cross-Entity Txns        │
│                                                                        │
│  2. Authoritative Time & Network  4. Extensible Systems Runtime        │
│     • 20 Hz Fixed Tick Simulation    • Type-erased EventQueue          │
│     • Kinematic Anti-Cheat & Jitter  • Lifecycle Hooks (Before/On)     │
│     • Edge Ingress (QUIC/UDP/TCP)    • ModularPacketRouter             │
│     • Lock-Free Route Dispatch       • Interaction & Action Primitives │
└────────────────────────────────────────────────────────────────────────┘
```

### 1. The Genre-Agnostic Substrate
The engine core (`crates/core`, `crates/node`, `crates/director`, `crates/gateway`) has **zero knowledge** of high-fantasy RPG tropes like "mana", "spellbooks", "global cooldowns", or "threat tables". Instead, the engine provides generalized simulation primitives:
- **Spatial Interaction & Targeting**: Cone, sphere, ray, and neighbor queries against the 3D Octree.
- **Cancellable Timed Actions**: Abstract tasks with durations and interruption flags (movement, damage, stun), usable as spell cast bars, lockpicking, weapon reloading, or warp-drive spooling.
- **Dynamic Attribute Sets**: Keyed scalar properties with modifiers, clamps, and regeneration curves.
- **Entity Identity & Components**: 64-bit generational GUIDs with dynamic, type-erased component stores.

### 2. Strict Server Authority & Anti-Cheat
Clients submit movement intentions and interaction requests; the server simulates truth. The 20 Hz fixed-step loop validates kinematic boundaries, sequence continuity, and jitter-tolerant elapsed time ($\Delta t$) to eliminate speed hacking, teleportation, and state desynchronization.

### 3. Elastic Spatial Distribution & Zero-Downtime Migration
Worlds are partitioned across multiple simulation nodes using a dynamic 3D Octree coordinated by the central Director. When an entity crosses partition boundaries, a Two-Phase Migration Protocol transfers authority with zero entity duplication, freezes local state during transit, and dynamically invalidates Edge Gateway routes without disconnecting the client.

### 4. Decoupled Gameplay via Plugins
Gameplay rules are isolated in self-contained plugins implementing [`GameplayPlugin`]. Plugins register dynamic serialization codecs, subscribe to synchronous lifecycle hooks ([`HookDispatcher`]), dispatch domain events through deferred queues ([`EventQueue`]), and attach batch tick systems ([`TickSystem`]) without touching core simulation code.

### 5. Resilient, Production-Ready Cluster
The cluster runs hermetically in Kubernetes (local `k3d` or cloud). The Director autonomously detects node failure within 5 seconds and reassigns orphaned spatial partitions; the Gateway dynamically redirects client traffic; persistence is backed by a production dual-engine PostgreSQL and SQLite storage layer with atomic transaction guarantees.

---

## Workspace Architecture

- **[`crates/core`](crates/core)**: The simulation foundation. Contains 64-bit GUIDs, dynamic ECS [`ComponentStore`](crates/core/src/entity.rs), Sparse 3D Octree spatial indexing, synchronous veto hooks ([`HookDispatcher`](crates/core/src/hook.rs)), open type-erased event queues ([`EventQueue`](crates/core/src/plugin.rs)), dynamic component serialization ([`ComponentCodecRegistry`](crates/core/src/plugin.rs)), and Prometheus telemetry endpoints.
- **[`crates/schema`](crates/schema)**: Protocol definitions. Contains FlatBuffers binary wire protocols for zero-copy client-server framing and gRPC / Tonic service schemas for internal cluster coordination (`ClusterDirector`, `ClusterNode`, `GatewayService`).
- **[`crates/node`](crates/node)**: Authoritative 20 Hz simulation daemon. Executes kinematic movement validation, Area of Interest (AoI) distance queries with $O(N)$ density throttling, live two-phase entity migration, and asynchronous write-behind database snapshotting.
- **[`crates/director`](crates/director)**: Cluster orchestrator. Manages node discovery, spatial boundary coordination, heartbeat health liveness, and automated orphan partition failover recovery.
- **[`crates/gateway`](crates/gateway)**: Edge ingress proxy. Handles dual TCP/QUIC UDP packet termination, session multiplexing, lock-free routing via `DashMap`, non-blocking downstream dispatch, and dynamic route handover on entity migration or node failover.
- **[`crates/storage`](crates/storage)**: Distributed persistence. Dual-backend async SQL engine supporting distributed PostgreSQL (`postgres://`) and hermetic SQLite (`sqlite:`), automated schema migrations, and atomic ACID transaction protocols (e.g. deadlock-free player-to-player item trading).
- **[`crates/gameplay`](crates/gameplay)**: Pluggable gameplay modules implementing the [`GameplayPlugin`](crates/core/src/plugin.rs) trait:
  - `CombatPlugin`: Health pools, armor mitigation, batch regen tick system, damage processing.
  - `ThreatPlugin`: Bounded hate tables, target switching thresholds, damage/healing threat accounting.
  - `InventoryPlugin`: Container bags, equipment constraints, encumbrance veto hooks.
  - `TradePlugin`: Atomic two-player trade transactions with mutual confirmation and distance guards.
  - `LootPlugin` & `QuestPlugin`: Probability drop tables, quest objectives, and reward distribution.
  - `GuildPlugin` & `PartyPlugin`: Multi-member rosters, party sync, and shared transactional vaults.
  - `MonsterAiPlugin`, `PerceptionPlugin`, `SpawnerPlugin`: Autonomous NPC perception scanning, finite state machines, and population-managed respawners.
- **[`crates/loadtest`](crates/loadtest)**: Distributed scale and chaos testing generator. Simulates thousands of concurrent virtual bot connections with configurable trajectory profiles (`RandomWalk`, `HotspotClustering`, `CombatInteraction`) and latency percentile metrics.

---

## Cluster Operations & Quickstart

Cluster lifecycle and testing are abstracted into single-step commands via [`scripts/cluster.sh`](scripts/cluster.sh):

```bash
# 1. Provision local Kubernetes cluster with port forwards
./scripts/cluster.sh up

# 2. Build images and deploy cluster (Director, Gateway, Nodes, Postgres, Grafana)
./scripts/cluster.sh all

# 3. Check cluster status
./scripts/cluster.sh status

# 4. Run bot swarm load test (e.g. 1,000 bots, 10s duration, hotspot behavior)
./scripts/cluster.sh loadtest 1000 10 hotspot

# 5. Run end-to-end chaos failover & recovery test
./scripts/cluster.sh chaos

# 6. Fast iteration: rebuild code, re-import image, and rolling restart
./scripts/cluster.sh reload
```

---

## Completed Architectural Milestones

- [x] **Core Primitives & Simulation Foundation**: 64-bit GUIDs, generational arena entity storage, Sparse 3D Octree with boundary neighbor discovery, and 20 Hz tick simulation.
- [x] **Cluster Infrastructure & Load Testing**: gRPC Cluster Director dynamic boundary balancing, Edge Gateway packet routing and dynamic handoff, headless virtual bot load test harness.
- [x] **Zero-Downtime Persistence**: Write-behind coalescing snapshotter with SQLite/PostgreSQL storage repositories and Redis session caching.
- [x] **Decoupled Gameplay Plugin Architecture**: Standalone plugin lifecycle, open typed event queue with veto hooks, dynamic component serialization codec registry, and modular packet routing.
- [x] **Classic MMO Content Plugins**: Reference implementations for Inventory/Equipment, Combat/Stats/HealthRegen, Atomic Two-Player Trade, Loot Tables & Containers, Quests & Objectives, Guild Rosters & Vaults, and Parties.
- [x] **Cross-Node Gameplay & Social Bus**: Cluster message broker (`SocialBus` with Memory and Redis backends) bridging whispers, global chat channels, party state sync, and guild vault updates across simulation nodes and edge gateways.
- [x] **Autonomous NPC & Monster AI**: Completely decoupled ECS-native systems for kinematic navigation, entity-driven spawners, sensory perception scanning, threat/hate tables, and modular FSM behaviors.
- [x] **Client AoI Packet Broadcasting**: Real-time position and state replication (`EntityState` and `EntityDespawn` packets) pushed from simulation nodes through the Edge Gateway to connected clients within the 3D Octree visibility radius with dynamic density throttling.
- [x] **Native QUIC UDP Transport**: Edge Gateway hybrid UDP transport using `quinn` with TLS 1.3, combining unreliable datagrams (ephemeral movement and AoI entity state sync) with multiplexed reliable streams (transactional interactions and chat).
- [x] **Track 1: Production Observability, Metrics & Health Telemetry**: Unified `TelemetryServer` in `crates/core` with `/metrics`, `/healthz`, and `/ready`, Kubernetes probes, and Grafana dashboards.
- [x] **Track 2: Dynamic Spatial Partitioning & Live Entity Migration**: Two-Phase Entity Migration Protocol with strict state freeze leases, zero-duplication guarantees, live Edge Gateway routing updates, and boundary hysteresis margins.
- [x] **Track 3: Production Distributed Storage (PostgreSQL)**: Multi-backend SQL storage abstraction, automated schema migrations, deadlock-free ACID item trading, and batched persistence.
- [x] **Track 4: Kubernetes Scale & Chaos Testing**: Headless bot swarm generator with hotspot steering, sequence-aware jitter-tolerant kinematics, autonomous 3.5s dead-node orphan recovery, and unified cluster command abstractions.