import { describe, it, expect, beforeEach, vi } from "vitest";
import * as THREE from "three";
import { PlayerControls } from "../controls.js";
import { WorldObjectRenderer } from "../world.js";
import { WorldRenderer, disposeHierarchy } from "../renderer.js";
import { ClientWorldBridge, getOrCreateSessionToken, formatConnectionError } from "../main.js";
import { UIController } from "../ui.js";
import { deriveWebSocketUrl, WebTransportClient } from "../transport.js";
import {
  OPCODES,
  INTERACTION_TYPES,
  DELTA_FLAGS,
  encodeAuthRequest,
  encodeInteraction,
  encodeMovement,
  encodeClientAck,
  putVarintU64,
  dequantizeDeltaI16,
  dequantizeVelocityI16,
  dequantizeYawU8,
  dequantizePitchI8,
  readVarintU64,
  readVarintU32,
} from "../protocol.js";

describe("PlayerControls", () => {

  it("initializes window event listeners and handles keydown/keyup events", () => {
    const addEventListenerMock = vi.fn();
    vi.stubGlobal('window', { addEventListener: addEventListenerMock });

    const controls = new PlayerControls();

    expect(addEventListenerMock).toHaveBeenCalledTimes(2);
    expect(addEventListenerMock).toHaveBeenCalledWith("keydown", expect.any(Function));
    expect(addEventListenerMock).toHaveBeenCalledWith("keyup", expect.any(Function));

    const keydownHandler = addEventListenerMock.mock.calls.find(call => call[0] === "keydown")[1];
    const keyupHandler = addEventListenerMock.mock.calls.find(call => call[0] === "keyup")[1];

    // Test keydown
    keydownHandler({ key: "w" });
    expect(controls.keys.w).toBe(true);
    keydownHandler({ key: "A" }); // test lowercase conversion
    expect(controls.keys.a).toBe(true);
    keydownHandler({ key: "s" });
    expect(controls.keys.s).toBe(true);
    keydownHandler({ key: "d" });
    expect(controls.keys.d).toBe(true);
    keydownHandler({ key: "q" });
    expect(controls.keys.q).toBe(true);
    keydownHandler({ key: "e" });
    expect(controls.keys.e).toBe(true);
    keydownHandler({ key: " ", code: "Space" });
    expect(controls.keys.space).toBe(true);

    // Test keyup
    keyupHandler({ key: "w" });
    expect(controls.keys.w).toBe(false);
    keyupHandler({ key: "A" });
    expect(controls.keys.a).toBe(false);
    keyupHandler({ key: "s" });
    expect(controls.keys.s).toBe(false);
    keyupHandler({ key: "d" });
    expect(controls.keys.d).toBe(false);
    keyupHandler({ key: "q" });
    expect(controls.keys.q).toBe(false);
    keyupHandler({ key: "e" });
    expect(controls.keys.e).toBe(false);
    keyupHandler({ key: " ", code: "Space" });
    expect(controls.keys.space).toBe(false);

    vi.unstubAllGlobals();
  });

  it("triggers onCastCallback when Space is pressed", () => {
    const addEventListenerMock = vi.fn();
    vi.stubGlobal('window', { addEventListener: addEventListenerMock });

    const controls = new PlayerControls();
    const castMock = vi.fn();
    controls.onCast(castMock);

    const keydownHandler = addEventListenerMock.mock.calls.find(call => call[0] === "keydown")[1];

    // Press Space
    keydownHandler({ key: " ", code: "Space" });

    expect(castMock).toHaveBeenCalledTimes(1);

    vi.unstubAllGlobals();
  });

  it("updates position correctly using setPosition", () => {
    const controls = new PlayerControls();
    controls.setPosition(10.5, 2.0, -5.5);

    expect(controls.position.x).toBe(10.5);
    expect(controls.position.y).toBe(2.0);
    expect(controls.position.z).toBe(-5.5);
  });

  it("cancels movement when opposite keys are pressed (w+s, a+d)", () => {
    const controls = new PlayerControls();

    const startX = controls.position.x;
    const startZ = controls.position.z;
    const dt = 0.5;

    // w + s
    controls.keys.w = true;
    controls.keys.s = true;
    controls.update(dt);

    expect(controls.position.x).toBeCloseTo(startX, 5);
    expect(controls.position.z).toBeCloseTo(startZ, 5);

    // a + d
    controls.keys.w = false;
    controls.keys.s = false;
    controls.keys.a = true;
    controls.keys.d = true;
    controls.update(dt);

    expect(controls.position.x).toBeCloseTo(startX, 5);
    expect(controls.position.z).toBeCloseTo(startZ, 5);
  });

  it("normalizes diagonal movement vector on w + d input", () => {
    const controls = new PlayerControls();
    controls.keys.w = true;
    controls.keys.d = true;

    const startX = controls.position.x;
    const startZ = controls.position.z;
    const dt = 0.5;

    controls.update(dt);

    const dx = controls.position.x - startX;
    const dz = controls.position.z - startZ;
    const dist = Math.hypot(dx, dz);

    expect(dist).toBeCloseTo(controls.speed * dt, 5);
  });

  it("reconciles server movement correction and updates sequence", () => {
    const controls = new PlayerControls();
    controls.setPosition(1.0, 0.0, 1.0);
    controls.sequence = 5;

    controls.reconcile(10, 20.0, 5.0, -15.0);
    expect(controls.position.x).toBe(20.0);
    expect(controls.position.y).toBe(5.0);
    expect(controls.position.z).toBe(-15.0);
    expect(controls.sequence).toBe(11);

    // Stale sequence correction does not decrement sequence
    controls.reconcile(8, 25.0, 5.0, -15.0);
    expect(controls.position.x).toBe(25.0);
    expect(controls.sequence).toBe(11);
  });

  it("rotates yaw with q and e keys and increments sequence monotonically", () => {
    const controls = new PlayerControls();
    const initialYaw = controls.yaw;
    const dt = 0.1;

    controls.keys.q = true;
    controls.update(dt);
    expect(controls.yaw).toBeGreaterThan(initialYaw);

    controls.keys.q = false;
    controls.keys.e = true;
    controls.update(dt);
    expect(controls.yaw).toBeCloseTo(initialYaw, 5);

    const s1 = controls.sample();
    const s2 = controls.sample();
    const s3 = controls.sample();

    expect(s1.sequence).toBe(1);
    expect(s2.sequence).toBe(2);
    expect(s3.sequence).toBe(3);
  });

  it("initializes at unobstructed plaza spawn (0, 0, 5) with speed headroom below 10.0 m/s", () => {
    const controls = new PlayerControls();
    expect(controls.position).toEqual({ x: 0.0, y: 0.0, z: 5.0 });
    expect(controls.speed).toBeGreaterThan(0.0);
    expect(controls.speed).toBeLessThan(10.0);
    expect(controls.speed).toBe(7.5);
  });
});

describe("WorldObjectRenderer", () => {
  it("decodes 64-bit object IDs, updates damaged appearance, scales HP bar, and despawns objects", () => {
    const scene = new THREE.Scene();
    const renderer = new WorldObjectRenderer(scene);

    const buf = new Float32Array(16);
    const u32 = new Uint32Array(buf.buffer);
    const lo = 3735928559; // 0xDEADBEEF (> 2^24)
    const hi = 305419896; // 0x12345678 (> 2^24)
    u32[0] = lo;
    buf[1] = 42; // prefabId
    buf[2] = 10.0; // x
    buf[3] = 0.0; // y
    buf[4] = 20.0; // z
    buf[5] = 0.0; // qx
    buf[6] = 0.0; // qy
    buf[7] = 0.0; // qz
    buf[8] = 1.0; // qw
    buf[9] = 1.0; // sx
    buf[10] = 1.0; // sy
    buf[11] = 1.0; // sz
    buf[12] = 16; // stateFlags (DAMAGED)
    buf[13] = 0.5; // hpPct
    buf[14] = 1; // revision
    u32[15] = hi;

    renderer.updateFromBuffer(buf);

    const expectedId = (BigInt(hi) << 32n) | BigInt(lo);
    expect(renderer.objects.has(expectedId)).toBe(true);

    const group = renderer.objects.get(expectedId);
    expect(group.userData.mesh.material).toBe(renderer.materials.damaged);
    expect(group.userData.hpBar.scale.x).toBeCloseTo(0.5, 5);

    // Empty buffer clears / despawns all objects
    renderer.updateFromBuffer(new Float32Array(0));
    expect(renderer.objects.size).toBe(0);
  });

  it("assigns distinct materials for Village prefabs: 10 (well), 11 (forge), 12 (stall), 13 (house)", () => {
    const scene = new THREE.Scene();
    const renderer = new WorldObjectRenderer(scene);

    const prefabs = [10, 11, 12, 13];
    const expectedMaterials = [
      renderer.materials.well,
      renderer.materials.forge,
      renderer.materials.stall,
      renderer.materials.house,
    ];

    prefabs.forEach((pid, idx) => {
      const buf = new Float32Array(16);
      const u32 = new Uint32Array(buf.buffer);
      u32[0] = idx + 1; // id lo
      buf[1] = pid; // prefabId
      buf[8] = 1.0; // qw
      buf[9] = 1.0; // sx
      buf[10] = 1.0; // sy
      buf[11] = 1.0; // sz
      u32[15] = 0; // id hi

      renderer.updateFromBuffer(buf);
      const obj = renderer.objects.get(BigInt(idx + 1));
      expect(obj).toBeDefined();
      expect(obj.userData.mesh.material).toBe(expectedMaterials[idx]);
    });
  });
});

describe("WorldRenderer Entities", () => {
  it("renders villager NPC (entityType === 2) with purple attire and floating golden merchant marker", () => {
    // Pure Three.js mesh factory testing (node-compatible, without WebGLRenderer requirement)
    const npcGroup = WorldRenderer.prototype.createEntityMesh(2, false);
    expect(npcGroup).toBeDefined();
    expect(npcGroup.userData.marker).toBeDefined();
    expect(npcGroup.userData.marker.material.color.getHex()).toBe(0xf59e0b);

    // Remote player (entityType === 1)
    const playerGroup = WorldRenderer.prototype.createEntityMesh(1, false);
    expect(playerGroup.userData.marker).toBeUndefined();

    // Local player
    const localGroup = WorldRenderer.prototype.createEntityMesh(1, true);
    expect(localGroup.userData.marker).toBeUndefined();
  });

  it("disposes geometries and materials recursively on entity hierarchy disposal", () => {
    const group = new THREE.Group();
    const geo = new THREE.BoxGeometry(1, 1, 1);
    const mat = new THREE.MeshBasicMaterial();
    const mesh = new THREE.Mesh(geo, mat);
    group.add(mesh);

    const geoDispose = vi.spyOn(geo, "dispose");
    const matDispose = vi.spyOn(mat, "dispose");

    disposeHierarchy(group);

    expect(geoDispose).toHaveBeenCalledTimes(1);
    expect(matDispose).toHaveBeenCalledTimes(1);
  });

  it("disposes texture references recursively on material disposal", () => {
    const group = new THREE.Group();
    const geo = new THREE.BoxGeometry(1, 1, 1);
    const mat = new THREE.MeshBasicMaterial();
    const tex = new THREE.Texture();
    mat.map = tex;
    const mesh = new THREE.Mesh(geo, mat);
    group.add(mesh);

    const texDispose = vi.spyOn(tex, "dispose");
    disposeHierarchy(group);
    expect(texDispose).toHaveBeenCalledTimes(1);
  });
});

describe("Protocol Encoders", () => {
  it("encodes ClientPacket::AuthRequest with 0x4D4D magic and 0x0011 opcode", () => {
    const frame = encodeAuthRequest("guest-user-123", 1);
    expect(frame[0]).toBe(0x4d);
    expect(frame[1]).toBe(0x4d);

    const view = new DataView(frame.buffer);
    const opcode = view.getUint16(2, true);
    expect(opcode).toBe(OPCODES.AUTH_REQUEST);

    const tokenLen = view.getUint16(8, true);
    expect(tokenLen).toBe("guest-user-123".length);
  });

  it("encodes ClientPacket::Interaction with target, action_id, and payload", () => {
    const payload = new TextEncoder().encode(JSON.stringify({ recipe_id: 1 }));
    const actionId = (0x0002 << 16) | 2; // Sub-opcode 2: NpcBarter
    const frame = encodeInteraction(1001, 2002, actionId, payload, INTERACTION_TYPES.TRADE);

    const view = new DataView(frame.buffer);
    expect(view.getUint16(2, true)).toBe(OPCODES.INTERACTION);
    expect(Number(view.getBigUint64(8, true))).toBe(1001);
    expect(Number(view.getBigUint64(16, true))).toBe(2002);
    expect(frame[24]).toBe(INTERACTION_TYPES.TRADE);
    expect(view.getUint32(25, true)).toBe(actionId);
    expect(view.getUint32(29, true)).toBe(payload.length);
  });

  it("encodes ClientPacket::Movement with entity ID, 52-byte payload (60 bytes total), timestamp and sequence", () => {
    const sample = {
      x: 12.5,
      y: 1.0,
      z: -8.2,
      vx: 1.5,
      vy: 0.0,
      vz: -0.5,
      yaw: 3.14,
      pitch: 0.5,
      timestamp: 1698765432000,
      sequence: 42,
    };
    const frame = encodeMovement(10001, sample);

    expect(frame.length).toBe(60);
    const view = new DataView(frame.buffer);
    expect(view.getUint16(2, true)).toBe(OPCODES.MOVEMENT);
    expect(view.getUint32(4, true)).toBe(52); // Payload length 52
    expect(Number(view.getBigUint64(8, true))).toBe(10001);
    expect(view.getFloat32(16, true)).toBeCloseTo(12.5, 5);
    expect(view.getFloat32(20, true)).toBeCloseTo(1.0, 5);
    expect(view.getFloat32(24, true)).toBeCloseTo(-8.2, 5);
    expect(view.getFloat32(28, true)).toBeCloseTo(1.5, 5);
    expect(view.getFloat32(32, true)).toBeCloseTo(0.0, 5);
    expect(view.getFloat32(36, true)).toBeCloseTo(-0.5, 5);
    expect(view.getFloat32(40, true)).toBeCloseTo(3.14, 5);
    expect(view.getFloat32(44, true)).toBeCloseTo(0.5, 5);
    expect(Number(view.getBigUint64(48, true))).toBe(1698765432000);
    expect(view.getUint32(56, true)).toBe(42);
  });
});

describe("ClientWorldBridge", () => {
  it("ingests AuthResponse packet and sets local session and player guid", () => {
    const bridge = new ClientWorldBridge();

    // Encode mock AuthResponse frame (opcode 0x0012)
    const frame = new Uint8Array(8 + 25);
    const view = new DataView(frame.buffer);
    frame[0] = 0x4d;
    frame[1] = 0x4d;
    view.setUint16(2, OPCODES.AUTH_RESPONSE, true);
    view.setUint32(4, 25, true);

    frame[8] = 1; // success: true
    view.setUint32(9, 0, true); // error_code
    view.setBigUint64(13, 777n, true); // session_id
    view.setBigUint64(21, 55555n, true); // player_guid

    let callbackTriggered = false;
    bridge.onAuthResponse((res) => {
      callbackTriggered = true;
      expect(res.sessionId).toBe(777);
      expect(res.playerGuid).toBe(55555);
    });

    bridge.processPacket(frame);
    expect(callbackTriggered).toBe(true);
    expect(bridge.sessionId).toBe(777);
    expect(bridge.localPlayerGuid).toBe(55555);
  });

  it("ingests InteractionResult packet and extracts JSON payload", () => {
    const bridge = new ClientWorldBridge();
    const payloadObj = {
      player_inventory: { currency: 65, current_weight: 12.0, max_weight: 100.0, slots: [] },
    };
    const jsonStr = JSON.stringify(payloadObj);
    const jsonBytes = new TextEncoder().encode(jsonStr);

    const frame = new Uint8Array(8 + 13 + jsonBytes.length);
    const view = new DataView(frame.buffer);
    frame[0] = 0x4d;
    frame[1] = 0x4d;
    view.setUint16(2, OPCODES.INTERACTION_RESULT, true);
    view.setUint32(4, 13 + jsonBytes.length, true);

    view.setUint32(8, (0x0002 << 16) | 1, true); // action_id
    frame[12] = 1; // success
    view.setUint32(13, 0, true); // error_code
    view.setUint32(17, jsonBytes.length, true); // payload_len
    frame.set(jsonBytes, 21);

    let receivedData = null;
    bridge.onInteractionResult((res) => {
      receivedData = res.data;
    });

    bridge.processPacket(frame);
    expect(receivedData).toEqual(payloadObj);
  });

  it("ingests authoritative VillageTradeStateResponse payload with inventory, current_weight, active_trade, and target_merchant", () => {
    const bridge = new ClientWorldBridge();
    const tradeState = {
      player_guid: 12345,
      inventory: { currency: 150, max_weight: 100.0, slots: [] },
      current_weight: 22.5,
      is_encumbered: false,
      active_trade: {
        session_id: 88,
        player_a: 12345,
        player_b: 67890,
        my_offer: { currency: 10, items: [] },
        their_offer: { currency: 50, items: [] },
        my_locked: true,
        their_locked: false,
        my_accepted: false,
        their_accepted: false,
        completed: false,
      },
      target_merchant: {
        name: "Elder Väinö",
        role: "Elder",
        greeting: "Welcome traveler.",
        recipes: [],
      },
      trade_completed: false,
    };
    const jsonStr = JSON.stringify(tradeState);
    const jsonBytes = new TextEncoder().encode(jsonStr);

    const frame = new Uint8Array(8 + 13 + jsonBytes.length);
    const view = new DataView(frame.buffer);
    frame[0] = 0x4d;
    frame[1] = 0x4d;
    view.setUint16(2, OPCODES.INTERACTION_RESULT, true);
    view.setUint32(4, 13 + jsonBytes.length, true);

    view.setUint32(8, (0x0002 << 16) | 1, true); // action_id
    frame[12] = 1; // success
    view.setUint32(13, 0, true); // error_code
    view.setUint32(17, jsonBytes.length, true); // payload_len
    frame.set(jsonBytes, 21);

    let receivedData = null;
    bridge.onInteractionResult((res) => {
      receivedData = res.data;
    });

    bridge.processPacket(frame);
    expect(receivedData).toEqual(tradeState);
    expect(receivedData.active_trade.session_id).toBe(88);
    expect(receivedData.active_trade.their_offer.currency).toBe(50);
    expect(receivedData.current_weight).toBe(22.5);
  });

  it("decodes WorldObjectSpawn and WorldObjectDespawn with LEB128 varints", () => {
    const bridge = new ClientWorldBridge();

    function writeVarint(val) {
      let v = BigInt(val);
      const bytes = [];
      while (v >= 0x80n) {
        bytes.push(Number((v & 0x7fn) | 0x80n));
        v >>= 7n;
      }
      bytes.push(Number(v & 0x7fn));
      return bytes;
    }

    const objId = 3000000000n;
    const objIdBytes = writeVarint(objId);
    const prefabBytes = writeVarint(12);
    const floatBytes = new Uint8Array(64);
    const fView = new DataView(floatBytes.buffer);
    fView.setFloat32(0, 15.0, true); // x
    fView.setFloat32(4, 2.0, true); // y
    fView.setFloat32(8, -30.0, true); // z
    fView.setFloat32(24, 1.0, true); // qw
    fView.setFloat32(28, 1.0, true); // sx
    fView.setFloat32(32, 1.0, true); // sy
    fView.setFloat32(36, 1.0, true); // sz

    const payload = [
      ...objIdBytes,
      ...prefabBytes,
      ...Array.from(floatBytes),
      0x00, // state_flags: 0
      0x00, // owner_type: 0
      0x00, // owner_id: ""
      0x00, // has_health: 0
      0x00, // properties count: 0
      0x01, // revision: 1
    ];

    const frame = new Uint8Array(8 + payload.length);
    const view = new DataView(frame.buffer);
    frame[0] = 0x4d;
    frame[1] = 0x4d;
    view.setUint16(2, OPCODES.WORLD_OBJECT_SPAWN, true);
    view.setUint32(4, payload.length, true);
    frame.set(payload, 8);

    bridge.processPacket(frame);
    expect(bridge.worldObjects.has(objId)).toBe(true);
    const obj = bridge.worldObjects.get(objId);
    expect(obj.prefabId).toBe(12);
    expect(obj.x).toBeCloseTo(15.0, 5);
    expect(obj.y).toBeCloseTo(2.0, 5);
    expect(obj.z).toBeCloseTo(-30.0, 5);

    // Now test WorldObjectDespawn with varint object_id
    const despawnPayload = [...objIdBytes];
    const despawnFrame = new Uint8Array(8 + despawnPayload.length);
    const dView = new DataView(despawnFrame.buffer);
    despawnFrame[0] = 0x4d;
    despawnFrame[1] = 0x4d;
    dView.setUint16(2, OPCODES.WORLD_OBJECT_DESPAWN, true);
    dView.setUint32(4, despawnPayload.length, true);
    despawnFrame.set(despawnPayload, 8);

    bridge.processPacket(despawnFrame);
    expect(bridge.worldObjects.has(objId)).toBe(false);
  });

  it("preserves 64-bit GUIDs (> 2^24) losslessly through getRenderBuffer and getWorldObjectsRenderBuffer", () => {
    const bridge = new ClientWorldBridge();
    const entityGuid = (0xdead_beefn << 32n) | 0xcafe_baben; // Both halves > 2^24
    const localPlayerGuid = (0x1234_5678n << 32n) | 0x8765_4321n;
    bridge.localPlayerGuid = localPlayerGuid;

    // Add remote entity
    bridge.entities.set(entityGuid, {
      id: entityGuid,
      x: 10.0,
      y: 1.0,
      z: 20.0,
      yaw: 0.0,
      hpPct: 0.9,
      castPct: 0.0,
      type: 2,
    });

    const entityRenderBuf = bridge.getRenderBuffer({ x: 0, y: 0, z: 0 }, 0);
    const u32Entity = new Uint32Array(entityRenderBuf.buffer);

    // Entity 0 is local player
    const recLocalLo = BigInt(u32Entity[0]);
    const recLocalHi = BigInt(u32Entity[1]);
    expect((recLocalHi << 32n) | recLocalLo).toBe(localPlayerGuid);

    // Entity 1 is remote entity
    const recRemoteLo = BigInt(u32Entity[9]);
    const recRemoteHi = BigInt(u32Entity[10]);
    expect((recRemoteHi << 32n) | recRemoteLo).toBe(entityGuid);

    // Verify WorldRenderer.syncEntities decodes exact 64-bit GUID without truncation
    const scene = new THREE.Scene();
    const worldRenderer = new WorldRenderer(null, scene);
    worldRenderer.syncEntities(entityRenderBuf, localPlayerGuid);
    expect(worldRenderer.entities.has(entityGuid)).toBe(true);
    expect(worldRenderer.entities.has(localPlayerGuid)).toBe(true);

    // Test WorldObjects render buffer fidelity
    const objectGuid = (0xcafe_d00dn << 32n) | 0xbeef_baben;
    bridge.worldObjects.set(objectGuid, {
      id: objectGuid,
      prefabId: 11,
      x: 5.0,
      y: 0.0,
      z: -10.0,
      qx: 0,
      qy: 0,
      qz: 0,
      qw: 1,
      sx: 1,
      sy: 1,
      sz: 1,
      stateFlags: 0,
      hpPct: 1.0,
      revision: 1,
    });

    const objRenderBuf = bridge.getWorldObjectsRenderBuffer();
    const objRenderer = new WorldObjectRenderer(scene);
    objRenderer.updateFromBuffer(objRenderBuf);
    expect(objRenderer.objects.has(objectGuid)).toBe(true);

    // Test WorldRenderer.clear releases all GPU meshes
    worldRenderer.clear();
    expect(worldRenderer.entities.size).toBe(0);
  });

  it("ingests MOVEMENT_CORRECTION (0x0002) packet and fires callback with full precision", () => {
    const bridge = new ClientWorldBridge();
    const frame = new Uint8Array(8 + 37);
    const view = new DataView(frame.buffer);
    frame[0] = 0x4d;
    frame[1] = 0x4d;
    view.setUint16(2, OPCODES.MOVEMENT_CORRECTION, true);
    view.setUint32(4, 37, true);

    const targetGuid = (0x0001n << 48n) | 123456789n; // High entity type bits
    view.setBigUint64(8, targetGuid, true);
    view.setFloat32(16, 12.5, true);  // x
    view.setFloat32(20, 1.0, true);   // y
    view.setFloat32(24, -30.5, true); // z
    view.setFloat32(28, 0.5, true);   // vx
    view.setFloat32(32, 0.0, true);   // vy
    view.setFloat32(36, -0.5, true);  // vz
    view.setUint32(40, 42, true);     // sequence
    frame[8 + 36] = 1;                // reason: Reconciliation

    let receivedCorr = null;
    bridge.onMovementCorrection((corr) => {
      receivedCorr = corr;
    });

    bridge.processPacket(frame);

    expect(receivedCorr).not.toBeNull();
    expect(BigInt(receivedCorr.entityId)).toBe(targetGuid);
    expect(receivedCorr.x).toBeCloseTo(12.5, 4);
    expect(receivedCorr.y).toBeCloseTo(1.0, 4);
    expect(receivedCorr.z).toBeCloseTo(-30.5, 4);
    expect(receivedCorr.vx).toBeCloseTo(0.5, 4);
    expect(receivedCorr.vy).toBeCloseTo(0.0, 4);
    expect(receivedCorr.vz).toBeCloseTo(-0.5, 4);
    expect(receivedCorr.sequence).toBe(42);
    expect(receivedCorr.reason).toBe(1);
  });
});

describe("WebTransportClient", () => {
  it("startPingLoop transmits OPCODES.PING (0x0007) and receives OPCODES.PONG (0x0008) to update pingMs", () => {
    vi.useFakeTimers();
    const client = new WebTransportClient();
    client.connected = true;

    let sentData = null;
    client.datagramWriter = {
      write: vi.fn().mockImplementation((data) => {
        sentData = data;
        return Promise.resolve();
      }),
    };

    client.startPingLoop();
    vi.advanceTimersByTime(2100);

    expect(client.datagramWriter.write).toHaveBeenCalled();
    expect(sentData).not.toBeNull();
    expect(sentData[0]).toBe(0x4d);
    expect(sentData[1]).toBe(0x4d);
    const view = new DataView(sentData.buffer);
    expect(view.getUint16(2, true)).toBe(OPCODES.PING); // 0x0007
    expect(view.getUint32(4, true)).toBe(8); // Payload len 8

    // Test receiving OPCODES.PONG (0x0008)
    const pongBuf = new Uint8Array(24);
    pongBuf[0] = 0x4d;
    pongBuf[1] = 0x4d;
    const pView = new DataView(pongBuf.buffer);
    pView.setUint16(2, OPCODES.PONG, true);
    pView.setUint32(4, 16, true);
    const pingSentTs = client.lastPingSentTime;
    pView.setBigUint64(8, BigInt(Math.floor(pingSentTs)), true);
    pView.setBigUint64(16, BigInt(Date.now()), true);

    client.handleIncomingPacket(pongBuf);
    expect(client.pingMs).toBeGreaterThanOrEqual(0);

    client.disconnect();
    expect(client.connected).toBe(false);
    expect(client.pingInterval).toBeNull();
    vi.useRealTimers();
  });

  it("observes transport.closed and marks client disconnected and notifies status", async () => {
    const client = new WebTransportClient();
    let statusNotified = null;
    client.onStatus((s) => {
      statusNotified = s;
    });

    let resolveClosed;
    const closedPromise = new Promise((resolve) => {
      resolveClosed = resolve;
    });

    // Mock WebTransport instance
    const mockTransport = {
      ready: Promise.resolve(),
      closed: closedPromise,
      datagrams: {
        writable: { getWriter: () => ({ write: vi.fn(), close: vi.fn() }) },
        readable: { getReader: () => ({ read: () => new Promise(() => {}), releaseLock: vi.fn() }) },
      },
      incomingBidirectionalStreams: {
        getReader: () => ({ read: () => new Promise(() => {}), releaseLock: vi.fn() }),
      },
    };

    vi.stubGlobal("WebTransport", vi.fn().mockImplementation(() => mockTransport));

    await client.connect("https://127.0.0.1:4433");
    expect(client.connected).toBe(true);

    // Trigger server-side closed
    resolveClosed();
    await closedPromise;
    // Allow microtasks to complete
    await new Promise((r) => setTimeout(r, 10));

    expect(client.connected).toBe(false);
    expect(client.pingInterval).toBeNull();
    expect(statusNotified.status).toBe("disconnected");

    vi.unstubAllGlobals();
  });

  it("deriveWebSocketUrl converts https URLs to ws/wss based on page protocol and preserves explicit ws URLs", () => {
    expect(deriveWebSocketUrl("https://example.com:4433", { pageProtocol: "http:" })).toBe(
      "ws://example.com:8443"
    );
    expect(deriveWebSocketUrl("https://example.com:4433", { pageProtocol: "https:", wsPort: 9443 })).toBe(
      "wss://example.com:9443"
    );
    expect(deriveWebSocketUrl("ws://127.0.0.1:8443")).toBe("ws://127.0.0.1:8443");
    expect(deriveWebSocketUrl("wss://127.0.0.1:8443")).toBe("wss://127.0.0.1:8443");
  });

  it("falls back automatically to WebSocket when WebTransport handshake fails", async () => {
    const client = new WebTransportClient();
    const statuses = [];
    client.onStatus((s) => statuses.push(s));

    const failingTransport = {
      ready: Promise.reject(new Error("UDP blocked")),
      closed: Promise.resolve(),
      close: vi.fn(),
    };
    vi.stubGlobal("WebTransport", vi.fn().mockImplementation(() => failingTransport));

    let createdWs = null;
    class MockWebSocket {
      constructor(url) {
        this.url = url;
        this.binaryType = "";
        this.readyState = 0;
        this.send = vi.fn();
        this.close = vi.fn();
        createdWs = this;
        queueMicrotask(() => {
          this.readyState = 1;
          if (this.onopen) this.onopen();
        });
      }
    }
    vi.stubGlobal("WebSocket", MockWebSocket);

    await client.connect("https://127.0.0.1:4433", "", { wsPort: 8443 });
    expect(client.connected).toBe(true);
    expect(client.transportMode).toBe("websocket");
    expect(createdWs.url).toBe("ws://127.0.0.1:8443");
    expect(createdWs.binaryType).toBe("arraybuffer");
    expect(statuses.some((s) => s.status === "connected" && s.mode === "websocket")).toBe(true);

    // sendDatagram & sendStream both route through WebSocket in fallback mode
    const pkt1 = new Uint8Array([0x4d, 0x4d, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00]);
    const pkt2 = new Uint8Array([0x4d, 0x4d, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00]);
    await client.sendDatagram(pkt1);
    await client.sendStream(pkt2);
    expect(createdWs.send).toHaveBeenCalledTimes(2);
    expect(createdWs.send).toHaveBeenNthCalledWith(1, pkt1);
    expect(createdWs.send).toHaveBeenNthCalledWith(2, pkt2);

    // Coalesced multi-frame binary message slices each 0x4D4D frame
    const received = [];
    client.onPacket((frame) => received.push(new Uint8Array(frame)));
    const coalesced = new Uint8Array(pkt1.length + pkt2.length);
    coalesced.set(pkt1, 0);
    coalesced.set(pkt2, pkt1.length);
    createdWs.onmessage({ data: coalesced.buffer });
    expect(received.length).toBe(2);
    expect(received[0][2]).toBe(0x01);
    expect(received[1][2]).toBe(0x03);

    // Server close triggers disconnect status
    createdWs.onclose({ code: 1000, reason: "" });
    expect(client.connected).toBe(false);
    expect(client.transportMode).toBeNull();
    expect(statuses[statuses.length - 1].status).toBe("disconnected");

    await client.disconnect();
    vi.unstubAllGlobals();
  });

  it("connects directly via WebSocket when given an explicit ws:// URL", async () => {
    const client = new WebTransportClient();
    const wtSpy = vi.fn();
    vi.stubGlobal("WebTransport", wtSpy);

    class MockWebSocket {
      constructor(url) {
        this.url = url;
        this.binaryType = "";
        this.readyState = 1;
        this.send = vi.fn();
        this.close = vi.fn();
        queueMicrotask(() => {
          if (this.onopen) this.onopen();
        });
      }
    }
    vi.stubGlobal("WebSocket", MockWebSocket);

    await client.connect("ws://localhost:8443");
    expect(wtSpy).not.toHaveBeenCalled();
    expect(client.connected).toBe(true);
    expect(client.transportMode).toBe("websocket");

    await client.disconnect();
    vi.unstubAllGlobals();
  });
});

describe("UIController Village HUD", () => {
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
      attributes: {},
      setAttribute(name, val) {
        this.attributes[name] = String(val);
      },
      getAttribute(name) {
        return this.attributes[name] || null;
      },
      replaceChildren(...newChildren) {
        this.children = [...newChildren];
      },
      _innerHTML: "",
      get innerHTML() {
        if (this._innerHTML) return this._innerHTML;
        return this.children
          .map((c) => {
            if (c.className) {
              return `<span class="${c.className}">${c.textContent || c.innerHTML}</span>`;
            }
            return c.textContent || c.innerHTML;
          })
          .join("");
      },
      set innerHTML(val) {
        this._innerHTML = val;
        this.children = [];
      },
      appendChild(child) {
        this.children.push(child);
      },
      addEventListener(evt, fn) {
        this._listeners = this._listeners || {};
        this._listeners[evt] = fn;
      },
      click() {
        if (this._listeners && this._listeners["click"]) {
          this._listeners["click"]({ target: this });
        }
      },
      querySelector(sel) {
        if (!sel) return null;
        const targetClass = sel.startsWith(".") ? sel.slice(1) : sel;
        const findRecursive = (node) => {
          if (node.className && node.className.split(" ").includes(targetClass)) {
            return node;
          }
          for (const child of node.children) {
            const found = findRecursive(child);
            if (found) return found;
          }
          return null;
        };
        return findRecursive(this) || createMockElement();
      },
    };
    el.classList = {
      add(...tokens) {
        const current = new Set((el.className || "").split(/\s+/).filter(Boolean));
        tokens.forEach((t) => current.add(t));
        el.className = Array.from(current).join(" ");
      },
      remove(...tokens) {
        const current = new Set((el.className || "").split(/\s+/).filter(Boolean));
        tokens.forEach((t) => current.delete(t));
        el.className = Array.from(current).join(" ");
      },
      contains(token) {
        return (el.className || "").split(/\s+/).filter(Boolean).includes(token);
      },
      toggle(token, force) {
        const has = this.contains(token);
        const shouldAdd = force !== undefined ? Boolean(force) : !has;
        if (shouldAdd) this.add(token);
        else this.remove(token);
        return shouldAdd;
      },
    };
    return el;
  }

  beforeEach(() => {
    elements = new Map();
    elements.set("connection-error", createMockElement("connection-error", "connection-error hidden"));
    globalThis.document = {
      getElementById(id) {
        if (!elements.has(id)) {
          elements.set(id, createMockElement(id));
        }
        return elements.get(id);
      },
      createElement(tag) {
        return createMockElement("", "");
      },
    };
  });

  it("updates gold, weight, and inventory slots safely without innerHTML XSS", () => {
    const ui = new UIController();
    const xssPayload = "<script>alert('pwn')</script><img src=x onerror=alert(1)>";
    const inv = {
      currency: 85,
      current_weight: 15.5,
      max_weight: 100.0,
      slots: [
        { item_id: 1001, name: "Iron Ore", quantity: 3, unit_weight: 2.0 },
        { item_id: 9999, name: xssPayload, quantity: 1, unit_weight: 0.5 },
        null,
      ],
    };

    ui.updateInventory(inv);

    expect(document.getElementById("val-gold").textContent).toBe(85);
    expect(document.getElementById("val-weight").textContent).toBe("15.5 / 100.0 kg");
    const grid = document.getElementById("inventory-grid");
    expect(grid.children.length).toBe(3);
    expect(grid.children[0].className).toContain("occupied");
    
    // Normal slot DOM nodes
    const slot1 = grid.children[0];
    expect(slot1.children.length).toBe(3);
    expect(slot1.children[0].className).toBe("slot-item-name");
    expect(slot1.children[0].textContent).toBe("Iron Ore");
    expect(slot1.children[1].className).toBe("slot-item-qty");
    expect(slot1.children[1].textContent).toBe("x3");
    expect(slot1.children[2].className).toBe("slot-item-weight");
    expect(slot1.children[2].textContent).toBe("6.0kg");

    // XSS slot must use textContent and NOT raw innerHTML
    const slotXss = grid.children[1];
    expect(slotXss.children[0].className).toBe("slot-item-name");
    expect(slotXss.children[0].textContent).toBe(xssPayload);
    // Verified slot element did not set innerHTML directly
    expect(slotXss._innerHTML).toBe("");

    expect(grid.children[2].className).toContain("empty");
  });

  it("shows and populates NPC barter modal safely with recipe barter action callbacks", () => {
    const ui = new UIController();
    let barterRecipeId = null;

    const merchant = {
      name: "Blacksmith Ilmari",
      role: "Blacksmith",
      greeting: "Fine iron tools.",
      recipes: [
        {
          recipe_id: 1,
          required_items: [{ name: "<img src=x onerror=alert(1)>", quantity: 2 }],
          required_currency: 0,
          reward_items: [{ name: "Iron Pickaxe", quantity: 1 }],
          reward_currency: 0,
        },
      ],
    };

    ui.showNpcBarter(merchant, (recipeId) => {
      barterRecipeId = recipeId;
    });

    const modal = document.getElementById("npc-barter-modal");
    expect(modal.style.display).toBe("flex");
    expect(document.getElementById("npc-name").textContent).toBe("Blacksmith Ilmari");
    expect(document.getElementById("npc-greeting").textContent).toBe("Fine iron tools.");

    const recipesList = document.getElementById("barter-recipes-list");
    expect(recipesList.children.length).toBe(1);
    const card = recipesList.children[0];
    
    // Safe textContent check for recipe exchange with XSS payload
    const reqsSpan = card.querySelector(".recipe-reqs");
    expect(reqsSpan.textContent).toContain("2x <img src=x onerror=alert(1)>");
    // Card was built programmatically via createElement, not innerHTML
    expect(card._innerHTML).toBe("");

    const barterBtn = card.querySelector(".barter-action-btn");
    expect(barterBtn.getAttribute("data-recipe-id")).toBe("1");
    barterBtn.click();
    expect(barterRecipeId).toBe(1);

    ui.hideNpcBarter();
    expect(modal.style.display).toBe("none");
  });

  it("updates inventory weight using authoritative top-level current_weight from VillageTradeStateResponse", () => {
    const ui = new UIController();
    const inv = {
      currency: 120,
      max_weight: 150.0,
      slots: [],
    };
    ui.updateInventory(inv, 45.2);

    expect(document.getElementById("val-gold").textContent).toBe(120);
    expect(document.getElementById("val-weight").textContent).toBe("45.2 / 150.0 kg");
  });

  it("shows player trade modal with partner currency from their_offer.currency", () => {
    const ui = new UIController();
    const tradeSession = {
      session_id: 105,
      my_offer: { currency: 25, items: [] },
      their_offer: { currency: 90, items: [] },
      my_locked: true,
      their_locked: true,
      my_accepted: false,
      their_accepted: false,
      completed: false,
    };

    ui.showPlayerTrade(tradeSession);

    const modal = document.getElementById("player-trade-modal");
    expect(modal.style.display).toBe("flex");
    expect(document.getElementById("trade-session-id").textContent).toBe(105);
    expect(document.getElementById("trade-partner-gold").textContent).toBe(90);
    expect(document.getElementById("trade-status-msg").textContent).toBe("Both locked");

    ui.hidePlayerTrade();
    expect(modal.style.display).toBe("none");
  });

  it("displays connection failures in #connection-error and clears/hides on connecting or connected", () => {
    const ui = new UIController();
    const errorEl = document.getElementById("connection-error");
    const statusDot = document.getElementById("status-dot");
    const btnConnect = document.getElementById("btn-connect");
    const btnDisconnect = document.getElementById("btn-disconnect");

    expect(errorEl.classList.contains("hidden")).toBe(true);
    expect(errorEl.textContent).toBe("");

    // Connection failure surfaces error message
    ui.setConnectionState(
      "disconnected",
      "Disconnected",
      "WebTransportError: Opening handshake failed."
    );
    expect(statusDot.className).toBe("status-indicator disconnected");
    expect(btnConnect.disabled).toBe(false);
    expect(btnDisconnect.disabled).toBe(true);
    expect(errorEl.classList.contains("hidden")).toBe(false);
    expect(errorEl.textContent).toBe("WebTransportError: Opening handshake failed.");

    // Subsequent connecting attempt clears and hides the error
    ui.setConnectionState("connecting", "Connecting");
    expect(statusDot.className).toBe("status-indicator connecting");
    expect(btnConnect.disabled).toBe(true);
    expect(btnDisconnect.disabled).toBe(false);
    expect(errorEl.classList.contains("hidden")).toBe(true);
    expect(errorEl.textContent).toBe("");

    // Another failure shows again, then connected clears and hides it
    ui.setConnectionState("disconnected", "Disconnected", "WebSocket connection failed to ws://127.0.0.1:8443");
    expect(errorEl.classList.contains("hidden")).toBe(false);
    expect(errorEl.textContent).toBe("WebSocket connection failed to ws://127.0.0.1:8443");

    ui.setConnectionState("connected", "Connected");
    expect(statusDot.className).toBe("status-indicator connected");
    expect(btnConnect.disabled).toBe(true);
    expect(btnDisconnect.disabled).toBe(false);
    expect(errorEl.classList.contains("hidden")).toBe(true);
    expect(errorEl.textContent).toBe("");

    // Clean disconnect without error keeps #connection-error hidden and empty
    ui.setConnectionState("disconnected", "Disconnected", "");
    expect(errorEl.classList.contains("hidden")).toBe(true);
    expect(errorEl.textContent).toBe("");
  });

  it("formats connection errors and clears HUD error across failed-then-successful transport attempts", async () => {
    expect(formatConnectionError(new Error("Opening handshake failed."))).toBe(
      "Opening handshake failed."
    );
    expect(formatConnectionError("")).toBe("Connection failed.");
    expect(formatConnectionError("x".repeat(220)).length).toBe(180);

    const ui = new UIController();
    const client = new WebTransportClient();
    const errorEl = document.getElementById("connection-error");

    client.onStatus(({ status, error }) => {
      const statusText =
        status === "connected"
          ? "Connected"
          : status === "connecting"
          ? "Connecting"
          : "Disconnected";
      const errorMsg =
        status === "disconnected" && error ? formatConnectionError(error) : "";
      ui.setConnectionState(status, statusText, errorMsg);
    });

    vi.stubGlobal(
      "WebTransport",
      vi.fn().mockImplementation(() => ({
        ready: Promise.reject(new Error("WebTransportError: Opening handshake failed.")),
        closed: Promise.resolve(),
        close: vi.fn(),
      }))
    );

    let failWs = true;
    class MockWebSocket {
      constructor(url) {
        this.url = url;
        this.binaryType = "";
        this.readyState = 0;
        this.send = vi.fn();
        this.close = vi.fn();
        queueMicrotask(() => {
          if (failWs) {
            if (this.onerror) this.onerror(new Error("ECONNREFUSED"));
          } else {
            this.readyState = 1;
            if (this.onopen) this.onopen();
          }
        });
      }
    }
    vi.stubGlobal("WebSocket", MockWebSocket);

    await expect(client.connect("https://127.0.0.1:4433", "", { wsPort: 8443 })).rejects.toThrow(
      "WebSocket connection failed to ws://127.0.0.1:8443"
    );
    expect(errorEl.classList.contains("hidden")).toBe(false);
    expect(errorEl.textContent).toBe("WebSocket connection failed to ws://127.0.0.1:8443");

    // Retry succeeds -> #connection-error cleared and hidden
    failWs = false;
    await client.connect("https://127.0.0.1:4433", "", { wsPort: 8443 });
    expect(client.connected).toBe(true);
    expect(errorEl.classList.contains("hidden")).toBe(true);
    expect(errorEl.textContent).toBe("");

    await client.disconnect();
    vi.unstubAllGlobals();
  });
});

describe("EntityDeltaBatch and Protocol Extensions", () => {
  it("encodes ClientPacket::ClientAck with 0x4D4D magic, opcode 0x000A, and correct varint payload", () => {
    const frame = encodeClientAck(12345n, 888n, 0b101);
    expect(frame[0]).toBe(0x4d);
    expect(frame[1]).toBe(0x4d);
    const view = new DataView(frame.buffer);
    expect(view.getUint16(2, true)).toBe(OPCODES.CLIENT_ACK);

    const payloadLen = view.getUint32(4, true);
    expect(frame.length).toBe(8 + payloadLen);

    // Read payload
    const { value: sessId, bytesRead: b1 } = readVarintU64(view, 8);
    expect(sessId).toBe(12345n);
    const { value: obsTick, bytesRead: b2 } = readVarintU64(view, 8 + b1);
    expect(obsTick).toBe(888n);
    const ackMask = view.getUint32(8 + b1 + b2, true);
    expect(ackMask).toBe(5);
  });

  it("dequantizes displacement deltas, velocities, yaw, and pitch accurately", () => {
    // 128 units per meter
    expect(dequantizeDeltaI16(128)).toBeCloseTo(1.0, 5);
    expect(dequantizeDeltaI16(-256)).toBeCloseTo(-2.0, 5);

    // 100 units per m/s
    expect(dequantizeVelocityI16(550)).toBeCloseTo(5.5, 5);
    expect(dequantizeVelocityI16(-100)).toBeCloseTo(-1.0, 5);

    // 256 units per 2*PI radians
    expect(dequantizeYawU8(0)).toBeCloseTo(0.0, 5);
    expect(dequantizeYawU8(128)).toBeCloseTo(Math.PI, 2);

    // 127 units per PI/2 radians
    expect(dequantizePitchI8(0)).toBeCloseTo(0.0, 5);
    expect(dequantizePitchI8(127)).toBeCloseTo(Math.PI * 0.5, 3);
  });

  it("decodes EntityDeltaBatch packet, tracks ticks, despawns entities, and reconciles keyframes and deltas", () => {
    const bridge = new ClientWorldBridge();
    bridge.sessionId = 999;
    let notifiedTick = null;
    bridge.onServerTick((info) => {
      notifiedTick = info;
    });

    // Construct mock EntityDeltaBatch payload:
    // header: [0x4d, 0x4d, opcode: 0x0009 (2B), payloadLen (4B)]
    // payload:
    //   server_tick: 50 (varint)
    //   baseline_tick: 48 (varint)
    //   despawns: 1 item -> entity 101n (varint)
    //   deltas: 2 items
    //     delta 1: entity 200n (remote player type 1), MASK_KEYFRAME (x=10, y=0, z=-15), yaw=128
    //     delta 2: entity 300n (NPC type 2), MASK_KEYFRAME (x=0, y=0, z=0)
    const payload = [];
    putVarintU64(50, payload); // server_tick
    putVarintU64(48, payload); // baseline_tick

    // Despawns: 1 despawn
    payload.push(1); // despawn_count = 1
    putVarintU64(101, payload); // despawn entity 101

    // Pre-populate entity 101 in bridge to test despawn
    bridge.entities.set(101, { id: 101, x: 0, y: 0, z: 0 });

    // Deltas: 2 deltas
    payload.push(2); // delta_count = 2

    // Delta 1: remote player GUID with type=1 in upper bits: (1n << 48n) | 200n
    const playerGuid = (1n << 48n) | 200n;
    putVarintU64(playerGuid, payload);
    const mask1 = DELTA_FLAGS.MASK_KEYFRAME | DELTA_FLAGS.MASK_YAW | DELTA_FLAGS.MASK_HEALTH | DELTA_FLAGS.MASK_MAX_HEALTH;
    payload.push(mask1 & 0xff);
    payload.push((mask1 >> 8) & 0xff);
    // Keyframe 3x f32
    const f32Buf = new ArrayBuffer(12);
    const f32View = new DataView(f32Buf);
    f32View.setFloat32(0, 10.5, true);
    f32View.setFloat32(4, 1.0, true);
    f32View.setFloat32(8, -15.2, true);
    new Uint8Array(f32Buf).forEach((b) => payload.push(b));
    // Yaw u8 (128 -> ~PI)
    payload.push(128);
    // Health (varint u32)
    payload.push(75);
    // MaxHealth (varint u32)
    payload.push(100);

    // Delta 2: Villager NPC with type=2 in upper bits: (2n << 48n) | 300n
    const npcGuid = (2n << 48n) | 300n;
    putVarintU64(npcGuid, payload);
    const mask2 = DELTA_FLAGS.MASK_KEYFRAME;
    payload.push(mask2 & 0xff);
    payload.push((mask2 >> 8) & 0xff);
    f32View.setFloat32(0, -9.0, true);
    f32View.setFloat32(4, 0.0, true);
    f32View.setFloat32(8, -6.0, true);
    new Uint8Array(f32Buf).forEach((b) => payload.push(b));

    // Build complete frame
    const frame = new Uint8Array(8 + payload.length);
    const view = new DataView(frame.buffer);
    frame[0] = 0x4d;
    frame[1] = 0x4d;
    view.setUint16(2, OPCODES.ENTITY_DELTA_BATCH, true);
    view.setUint32(4, payload.length, true);
    frame.set(payload, 8);

    bridge.processPacket(frame);

    // Assert tick update and callback
    expect(bridge.latestServerTick).toBe(50);
    expect(bridge.latestBaselineTick).toBe(48);
    expect(notifiedTick).toEqual({ sessionId: 999, serverTick: 50, baselineTick: 48 });

    // Assert despawn occurred
    expect(bridge.entities.has(101)).toBe(false);

    // Assert remote player replica
    const p1 = bridge.entities.get(Number(playerGuid));
    expect(p1).toBeDefined();
    expect(p1.type).toBe(1); // Player
    expect(p1.x).toBeCloseTo(10.5, 4);
    expect(p1.y).toBeCloseTo(1.0, 4);
    expect(p1.z).toBeCloseTo(-15.2, 4);
    expect(p1.yaw).toBeCloseTo(Math.PI, 2);
    expect(p1.hpPct).toBeCloseTo(0.75, 4);

    // Assert NPC replica
    const npc = bridge.entities.get(Number(npcGuid));
    expect(npc).toBeDefined();
    expect(npc.type).toBe(2); // Villager NPC
    expect(npc.x).toBeCloseTo(-9.0, 4);
    expect(npc.z).toBeCloseTo(-6.0, 4);

    // Assert telemetry entity count
    const tele = bridge.getTelemetry();
    expect(tele.latest_server_tick).toBe(50);
    expect(tele.latest_baseline_tick).toBe(48);
    expect(tele.entity_count).toBeGreaterThanOrEqual(2);
  });

  it("safely handles truncated or corrupted EntityDeltaBatch packets without throwing", () => {
    const bridge = new ClientWorldBridge();
    const badFrame = new Uint8Array([0x4d, 0x4d, 0x09, 0x00, 0x05, 0x00, 0x00, 0x00, 0x80, 0x80]);
    expect(() => bridge.processPacket(badFrame)).not.toThrow();
  });

  it("invokes onLocalPlayerKeyframe when EntityDeltaBatch contains localPlayerGuid", () => {
    const bridge = new ClientWorldBridge();
    const localGuid = (1n << 48n) | 77n;
    bridge.localPlayerGuid = Number(localGuid);

    let keyframe = null;
    bridge.onLocalPlayerKeyframe((state) => {
      keyframe = state;
    });

    const payload = [];
    const pushVarint = (val) => {
      let v = BigInt(val);
      while (v >= 0x80n) {
        payload.push(Number((v & 0x7fn) | 0x80n));
        v >>= 7n;
      }
      payload.push(Number(v));
    };

    pushVarint(1); // server_tick
    pushVarint(0); // baseline_tick
    pushVarint(0); // despawn_count
    pushVarint(1); // delta_count
    pushVarint(localGuid);
    payload.push(DELTA_FLAGS.MASK_KEYFRAME & 0xff, (DELTA_FLAGS.MASK_KEYFRAME >> 8) & 0xff);
    const f32Buf = new ArrayBuffer(12);
    const f32View = new DataView(f32Buf);
    f32View.setFloat32(0, 0.0, true);
    f32View.setFloat32(4, 0.0, true);
    f32View.setFloat32(8, 5.0, true);
    new Uint8Array(f32Buf).forEach((b) => payload.push(b));

    const frame = new Uint8Array(8 + payload.length);
    const view = new DataView(frame.buffer);
    frame[0] = 0x4d;
    frame[1] = 0x4d;
    view.setUint16(2, OPCODES.ENTITY_DELTA_BATCH, true);
    view.setUint32(4, payload.length, true);
    frame.set(payload, 8);

    bridge.processPacket(frame);

    expect(keyframe).not.toBeNull();
    expect(keyframe.x).toBeCloseTo(0.0, 4);
    expect(keyframe.y).toBeCloseTo(0.0, 4);
    expect(keyframe.z).toBeCloseTo(5.0, 4);
  });
});

describe("Session Token & Multi-Session Isolation", () => {
  beforeEach(() => {
    const store = new Map();
    vi.stubGlobal("sessionStorage", {
      getItem: (k) => store.get(k) || null,
      setItem: (k, v) => store.set(k, String(v)),
      clear: () => store.clear(),
    });
    vi.stubGlobal("window", {
      sessionStorage: globalThis.sessionStorage,
      location: { search: "" },
    });
  });

  it("generates unique guest tokens per session when sessionStorage is empty", () => {
    const token1 = getOrCreateSessionToken();
    expect(token1).toMatch(/^guest-[a-z0-9]+-[a-z0-9]+$/);

    // Same session retrieves existing token
    const token2 = getOrCreateSessionToken();
    expect(token2).toBe(token1);
  });

  it("prioritizes ?token= URL parameter when supplied", () => {
    window.location.search = "?token=custom-test-agent";
    const token = getOrCreateSessionToken();
    expect(token).toBe("custom-test-agent");
  });
});

describe("Spherical Orbit Camera Calculations", () => {
  it("calculates spherical orbit camera coordinates around player position", () => {
    const renderer = new WorldRenderer(null);
    renderer.camera = new THREE.PerspectiveCamera();
    renderer.orbitYaw = 0.0;
    renderer.orbitPitch = 0.0; // horizontal
    renderer.orbitDistance = 20.0;

    const targetPos = { x: 10.0, y: 0.0, z: -5.0 };
    renderer.updateCamera(targetPos);

    // With yaw=0, pitch=0: sin(0)=0, cos(0)=1
    // cx = target.x + 20 * 0 = 10.0
    // cy = target.y + 1.5 + 20 * 0 = 1.5
    // cz = target.z + 20 * 1 = 15.0
    expect(renderer.camera.position.x).toBeCloseTo(10.0, 4);
    expect(renderer.camera.position.y).toBeCloseTo(1.5, 4);
    expect(renderer.camera.position.z).toBeCloseTo(15.0, 4);
  });
});
