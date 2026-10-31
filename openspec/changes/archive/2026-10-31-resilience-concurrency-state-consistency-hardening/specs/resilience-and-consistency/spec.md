# Specification: Resilience, Error-Handling, and State-Consistency Hardening

## Capability: Storage Concurrency & 2PC Rollback

### Requirement: Atomic 2PC Transfer Abort
GIVEN an active container reservation for a transaction ID
WHEN `TransactionCoordinator::abort_transfer` is called
THEN the reservation deletion and transaction record insertion MUST occur within a single database transaction
AND the transaction record MUST preserve the real `src_container_id`, `dst_container_id`, and `item_instance_id`
AND any database failure MUST roll back the deletion and return an error.

### Requirement: Canonical Lock Ordering on Containers
GIVEN a concurrent operation involving two containers in `EscrowCoordinator` or `MarketplaceRepository`
WHEN locks are acquired with `SELECT ... FOR UPDATE`
THEN locks MUST be acquired in ascending order of `container_id` (`min(c1, c2)`, `max(c1, c2)`) to eliminate lock-inversion deadlocks.

### Requirement: Duplicate Item Instance Validation
GIVEN a request to trade items or create an escrow with a list of item instance IDs
WHEN the list contains duplicate item instance IDs
THEN the operation MUST be rejected before modifying container contents or state.

---

## Capability: Gateway and Storage Task Lifecycle

### Requirement: Intercept Bus Listener Synchronization
GIVEN incoming subscription requests for a topic
WHEN `ensure_bus_listener` is called concurrently
THEN any replaced `JoinHandle` MUST be aborted to prevent leaked listener tasks.

### Requirement: Subscription Cleanup on Disconnect
GIVEN an active client session with topic subscriptions
WHEN the client connection terminates
THEN `SessionDropGuard` MUST invoke `remove_session` on `GatewayInterceptHandler`
AND background bus listener tasks for topics with 0 subscribers MUST be aborted and unsubscribed from `SocialBus`.

---

## Capability: Gameplay Threat, Inventory, and Perception Hardening

### Requirement: Finite Threat Values and Total Ordering
GIVEN an entity with a `ThreatTableComponent`
WHEN threat is added with an invalid or non-finite float (e.g. `f32::NAN` or negative/zero)
THEN the threat addition MUST be ignored without panicking the simulation tick thread.

### Requirement: Strict Equipment Slot Validation and Swap Preservation
GIVEN an equip item request with an equipment slot index
WHEN the slot index is greater than 4 (unrecognized slot)
THEN the request MUST be rejected with a malformed payload error
AND when an already-equipped item is replaced, the unequipped item MUST be placed directly back into the vacated inventory slot.

### Requirement: Perception Target Pruning
GIVEN an entity with a `PerceptionComponent` and spotted hostile targets
WHEN a previously spotted target dies, despawns, or moves outside the sight radius
THEN the target MUST be pruned from `spotted_targets` during the perception scan tick.

---

## Capability: Voxel Backpressure and Entity Verification

### Requirement: Dirty Chunk Re-queue on Write-Behind Backpressure
GIVEN a modified voxel chunk marked dirty
WHEN `VoxelTickSystem` fails to enqueue the chunk into the bounded write-behind queue due to capacity saturation
THEN the chunk MUST NOT be marked clean and MUST remain dirty for retry on subsequent ticks.

### Requirement: Unknown Entity Rejection in Voxel Handler
GIVEN a voxel modification packet with an unknown or despawned entity GUID
WHEN `VoxelPacketHandler::handle` processes the request
THEN it MUST return an `OutOfReach` rejection response and MUST NOT process the modification at `(0, 0, 0)`.

---

## Capability: Director and Node Robustness

### Requirement: Transactional Octree Split
GIVEN a request to split an octree partition in `GlobalOctreeCoordinator`
WHEN `self.root.split_at` fails to split the target bounds
THEN the node registry and topology epoch MUST remain unchanged.

### Requirement: Migration RPC Timeouts and Lease Expiration
GIVEN an in-flight entity migration
WHEN the recipient node fails to respond within 5 seconds
THEN the migration gRPC call MUST time out, release the migration lease, and rollback locally
AND any abandoned migration lease in `TickSimulation` older than 5 seconds MUST be automatically expired during the tick.
