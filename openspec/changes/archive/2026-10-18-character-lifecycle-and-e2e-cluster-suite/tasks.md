# Implementation Tasks: Session Hydration Lifecycle, K8s Hardening & Scenario E2E Runner (Issues #81, #85)

- [x] 1. Player Session Hydration & Disconnect Snapshot Flush (`crates/node/src/sim.rs`)
  - [x] 1.1 Implement `hydrate_player_session` and `disconnect_player_session` on `TickSimulation` in `crates/node/src/sim.rs`, restoring position, rotation, and serialized components on login and capturing/enqueueing a final `EntitySnapshot` + despawning on disconnect.
  - [x] 1.2 Add unit tests in `crates/node/src/sim.rs` verifying full login hydration -> movement -> disconnect snapshot capture -> re-hydration roundtrip.

- [x] 2. Kubernetes StatefulSet PVC, PodSecurity & PDB Hardening (`k8s/base/`)
  - [x] 2.1 Convert `k8s/base/postgres.yaml` to a `StatefulSet` with `volumeClaimTemplates` (`10Gi` PVC), `secretKeyRef` credentials, and non-root `securityContext`.
  - [x] 2.2 Add hardened `securityContext` (`runAsNonRoot: true`, `allowPrivilegeEscalation: false`, `readOnlyRootFilesystem: true`, `capabilities: { drop: ["ALL"] }`) to `director.yaml`, `node-statefulset.yaml`, `gateway.yaml`, and `loadtest.yaml`, and add `PodDisruptionBudget` manifests in `k8s/base/pdb.yaml` + `kustomization.yaml`.

- [x] 3. Declarative Scenario-Driven E2E Test Runner (`crates/loadtest`)
  - [x] 3.1 Create `crates/loadtest/src/scenario.rs` defining `ScenarioManifest`, `ScenarioStep`, `ScenarioReport`, and `ScenarioRunner` driving `BotSession` instances.
  - [x] 3.2 Add `--scenario <Option<PathBuf>>` CLI option to `crates/loadtest/src/main.rs` and export `scenario` in `crates/loadtest/src/lib.rs`.
  - [x] 3.3 Add an end-to-end integration test in `crates/loadtest` spinning up a live `GatewayServer` + `NodeServiceImpl` and executing a multi-bot `ScenarioManifest` verifying authentication, movement, ping/pong RTT, AoI replication, and disconnect.

- [x] 4. Workspace Quality Gates
  - [x] 4.1 Run `cargo fmt --all -- --check`
  - [x] 4.2 Run `cargo clippy --workspace --all-targets -- -D warnings`
  - [x] 4.3 Run `cargo test --workspace`
