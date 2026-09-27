## Why

The Phase 1 Codebase Audit identified two critical P0 architectural vulnerabilities:
1. **Kinematics & Anti-Cheat**: The simulation currently trusts client-reported $\Delta t$ (`dt_sec`), allowing malicious clients to bypass speed limits by reporting artificially large `dt_sec` values (`dt_sec = 100.0`), effectively permitting instant teleports without triggering snap-back. Additionally, remote command handling contains an initiator fallback that can synthesize player identity from the source node ID.
2. **Gateway Concurrency & Contention**: The Edge Gateway synchronizes every packet dispatch through a central `Arc<RwLock<SessionRouter>>`. Under high packet rates (100,000+ packets/sec across thousands of concurrent sessions), multiple worker threads suffer severe cache-line bouncing on read-locks. Furthermore, stalled QUIC stream writes block the shared downstream broadcasting channels, and task aborts can leak active routes without RAII cleanup.

Hardening these P0 areas is essential to establish server-authoritative integrity and lock-free throughput before scaling the cluster.

## What Changes

- **Server-Authoritative Monotonic Kinematics**:
  - Calculate $\Delta t$ strictly on the server using monotonic clock timestamps (`tokio::time::Instant` or `std::time::Instant`) and track the last accepted packet timestamp per entity.
  - Track packet sequence numbers per entity to reject out-of-order or duplicate replayed movement packets.
  - Enforce server-side distance credit limits and speed caps based solely on server elapsed time, completely ignoring any client-provided `dt_sec`.
  - Fix remote command initiator validation in `crates/node/src/network.rs` by rejecting undersized or invalid payloads instead of synthesizing fallback player identities.
- **Lock-Free / Sharded Session Routing & Gateway Resilience**:
  - Replace `Arc<RwLock<SessionRouter>>` with `DashMap` (or fine-grained concurrent sharding) for `sessions` and `node_endpoints`, eliminating read lock contention on `dispatch_packet` and `process_frames`.
  - Implement an RAII session drop guard (`SessionDropGuard`) for connection tasks, ensuring that if a client task is aborted or terminated unexpectedly, its route in the session router and its cached session mapping are reliably deregistered.
  - Isolate downstream client stream writes with bounded per-session buffers and non-blocking delivery to prevent slow or stalled client connections from blocking the central broadcast loop.

## Capabilities

### New Capabilities
None.

### Modified Capabilities
- `simulation/tick`: Mandate that movement verification computes elapsed delta time strictly from server-side monotonic timestamps and sequence monotonicity, rejecting spoofed client time and undersized remote commands.
- `gateway/routing`: Require concurrent, lock-sharded packet routing without global read-lock contention, RAII session cleanup on connection abort, and isolated downstream stream delivery.

## Impact

- **Affected Crates**:
  - `crates/node`: `sim.rs`, `kinematics.rs`, `network.rs`
  - `crates/gateway`: `server.rs`, `router.rs`
  - `crates/schema`: `client.rs` (if framing adjustments are needed)
- **Dependencies**: `dashmap` in `crates/gateway` (already present or to add to `Cargo.toml`).
- **APIs & Wire Protocol**: Wire protocol remains backwards compatible with `ClientPacket::Movement`, but client-supplied `dt_sec` is strictly ignored by the server simulation.
