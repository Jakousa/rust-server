# Project Roadmap & Active Focus

This document outlines the current active work and future vision for the **Distributed Rust MMO Server Platform**.

Historical specifications, change proposals, and task manifests are permanently archived and tracked under [`openspec/changes/archive/`](changes/archive/). Detailed capability specifications (validated via `openspec validate --all`) live under [`openspec/specs/`](specs/).

---

## Active Focus: Cloud Deployment & Live Multi-Client Playtesting

Following the completion of the playable Village & Barter Vertical Slice ([`2026-11-07-village-barter-vertical-slice-poc`](changes/archive/2026-11-07-village-barter-vertical-slice-poc/)), automated GitOps Flux v2 delivery ([`2026-11-09-gitops-flux-rollout`](changes/archive/2026-11-09-gitops-flux-rollout/)), Director restart resilience ([`2026-11-13-node-director-registration-resilience`](changes/archive/2026-11-13-node-director-registration-resilience/)), WebSocket/TLS fallback ingress ([`2026-11-13-websocket-fallback-transport`](changes/archive/2026-11-13-websocket-fallback-transport/)), node split migration & topology-aware routing ([`2026-11-15-node-split-migration-and-routing`](changes/archive/2026-11-15-node-split-migration-and-routing/)), elastic cluster completion ([`2026-11-15-elastic-cluster-completion`](changes/archive/2026-11-15-elastic-cluster-completion/)), wire protocol stabilization & external client SDK contract ([`2026-11-15-wire-protocol-and-client-sdk`](changes/archive/2026-11-15-wire-protocol-and-client-sdk/)), Village & Barter demo polish ([`2026-11-15-village-demo-polish-and-docs`](changes/archive/2026-11-15-village-demo-polish-and-docs/)), and live cloud load & soak validation ([`2026-11-15-cloud-load-soak-validation`](changes/archive/2026-11-15-cloud-load-soak-validation/)):

| Focus Area | GitHub Issue(s) | Status | Target Crates / Artifacts | Summary |
| :--- | :---: | :---: | :--- | :--- |
| **Cloud VM & K3s GitOps Deployment** | [#143](https://github.com/Jakousa/rust-server/issues/143), [#153](https://github.com/Jakousa/rust-server/issues/153), [#188](https://github.com/Jakousa/rust-server/issues/188), [#189](https://github.com/Jakousa/rust-server/issues/189) | Live / Active | `docker-compose.yml`, `k8s/overlays/cloud/`, `k8s/flux-system/`, `crates/gateway`, `web/`, `docs/CLOUD_POC_DEPLOYMENT.md` | Automated CI image publishing (`jakousa/mmo-server`, `jakousa/mmo-web`), Flux v2 GitOps reconciliation on `deploy/cloud`, Let's Encrypt HTTPS ingress, WebTransport UDP `:4433` with `/cert-hash` pinning, and automatic WebSocket/TLS fallback (`ws://`/`wss://`) for restricted networks. |
| **Elastic Cluster Completion** | [#240](https://github.com/Jakousa/rust-server/issues/240) | Completed (`2026-11-15-elastic-cluster-completion`) | `crates/director`, `crates/node`, `crates/client-wasm`, `web/` | Automatic underloaded octree partition merging, graceful peer entity evacuation on node drain, cross-partition boundary ghost AoI replication, and cross-node tick rollover continuity. |
| **Wire Protocol & External Client SDK Contract** | [#241](https://github.com/Jakousa/rust-server/issues/241) | Completed (`2026-11-15-wire-protocol-and-client-sdk`) | `crates/schema`, `crates/client-wasm`, `docs/WIRE_PROTOCOL_AND_CLIENT_SDK.md`, `docs/GODOT_CLIENT_INTEGRATION.md` | Stabilized `0x4D4D` framing with `PROTOCOL_VERSION`, crate-root exports, golden wire byte-vector tests, SDK prediction/replica helpers, and Godot 4 integration docs. |
| **Village & Barter Demo Polish & Docs** | [#242](https://github.com/Jakousa/rust-server/issues/242) | Completed (`2026-11-15-village-demo-polish-and-docs`) | `web/`, `README.md`, `ARCHITECTURE.md` | HUD transport mode badge (`HTTP/3`/`WebSocket`/`Offline`), non-intrusive toast notifications, keycap onboarding card, editable-input focus guard, and updated showcase docs. |
| **Live Cloud Load & Soak Validation** | [#239](https://github.com/Jakousa/rust-server/issues/239) | Completed (`2026-11-15-cloud-load-soak-validation`) | `crates/loadtest`, `docs/CLOUD_POC_DEPLOYMENT.md` | Extended loadtest telemetry & jitter-safe handover detection, deterministic multi-node split/failover/persistence soak test suite, and live K3s benchmark report (25/65/120 bots). |
| **Dual-Game Architecture & Competitive 6DoF Voxel Asteroid Miners** | [#244](https://github.com/Jakousa/rust-server/issues/244) | Proposed ([`dual-game-voxel-asteroid-miners`](changes/dual-game-voxel-asteroid-miners/)) | `crates/gameplay`, `web/`, `Dockerfile`, `k8s/` | Second playable MMO vertical slice (**Game B: Competitive 6DoF Voxel Asteroid Miners** via `mmo-asteroid-node`) running alongside **Game A: Village RPG** (`mmo-village-node`) on the same core engine, combining `Newtonian6DoF` 3D flight, `VoxelTerrainPlugin` asteroid ore excavation, `CombatPlugin` full-loot PvP, dynamic `state_flags` ship shininess, and shared `InventoryPlugin` + `TradePlugin` orbital station banking/trading. |

---

## Upcoming Initiatives

1. **Multi-Region & Runtime Hardening Follow-Ups**: Expose `:8443` WebSocket fallback in K3s `mmo-gateway` service/ingress, enable `ObserverConfig` delta compression by default on cluster nodes, review snapshot version ordering across migrations (`entity_snapshots`), and expand to multi-region simulation node scaling.
2. **Dedicated Godot Client (Separate Repository)**: Standalone native Godot client repository demonstrating multi-client connectivity to the authoritative cluster against the stabilized `wire-protocol-contract`.

---

## Engineering Workflow for Changes

1. **Propose New Work**: Scaffold and plan non-trivial features using OpenSpec 2.0 (`openspec new change "<name>"` via `openspec-propose` or `openspec-pro-workflow`).
2. **Execute via Branch**: Implement on a dedicated `feat/<change-name>` branch adhering strictly to `design.md` and `tasks.md` (`openspec-apply-change`).
3. **Quality Gates**: Ensure `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `npm --prefix web test -- --run`, and `openspec validate --all` pass with zero warnings.
4. **Verify, Sync Specs & Archive**: Validate implementation with `openspec-verify-change`, sync delta specs to `openspec/specs/` and archive completed changes to `openspec/changes/archive/` via `openspec-archive-change`, and open a PR with squash auto-merge enabled.
