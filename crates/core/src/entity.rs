//! Entity and EntityStore module for entity management and dynamic component storage.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;

use slotmap::{new_key_type, SlotMap};

use crate::guid::Guid;

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
        self.components.insert(TypeId::of::<T>(), Box::new(component));
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

/// Representation of an in-world simulation entity.
#[derive(Debug)]
pub struct Entity {
    pub guid: Guid,
    pub position: (f32, f32, f32),
    pub rotation: f32,
    pub components: ComponentStore,
}

impl Entity {
    /// Creates a new entity with the given GUID and position, default rotation (0.0), and an empty component store.
    pub fn new(guid: Guid, position: (f32, f32, f32)) -> Self {
        Self {
            guid,
            position,
            rotation: 0.0,
            components: ComponentStore::new(),
        }
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
}

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
        Some(entity)
    }

    /// Despawns an entity by its `Guid`, removing it from storage and indexing.
    pub fn despawn_by_guid(&mut self, guid: Guid) -> Option<Entity> {
        let key = self.guid_to_key.remove(&guid)?;
        self.entities.remove(key)
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

        let health = entity.get_component::<Health>().expect("health component must exist");
        assert_eq!(
            health,
            &Health {
                current: 100,
                max: 100,
            }
        );

        let name = entity.get_component::<PlayerName>().expect("name component must exist");
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

            let health = ent_by_guid.get_component_mut::<Health>().expect("health must exist");
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
        entity2.rotation = 3.14;
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
        store.add_component(Health { current: 80, max: 100 });
        assert_eq!(store.iter().count(), 1);

        let vel = Velocity { x: 3.5, y: 7.0, z: 0.0 };
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
}
