# Technical Design: Storage SQLite & PostgreSQL Dialect Deduplication

## 1. Shared Dialect & Slot Helpers in `crates/storage/src/db.rs`

Add to `crates/storage/src/db.rs`:
```rust
pub(crate) fn current_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub(crate) fn current_time_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub(crate) fn compute_free_slots(
    occupied: &std::collections::HashSet<u32>,
    capacity: u32,
) -> Vec<u32> {
    (0..capacity).filter(|s| !occupied.contains(s)).collect()
}

pub(crate) fn find_first_free_slot(
    occupied: &std::collections::HashSet<u32>,
    capacity: u32,
) -> Option<u32> {
    (0..capacity).find(|s| !occupied.contains(s))
}
```
And on `impl Database`:
```rust
pub fn sql(&self, query: &str) -> String {
    sql_for_dialect(query, self.is_postgres())
}

pub fn for_update_suffix(&self) -> &'static str {
    if self.is_postgres() {
        " FOR UPDATE"
    } else {
        ""
    }
}
```

### Dialect Transaction Macro (`with_dialect_tx!`)
Because `sqlx::SqliteConnection` and `sqlx::PgConnection` are distinct concrete types, define a macro `with_dialect_tx!` in `crates/storage/src/db.rs` that takes `(&self.db, |conn, is_pg| async move { ... })`:
- For `DbBackend::Sqlite(pool)`: acquires `mut conn = pool.acquire().await?`, executes `sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?`, evaluates the body block with `(&mut *conn, false)`, and on `Ok(val)` executes `COMMIT` and returns `Ok(val)`, or on `Err(e)` executes `let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;` and returns `Err(e)`.
- For `DbBackend::Postgres(pool)`: starts `mut tx = pool.begin().await?`, evaluates the exact same body block with `(&mut *tx, true)`, and on `Ok(val)` executes `tx.commit().await?` and returns `Ok(val)`.
- Inside the body block, a helper macro `q!(sql_str)` expands to `sql_for_dialect(sql_str, is_pg)` and `lock_suffix!()` expands to `if is_pg { " FOR UPDATE" } else { "" }`.
- This eliminates 100% of duplicate `_postgres` methods across `escrow_coord.rs`, `marketplace.rs`, `transaction_coord.rs`, and `repository.rs` while keeping zero dynamic dispatch overhead!

---

## 2. Module-by-Module Deduplication

1. **`crates/storage/src/escrow_coord.rs`**:
   - Remove local `current_time_ms` and `current_time_secs`; import from `crate::db`.
   - Remove `create_escrow_postgres`, `release_escrow_postgres`, `refund_escrow_postgres`, `reap_expired_escrows_postgres`, and `get_escrow_postgres`.
   - Extract `parse_escrow_record!(row)` macro using `status_str.parse::<EscrowStatus>().map_err(StorageError::TradeError)?`.
   - Use `compute_free_slots` for slot capacity validation in `create_escrow`, `release_escrow`, and `refund_escrow`.

2. **`crates/storage/src/marketplace.rs`**:
   - Remove local `current_time_ms` and `current_time_secs`; import from `crate::db`.
   - Remove `create_listing_postgres`, `purchase_listing_postgres`, `cancel_listing_postgres`, `get_listing_postgres`, `query_listings_postgres`, and `reap_expired_listings_postgres`.
   - Use `find_first_free_slot` in `create_listing`, `purchase_listing`, `cancel_listing`, and `reap_expired_listings`.

3. **`crates/storage/src/transaction_coord.rs`**:
   - Remove local `current_time_ms` and `current_time_secs`; import from `crate::db`.
   - Remove `prepare_transfer_postgres`, `reserve_slot_postgres`, `commit_transfer_postgres`, `abort_transfer_postgres`, and `reap_expired_reservations_postgres`.

4. **`crates/storage/src/repository.rs` & `container_repo.rs`**:
   - Replace inline `SystemTime::now()...unwrap()` with `crate::db::current_time_secs()`.
   - Unify all SQLite/Postgres queries in `repository.rs` using `self.db.sql(...)` and `with_dialect_tx!`, removing `execute_trade_postgres`.
   - In `save_snapshot` (`lines 334-398`) and `save_snapshot_batch` (`lines 401-517`), share a single helper for snapshot UPSERT + character position update, and replace `let _ =` with `?` so database errors are properly propagated.

5. **`crates/storage/src/test_helpers.rs` & `models.rs`**:
   - Move `ItemRecordBuilder` and `ContainerRecordBuilder` definitions into `crates/storage/src/models.rs` (since `ItemRecord::builder` and `ContainerRecord::builder` are part of the model API), and gate `pub mod test_helpers;` in `crates/storage/src/lib.rs` behind `#[cfg(any(test, feature = "test-helpers"))]`.
