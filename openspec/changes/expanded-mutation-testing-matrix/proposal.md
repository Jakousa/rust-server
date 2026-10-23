# Expand Nightly Mutation Testing Matrix to Core, Node, and Gameplay Critical Modules Proposal

## Context & Problem Statement
Currently, `.github/workflows/mutation-tests.yml` only executes `cargo-mutants` against `mmo-schema` and `mmo-storage`. While those two crates now achieve zero surviving mutants, several security-critical, anti-cheat, and economic modules in other workspace crates are not covered by mutation testing:
1. `crates/core/src/collision.rs` (3D raycast, swept AABB collision, and bounded A* pathfinding + string-pulling).
2. `crates/node/src/kinematics.rs` (server-authoritative speed/teleport anti-cheat validation, `DiscreteGround` & `Newtonian6DoF` motion integration, and impulse allowances).
3. `crates/gameplay/src/trade.rs` (atomic two-player item and currency trade swap invariants).

## Proposed Solution
1. **Expand `.github/workflows/mutation-tests.yml` Matrix**:
   - Extend the nightly `mutation-tests` workflow matrix and `workflow_dispatch` options to include targeted runs for:
     - `mmo-core` (`--file crates/core/src/collision.rs`)
     - `mmo-node` (`--file crates/node/src/kinematics.rs`)
     - `mmo-gameplay` (`--file crates/gameplay/src/trade.rs`)
     alongside full-crate `mmo-schema` and `mmo-storage`.
2. **Mutation-Killing Unit Tests (`collision.rs`, `kinematics.rs`, `trade.rs`)**:
   - Run `cargo mutants` against `crates/core/src/collision.rs`, `crates/node/src/kinematics.rs`, and `crates/gameplay/src/trade.rs`, and add boundary/exact-equality unit tests to kill any surviving mutants (such as `<` vs `<=` boundary comparisons, sign flips, or early-return mutations).

## Scope & Non-Goals
- **In Scope**: `.github/workflows/mutation-tests.yml`, `crates/core/src/collision.rs`, `crates/node/src/kinematics.rs`, and `crates/gameplay/src/trade.rs`.
- **Non-Goals**: Running `cargo-mutants` across non-deterministic network I/O loops on every PR.
