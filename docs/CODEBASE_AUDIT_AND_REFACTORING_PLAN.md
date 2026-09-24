# Codebase Quality Audit & Refactoring Roadmap

This document captures the comprehensive codebase quality audit, architectural risk evaluation, and prioritized refactoring roadmap performed on September 23, 2026.

---

## Executive Summary

| Category | Health | Critical Risks Identified | Refactoring Urgency |
| :--- | :---: | :--- | :---: |
| **Kinematics & Anti-Cheat** | ⚠️ At Risk | Client can spoof delta time (`dt_sec = 100.0`) to bypass speed validation | **P0 (Immediate)** |
| **Concurrency & Gateway** | ⚠️ At Risk | Atomic cache-line contention on `Arc<RwLock<SessionRouter>>` per packet; stream write stalls | **P0 (Immediate)** |
| **Memory & Allocations** | ⚠️ Moderate | Heap allocation per packet in wire codec; per-tick vector allocations in AoI & gameplay | **P1 (High)** |
| **Simulation Loop** | ⚠️ Moderate | Randomized entity iteration order (non-deterministic); potential spiral of death in missed ticks | **P1 (High)** |
| **Storage & Persistence** | ⚠️ Moderate | SQLite missing WAL mode (`journal_mode=WAL`); write-behind snapshotter has unbounded queue | **P1 (High)** |
| **Cluster Orchestration** | ⚠️ Moderate | Node death leaves orphaned octree bounding boxes without reassignment; lacks epoch fencing | **P2 (Medium)** |
| **Testing & Tooling** | 🟢 Solid | 58 tests passing; lacks fuzzing (`proptest`), chaos network tests, and GitHub Actions CI | **P2 (Medium)** |

---

## Detailed Findings & Code Smells

### 1. Security & Anti-Cheat (Critical Vulnerabilities)

* **Client-Spoofed Delta Time (`crates/node/src/kinematics.rs:117`)**:
  - `speed = distance / dt_sec`. Currently, `dt_sec` is supplied directly by the client's packet (`crates/node/src/sim.rs:246`). A malicious client can send `dt_sec = 100.0`, resulting in a tiny apparent speed while traversing across the map in a single frame.
  - **Refactoring Solution**: *Never trust client $\Delta t$*. Calculate `dt_sec` strictly on the server using monotonic timestamps since the entity's last accepted movement packet. Maintain a server-side distance credit bucket and sequence monotonicity.
* **Remote Command Initiator Fallback (`crates/node/src/network.rs:167`)**:
  - If the payload is undersized, `extract_initiator` falls back to synthesizing a `Player` GUID derived blindly from `req.source_node_id`, allowing command initiator spoofing.

---

### 2. Concurrency, Contention & Network Throughput

* **`Arc<RwLock<SessionRouter>>` Contention (`crates/gateway/src/server.rs:110`)**:
  - Every single incoming packet in `process_frames` and `dispatch_packet` acquires a read lock on `router: Arc<RwLock<SessionRouter>>`. Under load (e.g. 5,000 players at 20 Hz = 100,000 packets/sec), multiple reader threads cause severe atomic lock cache-line bouncing.
  - **Refactoring Solution**: Replace `RwLock<SessionRouter>` with `DashMap` for sharded lock granularity, or use `arc-swap` for lock-free reads during steady state.
* **Backpressure Stalls in QUIC Stream Writing (`crates/gateway/src/server.rs:562-572`)**:
  - In `handle_quic_connection`, `downstream_handle` holds the stream lock across `send.write_all(&bytes).await`. If a client's link stalls, TCP/QUIC stream backpressure blocks the `mpsc::channel` consumer, backing up into the central `session_sinks` and delaying broadcasts to other sessions.
  - **Refactoring Solution**: Decouple individual stream flushes with bounded per-session queues that shed or buffer without stalling the main downstream receiver loop.
* **Drop Guard for Connection Cleanup**:
  - If a QUIC or TCP task is aborted externally, route cleanup in `server.remove_session_with_cache` may be skipped. Adding an RAII drop guard struct ensures sessions are cleanly unmapped even on panic or cancellation.

---

### 3. Simulation Determinism, Memory & Allocations

* **Non-Deterministic Entity Iteration (`crates/node/src/sim.rs:168`)**:
  - `self.entity_store.iter()` relies on an underlying `HashMap`, making iteration order non-deterministic between server restarts.
  - **Refactoring Solution**: Store keys in an index-ordered `SlotMap` or maintain a sorted vector of active GUIDs to guarantee deterministic tick processing.
* **Simulation Loop "Spiral of Death" (`crates/node/src/sim.rs:222`)**:
  - `MissedTickBehavior::Burst` will execute all accumulated missed ticks consecutively without yielding to Tokio if a heavy tick exceeds 50 ms. This starves network I/O.
  - **Refactoring Solution**: Enforce a clamped tick accumulator (max 3–4 catch-up ticks per cycle; drop or slow down simulation time if overrun persists).
* **Zero-Copy Misses in Wire Codec (`crates/schema/src/client.rs:287`)**:
  - `InteractionPacket` allocates a new `Vec<u8>` for its payload on every packet decode: `let mut payload = vec![0u8; payload_len]; src.copy_to_slice(&mut payload);`.
  - **Refactoring Solution**: Switch `payload` from `Vec<u8>` to `bytes::Bytes`, and use `src.copy_to_bytes(payload_len)` for zero-allocation buffer slicing.
* **AoI Allocations & Sorting (`crates/node/src/aoi.rs:202-227`)**:
  - Dynamic density throttling uses `sort_unstable_by` over the entire spatial query result vector ($O(N \log N)$). Replacing this with `select_nth_unstable` reduces this to $O(N)$ partitioning.
  - `packets` and `exited_guids` are allocated as fresh `Vec`s per observer on every tick. Pre-allocating reusable scratch buffers eliminates ~80,000 heap allocations/sec at scale.
* **$O(N)$ Full World Scans in Gameplay Plugins (`crates/gameplay/src/spawner.rs:178`, `threat.rs:165`)**:
  - Spawner and Threat systems run `ctx.entities.iter().filter_map(...).collect::<HashSet<Guid>>()` on every single tick across all world entities just to find dead entities.
  - **Refactoring Solution**: Subscribe to `EntityDespawnEvent` / death hooks to clean up spawners and threat tables reactively without iterating all entities.

---

### 4. Distributed State, Persistence & Storage

* **SQLite Missing WAL Mode (`crates/storage/src/db.rs:36`)**:
  - SQLite connections are initialized without `PRAGMA journal_mode=WAL`. Under concurrent writes from the write-behind snapshotter, readers encounter `database is locked` errors.
  - **Refactoring Solution**: Add `.pragma("journal_mode", "WAL")` and `.pragma("synchronous", "NORMAL")` in `SqliteConnectOptions`.
* **Write-Behind Snapshotter Queue Limits (`crates/node/src/snapshotter.rs:82`)**:
  - The dirty entity map in `WriteBehindSnapshotter` is unbounded. If database writes slow down, the map grows indefinitely. Also, batches should be chunked (e.g. 500 rows maximum) rather than writing thousands of rows in a single query.
* **Orphaned Octree Partitions on Node Failure (`crates/director/src/registry.rs:77`)**:
  - When `NodeRegistry::check_dead_nodes` evicts an unresponsive node, the Director does not reassign the deceased node's bounding box slices, creating a spatial void in the world.
  - **Refactoring Solution**: Hook node removal into `GlobalOctreeCoordinator` to merge or re-delegate orphaned octants to active neighboring nodes, stamped with an incremented epoch counter.

---

### 5. Code Quality, Testing & CI/CD

* **Enforcing Clippy Across All Targets**:
  - Fixed `clippy::approx_constant` (`3.14` $\rightarrow$ `std::f32::consts::PI`) in `entity.rs` and `repository.rs`, plus collapsed nested `if`s in `octree.rs`.
* **Testing Gaps**:
  - **Property-based testing (`proptest`)**: Spatial octree query boundaries, binary packet frame roundtrips, and kinematic speed limits are prime candidates for property testing.
  - **Chaos / network jitter tests**: Automated integration tests with simulated packet loss and out-of-order delivery over QUIC.
  - **CI Workflow**: Create `.github/workflows/rust.yml` running `cargo clippy --all-targets -- -D warnings` and `cargo test --workspace`.

---

## Prioritized Implementation Roadmap

### Phase 1: Critical Security & Concurrency (P0)
1. **Server-Side Delta Time & Anti-Cheat Validation**:
   - Store server-side monotonic timestamp and sequence counter per entity.
   - Compute `dt_sec = now - last_move_time` on the server and reject non-monotonic timestamps or excessive distance.
2. **Lock-Free / Sharded SessionRouter**:
   - Replace `RwLock<SessionRouter>` with `DashMap` or `arc-swap` to eliminate reader contention on the packet dispatch hot path.

### Phase 2: Memory Optimization & Simulation Determinism (P1)
3. **Zero-Copy Client Codec**:
   - Switch `InteractionPacket.payload` to `bytes::Bytes` and use `copy_to_bytes` in framing.
4. **AoI Allocations & Selection**:
   - Replace $O(N \log N)$ sort with $O(N)$ `select_nth_unstable`.
   - Pre-allocate scratch vectors on `AoiManager`.
5. **Event-Driven Entity Death Cleanup**:
   - Remove per-tick $O(N)$ full world entity iteration in `SpawnerPlugin` and `ThreatPlugin` by using `EntityDespawnEvent` subscriptions.
6. **SQLite WAL Mode & Bounded Snapshot Queue**:
   - Enable WAL mode in SQLite connection setup.
   - Enforce maximum queue bounds and chunked batch writes on `WriteBehindSnapshotter`.
7. **Fixed-Step Tick Accumulator**:
   - Replace `MissedTickBehavior::Burst` with clamped accumulator catching up at most 3-4 ticks.

### Phase 3: Cluster Resilience & Engineering Tooling (P2)
8. **Director Orphaned Partition Recovery**:
   - Automatically re-delegate lost spatial bounds when a node times out, guarded with epoch fencing.
9. **CI Workflow & Proptest**:
   - Add `.github/workflows/rust.yml` for automated linting and test passes.
   - Add `proptest` suites for packet framing and octree queries.
