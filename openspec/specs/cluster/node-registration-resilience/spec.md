# Specification: Node Registration Resilience

## Purpose

Ensures simulation nodes (`mmo-node`) reliably retry startup registration until acknowledged by `mmo-director`, distinguishes evicted nodes from unknown nodes after Director restarts, and performs self-healing re-registration without compromising split-brain fencing.

## Requirements

### Requirement: Startup Registration Retry
Simulation nodes SHALL retry `RegisterNode` with backoff until acknowledged before marking readiness and entering the heartbeat loop.

#### Scenario: Node retries RegisterNode until acknowledged
- **GIVEN** a simulation node starting up while `mmo-director` is unreachable or returns an error / `success = false` for `RegisterNode`
- **WHEN** `register_node` fails
- **THEN** the node SHALL retry `register_node` with backoff (1s) using bounded RPC timeouts until `RegisterNodeResponse.success == true`, and only then call `telemetry.mark_ready()` and begin the heartbeat loop.

### Requirement: Director Distinction Between Evicted and Unknown Nodes
The Director SHALL distinguish previously evicted nodes (which MUST remain fenced) from unknown non-evicted nodes (such as after a Director restart).

#### Scenario: Evicted node heartbeat rejected with failed_precondition
- **GIVEN** `mmo-director` with an in-memory `NodeRegistry`
- **WHEN** `check_dead_nodes` evicts a node ID due to heartbeat timeout
- **THEN** that node ID SHALL be recorded in `evicted_nodes` and subsequent `Heartbeat` calls from it (prior to a fresh `RegisterNode`) SHALL return `Status::failed_precondition` (split-brain fencing).

#### Scenario: Unknown non-evicted node heartbeat returns unacknowledged response
- **GIVEN** `mmo-director` with an in-memory `NodeRegistry`
- **WHEN** `Heartbeat` is received for a `node_id` that is neither in `nodes` nor in `evicted_nodes` (such as after a Director restart)
- **THEN** `mmo-director` SHALL return `Ok(HeartbeatResponse { acknowledged: false, should_split: false, topology_epoch })`.

### Requirement: Node Self-Healing Re-Registration on Heartbeat
Active simulation nodes SHALL automatically re-register upon receiving an unacknowledged heartbeat while preserving split-brain fencing and lease expiration safeguards.

#### Scenario: Unacknowledged heartbeat triggers immediate re-registration
- **GIVEN** an active simulation node in its heartbeat loop
- **WHEN** `Heartbeat` returns `Ok(HeartbeatResponse { acknowledged: false, .. })`
- **THEN** the node SHALL immediately re-invoke `register_node` with its `node_id`, `listen_addr`, and `bounds`, and call `telemetry.mark_ready()` upon `success == true`.

#### Scenario: FailedPrecondition heartbeat halts simulation
- **GIVEN** an active simulation node in its heartbeat loop
- **WHEN** `Heartbeat` returns `Err(status)` with `Code::FailedPrecondition`
- **THEN** the node SHALL halt simulation via `fenced_tx` (preserving split-brain fencing).

#### Scenario: Transient transport error reconnects without clobbering bounds
- **GIVEN** an active simulation node in its heartbeat loop
- **WHEN** `Heartbeat` returns a transient transport error (`Code::Unavailable`, `Code::DeadlineExceeded`, `Code::Unknown`, etc.)
- **THEN** the node SHALL drop the gRPC client and reconnect on subsequent ticks without clobbering runtime bounds via premature `register_node`, and SHALL only re-register if the reconnected Director responds with `acknowledged: false`.

#### Scenario: Optional lease timeout triggers self-fencing
- **GIVEN** an active simulation node with an optional `lease_timeout` configured
- **WHEN** no heartbeat acknowledgment is received within `lease_timeout`
- **THEN** the node SHALL self-fence via `fenced_tx` to prevent zombie split-brain after prolonged partitions.
