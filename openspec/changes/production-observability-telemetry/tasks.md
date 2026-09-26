## 1. Unified Telemetry Server in `crates/core`

- [x] 1.1 Add `metrics`, `metrics-exporter-prometheus`, and `axum` dependencies to `crates/core/Cargo.toml`.
- [x] 1.2 Implement `TelemetryServer` in `crates/core/src/telemetry.rs` using a `std::sync::OnceLock<PrometheusHandle>` for safe global recorder test isolation, providing `/metrics`, `/healthz`, and readiness toggling on `/ready`, exporting from `crates/core/src/lib.rs`.
- [x] 1.3 Add unit tests in `crates/core/src/telemetry.rs` verifying Prometheus exposition output, liveness 200 responses, and readiness state transitions (503 -> 200).

## 2. Simulation Node Telemetry Instrumentation

- [x] 2.1 Add `ADMIN_ADDR` configuration and initialize `TelemetryServer` in `crates/node/src/main.rs`, marking ready once initialization finishes.
- [x] 2.2 Instrument `TickSimulation` in `crates/node/src/sim.rs` with `mmo_node_tick_duration_seconds` (histogram), `mmo_node_tick_overruns_total` (counter), `mmo_node_active_entities` (gauge), and `mmo_node_aoi_observers` (gauge).
- [x] 2.3 Instrument `WriteBehindSnapshotter` in `crates/node/src/snapshotter.rs` with `mmo_node_snapshot_dirty_queue` (gauge) and `mmo_node_snapshot_flush_duration_seconds` (histogram).
- [x] 2.4 Verify node telemetry with unit tests in `crates/node/src/sim.rs` and `crates/node/src/snapshotter.rs`.

## 3. Edge Gateway Telemetry Instrumentation

- [x] 3.1 Add `ADMIN_ADDR` configuration and initialize `TelemetryServer` in `crates/gateway/src/main.rs`.
- [x] 3.2 Instrument `GatewayServer` in `crates/gateway/src/server.rs` with `mmo_gateway_active_sessions` (gauge), `mmo_gateway_packets_received_total` / `mmo_gateway_packets_sent_total` (counters with `opcode` label), `mmo_gateway_bytes_received_total` / `mmo_gateway_bytes_sent_total` (byte bandwidth counters), and `mmo_gateway_packets_dropped_total` (counter).
- [x] 3.3 Verify gateway telemetry with unit tests in `crates/gateway/src/server.rs`.

## 4. Director Orchestrator Telemetry Instrumentation

- [x] 4.1 Add `ADMIN_ADDR` configuration and initialize `TelemetryServer` in `crates/director/src/main.rs`.
- [x] 4.2 Instrument `DirectorServiceImpl` in `crates/director/src/service.rs` with `mmo_director_active_nodes` (gauge), `mmo_director_topology_epoch` (gauge), and `mmo_director_partition_recoveries_total` (counter).
- [x] 4.3 Verify director telemetry with unit tests in `crates/director/src/service.rs`.

## 5. Kubernetes Probes & Local Cluster Deployment

- [x] 5.1 Update Kubernetes manifests in `k8s/base/` with `ADMIN_ADDR` (port 9091), `livenessProbe` (`/healthz`), `readinessProbe` (`/ready`), and `prometheus.io/port: "9091"` annotations.
- [x] 5.2 Build container image, deploy to local `k3d` cluster via `./scripts/cluster.sh all`, and verify Prometheus (`http://localhost:9090`) discovers and scrapes all pods with `UP` health state.
- [x] 5.3 Run full workspace verification: `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`.
