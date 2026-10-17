# Implementation Tasks: Storage SQLite & PostgreSQL Dialect Deduplication (Issue #98)

- [ ] 1. Shared Time, Free-Slot & Dialect Transaction Helpers (`crates/storage/src/db.rs`)
  - [ ] 1.1 Add `current_time_ms()`, `current_time_secs()`, `compute_free_slots()`, `find_first_free_slot()`, `Database::sql()`, and the `with_dialect_tx!` macro to `crates/storage/src/db.rs`.
  - [ ] 1.2 Add unit tests in `crates/storage/src/db.rs` for `compute_free_slots`, `find_first_free_slot`, and `Database::sql`.

- [ ] 2. Deduplicate `escrow_coord.rs`, `marketplace.rs`, and `transaction_coord.rs`
  - [ ] 2.1 Refactor `crates/storage/src/escrow_coord.rs` to use `with_dialect_tx!`, `compute_free_slots`, and `parse_escrow_record!`, removing `create_escrow_postgres`, `release_escrow_postgres`, `refund_escrow_postgres`, `reap_expired_escrows_postgres`, and `get_escrow_postgres`.
  - [ ] 2.2 Refactor `crates/storage/src/marketplace.rs` to use `with_dialect_tx!` and `find_first_free_slot`, removing `create_listing_postgres`, `purchase_listing_postgres`, `cancel_listing_postgres`, `get_listing_postgres`, `query_listings_postgres`, and `reap_expired_listings_postgres`.
  - [ ] 2.3 Refactor `crates/storage/src/transaction_coord.rs` to use `with_dialect_tx!`, removing `prepare_transfer_postgres`, `reserve_slot_postgres`, `commit_transfer_postgres`, `abort_transfer_postgres`, and `reap_expired_reservations_postgres`.

- [ ] 3. Deduplicate `repository.rs` & Propagate Snapshot Errors
  - [ ] 3.1 Refactor `crates/storage/src/repository.rs` and `container_repo.rs` to use `current_time_secs()`, `self.db.sql(...)`, `compute_free_slots`, and `with_dialect_tx!`, removing `execute_trade_postgres`.
  - [ ] 3.2 Propagate SQL execution and commit errors via `?` in `save_snapshot` and `save_snapshot_batch` instead of discarding with `let _ =`.
  - [ ] 3.3 Gate `test_helpers` behind `#[cfg(any(test, feature = "test-helpers"))]` in `crates/storage/src/lib.rs` (moving `ItemRecordBuilder` and `ContainerRecordBuilder` to `models.rs` and re-exporting them).

- [ ] 4. Workspace Quality Gates
  - [ ] 4.1 Run `cargo fmt --all -- --check`
  - [ ] 4.2 Run `cargo clippy --workspace --all-targets -- -D warnings`
  - [ ] 4.3 Run `cargo test --workspace`
