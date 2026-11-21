# Proposal

## Why

Players visiting `/asteroid` (Game B: Competitive 6DoF Voxel Asteroid Miners) currently encounter two severe playability gaps and slow developer iteration loops:

1. **Asteroids Do Not Render After Connecting**: `ClientWorldBridge` initializes `localPlayerGuid = 1` until `AUTH_RESPONSE` arrives, yet `requestVisibleAsteroidChunks()` fires on the first animation frame while `transport.connected` is `true` and authentication is still pending (alongside `Movement` datagrams and a `200ms` `performInspect` timer). `mmo-gateway` rejects all pre-auth packets via its anti-spoofing check (`acting != session_guid`), while `AsteroidVoxelClient.requestedChunks` permanently caches those keys so post-auth recovery sends zero requests. Additionally, concurrent `WebTransportClient.sendStream()` calls race on `createBidirectionalStream()`, exceeding the gateway's stream limit.
2. **Lack of Orientation, Objectives, Solo Targets, and Fast Iteration Tools**: Even with chunks rendered, players spawn without a 3D aim highlight reticle, asteroid cluster beacons, objective guidance, flight-assist navigation (`Fly to Asteroid`, `Fly to Station`, `Brake`), or solo NPC drone targets when no other players are online. Furthermore, iterating on `/asteroid` currently requires slow cluster rebuilds rather than an instant in-browser Playtest Workbench, local sandbox simulation, isolated native dev runner, and automated Playwright E2E validation.

## What Changes

- **Authoritative Asteroid Belt Expansion & Autonomous Sector Drones (`crates/gameplay/src/asteroid.rs`)**:
  - Expand `SEEDED_ASTEROID_CHUNKS` from 6 to 10 chunks (retaining all 6 original coordinates and adding 4 closer, eye-level belt chunks strictly outside the `15.0m` safe zone and spawn lane).
  - Spawn deterministic `EntityType::Npc` autonomous patrol drones (`ROGUE_DRONE_SPAWNS`) outside the `15.0m` Home Orbital Station safe zone, equipped with `HealthComponent`, `InventoryComponent` ore loot (giving non-zero `state_flags` shininess), and `StationVaultComponent`.
  - Orbit NPC drones incrementally in `AsteroidMiningTickSystem` (updating `ctx.octree` and facing yaw every tick), reject player-to-NPC trade attempts (`sub_opcode = 4`), and respawn destroyed drones at their patrol anchors with restocked starter ore so solo playtesting always has active targets.
- **Reliable Voxel Chunk Streaming, Stream Serialization & Local Preview Fallback (`web/`)**:
  - Gate `Movement` datagrams, `performInspect()`, and `requestVisibleAsteroidChunks()` strictly behind `world.isAuthenticated`, reset `AsteroidVoxelClient` request tracking on `AUTH_RESPONSE`, prioritize `SEEDED_ASTEROID_CHUNKS`, and bound TTL retries (`2000ms`, capped for vacuum chunks).
  - Serialize `WebTransportClient.sendStream()` initialization and writes cleanly across connection epochs so burst chunk requests reuse a single bidirectional stream safely and recover from transient write errors.
  - Seed deterministic procedural preview chunks (`revision = 0, isPreview = true`) on `/asteroid` mount (and preserve/re-seed them when offline/disconnected) so the asteroid belt renders from frame 1 and is seamlessly overwritten when authoritative chunks (`revision >= 0, isPreview = false`) arrive.
  - Align client mining raycast/proximity reach (`18.0m` / `14.0m`) with the server's `20.0m` excavation reach.
- **Guided 3D Space UX, Beacons, Reticle & Flight Assist (`web/`)**:
  - Add 3D asteroid cluster marker beacons/lights around loaded chunks, a material-tinted 3D wireframe voxel aim reticle (`setHighlightedVoxel`), and smooth chase-camera heading alignment (`shipYaw + Math.PI` when not dragging).
  - Add flight-assist controls (`SpaceFlightControls.orientToward`, `X` Emergency Brake, `B` Dock & Bank), NPC drone target labeling (disabling `#btn-trade-pilot` on NPCs), and a compact 3-step Mission Objectives & Navigation checklist (`Fly to Asteroid Belt`, `Mine Ore Voxels`, `Dock & Bank Cargo`) inside `.ship-status-panel` respecting the `1280x800` right-rail budget.
- **Fast Iteration & Playtest Tooling (`web/`, `scripts/`, `docker-compose.yml`)**:
  - Add an in-browser **Playtest Tools** drawer (`DevToolsController` toggled via button or `` ` ``) supporting Local Sandbox Mode (isolated offline mining, shininess, target dummy combat, and station vault banking), instant teleports, simulated high-shininess target dummy spawning, and live voxel/chunk diagnostics.
  - Add `scripts/dev-local.sh` (`asteroid|village|both`) with isolated `mmo-director` instances and distinct ports per game, `npm --prefix web run dev:local`, `docker-compose.yml` asteroid profile services, and a simulated-gateway `/asteroid` Playwright E2E suite in `web/e2e/inspector.spec.js`.

## Capabilities

### Modified Capabilities
- `gameplay/voxel-asteroid-miners`: Expand seeded asteroid belt coverage (`SEEDED_ASTEROID_CHUNKS`), reject pilot-to-NPC trades, and add autonomous patrolling `EntityType::Npc` drone entities with full-loot cargo and respawn behavior in `crates/gameplay`.
- `deployment/dual-game-topology`: Require post-authentication networking/chunk streaming with WebTransport stream serialization and procedural preview fallback, guided `/asteroid` 3D HUD/reticle/flight-assist UX, and fast local/in-browser iteration tooling.

## Impact

- **`crates/core`, `crates/node`, `crates/schema`, `crates/gateway`, `crates/director`, `crates/storage`**: Zero modifications; strict genre-agnostic core boundaries preserved.
- **`crates/gameplay`**: Additive asteroid chunk coordinates, NPC trade guard, and autonomous `EntityType::Npc` drone lifecycle inside `crates/gameplay/src/asteroid.rs`.
- **`web/`**: Transport stream serialization, authenticated voxel chunk streaming, procedural preview fallback, 3D beacons/reticle, guided mission HUD, `DevToolsController` playtest drawer, and expanded unit + Playwright E2E tests.
- **`scripts/dev-local.sh` & `docker-compose.yml`**: Fast local native dev runner and isolated asteroid stack Compose profile.
