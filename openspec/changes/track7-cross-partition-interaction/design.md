# Technical Design: Track 7 - Cross-Partition Interaction & Boundary Ability Dispatch

## 1. System Architecture & Interaction Flow

```mermaid
sequenceDiagram
    autonumber
    actor PlayerA as Player A (Client)
    participant Node1 as Simulation Node 1 (Caster Node)
    participant Node2 as Simulation Node 2 (Target Node)
    actor PlayerB as Player B (Client)

    PlayerA->>Node1: CastAbility(AbilityID, Target: Entity B)
    Note over Node1: 1. Validate caster state & resource<br/>2. Resolve Target B in GhostStore (Owner: Node 2)<br/>3. Unified Spatial Query (Range & LoS)<br/>4. PROACTIVE: Deduct Resource & Start Cooldown
    Node1->>Node2: gRPC DispatchAbility(Caster: A, Target: B, CasterPos, AbilityID, MaxRange)
    
    Note over Node2: 5. Check Migration Lease (Not freezing)<br/>6. Validate Target B exists & alive<br/>7. ANTI-CHEAT: Re-validate Range(CasterPos, TargetPos) <= MaxRange * 1.2<br/>8. Apply Damage / Auras / Interruption<br/>9. Dispatch OnInteract / Combat events
    Node2-->>PlayerB: EntityState / Damage Event (via Gateway)
    Node2-->>Node1: RemoteAbilityResponse(Success, AppliedDamage, TargetDied)
    
    alt Remote Execution Failed (TargetDead / OutOfRange / TargetMigrating)
        Node1->>Node1: Queue Compensation via MPSC: Refund Resource & Clear Cooldown
    else Remote Execution Succeeded
        Note over Node1: Cooldown already active; notify client of success
    end
    Node1-->>PlayerA: AbilityResult / ActionSuccess
```

---

## 2. Core Primitives & Protobuf Definitions

### 2.1 Unified Spatial Target Candidates
In `crates/core/src/spatial.rs`:
```rust
/// Represents an entity candidate for 3D spatial queries, whether local or remote.
#[derive(Debug, Clone, PartialEq)]
pub struct SpatialTargetCandidate {
    pub guid: Guid,
    pub position: (f32, f32, f32),
    pub bounding_box: Option<BoundingBox3D>,
    /// Owning simulation node ID. If equal to local node ID, candidate is local.
    pub owning_node_id: u32,
}

impl SpatialTargetCandidate {
    pub fn is_local(&self, current_node_id: u32) -> bool {
        self.owning_node_id == current_node_id
    }
}
```

### 2.2 Protobuf Schema (`crates/schema/proto/cluster.proto`)
```protobuf
enum RemoteAbilityErrorCode {
  SUCCESS = 0;
  TARGET_NOT_FOUND = 1;
  TARGET_DEAD = 2;
  TARGET_MIGRATING = 3;
  VETOED = 4;
  OUT_OF_RANGE = 5;
}

message RemoteAbilityRequest {
  uint64 caster_guid = 1;
  uint64 target_guid = 2;
  uint32 ability_id = 3;
  Vector3 caster_position = 4;
  float max_range = 5;
  bytes payload = 6;
  uint32 source_node_id = 7;
  uint32 target_node_id = 8;
  uint64 client_timestamp = 9;
}

message RemoteAbilityResponse {
  bool success = 1;
  uint32 error_code = 2;
  string error_message = 3;
  float applied_amount = 4; // e.g. actual damage dealt or healed
  bool target_died = 5;
  bytes result_payload = 6;
}

message SingleTargetHit {
  uint64 target_guid = 1;
  float distance = 2;
}

message BatchRemoteAbilityRequest {
  uint64 caster_guid = 1;
  uint32 ability_id = 2;
  Vector3 caster_position = 3;
  float max_range = 4;
  bytes payload = 5;
  uint32 source_node_id = 6;
  uint32 target_node_id = 7;
  repeated SingleTargetHit targets = 8;
}

message BatchRemoteAbilityResponse {
  bool success = 1;
  uint32 processed_count = 2;
  repeated uint64 died_guids = 3;
  float total_applied_amount = 4;
}

service NodeService {
  rpc RemoteCommand (RemoteCommandRequest) returns (RemoteCommandResponse);
  rpc DispatchAbility (RemoteAbilityRequest) returns (RemoteAbilityResponse);
  rpc DispatchBatchAbility (BatchRemoteAbilityRequest) returns (BatchRemoteAbilityResponse);
  rpc SyncGhosts (GhostSyncRequest) returns (GhostSyncResponse);
  rpc MigrateEntity (MigrateEntityRequest) returns (MigrateEntityResponse);
}
```

---

## 3. Concurrency, Atomicity & Failure Modes

### 3.1 Proactive Cooldown & "Boundary Dance" Prevention
- In Phase 1 (Local):
  - Deduct caster resource (mana/energy).
  - Immediately activate the ability's cooldown and global cooldown (GCD) in `CooldownTracker`.
- If the caster migrates to another node while the remote dispatch is in flight:
  - `CooldownTracker` is serialized into `MigrateEntityRequest` and restored on the destination node.
  - The cooldown remains active, completely preventing players from crossing boundaries to spam high-cooldown abilities.
- If the remote dispatch returns an error (TargetDead, OutOfRange, TargetMigrating, Timeout):
  - A compensation event refunds the spent resource and resets the cooldown on the caster.

### 3.2 Non-Blocking Ingress & Lock Ordering
- The 20 Hz simulation tick loop (`TickSimulation::step()`) **never blocks** on network calls.
- Inbound gRPC responses submit `AbilityCompensationEvent`s to a lock-free unbounded MPSC channel (`ability_result_tx`).
- `TickSimulation::step()` drains `ability_result_rx` at the start of each tick, applying refunds safely within the single-writer deterministic loop with zero write-lock contention.

### 3.3 Authoritative Anti-Cheat Range Re-Validation
When `DispatchAbility` arrives on the target's node:
1. Target Node checks `dist(request.caster_position, target.position)`.
2. Tolerance factor is clamped at $\text{max\_range} \times 1.2$ (allowing for $1$ tick of $20\text{ Hz}$ motion).
3. If distance exceeds tolerance, Node 2 authoritatively rejects the cast with `RemoteAbilityErrorCode::OutOfRange`, preventing stale-ghost distance exploitation.

### 3.4 Target Node Atomicity & Lease Guards
1. **Migration Lease Check**:
   ```rust
   if sim.is_migrating(target) {
       return Ok(Response::new(RemoteAbilityResponse {
           success: false,
           error_code: RemoteAbilityErrorCode::TargetMigrating as u32,
           error_message: "Target entity is currently migrating".to_string(),
           ..Default::default()
       }));
   }
   ```
2. **Death & Existence Check**:
   Target must exist in `entity_store` and have `HealthComponent.is_dead == false`.
3. **Application**:
   Damage/healing is applied, casting actions are interrupted if flagged, and damage events are queued into the local event bus.

---

## 4. Telemetry & Metrics

- `mmo_cross_partition_ability_dispatched_total{ability_id, target_node}`: Outbound cross-partition dispatches.
- `mmo_cross_partition_ability_received_total{ability_id, source_node}`: Inbound remote ability executions.
- `mmo_cross_partition_ability_latency_seconds`: Round-trip times for inter-node ability gRPC calls.
- `mmo_cross_partition_ability_errors_total{error_code}`: Vetoes, out-of-range rejections, target dead, and timeouts.
