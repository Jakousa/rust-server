# Tasks: Dynamic Spatial Partition Rebalancing, Hotspot Mitigation & Zero-Disconnect Batch Handover

## Phase 1: Protocol & Schema Enhancements
- [x] 1.1 Update `crates/schema/proto/cluster.proto` with `tick_duration_ms` and `entity_centroid` in `HeartbeatRequest`.
- [x] 1.2 Add `BatchMigrateEntityRequest`, `BatchMigrateEntityResponse`, `BatchRouteUpdateRequest`, and `BatchRouteUpdateResponse` to `proto/cluster.proto`.
- [x] 1.3 Update `NodeService` and `GatewayService` RPC definitions in `proto/cluster.proto`.
- [x] 1.4 Recompile protobuf schemas and verify `cargo test -p mmo-schema`.

## Phase 2: Cluster Director Saturation & Load Balancing
- [x] 2.1 Update `RegisteredNode` in `crates/director/src/registry.rs` to store `tick_duration_ms` and calculate moving averages.
- [x] 2.2 Enhance `GlobalOctreeCoordinator` in `crates/director/src/coordinator.rs` with `should_split_node(entity_count, tick_duration_ms, last_split)` enforcing 500 entity / 35ms saturation thresholds and a 10s split cooldown.
- [x] 2.3 Implement composite load scoring for worker node selection in `handle_split`.
- [x] 2.4 Add unit tests in `crates/director` validating saturation splitting, split cooldowns, and load-balanced child assignments.

## Phase 3: Continuous Boundary Smoothing Algorithm
- [x] 3.1 Implement centroid calculation with coordinate bounding-box clamping in `crates/node/src/sim.rs`.
- [x] 3.2 Implement centroid-repulsion boundary plane adjustment in `GlobalOctreeCoordinator` with coordinate validation, hysteresis damping (1.0m), and max epoch shift clamping (25.0m).
- [x] 3.3 Add boundary adjustment tests with adjacent partitions having 3:1 entity imbalances and invalid/out-of-bounds centroid rejection.
- [x] 3.4 Expose boundary adjustment metrics in `crates/core/src/telemetry.rs` (`mmo_boundary_adjustments_total`).

## Phase 4: Zero-Disconnect Batch Migration Pipeline
- [x] 4.1 Implement `BatchRouteUpdate` handler in `crates/gateway` using lock-free batch insertion in `DashMap`.
- [x] 4.2 Implement `BatchMigrateEntity` handler in `crates/node/src/network.rs` with atomic all-or-nothing validation, entity insertion, and transaction commit.
- [x] 4.3 Implement `execute_batch_entity_migration` in `crates/node/src/network.rs` following strict point-of-no-return two-phase locking rules:
  - Atomic all-or-nothing lease acquisition for all entities.
  - Write lock release before gRPC transmission.
  - Recipient atomic ingestion validation; donor rollback only on recipient error or timeout.
  - Recipient success marks point of no return: donor write lock despawns entities locally.
  - Gateway batch route update dispatched post-ingestion with async retry.
- [x] 4.4 Write integration tests in `crates/node` demonstrating batch migration of 100+ entities with zero drops and simulated pre-commit failure rollback.

## Phase 5: Cluster Verification & Quality Gates
- [x] 5.1 Run full workspace linter: `cargo clippy --workspace --all-targets -- -D warnings`.
- [x] 5.2 Run full workspace test suite: `cargo test --workspace`.
- [x] 5.3 Run formatting verification: `cargo fmt --all -- --check`.
- [x] 5.4 Commit changes to `feat/dynamic-spatial-partition-rebalancing` and prepare PR.
