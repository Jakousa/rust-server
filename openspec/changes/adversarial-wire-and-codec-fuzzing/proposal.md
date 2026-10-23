# Adversarial Wire Protocol and RLE Codec Boundary & Proptest Fuzzing Suite Proposal

## Context & Problem Statement
While `crates/schema/tests/packet_proptest.rs` tests round-trip serialization of well-formed generated `ClientPacket` variants, untrusted internet traffic reaching `mmo-gateway`, `mmo-node`, `mmo-client-wasm`, and `mmo-plugin-voxel` can contain arbitrary malformed byte sequences, crafted length prefixes, truncated LEB128 varints, bit-packed delta streams with out-of-bounds flags, or RLE decompression bombs.

Currently:
1. `crates/schema/tests/packet_proptest.rs` does not fuzz `ClientPacket::decode`, `ClientPacket::decode_frame`, `BitReader`, or `unpack_entity_delta` with arbitrary adversarial byte slices or mutated valid packets.
2. `crates/plugins/voxel/src/codec.rs` and `crates/plugins/voxel/src/net.rs` lack property-based adversarial fuzz tests feeding random/corrupted byte payloads and span-overflow attacks into `decode_rle`, `decode_rle_raw`, and `VoxelNetPacket::decode`.
3. `crates/client-wasm/src/lib.rs` (`WasmClientWorld::process_packet`) lacks property-based fuzz tests verifying that arbitrary chunked/corrupted byte streams never panic and always leave `WasmClientWorld` in a recoverable state for subsequent valid frames.

## Proposed Solution
1. **Adversarial Wire & Bitpack Fuzzing in `crates/schema/tests/packet_proptest.rs`**:
   - Add `proptest!` suites that feed:
     - Completely arbitrary `Vec<u8>` byte slices (`0..8192` bytes) into `ClientPacket::decode`, `ClientPacket::decode_frame`, and `BitReader` operations, asserting zero panics.
     - Single-byte and multi-byte bit-flipped mutations of valid encoded `ClientPacket` frames (covering all packet tag IDs `0x01..=0x30`), asserting graceful `Ok` or `Err(SchemaError)` without panicking or allocating unbounded buffers.
2. **Voxel RLE & Network Codec Proptest Fuzzing (`crates/plugins/voxel/tests/voxel_codec_proptest.rs`)**:
   - Add `proptest!` suites verifying:
     - Arbitrary `PaletteChunk` voxel distributions round-trip losslessly through `encode_rle` -> `decode_rle`.
     - Arbitrary byte slices and crafted headers (`MAGIC_VXR1` + arbitrary `span_count` and span payloads) passed to `decode_rle`, `decode_rle_raw`, and `VoxelNetPacket::decode` never panic or exceed `CHUNK_VOXELS` (4,096) allocations.
3. **WASM Client Stream Fuzzing (`crates/client-wasm/src/lib.rs`)**:
   - Add `proptest` to `crates/client-wasm` `[dev-dependencies]` and add property tests feeding arbitrary corrupted byte chunks followed by valid `ClientPacket` frames into `WasmClientWorld::process_packet`, verifying zero panics and guaranteed recovery on subsequent valid packets.

## Scope & Non-Goals
- **In Scope**: `crates/schema/tests/packet_proptest.rs`, `crates/plugins/voxel/Cargo.toml`, `crates/plugins/voxel/tests/voxel_codec_proptest.rs`, `crates/client-wasm/Cargo.toml`, and `crates/client-wasm/src/lib.rs`.
- **Non-Goals**: Requiring nightly-only `cargo-fuzz` compiler instrumentation in standard stable CI runs.
