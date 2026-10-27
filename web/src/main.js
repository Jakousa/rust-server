import { WebTransportClient } from "./transport.js";
import { WorldRenderer } from "./renderer.js";
import { PlayerControls } from "./controls.js";
import { UIController } from "./ui.js";
import {
  OPCODES,
  INTERACTION_TYPES,
  encodeAuthRequest,
  encodeInteraction,
  encodeMovement,
} from "./protocol.js";

/**
 * JS/WASM World Adapter providing unified interface for packet ingestion,
 * interpolation, and flat render buffer generation.
 */
export class ClientWorldBridge {
  constructor() {
    this.entities = new Map();
    this.worldObjects = new Map();
    this.latestServerTick = 0;
    this.latestBaselineTick = 0;
    this.receivedPackets = 0;
    this.localPlayerGuid = 1;
    this.sessionId = 0;
    this.clientTime = 0.0;
    this.onInteractionResultCallback = null;
    this.onAuthResponseCallback = null;
  }

  onInteractionResult(callback) {
    this.onInteractionResultCallback = callback;
  }

  onAuthResponse(callback) {
    this.onAuthResponseCallback = callback;
  }

  processPacket(data) {
    const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
    if (view.byteLength < 4) return;

    // Check MM magic (0x4D, 0x4D)
    if (view.getUint8(0) !== 0x4d || view.getUint8(1) !== 0x4d) return;
    const opcode = view.getUint16(2, true);

    this.receivedPackets++;

    // Opcode 0x0012: AuthResponse
    if (opcode === OPCODES.AUTH_RESPONSE && view.byteLength >= 33) {
      const success = view.getUint8(8) === 1;
      const errorCode = view.getUint32(9, true);
      const sessionId = Number(view.getBigUint64(13, true));
      const playerGuid = Number(view.getBigUint64(21, true));
      if (success) {
        this.sessionId = sessionId;
        this.localPlayerGuid = playerGuid;
        console.log(`[Auth] Authenticated as Player Guid ${playerGuid}, Session ${sessionId}`);
        if (this.onAuthResponseCallback) {
          this.onAuthResponseCallback({ sessionId, playerGuid, success, errorCode });
        }
      }
      return;
    }

    // Opcode 0x0004: InteractionResult
    if (opcode === OPCODES.INTERACTION_RESULT && view.byteLength >= 21) {
      const actionId = view.getUint32(8, true);
      const success = view.getUint8(12) === 1;
      const errorCode = view.getUint32(13, true);
      const payloadLen = view.getUint32(17, true);
      let payloadJson = null;
      let payloadText = "";
      if (view.byteLength >= 21 + payloadLen && payloadLen > 0) {
        const payloadBytes = new Uint8Array(data.buffer, data.byteOffset + 21, payloadLen);
        payloadText = new TextDecoder().decode(payloadBytes);
        try {
          payloadJson = JSON.parse(payloadText);
        } catch (_) {}
      }

      if (this.onInteractionResultCallback) {
        this.onInteractionResultCallback({
          actionId,
          success,
          errorCode,
          text: payloadText,
          data: payloadJson,
        });
      }
      return;
    }

    // Opcode 0x0005: EntityState
    if (opcode === OPCODES.ENTITY_STATE && view.byteLength >= 68) {
      const entityId = Number(view.getBigUint64(8, true));
      const x = view.getFloat32(16, true);
      const y = view.getFloat32(20, true);
      const z = view.getFloat32(24, true);
      const yaw = view.getFloat32(40, true);
      const health = view.getUint32(48, true);
      const maxHealth = view.getUint32(52, true);

      const entityType = Number((BigInt(entityId) >> 48n) & 0xffffn);

      this.entities.set(entityId, {
        id: entityId,
        x,
        y,
        z,
        yaw,
        hpPct: maxHealth > 0 ? health / maxHealth : 1.0,
        castPct: 0.0,
        type: entityType,
      });
      return;
    }

    // Opcode 0x0006: EntityDespawn
    if (opcode === OPCODES.ENTITY_DESPAWN && view.byteLength >= 16) {
      const entityId = Number(view.getBigUint64(8, true));
      this.entities.delete(entityId);
      return;
    }

    // Opcode 0x0009: EntityDeltaBatch
    if (opcode === OPCODES.ENTITY_DELTA_BATCH && view.byteLength >= 24) {
      this.latestServerTick = Number(view.getBigUint64(8, true));
      this.latestBaselineTick = Number(view.getBigUint64(16, true));
      // Entity deltas are applied to replicas
    }

    // Opcode 0x000B: WorldObjectSpawn
    if (opcode === OPCODES.WORLD_OBJECT_SPAWN && view.byteLength >= 48) {
      // Simplified parsing for world object mock adapter
      const objId = Number(view.getBigUint64(8, true));
      const prefabId = view.getUint32(16, true);
      const x = view.getFloat32(20, true);
      const y = view.getFloat32(24, true);
      const z = view.getFloat32(28, true);
      this.worldObjects.set(objId, {
        id: objId,
        prefabId,
        x,
        y,
        z,
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
    }

    // Opcode 0x000D: WorldObjectDespawn
    if (opcode === OPCODES.WORLD_OBJECT_DESPAWN && view.byteLength >= 16) {
      const objId = Number(view.getBigUint64(8, true));
      this.worldObjects.delete(objId);
    }
  }

  update(dt) {
    this.clientTime += dt;
  }

  getRenderBuffer(localPlayerPos, localPlayerYaw) {
    // Layout: [guid_lo, guid_hi, x, y, z, yaw, hp_pct, cast_pct, entity_type] * N
    const buffer = [];

    const localGuidBigInt = BigInt(this.localPlayerGuid);
    const localLo = Number(localGuidBigInt & 0xffffffffn);
    const localHi = Number((localGuidBigInt >> 32n) & 0xffffffffn);

    // Local player entry
    buffer.push(
      localLo,
      localHi,
      localPlayerPos.x,
      localPlayerPos.y,
      localPlayerPos.z,
      localPlayerYaw,
      1.0,
      0.0,
      1.0
    );

    // Remote entity entries
    for (const ent of this.entities.values()) {
      if (ent.id === this.localPlayerGuid) continue;
      const entGuidBigInt = BigInt(ent.id);
      const entLo = Number(entGuidBigInt & 0xffffffffn);
      const entHi = Number((entGuidBigInt >> 32n) & 0xffffffffn);
      buffer.push(
        entLo,
        entHi,
        ent.x,
        ent.y,
        ent.z,
        ent.yaw,
        ent.hpPct,
        ent.castPct,
        ent.type
      );
    }

    return new Float32Array(buffer);
  }

  getWorldObjectsRenderBuffer() {
    const buffer = [];
    for (const obj of this.worldObjects.values()) {
      const objIdBigInt = BigInt(obj.id);
      const objLo = Number(objIdBigInt & 0xffffffffn);
      const objHi = Number((objIdBigInt >> 32n) & 0xffffffffn);
      buffer.push(
        objLo,
        obj.prefabId,
        obj.x,
        obj.y,
        obj.z,
        obj.qx,
        obj.qy,
        obj.qz,
        obj.qw,
        obj.sx,
        obj.sy,
        obj.sz,
        obj.stateFlags,
        obj.hpPct,
        obj.revision,
        objHi
      );
    }
    return new Float32Array(buffer);
  }

  getTelemetryJson() {
    return JSON.stringify({
      entity_count: this.entities.size + 1,
      latest_server_tick: this.latestServerTick,
      latest_baseline_tick: this.latestBaselineTick,
      received_packets: this.receivedPackets,
    });
  }
}

/**
 * Automatically retrieves the ephemeral SHA-256 certificate digest
 * exposed by TelemetryServer GET /cert-hash.
 */
export async function autoFetchCertHash() {
  const certInput = document.getElementById("cert-hash");
  if (certInput && certInput.value.trim().length > 0) return;
  try {
    const adminUrl = `http://${window.location.hostname || "127.0.0.1"}:9091/cert-hash`;
    const res = await fetch(adminUrl);
    if (res.ok) {
      const data = await res.json();
      if (data && data.cert_hash) {
        if (certInput) certInput.value = data.cert_hash;
        console.log("[WebTransport] Auto-discovered certificate hash:", data.cert_hash);
      }
    }
  } catch (err) {
    console.debug("[WebTransport] Cert discovery fallback:", err);
  }
}

// Application Bootstrap
if (typeof window !== "undefined") {
  window.addEventListener("DOMContentLoaded", async () => {
  const container = document.getElementById("canvas-container");
  const renderer = new WorldRenderer(container);
  const controls = new PlayerControls();
  const transport = new WebTransportClient();
  const ui = new UIController();
  const world = new ClientWorldBridge();

  // Try to load cert hash from URL parameters if provided (e.g. ?cert=aa:bb:cc...)
  const params = new URLSearchParams(window.location.search);
  const certFromUrl = params.get("cert");
  if (certFromUrl) {
    const certInput = document.getElementById("cert-hash");
    if (certInput) certInput.value = certFromUrl;
  } else {
    await autoFetchCertHash();
  }

  let nearestTarget = null;

  // Interaction response listener for Village HUD updates
  world.onInteractionResult((res) => {
    if (!res.data) return;

    // 1. Update player inventory if included
    if (res.data.player_inventory) {
      ui.updateInventory(res.data.player_inventory);
    }

    // 2. Open NPC Barter modal if merchant data present
    if (res.data.npc_merchant && nearestTarget) {
      ui.showNpcBarter(res.data.npc_merchant, (recipeId) => {
        // Send Sub-opcode 2: NpcBarter
        const actionId = (0x0002 << 16) | 2;
        const payload = new TextEncoder().encode(
          JSON.stringify({ target_guid: nearestTarget.id, recipe_id: recipeId })
        );
        const pkt = encodeInteraction(
          world.localPlayerGuid,
          nearestTarget.id,
          actionId,
          payload,
          INTERACTION_TYPES.TRADE
        );
        transport.sendStream(pkt);
      });
    }

    // 3. Open Player Trade modal if active trade session present
    if (res.data.trade_session) {
      ui.showPlayerTrade(res.data.trade_session);
    }
  });

  // Network Packet Ingestion
  transport.onPacket((data) => {
    world.processPacket(data);
  });

  transport.onStatus(({ status, error }) => {
    ui.setConnectionState(status);
    if (error) {
      console.warn("Transport status notice:", error);
    }
  });

  function performInspect(target) {
    if (!transport.connected || !target) return;
    // Sub-opcode 1: Inspect
    const actionId = (0x0002 << 16) | 1;
    const payload = new TextEncoder().encode(JSON.stringify({ target_guid: target.id }));
    const pkt = encodeInteraction(
      world.localPlayerGuid,
      target.id,
      actionId,
      payload,
      INTERACTION_TYPES.TRADE
    );
    transport.sendStream(pkt);
  }

  // UI Event Bindings
  ui.initEvents({
    onConnect: async (url, certHash) => {
      try {
        await transport.connect(url, certHash);
        // Send AuthRequest handshake upon connection
        const authPkt = encodeAuthRequest("guest-poc", 1);
        await transport.sendStream(authPkt);

        // Initial inventory inspection query
        setTimeout(() => {
          performInspect({ id: 0 });
        }, 200);
      } catch (err) {
        alert("Failed to connect via WebTransport: " + err.message);
      }
    },
    onDisconnect: async () => {
      await transport.disconnect();
    },
    onTogglePartitions: (visible) => renderer.setPartitionsVisible(visible),
    onToggleAoi: (visible) => renderer.setAoiVisible(visible),
    onToggleWireframe: (wireframe) => renderer.setWireframe(wireframe),
    onInteract: () => {
      if (nearestTarget) {
        performInspect(nearestTarget);
      }
    },
    onTradeOffer: (gold, items) => {
      if (!transport.connected) return;
      const actionId = (0x0002 << 16) | 4; // Sub-opcode 4: Offer
      const payload = new TextEncoder().encode(
        JSON.stringify({ session_id: 1, currency: gold, items })
      );
      const pkt = encodeInteraction(
        world.localPlayerGuid,
        nearestTarget ? nearestTarget.id : 0,
        actionId,
        payload,
        INTERACTION_TYPES.TRADE
      );
      transport.sendStream(pkt);
    },
    onTradeAccept: () => {
      if (!transport.connected) return;
      const actionId = (0x0002 << 16) | 5; // Sub-opcode 5: Accept
      const payload = new TextEncoder().encode(JSON.stringify({ session_id: 1 }));
      const pkt = encodeInteraction(
        world.localPlayerGuid,
        nearestTarget ? nearestTarget.id : 0,
        actionId,
        payload,
        INTERACTION_TYPES.TRADE
      );
      transport.sendStream(pkt);
    },
    onTradeCancel: () => {
      if (!transport.connected) return;
      const actionId = (0x0002 << 16) | 6; // Sub-opcode 6: Cancel
      const payload = new TextEncoder().encode(JSON.stringify({ session_id: 1 }));
      const pkt = encodeInteraction(
        world.localPlayerGuid,
        nearestTarget ? nearestTarget.id : 0,
        actionId,
        payload,
        INTERACTION_TYPES.TRADE
      );
      transport.sendStream(pkt);
    },
  });

  // Action Cast Hook
  controls.onCast(() => {
    if (!transport.connected) return;
    const pkt = encodeInteraction(
      world.localPlayerGuid,
      nearestTarget ? nearestTarget.id : 0,
      42,
      new Uint8Array(0),
      INTERACTION_TYPES.ATTACK
    );
    transport.sendStream(pkt);
  });

  // Keyboard shortcut: E to interact with nearest target
  window.addEventListener("keydown", (e) => {
    if ((e.key === "e" || e.key === "E") && !e.repeat) {
      // If modal is open, ignore
      if (ui.npcBarterModal && ui.npcBarterModal.style.display !== "none") return;
      if (ui.playerTradeModal && ui.playerTradeModal.style.display !== "none") return;

      if (nearestTarget) {
        performInspect(nearestTarget);
      }
    }
  });

  // Fixed 20 Hz movement update interval (50ms)
  let lastMoveSendTime = 0;
  const MOVE_INTERVAL_MS = 50;
  const MAX_INTERACT_DISTANCE = 5.0;

  let lastFrameTime = performance.now();

  function animate(now) {
    requestAnimationFrame(animate);

    const dt = Math.min((now - lastFrameTime) / 1000, 0.1);
    lastFrameTime = now;

    // 1. Advance controls and world interpolation
    controls.update(dt);
    world.update(dt);

    // 2. Discover nearest target entity within interact distance
    let closest = null;
    let minDistance = Infinity;
    const playerPos = controls.position;

    for (const ent of world.entities.values()) {
      if (ent.id === world.localPlayerGuid) continue;
      const dx = ent.x - playerPos.x;
      const dy = ent.y - playerPos.y;
      const dz = ent.z - playerPos.z;
      const dist = Math.hypot(dx, dy, dz);
      if (dist < minDistance && dist <= MAX_INTERACT_DISTANCE) {
        minDistance = dist;
        closest = { ...ent, distance: dist };
      }
    }

    nearestTarget = closest;
    if (nearestTarget) {
      let targetName = `Entity #${nearestTarget.id}`;
      let targetType = "Creature";
      if (nearestTarget.type === 2) {
        targetType = "Villager NPC";
        targetName =
          nearestTarget.id % 3 === 1
            ? "Blacksmith Ilmari"
            : nearestTarget.id % 3 === 2
            ? "Herbalist Aino"
            : "Elder Väinö";
      } else if (nearestTarget.type === 1) {
        targetType = "Player";
        targetName = `Traveler #${nearestTarget.id}`;
      }
      ui.showTarget(targetName, targetType, nearestTarget.distance);
    } else {
      ui.hideTarget();
    }

    // 3. Transmit 20 Hz movement updates over WebTransport datagrams
    if (now - lastMoveSendTime >= MOVE_INTERVAL_MS) {
      lastMoveSendTime = now;
      if (transport.connected) {
        const sample = controls.sample();
        const moveBuf = encodeMovement(world.localPlayerGuid, sample);
        transport.sendDatagram(moveBuf);
      }
    }

    // 4. Render synchronized 3D scene
    const renderBuffer = world.getRenderBuffer(controls.position, controls.yaw);
    renderer.syncEntities(renderBuffer, world.localPlayerGuid);
    renderer.updateWorldObjects(world.getWorldObjectsRenderBuffer());
    renderer.updateCamera(controls.position);
    renderer.render();

    // 5. Update HUD Telemetry
    ui.updateTelemetry(
      world.getTelemetryJson(),
      transport.pingMs,
      transport.bandwidthBps,
      controls.position
    );
  }

  requestAnimationFrame(animate);
});
}
