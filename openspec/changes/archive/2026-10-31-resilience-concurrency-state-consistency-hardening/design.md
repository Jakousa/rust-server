# Technical Design: Resilience, Error-Handling, and State-Consistency Hardening

## Architecture & Data Flow

### 1. Storage: Atomic 2PC Rollback, Canonical Lock Ordering, and Duplicate Item Validation
- **Atomic 2PC Abort (`TransactionCoordinator::abort_transfer`)**:
  - Wrap the operation in `with_dialect_tx!`.
  - Fetch existing reservations for `tx_id` prior to deletion:
    - Extract `src_container_id`, `dst_container_id`, and `item_instance_id`.
    - If no reservations are found, return `Ok(false)` immediately to prevent duplicate key violations on `container_transactions (tx_id PRIMARY KEY)`.
  - Delete reservation rows atomically inside the transaction.
  - Insert audit log into `container_transactions` with status `'aborted'`, using the real container and item IDs if found, propagating any DB error.
- **Canonical Lock Ordering (`EscrowCoordinator::create_escrow`, `MarketplaceRepository::create_listing`)**:
  - Guard against identical container IDs: `depositor_container_id != escrow_container_id` and `src_container_id != escrow_container_id`.
  - Whenever two distinct containers need to be locked (`depositor` and `escrow`, or `src` and `escrow`), sort their IDs:
    `first_id = min(c1, c2)` and `second_id = max(c1, c2)`.
  - Acquire `SELECT container_id FROM containers WHERE container_id = ? FOR UPDATE` strictly in `(first_id, second_id)` order.
- **Duplicate Item Validation**:
  - In `CharacterRepository::execute_trade` and `EscrowCoordinator::create_escrow`, enforce uniqueness of `item_instance_ids` via a `HashSet`. Return `StorageError::DuplicateItem` on duplicate IDs.

### 2. Gateway & Storage: Task Lifecycle & Subscription Cleanup
- **Gateway Intercept Task Synchronization (`GatewayInterceptHandler`)**:
  - In `ensure_bus_listener(topic)`:
    - Ensure that when inserting the new listener `JoinHandle` into `active_bus_subscriptions`, any previous `JoinHandle` for that topic is aborted.
    - Check if active subscribers for the topic already dropped to 0 while awaiting subscription; if so, abort the task and return early.
  - In `remove_session(session_id)`:
    - Remove `session_id` from `subscription_manager`.
    - For each topic returned whose active subscriber count is now 0, remove it from `active_bus_subscriptions` and invoke `handle.abort()`. Also notify `social_bus.unsubscribe(topic)` using `tokio::runtime::Handle::try_current()`.
- **RAII Session Guard (`SessionDropGuard`)**:
  - Add `pub intercept_handler: Option<Arc<GatewayInterceptHandler>>` to `SessionDropGuard`.
  - In `Drop::drop`:
    - If `intercept_handler` is present, call `intercept_handler.remove_session(self.session_id)`.
    - Also ensure `sub_mgr.remove_session(self.session_id)` is invoked.
  - Pass `Some(self.intercept_handler())` when instantiating `SessionDropGuard` in TCP and QUIC server connection handlers.
- **`SocialBus::unsubscribe` and Task Cancellation**:
  - Add `async fn unsubscribe(&self, channel: &str) -> Result<(), SocialBusError>` to `SocialBus` trait with default no-op implementation.
  - In `MemorySocialBus`, remove the channel entry from `channels` map if `sender.receiver_count() == 0`.
  - In `RedisSocialBus`, store active background subscriber tasks with their `AbortHandle` in `active_tasks: Arc<Mutex<HashMap<String, tokio::task::AbortHandle>>>`.
  - When `unsubscribe` is called:
    - Remove the channel from `subscribed_channels`.
    - Abort the corresponding task via `AbortHandle::abort()`.

### 3. Gameplay: Float Safety, Equipment Slot Checks, and Target Pruning
- **Finite Float Threat Table (`ThreatTableComponent`)**:
  - In `add_threat(target, amount)`:
    - Guard check: `if !amount.is_finite() || amount <= 0.0 { return; }`.
    - Replace all `partial_cmp().unwrap()` with `total_cmp()`.
  - In `evaluate_target()`:
    - Use `total_cmp()` when finding `max_by`.
- **Inventory Packet Handler (`InventoryPacketHandler`)**:
  - Validate `payload[1]` strictly:
    - `0 => EquipSlot::Head`, `1 => EquipSlot::Chest`, `2 => EquipSlot::MainHand`, `3 => EquipSlot::OffHand`, `4 => EquipSlot::Boots`.
    - Any other value returns `Err(PacketHandlerError::MalformedPayload { module_id: 0x01, message: format!("Invalid equipment slot index {}", payload[1]) })`.
  - When swapping equipped items (`old_item` is `Some`):
    - Place `old_item` directly back into the vacated inventory slot `inv_slot`:
      ```rust
      if inv_slot < inv.slots.len() {
          inv.slots[inv_slot] = Some(old);
      } else {
          let _ = inv.add_item(old);
      }
      ```
- **Perception Target Pruning (`PerceptionScanSystem`)**:
  - During each scan in `PerceptionScanSystem::update`:
    - For each observer, retain only those targets in `perception.spotted_targets` that:
      1. Still exist in `ctx.entities`.
      2. Are not dead (`HealthComponent.is_dead == false`).
      3. Are within `sight_radius` distance of the observer.

### 4. Voxel: Backpressure Retry & Entity Validation
- **Dirty Chunk Backpressure Retry (`VoxelTickSystem`)**:
  - In `VoxelTickSystem::update`:
    - When `write_behind.enqueue(coord, &chunk)` returns `false` (bounded channel full):
      - Do not call `chunk.mark_clean()`.
      - Re-insert `coord` back into `world.dirty_chunks` via `world.mark_chunk_dirty(coord)`.
- **Entity Validation in Voxel Handler (`VoxelPacketHandler`)**:
  - In `VoxelPacketHandler::handle`:
    - If `entities.get_by_guid(entity_guid)` returns `None`, return `VoxelModifyResponsePacket { status: 1 /* OutOfReach */, target_voxel: req.target_voxel, new_voxel: req.new_voxel }`.

### 5. Director, Node, and Core Action Wiring
- **Atomic Octree Splitting (`GlobalOctreeCoordinator::handle_split`)**:
  - Prepare `assignments_arr`.
  - Attempt `self.root.split_at(&original_bounds, &assignments_arr)`.
  - If it returns `false`, return `Vec::new()` immediately without modifying `registry` or incrementing `topology_epoch`.
  - Only on `true` proceed to update `registry` bounds and epoch.
- **Fallback Gateway Address in Director Service (`DirectorServiceImpl`)**:
  - Store `pub fallback_gateway_addr: String` on `DirectorServiceImpl`, initialized from `GATEWAY_GRPC_ADDR` env var or defaults.
  - In `deregister_node`, pass `&self.fallback_gateway_addr` to `broadcast_route_reassignments`.
- **Simulation Leases & 2PC Notifications (`TickSimulation`)**:
  - Implement `DomainEvent` on `ContainerTransferNotification`.
  - In `TickSimulation::step`:
    - Forward received `ContainerTransferNotification` items directly into `self.event_queue`.
    - Sweep expired `migrating_leases` older than 5 seconds (`now.saturating_duration_since(*start_time) > Duration::from_secs(5)`).
- **Migration RPC Timeouts (`crates/node/src/network/migration.rs`)**:
  - Wrap both single-entity `client.migrate_entity(req)` and batch-entity `client.migrate_batch_entities(batch_req)` in `tokio::time::timeout(Duration::from_secs(5), ...)`.
  - Handle `Err(_elapsed)` by invalidating the pool connection, releasing the migration leases, incrementing rollback metrics, and returning `MigrationError::RpcFailed("Migration RPC timed out".to_string())`.
- **Action Interrupts & Progress Events (`ActionTickSystem`)**:
  - In `ActionTickSystem::update`:
    - Avoid borrow checker conflicts by using a two-pass approach:
      - Pass 1 (Immutable check): Query entities and determine whether active actions should be interrupted, advanced, or completed.
        - `cancel_on_movement`: if `entity.velocity.0 != 0.0 || entity.velocity.1 != 0.0 || entity.velocity.2 != 0.0`, interrupt with `InterruptReason::Movement`.
        - `cancel_on_stun`: if `entity.is_incapacitated()`, interrupt with `InterruptReason::CrowdControl`.
        - `cancel_on_target_lost`: if `action.target_guid` is `Some(tgt)` and `ctx.entities.get_by_guid(tgt)` is missing or `tgt_entity.is_incapacitated()`, interrupt with `InterruptReason::TargetLost`.
      - Pass 2 (Mutable update): Mutably advance elapsed time, handle interrupts, and emit appropriate events:
        - If interrupted, queue `ActionInterruptedEvent`.
        - If active and not interrupted, advance `action.elapsed_sec += ctx.dt_sec` and emit `ActionProgressEvent { entity, action_id, elapsed_sec, duration_sec }`.
        - If `action.is_finished()`, complete action and emit `ActionCompletedEvent`.
