# Project Roadmap & Active Focus

This document outlines the current active work and future vision for the **Distributed Rust MMO Server Platform**.

Historical specifications, change proposals, and task manifests are permanently archived and tracked under [`openspec/changes/archive/`](changes/archive/). Detailed capability specifications live under [`openspec/specs/`](specs/).

---

## Active Focus: Cloud VM Deployment & Public Playtest

With the playable Village & Barter Vertical Slice completed and archived ([`2026-11-07-village-barter-vertical-slice-poc`](changes/archive/2026-11-07-village-barter-vertical-slice-poc/)), the active focus is live deployment and cluster playtesting:

| Focus Area | GitHub Issue(s) | Status | Target Crates / Artifacts | Summary |
| :--- | :---: | :---: | :--- | :--- |
| **Cloud VM Deployment & Live Testing** | [#143](https://github.com/Jakousa/rust-server/issues/143), [#153](https://github.com/Jakousa/rust-server/issues/153) | Active | `docker-compose.yml`, `web/`, `crates/gameplay`, `docs/CLOUD_POC_DEPLOYMENT.md` | Single-command deployment (`docker compose up -d`) on an inexpensive cloud VM (Hetzner/DO/AWS), serving the Three.js web client on port 3000 and gateway WebTransport over UDP 4433 with auto cert-hash discovery for multi-client playtesting. |

---

## Upcoming Initiatives

1. **Voxel FPS Arena Prototype**: Second distinct game genre testing the engine substrate's genre-agnostic claim via high-frequency input prediction, destructible voxel terrain (`crates/plugins/voxel`), and fast projectile combat.
2. **Dedicated Godot Client (Separate Repository)**: Standalone native Godot client repository demonstrating multi-client connectivity to the authoritative cluster.
3. **Cluster Auto-Scaling & Dynamic Partitioning**: Director-driven live partition boundary migration and load rebalancing across multi-region simulation nodes.

---

## Engineering Workflow for Changes

1. **Propose New Work**: Plan non-trivial features using OpenSpec (`openspec-propose` or `openspec-pro-workflow`).
2. **Execute via Branch**: Implement on a dedicated `feat/<change-name>` branch adhering strictly to `design.md` and `tasks.md`.
3. **Quality Gates**: Ensure `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` pass with zero warnings.
4. **Sync Specs & PR Auto-Merge**: Merge specs to `openspec/specs/`, move completed changes to `openspec/changes/archive/`, and open PR with auto-merge enabled.
