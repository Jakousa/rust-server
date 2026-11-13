# Specification: Node Registration Resilience

## Requirements

### 1. Startup Registration Retry
- **GIVEN** a simulation node starting up while `mmo-director` is unreachable or returns an error / `success = false` for `RegisterNode`
- **WHEN** `register_node` fails
- **THEN** the node SHALL retry `register_node` with backoff (1s) using bounded RPC timeouts until `RegisterNodeResponse.success == true`, and only then call `telemetry.mark_ready()` and begin the heartbeat loop.

### 2. Director Distinction Between Evicted and Unknown Nodes
- **GIVEN** `mmo-director` with an in-memory `NodeRegistry`
- **WHEN** `check_dead_nodes` evicts a node ID due to heartbeat timeout
- **THEN** that node ID SHALL be recorded in `evicted_nodes` and subsequent `Heartbeat` calls from it (prior to a fresh `RegisterNode`) SHALL return `Status::failed_precondition` (split-brain fencing).
- **WHEN** `Heartbeat` is received for a `node_id` that is neither in `nodes` nor in `evicted_nodes` (such as after a Director restart)
- **THEN** `mmo-director` SHALL return `Ok(HeartbeatResponse { acknowledged: false, should_split: false, topology_epoch })`.

### 3. Node Self-Healing Re-Registration on Heartbeat
- **GIVEN** an active simulation node in its heartbeat loop
- **WHEN** `Heartbeat` returns `Ok(HeartbeatResponse { acknowledged: false, .. })`
- **THEN** the node SHALL immediately re-invoke `register_node` with its `node_id`, `listen_addr`, and `bounds`, and call `telemetry.mark_ready()` upon `success == true`.
- **WHEN** `Heartbeat` returns `Err(status)` with `Code::FailedPrecondition`
- **THEN** the node SHALL halt simulation via `fenced_tx` (preserving split-brain fencing).
- **WHEN** `Heartbeat` returns a transient transport error (`Code::Unavailable`, `Code::DeadlineExceeded`, `Code::Unknown`, etc.)
- **THEN** the node SHALL drop the gRPC client and reconnect on subsequent ticks without clobbering runtime bounds via premature `register_node`, and SHALL only re-register if the reconnected Director responds with `acknowledged: false`.
- **WHEN** an optional `lease_timeout` is configured and no heartbeat acknowledgment is received within `lease_timeout`
- **THEN** the node SHALL self-fence via `fenced_tx` to prevent zombie split-brain after prolonged partitions.
