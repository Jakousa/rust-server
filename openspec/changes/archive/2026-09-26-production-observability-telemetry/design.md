# Design: Production Observability, Prometheus Metrics & Health Telemetry

## Context and Architectural Goals

In distributed gaming backends, observing simulation loop determinism, client network throughput, and spatial partition state in real time is vital. Under spike loads or node failure, metrics guide auto-scaling, alerting, and failure recovery.

The goal of this design is to provide:
1. A zero-boilerplate, unified HTTP telemetry server in `crates/core` hosting `/metrics`, `/healthz`, and `/ready`.
2. Standardized Prometheus instrumentation across Gateway, Director, and Simulation Nodes.
3. Clean integration with Kubernetes liveness and readiness probing and Prometheus pod scraping on port `9091`.

## Architectural Design

```
   [ Prometheus Server (k3d) ] ── (Scrapes port 9091 on pods via K8s SD)
                │
                ├──────────────────────┬──────────────────────┐
                ▼                      ▼                      ▼
       [ Director Pod ]         [ Node Pods ]         [ Gateway Pod ]
       - gRPC: 50051            - gRPC: 50052         - QUIC: 8080/udp
       - HTTP: 9091             - HTTP: 9091          - HTTP: 9091
         ├── /metrics             ├── /metrics          ├── /metrics
         ├── /healthz             ├── /healthz          ├── /healthz
         └── /ready               └── /ready            └── /ready
```

### 1. Unified `TelemetryServer` (`crates/core/src/telemetry.rs`)

The `TelemetryServer` encapsulates:
- A `PrometheusHandle` registered via `metrics-exporter-prometheus`.
  - **Test Concurrency Isolation**: In Rust unit test harnesses (`cargo test`), tests run in parallel within a single process. Since `install_recorder()` can only be registered once globally, `TelemetryServer::init()` caches the initial `PrometheusHandle` in a `static RECORDER_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new()`. Subsequent calls in tests return a clone of the cached handle without panicking.
- An `Arc<AtomicBool>` tracking service readiness.
- An `axum` HTTP server serving three lightweight endpoints:
  - `GET /metrics`: Renders the Prometheus text exposition buffer via `handle.render()`.
  - `GET /healthz`: Returns `StatusCode::OK` with body `"OK"`.
  - `GET /ready`: Returns `StatusCode::OK` with body `"READY"` if ready, or `StatusCode::SERVICE_UNAVAILABLE` with `"NOT_READY"` if unready.

```rust
pub struct TelemetryServer {
    handle: PrometheusHandle,
    ready: Arc<AtomicBool>,
}

impl TelemetryServer {
    pub fn init() -> Result<Self, TelemetryError>;
    pub fn mark_ready(&self);
    pub fn mark_unready(&self);
    pub fn is_ready(&self) -> bool;
    pub fn start(&self, addr: SocketAddr) -> tokio::task::JoinHandle<()>;
}
```

### 2. Standardized Metric Catalog

All metrics adhere to the Prometheus and OpenMetrics naming guidelines:

| Service | Metric Name | Type | Labels | Description |
|---|---|---|---|---|
| **Node** | `mmo_node_tick_duration_seconds` | Histogram | `node_id` | Execution duration of each 20 Hz simulation tick |
| **Node** | `mmo_node_tick_overruns_total` | Counter | `node_id` | Count of ticks exceeding the 50 ms budget |
| **Node** | `mmo_node_active_entities` | Gauge | `node_id` | Total entities simulated in local world |
| **Node** | `mmo_node_aoi_observers` | Gauge | `node_id` | Total AoI observers registered |
| **Node** | `mmo_node_snapshot_dirty_queue` | Gauge | `node_id` | Current entities queued in snapshotter |
| **Node** | `mmo_node_snapshot_flush_duration_seconds` | Histogram | `node_id` | Duration of database batch snapshot flush execution |
| **Gateway** | `mmo_gateway_active_sessions` | Gauge | — | Total concurrent active QUIC client sessions |
| **Gateway** | `mmo_gateway_packets_received_total` | Counter | `opcode` | Ingress packets received from clients |
| **Gateway** | `mmo_gateway_packets_sent_total` | Counter | `opcode` | Egress packets forwarded or sent to clients |
| **Gateway** | `mmo_gateway_bytes_received_total` | Counter | — | Ingress payload bandwidth bytes received from clients |
| **Gateway** | `mmo_gateway_bytes_sent_total` | Counter | — | Egress payload bandwidth bytes sent to clients |
| **Gateway** | `mmo_gateway_packets_dropped_total` | Counter | `reason` | Packets dropped due to buffer overflow or route error |
| **Director** | `mmo_director_active_nodes` | Gauge | — | Count of currently active registered simulation nodes |
| **Director** | `mmo_director_topology_epoch` | Gauge | — | Generational spatial partition topology epoch |
| **Director** | `mmo_director_partition_recoveries_total` | Counter | — | Total orphaned partitions recovered from dead nodes |

### 3. Kubernetes Pod Discovery Configuration

Because each pod in Kubernetes receives an isolated network namespace and IP address:
- Every service (Director, Node, Gateway) uses uniform environment variable `ADMIN_ADDR` defaulting to `0.0.0.0:9091`.
- In `k8s/base/`:
  - `annotations`:
    - `prometheus.io/scrape: "true"`
    - `prometheus.io/port: "9091"`
  - `livenessProbe`: `httpGet` path `/healthz`, port `9091`.
  - `readinessProbe`: `httpGet` path `/ready`, port `9091`.
- Prometheus automatically discovers all running pods via `kubernetes_sd_configs: [role: pod]` and scrapes `/metrics` at 2-second intervals.
