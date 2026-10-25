# WASM Target and Web Frontend CI Verification Specification

## Purpose
Ensures `mmo-client-wasm` compiles cleanly without warnings for `wasm32-unknown-unknown` and verifies the `web/` 3D Visual Inspector via automated Vitest unit tests and Vite production builds in CI.

## Requirements

### Requirement: wasm32-unknown-unknown Compilation Gate
The CI pipeline in `.github/workflows/rust.yml` SHALL install the `wasm32-unknown-unknown` target and verify `cargo clippy --package mmo-client-wasm --target wasm32-unknown-unknown -- -D warnings`.

#### Scenario: Compiling mmo-client-wasm for wasm32-unknown-unknown
- **GIVEN** the `mmo-client-wasm` crate
- **WHEN** `cargo clippy --package mmo-client-wasm --target wasm32-unknown-unknown -- -D warnings` is executed
- **THEN** compilation SHALL succeed with zero warnings or target-incompatibility errors

### Requirement: Web Frontend Vitest Suite and Production Build Gate
`web/package.json` SHALL define a `"test"` script using Vitest, and `.github/workflows/rust.yml` SHALL execute `npm test` and `npm run build` in `web/`.

#### Scenario: Running Vitest unit suite and Vite production build
- **GIVEN** the `web/` frontend workspace
- **WHEN** `npm test && npm run build` is executed in `web/`
- **THEN** all unit tests for `PlayerControls` and `WorldObjectRenderer` (including 64-bit `BigInt` ID reconstruction) SHALL pass and Vite SHALL produce a clean production bundle in `web/dist/`
