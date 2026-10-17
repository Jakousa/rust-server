//! GameplayPlugin implementation for decoupled voxel subsystem.

use std::time::SystemTime;

use mmo_core::entity::EntityStore;
use mmo_core::guid::Guid;
use mmo_core::plugin::{
    GameplayPlugin, ModularPacketHandler, PacketHandlerError, PluginContext, TickContext,
    TickSystem,
};

use crate::codec::encode_rle;
use crate::modify::VoxelModifyResult;
use crate::net::{
    VoxelChunkDataPacket, VoxelChunkRequestPacket, VoxelModifyRequestPacket,
    VoxelModifyResponsePacket, SUB_OPCODE_CHUNK_REQUEST, SUB_OPCODE_MODIFY_REQUEST,
    VOXEL_MODULE_ID,
};
use crate::persistence::VoxelWriteBehindService;
use crate::world::VoxelWorld;

/// Packet handler dispatching modular voxel networking packets.
pub struct VoxelPacketHandler {
    world: VoxelWorld,
}

impl VoxelPacketHandler {
    pub fn new(world: VoxelWorld) -> Self {
        Self { world }
    }
}

impl ModularPacketHandler for VoxelPacketHandler {
    fn handle(
        &self,
        entity_guid: Guid,
        sub_opcode: u16,
        payload: &[u8],
        entities: &mut EntityStore,
    ) -> Result<Option<Vec<u8>>, PacketHandlerError> {
        match sub_opcode {
            SUB_OPCODE_CHUNK_REQUEST => {
                let req = VoxelChunkRequestPacket::decode(payload).map_err(|e| {
                    PacketHandlerError::MalformedPayload {
                        module_id: VOXEL_MODULE_ID,
                        message: e.to_string(),
                    }
                })?;

                if let Some(chunk_arc) = self.world.get_chunk(req.chunk) {
                    let rle = {
                        let chunk = chunk_arc.read().unwrap_or_else(|e| e.into_inner());
                        encode_rle(&chunk)
                    };
                    let resp = VoxelChunkDataPacket {
                        chunk: req.chunk,
                        rle_payload: rle,
                    };
                    Ok(Some(resp.encode()))
                } else {
                    Ok(None)
                }
            }
            SUB_OPCODE_MODIFY_REQUEST => {
                let req = VoxelModifyRequestPacket::decode(payload).map_err(|e| {
                    PacketHandlerError::MalformedPayload {
                        module_id: VOXEL_MODULE_ID,
                        message: e.to_string(),
                    }
                })?;

                // Extract player position from EntityStore
                let player_pos = entities
                    .get_by_guid(entity_guid)
                    .map(|e| e.position)
                    .unwrap_or((0.0, 0.0, 0.0));

                let now_ms = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);

                let (res, _event) = self.world.modify_voxel(
                    entity_guid,
                    req.target_voxel,
                    req.new_voxel,
                    player_pos,
                    now_ms,
                );

                let status = match res {
                    VoxelModifyResult::Success { .. } => 0,
                    VoxelModifyResult::OutOfReach { .. } => 1,
                    VoxelModifyResult::RateLimited { .. } => 2,
                    VoxelModifyResult::NoChange => 3,
                    VoxelModifyResult::ChunkNotLoaded => 4,
                };

                let resp = VoxelModifyResponsePacket {
                    status,
                    target_voxel: req.target_voxel,
                    new_voxel: req.new_voxel,
                };

                Ok(Some(resp.encode()))
            }
            unknown => Err(PacketHandlerError::MalformedPayload {
                module_id: VOXEL_MODULE_ID,
                message: format!("Unknown sub-opcode 0x{:04X}", unknown),
            }),
        }
    }
}

/// Tick system managing voxel dirty chunk processing during simulation update.
pub struct VoxelTickSystem {
    world: VoxelWorld,
    write_behind: VoxelWriteBehindService,
}

impl VoxelTickSystem {
    pub fn new(world: VoxelWorld) -> Self {
        Self {
            world,
            write_behind: VoxelWriteBehindService::default(),
        }
    }

    pub fn with_write_behind(world: VoxelWorld, write_behind: VoxelWriteBehindService) -> Self {
        Self {
            world,
            write_behind,
        }
    }

    pub fn write_behind(&self) -> &VoxelWriteBehindService {
        &self.write_behind
    }
}

impl TickSystem for VoxelTickSystem {
    fn name(&self) -> &'static str {
        "voxel_terrain_tick_system"
    }

    fn update(&mut self, ctx: &mut TickContext) {
        // 1. Drain buffered VoxelChangeEvents into ctx.events
        for ev in self.world.drain_events() {
            ctx.events.push(ev);
        }

        // 2. Drain dirty chunks, enqueue to write-behind, mark clean
        for coord in self.world.drain_dirty_chunks() {
            if let Some(chunk_arc) = self.world.get_chunk(coord) {
                let mut chunk = chunk_arc.write().unwrap_or_else(|e| e.into_inner());
                if chunk.is_dirty() {
                    self.write_behind.enqueue(coord, &chunk);
                    chunk.mark_clean();
                }
            }
        }

        // 3. Prune despawned entities from rate limiter
        self.world.prune_rate_limiter(ctx.entities);
    }
}

/// Standalone, decoupled voxel terrain plugin implementing `GameplayPlugin`.
#[derive(Clone)]
pub struct VoxelTerrainPlugin {
    world: VoxelWorld,
    write_behind: VoxelWriteBehindService,
}

impl VoxelTerrainPlugin {
    pub fn new(world: VoxelWorld) -> Self {
        Self {
            world,
            write_behind: VoxelWriteBehindService::default(),
        }
    }

    pub fn with_write_behind(world: VoxelWorld, write_behind: VoxelWriteBehindService) -> Self {
        Self {
            world,
            write_behind,
        }
    }

    pub fn world(&self) -> &VoxelWorld {
        &self.world
    }

    pub fn write_behind(&self) -> &VoxelWriteBehindService {
        &self.write_behind
    }
}

impl GameplayPlugin for VoxelTerrainPlugin {
    fn id(&self) -> &'static str {
        "voxel_terrain"
    }

    fn build(&self, ctx: &mut PluginContext) {
        // 1. Register modular packet router under assigned VOXEL_MODULE_ID (0x00A0)
        ctx.packets.register(
            VOXEL_MODULE_ID,
            Box::new(VoxelPacketHandler::new(self.world.clone())),
        );

        // 2. Register tick system for dirty chunk management
        ctx.tick_systems
            .push(Box::new(VoxelTickSystem::with_write_behind(
                self.world.clone(),
                self.write_behind.clone(),
            )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::PaletteChunk;
    use crate::coords::{VoxelChunkCoord, VoxelCoord};
    use crate::net::VoxelChunkDataPacket;
    use mmo_core::entity::Entity;
    use mmo_core::guid::EntityType;
    use mmo_core::hook::HookDispatcher;
    use mmo_core::plugin::{
        ComponentCodecRegistry, EventQueue, ModularPacketRouter, PluginManager,
    };

    #[test]
    fn test_voxel_plugin_lifecycle_and_packet_routing() {
        let world = VoxelWorld::default();
        let chunk_coord = VoxelChunkCoord::new(0, 0, 0);
        let chunk = PaletteChunk::new_homogeneous(42);
        world.insert_chunk(chunk_coord, chunk);

        let plugin = VoxelTerrainPlugin::new(world.clone());
        let mut manager = PluginManager::new();
        manager.add_plugin(Box::new(plugin));

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

        assert!(packets.has_module(VOXEL_MODULE_ID));
        assert_eq!(tick_systems.len(), 1);

        // Test Packet Dispatch - Chunk Request
        let mut entities = EntityStore::new();
        let player_guid = Guid::new(EntityType::Player, 1, 10);
        let player_entity = Entity::new(player_guid, (0.0, 0.0, 0.0));
        entities.insert_entity(player_entity);

        let req_pkt = VoxelChunkRequestPacket { chunk: chunk_coord };
        let resp_bytes = packets
            .dispatch(
                VOXEL_MODULE_ID,
                SUB_OPCODE_CHUNK_REQUEST,
                player_guid,
                &req_pkt.encode(),
                &mut entities,
            )
            .expect("dispatch should succeed")
            .expect("should return data response");

        let data_pkt = VoxelChunkDataPacket::decode(&resp_bytes).expect("decode data packet");
        assert_eq!(data_pkt.chunk, chunk_coord);
        assert!(!data_pkt.rle_payload.is_empty());

        // Test Packet Dispatch - Modify Request within reach
        let mod_pkt = VoxelModifyRequestPacket {
            target_voxel: VoxelCoord::new(1, 0, 0),
            new_voxel: 99,
        };
        let mod_resp_bytes = packets
            .dispatch(
                VOXEL_MODULE_ID,
                SUB_OPCODE_MODIFY_REQUEST,
                player_guid,
                &mod_pkt.encode(),
                &mut entities,
            )
            .expect("dispatch")
            .expect("response");

        let mod_resp = VoxelModifyResponsePacket::decode(&mod_resp_bytes).expect("decode");
        assert_eq!(mod_resp.status, 0); // Success
        assert_eq!(mod_resp.new_voxel, 99);
    }
}
