## Summary

<!-- Briefly explain the purpose and context of this pull request. -->

## Changes

<!-- List key changes introduced by this PR. -->
- 

## Architectural Alignment & Invariants

<!-- Verify that these core invariants are preserved. -->
- [ ] **Server Authority**: Kinematics and action timings are authoritative and calculated on the server.
- [ ] **Zero Server Pollution**: WebAssembly and browser dependencies remain isolated in leaf crates (`mmo-client-wasm`, `web/`).
- [ ] **Zero Codec Duplication**: Wire protocols and packet codecs live exclusively in `mmo-schema`.
- [ ] **Non-Blocking Hot Paths**: Simulation tick loops and gateway routing perform no blocking I/O or mutex locks across `.await`.

## Quality Gates Verified Locally

- [ ] `cargo fmt --all -- --check` passes cleanly
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` has 0 warnings
- [ ] `cargo test --workspace` passes all tests
- [ ] Web inspector build (`npm --prefix web run build`) succeeds (if web/client assets changed)
