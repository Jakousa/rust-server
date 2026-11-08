# Change Proposal: Web Visual Inspector Fixes

## 1. Context & Motivation
Production deployment verification of the web client on `http://49.13.149.126/` (Issue #185) revealed multiple defects:
1. **Live Telemetry & Connectivity**: The client defaults to hardcoded `https://127.0.0.1:4433` and remains disconnected upon initial load. Only FPS and a hardcoded entity count of 1 update. Server tick, baseline, ping, packet counters, and bandwidth remain unpopulated.
2. **Multi-Session Visibility & Entity Delta Decoding**: Sessions hardcode a single `"guest-poc"` authentication token, colliding in character assignment. More critically, `ClientWorldBridge.processPacket` discards `OPCODES.ENTITY_DELTA_BATCH` (0x0009) bodies, causing all remote player movements and villager NPCs to remain invisible in the 3D scene.
3. **Camera Controls**: While HUD instructions claim "Mouse: Pan View", no pointer event listeners exist in `PlayerControls`, and `WorldRenderer.updateCamera` restricts the camera to a fixed third-person offset. Users cannot orbit, pitch, or zoom the camera.
4. **Village Environment & Playability**: Without connected packets, the world is an empty, dark void (`0x0f172a`). Spawned prefabs (well, forge, stall, houses) are unadorned single-color primitive boxes, lacking architectural fidelity and environmental cues.

## 2. Proposed Changes
- **Dynamic Connection & Unique Guest Sessions**:
  - Dynamically default the gateway URL to `https://${window.location.hostname}:4433` (or current browser host).
  - Automatically fetch the self-signed certificate fingerprint and connect on initial page load.
  - Generate a distinct guest token per tab/session (using `sessionStorage` or randomized ID) to enable multi-client play.
- **EntityDeltaBatch Binary Decoding**:
  - Implement full LEB128 varint parsing for `server_tick`, `baseline_tick`, `despawns`, and compressed `EntityDeltaState` updates matching `crates/schema/src/client.rs`.
  - Handle keyframe positions, quantized displacement deltas, quantized yaw, velocities, and health percentages.
  - Replicate remote entities in `this.entities` and process despawns.
- **Interactive Orbit & Zoom Camera Controls**:
  - Add pointer listeners (`pointerdown`, `pointermove`, `pointerup`, `wheel`) supporting mouse drag rotation (yaw/pitch) and scroll wheel zoom.
  - Update `updateCamera` to position the camera using spherical orbit coordinates around the player character.
- **Rich Village Environment & Modular Prefab Detailing**:
  - Enrich `WorldRenderer` with village terrain styling: circular cobblestone plaza, dirt paths radiating to buildings, boundary foliage/trees, and lanterns.
  - Enrich `WorldObjectRenderer` with detailed procedural prefab geometries (pitched roofs, stone well rim with wooden canopy and pulley, forge chimney and anvil, merchant stall canopy).
  - Provide fallback/offline village environment layout so the scene is readable and inviting even before network packets arrive.

## 3. Non-Goals
- Modifying backend server authoritative physics tick rates or wire protocol opcodes.
- Replacing Three.js with an external rendering framework.
- Introducing credentials or secrets into the repository.
