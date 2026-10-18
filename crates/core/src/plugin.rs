//! Modular Gameplay Plugin Architecture.
//!
//! Provides decoupled abstractions for gameplay plugins:
//! - [`GameplayPlugin`] & [`PluginContext`]: Lifecycle and registration interfaces.
//! - [`ComponentCodecRegistry`]: Dynamic JSON serialization and deserialization for attached entity components.
//! - [`EventQueue`]: Deferred type-erased event queue drained at tick boundaries to prevent borrow checker conflicts and re-entrancy.
//! - [`TickSystem`]: Batch per-tick update systems iterating over entities without dynamic dispatch overhead.
//! - [`ModularPacketRouter`]: Opcode-based network packet routing to plugin handlers.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

use serde::{de::DeserializeOwned, Serialize};

use crate::entity::{ComponentStore, EntityStore};
use crate::guid::Guid;
use crate::hook::HookDispatcher;

/// Errors that can occur during dynamic component serialization or deserialization.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("Downcast failed for component '{0}'")]
    DowncastFailed(&'static str),

    #[error("Serialization failed for component '{type_name}': {source}")]
    Serialization {
        type_name: &'static str,
        #[source]
        source: serde_json::Error,
    },

    #[error("Deserialization failed for component '{type_name}': {source}")]
    Deserialization {
        type_name: &'static str,
        #[source]
        source: serde_json::Error,
    },

    #[error("Binary serialization failed: {0}")]
    BinarySerialization(#[from] rmp_serde::encode::Error),

    #[error("Binary deserialization failed: {0}")]
    BinaryDeserialization(#[from] rmp_serde::decode::Error),

    #[error("Binary payload exceeds maximum allowed size ({size} > {max})")]
    PayloadTooLarge { size: usize, max: usize },
}

/// Errors that can occur during modular client packet dispatch.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PacketHandlerError {
    #[error("Target entity {0} not found")]
    EntityNotFound(Guid),

    #[error("No handler registered for module_id {0}")]
    ModuleNotFound(u16),

    #[error("Malformed packet payload for module {module_id}: {message}")]
    MalformedPayload { module_id: u16, message: String },

    #[error("Handler error: {0}")]
    Custom(String),
}

/// Trait implemented by components that can be serialized/deserialized dynamically.
pub trait ComponentCodec: Send + Sync {
    /// Friendly type name (e.g. "inventory", "health") used as JSON key.
    fn type_name(&self) -> &'static str;
    /// The runtime `TypeId` of the concrete component.
    fn component_type_id(&self) -> TypeId;
    /// Serializes a boxed component into a JSON Value.
    fn serialize(&self, boxed: &(dyn Any + Send + Sync)) -> Result<serde_json::Value, CodecError>;
    /// Deserializes a JSON Value into a type-erased component box.
    fn deserialize(
        &self,
        value: serde_json::Value,
    ) -> Result<Box<dyn Any + Send + Sync>, CodecError>;
}

/// Generic implementation of [`ComponentCodec`] for any serializable component.
pub struct TypedComponentCodec<T> {
    name: &'static str,
    _marker: std::marker::PhantomData<T>,
}

impl<T> TypedComponentCodec<T> {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            _marker: std::marker::PhantomData,
        }
    }
}

impl<T: 'static + Send + Sync + Serialize + DeserializeOwned> ComponentCodec
    for TypedComponentCodec<T>
{
    fn type_name(&self) -> &'static str {
        self.name
    }

    fn component_type_id(&self) -> TypeId {
        TypeId::of::<T>()
    }

    fn serialize(&self, boxed: &(dyn Any + Send + Sync)) -> Result<serde_json::Value, CodecError> {
        let concrete = boxed
            .downcast_ref::<T>()
            .ok_or(CodecError::DowncastFailed(self.name))?;
        serde_json::to_value(concrete).map_err(|source| CodecError::Serialization {
            type_name: self.name,
            source,
        })
    }

    fn deserialize(
        &self,
        value: serde_json::Value,
    ) -> Result<Box<dyn Any + Send + Sync>, CodecError> {
        let concrete: T =
            serde_json::from_value(value).map_err(|source| CodecError::Deserialization {
                type_name: self.name,
                source,
            })?;
        Ok(Box::new(concrete))
    }
}

/// Registry managing dynamic component codecs for serialization and persistence.
#[derive(Clone, Default)]
pub struct ComponentCodecRegistry {
    by_type_id: HashMap<TypeId, Arc<dyn ComponentCodec>>,
    by_name: HashMap<String, Arc<dyn ComponentCodec>>,
}

impl ComponentCodecRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a serializable component type with a friendly name.
    pub fn register<T: 'static + Send + Sync + Serialize + DeserializeOwned>(
        &mut self,
        name: &'static str,
    ) {
        let codec = Arc::new(TypedComponentCodec::<T>::new(name));
        self.by_type_id.insert(TypeId::of::<T>(), codec.clone());
        self.by_name.insert(name.to_string(), codec);
    }

    /// Serializes all registered components from a `ComponentStore` into a JSON Object.
    pub fn serialize_all(&self, store: &ComponentStore) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        for (type_id, boxed) in store.iter() {
            if let Some(codec) = self.by_type_id.get(type_id) {
                match codec.serialize(boxed.as_ref()) {
                    Ok(json) => {
                        map.insert(codec.type_name().to_string(), json);
                    }
                    Err(err) => {
                        tracing::warn!(
                            component = codec.type_name(),
                            error = %err,
                            "Failed to serialize component"
                        );
                    }
                }
            }
        }
        serde_json::Value::Object(map)
    }

    /// Deserializes components from a JSON map into the specified `ComponentStore`.
    pub fn deserialize_into(
        &self,
        attributes: &serde_json::Value,
        store: &mut ComponentStore,
    ) -> Result<usize, CodecError> {
        let map = match attributes.as_object() {
            Some(m) => m,
            None => return Ok(0),
        };

        let mut restored = 0;
        for (name, val) in map {
            if let Some(codec) = self.by_name.get(name) {
                match codec.deserialize(val.clone()) {
                    Ok(boxed) => {
                        store.insert_raw(codec.component_type_id(), boxed);
                        restored += 1;
                    }
                    Err(err) => {
                        tracing::warn!(
                            component = name.as_str(),
                            error = %err,
                            "Failed to deserialize component"
                        );
                        return Err(err);
                    }
                }
            }
        }
        Ok(restored)
    }

    /// Serializes all registered components from a `ComponentStore` into an opaque MessagePack binary byte vector.
    pub fn serialize_all_binary(&self, store: &ComponentStore) -> Result<Vec<u8>, CodecError> {
        let json_val = self.serialize_all(store);
        let bytes = rmp_serde::to_vec_named(&json_val)?;
        Ok(bytes)
    }

    /// Maximum allowed binary state blob size (16 MB) to prevent out-of-memory attacks on malformed input.
    pub const MAX_STATE_BLOB_SIZE: usize = 16 * 1024 * 1024;

    /// Deserializes components from an opaque MessagePack binary slice into the specified `ComponentStore`.
    /// Validates slice size against `MAX_STATE_BLOB_SIZE`.
    pub fn deserialize_binary_into(
        &self,
        blob: &[u8],
        store: &mut ComponentStore,
    ) -> Result<usize, CodecError> {
        if blob.len() > Self::MAX_STATE_BLOB_SIZE {
            return Err(CodecError::PayloadTooLarge {
                size: blob.len(),
                max: Self::MAX_STATE_BLOB_SIZE,
            });
        }
        if blob.is_empty() {
            return Ok(0);
        }
        let val: serde_json::Value = rmp_serde::from_slice(blob)?;
        self.deserialize_into(&val, store)
    }
}

/// Marker trait for domain events that can be enqueued into [`EventQueue`].
pub trait DomainEvent: 'static + Send + Sync {}

/// Domain event emitted when an entity is removed/despawned from simulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityDespawnEvent {
    pub guid: crate::guid::Guid,
}

impl DomainEvent for EntityDespawnEvent {}

/// Type-erased event queue for deferred domain events bucketed by [`TypeId`].
#[derive(Default)]
pub struct EventQueue {
    queues: HashMap<TypeId, Vec<Box<dyn Any + Send + Sync>>>,
}

impl EventQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueues a domain event.
    pub fn push<E: DomainEvent>(&mut self, event: E) {
        self.queues
            .entry(TypeId::of::<E>())
            .or_default()
            .push(Box::new(event));
    }

    /// Drains all queued events of the specified type `E`.
    pub fn drain<E: DomainEvent>(&mut self) -> Vec<E> {
        let Some(items) = self.queues.remove(&TypeId::of::<E>()) else {
            return Vec::new();
        };

        let mut matched = Vec::with_capacity(items.len());
        for item in items {
            if let Ok(concrete) = item.downcast::<E>() {
                matched.push(*concrete);
            }
        }
        matched
    }

    /// Returns `true` if the queue has no events.
    pub fn is_empty(&self) -> bool {
        self.queues.values().all(|v| v.is_empty())
    }

    /// Total number of events currently queued.
    pub fn len(&self) -> usize {
        self.queues.values().map(|v| v.len()).sum()
    }

    /// Clears all queued events.
    pub fn clear(&mut self) {
        self.queues.clear();
    }
}

/// Context provided to batch [`TickSystem`]s during simulation update.
pub struct TickContext<'a> {
    pub entities: &'a mut EntityStore,
    pub octree: &'a mut crate::octree::Octree,
    pub events: &'a mut EventQueue,
    pub dt_sec: f32,
    pub tick: u64,
}

/// Batch system executed during the simulation tick over the entity store.
pub trait TickSystem: Send + Sync {
    /// System name for logging/profiling.
    fn name(&self) -> &'static str;
    /// Executes the system logic over the entities for the elapsed delta time.
    fn update(&mut self, ctx: &mut TickContext);
}

/// Handler for modular client packets addressed to a specific module.
pub trait ModularPacketHandler: Send + Sync {
    fn handle(
        &self,
        entity_guid: Guid,
        sub_opcode: u16,
        payload: &[u8],
        entities: &mut EntityStore,
    ) -> Result<Option<Vec<u8>>, PacketHandlerError>;
}

/// Router dispatching modular packets based on a 16-bit `module_id`.
#[derive(Default)]
pub struct ModularPacketRouter {
    handlers: HashMap<u16, Box<dyn ModularPacketHandler>>,
}

impl ModularPacketRouter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a handler for a given `module_id`.
    pub fn register(&mut self, module_id: u16, handler: Box<dyn ModularPacketHandler>) {
        self.handlers.insert(module_id, handler);
    }

    /// Dispatches a packet to the registered module handler.
    pub fn dispatch(
        &self,
        module_id: u16,
        sub_opcode: u16,
        entity_guid: Guid,
        payload: &[u8],
        entities: &mut EntityStore,
    ) -> Result<Option<Vec<u8>>, PacketHandlerError> {
        let handler = self
            .handlers
            .get(&module_id)
            .ok_or(PacketHandlerError::ModuleNotFound(module_id))?;
        handler.handle(entity_guid, sub_opcode, payload, entities)
    }

    /// Returns `true` if a handler exists for `module_id`.
    pub fn has_module(&self, module_id: u16) -> bool {
        self.handlers.contains_key(&module_id)
    }
}

/// Registration context passed to [`GameplayPlugin::build`].
pub struct PluginContext<'a> {
    pub hooks: &'a mut HookDispatcher,
    pub codecs: &'a mut ComponentCodecRegistry,
    pub event_queue: &'a mut EventQueue,
    pub packets: &'a mut ModularPacketRouter,
    pub tick_systems: &'a mut Vec<Box<dyn TickSystem>>,
}

/// Trait defining a standalone, decoupled gameplay plugin.
pub trait GameplayPlugin: Send + Sync {
    /// Unique identifier for this plugin.
    fn id(&self) -> &'static str;

    /// Registers components, event handlers, tick systems, and packet routes.
    fn build(&self, ctx: &mut PluginContext);
}

/// Manager that registers and builds all installed gameplay plugins.
#[derive(Default)]
pub struct PluginManager {
    plugins: Vec<Box<dyn GameplayPlugin>>,
}

impl PluginManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a plugin to the manager.
    pub fn add_plugin(&mut self, plugin: Box<dyn GameplayPlugin>) {
        self.plugins.push(plugin);
    }

    /// Builds all plugins with the given contexts.
    pub fn build_all(
        &self,
        hooks: &mut HookDispatcher,
        codecs: &mut ComponentCodecRegistry,
        event_queue: &mut EventQueue,
        packets: &mut ModularPacketRouter,
        tick_systems: &mut Vec<Box<dyn TickSystem>>,
    ) {
        let mut ctx = PluginContext {
            hooks,
            codecs,
            event_queue,
            packets,
            tick_systems,
        };

        for plugin in &self.plugins {
            plugin.build(&mut ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guid::EntityType;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct MockHealth {
        hp: u32,
        max_hp: u32,
    }

    #[derive(Debug, Clone, PartialEq)]
    struct MockDamageEvent {
        target: Guid,
        amount: u32,
    }
    impl DomainEvent for MockDamageEvent {}

    #[test]
    fn test_codec_registry_roundtrip() {
        let mut registry = ComponentCodecRegistry::new();
        println!("Line 316 TypeId: {:?}", TypeId::of::<MockHealth>());
        registry.register::<MockHealth>("mock_health");

        let mut store = ComponentStore::new();
        store.add_component(MockHealth {
            hp: 75,
            max_hp: 100,
        });

        let json = registry.serialize_all(&store);
        assert!(json.get("mock_health").is_some());
        assert_eq!(json["mock_health"]["hp"], 75);

        let mut new_store = ComponentStore::new();
        let restored = registry.deserialize_into(&json, &mut new_store).unwrap();
        assert_eq!(restored, 1);

        let restored_health = new_store.get_component::<MockHealth>().unwrap();
        assert_eq!(restored_health.hp, 75);
        assert_eq!(restored_health.max_hp, 100);
    }

    #[test]
    fn test_event_queue_push_and_drain() {
        let mut queue = EventQueue::new();
        let g1 = Guid::new(EntityType::Player, 1, 10);
        let g2 = Guid::new(EntityType::Player, 1, 20);

        queue.push(MockDamageEvent {
            target: g1,
            amount: 25,
        });
        queue.push(MockDamageEvent {
            target: g2,
            amount: 50,
        });

        assert_eq!(queue.len(), 2);
        let drained = queue.drain::<MockDamageEvent>();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].amount, 25);
        assert_eq!(drained[1].amount, 50);
        assert!(queue.is_empty());
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct MockHealEvent {
        target: Guid,
        amount: u32,
    }
    impl DomainEvent for MockHealEvent {}

    #[test]
    fn test_event_queue_type_id_bucket_isolation() {
        let mut queue = EventQueue::new();
        let g1 = Guid::new(EntityType::Player, 1, 10);
        let g2 = Guid::new(EntityType::Player, 1, 20);

        queue.push(MockDamageEvent {
            target: g1,
            amount: 25,
        });
        queue.push(MockHealEvent {
            target: g2,
            amount: 40,
        });

        assert_eq!(queue.len(), 2);
        let damage_events = queue.drain::<MockDamageEvent>();
        assert_eq!(damage_events.len(), 1);
        assert_eq!(damage_events[0].amount, 25);
        assert_eq!(queue.len(), 1);

        let heal_events = queue.drain::<MockHealEvent>();
        assert_eq!(heal_events.len(), 1);
        assert_eq!(heal_events[0].amount, 40);
        assert!(queue.is_empty());
    }

    struct MockPlugin;
    impl GameplayPlugin for MockPlugin {
        fn id(&self) -> &'static str {
            "mock_plugin"
        }

        fn build(&self, ctx: &mut PluginContext) {
            ctx.codecs.register::<MockHealth>("health");
        }
    }

    #[test]
    fn test_plugin_lifecycle_registration() {
        let mut manager = PluginManager::new();
        manager.add_plugin(Box::new(MockPlugin));

        let mut hooks = HookDispatcher::new();
        let mut codecs = ComponentCodecRegistry::new();
        let mut event_queue = EventQueue::new();
        let mut packets = ModularPacketRouter::new();
        let mut tick_systems = Vec::new();

        manager.build_all(
            &mut hooks,
            &mut codecs,
            &mut event_queue,
            &mut packets,
            &mut tick_systems,
        );

        let mut store = ComponentStore::new();
        store.add_component(MockHealth { hp: 50, max_hp: 50 });
        let json = codecs.serialize_all(&store);
        assert!(json.get("health").is_some());
    }
}
