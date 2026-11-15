# Elastic Cluster Completion Proposal

## Why
Issue #240 tracks completing the elastic spatial clustering lifecycle built in #235/#236 (`node-split-migration-and-routing`):
1. **Partition Merging & Rebalancing**: `GlobalOctreeCoordinator` currently triggers octant splits above `split_threshold` / `tick_saturation_ms`, and implements `handle_merge` / `merge_at`, but lacks automatic evaluation and merging of underloaded 8-leaf sibling branches back onto consolidated nodes during heartbeats (and still uses exact float `==` instead of `bounds_approx_eq` in `find_branch_leaves`, `merge_at`, `assign_leaf_node`, and `update_leaf_bounds`).
2. **Graceful Node Drain**: On `SIGTERM` / `SIGINT`, `drain_and_shutdown_node` flushes write-behind database snapshots and calls Director `DeregisterNode`, but does not migrate active movable entities and attached player sessions to surviving peer nodes beforehand, causing unnecessary state reload or temporary session interruption on scale-down.
3. **Seamless Boundary AoI**: `GhostBroadcaster` and `GhostStore` exist on simulation nodes, but periodic boundary ghost replication from `GhostBroadcaster` is not wired into `runner.rs`, `AoiManager` does not include nearby `GhostStore` entities in observer AoI updates across partition seams, and client-side `InterpolationBuffer` / `ClientWorldBridge._pushInterpSnapshot` drops snapshots when a player transitions to a peer node whose authoritative `server_tick` counter is lower than the donor node's `server_tick` (causing entities to freeze or pop/rubber-band).

## Scope
- **Director (`crates/director`)**:
  - Replace exact float `==` checks in `GlobalOctreeNode::{find_branch_leaves, merge_at, assign_leaf_node, update_leaf_bounds}` with `bounds_approx_eq`.
  - Add `GlobalOctreeNode::find_collapsible_branches` and `GlobalOctreeCoordinator::evaluate_and_merge_underloaded` to collapse underloaded 8-leaf split branches (when distinct owner entity count upper bound `<= merge_threshold`, tick duration `< tick_saturation_ms`, and no branch owner is in `split_cooldown`), updating `NodeRegistry` and incrementing `topology_epoch` during `DirectorServiceImpl::heartbeat`.
- **Node Daemon (`crates/node`)**:
  - Add `is_draining: bool` to `TickSimulation`, reject inbound migrations while draining, and add `collect_all_entities_for_drain` + `evacuate_entities_on_drain` on shutdown before flushing `WriteBehindSnapshotter` and shutting down gRPC, deregistering from Director first so peer nodes converge onto reassigned bounds without bouncing entities back.
  - Wire periodic boundary ghost replication using `GhostBroadcaster` + `GrpcClientPool` in `runner.rs`.
  - Extend `AoiManager` with `update_observers_with_ghosts` so observers near partition seams seamlessly receive delta/keyframe state for nearby `GhostStore` entities across node boundaries.
- **Client Runtime (`crates/client-wasm`, `web/`)**:
  - Detect cross-node server tick resets / discontinuities in `InterpolationBuffer::push_snapshot` and `ClientWorldBridge._pushInterpSnapshot` so snapshots arriving after partition handover rebase cleanly instead of being dropped or causing entity pop/rubber-banding.
  - Prevent `hasKeyframe` local-player reconciliation from snapping predicted movement unless error exceeds snap threshold after initial spawn.

## Non-Goals
- Changing genre-specific gameplay rules in `crates/gameplay`.
- Adding persistent external storage for Director's in-memory `GlobalOctreeCoordinator` (tracked separately / covered by node self-healing re-registration).
