# Proposal: Node Split Response Execution, Live Entity Migration, Split-Loop Prevention, and Topology-Aware Gateway Routing

## Why
Issue #235 identified that when a simulation node exceeds the octree split threshold (50 entities) and invokes `ReportSplit` on `mmo-director`:
1. The simulation node (`crates/node/src/runner.rs`) discards `SplitReportResponse` and only mutates a local variable `current_bounds`, never updating `TickSimulation` owned partitions or migrating entities to peer nodes (`mmo-node-1`).
2. Because entities remain on the reporting node, subsequent heartbeats continue reporting `>= 50` entities, causing an infinite 10-second split loop and monotonically increasing `topology_epoch` while peer nodes remain empty.
3. Newly registering standby nodes (`node_id > 1`) initialized with default root bounds `(-100..100)^3` could overwrite active root leaf assignments in `GlobalOctreeNode::map_bounds`, and `0.0.0.0:<port>` listen addresses were not normalized to routable service DNS names (`mmo-node-{id-1}.mmo-node:<port>`).
4. `mmo-gateway` only routed new sessions to a static `default_node_addr` without consuming cluster partition topology from Director heartbeats.
5. In multi-node Kubernetes deployments (`k8s/base/node-statefulset.yaml`), `NODE_ID > 1` ran the bare `mmo-node` binary without gameplay plugins registered, preventing migrated entities with plugin components from deserializing cleanly.

## What Changes
- **Genre-agnostic cluster protocol & Director (`crates/schema`, `crates/director`)**:
  - Add `PartitionAssignment` protobuf message and return partition topology + peer addresses in `SplitReportResponse`, `HeartbeatResponse`, and `GatewayHeartbeatResponse`.
  - Normalize unspecified listen addresses (`0.0.0.0` / `::`) in `fallback_node_addr`, prevent standby registrations from stealing active root leaves or clobbering split branches, balance split octants across active nodes, and halt subdivision below `MIN_SPLIT_EXTENT`.
- **Node partition ownership & batch live migration (`crates/node`)**:
  - Track `owned_bounds` and `peer_partitions` on `TickSimulation`, distinguish owned partition bounds from world simulation octree bounds, migrate entities residing in peer-assigned octants immediately after split via `execute_batch_entity_migration_pooled`, and migrate boundary-crossing entities asynchronously after simulation ticks.
- **Topology-aware Gateway session routing (`crates/gateway`)**:
  - Propagate `PartitionAssignment` topology from Director heartbeats to `SessionRouter`, routing new sessions and spatial positions according to active cluster partitions.
- **Decoupled gameplay plugin bootstrap (`crates/gameplay`, `k8s/base/node-statefulset.yaml`)**:
  - Ensure all node replicas initialize gameplay plugins and static structures while restricting initial NPC spawning to `NODE_ID == 1`.

## Impact
- Restores horizontal spatial load distribution across multi-node clusters.
- Eliminates infinite Director split loops and empty peer node states.
- Keeps all core engine crates (`mmo-schema`, `mmo-director`, `mmo-node`, `mmo-gateway`) strictly genre-agnostic.
