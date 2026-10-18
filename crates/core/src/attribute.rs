//! Dynamic Attribute and Timed Modifier Primitives.
//!
//! Provides a genre-agnostic numerical attribute and aura system supporting:
//! - Scalar properties with deterministic bounds clamping.
//! - Order-independent flat and percentage modifier stacking: `(Base + Sum(Flat)) * (1.0 + Sum(Percent))`.
//! - Passive tick-based regeneration and decay curves.
//! - Timed modifier (aura) lifecycles with expiration event dispatch.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::guid::Guid;
use crate::plugin::{DomainEvent, EventQueue, TickContext, TickSystem};

/// Representation of a single numerical attribute with bounds and regeneration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttributeValue {
    pub base: f32,
    pub current: f32,
    pub min: f32,
    pub max: f32,
    pub regen_per_sec: f32,
}

impl AttributeValue {
    pub fn new(base: f32, min: f32, max: f32, regen_per_sec: f32) -> Self {
        let clamped_current = base.clamp(min, max);
        Self {
            base,
            current: clamped_current,
            min,
            max,
            regen_per_sec,
        }
    }
}

/// Type of modifier applied to an attribute value.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ModifierType {
    /// Adds a flat offset directly to the base value (e.g. +10.0 armor).
    Flat(f32),
    /// Multiplies the base + flat sum by a percentage factor (e.g. +0.20 for +20% speed).
    Percent(f32),
}

/// A timed modifier (buff, debuff, or temporary enchantment) modifying an attribute.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimedModifier {
    pub id: String,
    pub attribute_name: String,
    pub modifier: ModifierType,
    pub remaining_duration_sec: f32,
}

impl TimedModifier {
    pub fn new(
        id: impl Into<String>,
        attribute_name: impl Into<String>,
        modifier: ModifierType,
        duration_sec: f32,
    ) -> Self {
        Self {
            id: id.into(),
            attribute_name: attribute_name.into(),
            modifier,
            remaining_duration_sec: duration_sec.max(0.0),
        }
    }
}

/// Event dispatched when a timed modifier (aura) expires.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuraExpiredEvent {
    pub entity: Guid,
    pub modifier_id: String,
    pub attribute_name: String,
}
impl DomainEvent for AuraExpiredEvent {}

/// Component holding all dynamic attributes and active modifiers for an entity.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AttributeSet {
    pub attributes: HashMap<String, AttributeValue>,
    pub modifiers: Vec<TimedModifier>,
}

impl AttributeSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a new attribute with initial base bounds and regen rate.
    pub fn add_attribute(
        &mut self,
        name: impl Into<String>,
        base: f32,
        min: f32,
        max: f32,
        regen_per_sec: f32,
    ) {
        self.attributes.insert(
            name.into(),
            AttributeValue::new(base, min, max, regen_per_sec),
        );
    }

    /// Returns a reference to the raw [`AttributeValue`] if registered.
    pub fn get_attribute(&self, name: &str) -> Option<&AttributeValue> {
        self.attributes.get(name)
    }

    /// Calculates the effective maximum value applying all attached flat and percentage modifiers:
    /// `Effective = (Base + Sum(Flat)) * (1.0 + Sum(Percent))`
    pub fn get_effective_max(&self, name: &str) -> Option<f32> {
        let attr = self.attributes.get(name)?;
        Some(compute_effective_max_for(name, attr, &self.modifiers))
    }

    /// Returns the current value of the attribute, clamped to its effective maximum.
    pub fn get_current(&self, name: &str) -> Option<f32> {
        let attr = self.attributes.get(name)?;
        let effective_max = self.get_effective_max(name)?;
        Some(attr.current.clamp(attr.min, effective_max))
    }

    /// Modifies the current value by `delta` (positive for heal/gain, negative for damage/loss),
    /// clamping strictly to `[min, effective_max]`.
    pub fn modify_current(&mut self, name: &str, delta: f32) -> Result<f32, &'static str> {
        let effective_max = self
            .get_effective_max(name)
            .ok_or("Attribute not found in AttributeSet")?;
        let attr = self
            .attributes
            .get_mut(name)
            .ok_or("Attribute not found in AttributeSet")?;

        attr.current = (attr.current + delta).clamp(attr.min, effective_max);
        Ok(attr.current)
    }

    /// Directly sets the current value, strictly clamped to `[min, effective_max]`.
    pub fn set_current(&mut self, name: &str, value: f32) -> Result<f32, &'static str> {
        let effective_max = self
            .get_effective_max(name)
            .ok_or("Attribute not found in AttributeSet")?;
        let attr = self
            .attributes
            .get_mut(name)
            .ok_or("Attribute not found in AttributeSet")?;

        attr.current = value.clamp(attr.min, effective_max);
        Ok(attr.current)
    }

    /// Attaches a timed modifier.
    pub fn add_modifier(&mut self, modifier: TimedModifier) {
        self.modifiers.push(modifier);
    }

    /// Removes all modifiers matching the specified ID. Returns `true` if any were removed.
    pub fn remove_modifier(&mut self, id: &str) -> bool {
        let initial_len = self.modifiers.len();
        self.modifiers.retain(|m| m.id != id);
        self.modifiers.len() < initial_len
    }

    /// Ticks attribute regeneration and advances modifier expiration.
    pub fn tick(&mut self, entity: Guid, dt_sec: f32, events: &mut EventQueue) {
        // 1. Advance passive regeneration/decay
        for (name, attr) in &mut self.attributes {
            if attr.regen_per_sec != 0.0 {
                let effective_max = compute_effective_max_for(name, attr, &self.modifiers);
                let delta = attr.regen_per_sec * dt_sec;
                attr.current = (attr.current + delta).clamp(attr.min, effective_max);
            }
        }

        // 2. Advance timed modifiers and drain expired
        let mut expired = Vec::new();
        self.modifiers.retain_mut(|modifier| {
            modifier.remaining_duration_sec -= dt_sec;
            if modifier.remaining_duration_sec <= 0.0 {
                expired.push((modifier.id.clone(), modifier.attribute_name.clone()));
                false
            } else {
                true
            }
        });

        // 3. Emit expiration events
        for (modifier_id, attribute_name) in expired {
            events.push(AuraExpiredEvent {
                entity,
                modifier_id,
                attribute_name,
            });
        }
    }
}

/// Computes the effective maximum for an attribute given active modifiers without allocating.
#[inline]
fn compute_effective_max_for(
    name: &str,
    attr: &AttributeValue,
    modifiers: &[TimedModifier],
) -> f32 {
    let mut sum_flat = 0.0;
    let mut sum_percent = 0.0;

    for modifier in modifiers {
        if modifier.attribute_name == name {
            match modifier.modifier {
                ModifierType::Flat(val) => sum_flat += val,
                ModifierType::Percent(val) => sum_percent += val,
            }
        }
    }

    let effective = (attr.max + sum_flat) * (1.0 + sum_percent);
    effective.max(attr.min)
}

/// Batch system executed during the simulation tick to advance attributes and auras.
pub struct AttributeTickSystem;

impl TickSystem for AttributeTickSystem {
    fn name(&self) -> &'static str {
        "AttributeTickSystem"
    }

    fn update(&mut self, ctx: &mut TickContext) {
        for (_key, entity) in ctx.entities.iter_mut() {
            let entity_guid = entity.guid;
            if let Some(attr_set) = entity.get_component_mut::<AttributeSet>() {
                attr_set.tick(entity_guid, ctx.dt_sec, ctx.events);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guid::EntityType;
    use crate::octree::Octree;

    #[test]
    fn test_attribute_clamping_on_modification() {
        let mut set = AttributeSet::new();
        set.add_attribute("health", 100.0, 0.0, 100.0, 0.0);

        // Deduct 150 damage -> clamped to 0.0
        let current = set.modify_current("health", -150.0).unwrap();
        assert_eq!(current, 0.0);

        // Heal 200 -> clamped to 100.0
        let current = set.modify_current("health", 200.0).unwrap();
        assert_eq!(current, 100.0);
    }

    #[test]
    fn test_order_independent_flat_and_percent_modifier_stacking() {
        let mut set = AttributeSet::new();
        set.add_attribute("speed", 10.0, 0.0, 10.0, 0.0);

        // Modifier 1: Flat +2.0
        set.add_modifier(TimedModifier::new(
            "mod1",
            "speed",
            ModifierType::Flat(2.0),
            10.0,
        ));
        // Modifier 2: Percent +20% (+0.20)
        set.add_modifier(TimedModifier::new(
            "mod2",
            "speed",
            ModifierType::Percent(0.20),
            10.0,
        ));

        // Formula: (10.0 + 2.0) * (1.0 + 0.20) = 12.0 * 1.20 = 14.4
        let effective_max = set.get_effective_max("speed").unwrap();
        assert!((effective_max - 14.4).abs() < 1e-4);

        // Remove flat modifier mod1 -> (10.0 + 0) * 1.20 = 12.0
        assert!(set.remove_modifier("mod1"));
        let effective_max = set.get_effective_max("speed").unwrap();
        assert!((effective_max - 12.0).abs() < 1e-4);

        // Remove percent modifier mod2 -> 10.0
        assert!(set.remove_modifier("mod2"));
        let effective_max = set.get_effective_max("speed").unwrap();
        assert!((effective_max - 10.0).abs() < 1e-4);
    }

    #[test]
    fn test_passive_regeneration_and_aura_expiration_via_tick_system() {
        let mut entities = crate::entity::EntityStore::new();
        let bounds =
            crate::octree::BoundingBox3D::new((-100.0, -100.0, -100.0), (100.0, 100.0, 100.0));
        let mut octree = Octree::new(bounds);
        let mut events = EventQueue::new();
        let guid = Guid::new(EntityType::Player, 1, 500);

        let key = entities.spawn(guid, (0.0, 0.0, 0.0));
        let mut set = AttributeSet::new();
        // energy: base=50.0, min=0.0, max=100.0, regen=+10.0/sec
        set.add_attribute("energy", 50.0, 0.0, 100.0, 10.0);
        // Temporary buff: duration = 0.08s (80ms)
        set.add_modifier(TimedModifier::new(
            "haste",
            "energy",
            ModifierType::Flat(5.0),
            0.08,
        ));
        entities.get_mut(key).unwrap().add_component(set);

        let mut tick_system = AttributeTickSystem;

        // Tick 1: dt = 0.05s (50ms)
        // Regen: 50.0 + 10.0 * 0.05 = 50.5
        // Buff remaining: 0.08 - 0.05 = 0.03s -> still active
        {
            let mut ctx = TickContext {
                entities: &mut entities,
                octree: &mut octree,
                events: &mut events,
                dt_sec: 0.05,
                tick: 1,
            };
            tick_system.update(&mut ctx);
        }

        let expired = events.drain::<AuraExpiredEvent>();
        assert_eq!(expired.len(), 0);

        let ent = entities.get_by_guid(guid).unwrap();
        let attrs = ent.get_component::<AttributeSet>().unwrap();
        assert!((attrs.get_current("energy").unwrap() - 50.5).abs() < 1e-4);
        assert_eq!(attrs.modifiers.len(), 1);

        // Tick 2: dt = 0.05s (50ms)
        // Regen: 50.5 + 0.5 = 51.0
        // Buff remaining: 0.03 - 0.05 <= 0 -> Expired!
        {
            let mut ctx = TickContext {
                entities: &mut entities,
                octree: &mut octree,
                events: &mut events,
                dt_sec: 0.05,
                tick: 2,
            };
            tick_system.update(&mut ctx);
        }

        let expired = events.drain::<AuraExpiredEvent>();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].modifier_id, "haste");
        assert_eq!(expired[0].attribute_name, "energy");

        let ent = entities.get_by_guid(guid).unwrap();
        let attrs = ent.get_component::<AttributeSet>().unwrap();
        assert!((attrs.get_current("energy").unwrap() - 51.0).abs() < 1e-4);
        assert_eq!(attrs.modifiers.len(), 0);
    }
}
