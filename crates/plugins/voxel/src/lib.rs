//! Decoupled Volumetric Voxel & Deformable Terrain Plugin.
//!
//! Provides chunked 3D voxel representations, bit-packed palette compression,
//! boundary mesh extraction, authoritative excavation, and write-behind persistence.

pub mod chunk;
pub mod codec;
pub mod coords;
pub mod mesh;
pub mod modify;
pub mod net;
pub mod persistence;
pub mod plugin;
pub mod world;

pub use chunk::PaletteChunk;
pub use codec::{decode_rle, decode_rle_raw, encode_rle, encode_rle_raw, VoxelCodecError};
pub use coords::{
    index_from_local, local_from_index, VoxelChunkCoord, VoxelCoord, CHUNK_SIZE, CHUNK_VOXELS,
    VOXEL_SCALE,
};
pub use mesh::{generate_chunk_mesh, NeighborBoundarySlices, VoxelMesh};
pub use modify::{
    affected_neighbor_chunks, is_boundary_voxel, ExcavationConfig, ExcavationRateLimiter,
    VoxelChangeEvent, VoxelModifyResult,
};
pub use net::{
    VoxelChunkDataPacket, VoxelChunkRequestPacket, VoxelDeltaBatchPacket, VoxelDeltaEntry,
    VoxelModifyRequestPacket, VoxelModifyResponsePacket, VoxelPacketError, SUB_OPCODE_CHUNK_DATA,
    SUB_OPCODE_CHUNK_REQUEST, SUB_OPCODE_DELTA_BATCH, SUB_OPCODE_MODIFY_REQUEST,
    SUB_OPCODE_MODIFY_RESPONSE, VOXEL_MODULE_ID,
};
pub use persistence::{
    SerializedChunkSnapshot, VoxelWriteBehindService, VOXEL_CHUNKS_TABLE_SCHEMA,
};
pub use plugin::{VoxelPacketHandler, VoxelTerrainPlugin, VoxelTickSystem};
pub use world::VoxelWorld;
