# Project Roadmap & Active Focus

This document outlines the current active work and future vision for the **Distributed Rust MMO Server Platform**.

Historical specifications, change proposals, and task manifests are permanently archived and tracked under [`openspec/changes/archive/`](changes/archive/). Detailed capability specifications live under [`openspec/specs/`](specs/).

---

## Active Focus: Playable Village & Barter Vertical Slice

The current focus is connecting the foundational engine crates into the first end-to-end playable browser vertical slice:

| Focus Area | OpenSpec Change | GitHub Issue(s) | Status | Target Crates | Summary |
| :--- | :--- | :---: | :---: | :--- | :--- |
| **Playable Vertical Slice** | [`village-barter-vertical-slice-poc`](changes/village-barter-vertical-slice-poc/) | [#143](https://github.com/Jakousa/rust-server/issues/143), [#144](https://github.com/Jakousa/rust-server/issues/144) | In Progress | `mmo-gameplay`, `mmo-node`, `mmo-gateway`, `mmo-core`, `mmo-client-wasm`, `web/` | First end-to-end playable slice: 3 NPC villagers (Blacksmith Ilmari, Herbalist Aino, Elder Väinö), village structures, starter inventories, atomic NPC barter & player-to-player trade, WebTransport listener with auto cert-hash discovery, and browser Village HUD. |

---

## Upcoming Initiatives

1. **Cloud VM Deployment & Live Testing**: Deploy the vertical slice to a public cloud VM (using the guide in [`docs/CLOUD_POC_DEPLOYMENT.md`](../docs/CLOUD_POC_DEPLOYMENT.md)) for multi-client public browser verification.
2. **Voxel FPS Arena Prototype**: Second distinct game genre testing high-frequency input prediction, voxel chunk destruction/building, and fast projectile combat on the same engine substrate.
3. **Dedicated Godot Client (Separate Repository)**: Standalone native Godot client repository demonstrating multi-client connectivity to the authoritative cluster.

---

## Engineering Workflow for Changes

1. **Propose New Work**: Plan non-trivial features using OpenSpec (`openspec-propose` or `openspec-pro-workflow`).
2. **Execute via Branch**: Implement on a dedicated `feat/<change-name>` branch adhering strictly to `design.md` and `tasks.md`.
3. **Quality Gates**: Ensure `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` pass with zero warnings.
4. **Sync Specs & PR Auto-Merge**: Merge specs to `openspec/specs/`, move completed changes to `openspec/changes/archive/`, and open PR with auto-merge enabled.
