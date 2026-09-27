# Specification: Web Visual Inspector & Playtest Frontend

## Overview
A lightweight, zero-install browser frontend located in `web/` that connects to `mmo-gateway` via WebTransport. Uses Three.js for 3D rendering and calls `mmo-client-wasm` for packet decoding, interpolation, and state management.

---

## 1. Directory Structure

```
web/
├── index.html
├── style.css
├── package.json (Vite + Three.js)
└── src/
    ├── main.js (Application lifecycle and animation loop)
    ├── transport.js (WebTransport connection, certificate hashing, datagram loops)
    ├── renderer.js (Three.js scene, camera, meshes, partition grid)
    ├── controls.js (WASD input sampling and movement packet generation)
    └── ui.js (Developer HUD, telemetry overlay, health & cast bars)
```

---

## 2. Rendering Invariants (Simple Geometry First)

1. **Player Entity**: Blue capsule / cylinder with forward orientation pointer.
2. **Remote Players**: Green capsules with player name and health bar overhead.
3. **Hostile NPCs**: Red cubes with floating health bars.
4. **Partition Boundary Planes**: Semi-transparent colored vertical planes matching the server's spatial octree bounding box faces.
5. **AOI Frustum / Circle**: Wireframe circle on the ground indicating the client's current Area-of-Interest replication radius.

---

## 3. Input & Animation Loop (60/144 FPS)

```javascript
// main.js - animation frame
function animate(timestamp) {
  requestAnimationFrame(animate);
  
  const dt = clock.getDelta();
  
  // 1. Advance WASM interpolation buffer
  wasmWorld.update(dt);
  
  // 2. Sample WASD keyboard inputs and send 20 Hz movement
  if (movementTimer.tick(dt)) {
    const input = controls.sample();
    const packetBytes = wasmWorld.encode_movement(input.x, input.y, input.z, input.yaw, sequence++);
    transport.sendDatagram(packetBytes);
  }
  
  // 3. Update Three.js meshes from flat WASM render buffer
  const buffer = wasmWorld.get_render_buffer();
  renderer.syncEntities(buffer);
  
  // 4. Update HUD overlay
  ui.updateTelemetry(wasmWorld.get_telemetry_json());
}
```

---

## 4. Developer HUD & Telemetry Overlays
1. **Network Stats**:
   - Connection status (Connecting, Connected, Reconnecting).
   - RTT / Ping (ms).
   - Ingress bandwidth (KB/s) and packet loss percentage.
2. **Server State**:
   - Latest authoritative server tick.
   - Interpolation delay (ms).
   - Entities currently rendered in view.
3. **Debug Toggles**:
   - Toggle Partition Boundaries.
   - Toggle Bounding Boxes / Wireframes.
   - Toggle Flycam / God Mode vs Third-Person Player Follow.
