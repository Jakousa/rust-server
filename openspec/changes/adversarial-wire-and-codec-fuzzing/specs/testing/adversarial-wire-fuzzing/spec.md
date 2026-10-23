# Adversarial Wire Protocol and Codec Fuzzing Specification

## Purpose
Guarantees that all network frame decoders, bit-packed delta parsers, voxel RLE codecs, and WebAssembly client packet handlers are panic-free and memory-bounded when fed arbitrary or mutated untrusted byte streams.

## Requirements

### Requirement: Panic-Free Adversarial Packet and Bitpack Decoding
`ClientPacket::decode`, `ClientPacket::decode_frame`, and `BitReader` in `crates/schema` SHALL return structured `Result` values and SHALL NOT panic or allocate unbounded memory when given arbitrary byte slices or bit-flipped mutations of valid frames.

#### Scenario: Fuzzing ClientPacket::decode and decode_frame with arbitrary and mutated bytes
- **GIVEN** arbitrary byte vectors (`0..8192` bytes) and bit-flipped mutations of valid encoded `ClientPacket` variants
- **WHEN** passed to `ClientPacket::decode` and `ClientPacket::decode_frame`
- **THEN** decoding SHALL complete without panicking

### Requirement: Voxel RLE and Net Packet Adversarial Safety
`decode_rle`, `decode_rle_raw`, and `VoxelNetPacket::decode` in `crates/plugins/voxel` SHALL losslessly round-trip arbitrary `PaletteChunk` states and SHALL reject malformed headers, truncated spans, or span-sum mismatches (`sum != 4096`) without panicking.

#### Scenario: Fuzzing Voxel RLE decoder with crafted headers and random span payloads
- **GIVEN** a payload prefixed with `VXR1` followed by arbitrary `revision`, `span_count`, and span bytes
- **WHEN** `decode_rle` is invoked
- **THEN** it SHALL return `Ok` only if the spans sum to exactly `4096` and SHALL return `Err(VoxelCodecError)` without panicking on all malformed inputs

### Requirement: WasmClientWorld Malformed Stream Recovery
`WasmClientWorld::process_packet` in `crates/client-wasm` SHALL never panic on arbitrary byte slices and SHALL clear its internal framing buffer on error so that a subsequent valid frame is decoded cleanly.

#### Scenario: Feeding arbitrary corrupted bytes followed by a valid frame to WasmClientWorld
- **GIVEN** a `WasmClientWorld` instance fed an arbitrary corrupted byte slice
- **WHEN** `process_packet` returns `Err` and is immediately fed a valid framed `ClientPacket::Pong` or `WorldSnapshot`
- **THEN** `WasmClientWorld` SHALL process the subsequent valid packet without error and increment `received_packets`
