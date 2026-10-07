# Tasks: Lag Compensation, Server-Authoritative History Buffers & Client Prediction Reconciliation

## Phase 1: Protocol Schema & Wire Codecs (`mmo-schema`)
- [x] 1.1 In `crates/schema/src/client.rs`, implement `LagCompensationPayload` with binary encoder/decoder, server-synchronized timestamp documentation, length validation, and NaN/infinity float guards. *(Eligible for local subagent delegation)*
- [x] 1.2 In `crates/schema/src/client.rs`, implement `MovementCorrectionReason` enum and extend `MovementCorrectionPacket` docs/helpers. *(Eligible for local subagent delegation)*
- [x] 1.3 Add unit and roundtrip property tests in `crates/schema/tests/` validating `LagCompensationPayload` encoding, buffer bounds, and error handling. *(Eligible for local subagent delegation)*

## Phase 2: Server-Side Tick History Ring Buffer (`mmo-node`)
- [x] 2.1 In `crates/node/src/history.rs`, implement `HistoricalEntityRecord`, `HistoricalTickFrame`, and `SpatialHistoryBuffer` with fixed-capacity ring buffer and slab reuse to achieve zero per-tick allocations.
- [x] 2.2 In `crates/node/src/history.rs`, implement frame insertion, oldest-frame eviction, frame lookup by tick or monotonic timestamp, and interpolation between bounding tick frames.
- [x] 2.3 Integrate `SpatialHistoryBuffer` into `TickSimulation` (`crates/node/src/sim.rs`), recording active entities at the conclusion of every simulation step.
- [x] 2.4 Write comprehensive unit tests in `crates/node` validating ring buffer wrap-around, zero-allocation memory reuse, and frame timestamp indexing. *(Eligible for local subagent delegation)*

## Phase 3: Authoritative Lag-Compensated Raycasts & Anti-Cheat Clamping (`mmo-node`)
- [x] 3.1 In `crates/node/src/history.rs`, implement anti-cheat rewind window clamping enforcing $t \le t_{\text{now}}$ and $t_{\text{rewind}} \le 250\text{ms}$ with metric increments.
- [x] 3.2 In `crates/node/src/history.rs`, implement `raycast_historical` with direction vector re-normalization, degenerate vector rejection (< 1e-6 or non-finite), slab-based ray-AABB intersections against reconstructed entity bounds, and document despawned entity handling.
- [x] 3.3 Expose `raycast_historical` and `validate_historical_range` on `TickSimulation` with metric telemetry counters and minimal read-lock scopes to avoid tick write starvation.
- [x] 3.4 Write unit tests in `crates/node` validating anti-cheat rewind clamping (rejecting future timestamps, clamping >250ms rewinds, oldest-frame clamping), direction re-normalization, and lag-compensated hit registration against moving entities. *(Eligible for local subagent delegation)*

## Phase 4: Client-Side Prediction & Reconciliation Replay (`mmo-client-wasm`)
- [x] 4.1 In `crates/client-wasm/src/prediction.rs`, implement `ClientInput`, `PredictionBuffer`, and unacknowledged input FIFO queue.
- [x] 4.2 In `crates/client-wasm/src/prediction.rs`, implement reconciliation algorithm: input eviction up to sequence ack, divergence check ($\epsilon = 0.05\text{m}$), input replay forward from server authoritative position, and exponential visual offset decay.
- [x] 4.3 Integrate `PredictionBuffer` into `WasmClientWorld` in `crates/client-wasm/src/lib.rs`: wire input generation in `encode_movement`, server reconciliation on `MovementCorrection` and local player delta updates, and visual smoothing in `update(dt)`.
- [x] 4.4 Write unit tests in `crates/client-wasm` verifying input queue FIFO behavior, sub-tolerance convergence avoidance, divergence input replay, and hard snapback. *(Eligible for local subagent delegation)*

## Phase 5: Verification & Quality Gates
- [x] 5.1 Execute formatting verification: `cargo fmt --all -- --check`.
- [x] 5.2 Execute strict linting verification: `cargo clippy --workspace --all-targets -- -D warnings`.
- [x] 5.3 Execute workspace test suite: `cargo test --workspace`.
- [ ] 5.4 Push branch `feat/lag-compensation-history-buffer`, open pull request, and enable auto-merge.
