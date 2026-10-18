//! Standalone Entity-Driven Spawner Plugin with Deferred Command Buffering.
//!
//! Spawners are entities that periodically evaluate active population limits,
//! tick respawn cooldowns, and queue entity instantiations into a deferred
//! command buffer to preserve strict ECS aliasing safety.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use mmo_core::{
    DomainEvent, EntityDespawnEvent, EntityStore, GameplayPlugin, Guid, Octree, PluginContext,
    TickContext, TickSystem,
};

use crate::combat::{HealthComponent, StatsComponent};
use crate::navigation::{LeashComponent, NavAgentComponent};

/// Configuration template defining attributes for spawned NPC entities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpawnTemplate {
    pub template_id: u32,
    pub name: String,
    pub max_health: f32,
    pub speed: f32,
    pub attack_power: u32,
    pub armor: u32,
    pub leash_radius: f32,
    #[serde(default)]
    pub properties: mmo_core::DynamicProperties,
}

impl SpawnTemplate {
    pub fn new(
        template_id: u32,
        name: impl Into<String>,
        max_health: f32,
        speed: f32,
        attack_power: u32,
        armor: u32,
        leash_radius: f32,
    ) -> Self {
        Self {
            template_id,
            name: name.into(),
            max_health,
            speed,
            attack_power,
            armor,
            leash_radius,
            properties: mmo_core::DynamicProperties::new(),
        }
    }

    /// Creates a `SpawnTemplate` populated from a schemaless `DynamicProperties` container.
    pub fn from_properties(template_id: u32, props: &mmo_core::DynamicProperties) -> Self {
        Self {
            template_id,
            name: props.get_str("name").unwrap_or("NPC").to_string(),
            max_health: props.get_f64("max_health").unwrap_or(100.0) as f32,
            speed: props.get_f64("speed").unwrap_or(5.0) as f32,
            attack_power: props.get_i64("attack_power").unwrap_or(10) as u32,
            armor: props.get_i64("armor").unwrap_or(0) as u32,
            leash_radius: props.get_f64("leash_radius").unwrap_or(15.0) as f32,
            properties: props.clone(),
        }
    }
}

/// Dynamic component attached to spawner entities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpawnerComponent {
    pub template_id: u32,
    pub max_active_spawns: usize,
    pub respawn_cooldown_secs: f32,
    pub current_cooldown_secs: f32,
    pub spawn_radius: f32,
    pub spawned_entities: Vec<Guid>,
}

impl SpawnerComponent {
    pub fn new(
        template_id: u32,
        max_active_spawns: usize,
        respawn_cooldown_secs: f32,
        spawn_radius: f32,
    ) -> Self {
        Self {
            template_id,
            max_active_spawns,
            respawn_cooldown_secs,
            current_cooldown_secs: 0.0, // Initial spawn is ready immediately
            spawn_radius,
            spawned_entities: Vec::new(),
        }
    }
}

/// Deferred command requesting the instantiation of an NPC entity.
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnEntityCommand {
    pub spawner_guid: Guid,
    pub template_id: u32,
    pub position: (f32, f32, f32),
    pub leash_origin: (f32, f32, f32),
    pub leash_radius: f32,
    pub speed: f32,
    pub max_health: f32,
    pub attack_power: u32,
    pub armor: u32,
    pub properties: mmo_core::DynamicProperties,
}

impl DomainEvent for SpawnEntityCommand {}

/// Deferred command buffer accumulating structural changes during a tick.
#[derive(Default, Debug, Clone)]
pub struct SpawnCommandBuffer {
    pub pending_spawns: Vec<SpawnEntityCommand>,
    pub pending_despawns: Vec<Guid>,
}

impl SpawnCommandBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_spawn(&mut self, cmd: SpawnEntityCommand) {
        self.pending_spawns.push(cmd);
    }

    pub fn push_despawn(&mut self, guid: Guid) {
        self.pending_despawns.push(guid);
    }

    pub fn is_empty(&self) -> bool {
        self.pending_spawns.is_empty() && self.pending_despawns.is_empty()
    }
}

/// Batch system evaluating spawner cooldowns and queuing structural spawn commands.
pub struct SpawnerSystem {
    templates: HashMap<u32, SpawnTemplate>,
    pub command_buffer: SpawnCommandBuffer,
    pub auto_flush: bool,
    pub node_id: u16,
    pub next_sequence: u32,
}

impl Default for SpawnerSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl SpawnerSystem {
    pub fn new() -> Self {
        Self {
            templates: HashMap::new(),
            command_buffer: SpawnCommandBuffer::new(),
            auto_flush: true,
            node_id: 1,
            next_sequence: 10_000,
        }
    }

    pub fn register_template(&mut self, template: SpawnTemplate) {
        self.templates.insert(template.template_id, template);
    }

    pub fn get_template(&self, template_id: u32) -> Option<&SpawnTemplate> {
        self.templates.get(&template_id)
    }

    /// Flushes all pending structural mutations from the command buffer into `EntityStore` and `Octree`.
    pub fn flush_commands(
        &mut self,
        entities: &mut EntityStore,
        octree: &mut Octree,
        mut id_gen: impl FnMut() -> Guid,
    ) {
        // 1. Process despawns
        for guid in self.command_buffer.pending_despawns.drain(..) {
            if let Some(entity) = entities.despawn_by_guid(guid) {
                octree.remove(guid, entity.position);
            }
        }

        // 2. Process spawns
        for cmd in self.command_buffer.pending_spawns.drain(..) {
            let new_guid = id_gen();
            let key = entities.spawn(new_guid, cmd.position);
            octree.insert(new_guid, cmd.position);

            if let Some(ent) = entities.get_mut(key) {
                ent.add_component(HealthComponent::new(cmd.max_health, 0.0));
                ent.add_component(StatsComponent::new(cmd.attack_power, cmd.armor));
                ent.add_component(NavAgentComponent::new(cmd.speed, 0.5));
                ent.add_component(LeashComponent::new(cmd.leash_origin, cmd.leash_radius));
                ent.properties.merge(&cmd.properties);
            }

            // Track spawned entity in the parent spawner
            if let Some(spawner_ent) = entities.get_mut_by_guid(cmd.spawner_guid) {
                if let Some(spawner) = spawner_ent.get_component_mut::<SpawnerComponent>() {
                    spawner.spawned_entities.push(new_guid);
                }
            }
        }
    }
}

impl TickSystem for SpawnerSystem {
    fn name(&self) -> &'static str {
        "SpawnerSystem"
    }

    #[allow(clippy::type_complexity)]
    fn update(&mut self, ctx: &mut TickContext) {
        // Drain despawn events to prune despawned entities reactively
        let despawns = ctx.events.drain::<EntityDespawnEvent>();
        let mut dead_or_missing: std::collections::HashSet<Guid> =
            despawns.iter().map(|e| e.guid).collect();

        // 1. Identify only the specific entities tracked by spawners
        let mut tracked_guids = Vec::new();
        for (_key, entity) in ctx.entities.iter() {
            if let Some(spawner) = entity.get_component::<SpawnerComponent>() {
                tracked_guids.extend(spawner.spawned_entities.iter().copied());
            }
        }

        // Check health of tracked entities only (O(K_spawned) instead of O(N_world))
        for guid in tracked_guids {
            match ctx.entities.get_by_guid(guid) {
                Some(ent) => {
                    if let Some(health) = ent.get_component::<HealthComponent>() {
                        if health.is_dead {
                            dead_or_missing.insert(guid);
                        }
                    }
                }
                None => {
                    dead_or_missing.insert(guid);
                }
            }
        }

        // 2. Evaluate spawners needing evaluation
        let mut spawn_requests: Vec<(Guid, u32, (f32, f32, f32), f32)> = Vec::new();

        for (_key, entity) in ctx.entities.iter_mut() {
            let spawner_pos = entity.position;
            let spawner_guid = entity.guid;

            if let Some(spawner) = entity.get_component_mut::<SpawnerComponent>() {
                // Prune dead or despawned entities from tracking without scanning all world entities
                spawner
                    .spawned_entities
                    .retain(|guid| !dead_or_missing.contains(guid));

                if spawner.spawned_entities.len() < spawner.max_active_spawns {
                    if spawner.current_cooldown_secs <= 0.0 {
                        // Cooldown elapsed; request spawn
                        spawner.current_cooldown_secs = spawner.respawn_cooldown_secs;
                        spawn_requests.push((
                            spawner_guid,
                            spawner.template_id,
                            spawner_pos,
                            spawner.spawn_radius,
                        ));
                    } else {
                        spawner.current_cooldown_secs -= ctx.dt_sec;
                    }
                }
            }
        }

        // Generate commands from templates
        for (spawner_guid, template_id, origin, radius) in spawn_requests {
            if let Some(tmpl) = self.templates.get(&template_id) {
                // Spawn slightly offset within spawn_radius (deterministic offset based on guid/radius)
                let spawn_pos = (origin.0 + radius * 0.5, origin.1, origin.2 + radius * 0.5);

                self.command_buffer.push_spawn(SpawnEntityCommand {
                    spawner_guid,
                    template_id,
                    position: spawn_pos,
                    leash_origin: origin,
                    leash_radius: tmpl.leash_radius,
                    speed: tmpl.speed,
                    max_health: tmpl.max_health,
                    attack_power: tmpl.attack_power,
                    armor: tmpl.armor,
                    properties: tmpl.properties.clone(),
                });
            }
        }

        // Auto-flush structural commands into EntityStore and Octree if enabled
        if self.auto_flush {
            let node_id = self.node_id;
            let mut seq = self.next_sequence;
            self.flush_commands(ctx.entities, ctx.octree, || {
                seq += 1;
                Guid::new(mmo_core::EntityType::Npc, node_id, seq)
            });
            self.next_sequence = seq;
        }

        // Re-push despawn events so subsequent systems can also consume them
        for despawn in despawns {
            ctx.events.push(despawn);
        }
    }
}

/// Standalone plugin registering spawner components and serialization codecs.
#[derive(Clone, Default)]
pub struct SpawnerPlugin {
    pub templates: Vec<SpawnTemplate>,
}

impl SpawnerPlugin {
    pub fn new(templates: Vec<SpawnTemplate>) -> Self {
        Self { templates }
    }
}

impl GameplayPlugin for SpawnerPlugin {
    fn id(&self) -> &'static str {
        "spawner"
    }

    fn build(&self, ctx: &mut PluginContext) {
        ctx.codecs.register::<SpawnerComponent>("spawner");
        let mut system = SpawnerSystem::new();
        for tmpl in &self.templates {
            system.register_template(tmpl.clone());
        }
        ctx.tick_systems.push(Box::new(system));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::{BoundingBox3D, EntityType, EventQueue};

    #[test]
    fn test_spawner_lifecycle_and_deferred_flushing() {
        let mut store = EntityStore::new();
        let mut octree = Octree::new(BoundingBox3D::new(
            (-100.0, -100.0, -100.0),
            (100.0, 100.0, 100.0),
        ));
        let mut events = EventQueue::new();

        let mut spawner_system = SpawnerSystem::new();
        spawner_system.auto_flush = false;
        spawner_system.register_template(SpawnTemplate::new(
            1,
            "Goblin Raider",
            50.0,
            6.0,
            12,
            5,
            20.0,
        ));

        // Spawn a spawner entity
        let spawner_guid = Guid::new(EntityType::Npc, 1, 999);
        let spawner_key = store.spawn(spawner_guid, (10.0, 0.0, 10.0));
        octree.insert(spawner_guid, (10.0, 0.0, 10.0));

        let spawner_comp = SpawnerComponent::new(1, 2, 5.0, 2.0); // max 2 spawns, 5s respawn
        store
            .get_mut(spawner_key)
            .unwrap()
            .add_component(spawner_comp);

        // Tick 1: Initial cooldown is 0.0, so it queues 1 spawn
        {
            let mut ctx = TickContext {
                entities: &mut store,
                octree: &mut octree,
                events: &mut events,
                dt_sec: 0.1,
                tick: 1,
                collision_world: None,
            };
            spawner_system.update(&mut ctx);
        }

        assert_eq!(spawner_system.command_buffer.pending_spawns.len(), 1);

        // Flush structural commands
        let mut counter = 1000u32;
        spawner_system.flush_commands(&mut store, &mut octree, || {
            counter += 1;
            Guid::new(EntityType::Npc, 1, counter)
        });

        assert_eq!(spawner_system.command_buffer.pending_spawns.len(), 0);
        let spawned_guid = Guid::new(EntityType::Npc, 1, 1001);

        // Verify spawned entity has components attached
        let spawned_ent = store.get_by_guid(spawned_guid).unwrap();
        assert_eq!(
            spawned_ent
                .get_component::<HealthComponent>()
                .unwrap()
                .current_hp,
            50.0
        );
        assert_eq!(
            spawned_ent
                .get_component::<StatsComponent>()
                .unwrap()
                .attack_power,
            12
        );
        assert_eq!(
            spawned_ent
                .get_component::<NavAgentComponent>()
                .unwrap()
                .speed,
            6.0
        );

        // Verify spawner tracks the spawned GUID
        let spawner_ent = store.get(spawner_key).unwrap();
        assert_eq!(
            spawner_ent
                .get_component::<SpawnerComponent>()
                .unwrap()
                .spawned_entities,
            vec![spawned_guid]
        );

        // Simulate spawned entity death
        store
            .get_mut_by_guid(spawned_guid)
            .unwrap()
            .get_component_mut::<HealthComponent>()
            .unwrap()
            .is_dead = true;

        // Tick again: Spawner detects death and drops it from spawned_entities
        {
            let mut ctx = TickContext {
                entities: &mut store,
                octree: &mut octree,
                events: &mut events,
                dt_sec: 0.1,
                tick: 2,
                collision_world: None,
            };
            spawner_system.update(&mut ctx);
        }

        let spawner_ent = store.get(spawner_key).unwrap();
        assert!(spawner_ent
            .get_component::<SpawnerComponent>()
            .unwrap()
            .spawned_entities
            .is_empty());
    }

    #[test]
    fn test_spawner_from_schemaless_properties() {
        let mut store = EntityStore::new();
        let mut octree = Octree::new(BoundingBox3D::new(
            (-100.0, -100.0, -100.0),
            (100.0, 100.0, 100.0),
        ));
        let mut events = EventQueue::new();

        let mut dyn_props = mmo_core::DynamicProperties::new();
        dyn_props.set("name", "Elder Dragon");
        dyn_props.set("max_health", 5000.0);
        dyn_props.set("speed", 12.0);
        dyn_props.set("attack_power", 250);
        dyn_props.set("armor", 80);
        dyn_props.set("leash_radius", 50.0);
        dyn_props.set("element", "fire");
        dyn_props.set("loot_tier", "legendary");

        let tmpl = SpawnTemplate::from_properties(42, &dyn_props);
        assert_eq!(tmpl.name, "Elder Dragon");
        assert_eq!(tmpl.max_health, 5000.0);
        assert_eq!(tmpl.attack_power, 250);
        assert_eq!(tmpl.armor, 80);

        let mut spawner_system = SpawnerSystem::new();
        spawner_system.register_template(tmpl);

        let spawner_guid = Guid::new(EntityType::Npc, 1, 10);
        let spawner_key = store.spawn(spawner_guid, (0.0, 0.0, 0.0));
        store
            .get_mut(spawner_key)
            .unwrap()
            .add_component(SpawnerComponent::new(42, 1, 10.0, 5.0));

        let mut ctx = TickContext {
            entities: &mut store,
            octree: &mut octree,
            events: &mut events,
            dt_sec: 0.1,
            tick: 1,
            collision_world: None,
        };
        spawner_system.update(&mut ctx);

        // Find spawned entity
        let spawned = store
            .iter()
            .find(|(k, _)| *k != spawner_key)
            .map(|(_, e)| e)
            .unwrap();
        assert_eq!(
            spawned.get_component::<HealthComponent>().unwrap().max_hp,
            5000.0
        );
        assert_eq!(
            spawned
                .get_component::<StatsComponent>()
                .unwrap()
                .attack_power,
            250
        );
        assert_eq!(spawned.get_property_str("element"), Some("fire"));
        assert_eq!(spawned.get_property_str("loot_tier"), Some("legendary"));
    }
}
