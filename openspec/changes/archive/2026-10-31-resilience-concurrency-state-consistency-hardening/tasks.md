# Implementation Tasks: Resilience, Error-Handling, and State-Consistency Hardening

## 1. Storage Hardening (`crates/storage`)
- [x] 1.1 Update `TransactionCoordinator::abort_transfer` to use `with_dialect_tx!`, query existing reservation details to extract real container and item IDs, atomically delete reservations, and insert audit records while propagating errors.
- [x] 1.2 Update `EscrowCoordinator::create_escrow` to sort `depositor_container_id` and `escrow_container_id` before `SELECT ... FOR UPDATE` locking, and validate uniqueness of `item_instance_ids`.
- [x] 1.3 Update `MarketplaceRepository::create_listing` to sort `src_container_id` and `escrow_container_id` before `SELECT ... FOR UPDATE` locking.
- [x] 1.4 Update `CharacterRepository::execute_trade` to validate uniqueness of `item_instance_ids` before processing item transfers.
- [x] 1.5 Add unit and integration tests in `crates/storage` for `abort_transfer` atomicity, lock ordering, and duplicate item rejection.

## 2. Gateway & Social Bus Cleanup (`crates/gateway`, `crates/storage`)
- [x] 2.1 Add `async fn unsubscribe(&self, channel: &str) -> Result<(), SocialBusError>` to `SocialBus` trait and implement task cancellation in `RedisSocialBus`.
- [x] 2.2 Update `GatewayInterceptHandler::ensure_bus_listener` to abort any replaced `JoinHandle`, and update `remove_session` to abort any empty topic listeners (not just `whisper:`) and notify `SocialBus::unsubscribe`.
- [x] 2.3 Wire `intercept_handler` into `SessionDropGuard` and update TCP and QUIC server loops to pass `intercept_handler`.
- [x] 2.4 Add tests in `crates/gateway` verifying cleanup of topic tasks on session removal and disconnect.

## 3. Gameplay Hardening (`crates/gameplay`)
- [x] 3.1 Update `ThreatTableComponent::add_threat` and `evaluate_target` to reject non-finite/negative floats and use `f32::total_cmp`.
- [x] 3.2 Update `InventoryPacketHandler::handle` to validate equipment slot bounds (`<= 4`) and place replaced items directly into the vacated `inv_slot`.
- [x] 3.3 Update `PerceptionScanSystem::update` to prune dead, despawned, or out-of-range targets from `PerceptionComponent::spotted_targets`.
- [x] 3.4 Add tests in `crates/gameplay` verifying `NaN` threat safety, equip slot validation, swap placement, and perception target pruning.

## 4. Voxel Terrain Hardening (`crates/plugins/voxel`)
- [x] 4.1 Add `mark_chunk_dirty` method to `VoxelWorld`.
- [x] 4.2 Update `VoxelTickSystem::update` to re-mark chunk dirty if `write_behind.enqueue` returns `false`.
- [x] 4.3 Update `VoxelPacketHandler::handle` to return `OutOfReach` rejection when `entities.get_by_guid(entity_guid)` returns `None`.
- [x] 4.4 Add tests in `crates/plugins/voxel` verifying write-behind backpressure retry and unknown entity handling.

## 5. Director, Node, and Core Hardening (`crates/director`, `crates/node`, `crates/core`)
- [x] 5.1 In `GlobalOctreeCoordinator::handle_split`, execute `self.root.split_at` before mutating `NodeRegistry` or advancing epoch, returning early if split fails.
- [x] 5.2 Add `fallback_gateway_addr` field to `DirectorServiceImpl` and pass it in `deregister_node` when broadcasting route reassignments.
- [x] 5.3 Implement `DomainEvent` for `ContainerTransferNotification`, forward incoming notifications into `event_queue` in `TickSimulation::step`, and sweep expired `migrating_leases` older than 5 seconds.
- [x] 5.4 Wrap migration gRPC calls in `crates/node/src/network/migration.rs` with `tokio::time::timeout(Duration::from_secs(5), ...)`.
- [x] 5.5 In `crates/core/src/action.rs`, update `ActionTickSystem::update` to evaluate `cancel_on_movement`, `cancel_on_stun`, `cancel_on_target_lost` interrupt flags, emit `ActionProgressEvent`, and emit `ActionCompletedEvent`.
- [x] 5.6 Add tests in `crates/director`, `crates/node`, and `crates/core` verifying split safety, lease expiration, migration timeout, and action interrupts.

## 6. Verification & Quality Gates
- [x] 6.1 Run `cargo fmt --all -- --check`.
- [x] 6.2 Run `cargo clippy --workspace --all-targets -- -D warnings`.
- [x] 6.3 Run `cargo test --workspace`.
