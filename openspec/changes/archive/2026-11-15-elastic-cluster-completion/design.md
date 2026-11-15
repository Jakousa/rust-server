# Elastic Cluster Completion Architecture & Design

## 1. Prime Directive & Genre-Agnostic Boundary
All changes touch only spatial infrastructure crates (`mmo-director`, `mmo-node`, `mmo-client-wasm`) and generic presentation sync (`web/src/main.js`), operating strictly on `Guid`, `BoundingBox3D`, 3D vectors, entity counts, and transport/session IDs without genre assumptions.

## 2. Automatic Partition Merging & Rebalancing (`crates/director`)
- **Float Epsilon Safety**: Update `find_branch_leaves`, `merge_at`, `assign_leaf_node`, and `update_leaf_bounds` in `GlobalOctreeNode` to compare `BoundingBox3D` using `bounds_approx_eq`.
- **Collapsible Branch Detection**: Add `GlobalOctreeNode::find_collapsible_branches(&self, out: &mut Vec<(BoundingBox3D, Vec<Option<u32>>)>)`:
  - Matches `GlobalOctreeNode::Branch { bounds, children }` where all 8 children are non-empty `Leaf` variants (`!is_empty_slot(child)`).
- **Load Evaluation & Hysteresis**: Add `GlobalOctreeCoordinator::evaluate_and_merge_underloaded(&mut self, registry: &mut NodeRegistry) -> Vec<(BoundingBox3D, u32)>`:
  - Collect collapsible 8-leaf branches.
  - For each branch, collect unique `Some(owner_id)` owners among its 8 children. Skip if empty, if any active owner is in `last_split_times` within `split_cooldown`, or if any active owner reports `tick_duration_ms >= tick_saturation_ms`.
  - Compute the sum of `entity_count` across the distinct owners from `registry` (an upper bound on entities inside the branch).
  - If `sum <= self.merge_threshold`, choose the `target_node_id`: the active owner with the lowest composite `load_score()` (or if all child owners were somehow inactive, the lowest-load active node in `registry`).
  - Invoke `self.handle_merge(branch_bounds, target_node_id, registry)` (which removes all 8 child bounds from their owners, adds `branch_bounds` to `target_node_id`, collapses the branch to a single `Leaf`, increments `topology_epoch`), record cooldown in `last_split_times` for `target_node_id` to prevent immediate split/merge oscillation, and record metric `mmo_director_partition_merges_total`.
- **Heartbeat Trigger**: In `DirectorServiceImpl::heartbeat`, when `acknowledged && !should_split`, inspect under a read lock (or upgrade to write lock only if merge candidates exist under strict `registry -> coordinator` lock order!) and invoke `evaluate_and_merge_underloaded`. Wait — preserve strict lock order (`instance_registry -> registry -> coordinator`): release `reg` and `coord` read locks or acquire `reg` + `coord` write locks cleanly before calling `evaluate_and_merge_underloaded` so lock acquisition order is never inverted (`registry` always before `coordinator`)!
- **Migration Back to Consolidated Node**: Because `HeartbeatResponse` carries the updated `partitions`, non-target peer nodes receive the consolidated partition on their next heartbeat, update `owned_bounds` / `peer_partitions` in `apply_partitions_and_migrate`, and migrate entities back to `target_node_id` via `collect_entities_for_peer_partitions`.

## 3. Graceful Node Drain (`crates/node`)
- Add `pub is_draining: bool` (default `false`) to `TickSimulation`.
- When `sim.is_draining` is `true`, `NodeServiceImpl::migrate_entity` and `migrate_batch_entities` return `success = false` (`"Recipient node is draining"`) so peer nodes will not bounce entities back into a shutting-down node.
- Add `TickSimulation::collect_all_entities_for_drain(&self) -> HashMap<(u32, String), Vec<Guid>>`:
  - Iterates root movable entities (`parent_guid.is_none()`, `sectors.get_object(guid).is_none()`, not `is_migrating`, not `tombstoned`).
  - Selects destination peer partition from `self.peer_partitions` matching `entity.position` (`bounds.contains`) or falling back to minimum squared distance to `bounds` (`min_distance_sq_to_point`), grouping atomic `collect_migration_group` hierarchies by `(peer_node_id, peer_addr)`.
- **Drain Sequence in `run_node_daemon_with_sim`**:
  - Upon exiting the 20 Hz simulation tick loop (shutdown signal received, NOT on fencing if fenced by Director):
    1. Set `sim.write().await.is_draining = true`, call `telemetry.mark_unready()`, abort `heartbeat_handle`.
    2. Snapshot `batches = sim.read().await.collect_all_entities_for_drain()` and `gateway_addr`.
    3. Send `DeregisterNode` RPC to Director FIRST (bounded 2s timeout) so Director evicts this node, reassigns partitions to surviving nodes, increments `topology_epoch`, and broadcasts `ReassignNodeRoutes` to Gateways.
    4. Execute `dispatch_migration_batches` for `batches` via `GrpcClientPool` to transfer live movable entities, component state, kinematic sequences, and active player sessions (`UpdateBatchRoutes`) to surviving peers cleanly.
    5. Flush `WriteBehindSnapshotter` and send `grpc_shutdown_tx`.

## 4. Seamless Boundary AoI (`crates/node`, `crates/client-wasm`, `web/`)
- **Boundary Ghost Replication (`crates/node/src/runner.rs`)**:
  - Spawn a lightweight async ghost replication task in `run_node_daemon_with_sim` running at 10 Hz (`100ms` interval, cancelled cleanly on shutdown).
  - When `peer_partitions` is non-empty, build a `GhostBroadcaster` with boundary margin matching `AOI_BOUNDARY_MARGIN` (`100.0` world units) and unique peer endpoints from `peer_partitions` (`peer_id != self_node_id`), collect boundary entities (`find_boundary_entities`), and if non-empty, send `GhostSyncRequest` concurrently via `GrpcClientPool` (`send_sync_to_endpoint_pooled`).
- **Ghost-Aware AoI Observer Updates (`crates/node/src/aoi.rs` & `sim.rs`)**:
  - Add `AoiManager::update_observers_with_ghosts(&mut self, entity_store: &EntityStore, octree: &Octree, ghost_store: Option<&GhostStore>, current_tick: u64, timestamp_ms: u64) -> Vec<AoiBroadcastJob>`.
  - Preserve existing `update_observers` delegating to `update_observers_with_ghosts(..., None, ...)`.
  - When `ghost_store` is `Some(gs)` and `!gs.is_empty()`, query `gs.query_radius(observer_pos, observer.effective_radius)` into a scratch map for that observer. Insert ghost `(guid, ghost.position, dist_sq)` for any ghost not already present in local `entity_store` / `self.scratch_query` (filtering `!include_self` and frustum culling if enabled).
  - When building `EntityDeltaState` / `EntityState` for a visible GUID, look up local `entity_store.get_by_guid(guid)` first, falling back to the ghost map `(position, velocity, yaw, pitch, health=100, max_health=100, state_flags=0)` so observers see entities smoothly before they migrate across the partition boundary.
- **Client Cross-Partition Tick Continuity (`crates/client-wasm` & `web/`)**:
  - In `InterpolationBuffer::push_snapshot` (`crates/client-wasm/src/interp.rs`) and `ClientWorldBridge._pushInterpSnapshot` (`web/src/main.js`):
    - Keep out-of-order jitter handling for minor UDP/QUIC reordering (`1 <= back.tick - tick <= 4`).
    - If `back.tick.saturating_sub(tick) > 4` (node handover onto a peer node with a lower `server_tick` counter), clear the entity's stale queue and insert the new snapshot as a clean baseline instead of dropping updates.
  - Also in `WasmClientWorld::handle_parsed_packet` and `ClientWorldBridge`: when `EntityDeltaBatch` arrives with `batch.server_tick + 4 < self.latest_server_tick` (or `batch.server_tick < self.latest_server_tick && batch.baseline_tick == 0`), recognize a server node handover / tick epoch reset, clear stale interpolation queues across entities before pushing the batch deltas, and avoid hard-snapping `localPlayerGuid` in `web/src/main.js` unless `!hasSyncedInitialPosition` or error exceeds snap threshold (`> 10.0m`), eliminating rubber-banding on migration keyframes.
