# Proposal: Track 2 - Dynamic Spatial Partitioning & Live Entity Migration

## Why

In large-scale distributed virtual worlds and MMO clusters, entity density is inherently non-uniform and dynamic:
1. **Dynamic Spatial Hotspots**: Player density naturally concentrates into localized hubs (e.g., capitals, auction houses, raid portals, world boss encounters). Under static spatial partitioning, the simulation node authoritative for that bounding volume faces CPU overload and tick overruns (>50ms), while neighboring peripheral nodes sit idle.
2. **Boundary Snapping Stutter**: Current kinematics logic in `TickSimulation::process_client_move_with_seq` snaps entities back to their previous position if they cross the node's local octree bounding box (`if !self.octree.bounds.contains(new_pos) { return Err(current_pos); }`). Entities are effectively trapped within a single node's initial partition boundaries.
3. **Seamless Client Experience**: Real-time MMO gameplay demands that spatial boundary crossings and node splits/merges occur completely live without disconnecting the client, without interrupting the TCP/QUIC edge session, and without causing rubber-banding or ghost duplication.
4. **Resilient Distributed Consistency**: Distributed handovers between simulation nodes must guarantee **at-most-once / exactly-once** entity authority: if an inter-node network partition or RPC timeout occurs mid-migration, the donor node must safely retain authority and recover without entity loss or duplicate copies.

Track 2 transitions the MMO cluster from static bounding boxes into an elastic, self-balancing distributed simulation mesh capable of autonomous octree splitting, merging, and zero-stutter live entity migration.

---

## What Changes

- **Cluster Schema Protocol Extensions (`crates/schema/proto/cluster.proto`)**:
  - Add `MigrateEntityRequest` and `MigrateEntityResponse` to `NodeService` carrying entity GUID, coordinates, kinematics velocity/yaw/pitch, serialized component registry payloads, and client session ID.
  - Introduce `GatewayService` with `UpdateRoute(RouteUpdateRequest)` allowing simulation nodes and the Director to dynamically update `SessionRouter` tables in Edge Gateways when entity authority transitions.
  - Define `SplitExecutionCommand` and `MergeExecutionCommand` for Director-to-Node topology orchestration.
- **Dynamic Boundary Detection & Hysteresis in Simulation Nodes (`crates/node`)**:
  - Enhance `TickSimulation::process_client_move_with_seq` to detect when a valid movement vector exits the local octree bounds.
  - Implement a **hysteresis buffer band** and **migration cooldown** to prevent rapid oscillation ("ping-pong thrashing") when entities navigate back and forth along spatial boundaries.
  - Instead of rejecting boundary-crossing moves with snap-back, queue an entity migration task.
- **Two-Phase Entity Migration Protocol (`crates/node`)**:
  - **Phase 1 (Lease & State Freeze)**: The donor node marks the entity in a `Migrating` lease state. Incoming client movement inputs for the entity are staged/frozen. The donor node serializes the entity components and kinematics state into a `MigrateEntityRequest` without holding the main simulation lock across asynchronous network I/O.
  - **Phase 2 (Adoption & Clean Despawn)**: The recipient node receives the request, validates target bounds, instantiates the entity in its `EntityStore` and `Octree`, restores its kinematic state and AoI observer config, and responds with success. Upon receiving acknowledgment, the donor node despawns the local entity and releases the lease.
  - **Zero-Duplication Failure Recovery**: If the gRPC call times out or recipient returns an error, the donor node clears the `Migrating` lease, unfreezes the entity, snaps it to the boundary interior, and retains sole simulation authority.
- **Dynamic Gateway Route Invalidation (`crates/gateway`)**:
  - Implement `GatewayServiceServer` on the Edge Gateway exposing the gRPC route update endpoint.
  - Update `SessionRouter` lock-free tables (`guid_to_node`, `session_to_node`) immediately upon receiving `UpdateRoute`, ensuring all subsequent client datagrams are forwarded to the new recipient node without socket reconnects.
- **Dynamic Octree Split & Merge Automation (`crates/director`, `crates/node`)**:
  - Simulation nodes report entity density in periodic heartbeats.
  - When density surpasses `split_threshold`, Director issues `SplitReportRequest` / sub-octant reassignments.
  - Donor node initiates batch migration of all entities residing in reallocated child octants to the assigned peer nodes.
  - When entity counts drop below `merge_threshold`, Director consolidates sibling octants.

---

## Capabilities

### New Capabilities
- `spatial/dynamic-partitioning`: Dynamic octree subdivision, load-balanced sub-octant allocation, and merge consolidation based on real-time node load.
- `migration/two-phase-migration`: Authoritative two-phase entity migration protocol between simulation nodes with freeze leases, component transfer, and zero-duplication failure recovery.
- `routing/dynamic-route-invalidation`: Live edge gateway route invalidation and updating via gRPC without transport session disconnection.

---

## Impact

- `crates/schema`: Update `cluster.proto` with migration messages and `GatewayService`; rebuild generated code.
- `crates/core`: Expose entity state serialization/deserialization helpers for component registries.
- `crates/node`:
  - `sim.rs`: Add boundary migration triggering, hysteresis tracking, and `Migrating` lease management.
  - `network.rs`: Implement `MigrateEntity` handler in `NodeServiceImpl` and outgoing migration client.
- `crates/gateway`:
  - Implement `GatewayService` gRPC service to process live route updates into `SessionRouter`.
- `crates/director`:
  - Wire automated split/merge commands into the node heartbeat loop.
- Tests: Add multi-node migration integration tests, boundary oscillation property tests, and network timeout rollback verification.
