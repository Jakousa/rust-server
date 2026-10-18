# Node, Gateway & Core Modularity, Performance, and Loadtest Specification

## Purpose
Eliminates per-tick heap allocations and ephemeral gRPC connection overhead in the 20Hz simulation loop, decomposes monolithic network/gateway files into focused submodules with zero duplicated handshake/migration/raycast logic, and hardens wire decoding and loadtest bot sessions.

## Requirements

### Requirement: Pooled Inter-Node gRPC Channels and Zero-Allocation Tick Hot Paths
`crates/node` SHALL reuse pooled `tonic::transport::Channel` connections via `GrpcClientPool` across ghost synchronization, cross-node dispatch, and entity migration. `TickSimulation::step` (`SpatialHistoryBuffer::push_frame`), `AttributeSet::tick`, and `EventQueue::drain` SHALL execute without per-tick key/frame vector reallocations.

#### Scenario: Pooled gRPC channel reuse and O(1) TypeId event draining
- **GIVEN** multiple event types pushed into `EventQueue` and repeated `AttributeSet::tick` / `SpatialHistoryBuffer::push_frame` invocations
- **WHEN** `EventQueue::drain::<E>()` is called for a specific `DomainEvent` type `E`
- **THEN** only the `TypeId::of::<E>()` bucket SHALL be removed and downcast in $O(K)$ time without scanning or reallocating events of other types

### Requirement: Modular Subsystems and Unified Handshake, Migration, and Ray-AABB Primitives
`crates/node/src/network` and `crates/gateway/src/server` SHALL be organized into focused submodules. Session authentication across TCP, QUIC, and WebTransport SHALL use a single shared handshake routine; single and batch entity migration SHALL share ingestion and 2PC state transitions; and `BoundingBox3D`, `AabbCollisionWorld`, and `spatial::ray_intersects_aabb` SHALL share one canonical slab-intersection implementation while enforcing `SpatialQueryFilter::collision_layers`.

#### Scenario: Collision layer filtering in AabbCollisionWorld
- **GIVEN** an obstacle registered in `AabbCollisionWorld` with `layer_mask = 0b0010`
- **WHEN** `raycast`, `sweep_test`, or `is_point_traversable` is queried with `SpatialQueryFilter { collision_layers: 0b0001, .. }`
- **THEN** the obstacle SHALL be ignored by the query

### Requirement: Monotonic Loadtest Bot Sequencing, Baseline Acks, and Safe Wire Bounds
`BotSession` SHALL increment movement sequence numbers monotonically (`1, 2, 3, ...`), `SwarmManager::run` SHALL respond to `EntityDeltaBatch` with `ClientAckPacket`, `TrajectoryController::step` SHALL NOT recurse when `dist < 2.0`, and `crates/schema/src/client.rs` SHALL guard `u16` string lengths and varint loop counts against buffer underruns/truncation.

#### Scenario: TrajectoryController step in sub-2.0m bounds
- **GIVEN** a `TrajectoryController` configured with `hotspot_radius = 0.5` or `WorldBounds` of width `1.0`
- **WHEN** `step(0.05)` is invoked repeatedly
- **THEN** `step` SHALL return a valid `(position, velocity)` tuple in $O(1)$ stack frames without recursion
