//! Authoritative spatial sector manager, dynamic streaming, and anti-cheat placement validation.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use thiserror::Error;

use mmo_core::guid::Guid;
use mmo_core::octree::BoundingBox3D;
use mmo_core::world::{SectorCoord3D, SectorSpatialIndex, WorldObject};
use mmo_storage::db::StorageError;
use mmo_storage::world_repo::WorldObjectRepository;

use crate::world_snapshotter::record_to_world_object;

/// Maximum permissible placement and interaction range in meters.
pub const MAX_INTERACTION_DISTANCE: f32 = 6.0;

/// Default capacity for placement token bucket rate limiting.
pub const DEFAULT_RATE_LIMIT_CAPACITY: f32 = 10.0;

/// Default token refill rate per second for placement actions.
pub const DEFAULT_RATE_LIMIT_REFILL_PER_SEC: f32 = 5.0;

/// Standard eye height offset in meters for player line-of-sight checks.
pub const PLAYER_EYE_OFFSET_Y: f32 = 1.6;

/// Placement validation errors.
#[derive(Debug, Error, PartialEq)]
pub enum PlacementError {
    #[error("Placement distance {distance:.2}m exceeds maximum permissible distance of {max:.2}m")]
    OutOfRange { distance: f32, max: f32 },

    #[error("Placement rate limit exceeded (token bucket exhausted)")]
    RateLimitExceeded,

    #[error("Placement line-of-sight obstructed by object {0}")]
    LineOfSightBlocked(Guid),

    #[error("Target location intersects existing world object {0}")]
    IntersectingObject(Guid),

    #[error("Target sector is not loaded on this simulation node")]
    SectorNotLoaded(SectorCoord3D),

    #[error("Invalid transform or dimension parameters: {0}")]
    InvalidTransform(String),
}

/// Token bucket entry for tracking client action rates.
#[derive(Debug, Clone)]
struct TokenBucket {
    tokens: f32,
    last_update: Instant,
}

impl TokenBucket {
    fn new(capacity: f32) -> Self {
        Self {
            tokens: capacity,
            last_update: Instant::now(),
        }
    }

    fn try_consume(&mut self, capacity: f32, refill_per_sec: f32) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_update).as_secs_f32();
        self.last_update = now;

        self.tokens = (self.tokens + elapsed * refill_per_sec).min(capacity);

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Authoritative spatial sector manager for active world entities.
pub struct SectorManager {
    spatial_index: SectorSpatialIndex,
    loaded_sectors: HashSet<SectorCoord3D>,
    rate_limits: HashMap<Guid, TokenBucket>,
    rate_limit_capacity: f32,
    rate_limit_refill_per_sec: f32,
}

impl Default for SectorManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SectorManager {
    /// Constructs an empty `SectorManager` with default rate limiting parameters.
    pub fn new() -> Self {
        Self {
            spatial_index: SectorSpatialIndex::new(),
            loaded_sectors: HashSet::new(),
            rate_limits: HashMap::new(),
            rate_limit_capacity: DEFAULT_RATE_LIMIT_CAPACITY,
            rate_limit_refill_per_sec: DEFAULT_RATE_LIMIT_REFILL_PER_SEC,
        }
    }

    /// Returns the underlying spatial index.
    pub fn spatial_index(&self) -> &SectorSpatialIndex {
        &self.spatial_index
    }

    /// Returns a mutable reference to the underlying spatial index.
    pub fn spatial_index_mut(&mut self) -> &mut SectorSpatialIndex {
        &mut self.spatial_index
    }

    /// Returns the set of currently active/loaded sectors on this node.
    pub fn loaded_sectors(&self) -> &HashSet<SectorCoord3D> {
        &self.loaded_sectors
    }

    /// Returns `true` if the specified sector coordinate is currently loaded.
    pub fn is_sector_loaded(&self, sector: SectorCoord3D) -> bool {
        self.loaded_sectors.contains(&sector)
    }

    /// Mark a sector as loaded manually without populating from DB.
    pub fn mark_sector_loaded(&mut self, sector: SectorCoord3D) {
        self.loaded_sectors.insert(sector);
    }

    /// Asynchronously loads all persistent world objects in the given sector from storage.
    pub async fn load_sector(
        &mut self,
        world_id: u64,
        sector: SectorCoord3D,
        repo: &WorldObjectRepository,
    ) -> Result<usize, StorageError> {
        let records = repo
            .get_objects_in_sector(world_id, sector.x, sector.y, sector.z)
            .await?;

        let mut count = 0;
        for record in records {
            if let Some(obj) = record_to_world_object(&record) {
                self.spatial_index.insert(obj);
                count += 1;
            }
        }

        self.loaded_sectors.insert(sector);
        Ok(count)
    }

    /// Unloads a sector from memory, removing and returning all its resident world objects.
    pub fn unload_sector(&mut self, sector: SectorCoord3D) -> Option<Vec<WorldObject>> {
        if !self.loaded_sectors.remove(&sector) {
            return None;
        }

        let guids: Vec<Guid> = self
            .spatial_index
            .objects_in_sector(sector)
            .map(|map| map.keys().copied().collect())
            .unwrap_or_default();

        let mut unloaded = Vec::with_capacity(guids.len());
        for guid in guids {
            if let Some(obj) = self.spatial_index.remove(guid) {
                unloaded.push(obj);
            }
        }

        Some(unloaded)
    }

    /// Checks and consumes a token from the player's placement rate limiter bucket.
    pub fn check_rate_limit(&mut self, player_guid: Guid) -> bool {
        let cap = self.rate_limit_capacity;
        let refill = self.rate_limit_refill_per_sec;
        let bucket = self
            .rate_limits
            .entry(player_guid)
            .or_insert_with(|| TokenBucket::new(cap));
        bucket.try_consume(cap, refill)
    }

    /// Authoritatively validates a placement request against distance, rate limits, line of sight, and collision.
    pub fn validate_placement(
        &mut self,
        player_guid: Guid,
        player_pos: (f32, f32, f32),
        target_pos: (f32, f32, f32),
        candidate_bounds: &BoundingBox3D,
    ) -> Result<(), PlacementError> {
        // 1. Rate limiter check
        if !self.check_rate_limit(player_guid) {
            return Err(PlacementError::RateLimitExceeded);
        }

        // 2. Max distance validation
        let dx = target_pos.0 - player_pos.0;
        let dy = target_pos.1 - player_pos.1;
        let dz = target_pos.2 - player_pos.2;
        let distance = (dx * dx + dy * dy + dz * dz).sqrt();
        if distance > MAX_INTERACTION_DISTANCE {
            return Err(PlacementError::OutOfRange {
                distance,
                max: MAX_INTERACTION_DISTANCE,
            });
        }

        // 3. Sector loaded check
        let sector = SectorCoord3D::from_world_pos(target_pos);
        if !self.is_sector_loaded(sector) {
            return Err(PlacementError::SectorNotLoaded(sector));
        }

        // 4. Line-of-sight raycast check from eye level
        let eye_pos = (
            player_pos.0,
            player_pos.1 + PLAYER_EYE_OFFSET_Y,
            player_pos.2,
        );
        let ray_dir_unnorm = (
            target_pos.0 - eye_pos.0,
            target_pos.1 - eye_pos.1,
            target_pos.2 - eye_pos.2,
        );
        let ray_dist = (ray_dir_unnorm.0 * ray_dir_unnorm.0
            + ray_dir_unnorm.1 * ray_dir_unnorm.1
            + ray_dir_unnorm.2 * ray_dir_unnorm.2)
            .sqrt();

        if ray_dist > 1e-4 {
            let ray_dir = (
                ray_dir_unnorm.0 / ray_dist,
                ray_dir_unnorm.1 / ray_dist,
                ray_dir_unnorm.2 / ray_dist,
            );
            // Check if any object obstructs the line of sight before reaching target (leaving a 0.1m tolerance margin at destination)
            let max_check_dist = (ray_dist - 0.1).max(0.0);
            if max_check_dist > 0.0 {
                if let Some((hit_obj, hit_dist)) =
                    self.spatial_index.raycast(eye_pos, ray_dir, max_check_dist)
                {
                    if hit_dist < max_check_dist {
                        return Err(PlacementError::LineOfSightBlocked(hit_obj.object_id));
                    }
                }
            }
        }

        // 5. Non-penetration / AABB collision test
        let overlapping = self.spatial_index.query_aabb(candidate_bounds);
        for obj in overlapping {
            if obj.bounds.intersects(candidate_bounds) {
                return Err(PlacementError::IntersectingObject(obj.object_id));
            }
        }

        Ok(())
    }

    /// Inserts or updates an object in the spatial index.
    pub fn insert_object(&mut self, object: WorldObject) -> Option<WorldObject> {
        let sector = object.sector();
        self.loaded_sectors.insert(sector);
        self.spatial_index.insert(object)
    }

    /// Removes an object by GUID.
    pub fn remove_object(&mut self, object_id: Guid) -> Option<WorldObject> {
        self.spatial_index.remove(object_id)
    }

    /// Retrieves an object by GUID.
    pub fn get_object(&self, object_id: Guid) -> Option<&WorldObject> {
        self.spatial_index.get(object_id)
    }

    /// Retrieves a mutable reference to an object by GUID.
    pub fn get_object_mut(&mut self, object_id: Guid) -> Option<&mut WorldObject> {
        self.spatial_index.get_mut(object_id)
    }

    /// Queries all objects residing in a given sector.
    pub fn query_sector(&self, sector: SectorCoord3D) -> Option<&HashMap<Guid, WorldObject>> {
        self.spatial_index.objects_in_sector(sector)
    }

    /// Queries all objects overlapping a given AABB bounding box.
    pub fn query_aabb(&self, aabb: &BoundingBox3D) -> Vec<&WorldObject> {
        self.spatial_index.query_aabb(aabb)
    }

    /// Queries all objects within a spherical radius.
    pub fn query_sphere(&self, center: (f32, f32, f32), radius: f32) -> Vec<&WorldObject> {
        self.spatial_index.query_sphere(center, radius)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::container::ContainerOwner;
    use mmo_core::world::Transform3D;

    fn make_obj(id: u64, pos: (f32, f32, f32), min_extent: f32, max_extent: f32) -> WorldObject {
        WorldObject::new(
            Guid(id),
            1,
            Transform3D::new(pos, (0.0, 0.0, 0.0, 1.0), (1.0, 1.0, 1.0)),
            BoundingBox3D::new(
                (pos.0 - min_extent, pos.1 - min_extent, pos.2 - min_extent),
                (pos.0 + max_extent, pos.1 + max_extent, pos.2 + max_extent),
            ),
            ContainerOwner::Character(10),
        )
    }

    #[test]
    fn test_placement_validation_distance_and_bounds() {
        let mut mgr = SectorManager::new();
        let target_sector = SectorCoord3D::from_world_pos((3.0, 0.0, 0.0));
        mgr.mark_sector_loaded(target_sector);

        let player_pos = (0.0, 0.0, 0.0);
        let candidate_bounds = BoundingBox3D::new((2.5, 0.0, -0.5), (3.5, 1.0, 0.5));

        // 1. Valid placement within 6.0m
        let res = mgr.validate_placement(Guid(1), player_pos, (3.0, 0.0, 0.0), &candidate_bounds);
        assert!(res.is_ok());

        // 2. Out of range (> 6.0m)
        let far_pos = (10.0, 0.0, 0.0);
        let far_bounds = BoundingBox3D::new((9.5, 0.0, -0.5), (10.5, 1.0, 0.5));
        let far_res = mgr.validate_placement(Guid(1), player_pos, far_pos, &far_bounds);
        assert!(matches!(far_res, Err(PlacementError::OutOfRange { .. })));

        // 3. Collision with existing object
        let wall = make_obj(99, (3.3, 0.0, 0.0), 0.1, 0.1);
        mgr.insert_object(wall);

        let coll_res =
            mgr.validate_placement(Guid(1), player_pos, (3.0, 0.0, 0.0), &candidate_bounds);
        assert!(matches!(
            coll_res,
            Err(PlacementError::IntersectingObject(Guid(99)))
        ));
    }

    #[test]
    fn test_placement_los_block() {
        let mut mgr = SectorManager::new();
        let sector = SectorCoord3D::from_world_pos((4.0, 0.0, 0.0));
        mgr.mark_sector_loaded(sector);

        // Place obstacle wall directly between player (0,0,0) and target (4,0,0) at (2, 0.8, 0)
        let wall = make_obj(50, (2.0, 0.8, 0.0), 0.5, 0.5);
        mgr.insert_object(wall);

        let player_pos = (0.0, 0.0, 0.0);
        let target_pos = (4.0, 0.0, 0.0);
        let target_bounds = BoundingBox3D::new((3.8, 0.0, -0.2), (4.2, 1.0, 0.2));

        let res = mgr.validate_placement(Guid(1), player_pos, target_pos, &target_bounds);
        assert!(matches!(
            res,
            Err(PlacementError::LineOfSightBlocked(Guid(50)))
        ));
    }

    #[test]
    fn test_placement_rate_limiter() {
        let mut mgr = SectorManager::new();
        let sector = SectorCoord3D::from_world_pos((1.0, 0.0, 0.0));
        mgr.mark_sector_loaded(sector);

        let player = Guid(42);
        let player_pos = (0.0, 0.0, 0.0);
        let target_pos = (1.0, 0.0, 0.0);
        let bounds = BoundingBox3D::new((0.9, 0.0, -0.1), (1.1, 1.0, 0.1));

        // Consume all 10 tokens
        for _ in 0..10 {
            assert!(mgr
                .validate_placement(player, player_pos, target_pos, &bounds)
                .is_ok());
        }

        // 11th placement without time elapsed should be rejected by rate limiter
        let rate_limited = mgr.validate_placement(player, player_pos, target_pos, &bounds);
        assert_eq!(rate_limited, Err(PlacementError::RateLimitExceeded));
    }
}
