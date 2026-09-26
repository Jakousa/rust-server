# Proposal: Phase 2 (P1) Memory Optimization, Simulation Determinism, and Storage Hardening

## Why

During peak player density and high-throughput combat, the MMO cluster encounters memory allocation churn, non-deterministic iteration order, potential tick loop lockups under spike load, and database write contention:
1. **Wire Codec Allocations**: `InteractionPacket` allocates a new heap `Vec<u8>` on every incoming packet frame decode, causing high memory churn at tens of thousands of packets per second.
2. **AoI Spatial Selection**: Density throttling performs an $O(N \log N)$ `sort_unstable_by` over the entire spatial query result vector, and allocates fresh vectors on every tick for every observer.
3. **Gameplay Full-World Scans**: `SpawnerPlugin` and `ThreatPlugin` scan every entity in the world on every tick to clean up despawned or dead entities ($O(N)$), degrading simulation performance with world scale.
4. **Tick Loop "Spiral of Death"**: Under tick overruns exceeding 50 ms, `MissedTickBehavior::Burst` runs consecutive catch-up ticks without yielding to Tokio, starving network I/O and cascading into severe server stalls.
5. **Storage Contention & Unbounded Queues**: SQLite without WAL mode encounters `database is locked` errors during concurrent snapshotting, and `WriteBehindSnapshotter` possesses an unbounded dirty entity queue that can cause memory leaks under database degradation.

Resolving these P1 items guarantees zero-copy network frames, $O(N)$ partitioned spatial filtering, bounded catch-up tick recovery, and contention-free concurrent snapshot persistence.

## What Changes

- **Zero-Copy Interaction Framing**:
  - Update `InteractionPacket.payload` from `Vec<u8>` to `bytes::Bytes`.
  - Use `src.copy_to_bytes(payload_len)` in `ClientPacket::decode_frame` to eliminate heap copies on incoming interaction packets.
- **AoI Partitioning & Scratch Buffer Pooling**:
  - Replace $O(N \log N)$ `sort_unstable_by` in `AoiManager::throttle_density` with $O(N)$ `select_nth_unstable`.
  - Introduce reusable scratch buffers (`scratch_entities`, `scratch_enters`, `scratch_exits`) on `AoiManager` to eliminate vector re-allocation per observer per tick.
- **Clamped Fixed-Step Tick Accumulator**:
  - Replace unchecked burst execution with a clamped tick accumulator in `TickSimulation::run_loop` that catches up at most 3-4 ticks per cycle before yielding to the Tokio runtime.
- **Event-Driven Despawn Cleanup**:
  - Introduce despawn notifications or entity removal queues for `SpawnerPlugin` and `ThreatPlugin`, eliminating per-tick $O(N)$ full world entity iterations.
- **SQLite WAL Mode & Bounded Snapshotter**:
  - Set `PRAGMA journal_mode=WAL` and `PRAGMA synchronous=NORMAL` on SQLite connection options in `crates/storage/src/db.rs`.
  - Enforce a maximum capacity cap (e.g. 50,000 entities) on `WriteBehindSnapshotter` dirty queue with chunked batch writes (e.g. max 500 entities per flush query).

## Capabilities

### Modified Capabilities
- `simulation/aoi`: Update AoI requirements to mandate $O(N)$ density partition selection and zero-allocation scratch buffer reuse.
- `simulation/tick`: Update simulation tick requirements to mandate a clamped tick accumulator that prevents catch-up starvation.
- `storage/persistence`: Update storage persistence requirements to mandate WAL mode configuration, bounded snapshot queue capacity, and chunked batch flushes.
- `gameplay/plugins`: Update gameplay plugin requirements to mandate reactive/event-driven dead entity pruning instead of per-tick world iteration.

## Impact

- `crates/schema`: `InteractionPacket` uses `bytes::Bytes` for payload.
- `crates/node`: `aoi.rs` refactored to use `select_nth_unstable` and buffer pooling; `sim.rs` tick loop clamped; `snapshotter.rs` enforces bounded dirty queue and batch chunking.
- `crates/storage`: `db.rs` enables SQLite WAL mode.
- `crates/gameplay`: `spawner.rs` and `threat.rs` clean up dead entities reactively without full table scans.
- `crates/gateway` and `crates/loadtest`: adapted to `InteractionPacket.payload: Bytes`.
