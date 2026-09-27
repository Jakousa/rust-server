# Proposal: Track 7 - Cross-Partition Interaction & Boundary Ability Dispatch

## 1. Context & Motivation

The MMO server cluster coordinates an expansive 3D world across multiple authoritative simulation nodes via an Octree spatial domain decomposition. However, an essential architectural requirement for a seamless distributed MMO is that **spatial partition borders must remain completely invisible to players and gameplay systems**.

Currently:
- An entity simulated on Node 1 cannot directly target or execute combat abilities, spells, trades, or interactions against an entity simulated on adjacent Node 2.
- While Node 1 replicates boundary entities as read-only proxies in its [`GhostStore`](file:///home/japek/Github/rust-server/crates/node/src/network.rs#L69-L79), core spatial targeting queries (`crates/core/src/spatial.rs`) and gameplay systems (`crates/gameplay/src/ability.rs`) operate only against local `EntityStore` entries.
- If a player attempts an ability targeting a ghost entity, the action either fails with `EntityNotFound` or is ignored. Furthermore, area-of-effect (AoE) abilities (cleaves, radial explosions) cast near a boundary truncate at the partition plane, failing to hit neighboring entities.

To achieve seamless cross-border gameplay, the cluster requires:
1. **Unified Ghost & Authoritative Spatial Targeting**: Enabling raycasts, cones, and spheres to gather and validate both local entities and ghost proxies.
2. **Authoritative Cross-Partition Ability Dispatch**: A dedicated inter-node gRPC protocol allowing the caster node to validate cast initiation and deduct resources, while the owning node authoritatively validates target state, applies effects (damage, healing, auras, interruptions), and returns atomic results.
3. **Partition-Aware Multi-Target AoE Routing**: Decomposing boundary-straddling AoE hits by owning node and dispatching them in parallel without duplicated effects.
4. **Lifecycle & Migration Resilience**: Safely handling target death, mid-flight migration leases, and node disconnects during casting.

---

## 2. Proposed Capabilities

### In `crates/core`:
- **Unified Spatial Targeting (`crates/core/src/spatial.rs`)**:
  - Trait `SpatialTargetCandidate` abstracting entity position, bounds, and owning node.
  - Extended spatial queries (Point Range, Cone Cleave, Sphere AoE, Raycast LoS) capable of evaluating heterogeneous collections of local and ghost targets.
  - Batch target partitioner: grouping gathered targets by authoritative owning node ID (`target_node_id`).

### In `crates/schema`:
- **Cluster Proto Extensions (`crates/schema/proto/cluster.proto`)**:
  - `RemoteAbilityRequest`: Carries `caster_guid`, `target_guid`, `ability_id`, `caster_position`, `effect_payload`, and sequence identifier.
  - `RemoteAbilityResponse`: Returns `success`, `error_code`, `applied_effects` (damage dealt, auras applied, target died status).
  - `BatchRemoteAbilityRequest` / `BatchRemoteAbilityResponse`: Efficient batched multi-target hits for boundary AoE abilities.
  - New gRPC RPC on `NodeService`: `DispatchAbility(RemoteAbilityRequest)` and `DispatchBatchAbility(BatchRemoteAbilityRequest)`.

### In `crates/node`:
- **Cross-Partition Ability Dispatcher (`crates/node/src/ability_dispatcher.rs`)**:
  - Integration with `AbilityPlugin` and `TimedAction` completion.
  - Remote node gRPC client pool managing inter-node channels to adjacent simulation nodes.
  - Atomic two-phase ability resolution:
    - Phase 1 (Local): Verify cooldown, deduct caster resource (mana/energy), begin cast action.
    - Phase 2 (Remote or Local): If target is local, apply immediately; if target is ghost, dispatch gRPC call to `source_node_id`.
  - Mid-flight migration and lifecycle checks (verifying target is not migrating under active lease).
- **Node Service Handler (`crates/node/src/network.rs`)**:
  - Handling `DispatchAbility` and `DispatchBatchAbility` requests.
  - Authoritatively applying combat effects, damage reduction, attribute mutations, and action interruptions on target entities.

### In `crates/gameplay`:
- **Boundary-Aware Ability Execution (`crates/gameplay/src/ability.rs`)**:
  - Updating `execute_or_start_ability` to accept ghost candidates and route cross-border executions through the dispatch pipeline.

---

## 3. Non-Goals

- Continuous distributed lock managers or distributed 2PC transactions spanning three or more nodes for simple ability casts (single target mutations remain atomic on the target's owning node).
- Client-side prediction for remote node health changes (server retains authoritative simulation mastership).
- Cross-cluster or cross-datacenter multi-region interactions (scoped to nodes within the local Kubernetes cluster).

---

## 4. Architectural Invariants

1. **Zero Partition Artifacts**: Abilities cast across a partition seam behave identically in damage, timing, and cooldowns to abilities cast within the same node.
2. **Single-Writer Authority**: Target attribute mutations (health, auras, deaths) are executed exclusively on the node that owns the target entity; caster nodes never directly mutate ghost records.
3. **No Duplicate AoE Hits**: When an AoE ability gathers targets across a boundary, no entity receives duplicate damage packets or events.
4. **Migration Lease Safety**: An entity under an active migration lease strictly rejects remote ability hits with `MigrationVeto` to prevent split-brain state desynchronization.
