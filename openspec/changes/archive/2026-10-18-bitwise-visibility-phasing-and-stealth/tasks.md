# Implementation Tasks: Bitwise Visibility, Phasing & Stealth AoI Culling

- [ ] 1. Add Visibility, Phase & Stealth Fields to `Entity` and `WorldObject` (`crates/core`)
  - [ ] 1.1 Add `phase_mask: u64` (`1`), `visibility_mask: u64` (`u64::MAX`), `stealth_level: u8` (`0`), `detection_level: u8` (`0`), and `Entity::can_observe(&self, target: &Entity) -> bool` to `crates/core/src/entity.rs`.
  - [ ] 1.2 Add `phase_mask: u64` (default `1`) to `WorldObject` in `crates/core/src/world.rs`.
  - [ ] 1.3 Add unit tests in `crates/core/src/entity.rs` testing `can_observe` across phase masks, visibility masks, and stealth/detection levels.

- [ ] 2. Filter Entities and World Objects in `AoiManager` (`crates/node/src/aoi.rs`)
  - [ ] 2.1 Filter `self.scratch_query` by `obs.can_observe(tgt)` prior to `throttle_candidates` in `AoiManager::update_observers`.
  - [ ] 2.2 Filter `WorldObject` instances by `(observer_phase & obj.phase_mask) != 0` in `AoiManager::update_world_object_observers`.
  - [ ] 2.3 Add unit tests in `crates/node/src/aoi.rs` verifying: (a) stealthed entities are hidden from low-detection observers and visible to high-detection observers, (b) entering stealth or changing `phase_mask` emits an immediate despawn in `EntityDeltaBatchPacket.despawns`, and (c) stealthed entities do not consume `max_entities` density throttling slots.

- [ ] 3. Workspace Quality Gates
  - [ ] 3.1 Run `cargo fmt --all -- --check`
  - [ ] 3.2 Run `cargo clippy --workspace --all-targets -- -D warnings`
  - [ ] 3.3 Run `cargo test --workspace`
