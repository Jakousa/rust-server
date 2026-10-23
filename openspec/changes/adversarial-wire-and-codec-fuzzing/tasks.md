# Implementation Tasks: Adversarial Wire Protocol and RLE Codec Fuzzing Suite (Issue #129)

- [ ] 1. Schema Wire & BitReader Adversarial Proptests (`crates/schema/tests/packet_proptest.rs`)
  - [ ] 1.1 Add `fuzz_arbitrary_bytes_never_panic`, `fuzz_mutated_valid_packet_frames_never_panic`, and `fuzz_bitreader_arbitrary_bitstreams` to `crates/schema/tests/packet_proptest.rs`.
  - [ ] 1.2 Run `cargo test -p mmo-schema --test packet_proptest` to verify all property tests pass.

- [ ] 2. Voxel RLE & Network Codec Proptests (`crates/plugins/voxel`)
  - [ ] 2.1 Add `proptest = "1"` to `[dev-dependencies]` in `crates/plugins/voxel/Cargo.toml`.
  - [ ] 2.2 Create `crates/plugins/voxel/tests/voxel_codec_proptest.rs` testing arbitrary `PaletteChunk` RLE roundtrips and adversarial `VXR1` headers / `VoxelNetPacket::decode` inputs.
  - [ ] 2.3 Run `cargo test -p mmo-plugin-voxel` to verify all property tests pass.

- [ ] 3. WASM Client Framing Fuzz & Recovery Proptests (`crates/client-wasm`)
  - [ ] 3.1 Add `proptest = "1"` to `[dev-dependencies]` in `crates/client-wasm/Cargo.toml`.
  - [ ] 3.2 Add `fuzz_process_packet_arbitrary_bytes_and_recovery` in `crates/client-wasm/src/lib.rs`.
  - [ ] 3.3 Run `cargo test -p mmo-client-wasm` to verify all tests pass.

- [ ] 4. Workspace Quality Gates
  - [ ] 4.1 Run `cargo fmt --all -- --check`
  - [ ] 4.2 Run `cargo clippy --workspace --all-targets -- -D warnings`
  - [ ] 4.3 Run `cargo test --workspace`
