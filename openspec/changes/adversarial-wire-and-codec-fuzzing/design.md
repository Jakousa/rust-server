# Technical Design: Adversarial Wire Protocol and RLE Codec Boundary & Proptest Fuzzing Suite

## 1. Schema Adversarial Fuzzing (`crates/schema/tests/packet_proptest.rs`)

Add the following property tests to `crates/schema/tests/packet_proptest.rs`:
1. **`fuzz_arbitrary_bytes_never_panic`**:
   - Strategy: `proptest::collection::vec(any::<u8>(), 0..4096)`
   - Call `let _ = ClientPacket::decode(&bytes);`
   - Call `let mut buf = BytesMut::from(&bytes[..]); let _ = ClientPacket::decode_frame(&mut buf);`
2. **`fuzz_mutated_valid_packet_frames_never_panic`**:
   - Strategy: `(arb_any_client_packet(), proptest::collection::vec((0usize..512, any::<u8>()), 1..16))`
   - Encode the valid packet via `packet.encode()` and `packet.encode_framed()`, apply random byte mutations/truncations, and pass to `ClientPacket::decode` and `ClientPacket::decode_frame`, verifying zero panics.
3. **`fuzz_bitreader_arbitrary_bitstreams`**:
   - Strategy: `(proptest::collection::vec(any::<u8>(), 0..256), proptest::collection::vec(1u8..=64u8, 1..64))`
   - Construct `BitReader::new(&bytes)` and read sequence of arbitrary bit widths (`read_bits`, `read_varint`, `read_signed_varint`), verifying deterministic `None`/`Some` return without panics or arithmetic overflow.

---

## 2. Voxel Codec Property & Adversarial Tests (`crates/plugins/voxel/tests/voxel_codec_proptest.rs`)

1. Add `proptest = "1"` under `[dev-dependencies]` in `crates/plugins/voxel/Cargo.toml`.
2. Create `crates/plugins/voxel/tests/voxel_codec_proptest.rs`:
   - **`proptest_rle_roundtrip_arbitrary_chunk`**:
     - Generate arbitrary voxel updates (`vec((0usize..4096, 0u16..512), 0..512)`) and `revision: u64`, populate `PaletteChunk`, encode via `encode_rle(&chunk)`, decode via `decode_rle(&encoded)`, and assert every voxel `0..4096` and `revision` match.
   - **`proptest_rle_adversarial_bytes_never_panic`**:
     - Generate arbitrary byte slices (`0..4096`) and `VXR1`-prefixed byte slices with arbitrary `span_count` (`u32`) and body bytes; invoke `decode_rle(&bytes)` and `VoxelNetPacket::decode(&bytes)` and assert zero panics.

---

## 3. WASM Client Adversarial Recovery Tests (`crates/client-wasm/src/lib.rs`)

1. Add `proptest = "1"` under `[dev-dependencies]` in `crates/client-wasm/Cargo.toml`.
2. In `crates/client-wasm/src/lib.rs` `#[cfg(test)] mod tests`:
   - Add `proptest!` test `fuzz_process_packet_arbitrary_bytes_and_recovery`:
     - Feed `corrupted: Vec<u8>` (`0..1024` bytes) into `world.process_packet(&corrupted)`.
     - Regardless of whether `corrupted` returned `Ok` or `Err`, immediately feed a valid `ClientPacket::Pong { client_timestamp: 42, server_timestamp: 100 }.encode_framed()` into `world.process_packet(&valid_frame)` and assert `is_ok()`.
