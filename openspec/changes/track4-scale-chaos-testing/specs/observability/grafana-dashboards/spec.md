# Production Grafana Dashboards Specification

## Purpose
Specifies automated provisioning of pre-configured Grafana dashboards visualizing real-time cluster health, tick latency distributions, entity density, and gateway throughput.

## Requirements

### Requirement: Automated Dashboard Provisioning
The local Kubernetes overlay in `k8s/overlays/local/grafana.yaml` MUST configure a Grafana dashboard provider ConfigMap that automatically mounts and loads dashboard JSON definitions upon container startup without requiring manual UI configuration.

### Requirement: Critical Visual Metrics Panels
The dashboard MUST include panels for:
1. **Simulation Tick Budget**:
   - 20 Hz tick duration p50, p95, and p99 percentiles (threshold marked at $45\text{ ms}$).
   - Metrics source: `mmo_node_tick_duration_seconds`.
2. **Active Sessions & CCU**:
   - Total connected client sessions on Gateway: `mmo_gateway_active_sessions`.
   - Active entity count per simulation node: `mmo_node_active_entities`.
3. **Gateway Packet Rates & Buffer Drops**:
   - Packets received per second: `rate(mmo_gateway_packets_received_total[1m])`.
   - Packets sent per second: `rate(mmo_gateway_packets_sent_total[1m])`.
   - Buffer drops: `rate(mmo_gateway_packets_dropped_total[1m])`.
4. **Storage Persistence**:
   - Snapshot batch flush duration and snapshot queue size: `mmo_snapshotter_queue_len`.

#### Scenario: Real-Time Dashboard Rendering
- **GIVEN** Grafana pod running at `http://localhost:3000`
- **WHEN** user opens Grafana (credentials `admin:admin`)
- **THEN** the "MMO Cluster Overview" dashboard displays live Prometheus metric graphs.
