# Design: Node Split Execution, Live Entity Migration & Topology-Aware Routing

## 1. Protocol & Director Topology Coordination
- `cluster.proto` defines `PartitionAssignment { BoundingBox bounds = 1; uint32 node_id = 2; string node_address = 3; }`.
- `SplitReportResponse` adds `repeated string assigned_node_addresses = 4` and `repeated PartitionAssignment partitions = 5`.
- `HeartbeatResponse` adds `repeated PartitionAssignment partitions = 4` so nodes synchronize topology on every heartbeat.
- `GatewayHeartbeatResponse` adds `uint64 topology_epoch = 3` and `repeated PartitionAssignment partitions = 4` so gateways mirror authoritative partition ownership.
- `GlobalOctreeCoordinator` enforces:
  - `fallback_node_addr`: unspecified IPs (`0.0.0.0`, `::`) normalize to `mmo-node-{id-1}.mmo-node:{port}`.
  - `map_bounds`: a newly registering standby node with default world bounds never steals an active owner's root leaf and never flattens an already-split `Branch` into a `Leaf`.
  - `should_split_node`: refuses splits when no owned bound has extent `>= MIN_SPLIT_EXTENT` (`25.0`) or when the node owns no bounds.
  - `handle_split`: if `original_bounds` is already split (`Branch`), returns existing child assignments without incrementing `topology_epoch`; otherwise distributes the 8 octants balanced across active nodes.

## 2. Node Owned Partitions vs. World Octree Bounds
- `TickSimulation` retains `octree.bounds` for spatial collision/movement containment over the world while introducing `owned_bounds: Vec<BoundingBox3D>` and `peer_partitions: Vec<(BoundingBox3D, u32, String)>`.
- `distance_to_owned_bounds` evaluates boundary hysteresis margins against `owned_bounds` (reducing identically to `octree.bounds.distance_to_point` when single-node root bounds apply).
- On `SplitReportResponse` (and topology updates in `HeartbeatResponse`), `runner.rs` applies `apply_topology_partitions`, collects movable non-`WorldObject` entities located in peer-owned octants, and invokes `execute_batch_entity_migration_pooled` to transfer entities in batches and update Gateway routes via `UpdateBatchRoutes`.
- After simulation ticks, any `pending_migrations` whose entity positions lie outside `owned_bounds` and inside a peer partition are asynchronously migrated via a dedicated migration worker task.

## 3. Gateway Topology-Aware Routing
- `SessionRouter` stores `partitions: RwLock<Vec<PartitionRoute>>` and `topology_epoch: AtomicU64`, updated on every `GatewayHeartbeat` response in `GatewayServer::spawn_heartbeat_loop_with_router`.
- `select_node_for_new_session` resolves an initial simulation node using spawn coordinates if known, round-robins across active partition nodes otherwise, and falls back to `default_node_addr` when no partition table has been received yet.

## 4. Plugin Registration Symmetry Across Replicas
- `bootstrap_village_plugins_and_structures` separates plugin/codec/static-obstacle setup from initial villager NPC spawning (`NODE_ID == 1`), allowing `k8s/base/node-statefulset.yaml` to run `mmo-village-node` across all replicas without duplicating starter NPCs.
