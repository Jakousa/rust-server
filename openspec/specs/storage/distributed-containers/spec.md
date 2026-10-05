# Distributed Transactional Containers & Generic Escrow Specification

## Purpose

Provides a generic, high-performance container storage model, a distributed two-phase commit (2PC / Sagas) transaction coordinator for zero-duplication asset transfers across cluster nodes, and an asynchronous constant-time escrow state engine.

## Requirements

### Requirement: Universal Container Data Model
The storage tier SHALL provide a generic container subsystem supporting arbitrary container instances identified by a 64-bit integer GUID.

#### Scenario: Container access and capacity bounding
- **GIVEN** a container record with owner (`Character`, `Guild`, `WorldEntity`, or `System`), slot capacity, and access control policy (`OwnerOnly`, `PublicReadWrite`, `PublicReadOwnerWrite`, `RoleRestricted`)
- **WHEN** inserting items into the container
- **THEN** slots SHALL be indexed from 0 to `capacity - 1`, and insertions exceeding container capacity SHALL be rejected

### Requirement: Two-Phase Commit Asset Transfer
Cross-container transfers SHALL execute via a two-phase reservation protocol guaranteeing that items cannot be duplicated or deleted in transit.

#### Scenario: Phase 1 Prepare reservation lock
- **GIVEN** a request to transfer an item from a source container to a destination container
- **WHEN** `prepare_transfer` is executed
- **THEN** the coordinator SHALL acquire bounded-TTL reservations on the source item slot and destination slot with a unique transaction ID (`tx_id`)
- **AND** reserved items and slots SHALL be locked against concurrent gameplay modifications

#### Scenario: Phase 2 Commit finalization
- **GIVEN** a prepared transfer with active reservations
- **WHEN** `commit_transfer` is executed before lease expiry
- **THEN** the coordinator SHALL atomically verify and delete reservations (`DELETE FROM container_reservations WHERE tx_id = $1 AND expires_at_epoch_ms > $now`), transfer the item into the destination slot, and log the transaction in `container_transactions`

#### Scenario: Phase 3 Abort and rollback
- **GIVEN** a prepared transfer that encounters failure, network partition, or lease timeout
- **WHEN** `abort_transfer` is executed or the background reaper cleans expired leases
- **THEN** reservation locks SHALL be deleted without moving the item, restoring the source item to available status

### Requirement: Deadlock-Free Ordered Row Locking
Whenever a transaction involves multiple containers in the same database session, container locks SHALL be acquired in strictly increasing numerical order of `container_id` (`min(c1, c2)` followed by `max(c1, c2)`).

#### Scenario: Concurrent opposing transfers
- **GIVEN** two concurrent transfers between containers `C1` and `C2` in opposite directions
- **WHEN** transactions acquire locks
- **THEN** both transactions SHALL acquire lock on `min(C1, C2)` before `max(C1, C2)`, preventing circular-wait deadlocks

### Requirement: Generic Asynchronous Escrow State Machine
The storage subsystem SHALL provide a generic escrow primitive holding items conditionally pending authorization secret verification or expiration.

#### Scenario: Escrow creation and deposit
- **GIVEN** a depositor container and beneficiary container
- **WHEN** `create_escrow` is invoked with an authorization secret hash and TTL
- **THEN** items SHALL be moved to a system-owned escrow container with status `HeldInEscrow`

#### Scenario: Constant-time escrow release
- **GIVEN** an active escrow in `HeldInEscrow` status
- **WHEN** `release_escrow` is invoked with the authorization secret
- **THEN** the secret SHALL be verified against the stored hash using constant-time byte comparison (`subtle::ConstantTimeEq`), and items SHALL be transferred to the beneficiary container upon match

#### Scenario: Escrow cancellation or expiration refund
- **GIVEN** an escrow in `HeldInEscrow` status
- **WHEN** depositor cancels or the TTL expires
- **THEN** the escrow status SHALL transition to `Refunded` and all held items SHALL be returned to the depositor container
