//! Modular Gameplay Plugin Architecture.
//!
//! Provides decoupled abstractions for gameplay plugins:
//! - [`GameplayPlugin`] & [`PluginContext`]: Lifecycle and registration interfaces.
//! - [`ComponentCodecRegistry`]: Dynamic JSON serialization and deserialization for attached entity components.
//! - [`EventQueue`]: Deferred type-erased event queue drained at tick boundaries to prevent borrow checker conflicts and re-entrancy.
//! - [`TickSystem`]: Batch per-tick update systems iterating over entities without dynamic dispatch overhead.
//! - [`ModularPacketRouter`]: Opcode-based network packet routing to plugin handlers.

use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
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

    /// Serializes a boxed component directly into MessagePack bytes without an intermediate JSON tree.
    fn serialize_binary(
        &self,
        boxed: &(dyn Any + Send + Sync),
        out: &mut Vec<u8>,
    ) -> Result<(), CodecError> {
        let val = self.serialize(boxed)?;
        rmp_serde::encode::write_named(out, &val)?;
        Ok(())
    }

    /// Deserializes MessagePack bytes directly into a type-erased component box.
    fn deserialize_binary(&self, bytes: &[u8]) -> Result<Box<dyn Any + Send + Sync>, CodecError> {
        let val: serde_json::Value = rmp_serde::from_slice(bytes)?;
        self.deserialize(val)
    }
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

    fn serialize_binary(
        &self,
        boxed: &(dyn Any + Send + Sync),
        out: &mut Vec<u8>,
    ) -> Result<(), CodecError> {
        let concrete = boxed
            .downcast_ref::<T>()
            .ok_or(CodecError::DowncastFailed(self.name))?;
        rmp_serde::encode::write_named(out, concrete)?;
        Ok(())
    }

    fn deserialize_binary(&self, bytes: &[u8]) -> Result<Box<dyn Any + Send + Sync>, CodecError> {
        let concrete: T = rmp_serde::from_slice(bytes)?;
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

    /// Serializes all registered components from a `ComponentStore` into an opaque MessagePack binary byte vector
    /// without allocating an intermediate `serde_json::Value` tree.
    pub fn serialize_all_binary(&self, store: &ComponentStore) -> Result<Vec<u8>, CodecError> {
        use serde::ser::{SerializeMap, Serializer};

        struct MsgPackBytes<'a>(&'a [u8]);
        impl serde::Serialize for MsgPackBytes<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_bytes(self.0)
            }
        }

        let mut comp_buf = Vec::new();
        let mut entries: Vec<(&'static str, std::ops::Range<usize>)> =
            Vec::with_capacity(store.len());

        for (type_id, boxed) in store.iter() {
            if let Some(codec) = self.by_type_id.get(type_id) {
                let start = comp_buf.len();
                match codec.serialize_binary(boxed.as_ref(), &mut comp_buf) {
                    Ok(()) => entries.push((codec.type_name(), start..comp_buf.len())),
                    Err(err) => {
                        comp_buf.truncate(start);
                        tracing::warn!(
                            component = codec.type_name(),
                            error = %err,
                            "Failed to serialize component to binary"
                        );
                    }
                }
            }
        }

        let mut out = Vec::with_capacity(comp_buf.len() + entries.len() * 24 + 8);
        let mut ser = rmp_serde::Serializer::new(&mut out).with_struct_map();
        let mut map = ser.serialize_map(Some(entries.len()))?;
        for (name, range) in entries {
            map.serialize_entry(name, &MsgPackBytes(&comp_buf[range]))?;
        }
        map.end()?;
        Ok(out)
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

        enum ComponentBinaryValue<'de> {
            Bytes(&'de [u8]),
            OwnedBytes(Vec<u8>),
            LegacyJson(serde_json::Value),
        }

        struct ComponentBinaryValueSeed;

        impl<'de> serde::de::DeserializeSeed<'de> for ComponentBinaryValueSeed {
            type Value = ComponentBinaryValue<'de>;

            fn deserialize<D: serde::Deserializer<'de>>(
                self,
                deserializer: D,
            ) -> Result<Self::Value, D::Error> {
                struct ValueVisitor;

                impl<'de> serde::de::Visitor<'de> for ValueVisitor {
                    type Value = ComponentBinaryValue<'de>;

                    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        f.write_str("component binary bytes or legacy JSON-compatible value")
                    }

                    fn visit_borrowed_bytes<E: serde::de::Error>(
                        self,
                        v: &'de [u8],
                    ) -> Result<Self::Value, E> {
                        Ok(ComponentBinaryValue::Bytes(v))
                    }

                    fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<Self::Value, E> {
                        Ok(ComponentBinaryValue::OwnedBytes(v.to_vec()))
                    }

                    fn visit_byte_buf<E: serde::de::Error>(
                        self,
                        v: Vec<u8>,
                    ) -> Result<Self::Value, E> {
                        Ok(ComponentBinaryValue::OwnedBytes(v))
                    }

                    fn visit_map<A: serde::de::MapAccess<'de>>(
                        self,
                        map: A,
                    ) -> Result<Self::Value, A::Error> {
                        serde::Deserialize::deserialize(
                            serde::de::value::MapAccessDeserializer::new(map),
                        )
                        .map(ComponentBinaryValue::LegacyJson)
                    }

                    fn visit_seq<A: serde::de::SeqAccess<'de>>(
                        self,
                        seq: A,
                    ) -> Result<Self::Value, A::Error> {
                        serde::Deserialize::deserialize(
                            serde::de::value::SeqAccessDeserializer::new(seq),
                        )
                        .map(ComponentBinaryValue::LegacyJson)
                    }

                    fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> {
                        Ok(ComponentBinaryValue::LegacyJson(serde_json::Value::Bool(v)))
                    }

                    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                        Ok(ComponentBinaryValue::LegacyJson(serde_json::Value::Number(
                            v.into(),
                        )))
                    }

                    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
                        Ok(ComponentBinaryValue::LegacyJson(serde_json::Value::Number(
                            v.into(),
                        )))
                    }

                    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
                        let num = serde_json::Number::from_f64(v).ok_or_else(|| {
                            serde::de::Error::custom("non-finite float in component blob")
                        })?;
                        Ok(ComponentBinaryValue::LegacyJson(serde_json::Value::Number(
                            num,
                        )))
                    }

                    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                        Ok(ComponentBinaryValue::LegacyJson(serde_json::Value::String(
                            v.to_string(),
                        )))
                    }

                    fn visit_string<E: serde::de::Error>(
                        self,
                        v: String,
                    ) -> Result<Self::Value, E> {
                        Ok(ComponentBinaryValue::LegacyJson(serde_json::Value::String(
                            v,
                        )))
                    }

                    fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                        Ok(ComponentBinaryValue::LegacyJson(serde_json::Value::Null))
                    }
                }

                deserializer.deserialize_any(ValueVisitor)
            }
        }

        struct ComponentMapVisitor<'a> {
            registry: &'a ComponentCodecRegistry,
            store: &'a mut ComponentStore,
        }

        impl<'de> serde::de::Visitor<'de> for ComponentMapVisitor<'_> {
            type Value = Result<usize, CodecError>;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("MessagePack map of component name to component payload")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut restored = 0usize;
                while let Some(name) = map.next_key::<std::borrow::Cow<'de, str>>()? {
                    if let Some(codec) = self.registry.by_name.get(name.as_ref()) {
                        let val = map.next_value_seed(ComponentBinaryValueSeed)?;
                        let res = match val {
                            ComponentBinaryValue::Bytes(bytes) => codec.deserialize_binary(bytes),
                            ComponentBinaryValue::OwnedBytes(bytes) => {
                                codec.deserialize_binary(&bytes)
                            }
                            ComponentBinaryValue::LegacyJson(json_val) => {
                                codec.deserialize(json_val)
                            }
                        };
                        match res {
                            Ok(boxed) => {
                                self.store.insert_raw(codec.component_type_id(), boxed);
                                restored += 1;
                            }
                            Err(err) => {
                                tracing::warn!(
                                    component = name.as_ref(),
                                    error = %err,
                                    "Failed to deserialize component from binary"
                                );
                                return Ok(Err(err));
                            }
                        }
                    } else {
                        let _ = map.next_value::<serde::de::IgnoredAny>()?;
                    }
                }
                Ok(Ok(restored))
            }
        }

        let mut de = rmp_serde::Deserializer::from_read_ref(blob);
        serde::Deserializer::deserialize_map(
            &mut de,
            ComponentMapVisitor {
                registry: self,
                store,
            },
        )?
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

/// Domain event emitted when an entity is incapacitated or destroyed in simulation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityIncapacitatedEvent {
    pub entity: Guid,
    pub instigator: Option<Guid>,
}

impl DomainEvent for EntityIncapacitatedEvent {}

/// Type-erased event queue for deferred domain events bucketed by [`TypeId`].
#[derive(Default)]
pub struct EventQueue {
    queues: HashMap<TypeId, Vec<Box<dyn Any + Send + Sync>>>,
    ephemeral_types: HashSet<TypeId>,
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

    /// Clears all queued events across all types, releasing all allocated event buffers.
    pub fn clear(&mut self) {
        self.queues.clear();
    }

    /// Retains at most `max_per_type` most recent events per type, pruning older unhandled events
    /// to prevent unbounded memory growth in long-running simulation loops.
    pub fn retain_recent(&mut self, max_per_type: usize) {
        for queue in self.queues.values_mut() {
            if queue.len() > max_per_type {
                let to_drop = queue.len() - max_per_type;
                queue.drain(0..to_drop);
            }
        }
    }

    /// Registers the specified domain event type `E` as ephemeral.
    ///
    /// Ephemeral events are per-tick notifications that are cleared at tick boundaries
    /// via [`clear_ephemeral`] to prevent memory leaks in simulation loops.
    pub fn register_ephemeral<E: DomainEvent>(&mut self) {
        self.ephemeral_types.insert(TypeId::of::<E>());
    }

    /// Returns `true` if event type `E` is registered as ephemeral.
    pub fn is_ephemeral<E: DomainEvent>(&self) -> bool {
        self.ephemeral_types.contains(&TypeId::of::<E>())
    }

    /// Clears all queued events for registered ephemeral types while preserving buffer capacity.
    pub fn clear_ephemeral(&mut self) {
        for type_id in &self.ephemeral_types {
            if let Some(queue) = self.queues.get_mut(type_id) {
                queue.clear();
            }
        }
    }

    /// Clears all queued events of the specified type `E`.
    pub fn clear_type<E: DomainEvent>(&mut self) {
        self.queues.remove(&TypeId::of::<E>());
    }
}

/// Context provided to batch [`TickSystem`]s during simulation update.
pub struct TickContext<'a> {
    pub entities: &'a mut EntityStore,
    pub octree: &'a mut crate::octree::Octree,
    pub events: &'a mut EventQueue,
    pub dt_sec: f32,
    pub tick: u64,
    pub collision_world: Option<&'a dyn crate::collision::CollisionWorld>,
}

/// Batch system executed during the simulation tick over the entity store.
pub trait TickSystem: Send + Sync {
    /// System name for logging/profiling.
    fn name(&self) -> &'static str;
    /// Executes the system logic over the entities for the elapsed delta time.
    fn update(&mut self, ctx: &mut TickContext);
    /// Optional lifecycle callback executed at the end of the simulation tick
    /// prior to ephemeral event queue clearing.
    fn end_of_tick(&mut self, _ctx: &mut TickContext) {}
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
    pub combat_resolver: &'a mut Option<Arc<dyn crate::combat::CombatResolver>>,
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
        combat_resolver: &mut Option<Arc<dyn crate::combat::CombatResolver>>,
    ) {
        let mut ctx = PluginContext {
            hooks,
            codecs,
            event_queue,
            packets,
            tick_systems,
            combat_resolver,
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
        let mut combat_resolver = None;

        manager.build_all(
            &mut hooks,
            &mut codecs,
            &mut event_queue,
            &mut packets,
            &mut tick_systems,
            &mut combat_resolver,
        );

        let mut store = ComponentStore::new();
        store.add_component(MockHealth { hp: 50, max_hp: 50 });
        let json = codecs.serialize_all(&store);
        assert!(json.get("health").is_some());
    }

    #[test]
    fn test_event_queue_ephemeral_lifecycle() {
        let mut queue = EventQueue::new();
        let g1 = Guid::new(EntityType::Player, 1, 10);
        let g2 = Guid::new(EntityType::Player, 1, 20);

        queue.register_ephemeral::<MockDamageEvent>();
        assert!(queue.is_ephemeral::<MockDamageEvent>());
        assert!(!queue.is_ephemeral::<MockHealEvent>());

        queue.push(MockDamageEvent {
            target: g1,
            amount: 15,
        });
        queue.push(MockDamageEvent {
            target: g2,
            amount: 30,
        });
        queue.push(MockHealEvent {
            target: g1,
            amount: 50,
        });

        assert_eq!(queue.len(), 3);

        // Clearing ephemeral should remove MockDamageEvent instances while keeping MockHealEvent
        queue.clear_ephemeral();
        assert_eq!(queue.len(), 1);
        assert!(queue.drain::<MockDamageEvent>().is_empty());

        let heals = queue.drain::<MockHealEvent>();
        assert_eq!(heals.len(), 1);
        assert_eq!(heals[0].amount, 50);

        // Test clear_type
        queue.push(MockHealEvent {
            target: g2,
            amount: 25,
        });
        assert_eq!(queue.len(), 1);
        queue.clear_type::<MockHealEvent>();
        assert!(queue.is_empty());
    }

    #[test]
    fn test_codec_registry_binary_direct_roundtrip_and_legacy_compat() {
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        struct MockCooldowns {
            ability_cooldowns: HashMap<u32, f32>,
            gcd: f32,
        }

        let mut registry = ComponentCodecRegistry::new();
        registry.register::<MockHealth>("mock_health");
        registry.register::<MockCooldowns>("mock_cooldowns");

        let mut store = ComponentStore::new();
        store.add_component(MockHealth {
            hp: 42,
            max_hp: 100,
        });
        let mut cooldowns = HashMap::new();
        cooldowns.insert(101, 2.5);
        cooldowns.insert(202, 0.75);
        store.add_component(MockCooldowns {
            ability_cooldowns: cooldowns.clone(),
            gcd: 1.5,
        });

        let blob = registry.serialize_all_binary(&store).unwrap();
        assert!(!blob.is_empty());

        let mut restored_store = ComponentStore::new();
        let restored = registry
            .deserialize_binary_into(&blob, &mut restored_store)
            .unwrap();
        assert_eq!(restored, 2);
        assert_eq!(
            restored_store.get_component::<MockHealth>(),
            Some(&MockHealth {
                hp: 42,
                max_hp: 100,
            })
        );
        assert_eq!(
            restored_store.get_component::<MockCooldowns>(),
            Some(&MockCooldowns {
                ability_cooldowns: cooldowns,
                gcd: 1.5,
            })
        );

        // Verify empty store roundtrip
        let empty_blob = registry
            .serialize_all_binary(&ComponentStore::new())
            .unwrap();
        let mut empty_restored = ComponentStore::new();
        assert_eq!(
            registry
                .deserialize_binary_into(&empty_blob, &mut empty_restored)
                .unwrap(),
            0
        );

        // Verify backward compatibility with legacy serde_json::Value-encoded MessagePack blobs
        let mut legacy_store = ComponentStore::new();
        legacy_store.add_component(MockHealth {
            hp: 88,
            max_hp: 120,
        });
        let legacy_json = registry.serialize_all(&legacy_store);
        let legacy_blob = rmp_serde::to_vec_named(&legacy_json).unwrap();
        let mut legacy_restored = ComponentStore::new();
        assert_eq!(
            registry
                .deserialize_binary_into(&legacy_blob, &mut legacy_restored)
                .unwrap(),
            1
        );
        assert_eq!(
            legacy_restored.get_component::<MockHealth>(),
            Some(&MockHealth {
                hp: 88,
                max_hp: 120,
            })
        );
    }
}
