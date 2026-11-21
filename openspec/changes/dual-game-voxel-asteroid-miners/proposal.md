# Proposal

## Why

The core value proposition of this platform is that its core engine crates (`mmo-core`, `mmo-node`, `mmo-director`, `mmo-gateway`, `mmo-storage`, `mmo-schema`, `mmo-client-wasm`) form a strictly unopinionated, genre-agnostic MMO substrate where radically different games are composed purely by wiring different `GameplayPlugin` sets and kinematic modes into a simulation node binary. However, today only a single game vertical slice (**Game A: Medieval Village & Barter RPG** via `mmo-village-node`) is deployed and playable in the browser client. Building and deploying a second, diametrically opposite game (**Game B: Competitive 6DoF Voxel Asteroid Miners** via `mmo-asteroid-node`) alongside Game A—using 3D `Newtonian6DoF` space flight, volumetric deformable voxel asteroids (`mmo-plugin-voxel`), PvP full-loot ship combat (`CombatPlugin`), and shiny loot-tier ship visuals while deliberately **sharing** `InventoryPlugin` and `TradePlugin`—proves the engine's cross-genre plugin composability and multi-game Kubernetes deployment architecture end to end.

## What Changes

- **New Competitive Voxel Asteroid Mining Plugin (`AsteroidMiningPlugin`) & Node Daemon (`mmo-asteroid-node`)**:
  - Introduce `crates/gameplay/src/asteroid.rs` (`AsteroidMiningPlugin`, `module_id = 0x03`) and binary target `crates/gameplay/src/bin/mmo_asteroid_node.rs`.
  - Configure `TickSimulation` with `MotionMode::Newtonian6DoF` (3D zero-G thruster physics across X/Y/Z axes) while keeping `mmo-core`, `mmo-node`, `mmo-director`, `mmo-gateway`, and `mmo-storage` 100% untouched and genre-agnostic.
  - Compose **shared plugins** (`InventoryPlugin`, `TradePlugin`) with **genre-specific plugins** (`VoxelTerrainPlugin` from `mmo-plugin-voxel`, `CombatPlugin`, and `AsteroidMiningPlugin`).
  - Seed volumetric 3D voxel asteroids containing outer crust rock (`voxel_id = 1`) and embedded veins of Titanium (`voxel_id = 2`), Gold (`voxel_id = 3`), and Void Crystal (`voxel_id = 4`) around a Home Orbital Station safe dock.
  - Hook into `VoxelChangeEvent` emitted by `VoxelTerrainPlugin` (`module_id = 0x00A0`): when a ship blasts/excavates an ore voxel (`new_voxel = 0`), credit the corresponding ore `ItemStack` directly into the pilot's `InventoryComponent`.
  - Encode the ship's unbanked cargo value tier (`0..=255` richness byte) into the upper bits (`bits 16..23`) of the entity's authoritative `state_flags` so AoI delta snapshots automatically replicate "ship shininess" at 20 Hz without any wire-protocol schema changes.
  - Implement PvP laser weapons and full-loot destruction: when a ship's `HealthComponent` reaches `0` (`EntityDiedEvent`), transfer all unbanked ore from the victim's `InventoryComponent` to the attacker's `InventoryComponent` (updating both ships' `state_flags` shininess tiers immediately), restore the victim's hull HP, and respawn the victim at the Home Orbital Station dock.
  - Enable Home Orbital Station docking (`module_id = 0x03`): pilots within docking range of the station can bank unbanked ore into permanent station vault credits/storage or trade cargo with other docked pilots via `TradePlugin`.
- **Dual-Game Browser Client Split (`web/`)**:
  - Refactor `web/` to cleanly separate the shared networking/replication core (`transport.js`, `protocol.js`, `WasmClientWorld` + `WasmVoxelWorld`) from game-specific scene renderers, controls, and HUDs:
    - **Game A (`village` profile)**: Existing medieval cobblestone plaza, 2D ground WASD controls, patrolling villager NPCs, and NPC barter / player trade UI.
    - **Game B (`asteroid` profile)**: Deep-space starfield, 6DoF thruster controls (3D X/Y/Z flight + pitch/yaw aim), greedy-meshed volumetric 3D voxel asteroids (`WasmVoxelWorld`), mining/combat laser beams, dynamic emissive gold/crystal ship shaders driven by the replicated `state_flags` loot-shininess tier, and Orbital Station Vault / `TradePlugin` docking UI.
  - Support switching or deploying the frontend as two distinct games via environment configuration (`VITE_GAME_MODE=village|asteroid`), subdomain/path routing, or dual in-app game launcher selector.
- **Multi-Game Docker & Kubernetes Deployment Topology**:
  - Update `Dockerfile` to package `mmo-asteroid-node` alongside `mmo-village-node`, `mmo-node`, `mmo-director`, and `mmo-gateway` in the unified `mmo-server` image.
  - Add Kubernetes overlay/manifest structure supporting two parallel MMO game deployments in the cluster (Game A `mmo-village` and Game B `mmo-asteroid`), each running identical `mmo-director` and `mmo-gateway` containers paired with their respective node entrypoint (`mmo-village-node` vs. `mmo-asteroid-node`) and web frontend configuration.

## Capabilities

### New Capabilities
- `gameplay/voxel-asteroid-miners`: Authoritative 6DoF space flight configuration, volumetric voxel asteroid ore generation and excavation reward harvesting via `VoxelChangeEvent`, dynamic `state_flags` loot-shininess encoding, PvP full-loot cargo transfer on ship destruction, Home Orbital Station banking/trading (`module_id = 0x03`), and the `mmo-asteroid-node` daemon binary.
- `deployment/dual-game-topology`: Modular browser client separation into Game A (`village`) and Game B (`asteroid`) profiles over the shared `mmo-client-wasm` transport substrate, plus container and Kubernetes manifests for running two distinct MMO games concurrently in a cluster.

### Modified Capabilities
<!-- None: all existing core, node, gateway, director, voxel, and village requirements remain unchanged. -->

## Impact

- **`crates/gameplay`**: Add `crates/gameplay/src/asteroid.rs` (`AsteroidMiningPlugin`), `crates/gameplay/src/bin/mmo_asteroid_node.rs`, and dependency on `mmo-plugin-voxel` in `crates/gameplay/Cargo.toml`.
- **`crates/core`, `crates/node`, `crates/schema`, `crates/gateway`, `crates/director`, `crates/storage`**: Zero genre-specific modifications; preserves strict core engine decoupling.
- **`web/`**: Split scene/renderer/controls/HUD modules into pluggable game cartridges (`web/src/games/village/` and `web/src/games/asteroid/`) sharing `web/src/transport.js`, `web/src/protocol.js`, and `mmo-client-wasm`.
- **`Dockerfile` & `k8s/`**: Include `mmo-asteroid-node` binary in `Dockerfile` and add dual-game Kubernetes manifests/overlays for deploying both games concurrently.
