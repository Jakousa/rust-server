# Design: Resilient Node Registration and Director Restart Recovery

## Architecture Overview

### 1. `crates/director/src/registry.rs`
Add `evicted_nodes: HashSet<u32>` to `NodeRegistry`:
- `check_dead_nodes(&mut self, timeout: Duration) -> Vec<u32>`: collects `dead_nodes` during `self.nodes.retain(...)`, then iterates `dead_nodes` to insert each ID into `self.evicted_nodes` (borrow-checker safe).
- `register_node` / `register_node_with_id`: removes `node_id` from `self.evicted_nodes` when a node (re-)registers.
- `is_evicted(&self, node_id: u32) -> bool`: returns `self.evicted_nodes.contains(&node_id)`.

### 2. `crates/director/src/service.rs`
In `DirectorServiceImpl::heartbeat`:
```rust
if acknowledged {
    let should_split =
        coord.evaluate_heartbeat(req.node_id, req.entity_count, req.tick_duration_ms);
    Ok(Response::new(HeartbeatResponse {
        acknowledged: true,
        should_split,
        topology_epoch,
    }))
} else if reg.is_evicted(req.node_id) {
    metrics::counter!("mmo_director_node_fenced_total").increment(1);
    Err(Status::failed_precondition(format!(
        "Node {} is evicted from director topology (node fenced to prevent split-brain simulation)",
        req.node_id
    )))
} else {
    metrics::counter!("mmo_director_node_reregister_required_total").increment(1);
    Ok(Response::new(HeartbeatResponse {
        acknowledged: false,
        should_split: false,
        topology_epoch,
    }))
}
```
Lock order (`instance_registry` -> `registry` -> `coordinator`) is strictly preserved.

### 3. `crates/node/src/runner.rs`
Extract a testable async helper `run_director_registration_and_heartbeat_loop(...)` with parameters:
- `director_endpoint`, `node_id`, `listen_addr`, `bounds`, `sim`, `telemetry`, `fenced_tx`, `retry_interval`, `heartbeat_interval`, `rpc_timeout`, `lease_timeout: Option<Duration>`.

State machine refinements (incorporating architectural review):
1. **Decouple Reconnect from Re-registration**:
   - `ensure_connected(&mut client, &director_endpoint, rpc_timeout)`: connects `DirectorServiceClient` only if `client.is_none()`.
   - `register_until_acknowledged(...)`: called at startup (before entering the heartbeat loop) and ONLY when `heartbeat` explicitly returns `Ok(HeartbeatResponse { acknowledged: false, .. })`. Transient transport errors (`Unavailable`, timeout) only drop `client = None` so the next tick reconnects and sends a `HeartbeatRequest`—avoiding clobbering runtime split bounds in Director's registry.
2. **Bounded RPC Timeouts & Interval Behavior**:
   - Configure `hb_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay)` to avoid burst storms after reconnect.
   - Wrap `connect`, `register_node`, and `heartbeat` calls in `tokio::time::timeout(rpc_timeout, ...)` (default 2s).
3. **Optional Lease Expiry Self-Fencing**:
   - Track `last_ack: Instant` updated on initial registration and on each `HeartbeatResponse { acknowledged: true, .. }`.
   - If `lease_timeout` is `Some(limit)` (configurable via `DIRECTOR_LEASE_TIMEOUT_MS`, e.g. 30s—longer than typical Director rolling restarts, while bounding prolonged zombie partitions) and `last_ack.elapsed() > limit`, send to `fenced_tx` and `break`.
   - Note: On Director restart, `topology_epoch` resets to 1 in Director's in-memory `GlobalOctreeCoordinator`; nodes accept the new epoch upon re-registration.
