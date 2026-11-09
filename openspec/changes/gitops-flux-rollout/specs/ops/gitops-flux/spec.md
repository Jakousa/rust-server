# Specification: GitOps Continuous Delivery with Flux v2

## Capability: ops.gitops.flux

The cluster shall continuously reconcile its active state against the canonical git repository without manual intervention.

### Scenario: Continuous Git Reconciliation
- **GIVEN** a running Kubernetes cluster with Flux v2 installed in namespace `flux-system`
- **WHEN** commits are pushed to the `main` branch of `https://github.com/Jakousa/rust-server.git` affecting `k8s/overlays/cloud`
- **THEN** the `source-controller` detects the new commit within the configured reconciliation interval (`2m`)
- **AND** the `kustomize-controller` applies the generated manifests to the cluster using server-side apply
- **AND** existing untracked resources are pruned if removed from version control (`prune: true`).

### Scenario: Automated Image Tag Rollout
- **GIVEN** a successful image build and push to Docker Hub from GitHub Actions `docker-publish.yml`
- **WHEN** CI updates image tags in `k8s/overlays/cloud/kustomization.yaml` to `sha-<hash>` and pushes to `main`
- **THEN** Flux pulls the updated revision and triggers deployment rolling updates automatically.

### Scenario: Zero-Downtime Rolling Updates
- **GIVEN** existing replicas of `mmo-web`, `mmo-gateway`, and `mmo-director` serving traffic
- **WHEN** a new image tag or container spec is reconciled by Flux
- **THEN** Kubernetes updates the deployment using rolling update strategy (`maxUnavailable: 0`, `maxSurge: 1`)
- **AND** `mmo-web` validates HTTP readiness probes before old pods are decommissioned
- **AND** active client connections are never dropped prematurely.

### Scenario: Drift Detection and Self-Healing
- **GIVEN** an active deployment managed by Flux
- **WHEN** an operator manually alters deployment specs (e.g., replica count or image tag) directly via `kubectl`
- **THEN** Flux detects configuration drift during its next reconciliation loop
- **AND** restores the state defined in Git.
