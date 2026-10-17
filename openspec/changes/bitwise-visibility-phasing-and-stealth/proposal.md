# Bitwise Visibility Masks, Phasing, and Stealth/Detection Culling Substrate Proposal

## Context & Problem Statement
In popular MMOs across all genres—Fantasy RPGs (*World of Warcraft*, *FFXIV*), Space Simulations (*EVE Online*), and Tactical PvP Sandboxes (*Albion Online*)—Euclidean distance alone is insufficient to determine whether Observer A is allowed to receive network state updates for Entity B:

1. **Story / Dimension Phasing (`phase_mask`)**:
   - Two players standing at the exact same `(x, y, z)` coordinates may occupy different quest phases, dimensions, or wormhole layers.
2. **Visibility Groups / Fog-of-War (`visibility_mask`)**:
   - Spectating admins, faction-restricted illusions, or tactical vision channels require bitwise group matching.
3. **Stealth, Cloaking & Sensor Detection (`stealth_level` vs `detection_level`)**:
   - A stealthed rogue, cloaked spaceship, or camouflaged unit must **never** have its position packets sent over the wire to observers whose detection/sensor rating is lower than the target's stealth rating—otherwise client-side packet inspection or wallhack/radar tools trivially bypass stealth.
   - Furthermore, when an already-visible entity enters stealth or shifts into a disjoint phase, the server must immediately include its `Guid` in `EntityDeltaBatchPacket.despawns` on that exact tick.

## Proposed Solution

1. **Mathematical Visibility Header on `Entity` and `WorldObject` (`crates/core`)**:
   - Add `pub phase_mask: u64` (default `1`), `pub visibility_mask: u64` (default `u64::MAX`), `pub stealth_level: u8` (default `0`), and `pub detection_level: u8` (default `0`) to `mmo_core::Entity` (`crates/core/src/entity.rs`).
   - Add `pub phase_mask: u64` (default `1`) to `mmo_core::world::WorldObject` (`crates/core/src/world.rs`).
   - Implement a branchless/inline $O(1)$ predicate `Entity::can_observe(&self, target: &Entity) -> bool`:
     ```rust
     (self.phase_mask & target.phase_mask) != 0
         && (self.visibility_mask & target.visibility_mask) != 0
         && (target.stealth_level == 0 || self.detection_level >= target.stealth_level)
     ```

2. **Pre-Throttle AoI Filtering & Automatic Despawn Emission (`crates/node/src/aoi.rs`)**:
   - In `AoiManager::update_observers`, filter `self.scratch_query` using `observer_ent.can_observe(target_ent)` **before** `throttle_candidates` so stealthed or phased-out entities never consume an observer's `max_entities` density budget.
   - When an entity already in `observer.visible_entities` transitions to `can_observe == false` (e.g., entering stealth or changing phase), `AoiManager` automatically emits its `Guid` in `EntityDeltaBatchPacket.despawns` and removes it from `observer.visible_entities` and `observer.last_sent_states`.
   - In `AoiManager::update_world_object_observers`, filter world objects by `(observer_phase_mask & obj.phase_mask) != 0`.

## Scope & Non-Goals
- **In Scope**: `crates/core/src/{entity.rs, world.rs}`, `crates/node/src/aoi.rs`, and unit tests.
- **Non-Goals**: Hardcoding RPG class stealth formulas in core crates (plugins set `stealth_level`, `detection_level`, and `phase_mask`).
