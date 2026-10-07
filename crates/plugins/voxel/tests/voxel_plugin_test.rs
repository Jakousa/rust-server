//! End-to-end integration and stress tests for the Voxel Terrain Plugin.

use std::sync::Arc;

use mmo_core::entity::{Entity, EntityStore};
use mmo_core::guid::{EntityType, Guid};
use mmo_core::hook::HookDispatcher;
use mmo_core::plugin::{ComponentCodecRegistry, EventQueue, ModularPacketRouter, PluginManager};
use mmo_plugin_voxel::{
    decode_rle, index_from_local, ExcavationConfig, PaletteChunk, VoxelChunkCoord,
    VoxelChunkDataPacket, VoxelChunkRequestPacket, VoxelCoord, VoxelModifyRequestPacket,
    VoxelModifyResponsePacket, VoxelTerrainPlugin, VoxelWorld, VoxelWriteBehindService,
    SUB_OPCODE_CHUNK_REQUEST, SUB_OPCODE_MODIFY_REQUEST, VOXEL_MODULE_ID,
};

#[test]
fn test_e2e_authoritative_mining_and_delta_replication() {
    let config = ExcavationConfig {
        max_reach: 6.0,
        reach_tolerance: 0.5,
        cooldown_ms: 50,
    };
    let world = VoxelWorld::new(config);
    let chunk_coord = VoxelChunkCoord::new(0, 0, 0);

    // Populate initial chunk with solid stone (ID 1)
    let chunk = PaletteChunk::new_homogeneous(1);
    world.insert_chunk(chunk_coord, chunk);

    // Initial boundary mesh: Since 0,0,0 is bounded by unloaded neighbors (treated as solid),
    // and internal voxels are solid, 0 faces should be exposed!
    let initial_mesh = world.build_mesh(chunk_coord).expect("initial mesh");
    assert_eq!(initial_mesh.vertex_count(), 0);

    // Setup plugin & dispatcher
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

    // Spawn player at (5.0, 5.0, 5.0)
    let mut entities = EntityStore::new();
    let player_guid = Guid::new(EntityType::Player, 1, 100);
    let player = Entity::new(player_guid, (5.0, 5.0, 5.0));
    entities.insert_entity(player);

    // Player excavates center block at (5, 5, 5) to Air (0)
    let dig_req = VoxelModifyRequestPacket {
        target_voxel: VoxelCoord::new(5, 5, 5),
        new_voxel: 0,
    };

    let resp_bytes = packets
        .dispatch(
            VOXEL_MODULE_ID,
            SUB_OPCODE_MODIFY_REQUEST,
            player_guid,
            &dig_req.encode(),
            &mut entities,
        )
        .expect("dispatch")
        .expect("response");

    let resp = VoxelModifyResponsePacket::decode(&resp_bytes).expect("decode response");
    assert_eq!(resp.status, 0); // Success

    // Digging an interior solid voxel to air creates an empty pocket:
    // The 6 adjacent solid stone blocks now expose their faces into this air pocket!
    // 6 surrounding blocks * 1 inward face each = 6 faces * 6 vertices = 36 vertices!
    let updated_mesh = world.build_mesh(chunk_coord).expect("updated mesh");
    assert_eq!(updated_mesh.vertex_count(), 36);
    assert_eq!(updated_mesh.triangle_count(), 12);

    // Fetch full chunk data via network packet request
    let chunk_req = VoxelChunkRequestPacket { chunk: chunk_coord };
    let chunk_data_bytes = packets
        .dispatch(
            VOXEL_MODULE_ID,
            SUB_OPCODE_CHUNK_REQUEST,
            player_guid,
            &chunk_req.encode(),
            &mut entities,
        )
        .expect("dispatch")
        .expect("response");

    let data_packet = VoxelChunkDataPacket::decode(&chunk_data_bytes).expect("decode chunk data");
    assert_eq!(data_packet.chunk, chunk_coord);

    let decoded_chunk = decode_rle(&data_packet.rle_payload).expect("decode rle chunk");
    assert_eq!(decoded_chunk.get(index_from_local(5, 5, 5)), 0);
    assert_eq!(decoded_chunk.get(index_from_local(4, 5, 5)), 1);
    assert_eq!(decoded_chunk.revision(), 1);
}

#[test]
fn test_multi_chunk_deformation_boundary_propagation() {
    let world = VoxelWorld::default();
    let c0 = VoxelChunkCoord::new(0, 0, 0);
    let c1 = VoxelChunkCoord::new(1, 0, 0);

    // c0 is solid stone (1), c1 is air (0)
    let chunk0 = PaletteChunk::new_homogeneous(1);
    let chunk1 = PaletteChunk::new_homogeneous(0);

    world.insert_chunk(c0, chunk0);
    world.insert_chunk(c1, chunk1);

    // Boundary quads between c0 and c1:
    // c0's face at x=15 faces c1 (which is air).
    // A 16x16 face of quads = 256 faces * 6 vertices = 1,536 vertices!
    let mesh0 = world.build_mesh(c0).expect("mesh0");
    assert_eq!(mesh0.vertex_count(), 256 * 6);

    // Dig out an edge block in c0 at (15, 0, 0)
    let player_guid = Guid::new(EntityType::Player, 1, 1);
    let (res, _) = world.modify_voxel(
        player_guid,
        VoxelCoord::new(15, 0, 0),
        0, // set to air
        (15.0, 0.0, 0.0),
        1000,
    );
    assert!(matches!(
        res,
        mmo_plugin_voxel::VoxelModifyResult::Success { .. }
    ));

    // Boundary block at x=15 was modified, which flags both c0 and c1 as dirty!
    let dirty = world.drain_dirty_chunks();
    assert!(dirty.contains(&c0));
    assert!(dirty.contains(&c1));
}

#[test]
fn test_high_throughput_excavation_and_persistence_stress() {
    let write_behind = Arc::new(VoxelWriteBehindService::new(1000));
    let coord = VoxelChunkCoord::new(0, 0, 0);
    let mut chunk = PaletteChunk::new_homogeneous(1);

    // Perform 1,000 rapid block mutations
    for i in 0..1000 {
        chunk.set(i, ((i % 10) + 2) as u16);
        write_behind.enqueue(coord, &chunk);
    }

    // All 1,000 mutations to the same chunk must coalesce into 1 pending write
    assert_eq!(write_behind.pending_count(), 1);

    let drained = write_behind.drain_pending();
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].revision, 1000);
    assert_eq!(write_behind.pending_count(), 0);

    // Verify chunk integrity after serialization roundtrip
    let restored = decode_rle(&drained[0].data_blob).expect("restore");
    assert_eq!(restored.revision(), 1000);
    for i in 0..1000 {
        assert_eq!(restored.get(i), ((i % 10) + 2) as u16);
    }
}
