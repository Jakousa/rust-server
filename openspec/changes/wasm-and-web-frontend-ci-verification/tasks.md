# Implementation Tasks: WASM Target Check, Frontend Build Verification, and Vitest Suite (Issue #132)

- [ ] 1. Web Frontend Vitest Setup & Unit Tests (`web/`)
  - [ ] 1.1 Guard `window.addEventListener` in `web/src/controls.js` (`if (typeof window !== "undefined")`) so `PlayerControls` runs cleanly in Node/Vitest.
  - [ ] 1.2 Add `vitest` to `web/package.json` `devDependencies` and `"test": "vitest run"` to `scripts`, and create `web/src/__tests__/inspector.test.js` testing `PlayerControls` and `WorldObjectRenderer` (64-bit `BigInt` ID reconstruction, damaged state flags, HP bar scaling, and despawn cleanup).
  - [ ] 1.3 Run `npm test && npm run build` inside `web/` to verify all frontend unit tests and production build pass.

- [ ] 2. CI Workflow Integration (`.github/workflows/rust.yml`)
  - [ ] 2.1 Add `targets: wasm32-unknown-unknown` and `cargo clippy --package mmo-client-wasm --target wasm32-unknown-unknown -- -D warnings` to the `lint` job in `.github/workflows/rust.yml`.
  - [ ] 2.2 Add Node.js setup and `npm ci && npm test && npm run build` (`working-directory: web`) to `.github/workflows/rust.yml`.

- [ ] 3. Workspace Quality Gates
  - [ ] 3.1 Run `cargo fmt --all -- --check`
  - [ ] 3.2 Run `cargo clippy --workspace --all-targets -- -D warnings`
  - [ ] 3.3 Run `cargo test --workspace`
