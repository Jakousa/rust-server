# Technical Design: Node Simulation Runtime Hardening

## Architecture & Data Flow

### 1. Gateway Disconnect Full Lifecycle Cleanup (`crates/node/src/runner.rs`)
- In `crates/node/src/runner.rs:280-288`:
  - When `client.deliver_broadcast_jobs(req).await` yields `inner.disconnected_sessions`:
  - Acquire `sim_for_cleanup.write().await`.
  - For each `sid` in `disconnected_sessions`:
    - Safely unregister the AoI observer: `sim_guard.unregister_aoi_observer(sid)`.
    - If an observer existed (`Some(obs)`):
      - Extract `player_guid = obs.player_guid`.
      - Fetch latest session epoch: `sim_guard.session_epochs.get(&player_guid).copied().unwrap_or(0)`.
      - Invoke `sim_guard.disconnect_player_session(player_guid, epoch)`.
      - If `disconnect_player_session` returns `None` (e.g. entity was already despawned from `entity_store` or migrated):
        - Fallback to `sim_guard.despawn_entity(player_guid)` to guarantee full despawn cleanup, tombstone insertion, and removal from all tracking maps.
  - `disconnect_player_session` handles:
    - Authoritative entity snapshot serialization (`attributes`, `state_blob`).
    - Enqueueing payload to `sim_guard.snapshot_sender` for background asynchronous DB persistence.
    - Delegating to `sim_guard.despawn_entity(player_guid)`:
      - Kinematic state eviction.
      - Migration lease and tick eviction.
      - Session epoch eviction.
      - Insertion into `sim_guard.tombstoned_entities`.
      - Removal from AoI observer store (`aoi.unregister_by_guid(player_guid)`).
      - Clearing rate limits and instance assignments.
      - Despawning from `entity_store` and `octree`.
      - Dispatching `Event::OnDespawn` to registered `GameplayPlugin` hooks.
      - Pushing `EntityDespawnEvent` to `event_queue`.

### 2. Batch Entity Migration Tombstoning & Verification (`crates/node/src/network/migration.rs`)
- Point of no return in `execute_batch_entity_migration_pooled`:
  - Verify `sim_guard.tombstoned_entities.insert(guid);` is executed for all migrated GUIDs.
  - Implement dedicated unit test `test_batch_migration_tombstones_departed_entities` ensuring that upon successful batch ingestion, donor node marks every migrated GUID as tombstoned, preventing subsequent packet rehydration.

### 3. Atomic Multi-Node Boundary AoE Compensation Aggregator (`crates/node/src/ability_dispatcher.rs`)
- When casting a boundary AoE ability in `cast_boundary_sphere_aoe`:
  - Pre-filter reachable remote peers from `targeting.remote_batches`:
    ```rust
    let eligible_remote_batches: Vec<(u32, &Vec<Candidate>, String)> = targeting
        .remote_batches
        .iter()
        .filter_map(|(node_id, candidates)| {
            dispatcher.peer_endpoint(*node_id).map(|ep| (*node_id, candidates, ep.to_string()))
        })
        .collect();
    let total_batches = eligible_remote_batches.len();
    ```
  - If `local_hit_count == 0` and `total_batches == 0`:
    - Immediate refund: if `ability.resource_cost` is present, enqueue `AbilityCompensation` to `sim.queue_ability_compensation(...)` (or channel).
  - If `total_batches > 0`:
    - If `local_hit_count == 0` and `ability.resource_cost.is_some()`:
      - Instantiate `Arc<BoundaryAoeCompensationAggregator>`:
        ```rust
        pub struct BoundaryAoeCompensationAggregator {
            remaining_batches: AtomicUsize,
            any_hit_scored: AtomicBool,
            caster_guid: Guid,
            ability_id: u32,
            resource_name: Option<String>,
            resource_amount: f32,
            compensation_tx: tokio::sync::mpsc::Sender<AbilityCompensation>,
        }
        ```
      - For each remote peer request:
        - Dispatch remote batch request along with a clone of `Arc<BoundaryAoeCompensationAggregator>`.
      - In each remote task:
        - Inspect response: if `resp.results.iter().any(|r| r.success)`, store `any_hit_scored.store(true, Ordering::Release)`.
        - Atomically decrement `let prev = remaining_batches.fetch_sub(1, Ordering::AcqRel)`.
        - If `prev == 1` (the last remote batch to complete):
          - Check if `!any_hit_scored.load(Ordering::Acquire)`:
            - If zero hits occurred across all remote nodes, send exactly one `AbilityCompensation` refund to `compensation_tx`.
            - If at least one hit was scored, do not send any refund.
    - If `local_hit_count > 0`:
      - No aggregator or refund is dispatched, since local targets were struck and resources were legitimately consumed.

### 4. Movement Sequence Monotonicity & Modular Wraparound Arithmetic (`crates/node/src/sim.rs`)
- In `sim.rs:1608`:
  - Replace `s.last_sequence.saturating_add(1)` with `s.last_sequence.wrapping_add(1)`.
- In `process_client_move_with_seq`:
  - Ensure RFC 1982 modular comparison:
    ```rust
    let diff = sequence.wrapping_sub(state.last_sequence) as i32;
    if diff <= 0 {
        return Err(current_pos);
    }
    ```
- Add unit tests verifying:
  - Sequence jump from `u32::MAX - 1` -> `u32::MAX` -> `0` -> `1`.
  - Replay and backwards sequence numbers across wraparound boundary are rejected.

### 5. Independently Synchronized GhostStore with Tick-Based TTL Pruning (`crates/node/src/network/ghost.rs`)
- Encapsulate `GhostStore`:
  ```rust
  #[derive(Debug, Clone, Default)]
  pub struct GhostStore {
      inner: Arc<std::sync::RwLock<GhostStoreInner>>,
      current_tick: Arc<std::sync::atomic::AtomicU64>,
  }

  #[derive(Debug, Clone, Default)]
  struct GhostStoreInner {
      ghosts: HashMap<Guid, GhostRecord>,
  }
  ```
- In `GhostRecord`:
  - Add `pub local_tick: u64` tracking the local node tick when the ghost was upserted.
  - Implement helper `GhostRecord::new(...)` or default constructor to ease testing.
- In `GhostStore`:
  - `advance_tick(&self, tick: u64)`: updates `current_tick`.
  - `update_from_request(&self, req: &GhostSyncRequest) -> usize`:
    - Acquires read of `current_tick`.
    - Acquires write lock on `inner` ONCE for the entire batch.
    - Sets `record.local_tick = current_tick`.
    - Upserts all ghost records in the batch.
  - `prune_stale(&self, current_tick: u64, ttl_ticks: u64) -> usize`:
    - Acquires write lock on `inner`.
    - `ghosts.retain(|_, record| current_tick.saturating_sub(record.local_tick) < ttl_ticks)`.
    - Returns number of pruned ghost records.
  - `query_radius(&self, center: (f32, f32, f32), radius: f32) -> Vec<GhostRecord>`:
    - Acquires read lock on `inner`.
    - Returns cloned vector of matching `GhostRecord`s.
  - `get(&self, guid: Guid) -> Option<GhostRecord>`:
    - Acquires read lock on `inner`.
    - Returns `Option<GhostRecord>`.
  - `contains`, `len`, `is_empty`, `remove`, `clear`:
    - Synchronized internally via `inner`.
- In `TickSimulation::step(...)`:
  - Invoke `self.ghost_store.advance_tick(self.tick_counter)`.
  - Periodically (or every tick) invoke:
    `self.ghost_store.prune_stale(self.tick_counter, DEFAULT_GHOST_TTL_TICKS);`
    where `const DEFAULT_GHOST_TTL_TICKS: u64 = 20;` (1 second at 20 Hz).

### 6. Contention-Free Ghost Synchronization (`crates/node/src/network/service.rs`)
- In `NodeServiceImpl`:
  ```rust
  pub struct NodeServiceImpl {
      pub sim: Arc<tokio::sync::RwLock<TickSimulation>>,
      pub ghost_store: GhostStore,
      pub transaction_coord: Option<Arc<mmo_storage::TransactionCoordinator>>,
      input_tx: tokio::sync::mpsc::Sender<crate::sim::QueuedClientInput>,
  }
  ```
- In `NodeServiceImpl::sync_ghosts`:
  - Execute:
    ```rust
    let synced_count = self.ghost_store.update_from_request(&req);
    ```
  - **No lock on `self.sim` is acquired whatsoever.**
  - Incoming gRPC ghost sync calls update `GhostStore` concurrently and immediately return without contending with the simulation tick loop in `runner.rs`.
