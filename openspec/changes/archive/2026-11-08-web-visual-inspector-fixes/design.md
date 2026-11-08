# Technical Design: Web Visual Inspector Fixes

## 1. Network & Dynamic Connection Architecture
- **Host Resolution**:
  On bootstrap, `main.js` inspects `window.location.hostname`. If defined and not empty, the gateway URL input is populated with `https://${window.location.hostname}:4433`. The cert-hash lookup points to `http://${window.location.hostname}:9091/cert-hash`.
- **Auto-Connect**:
  Upon DOM loading, after `autoFetchCertHash()` resolves the digest:
  - If `typeof window.WebTransport === "undefined"`, the client gracefully displays a notice in the HUD without blocking execution.
  - Otherwise, auto-connect is triggered via `onConnect(url, certHash)`.
  - Blocking `window.alert()` calls are replaced with non-blocking HUD error indicators.
- **Session Identity & Multi-Tab Isolation**:
  The client generates a unique session token stored in `sessionStorage` (`guest-<base36-timestamp>-<random-hex>`). Opening a new tab creates a fresh `sessionStorage` entry with an isolated token, prompting the gateway to provision an isolated player GUID and character entity in the simulation.
- **State Reset on Disconnect**:
  On transport disconnect or reconnection, `world.entities.clear()` and `renderer.clear()` are executed to purge stale entity meshes and prevent ghost entities. A connection lock (`_isConnecting`) guards against duplicate concurrent connection attempts.

## 2. Binary Protocol & EntityDeltaBatch Codec
- **Wire Specification (`mmo_schema::client::Opcode::EntityDeltaBatch = 0x0009`)**:
  - `server_tick`: LEB128 varint `u64`
  - `baseline_tick`: LEB128 varint `u64`
  - `despawn_count`: LEB128 varint `u32`
  - `despawns`: `despawn_count` iterations of LEB128 varint `u64` (GUIDs)
  - `delta_count`: LEB128 varint `u32`
  - `deltas`: `delta_count` iterations of `EntityDeltaState`:
    - `entity_id`: LEB128 varint `u64`
    - `field_mask`: `u16` little-endian
    - If `field_mask & MASK_KEYFRAME (0x0001)`: 3x `f32` (x, y, z)
    - Else:
      - `MASK_DELTA_POS_X (0x0002)`: `int16` / 128.0 (little-endian)
      - `MASK_DELTA_POS_Y (0x0004)`: `int16` / 128.0
      - `MASK_DELTA_POS_Z (0x0008)`: `int16` / 128.0
    - If `MASK_VELOCITY (0x0010)`: 3x `int16` / 100.0 (vx, vy, vz)
    - If `MASK_YAW (0x0020)`: `uint8` / 256.0 * 2π
    - If `MASK_PITCH (0x0040)`: `int8` (clamped `[-127, 127]`) / 127.0 * (π / 2)
    - If `MASK_HEALTH (0x0080)`: varint `u32`
    - If `MASK_MAX_HEALTH (0x0100)`: varint `u32`
    - If `MASK_STATE_FLAGS (0x0200)`: varint `u32`
    - If `MASK_ACTION_ID (0x0400)`: varint `u32`
    - If `MASK_ATTRIBUTES (0x0800)`: varint count `u32`, followed by count * (varint attr_id `u32`, varint value `u32`)
- **Robust Exception Boundary**:
  All varint decoders verify buffer offsets (`offset + bytesRead <= view.byteLength`). The `EntityDeltaBatch` decoding loop is enclosed in a `try/catch` block. Truncated or malformed datagrams are logged and discarded without terminating `readDatagrams`.
- **Baseline Acknowledgment (`ClientAckPacket` Opcode 0x000A)**:
  To allow the authoritative node (`crates/node/src/aoi.rs`) to advance `confirmed_baseline_tick` and activate delta compression, `encodeClientAck(sessionId, observedTick, ackMask)` is implemented. Upon receiving each batch, the client sends `ClientAck` back over datagrams.

## 3. Interactive Camera Orbit & Spherical Coordinates
- **Camera Controller State**:
  - `orbitYaw`: Horizontal azimuth angle (radians), default `0.0`.
  - `orbitPitch`: Vertical elevation angle (radians), default `0.45` (~26°). Clamped to `[0.05, 1.45]` to prevent gimbal lock singularities and ground plane clipping.
  - `distance`: Spherical radius (meters), default `24.0`, clamped `[5.0, 60.0]`.
  - `targetOffset`: Vector3 `(0, 1.5, 0)` focusing on player chest/head level.
- **Pointer Event Handling**:
  - Attached directly to `renderer.domElement` with `setPointerCapture(e.pointerId)`.
  - Events originating inside `#hud-overlay` or modal dialogs are ignored.
  - Left or right mouse drag: adjusts `orbitYaw += deltaX * sensitivity` and `orbitPitch = clamp(orbitPitch - deltaY * sensitivity, 0.05, 1.45)`.
  - Mouse wheel: adjusts `distance = clamp(distance + deltaY * zoomFactor, 5.0, 60.0)`.
- **Camera Math**:
  ```javascript
  const cx = target.x + distance * Math.sin(orbitYaw) * Math.cos(orbitPitch);
  const cy = target.y + targetOffsetY + distance * Math.sin(orbitPitch);
  const cz = target.z + distance * Math.cos(orbitYaw) * Math.cos(orbitPitch);
  camera.position.set(cx, cy, cz);
  camera.lookAt(target.x, target.y + targetOffsetY, target.z);
  ```

## 4. Village Scene Aesthetics & Composite Prefab Meshes
- **Ground & Paths (In `WorldRenderer.initEnvironment()`)**:
  - Village meadow terrain with subtle grass/slate coloring (`0x1b3b22` / `0x243329`).
  - Circular cobblestone central plaza (`r = 14m`) centered around the well `(0, 0, 0)`.
  - 4 radial pathways connecting to the Blacksmith Forge `(-12, 0, -8)`, Herbalist Stall `(12, 0, -8)`, West House `(-18, 0, 10)`, and East House `(18, 0, 10)`.
  - Perimeter decorative conifers/trees and perimeter fencing at `~35m` radius.
  - Village lanterns with warm point lights around the well and plaza.
- **Composite WorldObject Prefab Meshes (In `WorldObjectRenderer`)**:
  - Dynamic prefabs are rendered exclusively when `WorldObjectSpawn` arrives to prevent z-fighting:
    - **Prefab 10 (Well)**: Stone cylindrical well base + 2 wooden posts + peaked wooden roof canopy + iron bucket and rope.
    - **Prefab 11 (Forge)**: Stone masonry hearth + heated coal basin with warm glow + iron chimney stack + external anvil block.
    - **Prefab 12 (Stall)**: Wooden timber counter + green striped fabric canopy supported by wooden struts + display crates.
    - **Prefab 13 (House)**: Timber log house walls + pitched roof with thatch/shingle overhang + stone chimney + front door frame.
