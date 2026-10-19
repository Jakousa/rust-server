---
name: openspec-bundle-executor
description: >-
  Step-by-step execution guide for implementing pre-architected OpenSpec groundwork
  bundles from openspec/ROADMAP.md. Designed so implementation agents and local
  subagents can execute complex changes deterministically without architectural drift.
license: MIT
metadata:
  author: MMO Product & Architecture Team
  version: "1.0"
---

# OpenSpec Groundwork Bundle Executor Workflow

Use this skill whenever implementing an existing OpenSpec groundwork bundle from [`openspec/ROADMAP.md`](../../../openspec/ROADMAP.md) or `openspec/changes/<bundle-name>/`.

Every bundle in `openspec/changes/` has already been architected by a Principal Architect agent with exact file paths, line numbers, struct definitions, method signatures, and test scenarios. **Your job is disciplined, faithful execution—not re-architecting.**

---

## 1. Pre-Flight Checklist

1. **Sync with `origin/main`**:
   ```bash
   git fetch origin
   git checkout main
   git pull --ff-only origin main
   ```
2. **Select the Target Bundle**:
   - Check [`openspec/ROADMAP.md`](../../../openspec/ROADMAP.md) and pick the next uncompleted bundle in dependency order (or the specific bundle requested by the user).
   - *(Note: All 10 initial groundwork bundles in `openspec/ROADMAP.md` have been completed and archived. If `openspec/changes/` contains no active bundles, use `openspec-propose` or `openspec-pro-workflow` to propose and plan a new change first.)*
3. **Read All 4 Bundle Artifacts Completely Before Writing Code**:
   - `openspec/changes/<bundle-name>/proposal.md`
   - `openspec/changes/<bundle-name>/specs/.../spec.md`
   - `openspec/changes/<bundle-name>/design.md`
   - `openspec/changes/<bundle-name>/tasks.md`
4. **Create Feature Branch**:
   ```bash
   git checkout -b feat/<bundle-name>
   ```

---

## 2. Strict Guardrails (Do Not Stray)

1. **Follow `design.md` Signatures & Invariants Exactly**:
   - Do not invent alternative struct names, skip edge-case checks described in `design.md`, or weaken error handling.
   - Respect the `AGENTS.md` Prime Directive at all times: never introduce RPG-specific or genre-specific types into `crates/core`, `crates/schema`, `crates/node`, `crates/director`, `crates/gateway`, or `crates/storage`.
2. **Execute `tasks.md` Sequentially**:
   - Work through task group `1`, then `2`, then `3`, etc.
   - After completing each sub-task, run `cargo test -p <crate>` to verify compilation and tests immediately before moving to the next sub-task.
   - Mark completed checkboxes `- [x]` in `openspec/changes/<bundle-name>/tasks.md` as you verify them.
3. **Delegate Self-Contained Sub-Tasks When Appropriate**:
   - Boilerplate structs, binary codecs, SQL queries, and isolated unit tests can be delegated to local models or subagents with the exact section of `design.md` pasted into their prompt.

---

## 3. Quality Gates, Spec Sync, Archive & PR Merge

Once all implementation tasks in `tasks.md` are checked `[x]`:

1. **Sync Delta Spec & Archive the Completed Bundle**:
   - Copy or merge `openspec/changes/<bundle-name>/specs/<domain>/<capability>/spec.md` into `openspec/specs/<domain>/<capability>/spec.md`.
   - Move `openspec/changes/<bundle-name>` to `openspec/changes/archive/YYYY-MM-DD-<bundle-name>`.
2. **Run Mandatory Workspace Quality Gates**:
   ```bash
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```
   *Zero warnings and zero test failures are allowed.*
3. **Commit, Push, Create PR & Enable Auto-Merge**:
   ```bash
   git add -A
   git commit -m "feat(<scope>): <concise summary> (#<issue_number>)"
   git push -u origin feat/<bundle-name>
   gh pr create --title "feat(<scope>): <concise summary> (#<issue_number>)" \
     --body "Implements OpenSpec bundle \`<bundle-name>\`.\n\nCloses #<issue_number>" \
     --base main
   gh pr merge --auto --squash
   ```
