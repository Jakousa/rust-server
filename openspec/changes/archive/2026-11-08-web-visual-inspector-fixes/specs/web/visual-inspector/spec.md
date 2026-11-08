# Specification: Web Visual Inspector

## Requirements

### Requirement: Dynamic Gateway Connection & Session Isolation
The web frontend MUST dynamically construct the default gateway URL from `window.location.hostname` and assign unique guest session tokens.

#### Scenario: Default Gateway URL Discovery
- GIVEN the web client is served from a public host `49.13.149.126`
- WHEN the DOM loads
- THEN the default gateway URL input MUST default to `https://49.13.149.126:4433`
- AND the client MUST auto-fetch the certificate digest from `http://49.13.149.126:9091/cert-hash`
- AND if `WebTransport` is supported by the browser, the client MUST initiate connection automatically without blocking alerts.

#### Scenario: Multi-Session Character Independence
- GIVEN two distinct browser tabs are opened to the visual inspector
- WHEN both tabs perform WebTransport authentication
- THEN each tab MUST provide a unique authentication token from `sessionStorage` (`guest-<timestamp>-<rand>`)
- AND the gateway MUST assign distinct character GUIDs to each session.

---

### Requirement: Binary EntityDeltaBatch Decoding & Replica Sync
The client MUST decode `OPCODES.ENTITY_DELTA_BATCH` (0x0009) packets in strict compliance with the `mmo_schema` binary framing and acknowledge observed ticks with `ClientAckPacket` (Opcode 0x000A).

#### Scenario: Server Tick and Baseline Update
- GIVEN an `EntityDeltaBatch` packet carrying LEB128 varint `server_tick` and `baseline_tick`
- WHEN the client processes the packet
- THEN `latestServerTick` and `latestBaselineTick` MUST reflect the decoded ticks
- AND the client MUST transmit a `ClientAckPacket` carrying the observed server tick
- AND the server MUST advance its confirmed baseline tick.

#### Scenario: Remote Entity Keyframe and Delta Ingestion
- GIVEN an `EntityDeltaBatch` containing keyframe or delta displacement for a remote entity (type 1 player or type 2 NPC)
- WHEN the packet is ingested by `ClientWorldBridge`
- THEN the remote entity MUST be stored in `world.entities`
- AND its coordinates, yaw, and health ratio MUST be updated
- AND `WorldRenderer.syncEntities` MUST render the remote entity mesh in the 3D scene.

#### Scenario: Entity Despawn Processing
- GIVEN an `EntityDeltaBatch` containing entity GUIDs in its despawn list
- WHEN the packet is ingested
- THEN the entity MUST be removed from `world.entities`
- AND its 3D mesh MUST be removed from the Three.js scene with GPU resources disposed.

#### Scenario: Robust Error Recovery
- GIVEN an incomplete, truncated, or malformed datagram
- WHEN `ClientWorldBridge.processPacket` decodes the payload
- THEN any parsing error MUST be caught and dropped gracefully
- AND the datagram reader loop MUST NOT terminate.

---

### Requirement: Interactive Camera Controls (Orbit, Pitch, Zoom)
The client camera MUST respond to mouse pointer interactions without locking the view to a static offset, while ignoring clicks on HUD panels.

#### Scenario: Mouse Drag Orbit & Pitch
- GIVEN the player is interacting with the 3D viewport canvas
- WHEN the user drags the pointer across the canvas
- THEN the camera's azimuth (yaw) and elevation (pitch) angles MUST update proportionally
- AND elevation MUST be clamped between safe angles (`[0.05, 1.45]` radians) to prevent gimbal lock spin or ground clipping.

#### Scenario: Mouse Wheel Zoom
- GIVEN the player view is active
- WHEN the user scrolls the mouse wheel
- THEN the camera orbit distance MUST increase or decrease smoothly
- AND the distance MUST be clamped within `[5m, 60m]`.

#### Scenario: HUD Event Isolation
- GIVEN the user clicks or drags within a HUD panel or modal dialog
- THEN the camera angles and zoom MUST NOT change.

---

### Requirement: Village Environment & Prefab Detailing
The visual inspector MUST present a readable, aesthetically pleasing village environment both offline and online.

#### Scenario: Village Terrain & Layout
- GIVEN the 3D scene is initialized
- THEN the ground MUST feature a cobblestone central plaza with radial dirt pathways leading towards village buildings
- AND ambient decorative elements (lanterns with warm lighting, perimeter trees, boundary fencing) MUST frame the village perimeter.

#### Scenario: Detailed Prefab Meshes
- GIVEN world objects for prefabs 10 (well), 11 (forge), 12 (stall), and 13 (house) are spawned
- THEN the meshes MUST render composite geometry with distinct structural features (e.g., roof gables, well canopy, forge chimney, market awning) rather than untextured featureless cubes.
