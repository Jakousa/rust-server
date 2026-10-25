use mmo_plugin_voxel::{
    decode_rle, decode_rle_raw, encode_rle, encode_rle_raw, PaletteChunk, VoxelChunkDataPacket,
    VoxelChunkRequestPacket, VoxelDeltaBatchPacket, VoxelModifyRequestPacket,
    VoxelModifyResponsePacket, CHUNK_VOXELS,
};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn proptest_rle_roundtrip_arbitrary_chunk(
        initial_voxel in 0u16..512,
        updates in proptest::collection::vec((0usize..CHUNK_VOXELS, 0u16..512), 0..512),
        revision in any::<u64>(),
    ) {
        let mut chunk = PaletteChunk::new_homogeneous(initial_voxel);
        for (idx, val) in updates {
            chunk.set(idx, val);
        }
        chunk.set_revision(revision);

        let encoded = encode_rle(&chunk);
        let decoded = decode_rle(&encoded).expect("Valid encoded chunk must decode");

        prop_assert_eq!(decoded.revision(), revision);
        for i in 0..CHUNK_VOXELS {
            prop_assert_eq!(decoded.get(i), chunk.get(i), "Voxel mismatch at index {}", i);
        }
    }

    #[test]
    fn proptest_rle_raw_roundtrip(
        voxels_raw in proptest::collection::vec(0u16..64, CHUNK_VOXELS),
        revision in any::<u64>(),
    ) {
        let mut voxels = Box::new([0u16; CHUNK_VOXELS]);
        voxels.copy_from_slice(&voxels_raw);

        let encoded = encode_rle_raw(&voxels, revision);
        let (decoded_voxels, dec_rev) = decode_rle_raw(&encoded).expect("Valid raw RLE must decode");

        prop_assert_eq!(dec_rev, revision);
        prop_assert_eq!(&decoded_voxels[..], &voxels[..]);
    }

    #[test]
    fn proptest_rle_adversarial_bytes_never_panic(
        bytes in proptest::collection::vec(any::<u8>(), 0..4096),
    ) {
        let _ = decode_rle(&bytes);
        let _ = decode_rle_raw(&bytes);
        let _ = VoxelChunkRequestPacket::decode(&bytes);
        let _ = VoxelChunkDataPacket::decode(&bytes);
        let _ = VoxelDeltaBatchPacket::decode(&bytes);
        let _ = VoxelModifyRequestPacket::decode(&bytes);
        let _ = VoxelModifyResponsePacket::decode(&bytes);
    }

    #[test]
    fn proptest_rle_crafted_vxr1_headers_never_panic(
        revision in any::<u64>(),
        span_count in any::<u32>(),
        body in proptest::collection::vec(any::<u8>(), 0..2048),
    ) {
        let mut bytes = Vec::with_capacity(16 + body.len());
        bytes.extend_from_slice(b"VXR1");
        bytes.extend_from_slice(&revision.to_le_bytes());
        bytes.extend_from_slice(&span_count.to_le_bytes());
        bytes.extend_from_slice(&body);

        let _ = decode_rle(&bytes);
        let _ = decode_rle_raw(&bytes);
    }
}
