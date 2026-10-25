# Add wasm32 Target Check, Frontend Build Verification, and Vitest Unit Suite to CI Proposal

## Context & Problem Statement
Currently, `.github/workflows/rust.yml` compiles and tests `mmo-client-wasm` only on the host target (`x86_64-unknown-linux-gnu`) and never builds or tests the `web/` 3D Visual Inspector frontend:
1. If `mmo-client-wasm` accidentally pulls in a non-WASM-compatible OS/socket dependency or breaks `#[wasm_bindgen]` ABI signatures on `wasm32-unknown-unknown`, CI will not catch it.
2. `web/package.json` has no `test` script (`vitest`), so browser-side 64-bit `BigInt` GUID reconstruction (`WorldObjectRenderer.updateFromBuffer` in `web/src/world.js`, `EntityRenderer.updateFromBuffer` in `web/src/renderer.js`) and `PlayerControls` kinematics (`web/src/controls.js`) lack automated unit tests and `npm run build` verification in CI.

## Proposed Solution
1. **WASM `wasm32-unknown-unknown` Target Check in CI (`.github/workflows/rust.yml`)**:
   - Add `targets: wasm32-unknown-unknown` to the `lint` job's Rust toolchain setup in `.github/workflows/rust.yml` and run `cargo clippy --package mmo-client-wasm --target wasm32-unknown-unknown -- -D warnings`.
   - Update the `filter` job in `.github/workflows/rust.yml` so changes to `web/**` also trigger CI verification.
2. **Vitest Unit Test Suite & Production Build Check for `web/`**:
   - Add `vitest` to `devDependencies` and `"test": "vitest run"` to `scripts` in `web/package.json`.
   - Create `web/src/__tests__/inspector.test.js` testing:
     - `PlayerControls` (`web/src/controls.js`): WASD movement normalization, Q/E yaw rotation, and monotonic `sample()` sequence increments.
     - `WorldObjectRenderer` (`web/src/world.js`): 16-float buffer parsing, 64-bit `BigInt` `(hi << 32n) | lo` reconstruction for IDs `> 2^53`, damaged state material switching (`stateFlags & 16`), health bar scaling, and despawned object cleanup.
   - Add a `web` verification step (or job) in `.github/workflows/rust.yml` running `npm ci && npm test && npm run build` in `web/`.

## Scope & Non-Goals
- **In Scope**: `web/package.json`, `web/package-lock.json`, `web/src/__tests__/inspector.test.js`, and `.github/workflows/rust.yml`.
- **Non-Goals**: Heavy headless browser WebGPU rendering pipelines in CI.
