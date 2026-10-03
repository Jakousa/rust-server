# Tasks: Distributed Transactional Containers & Generic Escrow

## Phase 1: Storage Schema & Dual-Engine Migrations
- [x] 1.1 Create migration `crates/storage/migrations/V3__containers_and_escrow.sql` for PostgreSQL and SQLite.
- [x] 1.2 Add schema tables: `containers`, `container_reservations`, `escrows`, `container_transactions`.
- [x] 1.3 Add `container_id` column to `items` table with backfill for existing character items (`container_id = character_guid`).
- [x] 1.4 Refactor `crates/storage/src/models.rs` (`ItemRecord`) and `crates/storage/src/repository.rs` (`parse_item_record!`, `add_item`, `get_item`) to support optional or populated `container_id`.
- [x] 1.5 Verify migration execution and rollback across both SQLite (`sqlite:`) and PostgreSQL test fixtures in `crates/storage`.

## Phase 2: Domain Models & Core Container Primitives
- [x] 2.1 In `crates/core/src/container.rs`, define `ContainerId`, `ContainerOwner`, `ContainerAccessPolicy`, and `ContainerSlot`.
- [x] 2.2 Export container types from `crates/core/src/lib.rs`.
- [x] 2.3 Implement unit tests in `crates/core/src/container.rs` verifying slot indexing, capacity bounds, and access policy checks.

## Phase 3: Distributed Transaction Coordinator (2PC / Sagas)
- [x] 3.1 In `crates/storage/src/container_repo.rs`, implement `ContainerRepository` with container creation, retrieval, and slot queries.
- [x] 3.2 In `crates/storage/src/transaction_coord.rs`, implement `TransactionCoordinator` with `prepare_transfer`, `commit_transfer`, and `abort_transfer`.
- [x] 3.3 Implement strict numerical GUID locking order on `containers` row locks (`SELECT FOR UPDATE` on PostgreSQL, `BEGIN IMMEDIATE` on SQLite).
- [x] 3.4 Implement atomic check-and-delete on `container_reservations` within `commit_transfer` (`WHERE expires_at_epoch_ms > now`) eliminating race conditions with lease reaper.
- [x] 3.5 Implement background `ReservationReaper` loop aborting expired reservation leases.
- [x] 3.6 Implement unit tests in `crates/storage` verifying reservation lease timeout, double-reservation rejection, and rollback on abort.

## Phase 4: Generic Asynchronous Escrow State Machine
- [x] 4.1 In `crates/storage/src/escrow_coord.rs`, implement `EscrowCoordinator` with `create_escrow`, `release_escrow`, and `refund_escrow`.
- [x] 4.2 Add constant-time secret hash verification for conditional escrow releases to prevent timing attacks.
- [x] 4.3 Implement automatic timeout refund triggering for expired escrows.
- [x] 4.4 Disseminate container and escrow transaction events over `SocialBus`.

## Phase 5: Cluster Protobuf Schema & Cross-Node Ingress
- [x] 5.1 In `crates/schema/proto/cluster.proto`, define `ReserveContainerSlot`, `CommitContainerTransfer`, and `AbortContainerTransfer` RPCs.
- [x] 5.2 Implement gRPC handlers in `crates/node/src/network.rs` enabling cross-node container transfer orchestration.
- [x] 5.3 Integrate non-blocking MPSC channel in simulation tick to dispatch container transfer results back to simulation authority.

## Phase 6: Comprehensive Verification & Proptests
- [x] 6.1 Create concurrent reverse-transfer integration test in `crates/storage/tests/container_concurrency_test.rs` asserting deadlock-free execution under 100+ concurrent opposing transfers.
- [x] 6.2 Create end-to-end multi-node container transfer and escrow integration test verifying cross-partition asset movement and timeout refund.
- [x] 6.3 Run full workspace quality gates:
  - `cargo clippy --workspace --all-targets -- -D warnings` (0 warnings).
  - `cargo test --workspace` (100% green).
  - `cargo fmt --all -- --check`.
