# Design

## Context

See `proposal.md` for motivation. The repository already includes all foundational primitives needed for a second, radically different game genre:
1. **3D 6DoF Kinematics**: `crates/node/src/kinematics.rs` provides `MotionMode::Newtonian6DoF` and `Newtonian6DoFIntegrator` (3D velocity, drag damping, speed clamping), while `TickSimulation` already supports per-entity and default kinematic configurations.
2. **Volumetric Voxel Terrain Engine**: `crates/plugins/voxel` (`mmo-plugin-voxel`) implements `VoxelTerrainPlugin` (`VOXEL_MODULE_ID = 0x00A0`), 32×32×32 `PaletteChunk` RLE codecs, server-side excavation validation (`VoxelModifyRequestPacket`), and `VoxelChangeEvent` emission into `TickContext::events`, while `crates/client-wasm/src/voxel.rs` (`WasmVoxelWorld`) provides browser-side RLE decoding and greedy face meshing (`VoxelMeshBuffer`).
3. **Reusable Gameplay Plugins**: `crates/gameplay` provides `InventoryPlugin` (`InventoryComponent`, `ItemStack`, encumbrance), `TradePlugin` (`TradeManager`, atomic two-player item/currency swap), and `CombatPlugin` (`HealthComponent`, `DamageEvent`, `EntityDiedEvent`).
4. **Reusable Node Runner**: `crates/node/src/runner.rs` exposes `run_node_daemon_with_sim(node_id, bounds, tick_sim)`, which `crates/gameplay/src/bin/mmo_village_node.rs` uses to launch Game A without polluting `mmo-node` with gameplay dependencies.

## Goals / Non-Goals

**Goals:**
- Demonstrate that two completely different MMO genres (**Game A: Medieval Village & Barter RPG** and **Game B: Competitive 6DoF Voxel Asteroid Miners**) can run on the exact same `mmo-core`, `mmo-node`, `mmo-director`, `mmo-gateway`, `mmo-storage`, and `mmo-client-wasm` crates with **zero core engine modifications**.
- Showcase both **plugin isolation** (Game A uses `VillagePlugin` + `NpcMovementPlugin`; Game B uses `VoxelTerrainPlugin` + `CombatPlugin` + `AsteroidMiningPlugin`) and **cross-genre plugin sharing** (both games share `InventoryPlugin` and `TradePlugin`).
- Implement the full competitive loop in Game B: 6DoF flight -> shooting voxel asteroids to uncover ore -> ship growing visibly shinier via `state_flags` as loot accumulates -> PvP dogfighting with full-loot cargo transfer on kill -> docking at the Home Orbital Station to bank loot or trade via `TradePlugin`.
- Structure `web/` and `k8s/` so both games can be built, previewed, and deployed as two separate live game deployments in the Kubernetes cluster.

**Non-Goals:**
- Changing wire-protocol framing (`0x4D4D`) or core gRPC cluster protobuf schemas (`mmo-schema`).
- Adding complex progression trees, crafting UIs, or persistent ship equipment customization beyond ore cargo, vault banking, and `TradePlugin` trading.

## Decisions

### 1. Plugin Composition Matrix & Binary Entrypoint (`mmo-asteroid-node`)
- **Decision**: Create `crates/gameplay/src/asteroid.rs` (`AsteroidMiningPlugin`) and `crates/gameplay/src/bin/mmo_asteroid_node.rs` mirroring the clean separation of `mmo_village_node.rs`.
- **Plugin Comparison**:
  - **Game A (`mmo-village-node`)**:
    - Kinematics: `MotionMode::DiscreteGround` (2D XZ movement)
    - Shared Plugins: `InventoryPlugin`, `TradePlugin`
    - Exclusive Plugins: `NpcMovementPlugin`, `VillagePlugin` (`module_id = 0x02`)
  - **Game B (`mmo-asteroid-node`)**:
    - Kinematics: `MotionMode::Newtonian6DoF` (3D XYZ movement)
    - Shared Plugins: `InventoryPlugin`, `TradePlugin`
    - Exclusive Plugins: `VoxelTerrainPlugin` (`module_id = 0x00A0`), `CombatPlugin`, `AsteroidMiningPlugin` (`module_id = 0x03`)
- **Rationale**: Keeps `mmo-node` 100% free of game dependencies while packaging both game node binaries in `crates/gameplay` (and the unified `Dockerfile`).

### 2. Event-Driven Voxel Ore Harvesting (`VoxelTerrainPlugin` -> `EventQueue` -> `InventoryPlugin`)
- **Decision**: Do not couple `mmo-plugin-voxel` to `InventoryPlugin`. Instead, `VoxelTickSystem` already drains `VoxelChangeEvent { actor, coord, old_voxel, new_voxel }` into `TickContext::events`. `AsteroidMiningTickSystem` (registered after `VoxelTerrainPlugin`) reads `ctx.events.read::<VoxelChangeEvent>()`:
  - Voxel material palette for seeded asteroid chunks:
    - `0`: Empty space (vacuum)
    - `1`: Asteroid Silicate Crust (structural rock; excavating carves tunnels to reach inner veins)
    - `2`: Titanium Vein -> grants `ItemStack::new(2001, "Titanium Ore", 1, 2.0)` (unit value: `10`)
    - `3`: Gold Vein -> grants `ItemStack::new(2002, "Solar Gold", 1, 3.0)` (unit value: `30`)
    - `4`: Void Crystal Core -> grants `ItemStack::new(2003, "Void Crystal", 1, 1.0)` (unit value: `75`)
  - When `new_voxel == 0` and `old_voxel` is `2..=4`, `AsteroidMiningTickSystem` credits the corresponding `ItemStack` to `actor`'s `InventoryComponent`.
- **Rationale**: Proves true decoupled communication between two independent plugins (`VoxelTerrainPlugin` and `InventoryPlugin`) mediated strictly by the core `EventQueue`.

### 3. Zero-Protocol-Change Ship "Shininess" Replication via `Entity::state_flags`
- **Decision**: Encode the ship's unbanked cargo loot-value tier (`0..=255`) into `bits 16..23` of `Entity::state_flags` (`(state_flags & 0xFF00_FFFF) | ((shininess_tier as u32) << 16)`):
  - `shininess_tier = ((total_unbanked_cargo_value / 5).min(255)) as u8`.
  - Whenever a ship mines ore, deposits ore at the station, trades ore, or loots a destroyed ship, `update_ship_shininess(entity)` recalculates the byte and updates `entity.state_flags`.
  - Because `AoiManager` already tracks and delta-compresses `state_flags` changes (`delta_flags::STATE_FLAGS`) at 20 Hz, every nearby browser client immediately receives the updated shininess byte in `WasmClientWorld`.
- **Rationale**: Requires zero changes to `mmo-schema` or `mmo-client-wasm` entity snapshot layouts while delivering real-time visual feedback ("the more loot a ship carries, the brighter golden/crystalline it glows").

### 4. PvP Laser Combat, Full-Loot Destruction, and Safe-Zone Respawn
- **Decision**:
  - **Home Orbital Station Safe Zone**: Located at `(0.0, 0.0, 0.0)` with docking/safe-zone radius `15.0` units. Ships inside this radius cannot deal or receive PvP weapon damage.
  - **Laser Weapon Fire (`module_id = 0x03`, `sub_opcode = 2`)**: Client sends `FireLaserRequest { target_guid }`. The server validates caller/target existence, range (`<= 45.0` units), cooldown (`500ms`), and confirms neither ship is inside the `15.0`-unit safe zone before applying hull damage (`25` HP per shot against `100` max HP) via `CombatPlugin` (`DamageEvent` / direct authoritative damage + `EntityDiedEvent`).
  - **Full-Loot Transfer on Kill**: When `EntityDiedEvent { entity: victim, killer: Some(attacker) }` is processed:
    1. All unbanked items and unbanked currency in `victim`'s `InventoryComponent` are transferred into `attacker`'s `InventoryComponent` (up to available slot/weight capacity; excess currency always transfers).
    2. `victim`'s `HealthComponent` is restored to `max_hp`, position is reset to the Home Orbital Station spawn ring `(0.0, 0.0, 8.0)`, and velocity is zeroed.
    3. Both `attacker` and `victim` have their `state_flags` shininess byte recalculated immediately.

### 5. Home Orbital Station Vault Banking & `TradePlugin` Integration (`module_id = 0x03`)
- **Decision**: Attach a serializable `StationVaultComponent { banked_credits: u64, banked_items: Vec<ItemStack>, kills: u32, deaths: u32 }` (registered in `ComponentCodecRegistry` as `"station_vault"`) to each player entity.
- **Modular Sub-Opcodes for `AsteroidMiningPlugin` (`module_id = 0x03`)**:
  - `sub_opcode = 1` (`InspectSectorState`): Returns caller's unbanked `InventoryComponent`, permanent `StationVaultComponent`, current `HealthComponent`, shininess tier, docking status (`in_safe_zone`), and optional inspected target ship/station info.
  - `sub_opcode = 2` (`FirePvpLaser`): Authoritative ship-to-ship weapon fire against `target_guid`.
  - `sub_opcode = 3` (`DepositToStationVault`): Requires caller to be within `15.0` units of the Home Orbital Station `(0.0, 0.0, 0.0)`. Converts/moves all unbanked ore from `InventoryComponent` into `StationVaultComponent` (`banked_credits` + `banked_items`) and resets ship `state_flags` shininess to `0`.
  - `sub_opcode = 4..=7` (`PlayerTradeStart`, `PlayerTradeOffer`, `PlayerTradeAccept`, `PlayerTradeCancel`): Delegates directly to the shared `TradeManager` (`TradePlugin`) so two pilots docked at the Home Orbital Station can trade rare ores or credits using the exact same atomic transaction engine as medieval villagers in Game A.

### 6. Dual-Game Browser Client Architecture (`web/`)
- **Decision**: Refactor `web/src/` into a shared engine core and two selectable game cartridges:
  - **Shared Substrate (`web/src/core/` or existing root modules)**:
    - `transport.js`: WebTransport + WebSocket fallback + offline demo transport.
    - `protocol.js`: Wire framing (`0x4D4D`), `MovementPacket`, `InteractionPacket`, AoI snapshot decoding.
    - `WasmClientWorld` & `WasmVoxelWorld`: Entity interpolation and greedy voxel chunk meshing.
  - **Game A Cartridge (`web/src/games/village/`)**:
    - Grounded 3D village plaza renderer, 2D ground WASD controller, villager NPC inspection, and Barter/Trade UI (`module_id = 0x02`).
  - **Game B Cartridge (`web/src/games/asteroid/`)**:
    - Deep-space starfield & Orbital Station 3D renderer, greedy-meshed voxel asteroid chunks (`WasmVoxelWorld`, `module_id = 0x00A0`) colored by voxel material (`Crust Rock`, `Titanium`, `Solar Gold`, `Void Crystal`).
    - 6DoF flight controls (`W/S` thrust forward/back, `A/D` strafe X, `Space/C` ascend/descend Y, mouse/click to fire mining beam at voxel asteroid or PvP laser at rival ship).
    - Dynamic Ship Shader: Reads `(entity.state_flags >> 16) & 0xFF` every frame and scales the ship's metallic gold/crystalline emissive material, point light aura, and loot-crown indicator so rich targets visibly shine across space.
    - Orbital Station HUD (`module_id = 0x03`): Hull HP bar, Unbanked Cargo vs. Safe Station Vault panel, "Dock & Bank Loot" button when inside the station safe ring, and Pilot-to-Pilot `TradePlugin` window.
  - **Mode Selection**: Resolved from `?game=village|asteroid` URL parameter, `import.meta.env.VITE_GAME_MODE`, or an in-UI game switcher banner so developers can also flip between Game A and Game B during local playtesting.

### 7. Multi-Game Kubernetes Deployment Layout (`k8s/`)
- **Decision**:
  - `Dockerfile` builds both `mmo-village-node` and `mmo-asteroid-node` into `/usr/local/bin/`.
  - Provide Kustomize overlays (e.g., `k8s/overlays/game-village/` in namespace `mmo-village` and `k8s/overlays/game-asteroid/` in namespace `mmo-asteroid`, or dual deployments within the cluster) where:
    - Both namespaces run the exact same `mmo-director` and `mmo-gateway` deployments.
    - `game-village` patches `mmo-node` StatefulSet command to `exec mmo-village-node` and `mmo-web` with `GAME_MODE=village`.
    - `game-asteroid` patches `mmo-node` StatefulSet command to `exec mmo-asteroid-node` and `mmo-web` with `GAME_MODE=asteroid`.

## Risks / Trade-offs

- **[Risk] High-rate PvP or voxel laser clicks flooding node ingress** → **Mitigation**: `VoxelWorld` already enforces per-entity `ExcavationRateLimiter` and reach checks; `AsteroidMiningPlugin` will enforce a strict `500ms` server-side weapon cooldown per entity GUID.
- **[Risk] Players losing banked progress on PvP death** → **Mitigation**: Only unbanked cargo in `InventoryComponent` is vulnerable to PvP full-loot transfer; once deposited into `StationVaultComponent` at the Home Orbital Station, resources are permanently safe.
- **[Risk] Maintaining two completely separate `web/` codebases causing drift in transport/protocol fixes** → **Mitigation**: Keep `web/` as a single package with shared `transport.js`, `protocol.js`, and `mmo-client-wasm` bindings, isolating only the scene renderer, input controller, and HUD panels per game mode.
