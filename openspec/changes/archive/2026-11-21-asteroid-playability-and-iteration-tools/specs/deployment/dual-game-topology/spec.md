# Dual-Game Browser Client & Multi-Game Kubernetes Deployment Specification (Delta)

## MODIFIED Requirements

### Requirement: Pluggable Dual-Game Browser Client over Shared WASM Networking Core
The browser client (`web/`) SHALL decouple shared networking (`transport.js`, `protocol.js`, `WasmClientWorld`, `WasmVoxelWorld`) from game-specific rendering, controls, and HUD cartridges (`village` vs. `asteroid`), gate gameplay packets and chunk requests behind `AUTH_RESPONSE`, serialize WebTransport stream writes, and render deterministic procedural asteroid previews (`isPreview = true`) immediately until replaced by authoritative server chunks (`revision >= 0`).

#### Scenario: Running the browser client in Asteroid Miners mode (`asteroid`)
- **GIVEN** the browser client launched in `asteroid` mode connected to a gateway routing to `mmo-asteroid-node`
- **WHEN** the client mounts `asteroid` mode and completes `AUTH_RESPONSE` authentication
- **THEN** the client SHALL render deterministic preview asteroid meshes immediately before sync, defer `Movement`, `Inspect`, and `0x00A0` chunk requests until `AUTH_RESPONSE` succeeds, serialize `sendStream` writes over a single bidirectional stream, replace `isPreview` chunks when authoritative `0x00A0` chunks (`revision >= 0`) arrive, display 3D asteroid cluster beacons and a material-tinted voxel aim highlight reticle, provide 6DoF flight-assist navigation (`Fly to Asteroid`, `Fly to Station`, `Brake`), and present a guided mission HUD alongside the Orbital Station Vault & Trade HUD (`module_id = 0x03`)

#### Scenario: Running the browser client in Village RPG mode (`village`)
- **GIVEN** the browser client launched in `village` mode connected to a gateway routing to `mmo-village-node`
- **WHEN** the client connects and receives AoI entity snapshots and village barter responses (`module_id = 0x02`)
- **THEN** the client SHALL render the medieval village plaza, 2D ground movement controls, patrolling villager NPCs, and the NPC barter / player trade UI without loading voxel asteroid meshes or 6DoF space controls

## ADDED Requirements

### Requirement: Browser Playtest Workbench & Fast Local Iteration Tooling
The browser client (`web/`) and workspace scripts (`scripts/`, `docker-compose.yml`) SHALL provide a toggleable in-browser Playtest Workbench (`DevToolsController`) for offline sandbox mining/combat, sector teleports, target dummy spawning, and voxel diagnostics alongside an isolated native local runner (`scripts/dev-local.sh`) without requiring container rebuilds or causing HUD overlap at `1280x800`.

#### Scenario: Iterating offline on `/asteroid` via the Playtest Workbench
- **GIVEN** a developer opening `/asteroid` without a live backend cluster
- **WHEN** the developer enables Local Sandbox Mode or uses the Playtest Tools drawer (`#btn-toggle-devtools`) to teleport to an asteroid, mine ore voxels, spawn a high-shininess target dummy, and return to the station dock
- **THEN** the browser client SHALL preserve local preview asteroids across offline disconnect events, simulate ore harvesting into unbanked cargo, update ship loot-shininess shaders (`state_flags`), resolve dummy laser combat with full-loot transfer, bank loot into the station vault, and keep all visible HUD panels free of viewport overlap at `1280x800`

#### Scenario: Switching to live authenticated session automatically exits sandbox isolation
- **GIVEN** a developer testing `/asteroid` with Local Sandbox Mode or simulated target dummies active
- **WHEN** the client completes `AUTH_RESPONSE` authentication with a live gateway
- **THEN** `DevToolsController` SHALL automatically disable Local Sandbox Mode and remove simulated dummy entities so live authoritative server state is never mixed with synthetic local state
