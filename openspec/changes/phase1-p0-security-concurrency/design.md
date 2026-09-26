## Context

See `proposal.md` for motivation. The MMO edge server must handle high client concurrency (5,000+ players per node) and maintain server-authoritative integrity against client tampering. Currently:
1. `crates/node/src/sim.rs` and `kinematics.rs` accept client-provided `dt_sec`, which enables clients to send arbitrary delta time values (e.g. `dt_sec = 100.0`) to mask rapid displacement and bypass speed hacks.
2. `crates/node/src/network.rs` synthesizes fallback player GUIDs for truncated remote command requests.
3. `crates/gateway/src/server.rs` uses `Arc<RwLock<SessionRouter>>` and `Arc<RwLock<HashMap<u64, mpsc::Sender<Bytes>>>>` on the critical packet reception and dispatch hot paths, causing reader lock cache-line contention and lock-poisoning risk across worker threads.
4. Client stream handling lacks RAII drop cleanup and can suffer backpressure stalls across downstream broadcast loops.

## Goals / Non-Goals

**Goals:**
- Eliminate client reliance for kinematics: compute server-side $\Delta t$ using monotonic timestamps, track packet sequence monotonicity, and reject speed hacks / teleports regardless of client claims.
- Validate remote entity command payloads strictly without fallback player identity synthesis.
- Replace coarse `RwLock` structures in `SessionRouter` and `GatewayServer` (`session_sinks`) with concurrent sharded collections (`DashMap`), achieving lock-free read dispatch.
- Implement an RAII `SessionDropGuard` to guarantee automatic session and route deregistration when client tasks terminate.
- Prevent downstream stream backpressure from stalling broadcast delivery to healthy sessions.

**Non-Goals:**
- Changing client packet wire format (wire layout remains backwards compatible; client-provided `dt_sec` is simply ignored by the server).
- Client prediction or lag compensation reconciliation (Phase 2/future work).
- Redesigning cluster octree spatial handover logic (Phase 3).

## Decisions

### 1. Server-Authoritative Kinematics & Sequence Monotonicity
- **Decision**: In `SimulationEngine`, maintain an entity kinematic tracking table:
  ```rust
  pub struct EntityKinematicState {
      pub last_move_time: std::time::Instant,
      pub last_sequence: u32,
  }
  ```
- **Monotonic $\Delta t$ Calculation & Deterministic Test Support**:
  - The server tracks the monotonic timestamp when an entity last moved.
  - To ensure unit tests running within sub-microsecond execution windows do not trigger division-by-zero or false positive speed violations, `SimulationEngine` supports deterministic/mockable time (e.g. `process_client_move_at(guid, new_pos, seq, timestamp)` or elapsed $\Delta t$ fallback when time does not advance).
  - When a movement packet arrives:
    - If first move or idle gap $> 1.0\text{s}$, clamp $\Delta t$ to nominal tick duration (50ms = 0.05s) to establish a fresh baseline and prevent "time banking" exploits.
    - Otherwise, compute $\Delta t = \text{now} - \text{last\_move\_time}$.
    - Completely ignore any client-provided `dt_sec`.
    - If `packet.sequence <= last_sequence` (without wrapping), reject replayed or out-of-order packets.
    - Pass through `kinematics.validate_move(current_pos, new_pos, dt_sec)`.
- **Alternatives Considered**:
  - *Trust client timestamp with RTT offset*: Flawed because clients can easily falsify timestamps. Monotonic server time is immune to client clock manipulation.

### 2. Strict Remote Command Initiator Validation
- **Decision**: In `crates/node/src/network.rs`, update `extract_initiator(req: &RemoteCommandRequest) -> Result<Guid, NetworkError>`.
  - If `req.payload.len() < 8`, return `Err(NetworkError::MalformedPayload("Remote command payload missing initiator GUID"))`.
  - If the decoded GUID is invalid (`Guid::from_raw(0)`), reject the request.
  - Update `execute_remote_command` to immediately reject the command with `ResponseStatus::Vetoed` / `ResponseStatus::Error` instead of executing under a synthetic player GUID.
- **Alternatives Considered**:
  - *Keep fallback for testing*: Security hazard. Unit tests should explicitly provide valid initiator GUIDs using `make_remote_command`.

### 3. Concurrent Lock-Sharded SessionRouter & Downstream Sinks
- **Decision**: Adopt `dashmap::DashMap` inside `SessionRouter` or `GatewayServer`.
  ```rust
  pub struct SessionRouter {
      session_to_guid: DashMap<u64, Guid>,
      guid_to_session: DashMap<Guid, u64>,
      guid_to_node: DashMap<Guid, String>,
      session_to_node: DashMap<u64, String>,
  }
  ```
  - `dispatch_packet` and `process_frames` execute concurrently across worker threads with $O(1)$ lock-free read lookups into `DashMap`, without holding any global `RwLock`.
  - **Multi-Map Transaction Ordering**: To avoid transient inconsistencies between parallel maps without cross-map locks:
    - During **registration**: update identity maps (`session_to_guid`, `guid_to_session`) *first*, and routing destinations (`session_to_node`, `guid_to_node`) *last*.
    - During **deregistration**: remove routing destinations *first*, and identity maps *last*. This guarantees packet dispatchers either find a valid route or safely fall back to unrouted without dangling references.
  - `session_sinks` in `GatewayServer` is similarly refactored from `RwLock<HashMap<u64, mpsc::Sender<Bytes>>>` to `Arc<DashMap<u64, mpsc::Sender<Bytes>>>`.

### 4. RAII `SessionDropGuard`
- **Decision**: Wrap client connection tasks with a guard struct:
  ```rust
  pub struct SessionDropGuard {
      session_id: u64,
      server: GatewayServer,
  }
  impl Drop for SessionDropGuard {
      fn drop(&mut self) {
          self.server.remove_session_sync(self.session_id);
      }
  }
  ```
  - When a QUIC or TCP connection task ends (whether by client disconnect, IO error, or task cancellation), `SessionDropGuard::drop` immediately removes the session from `SessionRouter`, `session_sinks`, and distributed session cache.

### 5. Downstream Stream Stall Isolation & Slow Consumer Disconnection
- **Decision**: In `send_downstream` and `broadcast_frame`, replace blocking channel operations with non-blocking `try_send`.
  - If a client's downstream buffer is saturated:
    - Ephemeral broadcast packets (`EntityState`) may be dropped with a diagnostic metric.
    - If a critical message (e.g. `MovementCorrection` or session control) fails `try_send`, the client connection is identified as severely stalled, and the gateway initiates a clean disconnection / drop of that slow consumer to protect server memory and downstream stability.

## Risks / Trade-offs

- **[Risk] Monotonic $\Delta t$ Jitter**: Real-world network latency jitter causes packets to arrive in bursts, resulting in very small $\Delta t$ for some packets followed by slightly larger $\Delta t$.
  - *Mitigation*: The kinematic validator already accepts up to `max_speed` with a small tolerance `EPSILON`. When bursts arrive, small $\Delta t$ with small displacement still passes. For large idle gaps, we clamp $\Delta t$ to single-tick nominal value.
- **[Risk] Test Fixtures Relying on Deterministic $\Delta t$**: Existing unit tests for `process_client_move` pass explicit `dt_sec = 0.05`.
  - *Mitigation*: Support `process_client_move_deterministic` / mockable time helper for test scenarios while the live production packet handler computes server monotonic $\Delta t$.
- **[Risk] DashMap Deadlocks with Cross-Key Lookups**: Mutating multiple maps in reverse order could deadlock if shard locks are acquired nestedly.
  - *Mitigation*: Map lookups in `SessionRouter` read individual keys sequentially without nesting write locks across maps.

## Migration Plan

1. Add `dashmap` to `crates/gateway/Cargo.toml`.
2. Refactor `SessionRouter` and `GatewayServer` to use `DashMap` with ordered updates.
3. Add `SessionDropGuard` to QUIC and TCP connection loops.
4. Refactor `crates/node/src/network.rs` remote command verification.
5. Implement server monotonic kinematics tracking in `crates/node/src/sim.rs` and `kinematics.rs`.
6. Verify with `cargo test --workspace` and dedicated anti-cheat / concurrency tests.
