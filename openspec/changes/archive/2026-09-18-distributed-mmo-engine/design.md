## Context

See `proposal.md` for motivation. The legacy codebase consisted of a single gRPC service on `[::1]:50051` with coarse `Arc<Mutex<Vec<PlayerEntity>>>` locking and manual 3D bounding box slices. This technical design outlines the multi-crate architecture, data structures, and concurrency model for the distributed engine reboot.

## Goals / Non-Goals

**Goals:**
- Establish a decoupled Cargo Workspace (`core`, `schema`, `director`, `node`, `gateway`).
- Implement 64-bit compact GUIDs and `SlotMap`-based entity arena with dynamic `TypeMap` components.
- Implement a hierarchical Sparse Octree with dynamic subdivision and O(log N) boundary neighbor lookups.
- Implement a fixed 20 Hz simulation loop with kinematic speed sanity validation.
- Implement an Edge Gateway terminating QUIC/FlatBuffers with dynamic routing tables.
- Implement a Director control plane monitoring node telemetry and managing octree allocations.
- Implement a synchronous hook pipeline with `EventContext` (veto, inspect, mutate).

**Non-Goals:**
- Game-specific combat mechanics, character classes, talent trees, or complex AI behaviors (reserved for game-specific modules).
- Full client-side game engine implementation (engine provides schemas and test client mocks).
- Full cross-data-center multi-region replication.

## Decisions

### 1. Multi-Crate Workspace Layout
- **Decision**: Divide the repository into distinct crates under `crates/`:
  - `crates/core`: Octree, SlotMap, GUID, Hooks, Math, Entity storage.
  - `crates/schema`: FlatBuffers schema definitions and compiled builders.
  - `crates/director`: Control plane service for cluster management.
  - `crates/node`: Authoritative simulation node with 20 Hz tick loop and boundary ghost manager.
  - `crates/gateway`: Edge proxy terminating QUIC client connections.
- **Alternatives Considered**: Monolithic single-crate with modules (rejected due to compilation coupling and inability to deploy gateway or director as isolated binaries).

### 2. Generational Arena (`slotmap`) for Entity Storage
- **Decision**: Use `slotmap::SlotMap<DefaultKey, Entity>` inside each simulation node.
- **Rationale**: Provides O(1) retrieval, memory locality, and generation counters to prevent ABA bugs when players disconnect or migrate.
- **Alternatives Considered**: Heavyweight ECS like Bevy or Hecs (rejected due to archetype reshuffling costs during high-frequency inter-slice migrations).

### 3. Dynamic TypeMap for Modular Components
- **Decision**: Embed an `AnyMap` / `TypeMap` in each `Entity`.
- **Rationale**: Allows independent modules (Inventory, Equipment) to register custom state structs without altering core engine definitions.
- **Alternatives Considered**: Module-owned external hash maps (rejected because serializing an entity for slice migration would require querying multiple external stores).

### 4. Movement Sanity Validation at 20 Hz
- **Decision**: Client computes local movement; server validates speed, boundary constraints, and timestamps at 20 Hz.
- **Rationale**: Standard TrinityCore / WoW architecture. Ensures responsive client controls while keeping server CPU low enough to host hundreds of entities per slice.
- **Alternatives Considered**: Full server-side physics simulation with rollback (rejected due to high CPU cost and player concurrency limits).

### 5. Transport Strategy: QUIC + FlatBuffers (External) vs gRPC/Tonic (Internal Mesh)
- **Decision**: 
  - Clients communicate with Edge Gateway via QUIC datagrams/streams and FlatBuffers (zero-copy deserialization).
  - Internal cluster communication (Gateway $\leftrightarrow$ Node, Node $\leftrightarrow$ Director, and inter-node Remote Command Forwarding) uses Tonic gRPC for type-safe RPCs and internal telemetry.
- **Alternatives Considered**: gRPC directly to client (rejected due to HTTP/2 overhead on tick streams) or raw TCP everywhere (rejected due to lack of RPC ergonomics internally).

## Risks / Trade-offs

- **[Risk] High boundary density saturating mesh bandwidth** → *Mitigation:* Area of Interest (AoI) throttling dynamically contracts the broadcast radius when crowded, keeping outbound packet rates capped.
- **[Risk] Packet loss or reordering during node handovers** → *Mitigation:* Edge Gateway buffers in-flight movement packets during the handoff ACK handshake before directing traffic to the target node.
- **[Risk] Desynchronization between Director octree and Node state** → *Mitigation:* Director state is authoritative; nodes acknowledge all split/merge commands via two-way handshake.

## Migration Plan

1. Scaffold Cargo workspace and migrate existing code into `crates/core`.
2. Implement and unit test the Octree, 64-bit GUID, and SlotMap entity storage.
3. Build the 20 Hz simulation node tick loop in `crates/node`.
4. Build the Director control plane in `crates/director`.
5. Implement the FlatBuffers schema and Edge Gateway in `crates/schema` and `crates/gateway`.
6. Deprecate legacy monolithic files in `src/`.
