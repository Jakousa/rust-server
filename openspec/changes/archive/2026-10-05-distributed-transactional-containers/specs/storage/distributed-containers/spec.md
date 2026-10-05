# Specification: Distributed Transactional Containers & Generic Escrow

## 1. Domain Model & Abstractions

### 1.1 Container Primitives
1. **Container Identifier**: 64-bit integer (`container_id: u64`), unique across the entire cluster.
2. **Container Ownership**:
   ```rust
   pub enum ContainerOwner {
       Character(u64),
       Guild(u64),
       WorldEntity(u64),
       System(String),
   }
   ```
3. **Container Metadata**:
   - `container_id: u64`
   - `owner: ContainerOwner`
   - `name: String`
   - `capacity: u32`
   - `access_policy: ContainerAccessPolicy`
   - `version: u64`
   - `updated_at: i64`
4. **Access Control Policies**:
   ```rust
   pub enum ContainerAccessPolicy {
       OwnerOnly,
       PublicReadWrite,
       PublicReadOwnerWrite,
       RoleRestricted { min_role: u32 },
   }
   ```
5. **Container Slot & Item Instance**:
   - An item inside a container is represented by `ItemRecord` keyed to `container_id` and `slot: u32` (where `0 <= slot < capacity`).
   - Slots are strictly bounded by `capacity`. Over-capacity insertions MUST be rejected.

---

## 2. Distributed Two-Phase Transaction Protocol (2PC / Sagas)

### 2.1 Transaction States
```
               +-----------+
               |  Pending  |
               +-----+-----+
                     |
         +-----------+-----------+
         |                       |
         v                       v
   +-----------+           +-----------+
   | Committed |           |  Aborted  |
   +-----------+           +-----------+
```

### 2.2 Protocol Phases
1. **Phase 1: Prepare (Reservation Lock)**
   - The coordinator generates a unique `tx_id: String` (UUIDv4) and acquires a bounded reservation lease on:
     - Source item slot: marked `ReservedForTransferOut(tx_id)` with `lease_expires_at`.
     - Target container slot: marked `ReservedForTransferIn(tx_id)` with `lease_expires_at`.
   - **Invariants**:
     - Reserved items cannot be moved, consumed, dropped, or modified by regular gameplay actions while leased.
     - Reserved destination slots cannot be filled by other concurrent transactions.
     - Lease duration is strictly bounded (default: `5,000ms`).
2. **Phase 2: Commit (Atomic Finalization)**
   - Coordinator invokes `CommitTransfer(tx_id)` within a database transaction:
     - Atomically verifies and deletes reservations:
       `DELETE FROM container_reservations WHERE tx_id = $1 AND expires_at_epoch_ms > $now`
       asserting that both source and destination reservation rows were deleted. If 0 rows were deleted, the commit fails with `TransactionExpiredOrAborted`.
     - Re-verifies `slot < capacity` on the destination container within the container row locks.
     - Moves the item from source container to destination container slot.
     - Increments version on both containers.
     - Logs the transfer record in `container_transactions`.
     - Dispatches `ContainerTransactionEvent::Committed` to `SocialBus`.
3. **Phase 3: Abort & Compensation (Rollback)**
   - If either node rejects the transfer, network disconnects, or the reservation expires:
     - Coordinator (or background `ReservationReaper`) invokes `AbortTransfer(tx_id)`.
     - Removes reservation locks without moving the item.
     - Source item returns to normal available state.
     - Dispatches `ContainerTransactionEvent::Aborted` to `SocialBus`.

---

## 3. Generic Asynchronous Escrow State Machine

### 3.1 Escrow Lifecycle
```
   +-------------------+
   |   PendingDeposit  |
   +---------+---------+
             | Deposit confirmed
             v
   +-------------------+
   |    HeldInEscrow   |
   +----+---------+----+
        |         |
 Release|         | Cancel / Timeout
        v         v
+-----------+ +-----------+
|  Settled  | |  Refunded |
+-----------+ +-----------+
```

### 3.2 Escrow Operations
1. **`CreateEscrow`**:
   - Inputs: `depositor_container_id`, `beneficiary_container_id`, `item_instance_ids`, `authorization_secret_hash`, `ttl_seconds`.
   - Action: Atomically moves items from `depositor_container_id` into a system-managed escrow container (`owner = System("escrow")`). Sets status to `HeldInEscrow`.
2. **`ReleaseEscrow`**:
   - Inputs: `escrow_id`, `authorization_secret`.
   - Validation: Secret matches hash using **constant-time byte comparison** (`subtle::ConstantTimeEq`) and status is `HeldInEscrow` and `now < expires_at`.
   - Action: Atomically transfers all escrowed items into `beneficiary_container_id`. Sets status to `Settled`.
3. **`RefundEscrow`**:
   - Inputs: `escrow_id`.
   - Validation: Initiated by depositor before settlement OR triggered automatically by background reaper after `now >= expires_at`.
   - Action: Atomically returns all escrowed items to `depositor_container_id`. Sets status to `Refunded`.

---

## 4. Concurrency & Deadlock Prevention

### 4.1 Strict Container Lock Ordering
- Whenever a transaction touches multiple containers in the same database or node:
  - Container locks MUST ALWAYS be acquired in strictly increasing numerical order of `container_id` (`min(c1, c2)` followed by `max(c1, c2)`).
  - This mathematically guarantees freedom from circular wait and deadlock cycles across concurrent opposing transfers.

---

## 5. Verification & Acceptance Criteria

1. **Dual-Engine Migrations**: Schema migrations must execute cleanly on both PostgreSQL (`postgres://`) and SQLite (`sqlite:`).
2. **Backwards Compatibility**: Character inventory operations (`get_inventory`, `add_item`) must map to Container 0 without breaking existing gameplay or persistence tests.
3. **Deadlock Freedom Proptest**: Concurrent reverse transfers between containers (`C1 -> C2` and `C2 -> C1`) under high concurrency must never deadlock and always maintain total item conservation.
4. **Crash / Timeout Compensation**: Uncommitted transfers must automatically expire and refund without manual operator intervention.
5. **Zero Clippy / Compiler Warnings**: Clean run under `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`.
