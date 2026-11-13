# Engineering & Agent Architecture Guidelines (`AGENTS.md`)

This document serves as the authoritative operational manual and architectural constraint guide for all AI coding agents working on this codebase.

---

## 1. Prime Directive: Unopinionated, Genre-Agnostic MMO Engine

1. **Zero Game-Specific Assumptions in Core**:
   * The core engine crates:
     - [`crates/core`](crates/core) (`mmo-core`)
     - [`crates/node`](crates/node) (`mmo-node`)
     - [`crates/schema`](crates/schema) (`mmo-schema`)
     - [`crates/storage`](crates/storage) (`mmo-storage`)
     - [`crates/gateway`](crates/gateway) (`mmo-gateway`)
     - [`crates/director`](crates/director) (`mmo-director`)
   * MUST remain strictly **genre-agnostic, unopinionated, and mathematically generalized**.
   * DO NOT hardcode fantasy RPG mechanics, space flight rules, survival crafting formulas, or voxel sandbox block arrays into core primitives.

2. **Decoupled Plugin & Leaf-Crate Architecture**:
   * ALL game-specific, genre-specific, and design-specific mechanics belong in:
     - [`crates/gameplay`](crates/gameplay) (`mmo-gameplay`) via the [`GameplayPlugin`](crates/core/src/plugin.rs) trait, OR
     - Dedicated modular extension crates under `crates/plugins/*` (e.g. [`crates/plugins/voxel`](crates/plugins/voxel) / `mmo-plugin-voxel`).
   * Plugins hook into the simulation tick via typed event queues, veto hooks, dynamic component stores, and modular packet handlers.
   * Client-specific crates ([`crates/client-wasm`](crates/client-wasm)) and browser assets ([`web/`](web/)) are strict leaf packages. Core server crates must never depend on `mmo-client-wasm` or WASM browser bindings.

3. **Core Primitives Are Generalized Substrates**:
   * Spatial indexing uses generalized 3D bounding boxes, collision traits, navgrids, and octree spatial domains.
   * Actions use generalized `TimedAction` state machines with interruption triggers.
   * Persistence uses generic containers with 64-bit GUIDs, access control policies, and 2PC transfer coordinators.
   * World persistence uses generalized spatial sectors, heightmaps, and dynamic entity/prefab instances.

---

## 2. Local Subagent Integration

To preserve cloud model context and conserve API tokens, agents should utilize available local models or specialized subagents whenever configured in the environment.

### What to Delegate to Subagents
* **Boilerplate & Structs**: Writing field definitions, getters, conversion traits (`From`/`Into`), and builders.
* **Codecs & Serializers**: Binary encoding/decoding implementations and packet match arms.
* **Database Queries**: SQL repository methods and migration scripts.
* **Unit & Property Tests**: Writing comprehensive edge-case test suites, boundary tests, and mutation-killing tests.

Keep the primary orchestrator focused on system architecture, OpenSpec planning, cross-crate integration, and quality gate verification.

---

## 3. Engineering Workflows & Quality Gates

1. **Master Roadmap & OpenSpec 2.0 Lifecycle (`openspec/ROADMAP.md`)**:
   * The repository maintains its active focus and upcoming initiatives in [`openspec/ROADMAP.md`](openspec/ROADMAP.md), with all completed changes archived under [`openspec/changes/archive/`](openspec/changes/archive/) and durable capability specifications under [`openspec/specs/`](openspec/specs/).
   * The standalone OpenSpec 2.0 CLI (`openspec`) is installed and configured with project rules in [`openspec/config.yaml`](openspec/config.yaml).
   * Always scaffold new changes via `openspec new change "<change-name>"` (using [`openspec-propose`](.agents/skills/openspec-propose/SKILL.md) or [`openspec-pro-workflow`](.agents/skills/openspec-pro-workflow/SKILL.md)). Never hand-create `openspec/changes/<name>` directories without CLI metadata.
   * Execute implementation tasks via [`openspec-apply-change`](.agents/skills/openspec-apply-change/SKILL.md), verify coherence via [`openspec-verify-change`](.agents/skills/openspec-verify-change/SKILL.md), and sync specs + archive completed changes via [`openspec-archive-change`](.agents/skills/openspec-archive-change/SKILL.md).

2. **Strict CI Quality Gates**:
   * Rust formatting: `cargo fmt --all -- --check`
   * Strict Rust linter: `cargo clippy --workspace --all-targets -- -D warnings` (zero warnings allowed)
   * Workspace test suite: `cargo test --workspace`
   * Web frontend tests & build (when modifying `web/` or `crates/client-wasm`): `npm --prefix web test -- --run` and `npm --prefix web run build`
   * OpenSpec specification validation: `openspec validate --all` (zero errors or warnings allowed)
   * Never commit directly to `main`. Create a feature branch, open a PR, and enable auto-merge with squash.
