# Technical Design: Session Hydration Lifecycle, K8s Hardening & Scenario E2E Runner

## 1. Player Session Hydration & Disconnect Flush (`crates/node/src/sim.rs`)

### 1.1 `TickSimulation::hydrate_player_session` & `disconnect_player_session`
In `crates/node/src/sim.rs`:
- `pub fn hydrate_player_session(&mut self, guid: Guid, session_id: u64, session_epoch: u64, saved_snapshot: Option<&mmo_storage::EntitySnapshot>, aoi_radius: f32) -> (f32, f32, f32)`:
  1. Check `session_epoch` fencing (`if !self.update_session_epoch(guid, session_epoch) { ... }`).
  2. Determine `(pos, rot)`: from `saved_snapshot.map(|s| (s.position, s.rotation)).unwrap_or(((0.0, 0.0, 0.0), 0.0))`.
  3. Spawn or update `Entity` in `self.entity_store` and `self.octree`, and initialize `EntityKinematicState` at `pos`.
  4. If `let Some(snap) = saved_snapshot`:
     - If `!snap.state_blob.is_empty()`, restore components via `self.codecs.deserialize_all_msgpack(&snap.state_blob, &mut entity.components)`.
     - Else if `!snap.components_json.is_empty()` and `let Ok(val) = serde_json::from_str(&snap.components_json)`, call `self.codecs.deserialize_into(&val, &mut entity.components)`.
  5. Register AoI observer: `self.aoi.register_observer(session_id, guid, aoi_radius);`.
  6. Return `pos`.
- `pub fn disconnect_player_session(&mut self, guid: Guid, session_epoch: u64) -> Option<mmo_storage::EntitySnapshot>`:
  1. Validate `self.is_valid_upstream_epoch(guid, session_epoch)`.
  2. Extract final `EntitySnapshot` for `guid` (position, rotation, serialized `state_blob` / `components_json`) and enqueue into `self.snapshotter` if present.
  3. Call `self.despawn_entity(guid)` (which cleans up `entity_store`, `octree`, `kinematic_states`, `aoi`, `session_epochs`, `rate_limits`).
  4. Return `Some(snapshot)`.

---

## 2. Kubernetes Manifests Hardening (`k8s/base/`)

1. **`k8s/base/postgres.yaml`**:
   - Change `kind: Deployment` to `kind: StatefulSet` with `serviceName: postgres`.
   - Reference `POSTGRES_PASSWORD` from a Kubernetes `Secret` (`postgres-credentials`) via `valueFrom.secretKeyRef`.
   - Replace `emptyDir: {}` with `volumeClaimTemplates` (`name: postgres-data`, `accessModes: ["ReadWriteOnce"]`, `storage: 10Gi`).
   - Add pod `securityContext` (`runAsNonRoot: true`, `runAsUser: 70`, `runAsGroup: 70`, `fsGroup: 70`, `seccompProfile: { type: RuntimeDefault }`) and container `securityContext` (`allowPrivilegeEscalation: false`, `capabilities: { drop: ["ALL"] }`).
2. **`k8s/base/{director.yaml, node-statefulset.yaml, gateway.yaml, loadtest.yaml}`**:
   - Add pod `securityContext` (`runAsNonRoot: true`, `runAsUser: 10001`, `runAsGroup: 10001`, `seccompProfile: { type: RuntimeDefault }`) and container `securityContext` (`allowPrivilegeEscalation: false`, `readOnlyRootFilesystem: true`, `capabilities: { drop: ["ALL"] }`).
   - Add `PodDisruptionBudget` manifests in `k8s/base/pdb.yaml` (and list in `k8s/base/kustomization.yaml`) for `mmo-node` and `mmo-gateway`.

---

## 3. Declarative Scenario-Driven E2E Runner (`crates/loadtest/src/scenario.rs`)

Create `crates/loadtest/src/scenario.rs`:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioManifest {
    pub name: String,
    pub gateway_addr: String,
    #[serde(default)]
    pub max_p99_rtt_ms: Option<f64>,
    pub steps: Vec<ScenarioStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ScenarioStep {
    Connect { bot_id: u32 },
    Authenticate { bot_id: u32, token: String, expect_success: bool },
    Move { bot_id: u32, position: [f32; 3], velocity: [f32; 3], yaw: f32 },
    Ping { bot_id: u32 },
    WaitMs { duration_ms: u64 },
    AssertKnownEntity { bot_id: u32, target_guid: u64, max_distance: Option<f32> },
    Chat { bot_id: u32, channel: String, message: String },
    Disconnect { bot_id: u32 },
}
```
- Implement `ScenarioRunner::run(manifest: &ScenarioManifest) -> Result<ScenarioReport, ScenarioError>` driving `BotSession` instances over TCP against `manifest.gateway_addr`.
- Expose `--scenario <FILE>` in `crates/loadtest/src/main.rs` so `mmo-loadtest --scenario <path.json>` executes the scenario and exits with `0` on pass or `1` on failure.
