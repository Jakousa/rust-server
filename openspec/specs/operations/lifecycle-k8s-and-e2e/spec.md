# Session Hydration Lifecycle, K8s Hardening, and Scenario E2E Testing Specification

## Purpose
Establishes end-to-end player entity hydration on login and snapshot persistence on disconnect, hardens Kubernetes manifests with persistent volumes and non-root security contexts, and provides a declarative scenario-driven E2E test runner.

## Requirements

### Requirement: Persistent Session Hydration on Login and Snapshot Flush on Disconnect
When a player session joins a simulation node, the node SHALL hydrate the player's saved `EntitySnapshot` (position, rotation, dynamic components) from storage if present, spawn the entity in `EntityStore` and `Octree`, and register the AoI observer. When the session disconnects, the node SHALL capture and flush the entity's latest snapshot and despawn the entity from the world.

#### Scenario: Player moves, disconnects, and reconnects at saved coordinates
- **GIVEN** a player session hydrated at `(0.0, 0.0, 0.0)` who moves to `(25.0, 0.0, -15.0)`
- **WHEN** the player disconnects and later reconnects with the same GUID
- **THEN** the disconnect SHALL persist an `EntitySnapshot` at `(25.0, 0.0, -15.0)` and despawn the active entity, and reconnecting SHALL hydrate the entity at `(25.0, 0.0, -15.0)`

### Requirement: Kubernetes StatefulSet PVC Persistence and Hardened Pod Security
`k8s/base/postgres.yaml` SHALL define a `StatefulSet` with `volumeClaimTemplates` (`PersistentVolumeClaim`) rather than `emptyDir: {}`, reference credentials via `secretKeyRef`, and all manifests in `k8s/base/` SHALL enforce non-root `securityContext` (`runAsNonRoot: true`, `allowPrivilegeEscalation: false`, dropped capabilities) and define `PodDisruptionBudget` policies.

#### Scenario: Validating Kubernetes manifests for persistence and security
- **GIVEN** the Kubernetes manifests under `k8s/base/`
- **WHEN** inspected for storage and security policies
- **THEN** `postgres.yaml` SHALL use `kind: StatefulSet` with `volumeClaimTemplates` and zero `emptyDir` database volumes, and all workloads SHALL specify `runAsNonRoot: true` and `allowPrivilegeEscalation: false`

### Requirement: Declarative Scenario-Driven E2E Test Runner
`crates/loadtest` SHALL provide a `ScenarioRunner` capable of executing structured multi-step scenarios (`Connect`, `Authenticate`, `Move`, `WaitTicks`, `AssertAoiEntity`, `Disconnect`) against a live or in-process gateway/node cluster and returning non-zero exit status on assertion or SLA failure.

#### Scenario: Executing player lifecycle and AoI assertion scenario
- **GIVEN** a declarative scenario connecting two bots to a gateway/node cluster, moving them within AoI range, and asserting mutual visibility
- **WHEN** `ScenarioRunner::execute` runs the scenario
- **THEN** all steps SHALL pass with zero packet decode errors and verify AoI entity replication
