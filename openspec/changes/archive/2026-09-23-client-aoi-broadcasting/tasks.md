## 1. Simulation Area-of-Interest (AoI) Manager

- [x] 1.0 Implement zero-allocation query methods (`query_radius_into`, `query_radius_with_pos_into`, `query_radius_with_dist_sq_into`) in `crates/core/src/octree.rs` and verify with unit tests
- [x] 1.1 Implement `ObserverConfig`, `CachedEntityState`, and `ObserverState` data structures in `crates/node/src/aoi.rs` and verify with unit tests
- [x] 1.2 Implement `AoiManager` with observer registration, deregistration, and position tracking using reusable scratch query buffers, verifying with unit tests
- [x] 1.3 Implement spatial delta calculation (`entered`, `updated`, `exited`) and packet emission (`EntityState`, `EntityDespawn`) in `AoiManager` and verify with unit tests
- [x] 1.4 Implement dynamic density-based radius throttling in `AoiManager` using distance-squared sort when visible entity count exceeds `max_entities` and verify with unit tests

## 2. Gateway Downstream Packet Delivery

- [x] 2.1 Add downstream session sink registry (`SessionSinks`) to `GatewayServer` allowing packet broadcasting by session ID and verify with unit tests
- [x] 2.2 Refactor `handle_connection` to split TCP stream into read/write halves and asynchronously pipe downstream packets from session channel to client socket, verifying with mock client test

## 3. Simulation Loop Integration & End-to-End Testing

- [x] 3.1 Integrate `AoiManager` into `TickSimulation`, ticking AoI broadcasting at 20 Hz and queuing broadcast jobs, verifying with unit tests
- [x] 3.2 Create end-to-end integration test `crates/node/tests/aoi_broadcast_e2e.rs` verifying multi-client visibility enter, update, and leave lifecycle over simulated gateway channels
- [x] 3.3 Run entire workspace test suite (`cargo test`) to ensure zero regressions across all crates
