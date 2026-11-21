# Competitive 6DoF Voxel Asteroid Miners Specification

## Purpose

Defines the authoritative gameplay mechanics, cross-plugin event integration, 6DoF kinematic configuration, loot-shininess visual state replication, PvP full-loot destruction, and Home Orbital Station docking/trading for the Competitive 6DoF Voxel Asteroid Miners MMO game (`mmo-asteroid-node`), demonstrating cross-genre reuse of `InventoryPlugin` and `TradePlugin` without modifying core engine crates.

## ADDED Requirements

### Requirement: 6DoF Asteroid Sector Bootstrap & Cross-Genre Plugin Composition
`mmo-asteroid-node` SHALL initialize `TickSimulation` with `MotionMode::Newtonian6DoF`, register `InventoryPlugin`, `TradePlugin`, `VoxelTerrainPlugin`, `CombatPlugin`, and `AsteroidMiningPlugin`, seed 3D voxel asteroids (`Crust = 1`, `Titanium = 2`, `Gold = 3`, `Void Crystal = 4`), and spawn a Home Orbital Station safe dock without adding game dependencies to `mmo-core` or `mmo-node`.

#### Scenario: Bootstrapping asteroid sector world and hydrating a newly spawned ship
- **GIVEN** a `TickSimulation` bootstrapped for the Asteroid Sector with `MotionMode::Newtonian6DoF` and `InventoryPlugin`, `TradePlugin`, `VoxelTerrainPlugin`, `CombatPlugin`, and `AsteroidMiningPlugin` installed
- **WHEN** a new `EntityType::Player` ship entity is spawned or hydrated into the simulation
- **THEN** the entity SHALL be spawned at the Home Orbital Station docking coordinates with `MotionMode::Newtonian6DoF`, an initialized `InventoryComponent` for cargo storage, a `HealthComponent` for ship hull integrity, and a `StationVaultComponent` tracking permanently banked resources

### Requirement: Voxel Asteroid Ore Excavation & Cargo Harvesting
`AsteroidMiningPlugin` SHALL observe `VoxelChangeEvent` notifications emitted by `VoxelTerrainPlugin` (`module_id = 0x00A0`) during the simulation tick and, whenever an ore voxel (`new_voxel = 0` and `old_voxel` in `{2, 3, 4}`) is excavated by a player entity, SHALL credit the corresponding ore `ItemStack` into the player's `InventoryComponent` subject to container capacity and weight limits.

#### Scenario: Blasting an embedded Gold or Void Crystal voxel inside an asteroid
- **GIVEN** a player ship within excavation reach of a loaded voxel asteroid chunk containing a Gold voxel (`old_voxel = 3`)
- **WHEN** the player excavates that voxel to empty space (`new_voxel = 0`) and `VoxelTerrainPlugin` pushes a `VoxelChangeEvent`
- **THEN** `AsteroidMiningPlugin` SHALL add the corresponding Gold Ore `ItemStack` to the player's `InventoryComponent` on the same simulation tick

### Requirement: Dynamic Ship Loot-Shininess Replication via Entity State Flags
`AsteroidMiningPlugin` SHALL compute a normalized cargo loot-value tier (`0..=255`) from the total value of unbanked ore in each ship's `InventoryComponent` and encode it into `bits 16..23` of the entity's authoritative `state_flags` so that Area-of-Interest (AoI) delta snapshots automatically broadcast the ship's visual shininess level to all nearby observers at 20 Hz without altering wire-protocol packet schemas.

#### Scenario: Ship shininess tier increases as high-value ore is mined
- **GIVEN** a player ship starting with an empty `InventoryComponent` and `state_flags` loot-shininess byte equal to `0`
- **WHEN** the player mines Titanium, Gold, or Void Crystal ore into their unbanked `InventoryComponent`
- **THEN** the entity's authoritative `state_flags` bits `16..23` SHALL be updated to reflect the increased cargo value tier (`> 0`) and marked dirty for the next AoI delta broadcast

### Requirement: PvP Ship Combat, Full-Loot Cargo Transfer, and Station Respawn
`AsteroidMiningPlugin` SHALL process ship-to-ship weapon fire (`module_id = 0x03`, `sub_opcode = 2`) with range, cooldown, and safe-zone validation via `CombatPlugin`, and on `EntityDiedEvent` SHALL transfer all unbanked ore from the victim's `InventoryComponent` to the killer's `InventoryComponent`, update both ships' `state_flags` shininess bytes, restore the victim's hull HP, and respawn the victim at the Home Orbital Station.

#### Scenario: Destroying a loot-laden rival ship transfers its unbanked ore and shininess to the attacker
- **GIVEN** Victim Ship B carrying unbanked Gold and Void Crystal ore in its `InventoryComponent` (`shininess > 0`) outside the Home Orbital Station safe zone, and Attacker Ship A within weapon range
- **WHEN** Attacker Ship A fires a laser weapon that reduces Victim Ship B's `HealthComponent` to `0` (`EntityDiedEvent`)
- **THEN** Victim Ship B's unbanked ore items SHALL be removed from Ship B's `InventoryComponent` and added to Attacker Ship A's `InventoryComponent`, Ship A's `state_flags` shininess byte SHALL increase while Ship B's resets to `0`, and Ship B SHALL respawn at the Home Orbital Station dock with full hull HP

#### Scenario: Safe-zone protection prevents PvP damage at the Home Orbital Station dock
- **GIVEN** a ship positioned within the Home Orbital Station safe-dock radius
- **WHEN** another ship attempts to fire a PvP weapon targeting that docked ship
- **THEN** the server SHALL reject the attack without applying hull damage or transferring cargo

### Requirement: Home Orbital Station Docking, Vault Banking, and Pilot-to-Pilot Trade (`module_id = 0x03`)
`AsteroidMiningPlugin` SHALL register a `ModularPacketHandler` (`module_id = 0x03`) enabling pilots within docking distance of the Home Orbital Station to inspect sector/vault state (`sub_opcode = 1`), deposit unbanked `InventoryComponent` ore into permanent `StationVaultComponent` storage (`sub_opcode = 3`, resetting ship shininess to `0`), and execute atomic two-player trades via `TradePlugin` (`sub_opcode = 4..=7`).

#### Scenario: Docking at Home Orbital Station to bank mined ore into permanent vault storage
- **GIVEN** a player ship carrying unbanked ore in its `InventoryComponent` (`shininess > 0`) positioned within docking distance of the Home Orbital Station
- **WHEN** the player sends a Station Deposit modular packet (`module_id = 0x03`, `sub_opcode = 3`)
- **THEN** the handler SHALL atomically move the unbanked ore from the player's `InventoryComponent` into the player's permanent `StationVaultComponent` balance, reset the ship's `state_flags` loot-shininess byte to `0`, and return the updated cargo and vault state
