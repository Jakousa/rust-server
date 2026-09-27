# Design: Phase 2 (P1) Memory Optimization, Simulation Determinism, and Storage Hardening

## Context

Following the Phase 1 hardening of kinematics and gateway lock-free concurrency, the cluster must now address memory allocations, spatial partition scaling, loop determinism, and database write contention identified in `docs/CODEBASE_AUDIT_AND_REFACTORING_PLAN.md`.

## Goals / Non-Goals

**Goals:**
- Eliminate heap allocations on network interaction frame decodes via `bytes::Bytes` and `Buf::copy_to_bytes`.
- Optimize AoI density throttling from $O(N \log N)$ to $O(N)$ with guarded `select_nth_unstable` and buffer pooling.
- Replace full-world per-tick entity scans in gameplay plugins with event-driven despawn hooks through `EventQueue`.
- Bound the tick catch-up loop with a monotonic `Instant` accumulator to prevent the simulation "spiral of death".
- Enable SQLite WAL mode and bound snapshot queue capacity with chunked batch persistence.

**Non-Goals:**
- Full ECS migration (keep existing `EntityStore` and `ComponentStore`).
- Replacing the Sparse Octree data structure.
- Multi-node spatial dynamic rebalancing (reserved for Phase 3).

## Decisions

### 1. Zero-Copy `InteractionPacket.payload` with `bytes::Bytes`
- **Choice**:
  - Change `InteractionPacket.payload` from `Vec<u8>` to `bytes::Bytes`.
  - In `InteractionPacket::decode(src: &mut impl Buf)`, use `src.copy_to_bytes(payload_len)`.
  - In `ClientPacket::decode_frame(buf: &mut BytesMut)`, freeze split bytes into `Bytes` (or pass `&mut frame_bytes` as `impl Buf`), ensuring `copy_to_bytes` creates a zero-copy atomic reference-counted slice rather than allocating heap memory.
- **Rationale**: `BytesMut::split_to` and `copy_to_bytes` enable zero-copy shallow slicing of incoming TCP/QUIC frames at line rate without per-packet allocations.
- **Alternatives**: Keeping `Vec<u8>` with buffer pooling (adds lifecycle complexity and borrow tracking).

### 2. $O(N)$ Partitioning via Guarded `select_nth_unstable` and Scratch Buffers
- **Choice**:
  - In `AoiManager::throttle_density`, strictly guard selection with `if candidates.len() > budget`: invoke `candidates.select_nth_unstable_by(budget, ...)` and truncate. If `candidates.len() <= budget`, skip selection to avoid out-of-bounds panics.
  - Equip `AoiManager` with pre-allocated scratch buffers (`scratch_candidates`, `scratch_enters`, `scratch_exits`) cleared between observer queries.
- **Rationale**: AoI only requires the $K$ closest entities, not a fully sorted array. `select_nth_unstable` executes in average $O(N)$ time. Reusing scratch vectors eliminates ~80,000 heap allocations/sec across active observers.
- **Alternatives**: Spatial k-d tree range queries (excessive overhead compared to octree + partial selection).

### 3. Event-Driven Despawn & Death Cleanup via `EventQueue`
- **Choice**:
  - In `TickSimulation::despawn_entity`, dispatch `Event::OnDespawn` via `HookDispatcher` and record despawns into `TickSimulation::event_queue` (`EventQueue`).
  - In `SpawnerPlugin` and `ThreatPlugin`, remove per-tick $O(N)$ full world entity iterations. `SpawnerSystem` evaluates only its own registered `spawner.spawned_entities` ($O(K)$ where $K \le 5$) and drains despawn events from `ctx.events` to prune dead entities.
- **Rationale**: Scanning all world entities every 50 ms to find dead entities scales poorly ($O(N)$ full table scan). Event dispatch combined with targeted spawner checks is $O(K)$ per spawner.
- **Alternatives**: Periodic garbage collection every 10 seconds (leaves dangling references active in spawners).

### 4. Clamped Fixed-Step Monotonic Tick Accumulator
- **Choice**:
  - Replace unchecked `tokio::time::interval(Burst)` in `TickSimulation::run_ticks` and `crates/node/src/main.rs` with a custom monotonic `Instant::now()` accumulator loop.
  - Cap catch-up ticks to `MAX_CATCHUP_TICKS = 3` per cycle. When accumulated lag exceeds this threshold, clamp the accumulator and drop residual time debt before sleeping until the next scheduled 50 ms tick boundary.
- **Rationale**: Under heavy load spikes, unbounded burst intervals repeatedly schedule catch-up ticks without yielding, starving network I/O and cascading into a complete server stall. Clamping to 3 ticks preserves cluster responsiveness.
- **Alternatives**: Variable time step `dt` (breaks kinematic reproducibility and physics stability).

### 5. SQLite WAL Mode and Bounded Snapshot Queue with Chunked Writes
- **Choice**:
  - Add `.pragma("journal_mode", "WAL")` and `.pragma("synchronous", "NORMAL")` in `mmo_storage::db::init_sqlite_pool`.
  - In `WriteBehindSnapshotter`:
    - Enforce `max_dirty_capacity: usize = 50_000`. If queue is at capacity, allow updates for existing GUIDs (coalescing), but reject new entity GUID registrations and log a rate-limited warning.
    - Chunk batch database writes into sequential slices of `chunk_size: usize = 500` to avoid SQLite variable/parameter limits.
- **Rationale**: WAL mode prevents `database is locked` errors during concurrent snapshotting. Coalescing capped queues prevents memory exhaustion while keeping active entity states fresh. Chunking prevents query statement size overflows.

## Risks / Trade-offs

- **[Risk] `select_nth_unstable` does not sort elements within the $K$ partition**:
  - *Mitigation*: AoI broadcast ordering does not require elements within the visible bubble to be strictly sorted; only the subset cutoff matters.
- **[Risk] Clamped accumulator drops simulation time under sustained CPU overload**:
  - *Mitigation*: Dropping excess lag under emergency overload preserves cluster responsiveness and prevents node death, which is preferable to complete freeze.
- **[Risk] Bounded snapshot queue discarding updates if storage fails completely**:
  - *Mitigation*: Coalescing dirty updates by GUID so existing entities retain their latest state; only new dirty registrations are throttled.
