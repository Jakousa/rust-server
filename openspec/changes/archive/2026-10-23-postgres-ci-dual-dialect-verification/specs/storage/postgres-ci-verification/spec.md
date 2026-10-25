# Live PostgreSQL CI and Dual-Dialect Storage Verification Specification

## Purpose
Ensures all storage repositories, 2PC container transfers, escrow trades, marketplace operations, and world persistence queries are verified against both SQLite and a live PostgreSQL instance in Continuous Integration.

## Requirements

### Requirement: Dual-Dialect Storage Test Harness
`crates/storage` SHALL provide a test helper capable of provisioning both an in-memory SQLite `Database` and, when `TEST_POSTGRES_URL` is set, a migrated and isolated PostgreSQL `Database`. When `REQUIRE_POSTGRES_TEST=1` is set, the test harness SHALL panic if `TEST_POSTGRES_URL` is unset or unreachable.

#### Scenario: Running dual-dialect storage integration tests with live PostgreSQL
- **GIVEN** `TEST_POSTGRES_URL` pointing to a running PostgreSQL 16 instance and `REQUIRE_POSTGRES_TEST=1`
- **WHEN** `cargo test --package mmo-storage --test postgres_dialect_integration_test` executes
- **THEN** all repository, container transfer, escrow trade, marketplace, document store, and world persistence assertions SHALL execute and succeed against both SQLite and PostgreSQL

### Requirement: GitHub Actions PostgreSQL Service Container
The `test` job in `.github/workflows/rust.yml` SHALL provision a `postgres:16-alpine` service container and configure `TEST_POSTGRES_URL` and `REQUIRE_POSTGRES_TEST=1` during workspace test execution.

#### Scenario: CI test workflow execution
- **GIVEN** a pull request or push to `main` triggering `.github/workflows/rust.yml`
- **WHEN** the `Test & Coverage` job runs `cargo llvm-cov nextest`
- **THEN** the PostgreSQL service container SHALL be healthy on port `5432` and `mmo-storage` dual-dialect tests SHALL verify live PostgreSQL queries
