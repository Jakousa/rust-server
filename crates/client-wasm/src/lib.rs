//! Rust WebAssembly Client Core for OpenSpec MMO.
//!
//! Provides zero-duplication client-side packet decoding, Track 6 delta decompression,
//! temporal entity smoothing (Hermite / LERP), and flat typed buffer generation for WebGL rendering.

pub mod interp;
pub mod state;

use std::collections::HashMap;

use bytes::{Bytes, BytesMut};
use serde::Serialize;
use wasm_bindgen::prelude::*;

use mmo_schema::client::{
    ClientAckPacket, ClientPacket, InteractionPacket, InteractionType, MovementPacket, Vec3,
};

use crate::interp::InterpolationBuffer;
use crate::state::ClientEntityReplica;

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
    local_player_id: Option<u64>,
    session_id: u64,
    latest_server_tick: u64,
    latest_baseline_tick: u64,
    received_packets: u64,
    client_time: f64,
    incoming_buffer: BytesMut,
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
            local_player_id: None,
            session_id: 0,
            latest_server_tick: 0,
            latest_baseline_tick: 0,
            received_packets: 0,
            client_time: 0.0,
            incoming_buffer: BytesMut::with_capacity(16384),
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

        // Check ACK encoding
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
}
