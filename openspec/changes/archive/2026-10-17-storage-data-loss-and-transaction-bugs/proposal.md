# Fix PostgreSQL Parameter Binding, Non-Atomic Redis Handover, and Escrow/Marketplace Item Loss Proposal

## Context & Problem Statement
Code review of persistence, caching, 2PC transfer coordination, and marketplace subsystems in `crates/storage` (Issue #97) identified several critical correctness, concurrency, and data-loss vulnerabilities across SQLite, PostgreSQL, and Redis:

1. **Broken SQL Parameter Placeholder `$2` in `prepare_transfer_postgres` (`crates/storage/src/transaction_coord.rs:571-576`)**:
   ```rust
   let c2 = sqlx::query(
       "SELECT container_id, capacity FROM containers WHERE container_id = $2 FOR UPDATE",
   )
   .bind(second_id as i64)
   .fetch_optional(&mut *tx)
   .await?;
   ```
   Only 1 parameter is bound (`second_id`), but the query references positional parameter `$2`. In PostgreSQL, prepared statement execution fails unconditionally at runtime with parameter index mismatch error, completely breaking all PostgreSQL-backed 2PC container transfers.

2. **Cross-Product Reservation Deletion in `prepare_transfer` (`crates/storage/src/transaction_coord.rs:135-142, 652-659`)**:
   ```sql
   DELETE FROM container_reservations
   WHERE container_id IN (?, ?) AND slot IN (?, ?) AND expires_at_epoch_ms <= ?
   ```
   Using `container_id IN (src, dst) AND slot IN (src_slot, dst_slot)` forms a Cartesian product. It unintentionally deletes expired reservations on `(src_container_id, dst_slot)` and `(dst_container_id, src_slot)`, interfering with unrelated concurrent transactions without recording proper audit logging.

3. **Non-Atomic `GET` + `DEL` in `RedisCache::claim_handover` (`crates/storage/src/cache.rs:155-165`)**:
   `claim_handover` executes `conn.get(&key)` followed by `conn.del(&key)` as separate commands. Concurrent handover claims (e.g., during boundary oscillation, split-second retries, or failover) can both read `Some(s)` before either deletes the key, causing duplicate entity handovers and split-brain state duplication across two simulation nodes.

4. **Silent Item Loss on Full Inventory in `EscrowCoordinator::refund_escrow` (`crates/storage/src/escrow_coord.rs:365-374, 776-787`)**:
   Unlike `create_escrow` and `release_escrow`, `refund_escrow` zips `escrow_items.iter().zip(free_slots.iter())` without verifying `if free_slots.len() < escrow_items.len()`. If the depositor's container lacks capacity for all refunded items, `.zip()` silently truncates the loop and marks the escrow `'refunded'`, permanently stranding the remaining unrefunded items in the escrow container forever.

5. **Stranded Items & Missing Payment Settlement in `MarketplaceRepository` (`crates/storage/src/marketplace.rs`)**:
   - In `reap_expired_listings` (`lines 736-760, 1300-1331`), if the seller's container does not exist or has no free slots (`free_slot` is `None`), the item is not returned to the seller, yet `market_listings.status` is updated to `'Expired'` and committed, permanently stranding the item in the marketplace escrow.
   - In `purchase_listing` (`lines 244-430, 911-1076`), `payment_container_id` ownership and `payment_price >= price` are validated, but payment transaction settlement is never recorded into `container_transactions`.

6. **Broken Cross-Node Subscriptions in `RedisSocialBus::subscribe` (`crates/storage/src/social.rs:186-207`)**:
   `RedisSocialBus::subscribe` only calls `self.memory_fallback.subscribe(channel).await` and never connects or registers a subscription with Redis PubSub. Messages published on Node A are never delivered to subscribers on Node B.

## Proposed Solution

1. **Fix PostgreSQL Parameter Placeholder in `prepare_transfer_postgres`**:
   - Change `$2` to `$1` in `SELECT container_id, capacity FROM containers WHERE container_id = $1 FOR UPDATE`.
   - Add SQLite and PostgreSQL test coverage verifying container transfer preparation and commit.

2. **Eliminate Cartesian Product in Reservation Deletions**:
   - Replace `IN (?, ?) AND slot IN (?, ?)` with exact pairwise predicates:
     `((container_id = ? AND slot = ?) OR (container_id = ? AND slot = ?)) AND expires_at_epoch_ms <= ?` in SQLite,
     and `$1..$5` in PostgreSQL.

3. **Atomic Redis Handover Claiming**:
   - Replace separate `GET` + `DEL` in `RedisCache::claim_handover` with an atomic Lua script (`GET` then `DEL` returning `val`).

4. **Capacity Validation in `refund_escrow`**:
   - Check `if free_slots.len() < escrow_items.len() { return Err(StorageError::ContainerFull(dep_cid as u64, dep_cap as u32)); }` in both SQLite and PostgreSQL `refund_escrow` implementations.

5. **Safe Marketplace Expiry Reaping & Audit Settlement**:
   - In `reap_expired_listings` (both SQLite and Postgres), verify the item was successfully moved to the seller's container. If the seller container is missing or full, rollback and leave the listing pending until space is freed.
   - In `purchase_listing` (both SQLite and Postgres), record transactional audit logging into `container_transactions` for both item transfer and payment settlement.

6. **Active Redis PubSub Forwarding in `RedisSocialBus`**:
   - Maintain subscribed channels and launch background PubSub subscription listeners for newly subscribed channels.
   - Forward incoming Redis messages into `memory_fallback`, with origin node deduplication to prevent self-echo.

## Scope & Non-Goals
- **In Scope**:
  - `crates/storage/src/transaction_coord.rs`
  - `crates/storage/src/cache.rs`
  - `crates/storage/src/escrow_coord.rs`
  - `crates/storage/src/marketplace.rs`
  - `crates/storage/src/social.rs`
  - Unit and integration tests for all 6 bug areas.
- **Non-Goals**:
  - Adding game-specific currency classes or RPG mechanics (must adhere to `AGENTS.md` Prime Directive).
  - Changing external gateway protocols or wire formats.
