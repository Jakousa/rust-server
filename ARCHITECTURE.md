# Cluster Architecture & System Design

This document details the architectural invariants, distributed synchronization protocols, and subsystem interactions of the distributed Rust MMO server platform.

---

## 1. High-Level Cluster Topology

The system separates edge connection termination, authoritative spatial simulation, global cluster coordination, and persistent state storage into dedicated tiers:

```mermaid
flowchart TD
    subgraph Clients["Clients"]
        WCLIENT["Web Browser Inspector\n(HTTP/3 WebTransport + WASM)"]
        NCLIENT["Native Game Client\n(QUIC / UDP / TLS 1.3)"]
    end

    subgraph EdgeTier["Edge Ingress Tier"]
        GW["Edge Gateway (mmo-gateway)\n- TLS 1.3 / HTTP/3 Termination\n- Lock-Free Session Router (DashMap)\n- Ephemeral Datagram & Reliable Stream Sinks"]
    end

    subgraph SimTier["Authoritative Simulation Cluster"]
        DIR["Cluster Director (mmo-director)\n- Global Octree Spatial Balancing\n- Node Discovery & Heartbeat Aggregation\n- Automated 3.5s Orphan Partition Recovery"]
        
        N1["Simulation Node 1 (mmo-node)\n- 20 Hz Fixed Tick Loop\n- Kinematic Anti-Cheat & Action Timers\n- 3D Octree AoI Density Throttling"]
        
        N2["Simulation Node 2 (mmo-node)\n- Replicated Boundary Ghost Entities\n- Two-Phase Live Migration Engine\n- Cross-Partition RPC Dispatcher"]
    end

    subgraph StorageTier["Persistence & PubSub Tier"]
        PG[("PostgreSQL / SQLite\n- Asynchronous Write-Behind DB\n- Deadlock-Free ACID Player Trades\n- Schema Migrations")]
        BUS["Social & Event Bus\n- Whispers, Global Chat, Party Sync\n(Memory / Redis)"]
    end

    WCLIENT <-->|WebTransport| GW
    NCLIENT <-->|QUIC UDP| GW
    GW <-->|gRPC / Internal Stream| N1
    GW <-->|gRPC / Internal Stream| N2
    DIR <-->|gRPC Heartbeat / Control| N1
    DIR <-->|gRPC Heartbeat / Control| N2
    N1 <-->|gRPC ClusterNode| N2
    N1 -->|Async Snapshot Batch| PG
    N2 -->|Async Snapshot Batch| PG
    N1 <-->|Pub/Sub| BUS
    N2 <-->|Pub/Sub| BUS
```

---

## 2. Authoritative Simulation Loop (20 Hz)

Each simulation node executes a fixed 50ms tick ($20\text{ Hz}$) driven by a monotonic high-resolution clock:

```mermaid
sequenceDiagram
    autonumber
    participant Sim as Simulation Tick (50ms)
    participant Pipe as Compensation Pipeline
    participant Kin as Kinematics Anti-Cheat
    participant Plugin as Gameplay Plugins & Systems
    participant Octree as 3D Spatial Octree
    participant AoI as AoI Broadcast & Delta Pack
    participant Mig as Two-Phase Migration
    participant DB as Write-Behind Snapshotter

    Sim->>Pipe: Drain Cross-Node Compensation Queue (refund failed RPCs)
    Sim->>Kin: Process Ingested Client Movement Packets
    Note over Kin: Validate sequence, displacement & speed limits<br/>Abort active actions if moved
    Sim->>Plugin: Execute Scheduled Systems (Actions, Attributes, Combat, AI)
    Sim->>Octree: Update Dynamic Entity Bounding Boxes
    Sim->>AoI: Query Visible Entity Sets (density throttled O(N))
    Note over AoI: Encode EntityDeltaBatch with LEB128 varints & fixed-point packing
    Sim->>Mig: Check Partition Boundaries (hysteresis margin = 1.0m)
    Note over Mig: Execute 2-Phase Live Migration for crossing entities
    Sim->>DB: Enqueue Dirty Entity Snapshots (write-behind coalesced)
```

### Tick Overrun Protection
If a tick's execution time exceeds $50\text{ms}$ due to temporary computational bursts, the tick accumulator is strictly clamped to a maximum of $150\text{ms}$ (3 ticks) to eliminate the "spiral of death" CPU starvation cycle.

---

## 3. Two-Phase Live Entity Migration Protocol

When an entity crosses a spatial partition boundary into a neighboring node's domain, authority is transferred without disconnecting the client:

```mermaid
sequenceDiagram
    autonumber
    participant Client as Client Session
    participant GW as Edge Gateway
    participant NodeA as Donor Node (Current Authority)
    participant NodeB as Recipient Node (Target Domain)
    participant DB as Distributed Cache / DB

    Note over NodeA: Entity crosses boundary + hysteresis margin (1.0m)
    NodeA->>NodeA: Phase 1: Acquire Migration Lease (Freeze entity inputs)
    NodeA->>NodeB: gRPC MigrateEntity(Packed Entity & Component Data)
    Note over NodeB: Deserializes entity into ECS arena & Octree
    NodeB-->>NodeA: MigrationAck(Success, Port/Route Info)
    NodeA->>GW: Phase 2: UpdateRoute(SessionID, Target Node Address)
    Note over GW: Atomically updates DashMap routing entry
    GW-->>NodeA: RouteUpdated(Success)
    NodeA->>DB: Synchronize Session Route Cache
    NodeA->>NodeA: Remove Entity from Donor Octree & Drop Lease
    Note over Client: Client continues sending movement packets seamlessly<br/>Gateway immediately forwards to NodeB
```

### Invariants:
1. **Zero Duplication**: Authority belongs exclusively to Node A until Node B acknowledges ingestion and the Gateway updates the routing table.
2. **Input Freeze**: During transit, incoming actions from the client are queued or buffered rather than discarded.
3. **Rollback on Failure**: If Node B fails during Phase 1, Node A aborts the migration, releases the lease, and retains simulation authority.

---

## 4. Network Wire Protocol & Compression

The network protocol operates across three transport mechanisms:
1. **HTTP/3 WebTransport (Browser)**: Multiplexes datagrams (movement, deltas) and streams (reliable actions).
2. **Native QUIC UDP (Client)**: TLS 1.3 encryption with sub-millisecond datagram delivery.
3. **Framed TCP (Fallback)**: 8-byte header (`[0x4D, 0x4D]`, opcode `u16`, length `u32`).

### Bit-Packing & Quantization
To minimize network egress bandwidth:
- **LEB128 Varints**: Entity IDs and field masks encoded with 7-bit variable-length chunks.
- **Fixed-Point Coordinates**: Delta positions quantized to $1/128\text{m}$ resolution ($\approx 7.8\text{mm}$ error).
- **8-Bit Angles**: Yaw ($\approx 1.41^\circ$) and Pitch quantized into single bytes.
- **Baseline Acknowledgement Protocol**: The server tracks per-client baseline ticks. Unchanged fields are suppressed via bitmasks, achieving **88.05% bandwidth reduction**.

---

## 5. Subsystem Crate Architecture

| Crate | Responsibility | Key Abstractions |
| :--- | :--- | :--- |
| **`mmo-core`** | Fundamental primitives & math | `Guid`, `ComponentStore`, `SparseOctree`, `HookDispatcher`, `EventQueue`, `TelemetryServer` |
| **`mmo-schema`** | Wire serialization & RPCs | `ClientPacket`, `BitWriter`, `BitReader`, `EntityDeltaState`, `ClusterNode`, `GatewayService` |
| **`mmo-node`** | Authoritative 20 Hz simulation | `TickSimulation`, `KinematicsValidator`, `AoiManager`, `MigrationCoordinator`, `CrossNodeDispatcher` |
| **`mmo-director`** | Cluster topology & health | `ClusterDirector`, `SpatialRegistry`, `HeartbeatTracker`, `OrphanRecoveryCoordinator` |
| **`mmo-gateway`** | Edge connection ingress | `GatewayServer`, `SessionRouter`, `WebTransportServer`, `SessionSink`, `SessionDropGuard` |
| **`mmo-storage`** | Distributed SQL persistence | `StorageEngine`, `PostgresRepository`, `SqliteRepository`, `TradeCoordinator`, `SocialBus` |
| **`mmo-gameplay`** | Pluggable game rules | `CombatPlugin`, `ThreatPlugin`, `InventoryPlugin`, `TradePlugin`, `AbilityPlugin`, `MonsterAiPlugin` |
| **`mmo-client-wasm`** | Zero-duplication browser WASM | `WasmClientWorld`, `InterpolationBuffer`, `ClientEntityReplica` |
| **`mmo-loadtest`** | Scale & chaos load harness | `BotSwarm`, `BotSession`, `MovementProfile`, `MetricsCollector` |

---

## 6. Storage & Write-Behind Persistence

The storage tier separates high-frequency simulation state updates from relational database persistence:
- **Write-Behind Coalesced Snapshotting**: Dirty entity state changes are coalesced in memory during the tick and flushed asynchronously in batches using parameterized multi-row `INSERT ... ON CONFLICT DO UPDATE` queries.
- **Atomic Two-Player Item Trading**: Player-to-player item exchanges execute under strict GUID-ordered row locking (`SELECT ... FOR UPDATE`) in PostgreSQL, preventing deadlocks and guaranteeing zero item duplication even under network drops.
- **Session Caching**: Dynamic gateway route lookups fall back to a distributed cache layer with automatic TTL-based expiration.
