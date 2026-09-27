//! Client-side entity replica and state tracking.

use mmo_schema::client::{delta_flags, EntityDeltaState, EntityStatePacket};

/// Extracts the entity type from the upper 16 bits of a GUID.
pub fn extract_entity_type(guid: u64) -> u32 {
    ((guid >> 48) & 0xFFFF) as u32
}

/// Visual representation of an entity tracked by the client world.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientEntityReplica {
    pub entity_id: u64,
    pub entity_type: u32,
    pub position: (f32, f32, f32),
    pub velocity: (f32, f32, f32),
    pub yaw: f32,
    pub pitch: f32,
    pub health: u32,
    pub max_health: u32,
    pub state_flags: u32,
    pub action_id: u32,
    pub cast_progress: f32,
}

impl ClientEntityReplica {
    /// Creates a new replica with defaults.
    pub fn new(entity_id: u64) -> Self {
        let entity_type = extract_entity_type(entity_id);
        Self {
            entity_id,
            entity_type,
            position: (0.0, 0.0, 0.0),
            velocity: (0.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            health: 100,
            max_health: 100,
            state_flags: 0,
            action_id: 0,
            cast_progress: 0.0,
        }
    }

    /// Creates a replica initialized from an authoritative `EntityStatePacket`.
    pub fn from_entity_state(pkt: &EntityStatePacket) -> Self {
        let entity_type = extract_entity_type(pkt.entity_id);
        Self {
            entity_id: pkt.entity_id,
            entity_type,
            position: (pkt.position.x, pkt.position.y, pkt.position.z),
            velocity: (pkt.velocity.x, pkt.velocity.y, pkt.velocity.z),
            yaw: pkt.yaw,
            pitch: pkt.pitch,
            health: pkt.health,
            max_health: pkt.max_health,
            state_flags: pkt.state_flags,
            action_id: 0,
            cast_progress: 0.0,
        }
    }

    /// Applies a compressed `EntityDeltaState` update from an `EntityDeltaBatchPacket`.
    pub fn apply_delta(&mut self, delta: &EntityDeltaState) {
        let mask = delta.field_mask;

        if mask & delta_flags::MASK_KEYFRAME != 0 {
            self.position = delta.position;
        } else {
            if mask & delta_flags::MASK_DELTA_POS_X != 0 {
                self.position.0 += delta.position.0;
            }
            if mask & delta_flags::MASK_DELTA_POS_Y != 0 {
                self.position.1 += delta.position.1;
            }
            if mask & delta_flags::MASK_DELTA_POS_Z != 0 {
                self.position.2 += delta.position.2;
            }
        }

        if mask & delta_flags::MASK_VELOCITY != 0 {
            self.velocity = delta.velocity;
        }
        if mask & delta_flags::MASK_YAW != 0 {
            self.yaw = delta.yaw;
        }
        if mask & delta_flags::MASK_PITCH != 0 {
            self.pitch = delta.pitch;
        }
        if mask & delta_flags::MASK_HEALTH != 0 {
            self.health = delta.health;
        }
        if mask & delta_flags::MASK_MAX_HEALTH != 0 {
            self.max_health = delta.max_health;
        }
        if mask & delta_flags::MASK_STATE_FLAGS != 0 {
            self.state_flags = delta.state_flags;
        }
        if mask & delta_flags::MASK_ACTION_ID != 0 {
            self.action_id = delta.action_id;
        }
    }

    /// Returns health normalized to 0.0 - 1.0 range.
    pub fn hp_percent(&self) -> f32 {
        if self.max_health > 0 {
            (self.health as f32 / self.max_health as f32).clamp(0.0, 1.0)
        } else {
            1.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replica_delta_application() {
        let mut replica = ClientEntityReplica::new(1);
        replica.position = (100.0, 50.0, 20.0);

        let delta = EntityDeltaState {
            entity_id: 1,
            field_mask: delta_flags::MASK_DELTA_POS_X
                | delta_flags::MASK_DELTA_POS_Z
                | delta_flags::MASK_HEALTH,
            position: (2.5, 0.0, -1.0),
            velocity: (0.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            health: 80,
            max_health: 100,
            state_flags: 0,
            action_id: 0,
        };

        replica.apply_delta(&delta);
        assert!((replica.position.0 - 102.5).abs() < 1e-4);
        assert!((replica.position.1 - 50.0).abs() < 1e-4);
        assert!((replica.position.2 - 19.0).abs() < 1e-4);
        assert_eq!(replica.health, 80);
        assert_eq!(replica.hp_percent(), 0.8);
    }
}
