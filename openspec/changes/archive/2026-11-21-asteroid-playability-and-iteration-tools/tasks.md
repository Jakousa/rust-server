# Implementation Tasks

## 1. OpenSpec & Architectural Validation
- [x] 1.1 Scaffold change `asteroid-playability-and-iteration-tools` and validate proposal, delta specs, design, and tasks (`openspec validate "asteroid-playability-and-iteration-tools"`).
- [x] 1.2 Run Pro Architectural Review subagent (`Model: "pro"`), incorporate recommendations into artifacts, and re-validate with zero warnings.

## 2. Authoritative Asteroid Sector Expansion & Autonomous Sector Drones (`crates/gameplay`)
- [x] 2.1 Expand `SEEDED_ASTEROID_CHUNKS` in `crates/gameplay/src/asteroid.rs` from 6 to 10 chunks (retaining the 6 original coordinates and adding 4 closer eye-level belt chunks strictly outside the `15.0m` safe zone).
- [x] 2.2 Reject player-to-NPC trade initiation (`sub_opcode = 4` when `target.entity_type() != EntityType::Player`) in `AsteroidPacketHandler`.
- [x] 2.3 Spawn deterministic `EntityType::Npc` patrol drones (`ROGUE_DRONE_SPAWNS`) outside the `15.0m` safe zone with `HealthComponent`, `InventoryComponent` ore loot, and `StationVaultComponent`, orbit them smoothly with per-tick `ctx.octree` & `rotation` sync in `AsteroidMiningTickSystem::update`, and respawn destroyed drones at their patrol anchor with reset phase and restocked ore cargo.
- [x] 2.4 Add Rust unit tests verifying all 10 `SEEDED_ASTEROID_CHUNKS` keep solid voxels outside `15.0m` and clear of spawn, NPC trade rejection, NPC patrol motion/octree sync, full-loot drone destruction, and drone respawn.

## 3. Authenticated Chunk Streaming, Stream Serialization & Preview Seeding (`web/`)
- [x] 3.1 Serialize `WebTransportClient.sendStream()` stream initialization and writes across connection epochs (`_streamInitPromise`, `_streamWriteChain`, error isolation, and cleanup on `disconnect()` / `closed`) in `web/src/transport.js`.
- [x] 3.2 Add `SEEDED_ASTEROID_CHUNKS`, `generateProceduralAsteroidChunk()`, `seedLocalPreviewChunks()` (`isPreview = true`), bounded TTL-based `pendingChunkRequests`, `resetRequestTracking()`, `getAsteroidClusterSummaries()`, and `getVoxelMaterialInfo()` in `web/src/games/asteroid/voxel_client.js`.
- [x] 3.3 Track `ClientWorldBridge.isAuthenticated`, gate `Movement` datagrams, `performInspect()`, and `requestVisibleAsteroidChunks()` on authentication, reset chunk request tracking on `AUTH_RESPONSE`, preserve/re-seed preview chunks on offline/disconnect in `asteroid` mode, and align client mining reach (`18.0m` / `14.0m`) in `web/src/main.js`.

## 4. 3D Visuals, Aim Reticle, Flight Assist & Guided Mission HUD (`web/`)
- [x] 4.1 Add asteroid cluster marker beacons, 3D material-tinted wireframe voxel aim highlight (`setHighlightedVoxel`), and chase-camera heading alignment (`shipYaw + Math.PI` when not dragging) in `web/src/games/asteroid/renderer.js`.
- [x] 4.2 Add `orientToward()`, emergency brake (`X`), and bank shortcut (`B`) to `SpaceFlightControls` in `web/src/games/asteroid/controls.js`.
- [x] 4.3 Add compact 3-step Mission Objectives checklist, target voxel/navigation readout, NPC drone labeling (disabling `#btn-trade-pilot` for NPCs), and Flight Assist buttons (`Fly to Asteroid`, `Fly to Station`, `Brake [X]`) in `web/index.html`, `web/src/ui.js`, and `web/style.css` staying within the `1280x800` right-rail budget.

## 5. Fast Iteration Tools & Automated Test Coverage (`web/`, `scripts/`, `docker-compose.yml`)
- [x] 5.1 Implement `web/src/dev_tools.js` (`DevToolsController`) and `#devtools-drawer` UI for isolated offline Local Sandbox Mode, instant teleports, target dummy spawning, and voxel/chunk diagnostics, wired into `web/src/main.js` (auto-disabling sandbox on live auth).
- [x] 5.2 Add `scripts/dev-local.sh` native local runner (`asteroid|village|both`) with isolated `mmo-director` instances and distinct ports, `dev:local` script in `web/package.json`, and `asteroid` profile services in `docker-compose.yml`.
- [x] 5.3 Expand unit tests in `web/src/__tests__/asteroid_game.test.js` and add simulated-gateway `/asteroid` E2E test (including `#asteroid-target-panel` viewport layout assertions) in `web/e2e/inspector.spec.js`.

## 6. Quality Gates & Archiving
- [x] 6.1 Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `npm --prefix web test -- --run`, `npm --prefix web run build`, and `npm --prefix web run test:e2e`.
- [x] 6.2 Verify implementation against OpenSpec artifacts, sync specs and archive change via `openspec`, run `openspec validate --all`, commit to feature branch, push, and open PR with auto-merge.
