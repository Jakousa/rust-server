import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import * as THREE from "three";
import {
  resolveGameMode,
  resolveGatewayConfig,
  ClientWorldBridge,
  ASTEROID_MODULE_ID,
} from "../main.js";
import { WebTransportClient } from "../transport.js";
import { UIController } from "../ui.js";
import { DevToolsController } from "../dev_tools.js";
import { OPCODES, DELTA_FLAGS, putVarintU64 } from "../protocol.js";
import { SpaceFlightControls } from "../games/asteroid/controls.js";
import {
  VOXEL_MODULE_ID,
  SUB_OPCODE_CHUNK_REQUEST,
  SUB_OPCODE_CHUNK_DATA,
  SUB_OPCODE_MODIFY_REQUEST,
  CHUNK_VOXELS,
  VOXEL_EMPTY,
  VOXEL_CRUST_ROCK,
  VOXEL_TITANIUM,
  VOXEL_SOLAR_GOLD,
  VOXEL_VOID_CRYSTAL,
  VOXEL_MATERIALS,
  SEEDED_ASTEROID_CHUNKS,
  getVoxelMaterialInfo,
  indexFromLocal,
  voxelCoordToChunk,
  encodeVoxelChunkRequest,
  encodeVoxelChunkRequestInteraction,
  encodeVoxelModifyRequest,
  encodeVoxelModifyInteraction,
  decodeRleVoxelChunk,
  decodeVoxelChunkData,
  decodeVoxelModifyResponse,
  buildChunkBufferGeometry,
  AsteroidVoxelClient,
} from "../games/asteroid/voxel_client.js";
import {
  HOME_STATION_POS,
  HOME_STATION_SAFE_RADIUS,
  extractShininessTier,
  computeShipShaderProps,
  AsteroidRenderer,
} from "../games/asteroid/renderer.js";

function encodeTestVxr1(spans, revision = 1n) {
  const buf = new Uint8Array(16 + spans.length * 4);
  const view = new DataView(buf.buffer);
  buf[0] = 0x56; // V
  buf[1] = 0x58; // X
  buf[2] = 0x52; // R
  buf[3] = 0x31; // 1
  view.setBigUint64(4, BigInt(revision), true);
  view.setUint32(12, spans.length, true);
  let off = 16;
  for (const [count, voxelId] of spans) {
    view.setUint16(off, count, true);
    view.setUint16(off + 2, voxelId, true);
    off += 4;
  }
  return buf;
}

function buildInteractionResultFrame(actionId, payloadBytes, success = true, errorCode = 0) {
  const payload =
    payloadBytes instanceof Uint8Array ? payloadBytes : new Uint8Array(payloadBytes || 0);
  const frame = new Uint8Array(8 + 13 + payload.byteLength);
  const view = new DataView(frame.buffer);
  frame[0] = 0x4d;
  frame[1] = 0x4d;
  view.setUint16(2, OPCODES.INTERACTION_RESULT, true);
  view.setUint32(4, 13 + payload.byteLength, true);
  view.setUint32(8, actionId >>> 0, true);
  frame[12] = success ? 1 : 0;
  view.setUint32(13, errorCode >>> 0, true);
  view.setUint32(17, payload.byteLength, true);
  frame.set(payload, 21);
  return frame;
}

describe("Dual-Game Mode & Subpath Resolution", () => {
  it("resolves ?game= query parameter over URL subpath and envMode, defaulting to village", () => {
    expect(resolveGameMode("", "")).toBe("village");
    expect(resolveGameMode("", "asteroid")).toBe("asteroid");
    expect(resolveGameMode("?game=asteroid", "village")).toBe("asteroid");
    expect(resolveGameMode("?game=village", "asteroid")).toBe("village");
    expect(resolveGameMode("?foo=1&game=ASTEROID", "")).toBe("asteroid");
    expect(resolveGameMode("game=asteroid", "village")).toBe("asteroid");
    expect(resolveGameMode("?game=invalid", "asteroid")).toBe("asteroid");
    expect(resolveGameMode("?game=invalid", "invalid")).toBe("village");

    // Subpath routing resolution (/village, /asteroid)
    expect(resolveGameMode("", "", "/asteroid")).toBe("asteroid");
    expect(resolveGameMode("", "", "/asteroid/")).toBe("asteroid");
    expect(resolveGameMode("", "", "/asteroids")).toBe("asteroid");
    expect(resolveGameMode("", "", "/village")).toBe("village");
    expect(resolveGameMode("", "asteroid", "/village/")).toBe("village");
    expect(resolveGameMode("?game=village", "asteroid", "/asteroid")).toBe("village");
    expect(resolveGameMode("?game=asteroid", "village", "/village")).toBe("asteroid");
    expect(resolveGameMode("", "asteroid", "/unknown")).toBe("asteroid");
  });

  it("resolves distinct WebTransport, WebSocket subpath, and cert-hash endpoints per game", () => {
    const loc = { hostname: "rust-server.kousa.dev", host: "rust-server.kousa.dev", protocol: "https:" };
    expect(resolveGatewayConfig("village", loc)).toEqual({
      mode: "village",
      subpath: "/village",
      wtUrl: "https://rust-server.kousa.dev:4433",
      wsPathUrl: "wss://rust-server.kousa.dev/village/ws",
      wsPortUrl: "wss://rust-server.kousa.dev:8443",
      wsPort: 8443,
      adminPort: 9091,
      certHashPaths: ["/village/cert-hash", "/cert-hash"],
    });
    expect(resolveGatewayConfig("asteroid", loc)).toEqual({
      mode: "asteroid",
      subpath: "/asteroid",
      wtUrl: "https://rust-server.kousa.dev:4434",
      wsPathUrl: "wss://rust-server.kousa.dev/asteroid/ws",
      wsPortUrl: "wss://rust-server.kousa.dev:8444",
      wsPort: 8444,
      adminPort: 9092,
      certHashPaths: ["/asteroid/cert-hash"],
    });
  });
});

describe("SpaceFlightControls (6DoF)", () => {
  it("initializes at station spawn (0, 0, 8) with speed headroom below 10.0 m/s", () => {
    const controls = new SpaceFlightControls();
    expect(controls.position).toEqual({ x: 0.0, y: 0.0, z: 8.0 });
    expect(controls.maxSpeed).toBeGreaterThan(0);
    expect(controls.maxSpeed).toBeLessThan(10.0);
  });

  it("thrusts along 3D yaw/pitch, strafes X/Z, thrusts vertically Y, and clamps maxSpeed", () => {
    const controls = new SpaceFlightControls({ x: 0, y: 0, z: 0, yaw: 0, pitch: 0 });

    // W along yaw=0, pitch=0 thrusts +Z
    controls.keys.w = true;
    controls.update(0.2);
    expect(controls.position.z).toBeGreaterThan(0);
    expect(controls.position.x).toBeCloseTo(0, 5);
    expect(controls.position.y).toBeCloseTo(0, 5);
    controls.keys.w = false;

    // Reset and pitch up +PI/4: W moves both +Y and +Z
    controls.setPosition(0, 0, 0);
    controls.velocity = { x: 0, y: 0, z: 0 };
    controls.setOrientation(0, Math.PI / 4);
    controls.keys.w = true;
    controls.update(0.2);
    expect(controls.position.y).toBeGreaterThan(0);
    expect(controls.position.z).toBeGreaterThan(0);
    expect(controls.position.y).toBeCloseTo(controls.position.z, 4);
    controls.keys.w = false;

    // Strafe A/D moves laterally along X at yaw=0
    controls.setPosition(0, 0, 0);
    controls.velocity = { x: 0, y: 0, z: 0 };
    controls.setOrientation(0, 0);
    controls.keys.d = true;
    controls.update(0.2);
    expect(controls.position.x).toBeLessThan(0); // +Z forward => right is -X
    controls.keys.d = false;

    // Space / Shift vertical thrust on Y
    controls.setPosition(0, 0, 0);
    controls.velocity = { x: 0, y: 0, z: 0 };
    controls.keys.space = true;
    for (let i = 0; i < 20; i++) controls.update(0.05);
    expect(controls.position.y).toBeGreaterThan(1.0);
    expect(Math.hypot(controls.velocity.x, controls.velocity.y, controls.velocity.z)).toBeLessThanOrEqual(
      controls.maxSpeed + 1e-5
    );
    controls.keys.space = false;

    controls.keys.shift = true;
    for (let i = 0; i < 40; i++) controls.update(0.05);
    expect(controls.velocity.y).toBeLessThan(0);
  });

  it("samples sequence monotonically, reconciles corrections, and isolates editable inputs", () => {
    const controls = new SpaceFlightControls();
    const mineSpy = vi.fn();
    const laserSpy = vi.fn();
    controls.onMine(mineSpy);
    controls.onFireLaser(laserSpy);

    // Editable input targets must not trigger flight or weapon keys
    controls.setKeyState({ key: "w", code: "KeyW", target: { tagName: "INPUT" } }, true);
    controls.setKeyState({ key: "j", code: "KeyJ", target: { tagName: "TEXTAREA" } }, true);
    controls.setKeyState({ key: "Enter", code: "Enter", target: { tagName: "SELECT" } }, true);
    expect(controls.keys.w).toBe(false);
    expect(mineSpy).not.toHaveBeenCalled();
    expect(laserSpy).not.toHaveBeenCalled();

    // Active canvas triggers mine & laser hooks
    controls.setKeyState({ key: "j", code: "KeyJ", target: { tagName: "CANVAS" } }, true);
    controls.setKeyState({ key: "Enter", code: "Enter", target: { tagName: "CANVAS" } }, true);
    expect(mineSpy).toHaveBeenCalledTimes(1);
    expect(laserSpy).toHaveBeenCalledTimes(1);

    const s1 = controls.sample();
    const s2 = controls.sample();
    expect(s1.sequence).toBe(1);
    expect(s2.sequence).toBe(2);
    expect(typeof s1.pitch).toBe("number");

    controls.reconcile(10, 1.0, 2.0, 3.0, 0.5, -0.5, 0.0);
    expect(controls.position).toEqual({ x: 1.0, y: 2.0, z: 3.0 });
    expect(controls.velocity).toEqual({ x: 0.5, y: -0.5, z: 0.0 });
    expect(controls.sequence).toBe(11);
  });
});

describe("AsteroidVoxelClient & 0x00A0 Voxel Binary Wire Protocol", () => {
  it("encodes VoxelChunkRequest (0x00A0|1) and VoxelModifyRequest (0x00A0|4) matching Rust layout", () => {
    const reqBytes = encodeVoxelChunkRequest(-2, 1, -1);
    expect(reqBytes.byteLength).toBe(12);
    const rView = new DataView(reqBytes.buffer);
    expect(rView.getInt32(0, true)).toBe(-2);
    expect(rView.getInt32(4, true)).toBe(1);
    expect(rView.getInt32(8, true)).toBe(-1);

    const reqFrame = encodeVoxelChunkRequestInteraction(999n, 0, 0, -2);
    const fView = new DataView(reqFrame.buffer);
    expect(fView.getUint16(2, true)).toBe(OPCODES.INTERACTION);
    expect(fView.getUint32(25, true)).toBe(((VOXEL_MODULE_ID << 16) | SUB_OPCODE_CHUNK_REQUEST) >>> 0);
    expect(fView.getUint32(29, true)).toBe(12);

    const modBytes = encodeVoxelModifyRequest(-17, 5, -30, VOXEL_EMPTY);
    expect(modBytes.byteLength).toBe(26);
    const mView = new DataView(modBytes.buffer);
    expect(Number(mView.getBigInt64(0, true))).toBe(-17);
    expect(Number(mView.getBigInt64(8, true))).toBe(5);
    expect(Number(mView.getBigInt64(16, true))).toBe(-30);
    expect(mView.getUint16(24, true)).toBe(0);

    const modFrame = encodeVoxelModifyInteraction(999n, 1, 2, 3, 0);
    const mfView = new DataView(modFrame.buffer);
    expect(mfView.getUint32(25, true)).toBe(((VOXEL_MODULE_ID << 16) | SUB_OPCODE_MODIFY_REQUEST) >>> 0);
  });

  it("decodes VXR1 RLE chunks and rejects decompression bombs, underflows, and bad magic", () => {
    const validRle = encodeTestVxr1(
      [
        [4000, VOXEL_CRUST_ROCK],
        [90, VOXEL_TITANIUM],
        [5, VOXEL_SOLAR_GOLD],
        [1, VOXEL_VOID_CRYSTAL],
      ],
      42n
    );
    const decoded = decodeRleVoxelChunk(validRle);
    expect(decoded.revision).toBe(42);
    expect(decoded.voxels.length).toBe(CHUNK_VOXELS);
    expect(decoded.voxels[0]).toBe(VOXEL_CRUST_ROCK);
    expect(decoded.voxels[4000]).toBe(VOXEL_TITANIUM);
    expect(decoded.voxels[4090]).toBe(VOXEL_SOLAR_GOLD);
    expect(decoded.voxels[4095]).toBe(VOXEL_VOID_CRYSTAL);

    // Bomb (> 4096)
    expect(() => decodeRleVoxelChunk(encodeTestVxr1([[5000, 1]]))).toThrow(/exceeds 4096/);
    // Underflow (< 4096)
    expect(() => decodeRleVoxelChunk(encodeTestVxr1([[4000, 1]]))).toThrow(/expected 4096/);
    // Bad magic
    const badMagic = new Uint8Array(validRle);
    badMagic[0] = 0x42;
    expect(() => decodeRleVoxelChunk(badMagic)).toThrow(/magic/);
  });

  it("forwards rawBytes in ClientWorldBridge INTERACTION_RESULT and updates voxel client state", () => {
    const bridge = new ClientWorldBridge();
    const voxelClient = new AsteroidVoxelClient();

    // 1. Chunk response payload: 12B chunk (-1, 0, -2) + VXR1
    const rle = encodeTestVxr1([
      [4095, VOXEL_EMPTY],
      [1, VOXEL_SOLAR_GOLD],
    ], 7n);
    const chunkPayload = new Uint8Array(12 + rle.length);
    const cView = new DataView(chunkPayload.buffer);
    cView.setInt32(0, -1, true);
    cView.setInt32(4, 0, true);
    cView.setInt32(8, -2, true);
    chunkPayload.set(rle, 12);

    const chunkFrame = buildInteractionResultFrame(
      (VOXEL_MODULE_ID << 16) | SUB_OPCODE_CHUNK_REQUEST,
      chunkPayload
    );

    const events = [];
    bridge.onInteractionResult((res) => {
      events.push(res);
      voxelClient.handleInteractionResult(res.actionId, res.rawBytes);
    });

    bridge.processPacket(chunkFrame);
    expect(events).toHaveLength(1);
    expect(events[0].rawBytes.byteLength).toBe(chunkPayload.byteLength);

    // Voxel at local (15, 15, 15) in chunk (-1, 0, -2) => world (-1, 15, -17)
    expect(voxelClient.getVoxel(-1, 15, -17)).toBe(VOXEL_SOLAR_GOLD);

    // 2. Modify response payload: status=0 + (-1, 15, -17) + new_voxel=0
    const modPayload = new Uint8Array(27);
    const mView = new DataView(modPayload.buffer);
    modPayload[0] = 0; // Success
    mView.setBigInt64(1, -1n, true);
    mView.setBigInt64(9, 15n, true);
    mView.setBigInt64(17, -17n, true);
    mView.setUint16(25, VOXEL_EMPTY, true);

    const modFrame = buildInteractionResultFrame(
      (VOXEL_MODULE_ID << 16) | SUB_OPCODE_MODIFY_REQUEST,
      modPayload
    );
    bridge.processPacket(modFrame);
    expect(voxelClient.getVoxel(-1, 15, -17)).toBe(VOXEL_EMPTY);
  });

  it("builds face-culled BufferGeometry with exact material colors and boundary neighbor culling", () => {
    // Empty chunk => 0 vertices
    const emptyGeo = buildChunkBufferGeometry(new Uint16Array(CHUNK_VOXELS));
    expect(emptyGeo.getAttribute("position").count).toBe(0);

    // Isolated block => 6 faces * 6 vertices = 36 vertices
    const single = new Uint16Array(CHUNK_VOXELS);
    single[indexFromLocal(5, 5, 5)] = VOXEL_VOID_CRYSTAL;
    const singleGeo = buildChunkBufferGeometry(single);
    expect(singleGeo.getAttribute("position").count).toBe(36);
    const cAttr = singleGeo.getAttribute("color");
    const expectedVoid = new THREE.Color(VOXEL_MATERIALS[VOXEL_VOID_CRYSTAL].hex);
    expect(cAttr.getX(0)).toBeCloseTo(expectedVoid.r, 4);
    expect(cAttr.getY(0)).toBeCloseTo(expectedVoid.g, 4);
    expect(cAttr.getZ(0)).toBeCloseTo(expectedVoid.b, 4);

    // 2x2x2 solid cube => 24 outer faces * 6 vertices = 144 vertices (internal faces culled)
    const cube = new Uint16Array(CHUNK_VOXELS);
    for (let x = 2; x <= 3; x++) {
      for (let y = 2; y <= 3; y++) {
        for (let z = 2; z <= 3; z++) {
          cube[indexFromLocal(x, y, z)] = VOXEL_SOLAR_GOLD;
        }
      }
    }
    const cubeGeo = buildChunkBufferGeometry(cube);
    expect(cubeGeo.getAttribute("position").count).toBe(144);

    // Cross-chunk neighbor culling: two touching border voxels at x=15 and neighbor x=0
    const leftChunk = new Uint16Array(CHUNK_VOXELS);
    leftChunk[indexFromLocal(15, 4, 4)] = VOXEL_CRUST_ROCK;
    const rightChunk = new Uint16Array(CHUNK_VOXELS);
    rightChunk[indexFromLocal(0, 4, 4)] = VOXEL_TITANIUM;

    const unculledGeo = buildChunkBufferGeometry(leftChunk, null);
    expect(unculledGeo.getAttribute("position").count).toBe(36);

    const culledGeo = buildChunkBufferGeometry(leftChunk, { posX: rightChunk });
    expect(culledGeo.getAttribute("position").count).toBe(30); // +X face culled!
  });

  it("raycasts first solid voxel along aim ray and finds nearest ore block within reach", () => {
    const client = new AsteroidVoxelClient();
    const voxels = new Uint16Array(CHUNK_VOXELS);
    voxels[indexFromLocal(2, 0, 5)] = VOXEL_CRUST_ROCK;
    voxels[indexFromLocal(2, 0, 6)] = VOXEL_VOID_CRYSTAL;
    client.setChunk(0, 0, 0, voxels, 1);

    const hit = client.raycastFirstSolid({ x: 2.5, y: 0.5, z: 0.5 }, { x: 0, y: 0, z: 1 }, 10.0);
    expect(hit).not.toBeNull();
    expect(hit.x).toBe(2);
    expect(hit.y).toBe(0);
    expect(hit.z).toBe(5);
    expect(hit.voxelId).toBe(VOXEL_CRUST_ROCK);

    const nearestOre = client.findNearestSolidVoxel({ x: 2.5, y: 0.5, z: 4.0 }, 5.0, true);
    expect(nearestOre).not.toBeNull();
    expect(nearestOre.voxelId).toBe(VOXEL_VOID_CRYSTAL);

    expect(voxelCoordToChunk(-1, -1, -1)).toEqual({
      chunk: { x: -1, y: -1, z: -1 },
      lx: 15,
      ly: 15,
      lz: 15,
      index: 4095,
    });
  });
});

describe("AsteroidRenderer & Loot-Shininess Shaders", () => {
  it("extracts bits 16..23 from stateFlags and scales shader metalness/emissive/aura", () => {
    expect(extractShininessTier(0)).toBe(0);
    expect(extractShininessTier(0x00ff0000)).toBe(255);
    expect(extractShininessTier(0xaa7f0001)).toBe(0x7f);

    const p0 = computeShipShaderProps(0);
    const pLow = computeShipShaderProps(20 << 16);
    const pMid = computeShipShaderProps(120 << 16);
    const pMax = computeShipShaderProps(255 << 16);

    expect(p0.tier).toBe(0);
    expect(p0.auraIntensity).toBe(0);
    expect(pLow.tier).toBe(20);
    expect(pLow.emissiveColor).toBe(0x38bdf8);
    expect(pMid.tier).toBe(120);
    expect(pMid.emissiveColor).toBe(0xfacc15);
    expect(pMax.tier).toBe(255);
    expect(pMax.emissiveColor).toBe(0xfef08a);
    expect(pMax.auraColor).toBe(0xc084fc);

    expect(pMax.metalness).toBeGreaterThan(pMid.metalness);
    expect(pMid.metalness).toBeGreaterThan(p0.metalness);
    expect(pMax.emissiveIntensity).toBeGreaterThan(pMid.emissiveIntensity);
    expect(pMax.auraIntensity).toBeGreaterThan(pLow.auraIntensity);
  });

  it("initializes space environment with Orbital Station safe zone at (0,0,0) radius 15 and updates ship shininess", () => {
    expect(HOME_STATION_POS).toEqual({ x: 0, y: 0, z: 0 });
    expect(HOME_STATION_SAFE_RADIUS).toBe(15.0);

    const renderer = new AsteroidRenderer(null);
    renderer.initEnvironment();
    expect(renderer.starfield).toBeDefined();
    expect(renderer.stationGroup).toBeDefined();
    expect(renderer.safeZoneSphere).toBeDefined();
    expect(renderer.safeZoneRing).toBeDefined();

    // Sync local + shiny remote ship from ClientWorldBridge EntityDeltaBatch
    const bridge = new ClientWorldBridge();
    bridge.localPlayerGuid = 1;
    const rivalGuid = (1n << 48n) | 42n;

    const payload = [];
    putVarintU64(10, payload); // tick
    putVarintU64(9, payload); // baseline
    payload.push(0); // 0 despawns
    payload.push(1); // 1 delta
    putVarintU64(rivalGuid, payload);
    const mask = DELTA_FLAGS.MASK_KEYFRAME | DELTA_FLAGS.MASK_STATE_FLAGS | DELTA_FLAGS.MASK_HEALTH;
    payload.push(mask & 0xff, (mask >> 8) & 0xff);
    const fBuf = new ArrayBuffer(12);
    const fView = new DataView(fBuf);
    fView.setFloat32(0, 12.0, true);
    fView.setFloat32(4, 3.0, true);
    fView.setFloat32(8, -20.0, true);
    new Uint8Array(fBuf).forEach((b) => payload.push(b));
    payload.push(75); // health
    putVarintU64(200 << 16, payload); // state_flags tier 200

    const batchFrame = new Uint8Array(8 + payload.length);
    const bView = new DataView(batchFrame.buffer);
    batchFrame[0] = 0x4d;
    batchFrame[1] = 0x4d;
    bView.setUint16(2, OPCODES.ENTITY_DELTA_BATCH, true);
    bView.setUint32(4, payload.length, true);
    batchFrame.set(payload, 8);

    bridge.processPacket(batchFrame);
    const rivalEnt = bridge.entities.get(Number(rivalGuid));
    expect(extractShininessTier(rivalEnt.stateFlags)).toBe(200);

    const renderBuf = bridge.getRenderBuffer({ x: 0, y: 0, z: 8 }, Math.PI);
    renderer.syncEntities(renderBuf, bridge.localPlayerGuid, bridge.entities);

    const rivalMesh = renderer.entities.get(rivalGuid);
    expect(rivalMesh).toBeDefined();
    expect(rivalMesh.userData.shininessTier).toBe(200);
    expect(rivalMesh.userData.auraHalo.visible).toBe(true);
    expect(rivalMesh.userData.auraLight.intensity).toBeGreaterThan(1.0);

    // Sync voxel chunks and laser beam
    const voxelClient = new AsteroidVoxelClient();
    const voxels = new Uint16Array(CHUNK_VOXELS);
    voxels[indexFromLocal(0, 0, 0)] = VOXEL_TITANIUM;
    voxelClient.setChunk(0, 0, -2, voxels, 1);

    renderer.syncVoxelChunks(voxelClient);
    expect(renderer.voxelChunkMeshes.has("0,0,-2")).toBe(true);
    expect(renderer.voxelChunkMeshes.get("0,0,-2").position.z).toBe(-32);

    const beam = renderer.spawnLaserBeam({ x: 0, y: 0, z: 8 }, { x: 0, y: 0, z: -20 }, 0x38bdf8, 200);
    expect(beam).not.toBeNull();
    expect(renderer.laserBeams).toHaveLength(1);

    renderer.clear();
    expect(renderer.entities.size).toBe(0);
    expect(renderer.voxelChunkMeshes.size).toBe(0);
    expect(renderer.laserBeams.length).toBe(0);
  });
});

describe("UIController Game Mode Switcher & Asteroid HUD", () => {
  let elements;

  function createMockElement(id = "", className = "") {
    const el = {
      id,
      className,
      style: {},
      children: [],
      textContent: "",
      value: "",
      disabled: false,
      dataset: {},
      _listeners: {},
      replaceChildren(...newChildren) {
        this.children = [...newChildren];
      },
      appendChild(child) {
        this.children.push(child);
      },
      addEventListener(evt, fn) {
        this._listeners[evt] = fn;
      },
      click() {
        if (this._listeners.click) this._listeners.click({ target: this });
      },
    };
    el.classList = {
      add(...tokens) {
        const s = new Set((el.className || "").split(/\s+/).filter(Boolean));
        tokens.forEach((t) => s.add(t));
        el.className = Array.from(s).join(" ");
      },
      remove(...tokens) {
        const s = new Set((el.className || "").split(/\s+/).filter(Boolean));
        tokens.forEach((t) => s.delete(t));
        el.className = Array.from(s).join(" ");
      },
      contains(token) {
        return (el.className || "").split(/\s+/).filter(Boolean).includes(token);
      },
    };
    return el;
  }

  beforeEach(() => {
    elements = new Map();
    globalThis.document = {
      title: "",
      body: { dataset: {} },
      getElementById(id) {
        if (!elements.has(id)) {
          elements.set(id, createMockElement(id));
        }
        return elements.get(id);
      },
      createElement() {
        return createMockElement();
      },
    };
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("toggles Village vs Asteroid HUD panels and fires game mode + asteroid action callbacks", () => {
    const ui = new UIController();
    const modeChanges = [];
    const mineSpy = vi.fn();
    const bankSpy = vi.fn();
    const laserSpy = vi.fn();
    const tradeSpy = vi.fn();

    ui.initEvents({
      onGameModeChange: (m) => modeChanges.push(m),
      onMineAsteroid: mineSpy,
      onBankLoot: bankSpy,
      onFireLaser: laserSpy,
      onTradePilot: tradeSpy,
    });

    ui.setGameMode("asteroid");
    expect(document.getElementById("village-hud").style.display).toBe("none");
    expect(document.getElementById("asteroid-hud").style.display).toBe("flex");
    expect(document.getElementById("btn-mode-asteroid").classList.contains("active")).toBe(true);
    expect(document.getElementById("btn-mode-village").classList.contains("active")).toBe(false);
    expect(document.getElementById("controls-hint-title").textContent).toContain("6DoF");
    expect(document.body.dataset.gameMode).toBe("asteroid");

    document.getElementById("btn-mode-village").click();
    document.getElementById("btn-mode-asteroid").click();
    expect(modeChanges).toEqual(["village", "asteroid"]);

    document.getElementById("btn-mine-asteroid").click();
    document.getElementById("btn-bank-loot").click();
    document.getElementById("btn-fire-laser").click();
    document.getElementById("btn-trade-pilot").click();
    expect(mineSpy).toHaveBeenCalledTimes(1);
    expect(bankSpy).toHaveBeenCalledTimes(1);
    expect(laserSpy).toHaveBeenCalledTimes(1);
    expect(tradeSpy).toHaveBeenCalledTimes(1);
  });

  it("renders AsteroidSectorStateResponse hull, shininess, cargo, vault, and target ship safely", () => {
    const ui = new UIController();
    const xssOre = "<img src=x onerror=alert(1)>";

    ui.updateAsteroidState({
      player_guid: 1001,
      hull_hp: 25,
      max_hull_hp: 100,
      shininess_tier: 195,
      unbanked_cargo_value: 975,
      current_weight: 14.0,
      max_weight: 150.0,
      in_safe_zone: false,
      inventory: {
        currency: 0,
        max_weight: 150.0,
        slots: [
          { item_id: 2003, name: "Void Crystal", quantity: 10, unit_weight: 1.0 },
          { item_id: 9999, name: xssOre, quantity: 1, unit_weight: 4.0 },
        ],
      },
      station_vault: {
        banked_credits: 1450,
        banked_items: [{ item_id: 2002, name: "Solar Gold", quantity: 12, unit_weight: 3.0 }],
        kills: 3,
        deaths: 1,
      },
    });

    expect(document.getElementById("asteroid-hull-text").textContent).toBe("25 / 100 HP");
    expect(document.getElementById("asteroid-hull-bar").style.width).toBe("25.0%");
    expect(document.getElementById("asteroid-hull-bar").classList.contains("low")).toBe(true);
    expect(document.getElementById("asteroid-shininess-text").textContent).toBe(
      "Tier 195 / 255 (Void Beacon)"
    );
    expect(document.getElementById("asteroid-safe-zone-badge").textContent).toBe("PVP OPEN SPACE");
    expect(document.getElementById("btn-bank-loot").disabled).toBe(true);
    expect(document.getElementById("asteroid-unbanked-value").textContent).toBe("975");
    expect(document.getElementById("asteroid-cargo-weight").textContent).toBe("14.0 / 150.0 kg");
    expect(document.getElementById("asteroid-vault-credits").textContent).toBe("1450");
    expect(document.getElementById("asteroid-kd-stats").textContent).toBe("Kills: 3 | Deaths: 1");

    const cargoGrid = document.getElementById("asteroid-cargo-grid");
    expect(cargoGrid.children).toHaveLength(2);
    expect(cargoGrid.children[1].children[0].textContent).toBe(xssOre);

    const vaultList = document.getElementById("asteroid-vault-items");
    expect(vaultList.children).toHaveLength(1);
    expect(vaultList.children[0].textContent).toBe("12x Solar Gold");

    ui.showAsteroidTarget({ id: 42, distance: 18.4, hpPct: 0.5, stateFlags: 120 << 16 }, false);
    expect(document.getElementById("asteroid-target-panel").style.display).toBe("block");
    expect(document.getElementById("asteroid-target-name").textContent).toBe("Pilot #42");
    expect(document.getElementById("asteroid-target-shininess").textContent).toBe("Tier 120");
    expect(document.getElementById("asteroid-target-distance").textContent).toBe("18.4 m");
    expect(document.getElementById("asteroid-target-hull").textContent).toBe("50%");
    expect(document.getElementById("btn-fire-laser").disabled).toBe(false);

    // Safe zone disables Fire PvP Laser button and enables Dock & Bank button
    ui.setSafeZoneStatus(true);
    ui.showAsteroidTarget({ id: 42, distance: 5.0, hpPct: 1.0, stateFlags: 0 }, true);
    expect(document.getElementById("btn-bank-loot").disabled).toBe(false);
    expect(document.getElementById("btn-fire-laser").disabled).toBe(true);
  });

  it("supports shared Player Trade modal item offers and ASTEROID_MODULE_ID = 0x0003", () => {
    expect(ASTEROID_MODULE_ID).toBe(0x0003);
    const ui = new UIController();
    const offered = [];
    ui.initEvents({
      onTradeOffer: (currency, items) => offered.push({ currency, items }),
    });

    ui.updateAsteroidState({
      hull_hp: 100,
      max_hull_hp: 100,
      inventory: {
        slots: [{ item_id: 2001, name: "Titanium Ore", quantity: 4, unit_weight: 2.0 }],
      },
    });

    ui.showPlayerTrade({
      session_id: 77,
      my_offer: { currency: 0, items: [] },
      their_offer: {
        currency: 50,
        items: [{ item_id: 2002, name: "Solar Gold", quantity: 2, unit_weight: 3.0 }],
      },
      my_locked: false,
      their_locked: true,
    });

    expect(document.getElementById("trade-partner-items").children[0].textContent).toBe(
      "2x Solar Gold"
    );

    // Toggle Titanium Ore offer chip and submit offer
    const myItemsContainer = document.getElementById("trade-my-items");
    expect(myItemsContainer.children).toHaveLength(1);
    const toggleBtn = myItemsContainer.children[0].children[1];
    toggleBtn.click();
    document.getElementById("trade-offer-gold").value = "30";
    document.getElementById("btn-trade-offer").click();

    expect(offered).toEqual([
      {
        currency: 30,
        items: [{ item_id: 2001, name: "Titanium Ore", quantity: 4, unit_weight: 2.0 }],
      },
    ]);
  });
});

describe("EntityState Shininess & DeltaBatch Voxel Updates", () => {
  it("decodes pitch and stateFlags from OPCODES.ENTITY_STATE and dynamically upgrades ship shininess", () => {
    const bridge = new ClientWorldBridge();
    bridge.localPlayerGuid = 1;
    const remoteGuid = (1n << 48n) | 88n;

    const makeEntityState = (pitchVal, flagsVal) => {
      const frame = new Uint8Array(68);
      const view = new DataView(frame.buffer);
      frame[0] = 0x4d;
      frame[1] = 0x4d;
      view.setUint16(2, OPCODES.ENTITY_STATE, true);
      view.setUint32(4, 60, true);
      view.setBigUint64(8, remoteGuid, true);
      view.setFloat32(16, 20.0, true); // x
      view.setFloat32(20, 5.0, true); // y
      view.setFloat32(24, -10.0, true); // z
      view.setFloat32(40, 1.0, true); // yaw
      view.setFloat32(44, pitchVal, true); // pitch
      view.setUint32(48, 75, true); // health
      view.setUint32(52, 100, true); // max_health
      view.setUint32(56, flagsVal, true); // state_flags
      return frame;
    };

    bridge.processPacket(makeEntityState(0.35, 0));
    const renderer = new AsteroidRenderer(null);
    const buf1 = bridge.getRenderBuffer({ x: 0, y: 0, z: 8 }, Math.PI);
    renderer.syncEntities(buf1, bridge.localPlayerGuid, bridge.entities);

    const ship = renderer.entities.get(remoteGuid);
    expect(ship).toBeDefined();
    expect(ship.userData.shininessTier).toBe(0);
    expect(ship.userData.auraHalo.visible).toBe(false);
    expect(ship.rotation.x).toBeCloseTo(-0.35, 4);

    // Upgrade stateFlags to tier 220 via subsequent EntityState
    bridge.processPacket(makeEntityState(-0.25, 220 << 16));
    const buf2 = bridge.getRenderBuffer({ x: 0, y: 0, z: 8 }, Math.PI);
    renderer.syncEntities(buf2, bridge.localPlayerGuid, bridge.entities);
    expect(ship.userData.shininessTier).toBe(220);
    expect(ship.userData.auraHalo.visible).toBe(true);
    expect(ship.userData.hullMat.emissive.getHex()).toBe(0xfef08a);
    expect(ship.rotation.x).toBeCloseTo(0.25, 4);
  });

  it("applies VoxelDeltaBatch (0x00A0|3) and linear damping + yaw/pitch controls", () => {
    const client = new AsteroidVoxelClient();
    const voxels = new Uint16Array(CHUNK_VOXELS);
    voxels[indexFromLocal(1, 2, 3)] = VOXEL_CRUST_ROCK;
    client.setChunk(0, 0, -1, voxels, 1);
    client.dirtyMeshes.clear();

    // Build VoxelDeltaBatch: chunk(0, 0, -1), rev 2, 1 delta (1, 2, 3 -> 0)
    const payload = new Uint8Array(22 + 5);
    const view = new DataView(payload.buffer);
    view.setInt32(0, 0, true);
    view.setInt32(4, 0, true);
    view.setInt32(8, -1, true);
    view.setBigUint64(12, 2n, true);
    view.setUint16(20, 1, true);
    payload[22] = 1;
    payload[23] = 2;
    payload[24] = 3;
    view.setUint16(25, VOXEL_EMPTY, true);

    const res = client.handleInteractionResult((VOXEL_MODULE_ID << 16) | 3, payload);
    expect(res.type).toBe("delta_batch");
    expect(client.getVoxel(1, 2, -16 + 3)).toBe(VOXEL_EMPTY);
    expect(client.dirtyMeshes.has("0,0,-1")).toBe(true);

    // SpaceFlightControls coasting damping & orientation clamping
    const ctrl = new SpaceFlightControls({ x: 0, y: 0, z: 0, yaw: 0, pitch: 0 });
    ctrl.keys.w = true;
    ctrl.update(0.1);
    const speedAfterThrust = Math.hypot(ctrl.velocity.x, ctrl.velocity.y, ctrl.velocity.z);
    expect(speedAfterThrust).toBeGreaterThan(0);

    ctrl.keys.w = false;
    ctrl.update(0.2);
    const speedAfterCoast = Math.hypot(ctrl.velocity.x, ctrl.velocity.y, ctrl.velocity.z);
    expect(speedAfterCoast).toBeLessThan(speedAfterThrust);

    ctrl.keys.arrowUp = true;
    ctrl.update(5.0);
    expect(ctrl.pitch).toBeCloseTo(1.45, 5);
  });
});

describe("WebTransportClient.sendStream Serialization & Recovery", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("serializes concurrent sendStream calls onto a single bidirectional stream in order", async () => {
    const client = new WebTransportClient();
    const written = [];
    const writer = {
      write: vi.fn(async (data) => {
        await Promise.resolve();
        written.push(data);
      }),
    };

    client.connected = true;
    client.transportMode = "webtransport";
    client.handleIncomingStream = vi.fn();
    client.transport = {
      createBidirectionalStream: vi.fn(async () => {
        await Promise.resolve();
        return {
          readable: {},
          writable: { getWriter: () => writer },
        };
      }),
    };

    const a = new Uint8Array([1, 2]);
    const b = new Uint8Array([3, 4]);
    const c = new Uint8Array([5, 6]);

    await Promise.all([client.sendStream(a), client.sendStream(b), client.sendStream(c)]);

    expect(client.transport.createBidirectionalStream).toHaveBeenCalledTimes(1);
    expect(client.handleIncomingStream).toHaveBeenCalledTimes(1);
    expect(writer.write).toHaveBeenCalledTimes(3);
    expect(written).toEqual([a, b, c]);
  });

  it("resets stream state on transient write rejection and recovers on subsequent sendStream", async () => {
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    const client = new WebTransportClient();
    const written = [];
    let attempt = 0;

    client.connected = true;
    client.transportMode = "webtransport";
    client.handleIncomingStream = vi.fn();
    client.transport = {
      createBidirectionalStream: vi.fn(async () => {
        attempt += 1;
        const streamIdx = attempt;
        return {
          readable: {},
          writable: {
            getWriter: () => ({
              write: vi.fn(async (data) => {
                if (streamIdx === 1) {
                  throw new Error("Transient QUIC stream reset");
                }
                written.push(data);
              }),
            }),
          },
        };
      }),
    };

    await client.sendStream(new Uint8Array([9]));
    expect(warnSpy).toHaveBeenCalled();
    expect(client.streamWriter).toBeNull();
    expect(client._streamInitPromise).toBeNull();

    const d = new Uint8Array([7, 8, 9]);
    await client.sendStream(d);

    expect(client.transport.createBidirectionalStream).toHaveBeenCalledTimes(2);
    expect(client.streamWriter).not.toBeNull();
    expect(written).toEqual([d]);
  });
});

describe("ClientWorldBridge.isAuthenticated Lifecycle", () => {
  function buildAuthResponseFrame(success = true, errorCode = 0, sessionId = 123n, playerGuid = 456n) {
    const frame = new Uint8Array(33);
    const view = new DataView(frame.buffer);
    frame[0] = 0x4d;
    frame[1] = 0x4d;
    view.setUint16(2, OPCODES.AUTH_RESPONSE, true);
    view.setUint32(4, 25, true);
    frame[8] = success ? 1 : 0;
    view.setUint32(9, errorCode >>> 0, true);
    view.setBigUint64(13, BigInt(sessionId), true);
    view.setBigUint64(21, BigInt(playerGuid), true);
    return frame;
  }

  it("starts false, ignores non-auth and failed auth packets, turns true on valid AUTH_RESPONSE, and resets on clear()", () => {
    const bridge = new ClientWorldBridge();
    expect(bridge.isAuthenticated).toBe(false);

    // Non-auth packet leaves isAuthenticated false
    const nonAuthFrame = buildInteractionResultFrame(1, new Uint8Array([1, 2, 3]));
    bridge.processPacket(nonAuthFrame);
    expect(bridge.isAuthenticated).toBe(false);

    // Failed AUTH_RESPONSE leaves isAuthenticated false
    bridge.processPacket(buildAuthResponseFrame(false, 401, 0n, 0n));
    expect(bridge.isAuthenticated).toBe(false);

    // Valid AUTH_RESPONSE flips isAuthenticated to true and records IDs
    const authCb = vi.fn();
    bridge.onAuthResponse(authCb);
    bridge.processPacket(buildAuthResponseFrame(true, 0, 777n, 888n));
    expect(bridge.isAuthenticated).toBe(true);
    expect(bridge.sessionId).toBe(777);
    expect(bridge.localPlayerGuid).toBe(888);
    expect(authCb).toHaveBeenCalledWith({
      sessionId: 777,
      playerGuid: 888,
      success: true,
      errorCode: 0,
    });

    // clear() resets isAuthenticated back to false
    bridge.clear();
    expect(bridge.isAuthenticated).toBe(false);
  });
});

describe("AsteroidVoxelClient Preview Seeding, Authoritative Tracking & Bounded Retries", () => {
  function buildChunkDataPayload(cx, cy, cz, spans, revision = 0n) {
    const rle = encodeTestVxr1(spans, revision);
    const payload = new Uint8Array(12 + rle.length);
    const view = new DataView(payload.buffer);
    view.setInt32(0, cx, true);
    view.setInt32(4, cy, true);
    view.setInt32(8, cz, true);
    payload.set(rle, 12);
    return payload;
  }

  it("seeds all 10 preview chunks, prioritizes seeded chunks, and clears isPreview on revision=0 CHUNK_DATA", () => {
    const client = new AsteroidVoxelClient();
    const seededCount = client.seedLocalPreviewChunks();
    expect(seededCount).toBe(SEEDED_ASTEROID_CHUNKS.length);
    expect(SEEDED_ASTEROID_CHUNKS).toHaveLength(10);
    expect(client.chunks.size).toBe(10);

    for (const c of SEEDED_ASTEROID_CHUNKS) {
      const entry = client.getChunk(c.x, c.y, c.z);
      expect(entry).not.toBeNull();
      expect(entry.isPreview).toBe(true);
      expect(entry.revision).toBe(0);
    }

    // Even with preview chunks loaded, collectUnrequestedChunks prioritizes SEEDED_ASTEROID_CHUNKS first
    const reqs = client.collectUnrequestedChunks({ x: 0, y: 0, z: 8 }, 1, 0, 1, 1000);
    expect(reqs.slice(0, SEEDED_ASTEROID_CHUNKS.length)).toEqual(
      SEEDED_ASTEROID_CHUNKS.map((c) => ({ x: c.x, y: c.y, z: c.z }))
    );

    // Authoritative SUB_OPCODE_CHUNK_DATA with revision=0 clears isPreview and stops re-requesting
    const first = SEEDED_ASTEROID_CHUNKS[0];
    const authPayload = buildChunkDataPayload(
      first.x,
      first.y,
      first.z,
      [[4095, VOXEL_EMPTY], [1, VOXEL_TITANIUM]],
      0n
    );
    const res = client.handleInteractionResult(
      (VOXEL_MODULE_ID << 16) | SUB_OPCODE_CHUNK_DATA,
      authPayload
    );
    expect(res.type).toBe("chunk_data");
    expect(client.getChunk(first.x, first.y, first.z).isPreview).toBe(false);
    expect(client.getChunk(first.x, first.y, first.z).revision).toBe(0);

    const nextReqs = client.collectUnrequestedChunks({ x: 0, y: 0, z: 8 }, 1, 0, 1, 10000);
    expect(nextReqs.some((c) => c.x === first.x && c.y === first.y && c.z === first.z)).toBe(false);
  });

  it("bounds empty vacuum chunk retries to maxEmptyChunkRetries (3) and resets via resetRequestTracking()", () => {
    const client = new AsteroidVoxelClient();
    expect(client.maxEmptyChunkRetries).toBe(3);

    const hasVacuum = (list) => list.some((c) => c.x === 0 && c.y === 0 && c.z === 0);

    // Attempt 1
    expect(hasVacuum(client.collectUnrequestedChunks({ x: 0, y: 0, z: 8 }, 0, 0, 0, 1000))).toBe(true);
    // Within TTL (2000ms) -> skipped
    expect(hasVacuum(client.collectUnrequestedChunks({ x: 0, y: 0, z: 8 }, 0, 0, 0, 2000))).toBe(false);
    // Attempt 2
    expect(hasVacuum(client.collectUnrequestedChunks({ x: 0, y: 0, z: 8 }, 0, 0, 0, 3500))).toBe(true);
    // Attempt 3
    expect(hasVacuum(client.collectUnrequestedChunks({ x: 0, y: 0, z: 8 }, 0, 0, 0, 6000))).toBe(true);
    // Attempt 4 -> stopped at maxEmptyChunkRetries = 3
    expect(hasVacuum(client.collectUnrequestedChunks({ x: 0, y: 0, z: 8 }, 0, 0, 0, 9000))).toBe(false);

    // Post-auth resetRequestTracking clears pending tracking so chunks can be re-requested
    client.resetRequestTracking();
    expect(client.pendingChunkRequests.size).toBe(0);
    expect(client.requestedChunks.size).toBe(0);
    expect(hasVacuum(client.collectUnrequestedChunks({ x: 0, y: 0, z: 8 }, 0, 0, 0, 9500))).toBe(true);
  });

  it("summarizes sorted asteroid clusters and exposes voxel material metadata", () => {
    const client = new AsteroidVoxelClient();

    const emptyVoxels = new Uint16Array(CHUNK_VOXELS);
    client.setChunk(0, 0, 0, emptyVoxels, 1);

    const farVoxels = new Uint16Array(CHUNK_VOXELS);
    farVoxels[indexFromLocal(0, 0, 0)] = VOXEL_CRUST_ROCK;
    client.setChunk(3, 0, 0, farVoxels, 1, { isPreview: true });

    const nearVoxels = new Uint16Array(CHUNK_VOXELS);
    nearVoxels[indexFromLocal(1, 1, 1)] = VOXEL_TITANIUM;
    nearVoxels[indexFromLocal(2, 1, 1)] = VOXEL_VOID_CRYSTAL;
    client.setChunk(0, 0, -1, nearVoxels, 2, { isPreview: false });

    const clusters = client.getAsteroidClusterSummaries({ x: 0, y: 0, z: 0 });
    expect(clusters).toHaveLength(2);
    expect(clusters[0].key).toBe("0,0,-1");
    expect(clusters[0].solidCount).toBe(2);
    expect(clusters[0].oreCount).toBe(2);
    expect(clusters[0].isPreview).toBe(false);
    expect(clusters[1].key).toBe("3,0,0");
    expect(clusters[1].isPreview).toBe(true);
    expect(clusters[0].distance).toBeLessThan(clusters[1].distance);

    expect(getVoxelMaterialInfo(VOXEL_TITANIUM).unitValue).toBe(10);
    expect(getVoxelMaterialInfo(VOXEL_SOLAR_GOLD).unitValue).toBe(30);
    expect(getVoxelMaterialInfo(VOXEL_VOID_CRYSTAL).unitValue).toBe(75);
    expect(getVoxelMaterialInfo(VOXEL_CRUST_ROCK).unitValue).toBe(0);
    expect(getVoxelMaterialInfo(VOXEL_EMPTY).unitValue).toBe(0);
  });
});

describe("SpaceFlightControls Autopilot, Brake (X) & Bank (B)", () => {
  it("orients toward target with autopilot and stops cleanly inside stopDistance", () => {
    const controls = new SpaceFlightControls({ x: 0, y: 0, z: 0, yaw: 0, pitch: 0 });
    const target = { x: 0, y: 0, z: -20 };
    controls.orientToward(target, true, 5);

    expect(controls.autopilotTarget).toEqual({ x: 0, y: 0, z: -20, stopDistance: 5 });
    expect(Math.abs(controls.yaw)).toBeCloseTo(Math.PI, 4);

    for (let i = 0; i < 100; i++) {
      controls.update(0.1);
    }

    const remainingDist = Math.hypot(
      target.x - controls.position.x,
      target.y - controls.position.y,
      target.z - controls.position.z
    );
    expect(remainingDist).toBeLessThanOrEqual(5.0);
    expect(controls.autopilotTarget).toBeNull();
    expect(controls.velocity).toEqual({ x: 0, y: 0, z: 0 });
    expect(controls.pitch).toBe(0);
  });

  it("handles KeyX / brake() and KeyB onBank callback while ignoring editable targets", () => {
    const controls = new SpaceFlightControls({ x: 0, y: 0, z: 0, yaw: 0, pitch: 0.4 });
    const bankSpy = vi.fn();
    controls.onBank(bankSpy);
    controls.velocity = { x: 3.0, y: -2.0, z: 4.0 };
    controls.orientToward({ x: 0, y: 0, z: -30 }, true, 5);

    // Editable targets must ignore KeyX and KeyB
    controls.setKeyState({ key: "b", code: "KeyB", target: { tagName: "INPUT" } }, true);
    controls.setKeyState({ key: "x", code: "KeyX", target: { tagName: "INPUT" } }, true);
    expect(bankSpy).not.toHaveBeenCalled();
    expect(controls.autopilotTarget).not.toBeNull();
    expect(controls.velocity.x).toBe(3.0);

    // Non-editable target fires onBank and KeyX brakes
    controls.setKeyState({ key: "b", code: "KeyB", target: { tagName: "CANVAS" } }, true);
    expect(bankSpy).toHaveBeenCalledTimes(1);

    controls.setKeyState({ key: "x", code: "KeyX", target: { tagName: "CANVAS" } }, true);
    expect(controls.velocity).toEqual({ x: 0, y: 0, z: 0 });
    expect(controls.pitch).toBe(0);
    expect(controls.autopilotTarget).toBeNull();
  });
});

describe("AsteroidRenderer Chunk Beacons, Voxel Reticle & Chase Camera", () => {
  it("manages chunkBeacons across non-empty, emptied, and cleared chunks", () => {
    const renderer = new AsteroidRenderer(null);
    const voxelClient = new AsteroidVoxelClient();

    const oreVoxels = new Uint16Array(CHUNK_VOXELS);
    oreVoxels[indexFromLocal(2, 3, 4)] = VOXEL_SOLAR_GOLD;
    voxelClient.setChunk(0, 0, -2, oreVoxels, 1);

    renderer.syncVoxelChunks(voxelClient);
    expect(renderer.chunkBeacons.has("0,0,-2")).toBe(true);
    expect(renderer.chunkBeacons.get("0,0,-2").userData.ringMat.color.getHex()).toBe(0xfacc15);

    // Emptying the only voxel removes both chunk mesh and beacon
    voxelClient.setVoxel(2, 3, -32 + 4, VOXEL_EMPTY);
    renderer.syncVoxelChunks(voxelClient);
    expect(renderer.chunkBeacons.has("0,0,-2")).toBe(false);
    expect(renderer.voxelChunkMeshes.has("0,0,-2")).toBe(false);

    // Non-ore rock chunk uses cyan beacon and is removed by clear()
    const rockVoxels = new Uint16Array(CHUNK_VOXELS);
    rockVoxels[indexFromLocal(0, 0, 0)] = VOXEL_CRUST_ROCK;
    voxelClient.setChunk(1, 0, 0, rockVoxels, 1);
    renderer.syncVoxelChunks(voxelClient);
    expect(renderer.chunkBeacons.has("1,0,0")).toBe(true);
    expect(renderer.chunkBeacons.get("1,0,0").userData.ringMat.color.getHex()).toBe(0x38bdf8);

    renderer.clear();
    expect(renderer.chunkBeacons.size).toBe(0);
  });

  it("positions and tints voxelHighlightMesh on hit and hides it on null", () => {
    const renderer = new AsteroidRenderer(null);
    expect(renderer.voxelHighlightMesh.visible).toBe(false);

    renderer.setHighlightedVoxel({ x: 3, y: 4, z: -5, voxelId: VOXEL_VOID_CRYSTAL });
    expect(renderer.voxelHighlightMesh.visible).toBe(true);
    expect(renderer.voxelHighlightMesh.position.x).toBeCloseTo(3.5, 5);
    expect(renderer.voxelHighlightMesh.position.y).toBeCloseTo(4.5, 5);
    expect(renderer.voxelHighlightMesh.position.z).toBeCloseTo(-4.5, 5);
    expect(renderer.voxelHighlightMesh.material.color.getHex()).toBe(
      VOXEL_MATERIALS[VOXEL_VOID_CRYSTAL].hex
    );

    renderer.setHighlightedVoxel(null);
    expect(renderer.voxelHighlightMesh.visible).toBe(false);
  });

  it("aligns camera behind ship and tracks yaw changes unless dragging", () => {
    const renderer = new AsteroidRenderer(null);
    renderer.alignCameraBehindShip(Math.PI, 0);
    expect(renderer.orbitYaw).toBeCloseTo(2 * Math.PI, 5);
    expect(renderer.orbitPitch).toBeCloseTo(0.28, 5);

    // Tracks yaw delta when not dragging
    renderer.isDragging = false;
    renderer.updateCamera({ x: 0, y: 0, z: 8 }, Math.PI + 0.5);
    expect(renderer.orbitYaw).toBeCloseTo(2 * Math.PI + 0.5, 5);

    // Yields orbitYaw when dragging
    const yawBeforeDrag = renderer.orbitYaw;
    renderer.isDragging = true;
    renderer.updateCamera({ x: 0, y: 0, z: 8 }, Math.PI + 1.2);
    expect(renderer.orbitYaw).toBeCloseTo(yawBeforeDrag, 5);
  });
});

describe("UIController NPC Drone Target & Mission Guidance", () => {
  let elements;

  function createMockEl(id = "", className = "") {
    const el = {
      id,
      className,
      style: {},
      children: [],
      textContent: "",
      value: "",
      disabled: false,
      dataset: {},
      _listeners: {},
      replaceChildren(...newChildren) {
        this.children = [...newChildren];
      },
      appendChild(child) {
        this.children.push(child);
      },
      addEventListener(evt, fn) {
        this._listeners[evt] = fn;
      },
    };
    el.classList = {
      add(...tokens) {
        const s = new Set((el.className || "").split(/\s+/).filter(Boolean));
        tokens.forEach((t) => s.add(t));
        el.className = Array.from(s).join(" ");
      },
      remove(...tokens) {
        const s = new Set((el.className || "").split(/\s+/).filter(Boolean));
        tokens.forEach((t) => s.delete(t));
        el.className = Array.from(s).join(" ");
      },
      contains(token) {
        return (el.className || "").split(/\s+/).filter(Boolean).includes(token);
      },
    };
    return el;
  }

  beforeEach(() => {
    elements = new Map();
    globalThis.document = {
      title: "",
      body: { dataset: {} },
      getElementById(id) {
        if (!elements.has(id)) {
          elements.set(id, createMockEl(id));
        }
        return elements.get(id);
      },
      createElement() {
        return createMockEl();
      },
    };
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("labels NPC Rogue Drone targets and hides/disables #btn-trade-pilot", () => {
    const ui = new UIController();
    ui.showAsteroidTarget(
      { id: 9, type: 2, distance: 20, hpPct: 0.75, stateFlags: 10 << 16 },
      false
    );

    expect(document.getElementById("asteroid-target-panel").style.display).toBe("block");
    expect(document.getElementById("asteroid-target-name").textContent).toBe("Rogue Drone #9");
    expect(document.getElementById("asteroid-target-shininess").textContent).toBe("Tier 10");
    expect(document.getElementById("asteroid-target-distance").textContent).toBe("20.0 m");
    expect(document.getElementById("asteroid-target-hull").textContent).toBe("75%");
    expect(document.getElementById("btn-fire-laser").disabled).toBe(false);
    expect(document.getElementById("btn-trade-pilot").disabled).toBe(true);
    expect(document.getElementById("btn-trade-pilot").style.display).toBe("none");

    // Re-enables trade button for player pilots (type !== 2)
    ui.showAsteroidTarget({ id: 4, type: 1, distance: 12, hpPct: 1.0, stateFlags: 0 }, false);
    expect(document.getElementById("asteroid-target-name").textContent).toBe("Pilot #4");
    expect(document.getElementById("btn-trade-pilot").disabled).toBe(false);
    expect(document.getElementById("btn-trade-pilot").style.display).toBe("");
  });

  it("updates #asteroid-nav-summary, #asteroid-aim-target, and mission step done/active classes", () => {
    const ui = new UIController();
    const stepApproach = document.getElementById("mission-step-approach");
    const stepMine = document.getElementById("mission-step-mine");
    const stepBank = document.getElementById("mission-step-bank");

    // Step 1 active: far from asteroid
    ui.updateAsteroidGuidance({
      distToStation: 8.0,
      nearestAsteroidDist: 30.0,
      cargoValue: 0,
      bankedCredits: 0,
    });
    expect(document.getElementById("asteroid-nav-summary").textContent).toBe("Belt: 30.0m | Dock: 8.0m");
    expect(document.getElementById("asteroid-aim-target").textContent).toBe(
      "Fly closer (<=18m) or click Fly Asteroid"
    );
    expect(stepApproach.classList.contains("active")).toBe(true);
    expect(stepApproach.classList.contains("done")).toBe(false);
    expect(stepMine.classList.contains("active")).toBe(false);

    // Step 2 active: approached and aiming at Solar Gold
    ui.updateAsteroidGuidance({
      distToStation: 22.5,
      nearestAsteroidDist: 10.0,
      aimedHit: { distance: 6.5 },
      aimedMaterial: VOXEL_MATERIALS[VOXEL_SOLAR_GOLD],
      cargoValue: 0,
      bankedCredits: 0,
    });
    expect(document.getElementById("asteroid-nav-summary").textContent).toBe("Belt: 10.0m | Dock: 22.5m");
    expect(document.getElementById("asteroid-aim-target").textContent).toBe(
      "Aim: Solar Gold (30 cr) 6.5m [Click/J]"
    );
    expect(stepApproach.classList.contains("done")).toBe(true);
    expect(stepApproach.classList.contains("active")).toBe(false);
    expect(stepMine.classList.contains("active")).toBe(true);
    expect(stepMine.classList.contains("done")).toBe(false);

    // Step 3 active: cargo collected, not yet banked
    ui.updateAsteroidGuidance({
      distToStation: 12.0,
      nearestAsteroidDist: 25.0,
      cargoValue: 30,
      bankedCredits: 0,
    });
    expect(stepMine.classList.contains("done")).toBe(true);
    expect(stepMine.classList.contains("active")).toBe(false);
    expect(stepBank.classList.contains("active")).toBe(true);
    expect(stepBank.classList.contains("done")).toBe(false);

    // All steps done after banking
    ui.updateAsteroidGuidance({
      distToStation: 8.0,
      nearestAsteroidDist: 25.0,
      cargoValue: 0,
      bankedCredits: 30,
    });
    expect(stepBank.classList.contains("done")).toBe(true);
    expect(stepBank.classList.contains("active")).toBe(false);
  });
});

describe("DevToolsController Isolated Offline Sandbox", () => {
  it("mines ore locally, updates unbanked_cargo_value and shininess_tier, and leaves crust rock uncredited", () => {
    const voxelClient = new AsteroidVoxelClient();
    const voxels = new Uint16Array(CHUNK_VOXELS);
    voxels[indexFromLocal(1, 0, 0)] = VOXEL_CRUST_ROCK;
    voxels[indexFromLocal(2, 0, 0)] = VOXEL_VOID_CRYSTAL;
    voxelClient.setChunk(0, 0, -1, voxels, 1);

    const onStateUpdate = vi.fn();
    const devTools = new DevToolsController({ voxelClient, onStateUpdate });
    devTools.setSandboxEnabled(true, { silent: true });

    // Mining crust rock clears block without crediting cargo or shininess
    const rockResp = devTools.mineVoxelLocally(1, 0, -16, { x: 0, y: 0, z: -18 });
    expect(voxelClient.getVoxel(1, 0, -16)).toBe(VOXEL_EMPTY);
    expect(rockResp.unbanked_cargo_value).toBe(0);
    expect(rockResp.shininess_tier).toBe(0);
    expect(rockResp.inventory.slots.every((s) => s === null)).toBe(true);

    // Mining Void Crystal credits 75 cr and raises shininess_tier = floor(75 / 5) = 15
    const oreResp = devTools.mineVoxelLocally(2, 0, -16, { x: 0, y: 0, z: -18 });
    expect(voxelClient.getVoxel(2, 0, -16)).toBe(VOXEL_EMPTY);
    expect(oreResp.unbanked_cargo_value).toBe(75);
    expect(oreResp.shininess_tier).toBe(15);
    expect(oreResp.inventory.slots[0]).toMatchObject({
      item_id: 2003,
      name: "Void Crystal",
      quantity: 1,
    });
    expect(onStateUpdate).toHaveBeenCalledTimes(2);
  });

  it("enforces safe zone, range, and cooldown in fireSandboxLaser, loots on kill, and banks in 15m safe zone", () => {
    const world = new ClientWorldBridge();
    const devTools = new DevToolsController({ world });

    const dummy = devTools.spawnTargetDummy({ x: 0, y: 0, z: -25 }, 200);
    expect(devTools.sandboxEnabled).toBe(true);
    expect(world.entities.has(dummy.id)).toBe(true);

    // Safe zone blocks attacker inside 15m
    const safeAttackerRes = devTools.fireSandboxLaser(dummy.id, { x: 0, y: 0, z: 8 }, 1000);
    expect(safeAttackerRes.ok).toBe(false);
    expect(safeAttackerRes.error).toMatch(/safe zone/i);

    // Safe zone protects target dummy inside 15m
    const safeDummy = devTools.spawnTargetDummy({ x: 0, y: 0, z: 5 }, 50);
    const safeTargetRes = devTools.fireSandboxLaser(safeDummy.id, { x: 0, y: 0, z: -20 }, 1000);
    expect(safeTargetRes.ok).toBe(false);
    expect(safeTargetRes.error).toMatch(/safe zone/i);

    // Range limit (> 45m) blocks fire
    const outOfRangeRes = devTools.fireSandboxLaser(dummy.id, { x: 0, y: 0, z: 30 }, 1000);
    expect(outOfRangeRes.ok).toBe(false);
    expect(outOfRangeRes.error).toMatch(/range/i);

    // In range outside safe zone: 25 dmg per shot, 500ms cooldown, 100 HP dummy dies on 4th hit
    const attackPos = { x: 0, y: 0, z: -18 };
    const hit1 = devTools.fireSandboxLaser(dummy.id, attackPos, 1000);
    expect(hit1).toMatchObject({ ok: true, destroyed: false, remainingHp: 75 });

    const cooldownRes = devTools.fireSandboxLaser(dummy.id, attackPos, 1200);
    expect(cooldownRes.ok).toBe(false);
    expect(cooldownRes.error).toMatch(/recharging/i);

    expect(devTools.fireSandboxLaser(dummy.id, attackPos, 1600)).toMatchObject({
      ok: true,
      destroyed: false,
      remainingHp: 50,
    });
    expect(devTools.fireSandboxLaser(dummy.id, attackPos, 2200)).toMatchObject({
      ok: true,
      destroyed: false,
      remainingHp: 25,
    });
    const killRes = devTools.fireSandboxLaser(dummy.id, attackPos, 2800);
    expect(killRes.ok).toBe(true);
    expect(killRes.destroyed).toBe(true);
    expect(killRes.remainingHp).toBe(0);
    expect(world.entities.has(dummy.id)).toBe(false);
    expect(killRes.state.station_vault.kills).toBe(1);
    // Loot: 2x Solar Gold (60) + 1x Void Crystal (75) = 135 cr
    expect(killRes.state.unbanked_cargo_value).toBe(135);

    // Banking outside 15m safe zone fails with error and leaves cargo unbanked
    const outsideBank = devTools.bankSandboxCargo({ x: 0, y: 0, z: -20 });
    expect(outsideBank.error).toMatch(/15\.0m/);
    expect(outsideBank.station_vault.banked_credits).toBe(0);
    expect(outsideBank.unbanked_cargo_value).toBe(135);

    // Banking inside 15m safe zone deposits items & credits into station_vault
    const insideBank = devTools.bankSandboxCargo({ x: 0, y: 0, z: 8 });
    expect(insideBank.error).toBeNull();
    expect(insideBank.unbanked_cargo_value).toBe(0);
    expect(insideBank.station_vault.banked_credits).toBe(135);
    expect(insideBank.station_vault.banked_items).toHaveLength(2);
    expect(insideBank.inventory.slots.every((s) => s === null)).toBe(true);
  });

  it("clears sandboxEnabled and removes dummy entities on onLiveAuthSucceeded()", () => {
    const world = new ClientWorldBridge();
    const devTools = new DevToolsController({ world });
    const dummy = devTools.spawnTargetDummy({ x: 10, y: 0, z: -20 }, 150);

    expect(devTools.sandboxEnabled).toBe(true);
    expect(world.entities.has(dummy.id)).toBe(true);

    devTools.onLiveAuthSucceeded();
    expect(devTools.sandboxEnabled).toBe(false);
    expect(devTools.dummies.size).toBe(0);
    expect(world.entities.has(dummy.id)).toBe(false);
  });
});
