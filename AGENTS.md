# Engineering & Agent Architecture Guidelines (`AGENTS.md`)

This document serves as the authoritative operational manual and architectural constraint guide for all AI coding agents working on this codebase.

---

## 1. Prime Directive: Unopinionated, Genre-Agnostic MMO Engine

1. **Zero Game-Specific Assumptions in Core**:
   * The core engine crates:
     - [`crates/core`](crates/core)
     - [`crates/node`](crates/node)
     - [`crates/schema`](crates/schema)
     - [`crates/storage`](crates/storage)
     - [`crates/gateway`](crates/gateway)
     - [`crates/director`](crates/director)
   * MUST remain strictly **genre-agnostic, unopinionated, and mathematically generalized**.
   * DO NOT hardcode fantasy RPG mechanics, space flight rules, survival crafting formulas, or voxel sandbox block arrays into core primitives.

2. **Decoupled Plugin Architecture**:
   * ALL game-specific, genre-specific, and design-specific mechanics belong in:
     - [`crates/gameplay`](crates/gameplay) via the [`GameplayPlugin`](crates/core/src/plugin.rs) trait, OR
     - Dedicated modular extension crates (e.g. `crates/plugins/*`).
   * Plugins hook into the simulation tick via typed event queues, veto hooks, dynamic component stores, and modular packet handlers.

3. **Core Primitives Are Generalized Substrates**:
   * Spatial indexing uses generalized 3D bounding boxes and octree spatial domains.
   * Actions use generalized `TimedAction` state machines with interruption triggers.
   * Persistence uses generic containers with 64-bit GUIDs, access control policies, and 2PC transfer coordinators.
   * World persistence uses generalized spatial sectors, heightmaps, and dynamic entity/prefab instances.

---

## 2. Local Subagent Integration

To preserve cloud model context and conserve API tokens, agents should utilize available local models or specialized subagents whenever configured in the environment.

### What to Delegate to Local Subagents
* **Boilerplate & Structs**: Writing field definitions, getters, conversion traits (`From`/`Into`), and builders.
* **Codecs & Serializers**: Binary encoding/decoding implementations and packet match arms.
* **Database Queries**: SQL repository methods and migration scripts.
* **Unit & Property Tests**: Writing comprehensive edge-case test suites, boundary tests, and mutation-killing tests.

Keep the primary orchestrator focused on system architecture, OpenSpec planning, cross-crate integration, and quality gate verification.

---

## 3. Engineering Workflows & Quality Gates

1. **Master Roadmap & Pre-Architected OpenSpec Bundles (`openspec/ROADMAP.md`)**:
   * The repository maintains an ordered Master Execution Roadmap in [`openspec/ROADMAP.md`](openspec/ROADMAP.md). All 10 initial groundwork bundles have been fully implemented, verified, and archived under `openspec/changes/archive/`.
   * When groundwork bundles exist under `openspec/changes/<bundle-name>/`, implementation agents must consult [`openspec/ROADMAP.md`](openspec/ROADMAP.md) and execute them using the [`openspec-bundle-executor`](.agents/skills/openspec-bundle-executor/SKILL.md) skill without architectural drift.
   * When `openspec/changes/` contains no active bundles, new changes must be proposed via the standard OpenSpec lifecycle (`openspec-propose` or `openspec-pro-workflow`).

2. **OpenSpec Lifecycle**:
   * All non-trivial changes must be planned via OpenSpec (`openspec/changes/<change-name>/`).
   * Pro model architectural review must validate concurrency, security, and edge cases before implementation.
   * Completed changes must be synced to main specs (`openspec/specs/`) and archived in `openspec/changes/archive/`.

3. **Strict CI Quality Gates**:
   * Code formatting: `cargo fmt --all -- --check`
   * Strict linter: `cargo clippy --workspace --all-targets -- -D warnings` (zero warnings allowed)
   * Test suite: `cargo test --workspace`
   * Never commit directly to `main`. Create a PR and enable auto-merge with squash.
