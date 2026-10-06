# Tasks: Dynamic Instancing, Transient Dungeons & Realm Federation

- [x] 1. Schema & Protobuf Extensions (`crates/schema`) <!-- id: 1 -->
  - [x] 1.1 Define Protobuf messages in `crates/schema/proto/cluster.proto`: `CreateInstanceRequest`, `CreateInstanceResponse`, `TerminateInstanceRequest`, `TerminateInstanceResponse`, `InstanceHeartbeatRequest`, `InstanceHeartbeatResponse`, `IssueRealmTransferClaimRequest`, `IssueRealmTransferClaimResponse`, `VerifyRealmTransferClaimRequest`, `VerifyRealmTransferClaimResponse` (with 64-bit `nonce` field).
  - [x] 1.2 Add corresponding RPC endpoints to `DirectorService` in `cluster.proto`.
  - [x] 1.3 Verify compilation and codegen in `mmo-schema`.

- [x] 2. Instance Registry & Federation Authority in Director (`crates/director`) <!-- id: 2 -->
  - [x] 2.1 Implement `InstanceRegistry` in `crates/director/src/instance.rs` supporting instance allocation, node assignment based on load scores, bounding box validation (`is_finite`, `min < max`), idle expiration detection, and reap cleanup.
  - [x] 2.2 Implement `RealmFederationAuthority` in `crates/director/src/federation.rs` with HMAC-SHA256 claim issuance, constant-time verification, and nonce-based replay cache.
  - [x] 2.3 Refactor `DirectorServiceImpl` in `crates/director/src/service.rs`:
    - Enforce global lock ordering: `instance_registry` -> `node_registry` -> `coordinator`.
    - Update `check_and_recover_dead_nodes` to acquire `instance_registry` first, remove instances on dead nodes, and trigger route cleanup.
    - Implement the new gRPC endpoints for instance management and realm transfer claims.
  - [x] 2.4 Add comprehensive unit tests in `crates/director` for instance allocation, expiration reaping, node crash cleanup, and federation ticket verification.

- [x] 3. Node-Side Instance Domain Manager (`crates/node`) <!-- id: 3 -->
  - [x] 3.1 Implement `InstanceDomainManager` in `crates/node/src/instance.rs` to track instance lifecycles, entity associations, and idle timeouts.
  - [x] 3.2 Integrate instance domain management into `TickSimulation` in `crates/node/src/sim.rs`.
  - [x] 3.3 Add unit tests verifying node instance creation, entity assignment, and idle teardown.

- [x] 4. Ingress Routing & Gateway Verification (`crates/gateway`) <!-- id: 4 -->
  - [x] 4.1 Ensure gateway route handover handles instance node endpoints seamlessly via `SessionRouter` and `handover.rs`.
  - [x] 4.2 Verify gateway tests passing with dynamic instance routing scenarios.

- [x] 5. Quality Gates & Cluster Verification <!-- id: 5 -->
  - [x] 5.1 Run `cargo fmt --all -- --check`.
  - [x] 5.2 Run `cargo clippy --workspace --all-targets -- -D warnings`.
  - [x] 5.3 Run `cargo test --workspace`.
