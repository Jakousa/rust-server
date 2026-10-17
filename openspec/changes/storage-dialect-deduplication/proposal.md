# Storage SQLite & PostgreSQL Dialect Deduplication Proposal

## Context & Problem Statement
Across `crates/storage` (Issue #98), over 1,500 lines of business logic, validation, and transaction state machines are duplicated nearly verbatim between SQLite and PostgreSQL backends:

1. **Duplicated Dialect Method Bodies**:
   - `crates/storage/src/escrow_coord.rs`: `create_escrow` vs `create_escrow_postgres`, `release_escrow` vs `release_escrow_postgres`, `refund_escrow` vs `refund_escrow_postgres`, `reap_expired_escrows` vs `reap_expired_escrows_postgres`, `get_escrow` vs `get_escrow_postgres`.
   - `crates/storage/src/marketplace.rs`: `create_listing` vs `create_listing_postgres`, `purchase_listing` vs `purchase_listing_postgres`, `cancel_listing` vs `cancel_listing_postgres`, `get_listing` vs `get_listing_postgres`, `query_listings` vs `query_listings_postgres`, `reap_expired_listings` vs `reap_expired_listings_postgres`.
   - `crates/storage/src/transaction_coord.rs`: `prepare_transfer` vs `prepare_transfer_postgres`, `reserve_slot` vs `reserve_slot_postgres`, `commit_transfer` vs `commit_transfer_postgres`, `abort_transfer` vs `abort_transfer_postgres`, `reap_expired_reservations` vs `reap_expired_reservations_postgres`.
   - `crates/storage/src/repository.rs`: Every method (`create_account`, `create_character`, `get_character`, `update_character_position`, `save_snapshot`, `save_snapshot_batch`, `get_snapshot`, `get_inventory`, `set_inventory_capacity`, `add_item`, `get_item`, `get_character_items`, `remove_item`, `execute_trade` vs `execute_trade_postgres`) manually duplicates `?` vs `$1..$N` queries without using `sql_for_dialect`.

2. **Duplicated Helper Routines**:
   - `current_time_ms()` and `current_time_secs()` are copy-pasted across `escrow_coord.rs`, `marketplace.rs`, `transaction_coord.rs`, and inlined with `.unwrap()` across `repository.rs` and `container_repo.rs`.
   - Container free-slot calculation (`SELECT slot FROM items WHERE container_id = ?` -> `HashSet<u32>` -> `(0..cap).filter(|s| !occupied.contains(s))`) is copy-pasted 12 times across `escrow_coord.rs`, `marketplace.rs`, and `repository.rs`.

3. **Unconditional Compilation of `test_helpers` (`crates/storage/src/lib.rs:14, 34`)**:
   - `pub mod test_helpers;` is compiled and exported unconditionally in production builds.

## Proposed Solution

1. **Shared Time, Slot-Lookup, and Dialect Adapter Helpers (`crates/storage/src/db.rs`)**:
   - Add `pub(crate) fn current_time_ms() -> i64`, `pub(crate) fn current_time_secs() -> i64`, `pub(crate) fn compute_free_slots(occupied: &HashSet<u32>, capacity: u32) -> Vec<u32>`, and `pub(crate) fn find_first_free_slot(occupied: &HashSet<u32>, capacity: u32) -> Option<u32>` in `crates/storage/src/db.rs`.
   - Add `pub fn sql(&self, query: &str) -> String` and `pub fn for_update_clause(&self) -> &'static str` on `Database`.
   - Provide a clean macro `with_dialect_tx!` (or dialect executor helper) in `crates/storage/src/db.rs` so a single transactional business logic body expands for both `DbBackend::Sqlite` (`BEGIN IMMEDIATE` / `COMMIT` / `ROLLBACK`) and `DbBackend::Postgres` (`pool.begin()` / `tx.commit()`), using `sql_for_dialect` and `for_update_clause`.

2. **Deduplicate `escrow_coord.rs`, `marketplace.rs`, `transaction_coord.rs`, and `repository.rs`**:
   - Eliminate all `*_postgres` duplicate methods in `escrow_coord.rs`, `marketplace.rs`, `transaction_coord.rs`, and `repository.rs` (`execute_trade_postgres`), and unify all single-query methods in `repository.rs` using `self.db.sql(...)`.
   - Replace silent `let _ =` error drops in `repository.rs` (`save_snapshot`, `save_snapshot_batch`) with proper `?` error propagation.

3. **Feature-Gate `test_helpers`**:
   - Gate `pub mod test_helpers;` behind `#[cfg(any(test, feature = "test-helpers"))]` in `crates/storage/src/lib.rs` (or move `ItemRecordBuilder` and `ContainerRecordBuilder` to `models.rs` and gate test-only database fixtures behind `#[cfg(any(test, feature = "test-helpers"))]`).

## Scope & Non-Goals
- **In Scope**: `crates/storage/Cargo.toml`, `crates/storage/src/{db.rs, lib.rs, models.rs, test_helpers.rs, escrow_coord.rs, marketplace.rs, transaction_coord.rs, repository.rs, container_repo.rs}`.
- **Non-Goals**: Changing database table schemas or public method signatures of `StorageRepository`, `EscrowCoordinator`, `MarketplaceRepository`, or `TransactionCoordinator`.
