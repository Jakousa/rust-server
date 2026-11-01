# Implementation Tasks: Node Simulation Runtime Hardening

## 1. Disconnect Cleanup & Persistence Lifecycle (`crates/node/src/runner.rs`)
- [x] 1.1 In `crates/node/src/runner.rs`, safely unregister the AoI observer via `sim_guard.unregister_aoi_observer(sid)`, resolve `player_guid` and epoch from `sim_guard.session_epochs`, and invoke `sim_guard.disconnect_player_session(player_guid, epoch)`, falling back to `sim_guard.despawn_entity(player_guid)` if `disconnect_player_session` returns `None`.
- [x] 1.2 Add integration / unit tests verifying that disconnect through broadcast jobs captures entity snapshots, triggers `OnDespawn` events, and tombstones the disconnected entity.

## 2. Batch Migration Tombstone Consistency (`crates/node/src/network/migration.rs`)
- [x] 2.1 Verify `sim_guard.tombstoned_entities.insert(guid)` in `execute_batch_entity_migration_pooled`.
- [x] 2.2 Add unit test `test_batch_migration_tombstones_departed_entities` verifying tombstone insertion post-migration.

## 3. Boundary AoE Multi-Node Atomic Compensation Aggregator (`crates/node/src/ability_dispatcher.rs`)
- [x] 3.1 Implement `BoundaryAoeCompensationAggregator` in `crates/node/src/ability_dispatcher.rs`.
- [x] 3.2 Update `cast_boundary_sphere_aoe` to pre-filter reachable remote peers, enqueue immediate compensation if `local_hit_count == 0` and `total_batches == 0`, and coordinate remote batch completion through the atomic aggregator if `total_batches > 0`.
- [x] 3.3 Add unit tests verifying:
  - If 1 remote batch fails and 1 succeeds, no refund is granted.
  - If all remote batches fail, exactly one refund is granted.
  - If local hit count > 0, no refund is granted.
  - If 0 local hits and 0 reachable remote peers, immediate refund is granted.

## 4. Movement Sequence Wraparound Hardening (`crates/node/src/sim.rs`)
- [x] 4.1 Update `process_client_move` at line 1608 to use `wrapping_add(1)`.
- [x] 4.2 Add unit tests for `process_client_move_with_seq` testing sequence wraparound from `u32::MAX` to `0` and `1`, and verifying out-of-order/stale rejection across the boundary.

## 5. Concurrent GhostStore with Tick-Based TTL Pruning (`crates/node/src/network/ghost.rs`)
- [x] 5.1 Refactor `GhostStore` to be internally synchronized (`Arc<std::sync::RwLock<GhostStoreInner>>`).
- [x] 5.2 Add `local_tick` to `GhostRecord`, `advance_tick` and `prune_stale` to `GhostStore`. Update existing `GhostRecord` constructor call sites.
- [x] 5.3 Wire periodic `self.ghost_store.prune_stale` into `TickSimulation::step`.
- [x] 5.4 Add unit tests for `GhostStore` concurrent operations and TTL pruning.

## 6. Zero-Contention sync_ghosts Implementation (`crates/node/src/network/service.rs`)
- [x] 6.1 Add `ghost_store: GhostStore` to `NodeServiceImpl` initialized during `NodeServiceImpl::new` from `sim`.
- [x] 6.2 Update `NodeServiceImpl::sync_ghosts` to update `self.ghost_store` without acquiring `self.sim.write().await`.
- [x] 6.3 Update test harness `TestNode` in `crates/gateway/tests/common/mod.rs` and `crates/node/tests/common/mod.rs` to share `ghost_store`.

## 7. Quality Gates & Verification
- [x] 7.1 Run `cargo fmt --all -- --check`.
- [x] 7.2 Run `cargo clippy --workspace --all-targets -- -D warnings`.
- [x] 7.3 Run `cargo test --workspace`.
