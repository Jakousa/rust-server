# Contributing to the Distributed Rust MMO Platform

Thank you for contributing to the Distributed Rust MMO Server Platform! This document outlines our engineering standards, architectural invariants, and development workflows.

---

## 1. Core Architectural Invariants

Every pull request must strictly uphold the following core system invariants (see also [`AGENTS.md`](AGENTS.md)):

1. **Genre-Agnostic Core**:
   - Core engine crates (`mmo-core`, `mmo-node`, `mmo-schema`, `mmo-storage`, `mmo-gateway`, `mmo-director`) must remain strictly genre-agnostic and mathematically generalized.
   - All game- or genre-specific mechanics belong in [`crates/gameplay`](crates/gameplay) or modular plugin crates under `crates/plugins/*` (such as [`crates/plugins/voxel`](crates/plugins/voxel)).

2. **Server Authority & Anti-Cheat**:
   - *Never trust client $\Delta t$ or coordinates*. Elapsed time ($\Delta t$) must always be calculated on the server using monotonic clocks.
   - Kinematic displacement must be validated against speed limits with a small tolerance for network jitter before state changes are accepted.
   - Actions (spell casts, interactions, channeling) must abort immediately upon non-zero displacement when configured with `cancel_on_movement`.

3. **Zero Server Pollution**:
   - Client-specific crates like [`crates/client-wasm`](crates/client-wasm) and frontend assets in [`web/`](web/) are strict leaf packages.
   - Core server crates (`mmo-core`, `mmo-node`, `mmo-director`, `mmo-gateway`, `mmo-storage`, `mmo-gameplay`, `mmo-plugin-voxel`) must **never** depend on `mmo-client-wasm` or browser WebAssembly runtime packages (`wasm-bindgen`, `web-sys`, `js-sys`).

4. **Zero Duplication**:
   - Packet framing, bit-packing, varints, and quantization codecs live exclusively in [`crates/schema`](crates/schema).
   - The WebAssembly client imports `mmo-schema` directly to guarantee single-source-of-truth parsing across native servers and web clients.

5. **Non-Blocking Hot Paths**:
   - The 20 Hz simulation tick loop and edge gateway routing paths must never perform blocking disk I/O, synchronous network calls, or hold mutexes across `.await` points.
   - Database operations use asynchronous write-behind queues; cross-node RPCs execute asynchronously with speculative cooldowns and compensation pipelines.

---

## 2. Engineering Workflow (OpenSpec 2.0 Pro Workflow)

All non-trivial architectural enhancements and new features follow the 5-stage OpenSpec 2.0 Pro Workflow (see [`.agents/skills/openspec-pro-workflow/SKILL.md`](.agents/skills/openspec-pro-workflow/SKILL.md)):

1. **Stage 1 (OpenSpec Proposal & Specs)**:
   - Scaffold a change with `openspec new change "<change-name>"` and author `proposal.md`, capability delta specs in `specs/`, `design.md`, and granular `tasks.md`.
2. **Stage 2 (Pro Architectural Review)**:
   - Run an architectural review evaluating concurrency, anti-cheat, edge cases, zero-pollution invariants, and testability.
3. **Stage 3 (Review Refinement)**:
   - Refine design and tasks based on review feedback and validate with `openspec validate "<change-name>"`.
4. **Stage 4 (Verified Implementation)**:
   - Implement tasks sequentially on a dedicated feature branch (`feat/<change-name>`), verifying unit, property, and integration tests.
5. **Stage 5 (Quality Gates, Archive & PR)**:
   - Pass all quality gates, sync delta specs to `openspec/specs/` and archive the change to `openspec/changes/archive/`, verify `openspec validate --all`, and submit a PR with squash auto-merge enabled.

---

## 3. Local Development & Quality Gates

Before opening a pull request, ensure all quality gates pass locally without warnings:

```bash
# 1. Code formatting
cargo fmt --all -- --check

# 2. Strict Clippy linter
cargo clippy --workspace --all-targets -- -D warnings

# 3. Full Rust test suite (all unit, property, and integration tests)
cargo test --workspace

# 4. Web inspector unit tests & production bundle build (if web/ or client-wasm modified)
npm --prefix web test -- --run
npm --prefix web run build

# 5. OpenSpec 2.0 specification validation
openspec validate --all
```

---

## 4. Cluster Testing & Chaos Operations

To verify cluster-wide distributed behaviors:

```bash
# Provision local Kubernetes cluster (k3d) via declarative config (k8s/k3d.yaml)
./scripts/cluster.sh up

# Build and deploy all cluster components
./scripts/cluster.sh all

# Run bot swarm load test (1,000 bots, 10 seconds, hotspot distribution)
./scripts/cluster.sh loadtest 1000 10 hotspot

# Run autonomous chaos kill & failover test
./scripts/cluster.sh chaos
```

---

## 5. Commit & Pull Request Guidelines

- Use conventional commits format: `feat(...)`, `fix(...)`, `refactor(...)`, `docs(...)`, `test(...)`, `chore(...)`.
- Include a summary of changes, architectural decisions made, and test output in pull request descriptions.
- Reference applicable OpenSpec change directories or roadmap issues.
