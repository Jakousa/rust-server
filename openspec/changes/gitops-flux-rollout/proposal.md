# Proposal: GitOps Automated Cluster Rollout with Flux v2 (Issue #188)

## Context & Problem Statement
Currently, container images (`jakousa/mmo-server` and `jakousa/mmo-web`) are automatically built and published to Docker Hub upon merges to `main` via `.github/workflows/docker-publish.yml`. However, updating the running Kubernetes cluster (`49.13.149.126`) requires manual operational intervention (`kubectl rollout restart` or manual `kubectl apply`).

Furthermore, manual cluster interventions suffer from configuration drift: changes applied imperatively to the cluster diverge from version control, making rollbacks error-prone and disaster recovery slow.

## Proposed Solution
Deploy a minimal, lightweight **Flux v2** GitOps architecture on the cluster:
1. **Lightweight Controller Footprint**: Install only `source-controller` and `kustomize-controller` in the `flux-system` namespace, maintaining memory usage below 120 MiB (<4% of node capacity).
2. **Declarative Synchronization**: Establish a `GitRepository` source watching `https://github.com/Jakousa/rust-server.git` (`main` branch) and a `Kustomization` reconciliation object targeting `k8s/overlays/cloud`.
3. **Automated Rollout Pipeline**:
   - Update `.github/workflows/docker-publish.yml` to tag published images with the git commit SHA (`sha-<hash>`).
   - Enable automated synchronization either via short-interval polling (1m) or webhook receiver, ensuring zero manual `kubectl` intervention for cluster updates.
   - Enforce zero-downtime rolling update healthcheck gates (readiness probes, pod disruption budgets) so rollouts safely drain and replace replicas without dropping active MMO player sessions.

## Scope
- Flux v2 minimal installation manifests in `k8s/flux-system/` or `k8s/clusters/cloud/`.
- GitRepository and Kustomization definitions for syncing `k8s/overlays/cloud`.
- Updates to CI workflow `.github/workflows/docker-publish.yml` for GitOps image tagging/rollout triggers.
- Verification on remote k3s cluster (`49.13.149.126`).

## Non-Goals
- Deploying heavyweight UI dashboards (ArgoCD or Flagger can be evaluated later if progressive traffic canarying is required).
- Modifying game server simulation logic or packet wire protocols in core crates.
