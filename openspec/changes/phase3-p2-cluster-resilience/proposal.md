# Proposal: Phase 3 (P2) Cluster Resilience, Property Testing, and CI/CD Quality Gates

## Why

As the MMO cluster scales across multiple distributed nodes and prepares for production deployment, cluster resilience under node failure and automated quality assurance become paramount:
1. **Orphaned Spatial Partitions on Node Eviction**: When `NodeRegistry::check_dead_nodes` detects an unresponsive node after heartbeat timeouts (e.g. crash, network partition), the Director evicts the node from the registry but does not reassign the deceased node's spatial bounding boxes in `GlobalOctreeCoordinator`. This leaves a spatial "void" in the world where no active simulation node is authoritative, causing player positioning, kinematic updates, and entity routing in that zone to fail silently.
2. **Missing Partition Epoch Fencing**: Spatial topology reallocations currently lack a generational epoch counter. Simulation nodes, gateway routers, and director services need a monotonically incrementing epoch so partition reassignments and split/merge transitions can be fenced against stale commands from network-delayed or split-brain nodes.
3. **Absence of Property-Based Testing (`proptest`)**: The codebase currently relies strictly on deterministic, handwritten unit and integration test fixtures. Complex edge cases—such as binary wire framing roundtrips with arbitrary randomized byte sequences, malformed packet payloads, non-canonical floating point values (NaN, Inf, subnormal), and spatial octree point insertions/queries near boundary limits—must be validated with generative property testing to prevent subtle crashes or out-of-bounds panics.
4. **Lack of Automated CI Quality Gate**: There is currently no GitHub Actions workflow (`.github/workflows/rust.yml`) enforcing compilation, unit testing, integration testing, formatting (`cargo fmt`), and strict linting (`cargo clippy --workspace --all-targets -- -D warnings`) on pull requests and pushes.

## What Changes

- **Director Orphaned Partition Recovery & Epoch Fencing**:
  - Extend `GlobalOctreeCoordinator` with `topology_epoch: u64` that increments on every topology mutation (splits, merges, node recovery).
  - Implement `GlobalOctreeCoordinator::recover_orphaned_partitions(&mut self, dead_node_ids: &[u32], registry: &mut NodeRegistry) -> Vec<PartitionRecoveryEvent>` to identify orphaned leaf octants and re-delegate them to active, least-loaded surviving nodes while keeping the spatial index and registry bounds synchronized.
  - Coordinate the background dead-node detection loop in `crates/director/src/main.rs` and `DirectorServiceImpl` to immediately trigger partition recovery when nodes time out.
- **Property-Based Testing (`proptest`)**:
  - Add `proptest` to workspace dev-dependencies.
  - Create property tests in `crates/schema/tests/packet_proptest.rs` testing arbitrary binary packet serialization, framing, and deserialization roundtrips.
  - Create property tests in `crates/core/tests/octree_proptest.rs` testing `BoundingBox3D` geometric predicates and `Octree` spatial radius queries against mathematical invariant oracles.
- **GitHub Actions CI Quality Pipeline**:
  - Add `.github/workflows/rust.yml` with jobs for checking formatting (`cargo fmt --check`), linting (`cargo clippy --workspace --all-targets -- -D warnings`), running the entire workspace test suite (`cargo test --workspace`), and verifying benchmark builds.

## Capabilities

### New Capabilities
- `testing/proptest`: Generative property-based testing suites for binary packet codecs and spatial partition trees.
- `ci/workflow`: Continuous Integration GitHub Actions workflow for pull request and main branch verification.

### Modified Capabilities
- `control/director`: Update Director requirements to mandate automatic recovery of orphaned spatial partitions upon node failure and generational epoch fencing.

## Impact

- `crates/director`: `coordinator.rs`, `registry.rs`, `service.rs`, `main.rs`, and unit/integration tests for automated partition recovery and epoch tracking.
- `crates/core`: `Cargo.toml` and new `tests/octree_proptest.rs` for property-based geometric testing.
- `crates/schema`: `Cargo.toml` and new `tests/packet_proptest.rs` for property-based wire codec testing.
- `.github/workflows/rust.yml`: Automated CI pipeline configuration.
