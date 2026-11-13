# Project Roadmap & Active Focus

This document outlines the current active work and future vision for the **Distributed Rust MMO Server Platform**.

Historical specifications, change proposals, and task manifests are permanently archived and tracked under [`openspec/changes/archive/`](changes/archive/). Detailed capability specifications (validated via `openspec validate --all`) live under [`openspec/specs/`](specs/).

---

## Active Focus: Cloud Deployment & Live Multi-Client Playtesting

Following the completion of the playable Village & Barter Vertical Slice ([`2026-11-07-village-barter-vertical-slice-poc`](changes/archive/2026-11-07-village-barter-vertical-slice-poc/)), automated GitOps Flux v2 delivery ([`2026-11-09-gitops-flux-rollout`](changes/archive/2026-11-09-gitops-flux-rollout/)), Director restart resilience ([`2026-11-13-node-director-registration-resilience`](changes/archive/2026-11-13-node-director-registration-resilience/)), and WebSocket/TLS fallback ingress ([`2026-11-13-websocket-fallback-transport`](changes/archive/2026-11-13-websocket-fallback-transport/)):

| Focus Area | GitHub Issue(s) | Status | Target Crates / Artifacts | Summary |
| :--- | :---: | :---: | :--- | :--- |
| **Cloud VM & K3s GitOps Deployment** | [#143](https://github.com/Jakousa/rust-server/issues/143), [#153](https://github.com/Jakousa/rust-server/issues/153), [#188](https://github.com/Jakousa/rust-server/issues/188), [#189](https://github.com/Jakousa/rust-server/issues/189) | Live / Active | `docker-compose.yml`, `k8s/overlays/cloud/`, `k8s/flux-system/`, `crates/gateway`, `web/`, `docs/CLOUD_POC_DEPLOYMENT.md` | Automated CI image publishing (`jakousa/mmo-server`, `jakousa/mmo-web`), Flux v2 GitOps reconciliation on `deploy/cloud`, Let's Encrypt HTTPS ingress, WebTransport UDP `:4433` with `/cert-hash` pinning, and automatic WebSocket/TLS fallback (`ws://`/`wss://`) for restricted networks. |

---

## Upcoming Initiatives

1. **Voxel FPS Arena Prototype**: Second distinct game genre testing the engine substrate's genre-agnostic claim via high-frequency input prediction, destructible voxel terrain ([`crates/plugins/voxel`](../crates/plugins/voxel)), and fast projectile combat.
2. **Dedicated Godot Client (Separate Repository)**: Standalone native Godot client repository demonstrating multi-client connectivity to the authoritative cluster.
3. **Cluster Auto-Scaling & Dynamic Partitioning**: Director-driven live partition boundary migration and load rebalancing across multi-region simulation nodes.

---

## Engineering Workflow for Changes

1. **Propose New Work**: Scaffold and plan non-trivial features using OpenSpec 2.0 (`openspec new change "<name>"` via `openspec-propose` or `openspec-pro-workflow`).
2. **Execute via Branch**: Implement on a dedicated `feat/<change-name>` branch adhering strictly to `design.md` and `tasks.md` (`openspec-apply-change`).
3. **Quality Gates**: Ensure `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `npm --prefix web test -- --run`, and `openspec validate --all` pass with zero warnings.
4. **Verify, Sync Specs & Archive**: Validate implementation with `openspec-verify-change`, sync delta specs to `openspec/specs/` and archive completed changes to `openspec/changes/archive/` via `openspec-archive-change`, and open a PR with squash auto-merge enabled.
