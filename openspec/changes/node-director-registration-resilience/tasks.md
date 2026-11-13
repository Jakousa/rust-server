# Tasks: Node Registration Resilience

1. [x] Update `NodeRegistry` in `crates/director/src/registry.rs` to track `evicted_nodes: HashSet<u32>` and expose `is_evicted(node_id)`.
2. [x] Update `DirectorServiceImpl::heartbeat` in `crates/director/src/service.rs` to return `FailedPrecondition` only for evicted nodes, and `Ok(HeartbeatResponse { acknowledged: false, .. })` for unknown/non-evicted nodes.
3. [x] Update `test_service_heartbeat` in `crates/director/src/service.rs` and add unit tests for evicted vs unknown node heartbeats.
4. [x] Refactor `crates/node/src/runner.rs` to extract `run_director_registration_and_heartbeat_loop` with startup registration retry loop, `telemetry.mark_ready()` only on confirmed registration, and self-healing re-registration on `acknowledged == false` or reconnect.
5. [x] Add integration tests in `crates/node` verifying:
   - Startup registration retry when Director is initially down, transitioning `/ready` from unready to ready once Director starts.
   - Director restart while node is active: node automatically re-registers and stays ready without halting.
   - Evicted node fencing: `FailedPrecondition` still halts the node.
6. [x] Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`.
