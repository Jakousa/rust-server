# Proposal: Live Cloud Validation & Multi-Client Load/Soak Testing (Issue #239)

## Why
Issue #239 calls for validating the distributed MMO engine under sustained multi-client load in both deterministic multi-node integration suites and the live Kubernetes (`k3s`) cloud deployment:
1. `mmo-loadtest` currently tracks packet counts, snapbacks, connection/auth errors, and RTT percentiles, but lacks soak telemetry for session authentication throughput (`sessions/sec`), AoI entity delta throughput, received wire bandwidth (`kbps`), and cross-node route handovers observed via server tick timeline transitions.
2. There is no end-to-end soak test in `crates/loadtest/tests/` combining a concurrent authenticating 20 Hz bot swarm across `GatewayServer` and multi-node `TickSimulation` instances with live `execute_batch_entity_migration`, failover route reassignment (`GatewayService::reassign_node_routes`), and durable snapshot verification (`WriteBehindSnapshotter` + `CharacterRepository`).
3. The live cloud `k3s` cluster lacks documented baseline load/soak benchmarks covering gateway TCP/HTTP/3 WebTransport/WebSocket ingress reachability, multi-node spatial partitioning under `>= 50` concurrent bots, tick budget stability, and RTT percentiles.

## What Changes
- **Loadtest Telemetry Extensions (`crates/loadtest`)**:
  - Extend `MetricsAggregator` and `MetricsSnapshot` (`crates/loadtest/src/metrics.rs`) with `authenticated_sessions` (`auth_throughput_sps`), `entity_deltas_received`, `route_handovers_observed`, and `total_bytes_received` (`recv_bandwidth_kbps` in kilobits per second).
  - Wire authentication, incoming byte counting, entity delta batch counting, and tick-reset route handover detection (`batch.server_tick.saturating_add(4) < last_observed_tick`) in `crates/loadtest/src/swarm.rs` and `crates/loadtest/src/session.rs`, and print the extended report in `crates/loadtest/src/main.rs`.
- **Multi-Node Migration, Failover & Persistence Soak Integration Test (`crates/loadtest/tests/cloud_soak_validation.rs`)**:
  - Exercise concurrent bot swarm authentication and 20 Hz kinematic movement against a live `GatewayServer` and two `NodeServiceImpl` / `TickSimulation` nodes.
  - Validate live batch entity migration (`execute_batch_entity_migration`) with Gateway `SessionRouter` route updates while bots remain connected and streaming movement, followed by node failover (`GatewayService::reassign_node_routes`) and SQLite snapshot durability (`WriteBehindSnapshotter` + `CharacterRepository`).
- **Live Cloud Soak Validation & Benchmark Documentation (`docs/CLOUD_POC_DEPLOYMENT.md`)**:
  - Probe gateway ingress endpoints (TCP `:8080`, HTTP/3 WebTransport `/cert-hash` discovery + `:4433`, WebSocket `:8443` reachability) and execute live `mmo-loadtest` runs against the running `k3s` cluster, capturing directly observed baseline metrics, Prometheus telemetry, and multi-node split/migration logs into `docs/CLOUD_POC_DEPLOYMENT.md`.

## Impact
- Provides quantitative soak-testing visibility for multi-node elastic splitting, route handover stability, and gateway throughput in any genre-agnostic virtual world deployment.
- Keeps all core engine crates untouched (`mmo-loadtest` and documentation only) with zero genre-specific assumptions.
