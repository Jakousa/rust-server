# Loadtest Runner Specification

## Purpose

Orchestrates concurrent virtual game clients to stress test cluster throughput, spatial boundary transitions, and latency.

## Requirements

### Requirement: Swarm Spawning and Connection Lifecycle
The loadtest runner SHALL spawn a configurable number of concurrent virtual client connections to the Edge Gateway using the binary client protocol (`mmo-schema::client`) and maintain them with periodic ping messages.

#### Scenario: Swarm initialization and keepalive
- **WHEN** the runner is executed with a target count of N bots
- **THEN** it SHALL establish N asynchronous connections to the gateway and exchange heartbeat ping/pong packets

### Requirement: Synthetic Kinematic Movement and Boundary Traversal
The loadtest runner SHALL compute continuous 3D trajectory updates within configured world coordinates for each simulated bot, steering bots across octant boundaries to trigger node handovers.

#### Scenario: Bot crosses spatial boundary
- **WHEN** an active bot traverses across an octree boundary plane
- **THEN** the bot SHALL continue streaming movement packets to the gateway without connection termination and receive updated entity states

### Requirement: Telemetry and Latency Percentiles
The loadtest runner SHALL record round-trip ping latency, packet throughput, and any movement snap-back correction packets returned by the server.

#### Scenario: Telemetry collection and reporting
- **WHEN** the loadtest simulation runs over a measurement window
- **THEN** the runner SHALL compute and display p50, p95, and p99 latency percentiles alongside total snap-back corrections and packet loss counts
