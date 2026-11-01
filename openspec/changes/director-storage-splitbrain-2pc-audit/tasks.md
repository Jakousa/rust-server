# Implementation Tasks: Director & Storage Resilience Hardening

## 1. Director Node Fencing & Split-Brain Prevention
- [x] 1.1 In `crates/director/src/service.rs`, update `DirectorServiceImpl::heartbeat` to return `tonic::Status::failed_precondition` when `reg.record_heartbeat` returns `false` (unacknowledged/evicted node) and increment `mmo_director_node_fenced_total` counter metric.
- [x] 1.2 In `crates/node/src/runner.rs`, introduce an inter-task cancellation/shutdown channel between `heartbeat_handle` and the 20 Hz simulation loop. On `tonic::Code::FailedPrecondition`, trigger node shutdown/fencing immediately and remove the obsolete background re-registration branch.
- [x] 1.3 Update existing tests in `crates/director/src/service.rs` (`test_service_heartbeat`) and add new tests verifying that evicted and unknown nodes receive `failed_precondition`.

## 2. Idempotent 2PC `commit_transfer`
- [x] 2.1 In `crates/storage/src/transaction_coord.rs`, query `container_transactions` by `tx_id` before evaluating `container_reservations`.
- [x] 2.2 Return existing `ContainerTransactionRecord` on `status == "committed"`, and return `StorageError::ReservationExpiredOrAborted` on `status == "aborted"`. If reservation deletion yields `deleted < 2` or `rows.len() != 2`, re-query `container_transactions` to handle concurrent duplicate commit races.
- [x] 2.3 Add unit tests for `commit_transfer` idempotency on sequential retries and concurrent race calls.

## 3. SQLite Foreign Key Constraints Verification
- [x] 3.1 Verify `PRAGMA foreign_keys = ON;` in SQLite connection options in `crates/storage/src/db.rs`.
- [x] 3.2 Add unit test verifying foreign key constraint enforcement in SQLite in `crates/storage/tests` or `crates/storage/src/db.rs`.

## 4. Marketplace Expired Listing Reaper Starvation
- [x] 4.1 In `crates/storage/src/models.rs`, add `ExpiredPendingClaim` variant to `ListingStatus`, update `Display` and `FromStr` implementations, and update enum parsing unit tests.
- [x] 4.2 In `crates/storage/src/marketplace.rs`, add `ORDER BY expires_at_epoch_ms ASC` to the candidate query in `reap_expired_listings`.
- [x] 4.3 If seller container is full or missing (`c_seller.is_none()`), transition listing status to `'ExpiredPendingClaim'` and update `updated_at_epoch_ms` instead of leaving in blocked active state.
- [x] 4.4 In `cancel_listing`, permit cancelling/reclaiming listings in `'Active'` or `'ExpiredPendingClaim'` status.
- [x] 4.5 Update existing test `test_marketplace_reap_expired_listings_seller_inventory_full_safety` to assert status `ExpiredPendingClaim`, and add unit tests verifying reaper starvation elimination and item reclaim after freeing container slots.

## 5. PostgreSQL Deadlock Prevention in 2PC Version Increment
- [x] 5.1 Verify sequential ascending container GUID order `(min, max)` for version increment in `commit_transfer`.
- [x] 5.2 Add concurrent bidirectional transfer test (`A -> B` and `B -> A`) to verify deadlock freedom.

## 6. Workspace Quality Gates & Verification
- [x] 6.1 Run `cargo fmt --all -- --check`.
- [x] 6.2 Run `cargo clippy --workspace --all-targets -- -D warnings`.
- [x] 6.3 Run `cargo test --workspace`.
