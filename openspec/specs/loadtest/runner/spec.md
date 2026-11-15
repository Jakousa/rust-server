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
The loadtest runner SHALL record round-trip ping latency, packet throughput, authentication throughput, AoI entity deltas received, cross-node route handovers observed, received wire bandwidth, and any movement snap-back correction packets returned by the server.

#### Scenario: Telemetry collection and reporting
- **GIVEN** a configured virtual bot swarm running against a gateway endpoint
- **WHEN** the loadtest simulation runs over a measurement window
- **THEN** the runner SHALL compute and display p50, p95, and p99 latency percentiles alongside packet send/receive rates, `authenticated_sessions` (`auth_throughput_sps`), `entity_deltas_received`, `route_handovers_observed`, `total_bytes_received` (`recv_bandwidth_kbps` in kilobits/sec), total snap-back corrections, and connection/auth failure counts

### Requirement: Tick-Timeline Route Handover Detection Without Jitter False Positives
The loadtest runner SHALL detect cross-node session route handovers when an incoming `EntityDeltaBatch` exhibits a significant backward reset in `server_tick` (`batch.server_tick + 4 < last_observed_tick`), while ignoring minor jitter or out-of-order frames within 4 ticks.

#### Scenario: Server tick timeline resets across node migration
- **GIVEN** a bot session that has previously observed `EntityDeltaBatch` frames at `server_tick = 500`
- **WHEN** the bot migrates to a target node whose `server_tick` is `120` and then continues receiving sequential batches (`121`, `122`)
- **THEN** the runner SHALL increment `route_handovers_observed` once for the migration transition and reset its observed tick cursor to `120` without double-counting subsequent ticks

#### Scenario: Minor packet reordering does not trigger false handover
- **GIVEN** a bot session whose `last_observed_tick` is `500`
- **WHEN** a slightly delayed `EntityDeltaBatch` arrives with `server_tick = 497` (`497 + 4 >= 500`)
- **THEN** the runner SHALL NOT increment `route_handovers_observed` and SHALL preserve `last_observed_tick = 500`

### Requirement: Multi-Node Migration, Failover, and Persistence Soak Verification
The loadtest crate SHALL include an integration test suite verifying concurrent bot swarm authentication and 20 Hz movement across `GatewayServer` and multi-node `TickSimulation` instances under live batch entity migration, gateway failover route reassignment, and asynchronous write-behind persistence snapshotting.

#### Scenario: Concurrent swarm survives live batch migration, route failover, and snapshot flush
- **GIVEN** a `GatewayServer` backed by `EdgeAuthenticator` and `GatewayServiceImpl`, two `NodeServiceImpl` simulation nodes, and a `WriteBehindSnapshotter` connected to SQLite storage
- **WHEN** a concurrent bot swarm authenticates, streams 20 Hz movement, undergoes live `execute_batch_entity_migration` and `GatewayService::reassign_node_routes`, and flushes snapshots
- **THEN** bots SHALL remain connected across route handovers, `SessionRouter` SHALL reflect migrated and failover routes with zero dropped sessions, and `CharacterRepository::get_snapshot` SHALL match authoritative entity positions
