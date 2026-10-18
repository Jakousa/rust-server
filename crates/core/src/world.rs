//! Persistent World Objects, Dynamic Prefabs, and Spatial Sector domain primitives.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::container::ContainerOwner;
use crate::guid::Guid;
use crate::octree::BoundingBox3D;

/// Uniform dimension of a cubic spatial sector in meters ($64\text{m} \times 64\text{m} \times 64\text{m}$).
pub const SECTOR_SIZE: f32 = 64.0;

/// Discrete 3D spatial sector coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SectorCoord3D {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl SectorCoord3D {
    /// Constructs a new `SectorCoord3D`.
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// Computes the sector coordinate enclosing the given floating-point world position.
    #[inline]
    pub fn from_world_pos(pos: (f32, f32, f32)) -> Self {
        Self {
            x: (pos.0 / SECTOR_SIZE).floor() as i32,
            y: (pos.1 / SECTOR_SIZE).floor() as i32,
            z: (pos.2 / SECTOR_SIZE).floor() as i32,
        }
    }

    /// Returns the Axis-Aligned Bounding Box (AABB) corresponding to this sector in world space.
    #[inline]
    pub fn bounds(&self) -> BoundingBox3D {
        let min_x = self.x as f32 * SECTOR_SIZE;
        let min_y = self.y as f32 * SECTOR_SIZE;
        let min_z = self.z as f32 * SECTOR_SIZE;
        BoundingBox3D::new(
            (min_x, min_y, min_z),
            (
                min_x + SECTOR_SIZE,
                min_y + SECTOR_SIZE,
                min_z + SECTOR_SIZE,
            ),
        )
    }

    /// Returns all 27 immediate spatial neighbor sector coordinates (including self).
    pub fn neighborhood(&self) -> Vec<SectorCoord3D> {
        let mut neighbors = Vec::with_capacity(27);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    neighbors.push(SectorCoord3D::new(self.x + dx, self.y + dy, self.z + dz));
                }
            }
        }
        neighbors
    }
}

impl fmt::Display for SectorCoord3D {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sector({}, {}, {})", self.x, self.y, self.z)
    }
}

/// 3D Spatial Transform comprising position, orientation quaternion, and scale.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transform3D {
    pub position: (f32, f32, f32),
    /// Quaternion rotation `(x, y, z, w)`. Default is identity `(0.0, 0.0, 0.0, 1.0)`.
    pub rotation: (f32, f32, f32, f32),
    pub scale: (f32, f32, f32),
}

impl Transform3D {
    /// Creates a transform with identity orientation and uniform 1.0 scale.
    pub const fn from_position(position: (f32, f32, f32)) -> Self {
        Self {
            position,
            rotation: (0.0, 0.0, 0.0, 1.0),
            scale: (1.0, 1.0, 1.0),
        }
    }

    /// Creates a fully specified 3D transform.
    pub const fn new(
        position: (f32, f32, f32),
        rotation: (f32, f32, f32, f32),
        scale: (f32, f32, f32),
    ) -> Self {
        Self {
            position,
            rotation,
            scale,
        }
    }
}

impl Default for Transform3D {
    fn default() -> Self {
        Self {
            position: (0.0, 0.0, 0.0),
            rotation: (0.0, 0.0, 0.0, 1.0),
            scale: (1.0, 1.0, 1.0),
        }
    }
}

/// Health and durability state of a world object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectHealth {
    pub current: u32,
    pub max: u32,
}

impl ObjectHealth {
    pub const fn new(current: u32, max: u32) -> Self {
        Self { current, max }
    }
}

/// Standardized bitflags for world object status.
pub mod world_object_flags {
    pub const ACTIVE: u32 = 1 << 0;
    pub const INTERACTIVE: u32 = 1 << 1;
    pub const LOCKED: u32 = 1 << 2;
    pub const OPEN: u32 = 1 << 3;
    pub const DAMAGED: u32 = 1 << 4;
    pub const DESTROYED: u32 = 1 << 5;
    pub const INVULNERABLE: u32 = 1 << 6;
    pub const HIDDEN: u32 = 1 << 7;
}

fn default_phase_mask() -> u64 {
    1
}

/// Generalized persistent world object or constructible prefab instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldObject {
    pub object_id: Guid,
    pub prefab_id: u32,
    pub transform: Transform3D,
    pub bounds: BoundingBox3D,
    pub state_flags: u32,
    #[serde(default = "default_phase_mask")]
    pub phase_mask: u64,
    pub owner: ContainerOwner,
    pub health: Option<ObjectHealth>,
    pub properties: HashMap<String, String>,
    pub dirty_revision: u64,
}

impl WorldObject {
    /// Creates a new `WorldObject` with default active flags, default phase mask (1), and revision 1.
    pub fn new(
        object_id: Guid,
        prefab_id: u32,
        transform: Transform3D,
        bounds: BoundingBox3D,
        owner: ContainerOwner,
    ) -> Self {
        Self {
            object_id,
            prefab_id,
            transform,
            bounds,
            state_flags: world_object_flags::ACTIVE | world_object_flags::INTERACTIVE,
            phase_mask: 1,
            owner,
            health: None,
            properties: HashMap::new(),
            dirty_revision: 1,
        }
    }

    /// Returns the sector coordinate containing this object's transform origin.
    #[inline]
    pub fn sector(&self) -> SectorCoord3D {
        SectorCoord3D::from_world_pos(self.transform.position)
    }

    /// Returns `true` if the object has the specified state flag set.
    #[inline]
    pub fn has_flag(&self, flag: u32) -> bool {
        (self.state_flags & flag) != 0
    }

    /// Sets or clears the specified state flag.
    #[inline]
    pub fn set_flag(&mut self, flag: u32, enabled: bool) {
        if enabled {
            self.state_flags |= flag;
        } else {
            self.state_flags &= !flag;
        }
        self.dirty_revision += 1;
    }
}

/// In-memory spatial index partitioning world objects into uniform sectors.
#[derive(Debug, Default, Clone)]
pub struct SectorSpatialIndex {
    sectors: HashMap<SectorCoord3D, HashMap<Guid, WorldObject>>,
    object_to_sector: HashMap<Guid, SectorCoord3D>,
}

impl SectorSpatialIndex {
    /// Constructs an empty `SectorSpatialIndex`.
    pub fn new() -> Self {
        Self {
            sectors: HashMap::new(),
            object_to_sector: HashMap::new(),
        }
    }

    /// Total number of indexed world objects.
    pub fn len(&self) -> usize {
        self.object_to_sector.len()
    }

    /// Returns `true` if no world objects are indexed.
    pub fn is_empty(&self) -> bool {
        self.object_to_sector.is_empty()
    }

    /// Inserts or updates a world object in the spatial index.
    ///
    /// Automatically handles movement across sector boundaries.
    /// Returns the previously stored object with the same GUID, if any.
    pub fn insert(&mut self, object: WorldObject) -> Option<WorldObject> {
        let object_id = object.object_id;
        let new_sector = object.sector();

        let prev = if let Some(&old_sector) = self.object_to_sector.get(&object_id) {
            if old_sector != new_sector {
                if let Some(sector_map) = self.sectors.get_mut(&old_sector) {
                    let removed = sector_map.remove(&object_id);
                    if sector_map.is_empty() {
                        self.sectors.remove(&old_sector);
                    }
                    removed
                } else {
                    None
                }
            } else {
                self.sectors
                    .get_mut(&old_sector)
                    .and_then(|m| m.remove(&object_id))
            }
        } else {
            None
        };

        self.object_to_sector.insert(object_id, new_sector);
        self.sectors
            .entry(new_sector)
            .or_default()
            .insert(object_id, object);

        prev
    }

    /// Removes a world object by its GUID, returning the removed instance.
    pub fn remove(&mut self, object_id: Guid) -> Option<WorldObject> {
        let sector = self.object_to_sector.remove(&object_id)?;
        if let Some(sector_map) = self.sectors.get_mut(&sector) {
            let removed = sector_map.remove(&object_id);
            if sector_map.is_empty() {
                self.sectors.remove(&sector);
            }
            removed
        } else {
            None
        }
    }

    /// Retrieves an immutable reference to a world object by its GUID.
    pub fn get(&self, object_id: Guid) -> Option<&WorldObject> {
        let sector = self.object_to_sector.get(&object_id)?;
        self.sectors.get(sector).and_then(|m| m.get(&object_id))
    }

    /// Retrieves a mutable reference to a world object by its GUID.
    ///
    /// Note: if modifying `transform.position`, re-insertion via `insert`
    /// should be invoked to update the containing sector index if the sector changed.
    pub fn get_mut(&mut self, object_id: Guid) -> Option<&mut WorldObject> {
        let sector = self.object_to_sector.get(&object_id)?;
        self.sectors
            .get_mut(sector)
            .and_then(|m| m.get_mut(&object_id))
    }

    /// Returns a reference to all objects residing in the specified sector.
    pub fn objects_in_sector(&self, sector: SectorCoord3D) -> Option<&HashMap<Guid, WorldObject>> {
        self.sectors.get(&sector)
    }

    /// Gathers all world objects within an Axis-Aligned Bounding Box.
    pub fn query_aabb(&self, aabb: &BoundingBox3D) -> Vec<&WorldObject> {
        let min_sector = SectorCoord3D::from_world_pos(aabb.min);
        let max_sector = SectorCoord3D::from_world_pos(aabb.max);

        let mut results = Vec::new();
        for sx in min_sector.x..=max_sector.x {
            for sy in min_sector.y..=max_sector.y {
                for sz in min_sector.z..=max_sector.z {
                    let sector_coord = SectorCoord3D::new(sx, sy, sz);
                    if let Some(sector_map) = self.sectors.get(&sector_coord) {
                        for object in sector_map.values() {
                            if object.bounds.intersects(aabb) {
                                results.push(object);
                            }
                        }
                    }
                }
            }
        }
        results
    }

    /// Gathers all world objects within a spherical Area of Interest.
    pub fn query_sphere(&self, center: (f32, f32, f32), radius: f32) -> Vec<&WorldObject> {
        if radius < 0.0 {
            return Vec::new();
        }
        let aabb = BoundingBox3D::new(
            (center.0 - radius, center.1 - radius, center.2 - radius),
            (center.0 + radius, center.1 + radius, center.2 + radius),
        );

        let candidates = self.query_aabb(&aabb);
        candidates
            .into_iter()
            .filter(|obj| obj.bounds.intersects_sphere(center, radius))
            .collect()
    }

    /// Performs an authoritative raycast through the spatial index.
    /// Returns the closest intersecting `WorldObject` and hit distance `t`, if any.
    pub fn raycast(
        &self,
        origin: (f32, f32, f32),
        dir: (f32, f32, f32),
        max_dist: f32,
    ) -> Option<(&WorldObject, f32)> {
        if max_dist <= 0.0 {
            return None;
        }

        let target = (
            origin.0 + dir.0 * max_dist,
            origin.1 + dir.1 * max_dist,
            origin.2 + dir.2 * max_dist,
        );
        let ray_box = BoundingBox3D::new(
            (
                origin.0.min(target.0),
                origin.1.min(target.1),
                origin.2.min(target.2),
            ),
            (
                origin.0.max(target.0),
                origin.1.max(target.1),
                origin.2.max(target.2),
            ),
        );

        let candidates = self.query_aabb(&ray_box);
        let mut closest_hit: Option<(&WorldObject, f32)> = None;

        for obj in candidates {
            if let Some(t) = obj.bounds.intersects_ray(origin, dir, max_dist) {
                if let Some((_, closest_t)) = closest_hit {
                    if t < closest_t {
                        closest_hit = Some((obj, t));
                    }
                } else {
                    closest_hit = Some((obj, t));
                }
            }
        }

        closest_hit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guid::EntityType;

    #[test]
    fn test_sector_coord_calculation() {
        let origin = SectorCoord3D::from_world_pos((0.0, 0.0, 0.0));
        assert_eq!(origin, SectorCoord3D::new(0, 0, 0));

        let pos1 = SectorCoord3D::from_world_pos((63.99, 127.5, 0.0));
        assert_eq!(pos1, SectorCoord3D::new(0, 1, 0));

        let neg_pos = SectorCoord3D::from_world_pos((-0.1, -64.0, -100.0));
        assert_eq!(neg_pos, SectorCoord3D::new(-1, -1, -2));
    }

    #[test]
    fn test_sector_bounds() {
        let sector = SectorCoord3D::new(1, 2, -1);
        let bounds = sector.bounds();
        assert_eq!(bounds.min, (64.0, 128.0, -64.0));
        assert_eq!(bounds.max, (128.0, 192.0, 0.0));
    }

    #[test]
    fn test_sector_neighborhood() {
        let sector = SectorCoord3D::new(5, 5, 5);
        let neighbors = sector.neighborhood();
        assert_eq!(neighbors.len(), 27);
        assert!(neighbors.contains(&SectorCoord3D::new(5, 5, 5)));
        assert!(neighbors.contains(&SectorCoord3D::new(4, 5, 6)));
        assert!(neighbors.contains(&SectorCoord3D::new(6, 6, 6)));
    }

    #[test]
    fn test_world_object_crud_and_cross_sector_migration() {
        let mut index = SectorSpatialIndex::new();
        assert!(index.is_empty());

        let obj_id = Guid::new(EntityType::DynamicObject, 1, 100);
        let obj1 = WorldObject::new(
            obj_id,
            10,
            Transform3D::from_position((10.0, 20.0, 30.0)),
            BoundingBox3D::new((9.0, 19.0, 29.0), (11.0, 21.0, 31.0)),
            ContainerOwner::System("system".into()),
        );

        index.insert(obj1);
        assert_eq!(index.len(), 1);
        assert!(index.get(obj_id).is_some());

        let sector_0_0_0 = SectorCoord3D::new(0, 0, 0);
        let objs = index.objects_in_sector(sector_0_0_0).unwrap();
        assert_eq!(objs.len(), 1);

        // Move to sector (1, 0, 0)
        let mut obj1_moved = index.get(obj_id).unwrap().clone();
        obj1_moved.transform.position = (70.0, 20.0, 30.0);
        obj1_moved.bounds = BoundingBox3D::new((69.0, 19.0, 29.0), (71.0, 21.0, 31.0));
        index.insert(obj1_moved);

        assert_eq!(index.len(), 1);
        assert!(index.objects_in_sector(sector_0_0_0).is_none());
        assert_eq!(
            index
                .objects_in_sector(SectorCoord3D::new(1, 0, 0))
                .unwrap()
                .len(),
            1
        );

        // Remove
        let removed = index.remove(obj_id);
        assert!(removed.is_some());
        assert_eq!(index.len(), 0);
        assert!(index.is_empty());
    }

    #[test]
    fn test_spatial_queries_aabb_and_sphere() {
        let mut index = SectorSpatialIndex::new();

        let id1 = Guid::new(EntityType::DynamicObject, 1, 1);
        let id2 = Guid::new(EntityType::DynamicObject, 1, 2);

        let obj1 = WorldObject::new(
            id1,
            1,
            Transform3D::from_position((10.0, 10.0, 10.0)),
            BoundingBox3D::new((9.0, 9.0, 9.0), (11.0, 11.0, 11.0)),
            ContainerOwner::Character(1),
        );
        let obj2 = WorldObject::new(
            id2,
            1,
            Transform3D::from_position((100.0, 100.0, 100.0)),
            BoundingBox3D::new((99.0, 99.0, 99.0), (101.0, 101.0, 101.0)),
            ContainerOwner::Character(2),
        );

        index.insert(obj1);
        index.insert(obj2);

        // Query sphere centered at (10, 10, 10) with radius 5m
        let hits = index.query_sphere((10.0, 10.0, 10.0), 5.0);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].object_id, id1);

        // Query sphere including both
        let hits_both = index.query_sphere((50.0, 50.0, 50.0), 100.0);
        assert_eq!(hits_both.len(), 2);

        // Query negative radius
        assert!(index.query_sphere((10.0, 10.0, 10.0), -1.0).is_empty());
    }

    #[test]
    fn test_world_object_flags() {
        let mut obj = WorldObject::new(
            Guid::new(EntityType::DynamicObject, 1, 1),
            1,
            Transform3D::default(),
            BoundingBox3D::new((-1.0, -1.0, -1.0), (1.0, 1.0, 1.0)),
            ContainerOwner::System("system".into()),
        );

        assert!(obj.has_flag(world_object_flags::ACTIVE));
        assert!(obj.has_flag(world_object_flags::INTERACTIVE));
        assert!(!obj.has_flag(world_object_flags::LOCKED));

        let rev_before = obj.dirty_revision;
        obj.set_flag(world_object_flags::LOCKED, true);
        assert!(obj.has_flag(world_object_flags::LOCKED));
        assert_eq!(obj.dirty_revision, rev_before + 1);

        obj.set_flag(world_object_flags::LOCKED, false);
        assert!(!obj.has_flag(world_object_flags::LOCKED));
    }
}
