# Design: Phase 3 (P2) Cluster Resilience, Property Testing, and CI/CD Quality Gates

## Context and Architectural Goals

In distributed MMO clusters, node dropouts and hardware failures are normal operational conditions rather than unexpected anomalies. When a simulation node crashes or is disconnected:
1. The global world topology must retain complete spatial coverage—no point within world boundaries should map to a dead node or become an unreachable void.
2. Partition reassignments must be fenced with a monotonically increasing generational epoch (`topology_epoch`) to reject stale routing updates or split reports from disconnected nodes.
3. Wire codecs and spatial partitioning must be mathematically validated against arbitrary fuzz inputs and geometric invariants via generative property testing (`proptest`).
4. Automated continuous integration (`.github/workflows/rust.yml`) must enforce formatting, zero-warning clippy compliance, and full workspace test passes.

## Architectural Design

### 1. Director Orphaned Partition Recovery & Epoch Fencing

```
       [ Node Heartbeat Timeout Detected ]
                      │
                      ▼
       [ DirectorServiceImpl::check_and_recover_dead_nodes ]
       (Acquires registry write lock -> coordinator write lock)
                      │
                      ├─► NodeRegistry::detect_dead_nodes(timeout)
                      │     └─ Evicts dead node records
                      │
                      ├─► GlobalOctreeCoordinator::recover_orphaned_partitions
                      │     ├─ Scans topology leaves for dead node assignments
                      │     ├─ Queries registry for least-loaded active nodes
                      │     ├─ Reassigns orphaned leaves to surviving nodes
                      │     ├─ Updates spatial_index with new authoritative nodes
                      │     └─ Increments topology_epoch (epoch fencing)
                      │
                      ▼
       [ Returns PartitionRecoveryEvents & Logs Reassignments ]
```

#### Monotonic Topology Epoch Counter & Wire Protocol Visibility
- Add `pub topology_epoch: u64` to `GlobalOctreeCoordinator` initialized to `1`.
- Any mutation of spatial boundaries (splits via `handle_split`, merges via `handle_merge`, or recovery via `recover_orphaned_partitions`) increments `topology_epoch += 1`.
- Expose `uint64 topology_epoch` over gRPC in `crates/schema/proto/cluster.proto`:
  - `RegisterNodeResponse` contains `uint64 topology_epoch`
  - `HeartbeatResponse` contains `uint64 topology_epoch`
  - `SplitReportResponse` contains `uint64 topology_epoch`
  This enables simulation nodes and gateway routers to observe generational topology shifts and fence stale routing state.
- `PartitionRecoveryEvent` captures:
  ```rust
  #[derive(Debug, Clone, PartialEq)]
  pub struct PartitionRecoveryEvent {
      pub bounds: BoundingBox3D,
      pub previous_node_id: u32,
      pub new_node_id: u32,
      pub epoch: u64,
  }
  ```

#### Recovery Algorithm in `GlobalOctreeCoordinator`
```rust
pub fn recover_orphaned_partitions(
    &mut self,
    dead_node_ids: &[u32],
    registry: &mut NodeRegistry,
) -> Vec<PartitionRecoveryEvent>
```
1. Recursively find all `GlobalOctreeNode::Leaf` nodes where `assigned_node_id` matches any ID in `dead_node_ids`.
2. For each orphaned leaf:
   - Identify candidate active nodes from `registry.active_node_ids()`.
   - If active nodes exist, choose the least loaded via `registry.find_least_loaded_node(&[])`.
   - Update leaf: `assigned_node_id = Some(target_node_id)`.
   - Update registry: `registry.add_assigned_bounds(target_node_id, bounds)`.
   - Update spatial index: remove previous node registration and insert new node registration at `bounds.center()`.
   - Record `PartitionRecoveryEvent`.
3. If no active nodes remain (cluster-wide blackout), set `assigned_node_id = None`.
4. If any partitions were reassigned, increment `self.topology_epoch += 1`.

#### Lock Ordering Guarantee
To avoid deadlocks between the gRPC request threads and the background monitoring loop:
- Lock acquisition order: `registry.write()` $\rightarrow$ `coordinator.write()`.
- Never acquire `registry` while holding `coordinator`.

### 2. Generative Property-Based Testing (`proptest`)

Introduce `proptest = "1.6"` as a dev-dependency in `crates/schema` and `crates/core`.

#### Wire Codec Invariants (`crates/schema/tests/packet_proptest.rs`)
- **Arbitrary Binary Framing**: Generate arbitrary payloads (0 to 64 KiB), opcodes, and sequence numbers. Verify `ClientPacket::encode` $\rightarrow$ `ClientPacket::decode_frame` reconstructs the identical packet.
- **Floating Point Stability**: Generate arbitrary finite coordinates and orientations for `MovementPacket` (constrained to finite normal ranges, avoiding non-canonical NaN representation mismatches). Verify roundtrip preservation within floating point precision limits.
- **Robust Framing Under Malformed Bytes**: Feed arbitrary random byte slices to `ClientPacket::decode_frame`. Invariant: it must return `Ok(None)` (partial frame) or `Err(DecodeError)`—it must never panic or enter infinite loops.

#### Spatial Octree Invariants (`crates/core/tests/octree_proptest.rs`)
- **Query Radius Oracle**: For arbitrary generated sets of points and query parameters `(center, radius)`:
  - Linear scan: filter points where $(\Delta x)^2 + (\Delta y)^2 + (\Delta z)^2 \le r^2$.
  - Octree scan: `octree.query_radius(center, radius)`.
  - Invariant: both sets must contain identical GUIDs (assert exact `HashSet` equality).
- **Bounding Box Geometric Consistency**:
  - `contains(point)` must be `true` for `center()` of any bounding box.
  - `intersects(other)` must be commutative: $A \cap B = B \cap A$.

### 3. GitHub Actions Continuous Integration (`.github/workflows/rust.yml`)

Create a standardized GitHub Actions workflow triggered on push and pull request to `main`:
- Runner: `ubuntu-latest`.
- Dependencies: `protobuf-compiler` (apt package for prost/tonic builds).
- Caching: `Swatinem/rust-cache@v2` caching target artifacts for accelerated PR and push feedback.
- Steps:
  1. `cargo fmt --all -- --check`
  2. `cargo clippy --workspace --all-targets -- -D warnings`
  3. `cargo test --workspace --verbose`
