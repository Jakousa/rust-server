//! Client-side world object replication, state management, and flat render buffer packing.

use std::collections::HashMap;

use mmo_schema::client::{
    WorldObjectDeltaPacket, WorldObjectDespawnPacket, WorldObjectSpawnPacket,
};

/// Replicated client-side representation of a stationary or constructible world object.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientWorldObjectReplica {
    pub object_id: u64,
    pub prefab_id: u32,
    pub position: (f32, f32, f32),
    pub rotation: (f32, f32, f32, f32),
    pub scale: (f32, f32, f32),
    pub bounds_min: (f32, f32, f32),
    pub bounds_max: (f32, f32, f32),
    pub state_flags: u32,
    pub health: Option<(u32, u32)>,
    pub properties: HashMap<String, String>,
    pub revision: u64,
}

impl ClientWorldObjectReplica {
    /// Creates a replica from an incoming `WorldObjectSpawnPacket`.
    pub fn from_spawn_packet(packet: &WorldObjectSpawnPacket) -> Self {
        Self {
            object_id: packet.object_id,
            prefab_id: packet.prefab_id,
            position: (packet.pos.x, packet.pos.y, packet.pos.z),
            rotation: packet.rot,
            scale: (packet.scale.x, packet.scale.y, packet.scale.z),
            bounds_min: (
                packet.bounds_min.x,
                packet.bounds_min.y,
                packet.bounds_min.z,
            ),
            bounds_max: (
                packet.bounds_max.x,
                packet.bounds_max.y,
                packet.bounds_max.z,
            ),
            state_flags: packet.state_flags,
            health: packet.health,
            properties: packet.properties.clone(),
            revision: packet.revision,
        }
    }

    /// Applies an incremental delta packet if the revision is newer.
    pub fn apply_delta(&mut self, delta: &WorldObjectDeltaPacket) -> bool {
        if delta.revision <= self.revision {
            return false;
        }
        self.revision = delta.revision;
        if let Some(flags) = delta.state_flags {
            self.state_flags = flags;
        }
        if let Some(health) = delta.health {
            self.health = Some(health);
        }
        if !delta.properties.is_empty() {
            self.properties.extend(delta.properties.clone());
        }
        true
    }

    /// Returns health percentage as a float in `[0.0, 1.0]`, or `-1.0` if object is indestructible/stateless.
    #[inline]
    pub fn hp_percent(&self) -> f32 {
        match self.health {
            Some((current, max)) if max > 0 => (current as f32 / max as f32).clamp(0.0, 1.0),
            _ => -1.0,
        }
    }
}

/// Client collection of active world objects in the player's Area of Interest.
#[derive(Debug, Clone, Default)]
pub struct WorldObjectStore {
    objects: HashMap<u64, ClientWorldObjectReplica>,
}

impl WorldObjectStore {
    /// Creates an empty `WorldObjectStore`.
    pub fn new() -> Self {
        Self {
            objects: HashMap::new(),
        }
    }

    /// Inserts or replaces a world object from a spawn packet.
    pub fn insert_or_update(&mut self, packet: &WorldObjectSpawnPacket) {
        if let Some(existing) = self.objects.get_mut(&packet.object_id) {
            if packet.revision >= existing.revision {
                *existing = ClientWorldObjectReplica::from_spawn_packet(packet);
            }
        } else {
            self.objects.insert(
                packet.object_id,
                ClientWorldObjectReplica::from_spawn_packet(packet),
            );
        }
    }

    /// Applies a delta packet to an existing object if present.
    pub fn apply_delta(&mut self, delta: &WorldObjectDeltaPacket) -> bool {
        if let Some(obj) = self.objects.get_mut(&delta.object_id) {
            obj.apply_delta(delta)
        } else {
            false
        }
    }

    /// Removes an object on despawn.
    pub fn remove(
        &mut self,
        despawn: &WorldObjectDespawnPacket,
    ) -> Option<ClientWorldObjectReplica> {
        self.objects.remove(&despawn.object_id)
    }

    /// Returns the number of currently active world objects.
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    /// Returns true if empty.
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// Clears all world objects.
    pub fn clear(&mut self) {
        self.objects.clear();
    }

    /// Returns an immutable reference to an object by ID.
    pub fn get(&self, object_id: u64) -> Option<&ClientWorldObjectReplica> {
        self.objects.get(&object_id)
    }

    /// Generates a flat packed `Float32Array` render buffer for WebGL / Three.js instancing.
    ///
    /// Memory layout per world object (16 floats):
    /// `[object_id_lo, prefab_id_f32, pos_x, pos_y, pos_z, rot_x, rot_y, rot_z, rot_w, scale_x, scale_y, scale_z, state_flags_f32, hp_pct, revision_f32, object_id_hi]`
    pub fn get_render_buffer(&self) -> Vec<f32> {
        let mut buffer = Vec::with_capacity(self.objects.len() * 16);
        for obj in self.objects.values() {
            buffer.push((obj.object_id & 0xFFFF_FFFF) as f32);
            buffer.push(obj.prefab_id as f32);
            buffer.push(obj.position.0);
            buffer.push(obj.position.1);
            buffer.push(obj.position.2);
            buffer.push(obj.rotation.0);
            buffer.push(obj.rotation.1);
            buffer.push(obj.rotation.2);
            buffer.push(obj.rotation.3);
            buffer.push(obj.scale.0);
            buffer.push(obj.scale.1);
            buffer.push(obj.scale.2);
            buffer.push(obj.state_flags as f32);
            buffer.push(obj.hp_percent());
            buffer.push(obj.revision as f32);
            buffer.push((obj.object_id >> 32) as f32);
        }
        buffer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_schema::client::Vec3;

    fn sample_spawn(id: u64, rev: u64) -> WorldObjectSpawnPacket {
        WorldObjectSpawnPacket {
            object_id: id,
            prefab_id: 42,
            pos: Vec3::new(10.0, 0.0, 20.0),
            rot: (0.0, 0.0, 0.0, 1.0),
            scale: Vec3::new(1.0, 1.0, 1.0),
            bounds_min: Vec3::new(9.0, 0.0, 19.0),
            bounds_max: Vec3::new(11.0, 2.0, 21.0),
            state_flags: 1,
            owner_type: 0,
            owner_id: "player1".to_string(),
            health: Some((80, 100)),
            properties: HashMap::new(),
            revision: rev,
        }
    }

    #[test]
    fn test_world_object_store_spawn_delta_despawn() {
        let mut store = WorldObjectStore::new();
        assert!(store.is_empty());

        // Spawn
        let spawn = sample_spawn(1001, 1);
        store.insert_or_update(&spawn);
        assert_eq!(store.len(), 1);

        let obj = store.get(1001).unwrap();
        assert_eq!(obj.object_id, 1001);
        assert_eq!(obj.prefab_id, 42);
        assert_eq!(obj.hp_percent(), 0.8);
        assert_eq!(obj.revision, 1);

        // Apply older delta -> ignored
        let stale_delta = WorldObjectDeltaPacket {
            object_id: 1001,
            state_flags: Some(2),
            health: None,
            properties: HashMap::new(),
            revision: 1,
        };
        assert!(!store.apply_delta(&stale_delta));
        assert_eq!(store.get(1001).unwrap().state_flags, 1);

        // Apply newer delta -> updated
        let new_delta = WorldObjectDeltaPacket {
            object_id: 1001,
            state_flags: Some(5),
            health: Some((50, 100)),
            properties: HashMap::new(),
            revision: 2,
        };
        assert!(store.apply_delta(&new_delta));
        let updated = store.get(1001).unwrap();
        assert_eq!(updated.state_flags, 5);
        assert_eq!(updated.hp_percent(), 0.5);
        assert_eq!(updated.revision, 2);

        // Render buffer
        let buf = store.get_render_buffer();
        assert_eq!(buf.len(), 16);
        assert_eq!(buf[0], 1001.0);
        assert_eq!(buf[1], 42.0);
        assert_eq!(buf[2], 10.0);
        assert_eq!(buf[12], 5.0);
        assert_eq!(buf[13], 0.5);
        assert_eq!(buf[14], 2.0);
        assert_eq!(buf[15], 0.0);

        // Despawn
        let despawn = WorldObjectDespawnPacket { object_id: 1001 };
        assert!(store.remove(&despawn).is_some());
        assert_eq!(store.len(), 0);
        assert!(store.is_empty());
    }

    #[test]
    fn test_64bit_object_id_fidelity_in_render_buffer() {
        let mut store = WorldObjectStore::new();
        let id_a = (1u64 << 48) | (1u64 << 32) | 1u64;
        let id_b = (1u64 << 48) | (1u64 << 32) | 2u64;

        store.insert_or_update(&sample_spawn(id_a, 1));
        store.insert_or_update(&sample_spawn(id_b, 1));

        let buf = store.get_render_buffer();
        assert_eq!(buf.len(), 32);

        let reconstructed_0 = ((buf[15] as u64) << 32) | (buf[0] as u64);
        let reconstructed_1 = ((buf[31] as u64) << 32) | (buf[16] as u64);

        let mut ids = vec![reconstructed_0, reconstructed_1];
        ids.sort_unstable();
        assert_eq!(ids, vec![id_a, id_b]);
        assert_ne!(buf[0], buf[16]);
    }
}
