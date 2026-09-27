# Implementation Tasks: Track 7 - Cross-Partition Interaction & Boundary Ability Dispatch

## Task 1: Unified Spatial Target Candidate Primitives
- [x] 1.1 Define `SpatialTargetCandidate` in `crates/core/src/spatial.rs` representing local or ghost entities with `owning_node_id`, `entity_id`, `position`, `entity_type`, and `health`.
- [x] 1.2 Implement unified cone, sphere, and raycast gathering methods accepting `&[SpatialTargetCandidate]`.
- [x] 1.3 Implement `partition_candidates_by_node(candidates, local_node_id)` returning local candidates and a map of remote batches grouped by node ID.
- [x] 1.4 Add unit tests in `crates/core/src/spatial.rs` verifying mixed local/ghost queries, distance sorting, and partitioning.
- [x] 1.5 Verification: `cargo test -p mmo-core -- spatial` passes cleanly.

## Task 2: Protobuf Schema & Inter-Node gRPC Services
- [x] 2.1 Add `RemoteAbilityErrorCode` enum (`SUCCESS`, `TARGET_NOT_FOUND`, `TARGET_DEAD`, `TARGET_MIGRATING`, `OUT_OF_RANGE`, `INTERNAL_ERROR`) in `crates/schema/proto/cluster.proto`.
- [x] 2.2 Add `RemoteAbilityRequest` (with `caster_position`, `max_range`, `ability_id`, `caster_entity_id`, `target_entity_id`, `damage`, `healing`, `resource_cost`), `RemoteAbilityResponse`, `BatchRemoteAbilityRequest`, and `BatchRemoteAbilityResponse` to `cluster.proto`.
- [x] 2.3 Add `DispatchAbility` and `DispatchBatchAbility` RPCs to `NodeService` in `cluster.proto`.
- [x] 2.4 Rebuild `mmo-schema` and verify code generation.
- [x] 2.5 Verification: `cargo check -p mmo-schema` compiles with generated gRPC methods.

## Task 3: Target Node Inbound Ability Execution & Range Re-Validation
- [x] 3.1 Implement `dispatch_ability` on `NodeServiceImpl` in `crates/node/src/network.rs`:
  - Validate target entity exists in `entity_store`.
  - Check active migration lease (`sim.is_migrating(target)`) and reject with `TARGET_MIGRATING` if freezing.
  - Verify target is alive (`HealthComponent.is_dead == false`), rejecting with `TARGET_DEAD` if dead.
  - Authoritative range re-validation: verify `distance(request.caster_position, target.position) <= max_range * 1.2`, rejecting with `OUT_OF_RANGE` if exceeded.
  - Apply damage, healing, or aura modifications to `AttributeSet` and `HealthComponent`.
  - Interrupt active cancellable `TimedAction`s on damage if applicable.
- [x] 3.2 Implement `dispatch_batch_ability` on `NodeServiceImpl` executing batched hits for boundary AoE attacks.
- [x] 3.3 Add unit tests in `crates/node/src/network.rs` verifying inbound remote ability execution, dead target rejection, out-of-range rejection, and migration veto.
- [x] 3.4 Verification: `cargo test -p mmo-node -- network` passes.

## Task 4: Caster Node Outbound Ability Dispatcher & Compensation Pipeline
- [x] 4.1 Create `CrossNodeDispatcher` in `crates/node/src/ability_dispatcher.rs` managing cached inter-node gRPC channels to peer simulation nodes.
- [x] 4.2 Implement proactive cooldown and GCD activation in Phase 1 (Local) so caster cannot exploit boundary crossing.
- [x] 4.3 Implement non-blocking MPSC channel (`ability_compensation_tx` / `rx`) on `TickSimulation` drained at the start of each tick:
  - If remote dispatch returns non-zero error code or network timeout (100ms), refund caster's resource cost and reset ability cooldown.
- [x] 4.4 Integrate `CrossNodeDispatcher` with `TickSimulation` and `GhostStore`.
- [x] 4.5 Verification: Unit tests verifying outbound channel resolution, proactive cooldown, and resource refund on error response.

## Task 5: Boundary Multi-Target AoE Routing & Partitioning
- [x] 5.1 Update `crates/gameplay/src/ability.rs` to support boundary AoE execution using `partition_candidates_by_node`.
- [x] 5.2 Implement parallel dispatch for multi-node AoE bursts, ensuring local hits commit immediately and remote hits execute concurrently.
- [x] 5.3 Ensure zero duplicate hits across partition boundaries.
- [x] 5.4 Verification: Unit tests in `crates/gameplay/src/ability.rs` testing multi-target boundary hit partitioning.

## Task 6: End-to-End Cross-Partition Integration Tests
- [x] 6.1 Create `crates/node/tests/cross_partition_ability_e2e.rs` running two live simulation nodes (Node 1 and Node 2) connected via gRPC.
- [x] 6.2 Test Case 1: Player A on Node 1 casts targeted spell on Ghost B (owned by Node 2); verify Node 2 health decrements and Node 1 receives success confirmation.
- [x] 6.3 Test Case 2: Player A casts boundary AoE cleave hitting both local Node 1 entities and remote Node 2 entities simultaneously.
- [x] 6.4 Test Case 3: Player A attempts ability on entity B during active migration freeze; verify clean veto (`TARGET_MIGRATING`) and caster resource refund.
- [x] 6.5 Test Case 4: Player A attempts ability with stale ghost beyond $1.2\times$ range; verify `OUT_OF_RANGE` veto and caster resource refund.
- [x] 6.6 Verification: `cargo test -p mmo-node --test cross_partition_ability_e2e` passes.

## Task 7: Quality Gates & Documentation Verification
- [x] 7.1 Run `cargo fmt --all -- --check`.
- [x] 7.2 Run strict linter: `cargo clippy --workspace --all-targets -- -D warnings`.
- [x] 7.3 Run full workspace test suite: `cargo test --workspace`.
- [x] 7.4 Mark completed tasks in `tasks.md` and update `ROADMAP.md`.
