# Implementation Tasks: Track 2 Dynamic Partitioning & Live Migration

## Task List

- [x] **Task 1: Protocol Schema Extensions (`crates/schema`)**
  - [x] 1.1 Update `crates/schema/proto/cluster.proto`:
    - Add `SerializedComponent` message.
    - Add `MigrateEntityRequest` (including `last_sequence` and `last_migration_tick`) and `MigrateEntityResponse` messages.
    - Add `RouteUpdateRequest` and `RouteUpdateResponse` messages.
    - Add `MigrateEntity` RPC to `NodeService`.
    - Define `GatewayService` with `UpdateRoute` RPC.
  - [x] 1.2 Update build scripts and compile schema crate:
    - Verify `cargo build -p mmo-schema` succeeds and exposes new types.

- [x] **Task 2: Dynamic Gateway Route Updating (`crates/gateway`)**
  - [x] 2.1 Refactor `SessionRouter` in `crates/gateway/src/router.rs` for atomic route updates.
  - [x] 2.2 Implement `GatewayServiceImpl` implementing `GatewayService` in `crates/gateway/src/server.rs`.
  - [x] 2.3 Wire `GatewayServiceServer` into the Gateway server listen loop.
  - [x] 2.4 Add unit test verifying that calling `update_route` via gRPC updates the underlying `SessionRouter` lock-free tables.
  - [x] 2.5 Verify with `cargo test -p mmo-gateway`.

- [x] **Task 3: Entity Migration State, Strict Freeze & Component Serialization (`crates/core`, `crates/node`)**
  - [x] 3.1 Add `Migrating` lease tracking to `TickSimulation` (`migrating_leases: HashMap<Guid, Instant>`).
  - [x] 3.2 Add helper methods to acquire, query, and release migration leases.
  - [x] 3.3 Enforce strict entity freeze:
    - In `NodeServiceImpl::remote_command`: reject interactions targeted at migrating entities with `RemoteCommandErrorCode::Vetoed`.
    - In `TickSimulation::step`: skip mutating entities under migration leases.
  - [x] 3.4 Implement serialization and deserialization of entity components, kinematic state (`last_sequence`), and `last_migration_tick` for migration payloads.
  - [x] 3.5 Add unit tests for entity serialization, lease acquisition, and freeze behavior.

- [x] **Task 4: Two-Phase Migration Inbound & Outbound Protocol (`crates/node`)**
  - [x] 4.1 Implement `migrate_entity` RPC handler in `NodeServiceImpl` in `crates/node/src/network.rs`:
    - Validate target coordinates against local partition bounds.
    - Insert entity into `EntityStore` and `Octree`.
    - Restore kinematic state (`last_sequence`), cooldown tick, and components.
    - If session ID present, register AoI observer.
    - Return `MigrateEntityResponse` with success.
  - [x] 4.2 Implement outbound migration client in `crates/node`:
    - Acquire lease without holding lock across `.await` points.
    - Send `MigrateEntityRequest` over gRPC to recipient node.
    - On success: donor calls Gateway `UpdateRoute` to commit route, then despawns local entity without death events.
    - On failure/timeout: donor rolls back lease and clamps entity to donor bounds (zero-duplication guarantee, gateway unaffected).
  - [x] 4.3 Add telemetry metrics (`mmo_migration_outbound_total`, `mmo_migration_inbound_total`, `mmo_migration_rollbacks_total`).
  - [x] 4.4 Verify with `cargo test -p mmo-node`.

- [x] **Task 5: Boundary Crossing Detection, Hysteresis & Cooldown (`crates/node`)**
  - [x] 5.1 Enhance `process_client_move_with_seq` in `TickSimulation`:
    - Detect when valid movement exits local partition bounds.
    - Evaluate hysteresis margin ($\delta_{\text{hysteresis}} = 1.0$) and `last_migration_tick` cooldown timer.
    - If boundary crossed beyond margin and cooldown elapsed, trigger asynchronous migration task instead of hard snap-back.
  - [x] 5.2 Add unit and property tests for boundary oscillation and hysteresis dampening.
  - [x] 5.3 Verify with `cargo test -p mmo-node`.

- [x] **Task 6: Dynamic Split Redistribution & Multi-Node Integration Tests**
  - [x] 6.1 Connect split trigger in node heartbeat loop to initiate batch entity migration for reallocated octants.
  - [x] 6.2 Write multi-node live entity migration integration test:
    - Spawn two simulation nodes with adjacent partition boundaries.
    - Spawn player entity on Node 1, move across boundary to Node 2.
    - Verify entity transfers to Node 2 without duplication or loss.
    - Verify Edge Gateway updates route and subsequent movements process on Node 2.
  - [x] 6.3 Write network timeout rollback test verifying zero-duplication guarantee.
  - [x] 6.4 Run full workspace tests and strict clippy checks:
    - `cargo test --workspace`
    - `cargo clippy --workspace --all-targets -- -D warnings`
