# ci/workflow Specification

## Purpose
Automates continuous integration quality checks across all crates in the MMO workspace to prevent regressions in compilation, formatting, linting, and test execution, while guaranteeing that required status checks pass for all PR types including documentation and specifications without allowing false-green passes on cancellations or filter failures.

## Requirements

### Requirement: Always-Reported Status Check Gate
The repository SHALL contain an automated GitHub Actions CI workflow executing on pull requests and commits to the `main` branch, reporting the required check `Lint, Test & Verify` on every execution.

#### Scenario: Non-code pull request (documentation / specifications)
- **WHEN** a pull request modifies only non-code files (e.g. `**.md`, `openspec/**`, `docs/**`, `.github/dependabot.yml`)
- **THEN** heavy compilation, linting, testing, and mutation jobs SHALL be skipped via path filtering
- **AND** the required gate job `Lint, Test & Verify` SHALL execute, verify `filter` succeeded, ensure no downstream jobs were cancelled or failed, and report success to GitHub.

#### Scenario: Code pull request quality gate execution
- **WHEN** a pull request modifies code, workflow files, or crate manifests
- **THEN** the CI workflow SHALL execute:
  1. Formatting check (`cargo fmt --all -- --check`)
  2. Clippy lint check (`cargo clippy --workspace --all-targets -- -D warnings`)
  3. Workspace test suite (`cargo llvm-cov nextest ...`)
  4. Incremental mutation test gate if schema code changed
- **AND** failure or cancellation of any executed check SHALL cause `Lint, Test & Verify` to fail and block PR merging.
