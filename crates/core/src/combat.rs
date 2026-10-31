//! Pluggable Combat Resolution Substrate.
//!
//! Provides genre-agnostic interfaces for resolving combat effects, damage mitigation,
//! and vitality adjustments across entities.

use crate::entity::Entity;
use crate::guid::Guid;
use crate::plugin::EventQueue;

/// Authoritative outcome of a combat calculation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CombatEffectOutcome {
    pub current_health: u32,
    pub effective_damage: f32,
    pub effective_healing: f32,
    pub died: bool,
}

/// Trait defining a pluggable combat resolution system.
pub trait CombatResolver: Send + Sync {
    /// Resolves combat effect (damage or healing) against a target entity.
    fn resolve_combat_effect(
        &self,
        target_ent: &mut Entity,
        target_guid: Guid,
        caster_guid: Option<Guid>,
        damage: u32,
        healing: u32,
        event_queue: &mut EventQueue,
    ) -> Result<CombatEffectOutcome, &'static str>;
}
