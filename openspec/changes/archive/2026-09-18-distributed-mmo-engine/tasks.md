## 1. Workspace Scaffolding

- [x] 1.1 Convert repository root `Cargo.toml` into a Cargo Workspace declaring `crates/core`, `crates/schema`, `crates/director`, `crates/node`, and `crates/gateway`, and verify with `cargo check --workspace`
- [x] 1.2 Initialize crate skeletons for `core`, `schema`, `director`, `node`, and `gateway` with basic dependencies and verify each compiles


## 2. Core Engine Implementation (`crates/core`)

- [x] 2.1 Implement `Guid` (64-bit structured entity identifier) with encoding/decoding for type, node ID, and sequence counter, and verify with unit tests
- [x] 2.2 Implement `Entity` and `EntityStore` using `slotmap::SlotMap` and a dynamic `TypeMap` for attached modular components, and verify insertion, lookup, and mutation via unit tests
- [x] 2.3 Implement the `Octree` spatial structure with recursive subdivision, point/bounding volume queries, and neighbor discovery, and verify with spatial boundary unit tests
- [x] 2.4 Implement `HookDispatcher` and `EventContext` for synchronous event dispatch (e.g. `OnBeforeMove`, `OnInteract`), and verify veto/cancel semantics via unit tests


## 3. Schemas & Protocols (`crates/schema`)

- [x] 3.1 Define FlatBuffers schema files (`.fbs`) for client movement, entity state, and interaction events, and configure `build.rs` to generate Rust bindings
- [x] 3.2 Define internal gRPC `.proto` services for Remote Command Forwarding and Director telemetry, and verify compilation via `tonic-build`

## 4. Simulation Node (`crates/node`)

- [x] 4.1 Implement the fixed 20 Hz (50ms) simulation tick loop driving `EntityStore` and hook dispatches, and verify tick timing accuracy via integration tests
- [x] 4.2 Implement kinematic movement validation (speed caps, teleport detection, snap-back generation), and verify with movement test scenarios
- [x] 4.3 Implement the boundary ghost replica broadcaster and Remote Command Forwarding gRPC service, and verify inter-node command execution via mock tests

## 5. Control Plane (`crates/director`)

- [x] 5.1 Implement node registry, heartbeat tracking, and telemetry aggregation service, and verify node timeout/deregistration via unit tests
- [x] 5.2 Implement the global Octree coordinator that issues split and merge commands when node density thresholds are crossed, and verify split command triggers

## 6. Edge Gateway (`crates/gateway`)

- [x] 6.1 Implement the QUIC connection listener and FlatBuffers packet framer, and verify mock client connection handshakes
- [x] 6.2 Implement the session routing table (`EntityId -> NodeAddress`) that dynamically forwards packets to authoritative nodes, and verify route switching during mock node handovers

## 7. Verification & Benchmarking

- [x] 7.1 Migrate or rewrite `benches/` to benchmark `crates/core` Octree lookups, SlotMap iterations, and 20 Hz tick execution under 1,000+ entities, and verify benchmark completes with Criterion
- [x] 7.2 Run end-to-end integration test with Gateway, Director, and 2 Simulation Nodes executing a player movement and boundary transition scenario
