## Context & Motivation

In high-concurrency MMO simulation nodes, player and entity states (position, rotation, health, inventory, and dynamic component sets) undergo continuous high-frequency mutation during the fixed 20 Hz (50ms) simulation tick. Performing synchronous database I/O directly within the tick loop inevitably introduces blocking delays, causing missed 50ms deadlines, server rubber-banding, and simulation degradation.

While the existing `WriteBehindSnapshotter` provides basic in-memory dirty tracking using an instantaneous mutex, it lacks direct simulation loop extraction, binary serialization of component state blobs, and an asynchronous, non-blocking MPSC channel pipeline. Rapid consecutive movements trigger unnecessary database churn without entity-level debouncing.

This change implements a hardened asynchronous write-behind persistence pipeline in `mmo-node` and `mmo-storage` fulfilling Issue #58.

## Problem Statement

- Simulation nodes need a reliable way to persist entity states to SQL databases (PostgreSQL/SQLite) without blocking the 20 Hz tick loop.
- The tick loop must periodically extract and push serialized binary state blobs into a lock-free channel with zero I/O wait.
- A dedicated background snapshot worker must dequeue these state blobs, coalesce and debounce updates for rapidly moving entities, and execute chunked batch `UPSERT` statements against durable storage.
- The entity persistence schema must remain strictly generic and genre-agnostic, storing opaque component blobs rather than hardcoded game-specific columns.

## Goals & Non-Goals

### Goals
- **Lock-Free Extraction & Pipeline**: The 20 Hz simulation loop extracts entity state snapshots and enqueues serialized binary blobs into a non-blocking MPSC channel in nanoseconds.
- **Generic Binary Component Blobs**: Serialize dynamic components into opaque binary blobs (MessagePack / `rmp_serde`), preserving engine agnosticism.
- **Background Worker Coalescing & Debouncing**: Dequeue snapshot blobs in a dedicated background task, coalescing rapid successive updates per entity within a configurable debounce window (e.g., 50ms - 200ms).
- **Chunked Database UPSERTs**: Flush snapshots to SQL databases in bounded chunks (e.g., 100-500 rows per transaction) with failure retention and retry logic.
- **Strict 20 Hz Tick Invariance**: Under heavy database load or temporary database delays, tick duration must remain unaffected (< 50ms).
- **Full Observability**: Prometheus gauges and histograms for queue depth, batch flush duration, and drop counts.

### Non-Goals
- Real-time synchronous persistence queries inside gameplay tick handlers.
- Game-specific RPG schema columns (e.g., dedicated database columns for mana, stamina, warrior classes). All game mechanics must remain in modular opaque component blobs.

## Affected Crates
- `crates/storage`: Add binary blob support to `EntitySnapshot` model, schema migrations (`state_blob` column), and batch UPSERT queries in `CharacterRepository`.
- `crates/node`: Implement lock-free MPSC snapshot pipeline, periodic tick extraction in `TickSimulation`, and worker debouncing in `WriteBehindSnapshotter`.
