# Competitive 6DoF Voxel Asteroid Miners Specification (Delta)

## MODIFIED Requirements

### Requirement: 6DoF Asteroid Sector Bootstrap & Cross-Genre Plugin Composition
`mmo-asteroid-node` SHALL initialize `TickSimulation` with `MotionMode::Newtonian6DoF`, register `InventoryPlugin`, `TradePlugin`, `VoxelTerrainPlugin`, `CombatPlugin`, and `AsteroidMiningPlugin`, seed 3D voxel asteroids across `SEEDED_ASTEROID_CHUNKS` outside the safe zone, spawn autonomous drone NPCs outside the safe zone, and spawn a Home Orbital Station safe dock without modifying `mmo-core` or `mmo-node`.

#### Scenario: Bootstrapping asteroid sector world and hydrating a newly spawned ship
- **GIVEN** a `TickSimulation` bootstrapped for the Asteroid Sector with `MotionMode::Newtonian6DoF` and `InventoryPlugin`, `TradePlugin`, `VoxelTerrainPlugin`, `CombatPlugin`, and `AsteroidMiningPlugin` installed
- **WHEN** a new `EntityType::Player` ship entity is spawned or hydrated into the simulation
- **THEN** the entity SHALL be spawned at the Home Orbital Station docking coordinates with `MotionMode::Newtonian6DoF`, an initialized `InventoryComponent` for cargo storage, a `HealthComponent` for ship hull integrity, and a `StationVaultComponent` tracking permanently banked resources, and all 10 `SEEDED_ASTEROID_CHUNKS` SHALL be populated outside `HOME_STATION_SAFE_RADIUS` (`15.0m`)

### Requirement: Home Orbital Station Docking, Vault Banking, and Pilot-to-Pilot Trade (`module_id = 0x03`)
`AsteroidMiningPlugin` SHALL register a `ModularPacketHandler` (`module_id = 0x03`) enabling pilots within docking distance of the Home Orbital Station to inspect sector/vault state (`sub_opcode = 1`), deposit unbanked `InventoryComponent` ore into permanent `StationVaultComponent` storage (`sub_opcode = 3`, resetting ship shininess to `0`), and execute atomic two-player trades via `TradePlugin` (`sub_opcode = 4..=7`) strictly between `EntityType::Player` ships.

#### Scenario: Docking at Home Orbital Station to bank mined ore into permanent vault storage
- **GIVEN** a player ship carrying unbanked ore in its `InventoryComponent` (`shininess > 0`) positioned within docking distance of the Home Orbital Station
- **WHEN** the player sends a Station Deposit modular packet (`module_id = 0x03`, `sub_opcode = 3`)
- **THEN** the handler SHALL atomically move the unbanked ore from the player's `InventoryComponent` into the player's permanent `StationVaultComponent` balance, reset the ship's `state_flags` loot-shininess byte to `0`, and return the updated cargo and vault state

#### Scenario: Rejecting pilot trade initiation with an NPC drone
- **GIVEN** a player ship within `5.0m` of an `EntityType::Npc` patrol drone
- **WHEN** the player attempts to initiate a trade session (`module_id = 0x03`, `sub_opcode = 4`) targeting the NPC drone
- **THEN** `AsteroidPacketHandler` SHALL reject the request without creating a `TradeSession`

## ADDED Requirements

### Requirement: Autonomous Sector Patrol Drones & Full-Loot NPC Respawn
`AsteroidMiningPlugin` SHALL spawn deterministic `EntityType::Npc` patrol drones outside the `15.0m` safe zone with `HealthComponent`, `InventoryComponent` ore cargo (`shininess_tier > 0`), and `StationVaultComponent`, advance their orbital patrol and `ctx.octree` coordinates on each tick, and respawn destroyed drones outside the safe zone with restocked ore after transferring loot to the killer.

#### Scenario: Solo pilot destroys a patrolling sector drone for full-loot ore
- **GIVEN** an `EntityType::Npc` patrol drone orbiting outside the `15.0m` safe zone carrying seeded Titanium Ore and Solar Gold (`shininess_tier > 0`)
- **WHEN** an `EntityType::Player` ship outside the safe zone fires `module_id = 0x03`, `sub_opcode = 2` until the drone's hull HP reaches `0`
- **THEN** the drone's unbanked ore cargo SHALL transfer to the player's `InventoryComponent` (increasing the player's `state_flags` shininess tier), and the drone SHALL respawn at its patrol anchor outside the safe zone with full hull HP, reset orbital phase, and restocked starter ore cargo
