## Why

The existing prototype was an early sketch using a single gRPC service, raw `Arc<Mutex<Vec<PlayerEntity>>>`, and naive bounding-box spatial checks that cannot scale to MMO player densities. This change re-architects the project into a modular, production-ready distributed MMO server engine inspired by TrinityCore, introducing dynamic hierarchical Octree spatial partitioning, high-frequency tick simulation, an Edge Gateway, and an extensible hook system without game-specific bloat in the core engine.

## What Changes

- **BREAKING**: Transition from a single-crate monolith to a multi-crate Cargo Workspace (`crates/core`, `crates/schema`, `crates/director`, `crates/node`, `crates/gateway`).
- **BREAKING**: Replace 128-bit `uuid::Uuid` with compact **64-bit Structured GUIDs** (Type, Node ID, Sequence ID) for an 8-byte network wire footprint.
- **BREAKING**: Replace static cuboid bounding boxes with a **Hierarchical Sparse Octree** supporting dynamic split/merge and Area of Interest (AoI) throttling under high crowd density.
- **BREAKING**: Replace `Arc<Mutex<Vec<PlayerEntity>>>` with a lockless/inbox-driven **Generational Arena (`slotmap`)** and attached dynamic `TypeMap` for modular components.
- Introduce a **Synchronous Event Hook Pipeline** allowing decoupled modules (Inventory, Trading, Party) to inspect, mutate, or veto/cancel gameplay actions.
- Introduce a **Fixed 20 Hz Simulation Tick Loop** with TrinityCore-style client-simulated movement and server kinematic sanity validation.
- Introduce an **Edge Gateway Proxy** terminating client QUIC streams/FlatBuffers and routing packets seamlessly across shifting slice nodes.
- Introduce a **Director / Orchestrator** control plane tracking node telemetry, cluster health, and the global Octree topology.

## Capabilities

### New Capabilities
- `spatial/octree`: Hierarchical Sparse Octree spatial partitioning, dynamic octant split/merge, coordinate mapping, and neighbor boundary queries.
- `entity/storage`: 64-bit Structured GUID, generational arena (`slotmap`), and dynamic `TypeMap` for modular component attachment.
- `simulation/tick`: Fixed 20 Hz tick loop, TrinityCore-style client movement kinematic sanity validation (speed caps, boundary checks).
- `interaction/hooks`: Synchronous event hook pipeline with `EventContext` (veto, inspect, mutate) for modular extensibility.
- `gateway/routing`: Edge Gateway session termination (QUIC / FlatBuffers) and transparent routing of client streams to authoritative slice nodes.
- `control/director`: Central control plane service for node heartbeats, telemetry aggregation, and dynamic spatial subdivision commands.

### Modified Capabilities
<!-- None: This establishes the initial OpenSpec capabilities for the engine -->

## Impact
- All existing source files in `src/` will be migrated and restructured into the new workspace crates.
- Dependencies in `Cargo.toml` will be updated to include `slotmap`, `quinn` (or QUIC transport), and FlatBuffers schema tools.
- gRPC is repurposed for internal mesh control and Remote Command Forwarding, while client-facing traffic transitions to QUIC + FlatBuffers.
