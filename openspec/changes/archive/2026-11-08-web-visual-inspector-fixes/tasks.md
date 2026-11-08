# Implementation Tasks: Web Visual Inspector Fixes

## 1. Network & Dynamic Connection
- [x] 1.1 In `web/src/main.js` and `web/index.html`, dynamically default the gateway URL to `https://${window.location.hostname || "127.0.0.1"}:4433`.
- [x] 1.2 Enable auto-connection on page load after certificate hash auto-discovery completes with `WebTransport` feature detection, replacing `alert()` with non-blocking HUD notices.
- [x] 1.3 Assign unique guest tokens per session/tab using `sessionStorage` (e.g. `guest-<timestamp>-<rand>`) so concurrent browser tabs authenticate as distinct characters.
- [x] 1.4 Add state reset (`world.entities.clear()` and `renderer.clear()`) on transport disconnect/reconnect and guard `connect()` against concurrent execution.

## 2. Protocol & EntityDeltaBatch Binary Deserialization
- [x] 2.1 In `web/src/protocol.js`, implement helper functions for signed 16-bit dequantization (`dequantizeDeltaI16`, `dequantizeVelocityI16`, `dequantizeYawU8`, `dequantizePitchI8`) and encodeClientAck (Opcode 0x000A).
- [x] 2.2 In `web/src/main.js` (`processPacket`), implement full binary decoding for `OPCODES.ENTITY_DELTA_BATCH` (0x0009):
  - Read LEB128 varint `server_tick` and `baseline_tick`.
  - Read LEB128 varint `despawn_count` and process each despawned GUID from `this.entities`.
  - Read LEB128 varint `delta_count` and decode each `EntityDeltaState` (entity GUID, field mask, keyframe/delta positions, velocity, yaw, health).
  - Wrap decoding in a guarded `try/catch` to safely drop malformed datagrams without terminating `readDatagrams`.
  - Add or update remote entity replicas in `this.entities`.
- [x] 2.3 Send `ClientAckPacket` upon receiving server ticks to advance the authoritative confirmed baseline on the node.
- [x] 2.4 Update HUD live telemetry calculation in `main.js` and `ui.js`:
  - Accurately compute active entities (`this.entities.size + (connected ? 1 : 0)`).
  - Update `latest_server_tick`, `latest_baseline_tick`, `val-packets`, and `val-bandwidth` in real time.

## 3. Interactive Camera Orbit & Zoom Controls
- [x] 3.1 In `web/src/renderer.js` / `web/src/controls.js`, implement pointer listeners for mouse dragging (left/right button) and mouse wheel events on `canvasContainer`/`domElement`, ignoring clicks on `#hud-overlay` and modals.
- [x] 3.2 Implement spherical orbit camera mathematics (`orbitYaw`, `orbitPitch`, `distance`, `targetOffset`) in `WorldRenderer.updateCamera`.
- [x] 3.3 Clamp pitch between `[0.05, 1.45]` radians and distance between `[5.0, 60.0]` meters, ensuring smooth following of player movements without gimbal lock.
- [x] 3.4 Update HUD controls hint text to accurately describe camera orbit and zoom controls.

## 4. Village Scene Aesthetics & Composite Prefab Meshes
- [x] 4.1 In `web/src/renderer.js`, enhance village terrain with stylized ground planes, a central cobblestone plaza around the well, and radial pathways connecting the structures.
- [x] 4.2 Add village environmental detailing (decorative perimeter trees, lantern posts with warm lighting, low perimeter fences).
- [x] 4.3 In `web/src/world.js`, upgrade `createObjectMesh` to construct composite procedural 3D models for prefabs:
  - Well (stone cylinder base, wooden roof canopy, support poles, bucket).
  - Forge (stone hearth, chimney stack, glowing coal basin, anvil).
  - Stall (counter, striped fabric awning canopy, crates).
  - House (log/stone walls, pitched roof with overhanging eaves, chimney, door frame).

## 5. Verification, Tests, PR & Deployment
- [x] 5.1 Add unit tests in `web/src/__tests__/inspector.test.js` validating `EntityDeltaBatch` decoding, `ClientAck` encoding, dequantization math, and camera orbit calculations.
- [x] 5.2 Run quality gates: `npm test` and `npm run build` in `web/`; `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`.
- [ ] 5.3 Commit changes, create PR against `main`, and merge after CI approval.
- [ ] 5.4 Restart servers in Kubernetes (`kubectl rollout restart deployment mmo-web -n mmo`) and verify deployment.
- [ ] 5.5 Using Playwright Chromium, take screenshots of `http://49.13.149.126/`, interact with the live server independently, and verify telemetry, camera controls, multi-session visibility, and village aesthetics.
- [ ] 5.6 File GitHub issue(s) for future improvements.
