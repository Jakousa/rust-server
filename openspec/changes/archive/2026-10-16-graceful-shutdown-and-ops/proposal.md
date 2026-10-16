# Graceful Shutdown Draining, Dynamic Gateway Discovery & Tracing Proposal

## Context & Problem Statement
Operational audit of cluster services identified three critical operational and high-availability vulnerabilities (Issue #84):

1. **Abrupt Daemon Termination & Data Loss Risk**:
   None of the server daemons (`mmo-gateway`, `mmo-node`, `mmo-director`) intercept `SIGTERM` or `SIGINT` signals. When running under Kubernetes or Docker during rolling updates, pod evictions, or node maintenance, the processes receive `SIGTERM` and are abruptly terminated.
   - In `mmo-node`, in-flight simulation state and dirty entity snapshot batches queued in the `WriteBehindSnapshotter` are never flushed to Postgres, risking rollback of character progress, inventory, or position. Furthermore, the node fails to cleanly deregister from the Director orchestrator.
   - In `mmo-gateway`, active TCP/QUIC client sessions are cut off abruptly without disconnection notifications, causing client connection storms, while downstream gRPC handlers are abruptly terminated mid-flight.
   - In `mmo-director`, tonic gRPC server does not use graceful shutdown, terminating in-flight registration or RPC requests abruptly.

2. **Hardcoded Gateway Discovery in Director Recovery**:
   In `crates/director/src/main.rs`, the background dead node detection loop only notifies a single hardcoded gateway read from the `GATEWAY_GRPC_ADDR` environment variable. The director's `gateway_registry` tracks all active registered gateways (`Arc<RwLock<GatewayRegistry>>`), but route reassignments upon dead node recovery are not broadcast across all active gateways, leading to stale gateway routing tables and dropped client packets in multi-gateway deployments.

3. **Missing Distributed Tracing Context Across Boundaries**:
   While Prometheus metrics are exposed on port 9091, there is no structured tracing context propagation across client-to-gateway and gateway-to-node/director interactions, making it difficult to trace packet propagation delays, AoI culling timings, and cross-partition migrations.

## Proposed Solution

1. **Structured Shutdown Signals & Draining**:
   - Provide a shared, cross-platform signal listener `mmo_core::shutdown_signal()` awaiting `SIGTERM` or `SIGINT` (Ctrl+C).
   - In `mmo-gateway`:
     - Upon shutdown signal, immediately transition `telemetry.mark_not_ready()` to signal Kubernetes readiness probe to stop routing incoming traffic.
     - Stop accepting new TCP connections.
     - Broadcast server shutdown notification / close `SessionSink`s to existing client connections.
     - Allow a bounded drain grace period (e.g., 3-5 seconds) for in-flight requests to complete before terminating.
   - In `mmo-node`:
     - Upon shutdown signal, transition `telemetry.mark_not_ready()`.
     - Stop simulation tick loop.
     - Trigger `snapshotter.shutdown()`, await background worker completion, and execute a final synchronous `flush_batch()` to persist all queued dirty snapshots to the database.
     - Deregister the node from `DirectorService` via `DeregisterNode` RPC or notify Director of clean shutdown.
     - Gracefully shutdown gRPC `NodeServiceServer` using `serve_with_shutdown`.
   - In `mmo-director`:
     - Transition `telemetry.mark_not_ready()`.
     - Gracefully shutdown `DirectorServiceServer` using `serve_with_shutdown`.

2. **Dynamic Gateway Discovery in Director Recovery**:
   - Update `crates/director/src/main.rs` dead node detection loop to read active gateways from `svc_clone.gateway_registry`.
   - Broadcast `ReassignNodeRoutesRequest` concurrently across all registered active gateways.
   - Fall back to `GATEWAY_GRPC_ADDR` if no dynamic gateways are registered.

3. **Structured Tracing Spans & gRPC Metadata Context**:
   - Wire `tracing` instrumentation across `mmo-gateway` packet ingestion, session routing, and `mmo-node` packet dispatch.
   - Trace spans for packet lifecycle: ingress -> route resolution -> gRPC forward -> simulation execution.

## Scope & Non-Goals
- **In Scope**:
  - `crates/core`: `shutdown_signal()` helper and readiness coordination.
  - `crates/schema`: `DeregisterNode` and `DeregisterGateway` RPCs in `cluster.proto`.
  - `crates/director`: Dynamic gateway broadcast for route reassignments, `DeregisterNode`/`DeregisterGateway` RPC handlers, graceful shutdown for `DirectorServiceServer`.
  - `crates/node`: Graceful shutdown signal integration, snapshot worker draining & final flush, clean deregistration call to Director, graceful gRPC shutdown.
  - `crates/gateway`: Graceful shutdown signal, readiness probe failover, listener termination, session draining, clean deregistration call to Director.
- **Non-Goals**:
  - Replacing the core Prometheus metrics exporter.
  - Modifying the underlying 2PC distributed transaction commit algorithm.
