# Dual-Game Browser Client & Multi-Game Kubernetes Deployment Specification

## Purpose

Defines the modular browser client architecture and Kubernetes deployment topology required to run two distinct MMO games—**Game A: Medieval Village & Barter RPG** (`mmo-village-node`) and **Game B: Competitive 6DoF Voxel Asteroid Miners** (`mmo-asteroid-node`)—concurrently from a single repository while sharing the core transport, WASM state replication, and server infrastructure crates.

## Requirements

### Requirement: Pluggable Dual-Game Browser Client over Shared WASM Networking Core
The browser client (`web/`) SHALL decouple the shared transport and state-replication layer (`transport.js`, `protocol.js`, `WasmClientWorld`, `WasmVoxelWorld`) from game-specific rendering, input controls, and HUD cartridges so it can run either the **Village RPG (`village`)** cartridge or the **Voxel Asteroid Miners (`asteroid`)** cartridge based on build/runtime configuration (`VITE_GAME_MODE` or URL query/selector).

#### Scenario: Running the browser client in Asteroid Miners mode (`asteroid`)
- **GIVEN** the browser client launched in `asteroid` mode connected to a gateway routing to `mmo-asteroid-node`
- **WHEN** the client receives AoI entity snapshots and voxel chunk payloads (`module_id = 0x00A0`)
- **THEN** the client SHALL render a 3D space scene with greedy-meshed volumetric voxel asteroids (`WasmVoxelWorld`), 6DoF flight and laser mining/combat controls, ship meshes whose emissive metallic/glow intensity scales dynamically with the replicated `state_flags` loot-shininess byte (`bits 16..23`), and an Orbital Station Vault & Trade HUD (`module_id = 0x03`)

#### Scenario: Running the browser client in Village RPG mode (`village`)
- **GIVEN** the browser client launched in `village` mode connected to a gateway routing to `mmo-village-node`
- **WHEN** the client connects and receives AoI entity snapshots and village barter responses (`module_id = 0x02`)
- **THEN** the client SHALL render the medieval village plaza, 2D ground movement controls, patrolling villager NPCs, and the NPC barter / player trade UI without loading voxel asteroid meshes or 6DoF space controls

### Requirement: Unified Container Packaging & Parallel Dual-Game Kubernetes Deployments
The server container image (`Dockerfile`) SHALL compile and bundle both `mmo-village-node` and `mmo-asteroid-node` alongside the genre-agnostic `mmo-director` and `mmo-gateway` binaries, and the Kubernetes manifests (`k8s/`) SHALL support running isolated Game A (`village`) and Game B (`asteroid`) stacks concurrently in the cluster using identical `mmo-director` and `mmo-gateway` images paired with each game's respective simulation node command and frontend configuration.

#### Scenario: Deploying Game A and Game B side-by-side in a Kubernetes cluster
- **GIVEN** the unified `mmo-server` and `mmo-web` container images deployed via the dual-game Kubernetes manifests/overlays
- **WHEN** both game stacks start in the cluster
- **THEN** the Village stack's `mmo-node` StatefulSet SHALL execute `mmo-village-node` serving `village` web clients, and the Asteroid stack's `mmo-node` StatefulSet SHALL execute `mmo-asteroid-node` serving `asteroid` web clients, with zero cross-talk between the two virtual worlds
