# Proposal: Director & Storage Resilience Audit Hardening

## Motivation & Problem Statement
A comprehensive audit of cluster control plane and storage systems (Issue #170) identified critical vulnerabilities across `crates/director`, `crates/node`, and `crates/storage`:

1. **Unfenced Partitioned Node Reconnection (Split-Brain Simulation Risk)**:
   - Location: `crates/director/src/service.rs`, `crates/director/src/registry.rs`, `crates/node/src/runner.rs`
   - When a simulation node experiences network partition or timeout, `Director` detects the dead node via `check_dead_nodes`, evicts it, and reassigns its spatial partition.
   - When the partitioned node attempts to heartbeat, `DirectorServiceImpl::heartbeat` returns `acknowledged: false` with `Ok(HeartbeatResponse)` rather than an error or poison pill.
   - The node continues its local simulation loop and tries background re-registration, creating dual simulation authority (split-brain) with the newly assigned node.
   - Remediation: Return gRPC `Status::failed_precondition` from `DirectorServiceImpl::heartbeat` when an evicted or unknown node heartbeats, and handle this in `runner.rs` to fence and terminate the node simulation immediately.

2. **Non-Idempotent 2PC `commit_transfer` (False Abort on Retries)**:
   - Location: `crates/storage/src/transaction_coord.rs`
   - During two-phase commit item transfers, `commit_transfer` deletes reservation rows upon successful completion and records the transaction in `container_transactions`.
   - If a transient network glitch or client timeout drops the commit response, subsequent client retry queries `container_reservations`. Because rows were already deleted, it returns `StorageError::ReservationExpiredOrAborted`, misreporting a committed transaction as aborted.
   - Remediation: Consult the `container_transactions` audit ledger first for prior `committed` status. If found, return the existing committed record idempotently.

3. **SQLite Foreign Key Constraints Silently Disabled**:
   - Location: `crates/storage/src/db.rs`
   - SQLite connections default to foreign key enforcement disabled unless `PRAGMA foreign_keys = ON;` is executed.
   - Remediation: Ensure `.pragma("foreign_keys", "ON")` is present and add test verification that cascade actions and foreign key violations are properly enforced.

4. **Marketplace Expired Listing Reaper Starvation when Seller Inventory is Full**:
   - Location: `crates/storage/src/marketplace.rs`
   - In `reap_expired_listings`, if a seller's container is full or missing, returning the item fails and the reaper returns `Ok(false)` leaving `status = 'Active'`.
   - The query `SELECT ... FROM market_listings WHERE status = 'Active' AND expires_at_epoch_ms <= ? LIMIT ?` repeatedly selects the same blocked listings on every sweep, completely starving all subsequent expired listings from being processed.
   - Remediation: Add `ORDER BY updated_at_epoch_ms ASC` to the reaper candidate query and transition un-reapable listings to `ExpiredPendingClaim` with updated timestamps, allowing sellers to claim them later via `cancel_listing` / claim workflow once inventory space is cleared.

5. **Potential PostgreSQL Deadlock in `commit_transfer` Version Increment**:
   - Location: `crates/storage/src/transaction_coord.rs`
   - Multi-row updates must strictly adhere to canonical ascending container GUID ordering (`min(src, dst), max(src, dst)`) to prevent engine-level deadlocks under concurrent bidirectional transfers.
   - Remediation: Ensure sequential updates in ascending GUID order and verify deadlock freedom under concurrent bidirectional transfer workloads.

## Proposed Changes & User Impact
- Total prevention of split-brain entity simulation on node partitioning.
- Strict idempotency for 2PC transfers across retries.
- Verified relational integrity and cascade deletes in SQLite.
- Elimination of marketplace reaper starvation for full inventories.
- Deadlock-free container transfers.

## Non-Goals
- Changing wire formats or external network protocols.
- Introducing game-genre specific rules into core crates.
