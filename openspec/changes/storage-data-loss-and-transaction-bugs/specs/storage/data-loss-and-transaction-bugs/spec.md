# Storage Data Loss, Concurrency, and Transaction Bugfixes Specification

## Requirements

### Requirement: Positional Parameter Alignment in PostgreSQL 2PC
The system SHALL bind SQL query parameters strictly aligned with positional indices in PostgreSQL queries.

#### Scenario: Preparing transfer on PostgreSQL backend
- GIVEN a PostgreSQL database backend with two valid containers
- WHEN `TransactionCoordinator::prepare_transfer_postgres` is invoked
- THEN both container rows are locked using query `SELECT container_id, capacity FROM containers WHERE container_id = $1 FOR UPDATE` without parameter count mismatch errors.

### Requirement: Disjoint Slot Pair Reservation Cleanup
The system SHALL clean up expired reservations strictly matching the exact `(container_id, slot)` pairs involved in the transfer, avoiding cross-product deletions.

#### Scenario: Cleaning expired reservations during prepare_transfer
- GIVEN expired reservations on `(src_container_id, dst_slot)` and `(dst_container_id, src_slot)`
- WHEN `TransactionCoordinator::prepare_transfer` runs for `(src_container_id, src_slot)` and `(dst_container_id, dst_slot)`
- THEN only expired reservations for `(src_container_id, src_slot)` and `(dst_container_id, dst_slot)` are deleted, and orthogonal slots remain untouched.

### Requirement: Atomic Handover Claiming in Redis
The system SHALL claim staged handover entities atomically so that concurrent requests cannot duplicate state across nodes.

#### Scenario: Concurrent handover claims
- GIVEN a staged handover payload in Redis cache for a character GUID
- WHEN two concurrent nodes or workers invoke `claim_handover(guid)`
- THEN exactly one node receives `Some(HandoverData)` and the other receives `None`.

### Requirement: Escrow Refund Capacity Verification & Non-Blocking Sweep
The system SHALL reject an escrow refund before updating items or status if the depositor's destination container lacks capacity for all escrowed items, and background reaper sweeps SHALL not abort on full depositor containers.

#### Scenario: Refunding escrow to a full container
- GIVEN an active escrow holding 3 items
- AND a depositor container with only 1 free slot
- WHEN `EscrowCoordinator::refund_escrow` is invoked
- THEN the operation returns `Err(StorageError::ContainerFull)` and no items or status are modified.

#### Scenario: Reaping batch of escrows when one depositor container is full
- GIVEN two expired escrows in the system
- AND escrow 1 belongs to a depositor whose container is full
- AND escrow 2 belongs to a depositor whose container has free space
- WHEN `EscrowCoordinator::reap_expired_escrows` executes
- THEN escrow 1 is skipped without stranding items
- AND escrow 2 is successfully refunded and settled
- AND the sweep completes without aborting.

### Requirement: Marketplace Expiry Safety and Settlement Logging
The system SHALL never mark an expired listing as `Expired` unless the escrowed item has been safely returned to the seller's container, SHALL condition reaper updates on `status = 'Active'`, and SHALL record transactional audit logging on purchase.

#### Scenario: Reaping expired listing when seller container is full
- GIVEN an expired marketplace listing
- AND the seller's container has 0 available slots
- WHEN `MarketplaceRepository::reap_expired_listings` is executed
- THEN the listing status remains unmodified, the item remains safely in escrow, and the transaction is rolled back.

#### Scenario: Reaping listing concurrently with purchase or cancel
- GIVEN an expired candidate listing
- WHEN a concurrent transaction purchases the listing (changing status to `Sold`) before reaper update executes
- THEN the reaper's conditional `WHERE listing_id = ? AND status = 'Active'` update affects 0 rows
- AND the reaper rolls back without overwriting `Sold` status or duplicating refunded items.

#### Scenario: Purchasing marketplace listing
- GIVEN an active marketplace listing with price `P`
- AND a buyer container with free capacity
- WHEN `MarketplaceRepository::purchase_listing` completes successfully
- THEN the item is transferred to the buyer container
- AND listing status is set to `Sold`
- AND audit rows for item delivery and payment settlement are inserted into `container_transactions`.

### Requirement: Redis PubSub Cross-Node Message Forwarding
The system SHALL forward topic messages published to Redis across cluster nodes to local subscribers on each node, and SHALL reconnect upon transient connection failures.

#### Scenario: Subscribing to Redis social bus
- GIVEN two instances of `RedisSocialBus` connected to Redis
- WHEN instance B subscribes to channel `"chat_global"`
- AND instance A publishes a `SocialEnvelope` to `"chat_global"`
- THEN instance B's subscription receiver yields the published `SocialEnvelope` without duplicates.
