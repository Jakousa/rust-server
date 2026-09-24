## Why

The distributed server simulates entities at 20 Hz, validates kinematic movement, and synchronizes ghost entities across simulation node boundaries. However, currently there is no downstream Area-of-Interest (AoI) packet broadcasting pipeline to stream visible world states (players, NPCs, spawns, despawns) back to connected clients. To enable playable multiplayer clients, simulation nodes must continuously track what each player sees within their spatial visibility radius, compute delta changes (entities entering, updating, or leaving the observer's field of view), and stream authoritative `EntityState` and `EntityDespawn` binary packets through the Edge Gateway to client sockets.

## What Changes

- **Per-Player AoI Observer Tracking**: Simulation nodes track registered player client sessions, maintain each player's previously observed entity set (`HashSet<Guid>`), and perform periodic (or per-tick) 3D radius queries against the spatial Octree.
- **Delta State & Despawn Generation**:
  - Entities newly entering the player's visibility bubble generate `EntityState` packets (full initial state / spawn).
  - Entities moving or updating within the visibility bubble generate delta `EntityState` packets with current position, velocity, orientation, and health.
  - Entities exiting the visibility bubble or despawning from the simulation generate `EntityDespawn` packets.
- **Dynamic AoI Radius Throttling**: Support configurable base visibility radius (e.g. 50.0m) with density-aware radius clamping to protect client bandwidth under high local entity counts.
- **Downstream Gateway Delivery**: Enhance the Edge Gateway connection handler to support bidirectional streaming, delivering outgoing binary packets from simulation node queues/channels directly to the connected client TCP/stream sockets.
- **End-to-End AoI Integration Tests**: Comprehensive tests verifying that player movements trigger appropriate spawn, state update, and despawn packets to observing peers, while unobserved entities outside the radius generate zero packets.

## Capabilities

### New Capabilities
- `simulation/aoi`: Area-of-Interest entity observation tracking, sphere range queries in the Octree, delta state tracking (`entered`, `updated`, `exited`), and emission of binary `EntityState` and `EntityDespawn` packets per player session.

### Modified Capabilities
- `gateway/routing`: The Edge Gateway SHALL maintain downstream packet delivery channels for connected client sessions, multiplexing incoming packet reception with outgoing server broadcast packet writes.

## Impact

- `crates/node`: New `aoi` module in `mmo_node`, integrating with `TickSimulation` step and `Octree::query_range`.
- `crates/gateway`: Addition of downstream client writer sinks/channels in `GatewayServer` connection loops.
- `crates/schema`: Reuse of existing `EntityStatePacket` and `EntityDespawnPacket` binary codecs.
- Performance: Minimal allocation per tick by reusing observer sets and spatial query buffers.
