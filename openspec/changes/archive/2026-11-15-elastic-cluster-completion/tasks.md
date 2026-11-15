# Tasks for Elastic Cluster Completion

- [x] 1.1 Update `GlobalOctreeNode` bounds matching (`find_branch_leaves`, `merge_at`, `assign_leaf_node`, `update_leaf_bounds`) to use `bounds_approx_eq` and add `find_collapsible_branches` in `crates/director/src/coordinator.rs`.
- [x] 1.2 Add `GlobalOctreeCoordinator::evaluate_and_merge_underloaded` and invoke it in `DirectorServiceImpl::heartbeat` (`crates/director/src/service.rs`) while strictly respecting `instance_registry -> registry -> coordinator` lock order.
- [x] 1.3 Add `is_draining` guard in `TickSimulation` / `NodeServiceImpl` and `collect_all_entities_for_drain` in `crates/node/src/sim.rs`; integrate live peer evacuation + Director deregistration into `run_node_daemon_with_sim` / `drain_and_shutdown_node` in `crates/node/src/runner.rs` & `snapshotter.rs`.
- [x] 1.4 Extend `AoiManager` with `update_observers_with_ghosts` in `crates/node/src/aoi.rs`, wire `self.ghost_store` in `TickSimulation::step`, and wire `GhostBroadcaster` periodic sync task in `crates/node/src/runner.rs`.
- [x] 1.5 Update `InterpolationBuffer::push_snapshot` & `WasmClientWorld` (`crates/client-wasm/src/interp.rs`, `lib.rs`) and `ClientWorldBridge` (`web/src/main.js`) to handle cross-partition tick discontinuities and keyframes cleanly without entity pop or rubber-banding.
- [x] 1.6 Add unit & integration tests across `mmo-director`, `mmo-node`, `mmo-client-wasm`, and `web/` covering partition merge rebalancing, graceful node drain, boundary ghost AoI visibility, and cross-node tick rollover.
- [x] 1.7 Run full quality gates (`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `npm --prefix web test -- --run`, `npm --prefix web run build`, `openspec validate --all`).
