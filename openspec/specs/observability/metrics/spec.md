# observability/metrics Specification

## Purpose
Defines Prometheus metrics collection, recording semantics, and HTTP scrape endpoints across all cluster components to support real-time monitoring and alerting.

## Requirements

### Requirement: Prometheus Metrics HTTP Endpoint
Each server binary (Gateway, Director, Node) SHALL expose an HTTP `GET /metrics` endpoint responding with standard Prometheus exposition format text.

#### Scenario: Prometheus scraper queries metrics endpoint
- **GIVEN** a running server service with the metrics exporter enabled
- **WHEN** an HTTP `GET /metrics` request is received
- **THEN** the server SHALL respond with HTTP status `200 OK`
- **AND** the response body SHALL contain text conforming to the Prometheus exposition format
- **AND** the response Content-Type SHALL indicate text/plain or Prometheus metrics format.

### Requirement: Simulation Node Telemetry Instrumentation
Simulation nodes SHALL record tick loop performance, entity density, and storage queue depth using Prometheus metrics.

#### Scenario: Tick loop records duration and overrun count
- **GIVEN** a running `TickSimulation`
- **WHEN** a 20 Hz simulation tick completes
- **THEN** the simulation SHALL record the tick execution time in `mmo_node_tick_duration_seconds`
- **AND** if the tick execution time exceeds 50 ms, `mmo_node_tick_overruns_total` SHALL be incremented by 1.

#### Scenario: Entity count and AoI gauges updated
- **WHEN** entities are spawned, despawned, or observers registered
- **THEN** `mmo_node_active_entities` and `mmo_node_aoi_observers` SHALL reflect the current counts.

#### Scenario: Database snapshot flush duration recorded
- **WHEN** `WriteBehindSnapshotter` flushes a batch of dirty entity snapshots to the database
- **THEN** the execution latency SHALL be recorded in `mmo_node_snapshot_flush_duration_seconds`
- **AND** the remaining dirty queue count SHALL be recorded in `mmo_node_snapshot_dirty_queue`.

### Requirement: Gateway Network Ingress and Egress Telemetry
The Edge Gateway SHALL record active session count, packet and byte throughput categorized by opcode, and drop counts.

#### Scenario: Packet and byte throughput counted
- **WHEN** a client packet is received or sent
- **THEN** `mmo_gateway_packets_received_total` or `mmo_gateway_packets_sent_total` SHALL increment with the packet's `opcode` label
- **AND** `mmo_gateway_bytes_received_total` or `mmo_gateway_bytes_sent_total` SHALL increment by the frame's byte length.

#### Scenario: Session connected and disconnected
- **WHEN** a client QUIC connection is established or terminated
- **THEN** `mmo_gateway_active_sessions` SHALL be updated to the exact active connection count.

### Requirement: Director Cluster Topology Telemetry
The Director SHALL record cluster topology epoch, active node count, and recovery event counts.

#### Scenario: Topology mutation updates Director metrics
- **WHEN** a partition split, merge, or recovery occurs
- **THEN** `mmo_director_topology_epoch` SHALL reflect the latest epoch number
- **AND** `mmo_director_active_nodes` SHALL reflect the count of healthy registered nodes.
