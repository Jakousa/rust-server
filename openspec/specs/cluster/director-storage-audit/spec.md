# Director Node Fencing and Storage Engine Hardening Specification

## Purpose
Defines split-brain node eviction fencing in `mmo-director`, idempotent and deadlock-free two-phase commit (2PC) container transfers in `mmo-storage`, SQLite foreign key enforcement, and non-blocking marketplace expiry reaping.

## Requirements

### Requirement: Node Fencing on Eviction & Split-Brain Prevention
The Cluster Director SHALL track evicted simulation nodes and reject their subsequent heartbeats with `FailedPrecondition` to prevent split-brain authority, while returning `acknowledged: false` for unknown non-evicted nodes to allow self-healing re-registration.

#### Scenario: Evicted node heartbeat rejected with failed_precondition
- **GIVEN** a registered simulation node that has timed out and been evicted by `Director`
- **WHEN** the evicted node sends a `HeartbeatRequest`
- **THEN** `DirectorServiceImpl::heartbeat` MUST return `tonic::Status::failed_precondition` containing node fencing instructions
- **AND** the node heartbeat client MUST detect `FailedPrecondition` and trigger immediate simulation shutdown to prevent split-brain simulation.

#### Scenario: Unknown non-evicted node heartbeat returns unacknowledged response
- **GIVEN** an unregistered node ID that has not been evicted
- **WHEN** the unknown node sends a `HeartbeatRequest`
- **THEN** `DirectorServiceImpl::heartbeat` MUST return `Ok(HeartbeatResponse { acknowledged: false, .. })` so the node can re-register cleanly across Director restarts.

---

### Requirement: Idempotent 2PC Transfer Commit
The storage `TransactionCoordinator` SHALL guarantee idempotent execution of `commit_transfer` across retries and concurrent invocations.

#### Scenario: Retried commit_transfer returns existing committed record
- **GIVEN** a successful 2PC transfer prepared and committed with transaction ID `tx_id`
- **WHEN** `TransactionCoordinator::commit_transfer(tx_id)` is called again for the same `tx_id` (sequentially or concurrently)
- **THEN** the coordinator checks `container_transactions` (and re-checks on reservation deletion race)
- **AND** returns `Ok(ContainerTransactionRecord)` matching the original committed transaction without executing duplicate item moves or returning `ReservationExpiredOrAborted`.

#### Scenario: Retried commit_transfer on aborted transaction returns error
- **GIVEN** a transfer prepared and aborted with transaction ID `tx_id`
- **WHEN** `TransactionCoordinator::commit_transfer(tx_id)` is called
- **THEN** the coordinator returns `Err(StorageError::ReservationExpiredOrAborted)`.

---

### Requirement: SQLite Foreign Key Integrity
All SQLite connection pools SHALL enforce relational foreign key constraints by default.

#### Scenario: SQLite foreign key enforcement active
- **GIVEN** an active SQLite database connection pool created via `Database::connect` or `Database::connect_memory`
- **WHEN** foreign key pragma is queried or invalid foreign key relationships are inserted
- **THEN** foreign keys MUST be enabled (`PRAGMA foreign_keys = ON`)
- **AND** constraint violations MUST be rejected with database integrity errors.

---

### Requirement: Marketplace Expired Listing Reaper Starvation Elimination
The marketplace expired-listing reaper SHALL transition unclaimable expired listings (where the seller's inventory is full) to `'ExpiredPendingClaim'` so they do not starve subsequent expired listings.

#### Scenario: Expired listing with full seller inventory transitioned to ExpiredPendingClaim
- **GIVEN** an expired active listing whose seller container has 0 available slots
- **WHEN** `MarketplaceRepository::reap_expired_listings` runs
- **THEN** the listing status MUST be updated to `'ExpiredPendingClaim'` with updated timestamp
- **AND** the reaper MUST NOT be blocked from processing other expired listings in the queue
- **AND** the candidate query MUST order listings by `updated_at_epoch_ms ASC`.

#### Scenario: Seller claims or cancels ExpiredPendingClaim listing after freeing space
- **GIVEN** an expired listing in status `'ExpiredPendingClaim'`
- **WHEN** the seller frees inventory space and calls `cancel_listing`
- **THEN** the item is safely returned to the seller's container and status transitioned to `'Cancelled'`.

---

### Requirement: Deadlock-Free Bidirectional 2PC Transfers
Two-phase commit container operations SHALL acquire container locks in deterministic ascending GUID order to eliminate reciprocal deadlocks.

#### Scenario: Concurrent transfers between reciprocal containers do not deadlock
- **GIVEN** container A and container B
- **WHEN** concurrent tasks transfer items from A -> B and B -> A simultaneously
- **THEN** container lock acquisition and version updates follow strictly ascending GUID ordering
- **AND** both transfers commit successfully without database deadlock exceptions.
