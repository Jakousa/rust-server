//! Entity and EntityStore module for entity management and dynamic component storage.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;

use slotmap::{new_key_type, SlotMap};

use crate::guid::Guid;
use crate::property::{DynamicProperties, PropertyValue};

new_key_type! {
    /// SlotMap generational key for uniquely addressing entities in the entity store.
    pub struct EntityKey;
}

/// A type-safe component store supporting dynamic component attachment via `TypeId` mapping.
#[derive(Default)]
pub struct ComponentStore {
    components: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl ComponentStore {
    /// Creates a new empty `ComponentStore`.
    pub fn new() -> Self {
        Self {
            components: HashMap::new(),
        }
    }

    /// Attaches a component to the store. Replaces any existing component of the same type.
    pub fn add_component<T: 'static + Send + Sync>(&mut self, component: T) {
        self.components
            .insert(TypeId::of::<T>(), Box::new(component));
    }

    /// Retrieves an immutable reference to the component of type `T`, if present.
    pub fn get_component<T: 'static>(&self) -> Option<&T> {
        self.components
            .get(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_ref::<T>())
    }

    /// Retrieves a mutable reference to the component of type `T`, if present.
    pub fn get_component_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.components
            .get_mut(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_mut::<T>())
    }

    /// Removes and returns the component of type `T`, if present.
    pub fn remove_component<T: 'static>(&mut self) -> Option<Box<T>> {
        self.components
            .remove(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast::<T>().ok())
    }

    /// Returns `true` if a component of type `T` exists in the store.
    pub fn has_component<T: 'static>(&self) -> bool {
        self.components.contains_key(&TypeId::of::<T>())
    }

    /// Returns the number of components attached to this store.
    pub fn len(&self) -> usize {
        self.components.len()
    }

    /// Returns `true` if no components are attached.
    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }

    /// Returns an iterator over all attached components as `(&TypeId, &Box<dyn Any + Send + Sync>)`.
    pub fn iter(&self) -> impl Iterator<Item = (&TypeId, &Box<dyn Any + Send + Sync>)> {
        self.components.iter()
    }

    /// Inserts a type-erased component box with the specified `TypeId`.
    pub fn insert_raw(&mut self, type_id: TypeId, component: Box<dyn Any + Send + Sync>) {
        self.components.insert(type_id, component);
    }

    /// Clears all attached components.
    pub fn clear(&mut self) {
        self.components.clear();
    }
}

impl fmt::Debug for ComponentStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComponentStore")
            .field("component_count", &self.components.len())
            .finish()
    }
}

pub mod state_flags {
    pub const INCAPACITATED: u32 = 1 << 0;
    pub const MOVEMENT_LOCKED: u32 = 1 << 1;
    pub const COMBAT_ACTIVE: u32 = 1 << 2;
    pub const INVULNERABLE: u32 = 1 << 3;
}

/// Representation of an in-world simulation entity.
#[derive(Debug)]
pub struct Entity {
    pub guid: Guid,
    pub position: (f32, f32, f32),
    pub rotation: f32,
    pub velocity: (f32, f32, f32),
    pub collider_half_extents: (f32, f32, f32),
    pub parent_guid: Option<Guid>,
    pub local_position: (f32, f32, f32),
    pub local_rotation: f32,
    pub children: Vec<Guid>,
    pub state_flags: u32,
    pub phase_mask: u64,
    pub visibility_mask: u64,
    pub stealth_level: u8,
    pub detection_level: u8,
    pub properties: DynamicProperties,
    pub components: ComponentStore,
}

impl Entity {
    /// Creates a new entity with the given GUID and position, default rotation (0.0), empty properties, and an empty component store.
    pub fn new(guid: Guid, position: (f32, f32, f32)) -> Self {
        Self {
            guid,
            position,
            rotation: 0.0,
            velocity: (0.0, 0.0, 0.0),
            collider_half_extents: (0.4, 0.9, 0.4),
            parent_guid: None,
            local_position: (0.0, 0.0, 0.0),
            local_rotation: 0.0,
            children: Vec::new(),
            state_flags: 0,
            phase_mask: 1,
            visibility_mask: u64::MAX,
            stealth_level: 0,
            detection_level: 0,
            properties: DynamicProperties::new(),
            components: ComponentStore::new(),
        }
    }

    /// Returns `true` if this entity can observe `target`, taking into account:
    /// 1. Phase alignment (`self.phase_mask & target.phase_mask != 0`),
    /// 2. Visibility group overlap (`self.visibility_mask & target.visibility_mask != 0`),
    /// 3. Stealth vs. detection rating (`target.stealth_level == 0 || self.detection_level >= target.stealth_level`).
    #[inline]
    pub fn can_observe(&self, target: &Entity) -> bool {
        (self.phase_mask & target.phase_mask) != 0
            && (self.visibility_mask & target.visibility_mask) != 0
            && (target.stealth_level == 0 || self.detection_level >= target.stealth_level)
    }

    /// Returns `true` if the entity has the `INCAPACITATED` state flag set.
    #[inline]
    pub fn is_incapacitated(&self) -> bool {
        (self.state_flags & state_flags::INCAPACITATED) != 0
    }

    /// Returns `true` if the entity has the `MOVEMENT_LOCKED` state flag set.
    #[inline]
    pub fn is_movement_locked(&self) -> bool {
        (self.state_flags & state_flags::MOVEMENT_LOCKED) != 0
    }

    /// Sets the rotation of the entity.
    pub fn with_rotation(mut self, rotation: f32) -> Self {
        self.rotation = rotation;
        self
    }

    /// Attaches a component to the entity.
    pub fn add_component<T: 'static + Send + Sync>(&mut self, component: T) {
        self.components.add_component(component);
    }

    /// Retrieves an immutable reference to the component of type `T`.
    pub fn get_component<T: 'static>(&self) -> Option<&T> {
        self.components.get_component::<T>()
    }

    /// Retrieves a mutable reference to the component of type `T`.
    pub fn get_component_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.components.get_component_mut::<T>()
    }

    /// Removes and returns the component of type `T`.
    pub fn remove_component<T: 'static>(&mut self) -> Option<Box<T>> {
        self.components.remove_component::<T>()
    }

    /// Returns `true` if the entity has a component of type `T`.
    pub fn has_component<T: 'static>(&self) -> bool {
        self.components.has_component::<T>()
    }

    /// Returns an immutable reference to the property value associated with the specified key.
    pub fn get_property(&self, key: &str) -> Option<&PropertyValue> {
        self.properties.get(key)
    }

    /// Sets a dynamic property value by key.
    pub fn set_property(&mut self, key: impl Into<String>, value: impl Into<PropertyValue>) {
        self.properties.set(key, value);
    }

    /// Returns a string property slice if present and of string type.
    pub fn get_property_str(&self, key: &str) -> Option<&str> {
        self.properties.get_str(key)
    }

    /// Returns an integer property value if present.
    pub fn get_property_i64(&self, key: &str) -> Option<i64> {
        self.properties.get_i64(key)
    }

    /// Returns a float property value (with integer auto-coercion) if present.
    pub fn get_property_f64(&self, key: &str) -> Option<f64> {
        self.properties.get_f64(key)
    }

    /// Returns a boolean property value if present.
    pub fn get_property_bool(&self, key: &str) -> Option<bool> {
        self.properties.get_bool(key)
    }
}

/// Represents an updated entity position resulting from hierarchy transform propagation:
/// `(child_guid, old_world_pos, new_world_pos)`.
pub type HierarchyTransformUpdate = (Guid, (f32, f32, f32), (f32, f32, f32));

/// Central entity container providing generational slot-map storage and O(1) GUID lookup.
#[derive(Debug, Default)]
pub struct EntityStore {
    entities: SlotMap<EntityKey, Entity>,
    guid_to_key: HashMap<Guid, EntityKey>,
}

impl EntityStore {
    /// Creates a new empty `EntityStore`.
    pub fn new() -> Self {
        Self {
            entities: SlotMap::with_key(),
            guid_to_key: HashMap::new(),
        }
    }

    /// Creates a new `EntityStore` with pre-allocated capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entities: SlotMap::with_capacity_and_key(capacity),
            guid_to_key: HashMap::with_capacity(capacity),
        }
    }

    /// Spawns a new entity into the store with the given GUID and position, returning its `EntityKey`.
    pub fn spawn(&mut self, guid: Guid, position: (f32, f32, f32)) -> EntityKey {
        let entity = Entity::new(guid, position);
        self.insert_entity(entity)
    }

    /// Inserts an already initialized `Entity`, returning its `EntityKey`.
    ///
    /// If an entity with the same GUID already existed in the store, the previous entity is removed.
    pub fn insert_entity(&mut self, entity: Entity) -> EntityKey {
        let guid = entity.guid;
        let key = self.entities.insert(entity);
        if let Some(old_key) = self.guid_to_key.insert(guid, key) {
            self.entities.remove(old_key);
        }
        key
    }

    /// Despawns an entity by its `EntityKey`, removing it from storage and indexing.
    pub fn despawn(&mut self, key: EntityKey) -> Option<Entity> {
        let entity = self.entities.remove(key)?;
        self.guid_to_key.remove(&entity.guid);
        if let Some(parent_guid) = entity.parent_guid {
            if let Some(parent) = self.get_mut_by_guid(parent_guid) {
                parent.children.retain(|&g| g != entity.guid);
            }
        }
        for child_guid in &entity.children {
            if let Some(child) = self.get_mut_by_guid(*child_guid) {
                child.parent_guid = None;
            }
        }
        Some(entity)
    }

    /// Despawns an entity by its `Guid`, removing it from storage and indexing.
    pub fn despawn_by_guid(&mut self, guid: Guid) -> Option<Entity> {
        let key = self.guid_to_key.remove(&guid)?;
        let entity = self.entities.remove(key)?;
        if let Some(parent_guid) = entity.parent_guid {
            if let Some(parent) = self.get_mut_by_guid(parent_guid) {
                parent.children.retain(|&g| g != guid);
            }
        }
        for child_guid in &entity.children {
            if let Some(child) = self.get_mut_by_guid(*child_guid) {
                child.parent_guid = None;
            }
        }
        Some(entity)
    }

    /// Returns an immutable reference to the entity associated with the given key.
    pub fn get(&self, key: EntityKey) -> Option<&Entity> {
        self.entities.get(key)
    }

    /// Returns a mutable reference to the entity associated with the given key.
    pub fn get_mut(&mut self, key: EntityKey) -> Option<&mut Entity> {
        self.entities.get_mut(key)
    }

    /// Returns an immutable reference to the entity associated with the given GUID.
    pub fn get_by_guid(&self, guid: Guid) -> Option<&Entity> {
        let key = self.guid_to_key.get(&guid)?;
        self.entities.get(*key)
    }

    /// Returns a mutable reference to the entity associated with the given GUID.
    pub fn get_mut_by_guid(&mut self, guid: Guid) -> Option<&mut Entity> {
        let key = self.guid_to_key.get(&guid)?;
        self.entities.get_mut(*key)
    }

    /// Safely retrieves mutable references to two distinct entities by their GUIDs.
    ///
    /// Returns `None` if the GUIDs are identical or if either entity does not exist in the store.
    pub fn get_two_mut(&mut self, g1: Guid, g2: Guid) -> Option<(&mut Entity, &mut Entity)> {
        if g1 == g2 {
            return None;
        }
        let k1 = *self.guid_to_key.get(&g1)?;
        let k2 = *self.guid_to_key.get(&g2)?;
        let [e1, e2] = self.entities.get_disjoint_mut([k1, k2])?;
        Some((e1, e2))
    }

    /// Looks up the `EntityKey` corresponding to a `Guid`.
    pub fn get_key_by_guid(&self, guid: Guid) -> Option<EntityKey> {
        self.guid_to_key.get(&guid).copied()
    }

    /// Returns `true` if the store contains an entity with the given key.
    pub fn contains_key(&self, key: EntityKey) -> bool {
        self.entities.contains_key(key)
    }

    /// Returns `true` if the store contains an entity with the given GUID.
    pub fn contains_guid(&self, guid: Guid) -> bool {
        self.guid_to_key.contains_key(&guid)
    }

    /// Returns the total number of alive entities in the store.
    pub fn count(&self) -> usize {
        self.entities.len()
    }

    /// Returns `true` if the store contains no entities.
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// Returns an iterator over `(EntityKey, &Entity)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (EntityKey, &Entity)> {
        self.entities.iter()
    }

    /// Returns a mutable iterator over `(EntityKey, &mut Entity)` pairs.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (EntityKey, &mut Entity)> {
        self.entities.iter_mut()
    }

    /// Clears all entities and indexes from the store.
    pub fn clear(&mut self) {
        self.entities.clear();
        self.guid_to_key.clear();
    }

    /// Converts local coordinates relative to a parent entity's transform to world coordinates.
    #[inline]
    pub fn local_to_world(
        parent_pos: (f32, f32, f32),
        parent_yaw: f32,
        local_pos: (f32, f32, f32),
    ) -> (f32, f32, f32) {
        local_to_world(parent_pos, parent_yaw, local_pos)
    }

    /// Converts world coordinates to local coordinates relative to a parent entity's transform.
    #[inline]
    pub fn world_to_local(
        parent_pos: (f32, f32, f32),
        parent_yaw: f32,
        world_pos: (f32, f32, f32),
    ) -> (f32, f32, f32) {
        world_to_local(parent_pos, parent_yaw, world_pos)
    }

    /// Attaches `child_guid` as a child of `parent_guid` at the specified local position and rotation.
    ///
    /// Returns `false` if `child_guid == parent_guid`, either entity does not exist, or attaching
    /// would introduce a circular hierarchy. Updates the child's world position and rotation immediately.
    pub fn attach_child(
        &mut self,
        child_guid: Guid,
        parent_guid: Guid,
        local_pos: (f32, f32, f32),
        local_rot: f32,
    ) -> bool {
        if child_guid == parent_guid {
            return false;
        }
        if !self.contains_guid(child_guid) || !self.contains_guid(parent_guid) {
            return false;
        }
        // Cycle check: verify parent is not a descendant of child
        let mut curr = Some(parent_guid);
        while let Some(ancestor_guid) = curr {
            if ancestor_guid == child_guid {
                return false;
            }
            curr = self.get_by_guid(ancestor_guid).and_then(|e| e.parent_guid);
        }

        // Detach from previous parent if any
        self.detach_child(child_guid);

        // Fetch parent transform
        let (parent_pos, parent_rot) = {
            let parent = self.get_by_guid(parent_guid).unwrap();
            (parent.position, parent.rotation)
        };

        // Add to parent.children
        if let Some(parent) = self.get_mut_by_guid(parent_guid) {
            if !parent.children.contains(&child_guid) {
                parent.children.push(child_guid);
            }
        }

        // Update child fields and world transform
        let world_pos = Self::local_to_world(parent_pos, parent_rot, local_pos);
        let world_rot = parent_rot + local_rot;
        if let Some(child) = self.get_mut_by_guid(child_guid) {
            child.parent_guid = Some(parent_guid);
            child.local_position = local_pos;
            child.local_rotation = local_rot;
            child.position = world_pos;
            child.rotation = world_rot;
        }

        // If the child itself has children, propagate their world transforms too
        self.propagate_hierarchy_transforms(child_guid);

        true
    }

    /// Detaches a child entity from its current parent, if attached.
    ///
    /// Returns `true` if detached, or `false` if the child does not exist or had no parent.
    pub fn detach_child(&mut self, child_guid: Guid) -> bool {
        let parent_guid = match self.get_by_guid(child_guid) {
            Some(c) => c.parent_guid,
            None => return false,
        };

        let Some(parent_guid) = parent_guid else {
            return false;
        };

        if let Some(parent) = self.get_mut_by_guid(parent_guid) {
            parent.children.retain(|&g| g != child_guid);
        }

        if let Some(child) = self.get_mut_by_guid(child_guid) {
            child.parent_guid = None;
        }

        true
    }

    /// Propagates transform changes from a root parent down to all transitive children in BFS order.
    ///
    /// Updates each child's world `position` and `rotation`, and returns a vector of
    /// `(child_guid, old_world_pos, new_world_pos)` for spatial index / AoI / kinematic synchronization.
    pub fn propagate_hierarchy_transforms(
        &mut self,
        root_guid: Guid,
    ) -> Vec<HierarchyTransformUpdate> {
        let mut results = Vec::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(root_guid);

        while let Some(parent_guid) = queue.pop_front() {
            let Some(parent) = self.get_by_guid(parent_guid) else {
                continue;
            };
            let parent_pos = parent.position;
            let parent_rot = parent.rotation;
            let children = parent.children.clone();

            for child_guid in children {
                let Some(child) = self.get_mut_by_guid(child_guid) else {
                    continue;
                };
                let old_pos = child.position;
                let new_pos = Self::local_to_world(parent_pos, parent_rot, child.local_position);
                let new_rot = parent_rot + child.local_rotation;

                child.position = new_pos;
                child.rotation = new_rot;
                results.push((child_guid, old_pos, new_pos));

                queue.push_back(child_guid);
            }
        }

        results
    }

    /// Collects an entity and all its transitive descendants in topological parent-first order.
    ///
    /// Returns `[root_guid, child1, child2, grandchild1, ...]`. If the entity does not exist,
    /// returns an empty vector.
    pub fn collect_hierarchy_group(&self, root_guid: Guid) -> Vec<Guid> {
        if !self.contains_guid(root_guid) {
            return Vec::new();
        }
        let mut results = Vec::new();
        let mut queue = std::collections::VecDeque::new();
        results.push(root_guid);
        queue.push_back(root_guid);

        while let Some(curr_guid) = queue.pop_front() {
            let Some(entity) = self.get_by_guid(curr_guid) else {
                continue;
            };
            for &child_guid in &entity.children {
                if self.contains_guid(child_guid) && !results.contains(&child_guid) {
                    results.push(child_guid);
                    queue.push_back(child_guid);
                }
            }
        }

        results
    }
}

/// Converts local coordinates relative to a parent entity's position and yaw rotation to world coordinates.
#[inline]
pub fn local_to_world(
    parent_pos: (f32, f32, f32),
    parent_yaw: f32,
    local_pos: (f32, f32, f32),
) -> (f32, f32, f32) {
    let (sin_y, cos_y) = parent_yaw.sin_cos();
    (
        parent_pos.0 + local_pos.0 * cos_y - local_pos.2 * sin_y,
        parent_pos.1 + local_pos.1,
        parent_pos.2 + local_pos.0 * sin_y + local_pos.2 * cos_y,
    )
}

/// Converts world coordinates to local coordinates relative to a parent entity's position and yaw rotation.
#[inline]
pub fn world_to_local(
    parent_pos: (f32, f32, f32),
    parent_yaw: f32,
    world_pos: (f32, f32, f32),
) -> (f32, f32, f32) {
    let dx = world_pos.0 - parent_pos.0;
    let dy = world_pos.1 - parent_pos.1;
    let dz = world_pos.2 - parent_pos.2;
    let (sin_y, cos_y) = (-parent_yaw).sin_cos();
    (dx * cos_y - dz * sin_y, dy, dx * sin_y + dz * cos_y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guid::EntityType;

    #[derive(Debug, PartialEq, Eq)]
    struct Health {
        current: u32,
        max: u32,
    }

    #[derive(Debug, PartialEq)]
    struct Velocity {
        x: f32,
        y: f32,
        z: f32,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct PlayerName(String);

    #[test]
    fn test_spawn_and_retrieve() {
        let mut store = EntityStore::new();
        assert_eq!(store.count(), 0);
        assert!(store.is_empty());

        let guid = Guid::new(EntityType::Player, 1, 100);
        let pos = (10.0, 20.0, 30.0);
        let key = store.spawn(guid, pos);

        assert_eq!(store.count(), 1);
        assert!(!store.is_empty());
        assert!(store.contains_key(key));
        assert!(store.contains_guid(guid));

        let entity = store.get(key).expect("entity must exist");
        assert_eq!(entity.guid, guid);
        assert_eq!(entity.position, pos);
        assert_eq!(entity.rotation, 0.0);
    }

    #[test]
    fn test_guid_indexing() {
        let mut store = EntityStore::new();
        let guid1 = Guid::new(EntityType::Player, 1, 101);
        let guid2 = Guid::new(EntityType::Npc, 1, 102);

        let key1 = store.spawn(guid1, (1.0, 2.0, 3.0));
        let key2 = store.spawn(guid2, (4.0, 5.0, 6.0));

        assert_eq!(store.get_key_by_guid(guid1), Some(key1));
        assert_eq!(store.get_key_by_guid(guid2), Some(key2));

        let ent1 = store.get_by_guid(guid1).expect("ent1 must exist");
        assert_eq!(ent1.guid, guid1);
        assert_eq!(ent1.position, (1.0, 2.0, 3.0));

        let ent2 = store.get_by_guid(guid2).expect("ent2 must exist");
        assert_eq!(ent2.guid, guid2);
        assert_eq!(ent2.position, (4.0, 5.0, 6.0));

        let non_existent = Guid::new(EntityType::DynamicObject, 99, 999);
        assert!(store.get_by_guid(non_existent).is_none());
        assert!(store.get_key_by_guid(non_existent).is_none());
    }

    #[test]
    fn test_component_add_and_retrieve() {
        let mut entity = Entity::new(Guid::new(EntityType::Player, 1, 200), (0.0, 0.0, 0.0));

        assert!(!entity.has_component::<Health>());
        assert!(entity.get_component::<Health>().is_none());

        entity.add_component(Health {
            current: 100,
            max: 100,
        });
        entity.add_component(PlayerName("Hero".to_string()));

        assert!(entity.has_component::<Health>());
        assert!(entity.has_component::<PlayerName>());
        assert!(!entity.has_component::<Velocity>());

        let health = entity
            .get_component::<Health>()
            .expect("health component must exist");
        assert_eq!(
            health,
            &Health {
                current: 100,
                max: 100,
            }
        );

        let name = entity
            .get_component::<PlayerName>()
            .expect("name component must exist");
        assert_eq!(name.0, "Hero");
    }

    #[test]
    fn test_component_mutation() {
        let mut store = EntityStore::new();
        let guid = Guid::new(EntityType::Player, 1, 300);
        let key = store.spawn(guid, (0.0, 0.0, 0.0));

        {
            let ent = store.get_mut(key).expect("entity must exist");
            ent.position = (100.0, 200.0, 300.0);
            ent.rotation = 1.57;
            ent.add_component(Health {
                current: 80,
                max: 100,
            });
        }

        {
            let ent_by_guid = store.get_mut_by_guid(guid).expect("entity must exist");
            assert_eq!(ent_by_guid.position, (100.0, 200.0, 300.0));
            assert_eq!(ent_by_guid.rotation, 1.57);

            let health = ent_by_guid
                .get_component_mut::<Health>()
                .expect("health must exist");
            health.current -= 30;
        }

        let ent = store.get(key).expect("entity must exist");
        let health = ent.get_component::<Health>().expect("health must exist");
        assert_eq!(health.current, 50);
    }

    #[test]
    fn test_component_removal() {
        let mut entity = Entity::new(Guid::new(EntityType::Player, 1, 400), (0.0, 0.0, 0.0));
        entity.add_component(Velocity {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        });

        assert!(entity.has_component::<Velocity>());
        let removed = entity.remove_component::<Velocity>();
        assert_eq!(
            removed,
            Some(Box::new(Velocity {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            }))
        );

        assert!(!entity.has_component::<Velocity>());
        assert!(entity.get_component::<Velocity>().is_none());
        assert!(entity.remove_component::<Velocity>().is_none());
    }

    #[test]
    fn test_despawn_by_key() {
        let mut store = EntityStore::new();
        let guid = Guid::new(EntityType::Player, 1, 500);
        let key = store.spawn(guid, (5.0, 10.0, 15.0));

        assert_eq!(store.count(), 1);
        let despawned = store.despawn(key);
        assert!(despawned.is_some());
        let ent = despawned.unwrap();
        assert_eq!(ent.guid, guid);
        assert_eq!(ent.position, (5.0, 10.0, 15.0));

        assert_eq!(store.count(), 0);
        assert!(store.get(key).is_none());
        assert!(store.get_by_guid(guid).is_none());
        assert!(store.get_key_by_guid(guid).is_none());
        assert!(!store.contains_key(key));
        assert!(!store.contains_guid(guid));

        // Despawn already removed key
        assert!(store.despawn(key).is_none());
    }

    #[test]
    fn test_despawn_by_guid() {
        let mut store = EntityStore::new();
        let guid = Guid::new(EntityType::Npc, 2, 600);
        let key = store.spawn(guid, (1.0, 1.0, 1.0));

        assert_eq!(store.count(), 1);
        let despawned = store.despawn_by_guid(guid);
        assert!(despawned.is_some());
        assert_eq!(despawned.unwrap().guid, guid);

        assert_eq!(store.count(), 0);
        assert!(store.get(key).is_none());
        assert!(store.get_by_guid(guid).is_none());
        assert!(store.despawn_by_guid(guid).is_none());
    }

    #[test]
    fn test_iteration_and_mutation() {
        let mut store = EntityStore::new();
        for i in 0..10 {
            let guid = Guid::new(EntityType::Npc, 1, i);
            store.spawn(guid, (i as f32, 0.0, 0.0));
        }

        assert_eq!(store.count(), 10);
        let count = store.iter().count();
        assert_eq!(count, 10);

        for (_key, entity) in store.iter_mut() {
            entity.position.1 = 42.0;
        }

        for (_key, entity) in store.iter() {
            assert_eq!(entity.position.1, 42.0);
        }
    }

    #[test]
    fn test_insert_entity_guid_collision_replaces() {
        let mut store = EntityStore::new();
        let guid = Guid::new(EntityType::Player, 1, 700);

        let key1 = store.spawn(guid, (1.0, 1.0, 1.0));
        assert_eq!(store.count(), 1);

        // Insert new entity with same GUID
        let mut entity2 = Entity::new(guid, (2.0, 2.0, 2.0));
        entity2.rotation = std::f32::consts::PI;
        let key2 = store.insert_entity(entity2);

        assert_eq!(store.count(), 1);
        assert!(store.get(key1).is_none());
        assert!(store.get(key2).is_some());
        assert_eq!(store.get_by_guid(guid).unwrap().position, (2.0, 2.0, 2.0));
        assert_eq!(store.get_key_by_guid(guid), Some(key2));
    }

    #[test]
    fn test_component_store_iter_and_insert_raw() {
        let mut store = ComponentStore::new();
        store.add_component(Health {
            current: 80,
            max: 100,
        });
        assert_eq!(store.iter().count(), 1);

        let vel = Velocity {
            x: 3.5,
            y: 7.0,
            z: 0.0,
        };
        store.insert_raw(TypeId::of::<Velocity>(), Box::new(vel));
        assert_eq!(store.len(), 2);
        assert_eq!(store.iter().count(), 2);

        let retrieved_vel = store.get_component::<Velocity>();
        assert!(retrieved_vel.is_some());
        assert_eq!(retrieved_vel.unwrap().x, 3.5);
    }

    #[test]
    fn test_entity_store_get_two_mut() {
        let mut store = EntityStore::new();
        let g1 = Guid::new(EntityType::Player, 1, 101);
        let g2 = Guid::new(EntityType::Player, 1, 102);

        store.spawn(g1, (0.0, 0.0, 0.0));
        store.spawn(g2, (10.0, 0.0, 0.0));

        // Simultaneous disjoint mutable borrow
        let (e1, e2) = store.get_two_mut(g1, g2).expect("Both entities exist");
        e1.position.0 = 5.0;
        e2.position.0 = 15.0;

        assert_eq!(store.get_by_guid(g1).unwrap().position.0, 5.0);
        assert_eq!(store.get_by_guid(g2).unwrap().position.0, 15.0);

        // Same GUID should return None
        assert!(store.get_two_mut(g1, g1).is_none());

        // Non-existent GUID should return None
        let g_missing = Guid::new(EntityType::Player, 1, 999);
        assert!(store.get_two_mut(g1, g_missing).is_none());
    }

    #[test]
    fn test_entity_dynamic_properties() {
        let mut entity = Entity::new(Guid::new(EntityType::Npc, 1, 800), (1.0, 2.0, 3.0));
        assert!(entity.properties.is_empty());

        entity.set_property("rank", "elite");
        entity.set_property("power", 75);
        entity.set_property("scale", 1.5);
        entity.set_property("is_boss", true);

        assert_eq!(entity.get_property_str("rank"), Some("elite"));
        assert_eq!(entity.get_property_i64("power"), Some(75));
        assert_eq!(entity.get_property_f64("scale"), Some(1.5));
        assert_eq!(entity.get_property_bool("is_boss"), Some(true));
        assert_eq!(entity.get_property("missing"), None);
    }

    #[test]
    fn test_entity_can_observe_phasing_visibility_and_stealth() {
        let mut obs = Entity::new(Guid::new(EntityType::Player, 1, 1), (0.0, 0.0, 0.0));
        let mut tgt = Entity::new(Guid::new(EntityType::Player, 1, 2), (0.0, 0.0, 0.0));

        // 1. Default settings: both in phase 1, full visibility mask, 0 stealth/detection -> can observe
        assert!(obs.can_observe(&tgt));

        // 2. Disjoint phase masks -> cannot observe
        obs.phase_mask = 0b01;
        tgt.phase_mask = 0b10;
        assert!(!obs.can_observe(&tgt));

        // Overlapping phase masks -> can observe
        tgt.phase_mask = 0b11;
        assert!(obs.can_observe(&tgt));

        // Reset phase masks
        obs.phase_mask = 1;
        tgt.phase_mask = 1;

        // 3. Disjoint visibility masks -> cannot observe
        obs.visibility_mask = 0b0010;
        tgt.visibility_mask = 0b0100;
        assert!(!obs.can_observe(&tgt));

        // Overlapping visibility masks -> can observe
        tgt.visibility_mask = 0b0110;
        assert!(obs.can_observe(&tgt));

        // Reset visibility masks
        obs.visibility_mask = u64::MAX;
        tgt.visibility_mask = u64::MAX;

        // 4. Stealth vs Detection
        tgt.stealth_level = 5;
        obs.detection_level = 0;
        assert!(!obs.can_observe(&tgt));

        obs.detection_level = 4;
        assert!(!obs.can_observe(&tgt));

        obs.detection_level = 5;
        assert!(obs.can_observe(&tgt));

        obs.detection_level = 10;
        assert!(obs.can_observe(&tgt));
    }

    #[test]
    fn test_entity_local_to_world_roundtrip() {
        let parent_pos = (120.0, 15.0, -85.0);
        let parent_yaw = 1.25; // arbitrary angle
        let local_pos = (3.5, -1.0, 7.2);

        let world_pos = local_to_world(parent_pos, parent_yaw, local_pos);
        let roundtrip_local = world_to_local(parent_pos, parent_yaw, world_pos);

        assert!((local_pos.0 - roundtrip_local.0).abs() < 1e-4);
        assert!((local_pos.1 - roundtrip_local.1).abs() < 1e-4);
        assert!((local_pos.2 - roundtrip_local.2).abs() < 1e-4);
    }

    #[test]
    fn test_entity_hierarchy_attachment_and_transform_propagation() {
        let mut store = EntityStore::new();

        let ship_guid = Guid::new(EntityType::Player, 1, 100);
        let cannon_guid = Guid::new(EntityType::Player, 1, 101);
        let mast_guid = Guid::new(EntityType::Player, 1, 102);
        let crow_nest_guid = Guid::new(EntityType::Player, 1, 103);

        store.spawn(ship_guid, (100.0, 0.0, 100.0));
        store.spawn(cannon_guid, (0.0, 0.0, 0.0));
        store.spawn(mast_guid, (0.0, 0.0, 0.0));
        store.spawn(crow_nest_guid, (0.0, 0.0, 0.0));

        // Attach cannon and mast to ship
        assert!(store.attach_child(cannon_guid, ship_guid, (5.0, 1.0, 2.0), 0.5));
        assert!(store.attach_child(mast_guid, ship_guid, (0.0, 2.0, 0.0), 0.0));

        // Attach crow's nest to mast (grandchild of ship)
        assert!(store.attach_child(crow_nest_guid, mast_guid, (0.0, 10.0, 0.0), 0.0));

        // Verify initial world positions
        let cannon = store.get_by_guid(cannon_guid).unwrap();
        assert_eq!(cannon.position, (105.0, 1.0, 102.0));
        assert_eq!(cannon.rotation, 0.5);
        assert_eq!(cannon.parent_guid, Some(ship_guid));

        let mast = store.get_by_guid(mast_guid).unwrap();
        assert_eq!(mast.position, (100.0, 2.0, 100.0));

        let crow = store.get_by_guid(crow_nest_guid).unwrap();
        assert_eq!(crow.position, (100.0, 12.0, 100.0));
        assert_eq!(crow.parent_guid, Some(mast_guid));

        // Move and rotate ship: 90 degrees (+PI/2) yaw, pos at (200.0, 10.0, 300.0)
        let new_ship_pos = (200.0, 10.0, 300.0);
        let new_ship_rot = std::f32::consts::FRAC_PI_2;
        {
            let ship = store.get_mut_by_guid(ship_guid).unwrap();
            ship.position = new_ship_pos;
            ship.rotation = new_ship_rot;
        }

        // Propagate hierarchy transforms
        let updates = store.propagate_hierarchy_transforms(ship_guid);
        assert_eq!(updates.len(), 3); // cannon, mast, crow_nest

        // Check cannon world pos after ship 90 deg rotation
        // local (5.0, 1.0, 2.0) rotated 90 deg -> x: 200 + (5*0 - 2*1) = 198, y: 11, z: 300 + (5*1 + 2*0) = 305
        let cannon = store.get_by_guid(cannon_guid).unwrap();
        assert!((cannon.position.0 - 198.0).abs() < 1e-4);
        assert!((cannon.position.1 - 11.0).abs() < 1e-4);
        assert!((cannon.position.2 - 305.0).abs() < 1e-4);
        assert!((cannon.rotation - (new_ship_rot + 0.5)).abs() < 1e-4);

        // Check crow's nest world pos
        let crow = store.get_by_guid(crow_nest_guid).unwrap();
        assert!((crow.position.0 - 200.0).abs() < 1e-4);
        assert!((crow.position.1 - 22.0).abs() < 1e-4); // 10 (ship) + 2 (mast) + 10 (crow) = 22
        assert!((crow.position.2 - 300.0).abs() < 1e-4);
    }

    #[test]
    fn test_entity_hierarchy_cycle_prevention_and_self_attachment() {
        let mut store = EntityStore::new();

        let a = Guid::new(EntityType::Player, 1, 1);
        let b = Guid::new(EntityType::Player, 1, 2);
        let c = Guid::new(EntityType::Player, 1, 3);

        store.spawn(a, (0.0, 0.0, 0.0));
        store.spawn(b, (0.0, 0.0, 0.0));
        store.spawn(c, (0.0, 0.0, 0.0));

        // Self attachment
        assert!(!store.attach_child(a, a, (0.0, 0.0, 0.0), 0.0));

        // A -> B -> C
        assert!(store.attach_child(b, a, (0.0, 0.0, 0.0), 0.0));
        assert!(store.attach_child(c, b, (0.0, 0.0, 0.0), 0.0));

        // Attempting to attach A to C (cycle: A -> B -> C -> A) must fail
        assert!(!store.attach_child(a, c, (0.0, 0.0, 0.0), 0.0));
        // Attempting to attach A to B (cycle: A -> B -> A) must fail
        assert!(!store.attach_child(a, b, (0.0, 0.0, 0.0), 0.0));
    }

    #[test]
    fn test_entity_hierarchy_group_collection_and_detachment() {
        let mut store = EntityStore::new();

        let a = Guid::new(EntityType::Player, 1, 10);
        let b = Guid::new(EntityType::Player, 1, 20);
        let c = Guid::new(EntityType::Player, 1, 30);
        let d = Guid::new(EntityType::Player, 1, 40);

        store.spawn(a, (0.0, 0.0, 0.0));
        store.spawn(b, (0.0, 0.0, 0.0));
        store.spawn(c, (0.0, 0.0, 0.0));
        store.spawn(d, (0.0, 0.0, 0.0));

        // A has children B and C; B has child D
        assert!(store.attach_child(b, a, (1.0, 0.0, 0.0), 0.0));
        assert!(store.attach_child(c, a, (2.0, 0.0, 0.0), 0.0));
        assert!(store.attach_child(d, b, (0.0, 1.0, 0.0), 0.0));

        let group_a = store.collect_hierarchy_group(a);
        assert_eq!(group_a.len(), 4);
        assert_eq!(group_a[0], a);
        assert!(group_a.contains(&b));
        assert!(group_a.contains(&c));
        assert!(group_a.contains(&d));

        let group_b = store.collect_hierarchy_group(b);
        assert_eq!(group_b, vec![b, d]);

        // Detach B from A
        assert!(store.detach_child(b));
        assert_eq!(store.get_by_guid(b).unwrap().parent_guid, None);
        assert!(!store.get_by_guid(a).unwrap().children.contains(&b));

        let group_a_after = store.collect_hierarchy_group(a);
        assert_eq!(group_a_after, vec![a, c]);

        // Despawning an entity cleans up parent and children links
        store.despawn_by_guid(b);
        assert_eq!(store.get_by_guid(d).unwrap().parent_guid, None);
    }
}
