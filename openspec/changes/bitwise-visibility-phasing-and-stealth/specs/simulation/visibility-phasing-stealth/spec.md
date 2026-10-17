# Bitwise Visibility, Phasing, and Stealth AoI Culling Specification

## Purpose
Provides a $O(1)$ mathematical visibility substrate (`phase_mask`, `visibility_mask`, `stealth_level`, `detection_level`) in `mmo-core` and `AoiManager` so stealthed, cloaked, or phased-out entities are culled server-side before delta encoding and density throttling.

## Requirements

### Requirement: Bitwise Phase, Visibility Mask, and Stealth vs Detection Predicate
Every `Entity` SHALL carry `phase_mask: u64` (default `1`), `visibility_mask: u64` (default `u64::MAX`), `stealth_level: u8` (default `0`), and `detection_level: u8` (default `0`). `observer.can_observe(target)` SHALL evaluate to `true` if and only if `(observer.phase_mask & target.phase_mask) != 0`, `(observer.visibility_mask & target.visibility_mask) != 0`, and `(target.stealth_level == 0 || observer.detection_level >= target.stealth_level)`.

#### Scenario: Stealth culling and sensor detection override
- **GIVEN** Observer A (`detection_level = 0`), Observer B (`detection_level = 20`), and Target T (`stealth_level = 15`) all at `(0.0, 0.0, 0.0)` in phase `1`
- **WHEN** `AoiManager::update_observers` evaluates visibility
- **THEN** Observer A SHALL NOT receive deltas for Target T, and Observer B SHALL receive deltas for Target T

### Requirement: Immediate Despawn Packet on Stealth Entry or Phase Transition
When an entity previously visible to an observer changes its `stealth_level`, `phase_mask`, or `visibility_mask` such that `observer.can_observe(target)` becomes `false`, `AoiManager::update_observers` SHALL immediately emit the entity's `Guid` in `EntityDeltaBatchPacket.despawns` on that tick.

#### Scenario: Visible target activates stealth
- **GIVEN** Target T is in `observer.visible_entities` on tick `1`
- **WHEN** Target T sets `stealth_level = 10` on tick `2` while the observer has `detection_level = 0`
- **THEN** the observer's `EntityDeltaBatchPacket` on tick `2` SHALL contain Target T's `Guid` in `despawns` and zero delta records for Target T

### Requirement: World Object Phase Mask Filtering
`WorldObject` SHALL carry `phase_mask: u64` (default `1`), and `AoiManager::update_world_object_observers` SHALL only replicate world objects to observers where `(observer.phase_mask & object.phase_mask) != 0`.

#### Scenario: Phased world object isolation
- **GIVEN** a `WorldObject` with `phase_mask = 0b0010` and an observer in `phase_mask = 0b0001`
- **WHEN** `AoiManager::update_world_object_observers` runs
- **THEN** the world object SHALL NOT be spawned for that observer
