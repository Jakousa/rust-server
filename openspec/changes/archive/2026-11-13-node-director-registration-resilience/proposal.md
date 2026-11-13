# Proposal: Resilient Node Registration and Director Restart Recovery

## Context
During Kubernetes rolling restarts or deployments where `mmo-director` restarts, simulation nodes (`mmo-node`) can enter a perpetual unready state or halt unnecessarily:
1. In `crates/node/src/runner.rs`, `register_node` is a one-shot call. If it fails or returns `success = false`, `telemetry.mark_ready()` is never called and the node enters the heartbeat loop permanently unready (HTTP 503 on `/ready`), stalling StatefulSet rollouts.
2. In `crates/director/src/service.rs`, `heartbeat` rejects any `node_id` not present in the in-memory `NodeRegistry` with `Status::failed_precondition` (split-brain fencing). When `mmo-director` restarts with an empty in-memory registry, active simulation nodes receive `FailedPrecondition` on their next heartbeat and immediately halt their simulation (`fenced_tx`), instead of re-registering.

## Scope
- **Node Registration Retry (`crates/node/src/runner.rs`)**: Retry `register_node` with backoff until acknowledged (`success == true`) before calling `telemetry.mark_ready()` and entering the heartbeat loop.
- **Director Eviction vs Unknown Distinction (`crates/director/src/registry.rs`, `crates/director/src/service.rs`)**: Track `evicted_nodes: HashSet<u32>` in `NodeRegistry`. Return `FailedPrecondition` only for evicted nodes (preserving split-brain fencing). For unknown/non-evicted nodes (e.g. after Director restart), return `HeartbeatResponse { acknowledged: false, should_split: false, topology_epoch }`.
- **Node Self-Healing Re-Registration (`crates/node/src/runner.rs`)**: On `HeartbeatResponse { acknowledged: false, .. }`, `Status::not_found`, or transient transport/unavailable errors on reconnect, automatically re-invoke `register_node` and call `telemetry.mark_ready()`.
- **Tests**: Unit and integration tests covering startup registration retry, Director restart + node auto-re-registration, and evicted-node split-brain fencing preservation.

## Non-Goals
- Persisting `NodeRegistry` to external storage (Redis/PostgreSQL); Director remains in-memory with fast node re-registration.
- Changing `cluster.proto` wire format (`HeartbeatResponse.acknowledged` already exists).
