# Session Character Hydration/Logout Lifecycle, K8s Production Hardening & Scenario-Driven E2E Cluster Runner Proposal

## Context & Problem Statement
To bring the platform to full production fruition across live deployments and automated CI verification (Issues #81 & #85 + Gaps 5 & 7), three final end-to-end pillars are needed:

1. **Session Hydration on Login & Final Snapshot Flush on Logout (Gap 5)**:
   - Currently, when a client authenticates at `mmo-gateway`, the Gateway maps the session route in `SessionRouter` and returns `AuthResponse { success: true }`, without instructing the target `mmo-node` to load the player's saved `EntitySnapshot` / `CharacterRecord` from `StorageRepository` and register the session observer.
   - When a client disconnects (`SessionDropGuard::drop`), the Gateway removes the route locally without notifying `mmo-node` to capture a final snapshot, flush it to storage, and despawn the in-world entity.

2. **Kubernetes Production Persistence & Pod Security Hardening (Issue #85)**:
   - `k8s/base/postgres.yaml` uses `kind: Deployment` with `emptyDir: {}` (`lines 72-74`), wiping all persistent world and character state whenever the Postgres pod restarts, and hardcodes plaintext credentials in the deployment spec.
   - Workloads under `k8s/base/` lack hardened `securityContext` (`runAsNonRoot: true`, `allowPrivilegeEscalation: false`, `capabilities: { drop: ["ALL"] }`) and `PodDisruptionBudget` manifests.

3. **Scenario-Driven E2E Cluster Test Runner (Issue #81 & Gap 7)**:
   - `crates/loadtest` currently only runs random/hotspot movement swarms without deterministic step assertions, cross-node boundary handoff checks, or snapshot persistence verification.

## Proposed Solution

1. **Player Session Hydration & Logout Lifecycle (`crates/node` & `crates/gateway`)**:
   - Add `hydrate_player_session` and `disconnect_player_session` methods on `TickSimulation` / `NodeServiceImpl` (and wire via `SpawnEntity` / `ForwardCommand` or dedicated `NodeService` handlers) so that:
     - On login, the node loads any saved `EntitySnapshot` from `StorageRepository` (restoring coordinates, rotation, and serialized components via `ComponentCodecRegistry`), spawns the entity in `EntityStore` and `Octree`, and registers the AoI observer.
     - On disconnect, the node captures the entity's latest `EntitySnapshot`, enqueues/flushes it to `WriteBehindSnapshotter` / `StorageRepository`, and cleanly despawns the entity from `EntityStore`, `Octree`, and `AoiManager`.

2. **Kubernetes StatefulSet PVC, PodSecurity & PDB Hardening (`k8s/base/`)**:
   - Convert `k8s/base/postgres.yaml` from `Deployment` + `emptyDir` to a `StatefulSet` with `volumeClaimTemplates` (`10Gi` `ReadWriteOnce` PVC), secret references (`secretKeyRef`), and hardened `securityContext`.
   - Add `securityContext` (`runAsNonRoot: true`, `allowPrivilegeEscalation: false`, `readOnlyRootFilesystem: true`, `capabilities: { drop: ["ALL"] }`) and `PodDisruptionBudget` resources across `k8s/base/{director.yaml, node-statefulset.yaml, gateway.yaml, loadtest.yaml}`.

3. **Declarative Scenario Runner in `crates/loadtest/src/scenario.rs` (Issue #81)**:
   - Implement a declarative scenario runner (`ScenarioManifest`, `ScenarioStep`, `ScenarioRunner`) supporting YAML/JSON/TOML or programmatic scenario definitions (`Connect`, `Authenticate`, `MoveTo`, `WaitTicks`, `AssertAoiEntity`, `Chat`, `Disconnect`) with SLA assertions (`max_p99_rtt_ms`, `zero_decode_errors`) and CLI subcommand/flag `--scenario <path>` in `mmo-loadtest`.

## Scope & Non-Goals
- **In Scope**: `crates/node/src/{sim.rs, network.rs}`, `crates/gateway/src/server.rs`, `crates/loadtest/src/{lib.rs, main.rs, scenario.rs}`, and `k8s/base/*.yaml`.
- **Non-Goals**: External cloud-provider-specific Terraform scripts.
