# Node Registration Resilience Specification

## Purpose
Defines resilient startup registration retry, split-brain fencing versus unknown-node heartbeat handling on `mmo-director`, and automatic self-healing re-registration on `mmo-node` across Director restarts.

## Requirements

### Requirement: Startup Registration Retry
The simulation node SHALL retry initial registration with `mmo-director` using bounded RPC timeouts and backoff before marking readiness or starting its heartbeat loop.

#### Scenario: Director unreachable during node startup
- **GIVEN** a simulation node starting up while `mmo-director` is unreachable or returns an error / `success = false` for `RegisterNode`
- **WHEN** `register_node` fails
- **THEN** the node SHALL retry `register_node` with backoff (1s) using bounded RPC timeouts until `RegisterNodeResponse.success == true`, and only then call `telemetry.mark_ready()` and begin the heartbeat loop.

### Requirement: Director Distinction Between Evicted and Unknown Nodes
The Cluster Director SHALL distinguish between explicitly evicted nodes (which must be fenced) and unknown nodes (such as after a Director restart).

#### Scenario: Evicted node heartbeat rejected with FailedPrecondition
- **GIVEN** `mmo-director` with an in-memory `NodeRegistry`
- **WHEN** `check_dead_nodes` evicts a node ID due to heartbeat timeout
- **THEN** that node ID SHALL be recorded in `evicted_nodes` and subsequent `Heartbeat` calls from it (prior to a fresh `RegisterNode`) SHALL return `Status::failed_precondition` (split-brain fencing).

#### Scenario: Unknown node heartbeat returns unacknowledged response
- **GIVEN** `mmo-director` receiving a `Heartbeat` for a `node_id` that is neither in `nodes` nor in `evicted_nodes` (such as after a Director restart)
- **WHEN** `DirectorServiceImpl::heartbeat` processes the request
- **THEN** `mmo-director` SHALL return `Ok(HeartbeatResponse { acknowledged: false, should_split: false, topology_epoch })`.

### Requirement: Node Self-Healing Re-Registration on Heartbeat
Simulation nodes SHALL automatically re-register when the Director reports `acknowledged: false` while still honoring `FailedPrecondition` fencing and optional lease timeouts.

#### Scenario: Unacknowledged heartbeat triggers re-registration
- **GIVEN** an active simulation node in its heartbeat loop
- **WHEN** `Heartbeat` returns `Ok(HeartbeatResponse { acknowledged: false, .. })`
- **THEN** the node SHALL immediately re-invoke `register_node` with its `node_id`, `listen_addr`, and `bounds`, and call `telemetry.mark_ready()` upon `success == true`.

#### Scenario: Evicted node halts on FailedPrecondition
- **GIVEN** an active simulation node in its heartbeat loop
- **WHEN** `Heartbeat` returns `Err(status)` with `Code::FailedPrecondition`
- **THEN** the node SHALL halt simulation via `fenced_tx` (preserving split-brain fencing).

#### Scenario: Transient transport error reconnects without premature re-registration
- **GIVEN** an active simulation node in its heartbeat loop
- **WHEN** `Heartbeat` returns a transient transport error (`Code::Unavailable`, `Code::DeadlineExceeded`, `Code::Unknown`, etc.)
- **THEN** the node SHALL drop the gRPC client and reconnect on subsequent ticks without clobbering runtime bounds via premature `register_node`, and SHALL only re-register if the reconnected Director responds with `acknowledged: false`.

#### Scenario: Lease timeout self-fencing
- **GIVEN** an optional `lease_timeout` is configured on the simulation node
- **WHEN** no heartbeat acknowledgment is received within `lease_timeout`
- **THEN** the node SHALL self-fence via `fenced_tx` to prevent zombie split-brain after prolonged partitions.
