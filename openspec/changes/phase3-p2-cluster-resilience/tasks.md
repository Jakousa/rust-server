## 1. Director Orphaned Partition Recovery & Epoch Fencing

- [x] 1.1 Update `crates/schema/proto/cluster.proto` to include `uint64 topology_epoch` in `RegisterNodeResponse`, `HeartbeatResponse`, and `SplitReportResponse`.
- [x] 1.2 Add `topology_epoch: u64` and `PartitionRecoveryEvent` to `GlobalOctreeCoordinator` in `crates/director/src/coordinator.rs`, incrementing epoch on splits, merges, and recovery operations.
- [x] 1.3 Implement `GlobalOctreeCoordinator::recover_orphaned_partitions` to reassign orphaned leaf octants to surviving active nodes, updating the spatial index and node registry.
- [x] 1.4 Add `check_and_recover_dead_nodes` method on `DirectorServiceImpl` in `crates/director/src/service.rs`, populate `topology_epoch` across all gRPC responses, enforce strict lock ordering (`registry` then `coordinator`), and update the background detection task in `crates/director/src/main.rs`.
- [x] 1.5 Add unit and integration tests in `crates/director/src/coordinator.rs` and `crates/director/src/service.rs` verifying recovery of orphaned partitions, epoch progression, and cluster blackout handling.

## 2. Property-Based Testing (`proptest`)

- [x] 2.1 Add `proptest = "1.6"` to `dev-dependencies` in `crates/schema/Cargo.toml` and implement property-based tests in `crates/schema/tests/packet_proptest.rs` verifying arbitrary byte payload roundtrips, fuzz resilience without panics, and floating-point movement fidelity under finite ranges.
- [x] 2.2 Add `proptest = "1.6"` to `dev-dependencies` in `crates/core/Cargo.toml` and implement property-based tests in `crates/core/tests/octree_proptest.rs` validating `BoundingBox3D` geometric commutativity and `Octree::query_radius` correctness against an $O(N)$ linear oracle using exact `HashSet` comparison.

## 3. GitHub Actions CI Quality Gate

- [x] 3.1 Create `.github/workflows/rust.yml` configured for Ubuntu with `protobuf-compiler`, `Swatinem/rust-cache@v2`, and stable Rust, executing `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`.
- [x] 3.2 Verify workflow validity locally by running format checks, strict clippy, and full workspace tests.

## 4. End-to-End Verification & Workspace Polish

- [x] 4.1 Run `cargo test --workspace` across all crates to ensure 100% of unit, integration, and property tests pass.
- [x] 4.2 Run `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all -- --check` to ensure zero warnings and proper formatting.

