# Tasks: Live Cloud Validation & Multi-Client Load/Soak Testing (Issue #239)

- [x] **1. OpenSpec 2.0 Change Proposal (`openspec/changes/cloud-load-soak-validation/`)**
  - [x] Scaffold change via `openspec new change "cloud-load-soak-validation"`.
  - [x] Author `proposal.md`, `design.md`, `specs/loadtest/runner/spec.md`, and `tasks.md`.
  - [x] Validate change artifacts with `openspec validate "cloud-load-soak-validation"`.

- [x] **2. Load/Soak Telemetry Enhancements (`crates/loadtest/src/`)**
  - [x] Extend `MetricsAggregator` and `MetricsSnapshot` in `crates/loadtest/src/metrics.rs` with `authenticated_sessions` (`auth_throughput_sps`), `entity_deltas_received`, `route_handovers_observed`, and `total_bytes_received` (`recv_bandwidth_kbps` in kbps).
  - [x] Wire new telemetry counters and jitter-safe tick-reset handover detection (`batch.server_tick.saturating_add(4) < last_observed_tick`) in `crates/loadtest/src/swarm.rs` and `crates/loadtest/src/session.rs`, and print them in `crates/loadtest/src/main.rs`.
  - [x] Add unit tests in `crates/loadtest/src/metrics.rs` and `crates/loadtest/src/swarm.rs` / `session.rs` covering the new counters, rate calculations, and handover jitter tolerance.

- [x] **3. Multi-Node Split, Migration, Failover & Persistence Soak Test (`crates/loadtest/tests/cloud_soak_validation.rs`)**
  - [x] Implement end-to-end soak test covering concurrent bot swarm JWT authentication, 20 Hz movement against `GatewayServer` + multi-node `NodeServiceImpl` (`TickSimulation`), live `execute_batch_entity_migration` with `UpdateBatchRoutes`, `GatewayService::reassign_node_routes` failover, and `WriteBehindSnapshotter` / `CharacterRepository` SQLite persistence integrity.

- [x] **4. Live Cloud Cluster Soak Validation & Baseline Documentation (`docs/CLOUD_POC_DEPLOYMENT.md`)**
  - [x] Inspect live `k3s` cluster pods, services, logs, and Prometheus metrics (read-only) and verify gateway ingress reachability (TCP `:8080`, HTTP/3 WebTransport `/cert-hash` & UDP `:4433`, WebSocket `:8443` status).
  - [x] Execute `mmo-loadtest` soak runs against the live `k3s` cluster gateway (`49.13.149.126:8080`) including `>= 50` bots to trigger/observe cluster multi-node partitioning, tick duration, auth throughput, and RTT percentiles.
  - [x] Document directly observed baseline metrics and ingress findings in `docs/CLOUD_POC_DEPLOYMENT.md` (`## 5. Live Cloud Load & Soak Validation Benchmarks`).

- [x] **5. Verification & Quality Gates**
  - [x] Run `cargo fmt --all -- --check`
  - [x] Run `cargo clippy -p mmo-loadtest --all-targets -- -D warnings`
  - [x] Run `cargo test -p mmo-loadtest`
  - [x] Run `openspec validate "cloud-load-soak-validation"`
