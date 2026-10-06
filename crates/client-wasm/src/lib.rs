//! Rust WebAssembly Client Core for OpenSpec MMO.
//!
//! Provides zero-duplication client-side packet decoding, Track 6 delta decompression,
//! temporal entity smoothing (Hermite / LERP), and flat typed buffer generation for WebGL rendering.

pub mod interp;
pub mod state;
pub mod world;

use std::collections::{HashMap, VecDeque};

use bytes::{Bytes, BytesMut};
use serde::Serialize;
use wasm_bindgen::prelude::*;

use mmo_schema::client::{
    ClientAckPacket, ClientPacket, InteractionPacket, InteractionType, MovementPacket, Vec3,
};

use crate::interp::InterpolationBuffer;
use crate::state::ClientEntityReplica;
use crate::world::WorldObjectStore;

/// Telemetry structure serialized to JSON for developer HUD overlays.
#[derive(Debug, Serialize)]
pub struct ClientTelemetry {
    pub entity_count: usize,
    pub latest_server_tick: u64,
    pub latest_baseline_tick: u64,
    pub received_packets: u64,
    pub client_time: f64,
    pub local_player_id: Option<u64>,
}

/// WebAssembly client world holding entity replicas, interpolation history, and network state.
#[wasm_bindgen]
pub struct WasmClientWorld {
    entities: HashMap<u64, ClientEntityReplica>,
    interp_buffer: InterpolationBuffer,
    world_objects: WorldObjectStore,
    local_player_id: Option<u64>,
    session_id: u64,
    latest_server_tick: u64,
    latest_baseline_tick: u64,
    received_packets: u64,
    client_time: f64,
    incoming_buffer: BytesMut,
    pending_acks: VecDeque<u64>,
}

impl Default for WasmClientWorld {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl WasmClientWorld {
    /// Creates a new `WasmClientWorld`.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            entities: HashMap::new(),
            interp_buffer: InterpolationBuffer::new(0.10),
            world_objects: WorldObjectStore::new(),
            local_player_id: None,
            session_id: 0,
            latest_server_tick: 0,
            latest_baseline_tick: 0,
            received_packets: 0,
            client_time: 0.0,
            incoming_buffer: BytesMut::with_capacity(16384),
            pending_acks: VecDeque::new(),
        }
    }

    /// Sets the assigned session ID from gateway handshake.
    pub fn set_session_id(&mut self, session_id: u64) {
        self.session_id = session_id;
    }

    /// Gets the current session ID.
    pub fn session_id(&self) -> u64 {
        self.session_id
    }

    /// Sets the local player's entity ID (GUID raw).
    pub fn set_local_player_id(&mut self, entity_id: u64) {
        self.local_player_id = Some(entity_id);
    }

    /// Gets the local player's entity ID.
    pub fn local_player_id(&self) -> Option<u64> {
        self.local_player_id
    }

    /// Ingests a raw binary WebTransport datagram or stream chunk.
    ///
    /// Buffers incoming bytes, parses all complete framed `ClientPacket` instances,
    /// and updates internal entity replicas and interpolation queues.
    pub fn process_packet(&mut self, data: &[u8]) -> Result<(), String> {
        self.incoming_buffer.extend_from_slice(data);

        while let Ok(Some(packet)) = ClientPacket::decode_frame(&mut self.incoming_buffer) {
            self.received_packets += 1;
            self.handle_parsed_packet(packet);
        }

        Ok(())
    }

    /// Internal handler dispatching parsed schema packets.
    fn handle_parsed_packet(&mut self, packet: ClientPacket) {
        match packet {
            ClientPacket::EntityDeltaBatch(batch) => {
                self.latest_server_tick = batch.server_tick;
                self.latest_baseline_tick = batch.baseline_tick;

                // Queue server tick receipt for delta baseline acknowledgment
                if !self.pending_acks.contains(&batch.server_tick) {
                    if self.pending_acks.len() >= 128 {
                        self.pending_acks.pop_front();
                    }
                    self.pending_acks.push_back(batch.server_tick);
                }

                // Process despawns included in the batch
                for despawn_id in batch.despawns {
                    self.entities.remove(&despawn_id);
                    self.interp_buffer.remove_entity(despawn_id);
                }

                // Process entity deltas
                for delta in batch.deltas {
                    let entity_id = delta.entity_id;
                    let replica = self
                        .entities
                        .entry(entity_id)
                        .or_insert_with(|| ClientEntityReplica::new(entity_id));

                    replica.apply_delta(&delta);

                    self.interp_buffer.push_snapshot(
                        entity_id,
                        batch.server_tick,
                        self.client_time,
                        replica.position,
                        replica.velocity,
                        replica.yaw,
                    );
                }
            }
            ClientPacket::EntityState(state) => {
                let entity_id = state.entity_id;
                let replica = ClientEntityReplica::from_entity_state(&state);
                let pos = replica.position;
                let vel = replica.velocity;
                let yaw = replica.yaw;

                self.entities.insert(entity_id, replica);
                self.interp_buffer.push_snapshot(
                    entity_id,
                    self.latest_server_tick,
                    self.client_time,
                    pos,
                    vel,
                    yaw,
                );
            }
            ClientPacket::EntityDespawn(despawn) => {
                self.entities.remove(&despawn.entity_id);
                self.interp_buffer.remove_entity(despawn.entity_id);
            }
            ClientPacket::MovementCorrection(correction) => {
                if let Some(local_id) = self.local_player_id {
                    if correction.entity_id == local_id {
                        if let Some(replica) = self.entities.get_mut(&local_id) {
                            replica.position = (
                                correction.corrected_position.x,
                                correction.corrected_position.y,
                                correction.corrected_position.z,
                            );
                            replica.velocity = (
                                correction.corrected_velocity.x,
                                correction.corrected_velocity.y,
                                correction.corrected_velocity.z,
                            );
                        }
                    }
                }
            }
            ClientPacket::Pong(_pong) => {
                // Heartbeat reply acknowledged
            }
            ClientPacket::InteractionResult(_res) => {
                // Action result acknowledged
            }
            ClientPacket::WorldObjectSpawn(spawn) => {
                self.world_objects.insert_or_update(&spawn);
            }
            ClientPacket::WorldObjectDelta(delta) => {
                self.world_objects.apply_delta(&delta);
            }
            ClientPacket::WorldObjectDespawn(despawn) => {
                self.world_objects.remove(&despawn);
            }
            _ => {}
        }
    }

    /// Advances client time by `dt_sec` and smoothly updates entity positions via interpolation.
    pub fn update(&mut self, dt_sec: f32) {
        self.client_time += dt_sec as f64;

        for (entity_id, replica) in self.entities.iter_mut() {
            // Local player is predicted immediately on input; other entities are interpolated
            if Some(*entity_id) == self.local_player_id {
                continue;
            }

            if let Some((smooth_pos, smooth_yaw)) =
                self.interp_buffer.sample(*entity_id, self.client_time)
            {
                replica.position = smooth_pos;
                replica.yaw = smooth_yaw;
            }
        }
    }

    /// Encodes a client movement packet ready for WebTransport datagram transmission.
    pub fn encode_movement(&mut self, x: f32, y: f32, z: f32, yaw: f32, sequence: u32) -> Vec<u8> {
        let entity_id = self.local_player_id.unwrap_or(0);

        if let Some(replica) = self.entities.get_mut(&entity_id) {
            replica.position = (x, y, z);
            replica.yaw = yaw;
        }

        let pkt = ClientPacket::Movement(MovementPacket {
            entity_id,
            sequence,
            position: Vec3 { x, y, z },
            velocity: Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            yaw,
            pitch: 0.0,
            timestamp: (self.client_time * 1000.0) as u64,
        });

        pkt.encode()
    }

    /// Serializes an outgoing `ClientAckPacket` acknowledging the latest received server tick.
    pub fn encode_ack(&self) -> Option<Vec<u8>> {
        if self.latest_server_tick == 0 {
            return None;
        }

        let pkt = ClientPacket::ClientAck(ClientAckPacket {
            session_id: self.session_id,
            observed_tick: self.latest_server_tick,
            ack_mask: 0,
        });

        Some(pkt.encode())
    }

    /// Drains the oldest pending ACK packet for WASM JS transmission.
    pub fn drain_ack_packet(&mut self) -> Option<Vec<u8>> {
        let tick = self.pending_acks.pop_front()?;
        let pkt = ClientPacket::ClientAck(ClientAckPacket {
            session_id: self.session_id,
            observed_tick: tick,
            ack_mask: 0,
        });
        Some(pkt.encode())
    }

    /// Returns the number of currently queued unacknowledged server ticks for JS/WASM.
    pub fn pending_acks_len(&self) -> usize {
        self.pending_acks.len()
    }

    /// Retrieves a scalar attribute value for a given entity, if present.
    pub fn get_entity_attribute(&self, entity_id: u64, attribute_id: u16) -> Option<u32> {
        self.entities
            .get(&entity_id)
            .and_then(|r| r.get_attribute(attribute_id))
    }

    /// Encodes an action or ability cast interaction for reliable stream transmission.
    pub fn encode_interaction(&mut self, target_guid: u64, action_id: u32) -> Vec<u8> {
        let source_guid = self.local_player_id.unwrap_or(0);

        let pkt = ClientPacket::Interaction(InteractionPacket {
            source_guid,
            target_guid,
            interaction_type: InteractionType::Attack,
            action_id,
            payload: Bytes::new(),
        });

        pkt.encode()
    }

    /// Returns a flat `Float32Array` render buffer for high-performance WebGL rendering.
    ///
    /// Memory layout per entity (8 floats):
    /// `[guid_raw_f32, x, y, z, yaw, hp_pct, cast_pct, entity_type_f32]`
    pub fn get_render_buffer(&self) -> Vec<f32> {
        let mut buffer = Vec::with_capacity(self.entities.len() * 8);

        for replica in self.entities.values() {
            buffer.push(replica.entity_id as f32);
            buffer.push(replica.position.0);
            buffer.push(replica.position.1);
            buffer.push(replica.position.2);
            buffer.push(replica.yaw);
            buffer.push(replica.hp_percent());
            buffer.push(replica.cast_progress);
            buffer.push(replica.entity_type as f32);
        }

        buffer
    }

    /// Returns the number of currently active world objects in client memory.
    pub fn world_objects_count(&self) -> usize {
        self.world_objects.len()
    }

    /// Returns a flat Float32Array render buffer for all active world objects (16 floats per object).
    ///
    /// Memory layout per world object (16 floats):
    /// `[object_id_f32, prefab_id_f32, pos_x, pos_y, pos_z, rot_x, rot_y, rot_z, rot_w, scale_x, scale_y, scale_z, state_flags_f32, hp_pct, revision_f32, 0.0]`
    pub fn get_world_objects_render_buffer(&self) -> Vec<f32> {
        self.world_objects.get_render_buffer()
    }

    /// Encodes a client `PlaceObjectPacket` ready for transmission over reliable stream.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_place_object(
        &self,
        prefab_id: u32,
        x: f32,
        y: f32,
        z: f32,
        qx: f32,
        qy: f32,
        qz: f32,
        qw: f32,
        sx: f32,
        sy: f32,
        sz: f32,
    ) -> Vec<u8> {
        let pkt = ClientPacket::PlaceObject(mmo_schema::client::PlaceObjectPacket {
            prefab_id,
            pos: Vec3::new(x, y, z),
            rot: (qx, qy, qz, qw),
            scale: Vec3::new(sx, sy, sz),
            properties: HashMap::new(),
        });
        pkt.encode()
    }

    /// Encodes a client `InteractObjectPacket` for transmission over reliable stream.
    pub fn encode_interact_object(&self, object_id: u64, action_type: u32) -> Vec<u8> {
        let pkt = ClientPacket::InteractObject(mmo_schema::client::InteractObjectPacket {
            object_id,
            action_type,
        });
        pkt.encode()
    }

    /// Encodes a client `WorldObjectResyncPacket` requesting full object state resynchronization.
    pub fn encode_world_object_resync(&self, object_id: u64) -> Vec<u8> {
        let pkt = ClientPacket::WorldObjectResync(mmo_schema::client::WorldObjectResyncPacket {
            object_id,
        });
        pkt.encode()
    }

    /// Returns telemetry metrics formatted as a JSON string for the HUD.
    pub fn get_telemetry_json(&self) -> String {
        let telemetry = ClientTelemetry {
            entity_count: self.entities.len(),
            latest_server_tick: self.latest_server_tick,
            latest_baseline_tick: self.latest_baseline_tick,
            received_packets: self.received_packets,
            client_time: self.client_time,
            local_player_id: self.local_player_id,
        };

        serde_json::to_string(&telemetry).unwrap_or_else(|_| "{}".to_string())
    }
}

impl WasmClientWorld {
    /// Drains all pending unacknowledged tick receipts and returns their serialized binary `ClientAckPacket`s.
    pub fn drain_pending_acks(&mut self) -> Vec<Vec<u8>> {
        let mut packets = Vec::with_capacity(self.pending_acks.len());
        for tick in self.pending_acks.drain(..) {
            let pkt = ClientPacket::ClientAck(ClientAckPacket {
                session_id: self.session_id,
                observed_tick: tick,
                ack_mask: 0,
            });
            packets.push(pkt.encode());
        }
        packets
    }

    /// Returns the number of currently queued unacknowledged server ticks.
    pub fn pending_acks_count(&self) -> usize {
        self.pending_acks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_schema::client::{
        delta_flags, EntityDeltaBatchPacket, EntityDeltaState, EntityDespawnPacket,
    };

    #[test]
    fn test_wasm_client_world_lifecycle_and_render_buffer() {
        let mut world = WasmClientWorld::new();
        world.set_session_id(77);

        let player_guid = (1u64 << 48) | 100;
        world.set_local_player_id(player_guid);

        // Ingest state packet for mob
        let mob_guid = (2u64 << 48) | 200;
        let state = ClientPacket::EntityState(mmo_schema::client::EntityStatePacket {
            entity_id: mob_guid,
            position: Vec3 {
                x: 10.0,
                y: 0.0,
                z: 20.0,
            },
            velocity: Vec3 {
                x: 1.0,
                y: 0.0,
                z: 0.0,
            },
            yaw: 1.57,
            pitch: 0.0,
            health: 100,
            max_health: 100,
            state_flags: 0,
            timestamp: 1000,
        });

        let state_bytes = state.encode();
        world
            .process_packet(&state_bytes)
            .expect("should process state");

        // Ingest delta packet updating mob
        let delta = ClientPacket::EntityDeltaBatch(EntityDeltaBatchPacket {
            server_tick: 2,
            baseline_tick: 1,
            despawns: Vec::new(),
            deltas: vec![EntityDeltaState {
                entity_id: mob_guid,
                field_mask: delta_flags::MASK_DELTA_POS_X | delta_flags::MASK_HEALTH,
                position: (5.0, 0.0, 0.0),
                velocity: (0.0, 0.0, 0.0),
                yaw: 0.0,
                pitch: 0.0,
                health: 75,
                max_health: 100,
                state_flags: 0,
                action_id: 0,
                attributes: Vec::new(),
            }],
        });

        let delta_bytes = delta.encode();
        world
            .process_packet(&delta_bytes)
            .expect("should process delta");

        // Check telemetry
        let tele_json = world.get_telemetry_json();
        assert!(tele_json.contains("\"entity_count\":1"));
        assert!(tele_json.contains("\"latest_server_tick\":2"));

        // Check render buffer layout
        let render_buf = world.get_render_buffer();
        assert_eq!(render_buf.len(), 8);
        assert_eq!(render_buf[0], mob_guid as f32);
        assert_eq!(render_buf[5], 0.75); // 75 / 100 HP

        // Check movement encoding
        let move_bytes = world.encode_movement(12.0, 0.0, 15.0, std::f32::consts::PI, 1);
        assert!(!move_bytes.is_empty());

        // Check ACK queue and encoding
        assert_eq!(world.pending_acks_count(), 1);
        let drained_acks = world.drain_pending_acks();
        assert_eq!(drained_acks.len(), 1);
        assert_eq!(world.pending_acks_count(), 0);

        let ack_bytes = world.encode_ack().expect("should encode ack");
        assert!(!ack_bytes.is_empty());

        // Check Interaction encoding
        let interact_bytes = world.encode_interaction(mob_guid, 42);
        assert!(!interact_bytes.is_empty());

        // Despawn mob
        let despawn = ClientPacket::EntityDespawn(EntityDespawnPacket {
            entity_id: mob_guid,
        });
        world
            .process_packet(&despawn.encode())
            .expect("should process despawn");
        assert_eq!(world.get_render_buffer().len(), 0);
    }

    #[test]
    fn test_wasm_client_world_world_objects_lifecycle() {
        use mmo_schema::client::{
            WorldObjectDeltaPacket, WorldObjectDespawnPacket, WorldObjectSpawnPacket,
        };

        let mut world = WasmClientWorld::new();
        assert_eq!(world.world_objects_count(), 0);

        // 1. Process WorldObjectSpawn
        let spawn = ClientPacket::WorldObjectSpawn(WorldObjectSpawnPacket {
            object_id: 5001,
            prefab_id: 10,
            pos: Vec3::new(25.0, 0.0, 30.0),
            rot: (0.0, 0.0, 0.0, 1.0),
            scale: Vec3::new(2.0, 2.0, 2.0),
            bounds_min: Vec3::new(24.0, 0.0, 29.0),
            bounds_max: Vec3::new(26.0, 4.0, 31.0),
            state_flags: 1,
            owner_type: 0,
            owner_id: "hero".to_string(),
            health: Some((100, 100)),
            properties: HashMap::new(),
            revision: 1,
        });

        world
            .process_packet(&spawn.encode())
            .expect("should process spawn");
        assert_eq!(world.world_objects_count(), 1);

        let buf = world.get_world_objects_render_buffer();
        assert_eq!(buf.len(), 16);
        assert_eq!(buf[0], 5001.0);
        assert_eq!(buf[1], 10.0);
        assert_eq!(buf[2], 25.0);
        assert_eq!(buf[13], 1.0); // 100 / 100 HP

        // 2. Process WorldObjectDelta
        let delta = ClientPacket::WorldObjectDelta(WorldObjectDeltaPacket {
            object_id: 5001,
            revision: 2,
            state_flags: Some(3),
            health: Some((50, 100)),
            properties: HashMap::new(),
        });
        world
            .process_packet(&delta.encode())
            .expect("should process delta");
        let buf_v2 = world.get_world_objects_render_buffer();
        assert_eq!(buf_v2[12], 3.0); // state_flags
        assert_eq!(buf_v2[13], 0.5); // 50 / 100 HP
        assert_eq!(buf_v2[14], 2.0); // revision

        // 3. Encode place, interact, and resync packets
        let place_bytes =
            world.encode_place_object(10, 25.0, 0.0, 30.0, 0.0, 0.0, 0.0, 1.0, 2.0, 2.0, 2.0);
        assert!(!place_bytes.is_empty());

        let interact_bytes = world.encode_interact_object(5001, 1);
        assert!(!interact_bytes.is_empty());

        let resync_bytes = world.encode_world_object_resync(5001);
        assert!(!resync_bytes.is_empty());

        // 4. Process WorldObjectDespawn
        let despawn =
            ClientPacket::WorldObjectDespawn(WorldObjectDespawnPacket { object_id: 5001 });
        world
            .process_packet(&despawn.encode())
            .expect("should process despawn");
        assert_eq!(world.world_objects_count(), 0);
        assert_eq!(world.get_world_objects_render_buffer().len(), 0);
    }

    #[test]
    fn test_wasm_client_world_auto_ack_and_scalar_attributes() {
        use mmo_schema::client::ScalarAttributeDelta;

        let mut world = WasmClientWorld::new();
        world.set_session_id(777);

        assert_eq!(world.pending_acks_count(), 0);
        assert_eq!(world.pending_acks_len(), 0);

        // Receive batch 1 at tick 10
        let batch1 = ClientPacket::EntityDeltaBatch(EntityDeltaBatchPacket {
            server_tick: 10,
            baseline_tick: 0,
            despawns: Vec::new(),
            deltas: vec![EntityDeltaState {
                entity_id: 100,
                field_mask: delta_flags::MASK_ATTRIBUTES,
                position: (0.0, 0.0, 0.0),
                velocity: (0.0, 0.0, 0.0),
                yaw: 0.0,
                pitch: 0.0,
                health: 0,
                max_health: 0,
                state_flags: 0,
                action_id: 0,
                attributes: vec![
                    ScalarAttributeDelta {
                        attribute_id: 5,
                        value: 42,
                    },
                    ScalarAttributeDelta {
                        attribute_id: 0,
                        value: 95,
                    },
                ],
            }],
        });
        world
            .process_packet(&batch1.encode())
            .expect("process batch 1");

        assert_eq!(world.pending_acks_count(), 1);
        assert_eq!(world.get_entity_attribute(100, 5), Some(42));
        assert_eq!(world.get_entity_attribute(100, 0), Some(95));
        assert_eq!(world.get_entity_attribute(100, 99), None);

        // Receive batch 2 at tick 11
        let batch2 = ClientPacket::EntityDeltaBatch(EntityDeltaBatchPacket {
            server_tick: 11,
            baseline_tick: 10,
            despawns: Vec::new(),
            deltas: vec![EntityDeltaState {
                entity_id: 100,
                field_mask: delta_flags::MASK_ATTRIBUTES,
                position: (0.0, 0.0, 0.0),
                velocity: (0.0, 0.0, 0.0),
                yaw: 0.0,
                pitch: 0.0,
                health: 0,
                max_health: 0,
                state_flags: 0,
                action_id: 0,
                attributes: vec![ScalarAttributeDelta {
                    attribute_id: 5,
                    value: 84,
                }],
            }],
        });
        world
            .process_packet(&batch2.encode())
            .expect("process batch 2");

        assert_eq!(world.pending_acks_count(), 2);
        assert_eq!(world.get_entity_attribute(100, 5), Some(84));

        // Drain first ack via drain_ack_packet
        let ack1_bytes = world.drain_ack_packet().expect("must have ack 1");
        assert_eq!(world.pending_acks_len(), 1);
        let mut buf1 = bytes::BytesMut::from(&ack1_bytes[..]);
        if let Ok(Some(ClientPacket::ClientAck(ack1))) = ClientPacket::decode_frame(&mut buf1) {
            assert_eq!(ack1.session_id, 777);
            assert_eq!(ack1.observed_tick, 10);
        } else {
            panic!("Expected ClientAckPacket for tick 10");
        }

        // Drain remaining acks via drain_pending_acks
        let remaining_acks = world.drain_pending_acks();
        assert_eq!(remaining_acks.len(), 1);
        assert_eq!(world.pending_acks_count(), 0);

        let mut buf2 = bytes::BytesMut::from(&remaining_acks[0][..]);
        if let Ok(Some(ClientPacket::ClientAck(ack2))) = ClientPacket::decode_frame(&mut buf2) {
            assert_eq!(ack2.session_id, 777);
            assert_eq!(ack2.observed_tick, 11);
        } else {
            panic!("Expected ClientAckPacket for tick 11");
        }
    }
}
