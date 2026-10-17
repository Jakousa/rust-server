# Proposal: Fix CI Paths Filter and Harden Nightly Mutation Testing

## Context
Two critical CI/CD pipeline issues currently hinder repository automation:
1. **Unmergeable Documentation / Spec Pull Requests**: Pull requests modifying only non-code paths (such as `openspec/**`, `docs/**`, or markdown files, e.g. PR #80) cannot be merged even with auto-merge enabled. The repository ruleset on branch `main` requires the status check `Lint, Test & Verify` to pass. Because `.github/workflows/rust.yml` configures `paths-ignore` at the workflow trigger level, the CI pipeline is completely skipped on documentation changes. As a consequence, `Lint, Test & Verify` is never reported to GitHub, and the PR remains in a permanent `BLOCKED` state.
2. **Nightly Mutation Testing Failures**: The nightly mutation test suite (`mutation-tests.yml`) fails on both `mmo-schema` and `mmo-storage`:
   - In `mmo-schema`, 6 mutants survive due to missing boundary coverage on buffer sizes (`<` vs `<=`) in `client.rs` packet decoders and unexercised enum mapping in `From<MovementCorrectionReason> for u8`.
   - In `mmo-storage`, 85 mutants survive because `DbBackend::Postgres` branches in repositories and transaction coordinators are unreachable during headless in-memory SQLite unit tests, and `.cargo/mutants.toml` lacked broad exclusions for unexercised Postgres branches. In addition, unit test coverage in `datastore.rs` missed specific getter and filter boundaries.

## Objectives
- Replace top-level `paths-ignore` in `.github/workflows/rust.yml` with `dorny/paths-filter`. Ensure that `Continuous Integration` always triggers on pull requests and pushes to `main`. When only non-code files change, heavy compilation and test jobs are skipped while the required gate job `Lint, Test & Verify` executes and passes immediately.
- Eliminate all surviving mutants in `mmo-schema` by adding targeted boundary test cases in `crates/schema/src/client.rs`.
- Eliminate surviving mutants in `mmo-storage` by:
  - Adding boundary unit tests in `crates/storage/src/datastore.rs` and other storage coordinators.
  - Adding declarative regex exclusions in `.cargo/mutants.toml` for unreachable `DbBackend::Postgres` match arms and dialect-specific execution paths that cannot run in headless unit test environments.
- Verify 100% mutation test pass rate in both packages locally and ensure quality gates (`cargo fmt`, `cargo clippy`, `cargo test`) succeed.

## Non-Goals
- Spinning up a dedicated live PostgreSQL cluster inside unit tests (PostgreSQL integration testing is handled in integration environments).
- Modifying game logic or protocol definitions outside of unit test assertions and CI/mutant configuration.
