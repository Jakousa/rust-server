//! Rust WebAssembly Client Core for OpenSpec MMO.
//!
//! Provides zero-duplication client-side packet decoding, Track 6 delta decompression,
//! temporal entity smoothing (Hermite / LERP), and flat typed buffer generation for WebGL rendering.

pub mod interp;
pub mod prediction;
pub mod state;
pub mod voxel;
pub mod world;

use std::collections::{HashMap, VecDeque};

use bytes::{Bytes, BytesMut};
use serde::Serialize;
use wasm_bindgen::prelude::*;

use mmo_schema::client::{
    delta_flags, ClientAckPacket, ClientPacket, InteractionPacket, InteractionType, MovementPacket,
    Vec3,
};

use crate::interp::InterpolationBuffer;
use crate::state::ClientEntityReplica;
use crate::world::WorldObjectStore;
pub use prediction::{ClientInput, PredictionBuffer};

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
    prediction_buffer: PredictionBuffer,
    world_objects: WorldObjectStore,
    local_player_id: Option<u64>,
    session_id: u64,
    latest_server_tick: u64,
    latest_baseline_tick: u64,
    received_packets: u64,
    client_time: f64,
    incoming_buffer: BytesMut,
    pending_acks: VecDeque<u64>,
    last_interaction_result: Option<String>,
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
            prediction_buffer: PredictionBuffer::new(),
            world_objects: WorldObjectStore::new(),
            local_player_id: None,
            session_id: 0,
            latest_server_tick: 0,
            latest_baseline_tick: 0,
            received_packets: 0,
            client_time: 0.0,
            incoming_buffer: BytesMut::with_capacity(16384),
            pending_acks: VecDeque::new(),
            last_interaction_result: None,
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

        loop {
            match ClientPacket::decode_frame(&mut self.incoming_buffer) {
                Ok(Some(packet)) => {
                    self.received_packets += 1;
                    self.handle_parsed_packet(packet);
                }
                Ok(None) => break Ok(()),
                Err(err) => {
                    self.incoming_buffer.clear();
                    return Err(err.to_string());
                }
            }
        }
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

                    if Some(entity_id) == self.local_player_id {
                        let server_pos = replica.position;
                        if self.prediction_buffer.pending_input_count() == 0 {
                            if delta.field_mask & delta_flags::MASK_KEYFRAME != 0
                                || delta.field_mask & delta_flags::MASK_DELTA_POS_X != 0
                            {
                                let reconciled = self.prediction_buffer.reconcile(0, server_pos);
                                replica.position =
                                    self.prediction_buffer.visual_position(reconciled);
                            }
                        } else {
                            let (cx, cy, cz) = self.prediction_buffer.current_sim_pos;
                            let (sx, sy, sz) = server_pos;
                            let dx = cx - sx;
                            let dy = cy - sy;
                            let dz = cz - sz;
                            let dist_sq = dx * dx + dy * dy + dz * dz;
                            let hard_snap_thresh = self.prediction_buffer.hard_snap_threshold;
                            if dist_sq > hard_snap_thresh * hard_snap_thresh {
                                let reconciled = self.prediction_buffer.reconcile(0, server_pos);
                                replica.position =
                                    self.prediction_buffer.visual_position(reconciled);
                            } else {
                                replica.position = self
                                    .prediction_buffer
                                    .visual_position(self.prediction_buffer.current_sim_pos);
                            }
                        }
                    } else {
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
                        let server_pos = (
                            correction.corrected_position.x,
                            correction.corrected_position.y,
                            correction.corrected_position.z,
                        );
                        let reconciled_pos = self
                            .prediction_buffer
                            .reconcile(correction.sequence, server_pos);
                        if let Some(replica) = self.entities.get_mut(&local_id) {
                            replica.position =
                                self.prediction_buffer.visual_position(reconciled_pos);
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
            ClientPacket::AuthResponse(resp) => {
                if resp.success {
                    self.session_id = resp.session_id;
                    self.local_player_id = Some(resp.player_guid);
                }
            }
            ClientPacket::InteractionResult(res) => {
                let payload_str = if res.payload.is_empty() {
                    serde_json::to_string(&serde_json::json!({
                        "action_id": res.action_id,
                        "success": res.success,
                        "error_code": res.error_code,
                    }))
                    .unwrap_or_default()
                } else {
                    match String::from_utf8(res.payload) {
                        Ok(s) => s,
                        Err(e) => String::from_utf8_lossy(&e.into_bytes()).into_owned(),
                    }
                };
                self.last_interaction_result = Some(payload_str);
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
        self.prediction_buffer.update(dt_sec);

        // Update local player visual position with decayed error offset
        if let Some(local_id) = self.local_player_id {
            if let Some(replica) = self.entities.get_mut(&local_id) {
                replica.position = self
                    .prediction_buffer
                    .visual_position(self.prediction_buffer.current_sim_pos);
            }
        }

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

        let prev_sim = self.prediction_buffer.current_sim_pos;
        let displacement = (x - prev_sim.0, y - prev_sim.1, z - prev_sim.2);
        self.prediction_buffer.push_input(
            sequence,
            (x, y, z),
            displacement,
            0.05,
            self.client_time,
        );

        if let Some(replica) = self.entities.get_mut(&entity_id) {
            replica.position = self.prediction_buffer.visual_position((x, y, z));
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

    /// Encodes a custom interaction with arbitrary payload bytes (e.g. Village NPC barter or player trade).
    pub fn encode_custom_interaction(
        &self,
        target_guid: u64,
        action_id: u32,
        payload: &[u8],
    ) -> Vec<u8> {
        let source_guid = self.local_player_id.unwrap_or(0);

        let pkt = ClientPacket::Interaction(InteractionPacket {
            source_guid,
            target_guid,
            interaction_type: InteractionType::Custom(0),
            action_id,
            payload: Bytes::copy_from_slice(payload),
        });

        pkt.encode()
    }

    /// Returns and clears the latest InteractionResult payload (JSON string), if available.
    pub fn take_last_interaction_result(&mut self) -> Option<String> {
        self.last_interaction_result.take()
    }

    /// Returns a flat `Float32Array` render buffer for high-performance WebGL rendering.
    ///
    /// Memory layout per entity (9 floats):
    /// `[guid_lo_f32, guid_hi_f32, x, y, z, yaw, hp_pct, cast_pct, entity_type_f32]`
    pub fn get_render_buffer(&self) -> Vec<f32> {
        let mut buffer = Vec::with_capacity(self.entities.len() * 9);

        for replica in self.entities.values() {
            buffer.push((replica.entity_id & 0xFFFF_FFFF) as f32);
            buffer.push((replica.entity_id >> 32) as f32);
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

    /// Returns all entity 64-bit GUIDs currently active in client memory.
    pub fn get_entity_guids(&self) -> Vec<u64> {
        self.entities.keys().copied().collect()
    }

    /// Returns the number of currently active entities in client memory.
    pub fn entities_count(&self) -> usize {
        self.entities.len()
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

    /// Returns a reference to the client prediction buffer.
    pub fn prediction_buffer(&self) -> &PredictionBuffer {
        &self.prediction_buffer
    }

    /// Returns a mutable reference to the client prediction buffer.
    pub fn prediction_buffer_mut(&mut self) -> &mut PredictionBuffer {
        &mut self.prediction_buffer
    }

    /// Returns the current (x, y, z) position of an entity, if tracked.
    pub fn get_entity_position(&self, entity_id: u64) -> Option<(f32, f32, f32)> {
        self.entities.get(&entity_id).map(|r| r.position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_schema::client::{
        delta_flags, EntityDeltaBatchPacket, EntityDeltaState, EntityDespawnPacket,
    };
    use proptest::prelude::*;

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
        assert_eq!(render_buf.len(), 9);
        assert_eq!(render_buf[0], (mob_guid & 0xFFFF_FFFF) as f32);
        assert_eq!(render_buf[1], (mob_guid >> 32) as f32);
        assert_eq!(render_buf[6], 0.75); // 75 / 100 HP

        // Check movement encoding
        let move_bytes = world.encode_movement(12.0, 0.0, 15.0, std::f32::consts::PI, 1);
        assert!(!move_bytes.is_empty());

        // Check ACK queue and encoding
        assert_eq!(world.pending_acks_len(), 1);
        let drained_acks = world.drain_pending_acks();
        assert_eq!(drained_acks.len(), 1);
        assert_eq!(world.pending_acks_len(), 0);

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
    fn test_wasm_client_world_auth_response_and_interaction_result() {
        use mmo_schema::client::{AuthResponsePacket, InteractionResultPacket};

        let mut world = WasmClientWorld::new();
        assert_eq!(world.session_id(), 0);
        assert_eq!(world.local_player_id(), None);
        assert_eq!(world.take_last_interaction_result(), None);

        // 1. Process AuthResponse
        let auth_resp = ClientPacket::AuthResponse(AuthResponsePacket {
            success: true,
            error_code: 0,
            session_id: 1234,
            player_guid: 987654321,
            assigned_node: "127.0.0.1:50052".to_string(),
            error_message: String::new(),
        });
        world.process_packet(&auth_resp.encode()).unwrap();

        assert_eq!(world.session_id(), 1234);
        assert_eq!(world.local_player_id(), Some(987654321));

        // 2. Encode custom interaction
        let custom_payload = b"{\"recipe_id\":2}";
        let action_id = (0x0002 << 16) | 2;
        let encoded = world.encode_custom_interaction(555, action_id, custom_payload);
        assert!(!encoded.is_empty());

        let mut decode_buf = bytes::BytesMut::from(&encoded[..]);
        if let Ok(Some(ClientPacket::Interaction(pkt))) =
            ClientPacket::decode_frame(&mut decode_buf)
        {
            assert_eq!(pkt.source_guid, 987654321);
            assert_eq!(pkt.target_guid, 555);
            assert_eq!(pkt.action_id, action_id);
            assert_eq!(&pkt.payload[..], custom_payload);
        } else {
            panic!("Expected decoded Interaction packet");
        }

        // 3. Process InteractionResult
        let json_result = "{\"status\":\"success\",\"gold\":65}";
        let interact_res = ClientPacket::InteractionResult(InteractionResultPacket {
            action_id,
            success: true,
            error_code: 0,
            payload: json_result.as_bytes().to_vec(),
        });
        world.process_packet(&interact_res.encode()).unwrap();

        let taken = world.take_last_interaction_result();
        assert_eq!(taken, Some(json_result.to_string()));
        // Subsequent take returns None (drained)
        assert_eq!(world.take_last_interaction_result(), None);
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

        assert_eq!(world.pending_acks_len(), 1);
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

        assert_eq!(world.pending_acks_len(), 2);
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
        assert_eq!(world.pending_acks_len(), 0);

        let mut buf2 = bytes::BytesMut::from(&remaining_acks[0][..]);
        if let Ok(Some(ClientPacket::ClientAck(ack2))) = ClientPacket::decode_frame(&mut buf2) {
            assert_eq!(ack2.session_id, 777);
            assert_eq!(ack2.observed_tick, 11);
        } else {
            panic!("Expected ClientAckPacket for tick 11");
        }
    }

    #[test]
    fn test_client_movement_prediction_and_reconciliation() {
        use mmo_schema::client::{MovementCorrectionPacket, MovementCorrectionReason};

        let mut world = WasmClientWorld::new();
        let local_id = 42;
        world.set_local_player_id(local_id);

        let state = ClientPacket::EntityState(mmo_schema::client::EntityStatePacket {
            entity_id: local_id,
            position: Vec3::new(0.0, 0.0, 0.0),
            velocity: Vec3::new(0.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            health: 100,
            max_health: 100,
            state_flags: 0,
            timestamp: 0,
        });
        world.process_packet(&state.encode()).unwrap();

        // Client predicts steps 1, 2, 3
        let _ = world.encode_movement(1.0, 0.0, 0.0, 0.0, 1);
        let _ = world.encode_movement(2.0, 0.0, 0.0, 0.0, 2);
        let _ = world.encode_movement(3.0, 0.0, 0.0, 0.0, 3);

        assert_eq!(world.prediction_buffer.unacknowledged_inputs.len(), 3);
        assert_eq!(world.get_entity_position(local_id), Some((3.0, 0.0, 0.0)));

        // Server sends correction for sequence 1 at (0.5, 0.0, 0.0) -> soft divergence
        let correction = ClientPacket::MovementCorrection(MovementCorrectionPacket {
            entity_id: local_id,
            corrected_position: Vec3::new(0.5, 0.0, 0.0),
            corrected_velocity: Vec3::new(0.0, 0.0, 0.0),
            sequence: 1,
            reason: MovementCorrectionReason::Reconciliation.into(),
        });

        world
            .process_packet(&correction.encode())
            .expect("process correction");

        // 1 acknowledged, 2 remaining in queue
        assert_eq!(world.prediction_buffer.unacknowledged_inputs.len(), 2);
        // Simulation position replayed: 0.5 + 1.0 + 1.0 = 2.5
        assert!((world.prediction_buffer.current_sim_pos.0 - 2.5).abs() < 1e-4);

        // Visual smoothing: previous visual was 3.0, new sim is 2.5 -> error offset +0.5
        assert!((world.prediction_buffer.visual_error_offset.0 - 0.5).abs() < 1e-4);

        // Update with dt to smoothly bleed out error offset
        world.update(0.10);
        assert!(world.prediction_buffer.visual_error_offset.0 < 0.5);
        assert!(world.prediction_buffer.visual_error_offset.0 > 0.0);

        // Fully decay error offset
        world.update(1.0);
        assert_eq!(world.prediction_buffer.visual_error_offset.0, 0.0);
        assert_eq!(world.get_entity_position(local_id), Some((2.5, 0.0, 0.0)));
    }

    #[test]
    fn test_client_movement_correction_hard_snap() {
        use mmo_schema::client::{MovementCorrectionPacket, MovementCorrectionReason};

        let mut world = WasmClientWorld::new();
        let local_id = 42;
        world.set_local_player_id(local_id);

        let state = ClientPacket::EntityState(mmo_schema::client::EntityStatePacket {
            entity_id: local_id,
            position: Vec3::new(0.0, 0.0, 0.0),
            velocity: Vec3::new(0.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            health: 100,
            max_health: 100,
            state_flags: 0,
            timestamp: 0,
        });
        world.process_packet(&state.encode()).unwrap();

        let _ = world.encode_movement(1.0, 0.0, 0.0, 0.0, 1);
        let _ = world.encode_movement(2.0, 0.0, 0.0, 0.0, 2);

        // Severe teleport rollback: server reports (100.0, 0.0, 0.0)
        let correction = ClientPacket::MovementCorrection(MovementCorrectionPacket {
            entity_id: local_id,
            corrected_position: Vec3::new(100.0, 0.0, 0.0),
            corrected_velocity: Vec3::new(0.0, 0.0, 0.0),
            sequence: 1,
            reason: MovementCorrectionReason::KinematicViolation.into(),
        });

        world
            .process_packet(&correction.encode())
            .expect("process correction");

        // Hard snap: unacknowledged inputs cleared, visual error offset zeroed
        assert!(world.prediction_buffer.unacknowledged_inputs.is_empty());
        assert_eq!(world.prediction_buffer.visual_error_offset, (0.0, 0.0, 0.0));
        assert_eq!(world.get_entity_position(local_id), Some((100.0, 0.0, 0.0)));
    }

    #[test]
    fn test_64bit_guid_fidelity_in_render_buffer() {
        let mut world = WasmClientWorld::new();
        let guid_1 = (1u64 << 48) | (1u64 << 32) | 1u64;
        let guid_2 = (1u64 << 48) | (1u64 << 32) | 2u64;

        let state_1 = ClientPacket::EntityState(mmo_schema::client::EntityStatePacket {
            entity_id: guid_1,
            position: Vec3::new(1.0, 2.0, 3.0),
            velocity: Vec3::new(0.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            health: 100,
            max_health: 100,
            state_flags: 0,
            timestamp: 0,
        });
        let state_2 = ClientPacket::EntityState(mmo_schema::client::EntityStatePacket {
            entity_id: guid_2,
            position: Vec3::new(4.0, 5.0, 6.0),
            velocity: Vec3::new(0.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            health: 100,
            max_health: 100,
            state_flags: 0,
            timestamp: 0,
        });

        world.process_packet(&state_1.encode()).unwrap();
        world.process_packet(&state_2.encode()).unwrap();

        let buf = world.get_render_buffer();
        assert_eq!(buf.len(), 18); // 2 entities * 9 floats

        let guids = world.get_entity_guids();
        assert_eq!(guids.len(), 2);
        assert!(guids.contains(&guid_1));
        assert!(guids.contains(&guid_2));

        // Reconstruct both 64-bit GUIDs from render buffer
        let rec_0 = ((buf[1] as u64) << 32) | (buf[0] as u64);
        let rec_1 = ((buf[10] as u64) << 32) | (buf[9] as u64);

        let mut rec_ids = vec![rec_0, rec_1];
        rec_ids.sort_unstable();
        let mut expected_ids = vec![guid_1, guid_2];
        expected_ids.sort_unstable();
        assert_eq!(rec_ids, expected_ids);

        // Lower halves in index 0 and 9 MUST NOT be equal (counter 1 vs 2)
        assert_ne!(buf[0], buf[9]);
    }

    #[test]
    fn test_malformed_packet_clears_buffer_and_returns_err() {
        let mut world = WasmClientWorld::new();

        // Feed corrupted packet bytes with valid magic but unknown opcode (8 bytes header)
        let corrupted = vec![0x4D, 0x4D, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00];
        let res = world.process_packet(&corrupted);
        assert!(res.is_err(), "Expected malformed packet to return Err");
        assert!(
            world.incoming_buffer.is_empty(),
            "Buffer must be cleared on decode error"
        );

        // Subsequent valid packet should process cleanly without being blocked
        let valid_state = ClientPacket::EntityState(mmo_schema::client::EntityStatePacket {
            entity_id: 12345,
            position: Vec3::new(10.0, 20.0, 30.0),
            velocity: Vec3::new(0.0, 0.0, 0.0),
            yaw: 1.0,
            pitch: 0.0,
            health: 50,
            max_health: 100,
            state_flags: 0,
            timestamp: 100,
        });
        assert!(world.process_packet(&valid_state.encode()).is_ok());
        assert_eq!(world.entities_count(), 1);
    }

    #[test]
    fn test_keyframe_arrival_during_active_prediction() {
        let mut world = WasmClientWorld::new();
        let local_id = 999;
        world.set_local_player_id(local_id);

        let init_state = ClientPacket::EntityState(mmo_schema::client::EntityStatePacket {
            entity_id: local_id,
            position: Vec3::new(0.0, 0.0, 0.0),
            velocity: Vec3::new(0.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            health: 100,
            max_health: 100,
            state_flags: 0,
            timestamp: 0,
        });
        world.process_packet(&init_state.encode()).unwrap();

        // Push client predicted movement
        let _ = world.encode_movement(5.0, 0.0, 0.0, 0.0, 1);
        assert_eq!(world.prediction_buffer.pending_input_count(), 1);

        // Keyframe arrives from server with slight latency lag: (4.0, 0.0, 0.0)
        let keyframe_batch = ClientPacket::EntityDeltaBatch(EntityDeltaBatchPacket {
            server_tick: 5,
            baseline_tick: 0,
            despawns: Vec::new(),
            deltas: vec![EntityDeltaState {
                entity_id: local_id,
                field_mask: delta_flags::MASK_KEYFRAME | delta_flags::MASK_DELTA_POS_X,
                position: (4.0, 0.0, 0.0),
                velocity: (0.0, 0.0, 0.0),
                yaw: 0.0,
                pitch: 0.0,
                health: 100,
                max_health: 100,
                state_flags: 0,
                action_id: 0,
                attributes: Vec::new(),
            }],
        });

        world.process_packet(&keyframe_batch.encode()).unwrap();

        // Active prediction must NOT be wiped by unsequenced keyframe when pending inputs exist within hard snap threshold
        assert_eq!(world.prediction_buffer.pending_input_count(), 1);
        assert_eq!(world.get_entity_position(local_id), Some((5.0, 0.0, 0.0)));

        // Now test keyframe exceeding hard snap threshold (20m): server teleports player to (50.0, 0.0, 0.0)
        let snap_keyframe_batch = ClientPacket::EntityDeltaBatch(EntityDeltaBatchPacket {
            server_tick: 6,
            baseline_tick: 5,
            despawns: Vec::new(),
            deltas: vec![EntityDeltaState {
                entity_id: local_id,
                field_mask: delta_flags::MASK_KEYFRAME | delta_flags::MASK_DELTA_POS_X,
                position: (50.0, 0.0, 0.0),
                velocity: (0.0, 0.0, 0.0),
                yaw: 0.0,
                pitch: 0.0,
                health: 100,
                max_health: 100,
                state_flags: 0,
                action_id: 0,
                attributes: Vec::new(),
            }],
        });

        world.process_packet(&snap_keyframe_batch.encode()).unwrap();

        // Hard snap threshold exceeded -> hard snap reconciled
        assert_eq!(world.prediction_buffer.pending_input_count(), 0);
        assert_eq!(world.get_entity_position(local_id), Some((50.0, 0.0, 0.0)));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn fuzz_process_packet_arbitrary_bytes_and_recovery(
            corrupted in proptest::collection::vec(any::<u8>(), 0..1024),
            client_ts in any::<u64>(),
            server_ts in any::<u64>(),
        ) {
            let mut world = WasmClientWorld::new();
            let res = world.process_packet(&corrupted);

            let valid_packet = ClientPacket::Pong(mmo_schema::client::PongPacket {
                client_timestamp: client_ts,
                server_timestamp: server_ts,
            });
            let valid_frame = valid_packet.encode();

            if res.is_err() {
                let initial = world.received_packets;
                prop_assert!(world.process_packet(&valid_frame).is_ok());
                prop_assert_eq!(world.received_packets, initial + 1);
            } else if world.incoming_buffer.is_empty() {
                let initial = world.received_packets;
                prop_assert!(world.process_packet(&valid_frame).is_ok());
                prop_assert_eq!(world.received_packets, initial + 1);
            } else {
                if world.process_packet(&valid_frame).is_err() {
                    let initial = world.received_packets;
                    prop_assert!(world.process_packet(&valid_frame).is_ok());
                    prop_assert_eq!(world.received_packets, initial + 1);
                }
            }
        }
    }
}
