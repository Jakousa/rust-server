# testing/mutation-testing Specification

## Purpose
Ensures maximum test suite effectiveness and test-case rigor by verifying that code mutations introduced by `cargo-mutants` are reliably caught by automated tests, with 0 surviving mutants in CI pipelines.

## Requirements

### Requirement: Zero Surviving Mutants in Schema and Storage Suites
The nightly mutation test workflow (`mutation-tests.yml`) SHALL test crates `mmo-schema` and `mmo-storage` without any missed mutants surviving.

#### Scenario: Codec and boundary mutation testing in mmo-schema
- **WHEN** mutations are introduced into packet decoders, encoders, or enum converters in `mmo-schema`
- **THEN** automated unit tests SHALL detect the mutation and cause test failure, achieving 0 missed mutants.

#### Scenario: Headless unit test execution in mmo-storage
- **WHEN** mutation testing runs in headless unit test environments with SQLite in-memory databases
- **THEN** unexercised external service backend code paths (e.g. `DbBackend::Postgres`) SHALL be declaratively excluded in `.cargo/mutants.toml`
- **AND** all core storage coordination and datastore operations SHALL catch 100% of generated mutants.
