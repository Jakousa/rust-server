# Technical Design: Bitwise Visibility, Phasing & Stealth AoI Culling

## 1. `Entity` & `WorldObject` Visibility Fields (`crates/core`)

### 1.1 `crates/core/src/entity.rs`
Add fields to `Entity`:
```rust
pub phase_mask: u64,
pub visibility_mask: u64,
pub stealth_level: u8,
pub detection_level: u8,
```
Initialize in `Entity::new` and `Entity::with_components`:
- `phase_mask: 1`
- `visibility_mask: u64::MAX`
- `stealth_level: 0`
- `detection_level: 0`

Add method on `Entity`:
```rust
#[inline]
pub fn can_observe(&self, target: &Entity) -> bool {
    (self.phase_mask & target.phase_mask) != 0
        && (self.visibility_mask & target.visibility_mask) != 0
        && (target.stealth_level == 0 || self.detection_level >= target.stealth_level)
}
```

### 1.2 `crates/core/src/world.rs`
Add `#[serde(default = "default_phase_mask")] pub phase_mask: u64` (`fn default_phase_mask() -> u64 { 1 }`) to `WorldObject`.

---

## 2. Pre-Throttle AoI Culling (`crates/node/src/aoi.rs`)

### 2.1 `AoiManager::update_observers` (`crates/node/src/aoi.rs:491-560`)
- After `octree.query_radius_with_dist_sq_into(observer_pos, observer.effective_radius, &mut self.scratch_query)`:
- Look up `let observer_ent = entities.get_by_guid(observer_guid);`.
- In `self.scratch_query.retain(|&(guid, _)| ...)`:
  ```rust
  if guid == observer_guid {
      return false;
  }
  if let (Some(obs), Some(tgt)) = (observer_ent, entities.get_by_guid(guid)) {
      if !obs.can_observe(tgt) {
          return false;
      }
  }
  true
  ```
- Because this runs **before** `Self::throttle_candidates(&mut self.scratch_query, observer.max_entities)`, invisible or phased-out entities never crowd out visible entities under high density!
- And because `self.scratch_visible_guids` only contains GUIDs that passed `can_observe`, any previously visible entity that entered stealth or switched phases is automatically added to `despawned_guids` and removed from `observer.visible_entities` and `observer.last_sent_states`.

### 2.2 `AoiManager::update_world_object_observers` (`crates/node/src/aoi.rs:881-1012`)
- Look up `let observer_phase = entities.get_by_guid(observer_guid).map(|e| e.phase_mask).unwrap_or(1);`.
- When iterating sector objects, skip any object where `(observer_phase & obj.phase_mask) == 0`.
