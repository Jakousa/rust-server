## Context

The server engine simulates 3D worlds at 20 Hz in `mmo_node`, utilizing `Octree` for spatial indexing and `EntityStore` for generational entity components. The `GatewayServer` in `mmo_gateway` handles incoming client TCP connections, decoding client packets (`Movement`, `Interaction`, `Ping`). The wire protocol in `mmo_schema::client` already defines `EntityStatePacket` and `EntityDespawnPacket` with binary codecs.

However, currently the gateway connection loop only handles reading from client sockets (with simple Ping-Pong response writes), and the simulation node does not track per-client view bubbles or generate delta broadcast packets. See `proposal.md` for motivation.

## Goals / Non-Goals

**Goals:**
- Provide an `AoiManager` within `mmo_node` tracking registered player sessions, their spatial observation bubbles, and visible entity sets.
- Zero-allocation spatial queries against the `Octree` by passing reusable scratch query buffers (`query_radius_with_dist_sq_into`), avoiding thousands of heap allocations per tick.
- Compute delta visibility sets:
  - `entered`: entities entering observer view $\rightarrow$ full `EntityState` packet.
  - `updated`: entities inside view that moved or updated state $\rightarrow$ delta `EntityState` packet.
  - `exited`: entities exiting view or despawned $\rightarrow$ `EntityDespawn` packet.
- Dynamic density-based radius throttling using pre-computed squared distances to protect client bandwidth when crowded.
- Bidirectional socket handling in `GatewayServer` via `TcpStream::into_split()`, managing a concurrent writer loop and session channel registry (`session_writers`).
- Clean integration with `TickSimulation` step and comprehensive unit and e2e integration tests.

**Non-Goals:**
- QUIC transport migration (scheduled for the following milestone).
- Client-side prediction and client-side dead reckoning interpolation algorithms.
- Lossy compression of floats (using existing 32-bit float binary codecs in `mmo_schema`).

## Decisions

### 1. Zero-Allocation Spatial Query Support in `Octree`
- Expose buffer-reusing query methods in `crates/core/src/octree.rs`:
  ```rust
  pub fn query_radius_into(&self, center: (f32, f32, f32), radius: f32, out: &mut Vec<Guid>);
  pub fn query_radius_with_pos_into(&self, center: (f32, f32, f32), radius: f32, out: &mut Vec<(Guid, (f32, f32, f32))>);
  pub fn query_radius_with_dist_sq_into(&self, center: (f32, f32, f32), radius: f32, out: &mut Vec<(Guid, (f32, f32, f32), f32)>);
  ```
- By providing pre-computed `dist_sq`, dynamic density pruning can sort and clamp outer entities without redundant distance recalculations.
- `AoiManager` maintains reusable scratch buffers across ticks.

### 2. `AoiManager` in `mmo_node::aoi`
- We introduce `AoiManager` managing active observer sessions:
  ```rust
  pub struct ObserverState {
      pub session_id: u64,
      pub player_guid: Guid,
      pub base_radius: f32,
      pub effective_radius: f32,
      pub max_entities: usize,
      pub observed_entities: HashMap<Guid, CachedEntityState>,
  }
  ```
- Cached state tracks last transmitted position, velocity, and health to avoid redundant updates for stationary entities.

### 3. Dual-Set Delta Diffing Algorithm
- Each tick (or broadcast cadence):
  1. Look up observer's current position from `EntityStore`.
  2. Clear scratch buffer and query `Octree::query_radius_with_dist_sq_into(pos, effective_radius, &mut scratch_buffer)`.
  3. If visible count $> \text{max\_entities}$, sort by `dist_sq`, dynamically calculate clamped `effective_radius`, and truncate to `max_entities`.
  4. Compare `new_visible_set` with `observed_entities`:
     - Entities in `new_visible_set` but not in `observed_entities` $\rightarrow$ `entered`.
     - Entities in `observed_entities` but not in `new_visible_set` $\rightarrow$ `exited`.
     - Entities in both $\rightarrow$ check if position delta $> \epsilon$ (0.01m), velocity, or health changed $\rightarrow$ `updated`.
  5. Update `observed_entities` cache and emit `AoiBroadcastJob { session_id, packets }`.

### 4. Bidirectional Gateway Architecture
- In `GatewayServer::handle_connection`:
  - Split `TcpStream` into `OwnedReadHalf` and `OwnedWriteHalf`.
  - Create a bounded `mpsc::channel<bytes::Bytes>(1024)`.
  - Register the sender in a thread-safe `session_sinks: Arc<RwLock<HashMap<u64, mpsc::Sender<Bytes>>>>`.
  - Spawn an asynchronous writer task forwarding bytes from channel to socket.
  - On connection close / error, unregister the session sink.

## Risks / Trade-offs

- **[High CPU usage on dense clusters]** $\rightarrow$ Mitigated by Octree spatial partitioning pruning queries to local octants, zero-allocation buffer reuse, and dynamic radius clamping.
- **[Slow client stalling gateway writer loop]** $\rightarrow$ Mitigated by using bounded mpsc channels per session; if a client buffer overflows, drops oldest non-critical packets or drops the slow connection.
- **[Stationary entity packet storm]** $\rightarrow$ Mitigated by delta state diffing; stationary entities with unchanged health generate zero packets after initial entry.
