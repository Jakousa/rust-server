# Implementation Tasks: Adversarial Wire Protocol and RLE Codec Fuzzing Suite (Issue #129)

- [x] 1. Schema Wire & BitReader Adversarial Proptests (`crates/schema/tests/packet_proptest.rs`)
  - [x] 1.1 Add `fuzz_arbitrary_bytes_never_panic`, `fuzz_mutated_valid_packet_frames_never_panic`, and `fuzz_bitreader_arbitrary_bitstreams` to `crates/schema/tests/packet_proptest.rs`.
  - [x] 1.2 Run `cargo test -p mmo-schema --test packet_proptest` to verify all property tests pass.

- [x] 2. Voxel RLE & Network Codec Proptests (`crates/plugins/voxel`)
  - [x] 2.1 Add `proptest = "1"` to `[dev-dependencies]` in `crates/plugins/voxel/Cargo.toml`.
  - [x] 2.2 Create `crates/plugins/voxel/tests/voxel_codec_proptest.rs` testing arbitrary `PaletteChunk` RLE roundtrips and adversarial `VXR1` headers / `VoxelNetPacket::decode` inputs.
  - [x] 2.3 Run `cargo test -p mmo-plugin-voxel` to verify all property tests pass.

- [x] 3. WASM Client Framing Fuzz & Recovery Proptests (`crates/client-wasm`)
  - [x] 3.1 Add `proptest = "1"` to `[dev-dependencies]` in `crates/client-wasm/Cargo.toml`.
  - [x] 3.2 Add `fuzz_process_packet_arbitrary_bytes_and_recovery` in `crates/client-wasm/src/lib.rs`.
  - [x] 3.3 Run `cargo test -p mmo-client-wasm` to verify all tests pass.

- [x] 4. Workspace Quality Gates
  - [x] 4.1 Run `cargo fmt --all -- --check`
  - [x] 4.2 Run `cargo clippy --workspace --all-targets -- -D warnings`
  - [x] 4.3 Run `cargo test --workspace`
