# Expanded Nightly Mutation Testing Matrix Specification

## Purpose
Extends automated mutation testing to cover collision/pathfinding (`mmo-core`), kinematic anti-cheat (`mmo-node`), and atomic player trading (`mmo-gameplay`) with zero surviving mutants.

## Requirements

### Requirement: Expanded Nightly Mutation Matrix
`.github/workflows/mutation-tests.yml` SHALL include matrix targets for `mmo-schema`, `mmo-storage`, `mmo-core` (`crates/core/src/collision.rs`), `mmo-node` (`crates/node/src/kinematics.rs`), and `mmo-gameplay` (`crates/gameplay/src/trade.rs`).

#### Scenario: Running cargo-mutants on collision, kinematics, and trade modules
- **GIVEN** the target files `crates/core/src/collision.rs`, `crates/node/src/kinematics.rs`, and `crates/gameplay/src/trade.rs`
- **WHEN** `cargo mutants` is executed against each module's unit tests (`-- --lib`)
- **THEN** all viable mutants SHALL be caught with zero missed mutants
