# Proposal: Distributed Transactional Containers, Cross-Node Vaults & Generic Escrow

## Motivation & Background
The existing item and storage system in `mmo-storage` relies on a single-purpose `inventories` table keyed directly to `character_guid`, and a specialized `execute_trade` method that only executes immediate two-player inventory exchanges within a single SQL transaction.

However, modern distributed MMO architectures require moving items and assets between arbitrary storage containers:
- **Personal Bags & Equipment**: Active character inventory carried during simulation.
- **Personal Bank Vaults**: Long-term storage accessed in towns or safe zones.
- **Shared Guild / Faction Vaults**: Multi-user shared containers accessed concurrently across different simulation nodes.
- **In-World Containers**: Ground chests, loot boxes, resource silos, and vehicle/mount cargo trunks.
- **Generic Escrow Buffers**: Conditional holding containers for pending marketplace sales, barter swaps, quest deliveries, and trade negotiations.

Hardcoding game-specific features like an "Auction House" into the server core violates the engine's unopinionated, genre-agnostic philosophy. Instead, the server platform must provide robust, generic **Distributed Transactional Containers** and an **Asynchronous Escrow Engine** that guarantee:
1. **Zero Item Duplication or Deletion**: Strict two-phase reservation locks ensure assets cannot be double-spent or lost in transit.
2. **Cross-Node Atomic Execution**: Safe transfers between containers whether they reside on the same simulation node, different simulation nodes, or offline in storage.
3. **Automated Compensation on Failure**: Network drops, node blackouts, or capacity exhaustion automatically trigger compensating rollbacks via lease timeouts.
4. **Deadlock Freedom**: Strict GUID-ordered locking across containers guarantees mathematical freedom from deadlocks during concurrent reverse transfers.

---

## Architectural Objectives
1. **Universal Container Subsystem**:
   - Model all item storage via a generic `Container` with a 64-bit `container_id`, `owner: ContainerOwner` (`Character`, `Guild`, `WorldEntity`, `System`), configurable capacity, and access control policies (`OwnerOnly`, `Public`, `RoleRestricted`).
   - Seamlessly back-migrate existing character inventories into `Container` format without data loss.
2. **Distributed Transaction Coordinator (2PC / Sagas)**:
   - Provide a `TransactionCoordinator` managing multi-container transfers:
     - `PrepareTransfer`: Reserves source item and destination slot with a bounded TTL lease.
     - `CommitTransfer`: Atomically applies the transfer and clears reservations.
     - `AbortTransfer`: Releases reservation locks and refunds original positions.
3. **Generic Asynchronous Escrow State Machine**:
   - Introduce an `Escrow` primitive holding items conditionally:
     - `CreateEscrow`: Locks deposited items in escrow pending fulfillment criteria.
     - `ReleaseEscrow`: Atomically transfers held items to the beneficiary upon verification token presentation.
     - `RefundEscrow`: Returns held items to the depositor upon cancellation or TTL expiry.
4. **Dual-Engine Persistence & Migrations**:
   - Support both distributed PostgreSQL and embedded SQLite with automated migration script `V3__containers_and_escrow.sql`.
5. **Cluster Event Notification**:
   - Broadcast container and escrow state updates across `SocialBus` to ensure real-time synchronization for connected clients across all nodes.
