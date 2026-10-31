import { describe, it, expect, beforeEach, vi } from "vitest";
import * as THREE from "three";
import { PlayerControls } from "../controls.js";
import { WorldObjectRenderer } from "../world.js";
import { WorldRenderer } from "../renderer.js";
import { ClientWorldBridge } from "../main.js";
import { UIController } from "../ui.js";
import { WebTransportClient } from "../transport.js";
import {
  OPCODES,
  INTERACTION_TYPES,
  encodeAuthRequest,
  encodeInteraction,
  encodeMovement,
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
});

describe("UIController Village HUD", () => {
  let elements;

  function createMockElement(id = "", className = "") {
    return {
      id,
      className,
      style: {},
      children: [],
      textContent: "",
      value: "",
      disabled: false,
      _innerHTML: "",
      get innerHTML() {
        return this._innerHTML;
      },
      set innerHTML(val) {
        this._innerHTML = val;
        if (typeof val === "string" && val.includes("<button")) {
          const btn = createMockElement("", "barter-action-btn");
          this.children = [btn];
        }
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
        return this.children[0] || createMockElement();
      },
    };
  }

  beforeEach(() => {
    elements = new Map();
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

  it("updates gold, weight, and inventory slots", () => {
    const ui = new UIController();
    const inv = {
      currency: 85,
      current_weight: 15.5,
      max_weight: 100.0,
      slots: [
        { item_id: 1001, name: "Iron Ore", quantity: 3, unit_weight: 2.0 },
        null,
      ],
    };

    ui.updateInventory(inv);

    expect(document.getElementById("val-gold").textContent).toBe(85);
    expect(document.getElementById("val-weight").textContent).toBe("15.5 / 100.0 kg");
    const grid = document.getElementById("inventory-grid");
    expect(grid.children.length).toBe(2);
    expect(grid.children[0].className).toContain("occupied");
    expect(grid.children[0].innerHTML).toContain("Iron Ore");
    expect(grid.children[1].className).toContain("empty");
  });

  it("shows and populates NPC barter modal with recipe barter action callbacks", () => {
    const ui = new UIController();
    let barterRecipeId = null;

    const merchant = {
      name: "Blacksmith Ilmari",
      role: "Blacksmith",
      greeting: "Fine iron tools.",
      recipes: [
        {
          recipe_id: 1,
          required_items: [{ name: "Iron Ore", quantity: 2 }],
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
    expect(card.innerHTML).toContain("2x Iron Ore");
    expect(card.innerHTML).toContain("1x Iron Pickaxe");

    const barterBtn = card.querySelector(".barter-action-btn");
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
});
