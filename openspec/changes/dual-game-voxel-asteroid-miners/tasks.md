# Tasks

## 1. Authoritative Asteroid Mining Plugin & 6DoF Sector Bootstrap (`crates/gameplay`)

- [ ] 1.1 Add `mmo-plugin-voxel = { path = "../plugins/voxel" }` to `crates/gameplay/Cargo.toml` and declare `pub mod asteroid;` in `crates/gameplay/src/lib.rs`; verify `cargo check -p mmo-gameplay` succeeds while `crates/node/Cargo.toml` remains free of `mmo-gameplay` or `mmo-plugin-voxel` dependencies.
- [ ] 1.2 Implement `StationVaultComponent`, `PilotStarterKitHook` (attaching `InventoryComponent`, `HealthComponent`, `StationVaultComponent`, and `MotionMode::Newtonian6DoF`), procedural 3D voxel asteroid seeding (`Crust Rock = 1`, `Titanium = 2`, `Solar Gold = 3`, `Void Crystal = 4`), and `update_ship_shininess` (`state_flags` bits `16..23`) in `crates/gameplay/src/asteroid.rs`; verify unit tests cover component codec roundtrips and shininess flag calculation.
- [ ] 1.3 Implement `AsteroidMiningTickSystem` in `crates/gameplay/src/asteroid.rs` to harvest ore from `VoxelChangeEvent` notifications (`VoxelTerrainPlugin` -> `InventoryComponent`) and process `EntityDiedEvent` full-loot cargo transfers from victim to killer with station dock respawn; verify unit tests cover ore harvesting, full-loot transfer, shininess updates, and respawn coordinates.
- [ ] 1.4 Implement `AsteroidPacketHandler` (`module_id = 0x03`, sub-opcodes `1..=7` for sector inspection, PvP laser firing with safe-zone and range/cooldown guards, Home Orbital Station vault deposit, and `TradePlugin` pilot-to-player atomic trade) and `bootstrap_asteroid_world` in `crates/gameplay/src/asteroid.rs`; verify unit and integration tests pass via `cargo test -p mmo-gameplay`.
- [ ] 1.5 Create the `mmo-asteroid-node` binary entrypoint in `crates/gameplay/src/bin/mmo_asteroid_node.rs` (configuring `MotionMode::Newtonian6DoF` and calling `bootstrap_asteroid_world` + `run_node_daemon_with_sim`) and register `[[bin]] name = "mmo-asteroid-node"` in `crates/gameplay/Cargo.toml`; verify `cargo build --bin mmo-asteroid-node` succeeds.

## 2. Dual-Game Browser Client Cartridge Architecture (`web/`)

- [ ] 2.1 Refactor `web/src/` to extract the shared transport/protocol/WASM replication hooks while organizing **Game A (`village`)** into `web/src/games/village/` and **Game B (`asteroid`)** into `web/src/games/asteroid/` with mode resolution via `?game=village|asteroid`, `VITE_GAME_MODE`, and a top-bar game mode switcher; verify existing Village RPG Vitest tests pass (`npm --prefix web test -- --run`).
- [ ] 2.2 Implement the **Game B (`asteroid`)** 3D Three.js space renderer, 6DoF flight & mining/combat laser controls, greedy-meshed volumetric voxel asteroid chunk rendering (`WasmVoxelWorld`, `module_id = 0x00A0`), and dynamic ship metallic/emissive glow shader driven by `(state_flags >> 16) & 0xFF`; verify unit tests cover shininess tier decoding and voxel chunk mesh buffer wiring.
- [ ] 2.3 Implement the **Game B (`asteroid`)** Orbital Station HUD (`module_id = 0x03`) displaying ship hull HP, unbanked cargo inventory, shininess risk meter, safe-zone docking indicator, one-click Station Vault deposit, and pilot-to-pilot `TradePlugin` session controls; verify frontend unit tests and production build pass (`npm --prefix web test -- --run && npm --prefix web run build`).

## 3. Container Packaging, Multi-Game Kubernetes Overlays & Verification

- [ ] 3.1 Update `Dockerfile` to copy `/app/target/release/mmo-asteroid-node` into `/usr/local/bin/mmo-asteroid-node` alongside `mmo-village-node`, `mmo-node`, `mmo-director`, and `mmo-gateway`; verify Dockerfile syntax and binary list completeness.
- [ ] 3.2 Add dual-game Kubernetes deployment manifests/overlays under `k8s/` (configuring parallel `village` and `asteroid` node StatefulSets and web frontend modes using the shared `mmo-server` and `mmo-web` images) and document the dual-game architecture in `README.md` and `ARCHITECTURE.md`.
- [ ] 3.3 Run all workspace quality gates (`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `npm --prefix web test -- --run`, `npm --prefix web run build`, and `openspec validate --all`) and verify zero warnings or errors.

## Workflow follow-up

- Sync delta specs to `openspec/specs/` and archive `dual-game-voxel-asteroid-miners` once implementation and verification are complete.
