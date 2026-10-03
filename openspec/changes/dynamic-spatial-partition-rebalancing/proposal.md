# Proposal: Dynamic Spatial Partition Rebalancing, Hotspot Mitigation & Zero-Disconnect Batch Handover

## 1. Problem Statement & Motivation

In an authoritative distributed virtual world, player distribution is non-uniform. Spatial clusters naturally form during world events, boss encounters, and central hub gatherings (e.g., hundreds of players congregating within an area of a few hundred square meters). Under static spatial octree partitions, the single simulation node responsible for that spatial octant experiences severe CPU saturation and tick degradation (> 35ms tick time, degrading 20 Hz simulation), while neighboring nodes remain essentially idle.

Furthermore, when an overloaded partition subdivides into smaller spatial sub-octants assigned to scale-out worker nodes, transferring hundreds of player sessions individually one-by-one via separate single-entity migrations causes:
1. Head-of-line RPC delays and route update races at the Edge Gateway.
2. High simulation locking overhead across repeated write lock cycles.
3. Temporary visibility blackouts or desynchronizations for clustered players.

## 2. Goals

1. **Continuous Saturation & Hotspot Telemetry**: Extend node heartbeat telemetry and the Cluster Director to monitor both entity density (> 500 entities) and moving-average tick duration (> 35ms) to trigger automated sub-domain splitting.
2. **Sub-Domain Splitting & Provisioning**: Automatically subdivide saturated leaf octants into sub-octants, distributing them across least-loaded or newly provisioned simulation nodes based on composite load metrics (entity count, CPU %, tick duration).
3. **Continuous Boundary Smoothing**: Provide an elastic centroid-repulsion algorithm in the director that dynamically shifts partition boundaries between adjacent nodes to prevent sudden boundary thrashing and smooth the entity density gradient.
4. **Zero-Disconnect Batch Handover**: Implement high-throughput batch migration primitives (`BatchMigrateEntity` and `BatchRouteUpdate`) capable of atomically transferring up to 500 entities across simulation nodes in a single gRPC round-trip with zero duplication and strict rollback guarantees.

## 3. Non-Goals

1. Dynamic auto-scaling of Kubernetes Pods (HPA) itself — the director assigns work to registered online nodes; external cluster autoscalers manage pod counts.
2. Re-architecting the 20 Hz ECS tick loop or physics kinematics model.
3. Modifying the WebAssembly client rendering pipeline.

## 4. Architectural Impact

- **`proto/cluster.proto`**: Added `tick_duration_ms` to `HeartbeatRequest`, `BatchMigrateEntityRequest`/`Response`, and `BatchRouteUpdateRequest`/`Response`.
- **`crates/director`**: Enhanced `NodeRegistry` with tick timing telemetry, `GlobalOctreeCoordinator` with saturation checks and elastic centroid-repulsion boundary adjustments.
- **`crates/node`**: Batch entity migration pipeline with atomic lease reservations, zero-copy component serialization, and single-shot gateway route handover.
- **`crates/gateway`**: Lock-free batch route updates to minimize `DashMap` contention under high-density handovers.
