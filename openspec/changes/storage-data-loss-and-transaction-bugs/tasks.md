# Implementation Tasks: Storage Data Loss, Concurrency, and Transaction Bugfixes

- [x] 1. Fix PostgreSQL Parameter Binding & Pairwise Reservation Deletion in `transaction_coord.rs` <!-- id: 1 -->
  - [x] 1.1 Correct `$2` to `$1` in `prepare_transfer_postgres` container lock query <!-- id: 1.1 -->
  - [x] 1.2 Update SQLite and PostgreSQL expired reservation deletions to use explicit pairwise `((container_id = ? AND slot = ?) OR (container_id = ? AND slot = ?))` predicates <!-- id: 1.2 -->
  - [x] 1.3 Add tests verifying pairwise reservation deletions and PostgreSQL/SQLite transfer preparation parity <!-- id: 1.3 -->

- [x] 2. Make `RedisCache::claim_handover` Atomic <!-- id: 2 -->
  - [x] 2.1 Replace `conn.get` + `conn.del` with atomic Redis Lua script returning `val` <!-- id: 2.1 -->
  - [x] 2.2 Add unit and concurrency tests for `claim_handover` in `cache.rs` <!-- id: 2.2 -->

- [x] 3. Prevent Item Loss on Escrow Refund & Harden Reaper Sweep <!-- id: 3 -->
  - [x] 3.1 Add `free_slots.len() < escrow_items.len()` capacity check to SQLite `refund_escrow` <!-- id: 3.1 -->
  - [x] 3.2 Add `free_slots.len() < escrow_items.len()` capacity check to PostgreSQL `refund_escrow_postgres` <!-- id: 3.2 -->
  - [x] 3.3 Update `reap_expired_escrows` to handle `Err(StorageError::ContainerFull(..))` without aborting the sweep <!-- id: 3.3 -->
  - [x] 3.4 Add unit tests asserting `ContainerFull` error on refund and non-blocking reaper sweep when a container is full <!-- id: 3.4 -->

- [x] 4. Fix Marketplace Expiration Reaping & Payment Audit Settlement <!-- id: 4 -->
  - [x] 4.1 In `MarketplaceRepository::reap_expired_listings` (both SQLite and Postgres), verify successful item return and rollback transaction if seller container is missing or full <!-- id: 4.1 -->
  - [x] 4.2 In `reap_expired_listings` (both SQLite and Postgres), enforce `WHERE listing_id = ? AND status = 'Active'` check and assert 1 row affected <!-- id: 4.2 -->
  - [x] 4.3 In `purchase_listing_postgres`, add `ORDER BY container_id ASC` to the container lock query <!-- id: 4.3 -->
  - [x] 4.4 In `MarketplaceRepository::purchase_listing` (both SQLite and Postgres), insert audit records into `container_transactions` for item transfer and payment settlement <!-- id: 4.4 -->
  - [x] 4.5 Add unit tests asserting listing remains unexpired when seller inventory is full during reaper pass and verifying `container_transactions` audit entries on purchase <!-- id: 4.5 -->

- [x] 5. Implement Redis PubSub Subscription Forwarding in `RedisSocialBus` <!-- id: 5 -->
  - [x] 5.1 Add `node_id` and tracked `subscribed_channels` to `RedisSocialBus` <!-- id: 5.1 -->
  - [x] 5.2 Implement active Redis PubSub subscription listener in `RedisSocialBus::subscribe` forwarding to `memory_fallback` with reconnect handling <!-- id: 5.2 -->
  - [x] 5.3 Filter out self-published envelopes using origin `node_id` <!-- id: 5.3 -->
  - [x] 5.4 Add tests for Redis pubsub subscription forwarding, wire message codec, and envelope delivery <!-- id: 5.4 -->

- [x] 6. Workspace Quality Gates & Verification <!-- id: 6 -->
  - [x] 6.1 Run `cargo test -p mmo-storage` and workspace tests <!-- id: 6.1 -->
  - [x] 6.2 Run `cargo clippy --workspace --all-targets -- -D warnings` <!-- id: 6.2 -->
  - [x] 6.3 Run `cargo fmt --all -- --check` <!-- id: 6.3 -->
