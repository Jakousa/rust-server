# Proposal: Node Simulation Runtime Hardening

## Motivation & Problem Statement
A thorough architectural and security audit of the simulation and node runtime crate (`crates/node`) in Issue #168 uncovered critical lifecycle, state consistency, concurrency, and memory management defects:

1. **Gateway Disconnect Cleanup Bypasses State Persistence & Lifecycle Cleanup (`crates/node/src/runner.rs:280-288`)**:
   - `runner.rs` directly despawns disconnected sessions returned by gateway broadcast jobs from `entity_store` and `octree` without calling `sim.disconnect_player_session(...)`.
   - This bypasses snapshot persistence (player state never saved on disconnect), leaks entries in kinematic and session maps, causes zombie rehydration on late client packets due to absent tombstoning, and omits `OnDespawn` gameplay plugin events.

2. **Batch Entity Migration Tombstone Protection & Verification (`crates/node/src/network/migration.rs:430-450`)**:
   - Departed entity GUIDs during batch migration must be strictly tombstoned to prevent split-brain zombie rehydration on late packets from the donor node. While initial tombstone insertion was added, comprehensive regression verification is required.

3. **Multi-Node Boundary AoE Triggers Multiply-Refunded Resources or Premature Refund (`crates/node/src/ability_dispatcher.rs:474-482`)**:
   - In `cast_boundary_sphere_aoe`, if `local_hit_count == 0` and targets span multiple peer partitions, each remote batch task receives an independent refund payload.
   - If multiple remote batches fail, the resource is refunded multiple times (resource duplication exploit).
   - If one succeeds and another fails, the resource is refunded even though targets were struck on the successful partition.

4. **Movement Sequence Monotonicity Rejection on Wraparound (`crates/node/src/sim.rs:1425-1430`, `1608`)**:
   - While RFC 1982 modular comparison was added in `process_client_move_with_seq`, synthetic and deterministic callers using `saturating_add(1)` at line 1608 freeze upon sequence wraparound (`u32::MAX`).
   - Movement sequence wraparound needs consistent wrapping arithmetic (`wrapping_add(1)`) and dedicated boundary unit test coverage.

5. **GhostStore Unbounded Memory Leak & Perpetual Phantom Ghost Accumulation (`crates/node/src/network/ghost.rs:83-93`, `crates/node/src/sim.rs:1020-1065`)**:
   - `GhostStore` currently has no TTL or pruning mechanism.
   - Ghost records replicated from neighboring partitions accumulate perpetually, leading to monotonic heap memory growth and allowing ability targeting against dead or departed entities.

6. **Heavy Contention on Monolithic `RwLock<TickSimulation>` Induced by `sync_ghosts` (`crates/node/src/network/service.rs:205`, `crates/node/src/runner.rs:333`)**:
   - `NodeServiceImpl::sync_ghosts` acquires an exclusive write lock on the monolithic `TickSimulation` container at 20 Hz across all cluster peers, severely starving the main 20 Hz simulation tick loop in `runner.rs`.
   - `GhostStore` must be decoupled into an independently locked concurrent container so incoming ghost synchronizations never block simulation ticks.

## Proposed Changes & User Impact
- Guarantee full snapshot capture, storage persistence, and clean lifecycle hook execution when sessions disconnect via gateway broadcast cleanup.
- Prevent resource multiplication and improper refunds in cross-partition AoE abilities via an atomic multi-node batch completion aggregator.
- Guarantee continuous client movement processing across `u32` sequence wraparounds without lockout.
- Eliminate perpetual ghost accumulation via tick-based TTL pruning in `GhostStore` swept periodically during simulation ticks.
- Decouple `GhostStore` concurrency from `TickSimulation` to eliminate write-lock starvation in the 20 Hz game loop.

## Non-Goals
- Introducing game-specific combat, movement, or boundary logic into the core/node engine substrate.
- Modifying protobuf wire formats or breaking backwards compatibility with existing cluster gRPC clients.
