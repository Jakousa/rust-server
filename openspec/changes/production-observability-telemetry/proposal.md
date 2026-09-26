# Proposal: Production Observability, Prometheus Metrics & Health Telemetry

## Why

As the distributed MMO cluster transitions toward multi-node orchestration in Kubernetes, real-time observability into engine health, simulation determinism, network throughput, and spatial load distribution is essential:
1. **Simulation Tick Health Blindspots**: Simulation nodes currently execute their 20 Hz fixed-step loops with console logging only. Under heavy entity density or combat calculations, operators have no metrics to measure tick execution duration histograms, p99 overrun counts, catch-up frequency, or active AoI observer load.
2. **Gateway Ingress & Session Tracking**: The Edge Gateway handles high-throughput QUIC datagrams, but lacks telemetry on active concurrent sessions (CCU), ingress/egress packet rates by opcode, route cache hit/miss rates, and packet drop counts.
3. **Director Topology & Cluster State**: The Director manages spatial subdivision and dead-node partition recovery, but cluster topology epoch, active node count, and heartbeat health are not observable via standard monitoring tools.
4. **Kubernetes Health & Readiness Probing**: In containerized environments, Kubernetes requires distinct liveness (`/healthz`) and readiness (`/ready`) HTTP probes to avoid routing traffic to uninitialized nodes or premature pod restarts during heavy load.

Implementing standardized Prometheus metrics scraping and HTTP health probes across all services will enable full operational visibility, proactive alerting, and real-time dashboard monitoring in Grafana.

## What Changes

- **Unified Admin HTTP & Metrics Server**:
  - Implement a lightweight, reusable HTTP telemetry server (in `mmo-core` or dedicated telemetry module) using `axum` and `metrics-exporter-prometheus`.
  - Expose `/metrics` returning standard Prometheus exposition format.
  - Expose `/healthz` returning `200 OK` for container liveness.
  - Expose `/ready` returning `200 OK` when the service is fully initialized and operational, or `503 Service Unavailable` during startup.
- **Simulation Node Instrumentation**:
  - Instrument `TickSimulation`:
    - `mmo_node_tick_duration_seconds` (histogram): execution time per 50 ms tick.
    - `mmo_node_tick_overruns_total` (counter): ticks exceeding the 50 ms budget.
    - `mmo_node_active_entities` (gauge): total active entities in the simulation world.
    - `mmo_node_aoi_observers` (gauge): total active AoI registered observers.
    - `mmo_node_snapshot_dirty_queue` (gauge): queued entities awaiting database snapshot flush.
- **Edge Gateway Instrumentation**:
  - Instrument `GatewayServer`:
    - `mmo_gateway_active_sessions` (gauge): current active QUIC sessions.
    - `mmo_gateway_packets_received_total` (counter, labeled by `opcode`): ingress packet throughput.
    - `mmo_gateway_packets_sent_total` (counter, labeled by `opcode`): egress packet throughput.
    - `mmo_gateway_packets_dropped_total` (counter, labeled by `reason`): rate-limited or unroutable packets.
- **Director Orchestrator Instrumentation**:
  - Instrument `DirectorServiceImpl`:
    - `mmo_director_active_nodes` (gauge): count of currently registered, healthy simulation nodes.
    - `mmo_director_topology_epoch` (gauge): current generational spatial partition epoch.
    - `mmo_director_partition_recoveries_total` (counter): total recovered orphaned partitions.
- **Kubernetes Manifest Updates**:
  - Update `k8s/base/` manifests with correct admin ports, liveness/readiness probes, and Prometheus scraping annotations.

## Capabilities

### New Capabilities
- `observability/metrics`: Standardized Prometheus metrics instrumentation, recording, and HTTP exposition across Gateway, Director, and Simulation Node services.
- `observability/health`: Standardized HTTP liveness (`/healthz`) and readiness (`/ready`) probes for container orchestration.

## Impact

- `crates/core`: Add `telemetry` module exporting `TelemetryServer` with `/metrics`, `/healthz`, `/ready`.
- `crates/node`: Start telemetry listener, instrument tick loop, AoI manager, and snapshotter.
- `crates/gateway`: Start telemetry listener, instrument session tracker and packet dispatcher.
- `crates/director`: Start telemetry listener, instrument node registry and partition coordinator.
- `k8s/`: Configure liveness/readiness probes and verify Prometheus scrape targets.
