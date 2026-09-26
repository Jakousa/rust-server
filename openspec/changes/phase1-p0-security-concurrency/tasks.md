## 1. Kinematics & Anti-Cheat Hardening

- [x] 1.1 Update remote command initiator extraction in `crates/node/src/network.rs` to strictly require a valid payload of at least 8 bytes, returning an error for undersized payloads instead of synthesizing fallback player identities, and verify with updated unit tests in `crates/node/src/network.rs`.
- [x] 1.2 Implement server-authoritative monotonic kinematic state tracking in `crates/node/src/sim.rs` and `crates/node/src/kinematics.rs` (tracking last accepted move timestamp and sequence per entity, calculating server elapsed $\Delta t$, clamping idle gaps, ignoring client-supplied `dt_sec`, and providing deterministic mockable time support for test execution), and verify with kinematic unit tests.
- [x] 1.3 Add anti-cheat integration tests in `crates/node/src/sim.rs` explicitly verifying that a client attempting to spoof `dt_sec = 100.0` with a large displacement triggers a speed hack violation and snap-back correction.

## 2. Lock-Free Gateway Routing & Concurrency

- [x] 2.1 Add `dashmap = "6.1"` dependency to `crates/gateway/Cargo.toml` and verify package compiles cleanly with `cargo check -p mmo-gateway`.
- [x] 2.2 Refactor `SessionRouter` in `crates/gateway/src/router.rs` to use sharded `DashMap` storage with strictly ordered map mutations (destination routes updated last on insert, first on remove) for concurrent lock-free lookups, and verify that all unit tests in `router.rs` pass.
- [x] 2.3 Refactor `GatewayServer` and `PacketDispatcher` in `crates/gateway/src/server.rs` to leverage lock-free `SessionRouter` and `DashMap`-backed `session_sinks`, removing global `RwLock` wrappers on the packet dispatch path.
- [x] 2.4 Implement `SessionDropGuard` in `crates/gateway/src/server.rs` for automatic RAII session deregistration on client task exit or cancellation, and apply non-blocking downstream dispatch (`try_send`) with slow-consumer disconnection on critical packet saturation.

## 3. End-to-End Verification & Workspace Quality

- [x] 3.1 Run `cargo test --workspace` and verify that all unit, cluster, and gateway integration tests pass cleanly.
- [x] 3.2 Run `cargo clippy --workspace --all-targets -- -D warnings` and verify zero compiler or clippy warnings.
