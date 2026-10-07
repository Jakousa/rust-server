# Proposal: Decoupled Volumetric Voxel & Deformable Terrain Plugin

## Problem Statement
The MMO server core engine (`crates/core`, `crates/schema`, `crates/storage`, `crates/node`, `crates/gateway`, `crates/director`) is governed by a strict Prime Directive: it must remain genre-agnostic, mathematically generalized, and completely free of hardcoded game-specific mechanics. Core spatial indexing provides 64m discrete sectors (`SectorCoord3D`), bounding boxes (`BoundingBox3D`), octrees, and Area of Interest (AoI) observers.

However, games requiring volumetric sandbox terrain, subterranean mining, destructive environments, and dynamic voxel excavation need:
1. Chunked 3D voxel grid representations (e.g. 16×16×16 blocks) embedded within or aligned with 64m sectors.
2. Bit-packed, palette-compressed memory storage to prevent massive RAM overhead across thousands of active chunks.
3. Authoritative server-side excavation and placement with line-of-sight reach validation, rate limiting, and anti-cheat checks.
4. Fast dynamic boundary mesh extraction (greedy meshing / surface extraction) for dirty chunks.
5. Bandwidth-efficient chunk streaming and delta replication over WebTransport / QUIC streams.
6. Asynchronous write-behind persistence to SQLite / PostgreSQL storage without stalling the 20 Hz simulation tick.
7. Zero-duplication client-side decoding and mesh generation in the WebAssembly inspector (`crates/client-wasm`).

## Motivation & Architecture
In accordance with the decoupled plugin architecture outlined in `AGENTS.md`, all voxel mechanics belong in a modular extension crate: `crates/plugins/voxel` implementing `GameplayPlugin`.

This plugin will:
- Implement `GameplayPlugin` and register via `PluginContext` with a dedicated module ID (`0x00A0`) on `ModularPacketRouter`.
- Store chunks as `PaletteChunk` structs using dynamic bit-packing (1, 2, 4, 8, or 16 bits per voxel) and run-length encoding (RLE).
- Intercept player modification requests (`VoxelModifyPacket`), validating reach distance (clamped to max reach, e.g. 6.0m), line-of-sight, and rate limits.
- Dispatch dirty chunk boundary events and calculate greedy/culled face meshes for server collision updates or debug extraction.
- Stream initial chunk payloads and compact voxel delta batches to clients observing the sector.
- Enqueue dirty chunk snapshots into a non-blocking write-behind channel to persist compressed BLOBs to storage asynchronously.
- Provide a client-side WASM decoder in `crates/client-wasm` that unpacks palette chunks and generates flat WebGL-ready vertex buffers.

## Non-Goals
- Modifying `crates/core` or `crates/schema` with hardcoded voxel types or block arrays.
- Imposing voxel physics or block gravity onto non-voxel entities (handled via generalized bounding boxes).
- Synchronous blocking database operations during simulation ticks.
