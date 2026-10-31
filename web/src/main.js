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
  readVarintU64,
  readVarintU32,
  readVarintString,
  skipPropertyMap,
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

    // Opcode 0x000B: WorldObjectSpawn (LEB128 varints for object_id, prefab_id)
    if (opcode === OPCODES.WORLD_OBJECT_SPAWN && view.byteLength >= 10) {
      let offset = 8;
      const { value: objIdBig, bytesRead: b1 } = readVarintU64(view, offset);
      offset += b1;
      const { value: prefabId, bytesRead: b2 } = readVarintU32(view, offset);
      offset += b2;

      let x = 0, y = 0, z = 0;
      let qx = 0, qy = 0, qz = 0, qw = 1;
      let sx = 1, sy = 1, sz = 1;
      let stateFlags = 0;
      let hpPct = 1.0;
      let revision = 1;

      if (view.byteLength >= offset + 16 * 4) {
        x = view.getFloat32(offset, true);
        y = view.getFloat32(offset + 4, true);
        z = view.getFloat32(offset + 8, true);
        qx = view.getFloat32(offset + 12, true);
        qy = view.getFloat32(offset + 16, true);
        qz = view.getFloat32(offset + 20, true);
        qw = view.getFloat32(offset + 24, true);
        sx = view.getFloat32(offset + 28, true);
        sy = view.getFloat32(offset + 32, true);
        sz = view.getFloat32(offset + 36, true);
        // bounds_min (12 bytes) + bounds_max (12 bytes)
        offset += 16 * 4;

        if (offset < view.byteLength) {
          const sfRes = readVarintU32(view, offset);
          stateFlags = sfRes.value;
          offset += sfRes.bytesRead;
          if (offset < view.byteLength) {
            // owner_type
            offset += 1;
            const sRes = readVarintString(view, offset);
            offset += sRes.bytesRead;
            if (offset < view.byteLength) {
              const hasHealth = view.getUint8(offset);
              offset += 1;
              if (hasHealth !== 0 && offset < view.byteLength) {
                const curHp = readVarintU32(view, offset);
                offset += curHp.bytesRead;
                const maxHp = readVarintU32(view, offset);
                offset += maxHp.bytesRead;
                hpPct = maxHp.value > 0 ? curHp.value / maxHp.value : 1.0;
              }
              const propBytes = skipPropertyMap(view, offset);
              offset += propBytes;
              if (offset < view.byteLength) {
                const revRes = readVarintU64(view, offset);
                revision = Number(revRes.value);
              }
            }
          }
        }
      }

      this.worldObjects.set(objIdBig, {
        id: objIdBig,
        prefabId,
        x,
        y,
        z,
        qx,
        qy,
        qz,
        qw,
        sx,
        sy,
        sz,
        stateFlags,
        hpPct,
        revision,
      });
      return;
    }

    // Opcode 0x000D: WorldObjectDespawn (LEB128 varint for object_id)
    if (opcode === OPCODES.WORLD_OBJECT_DESPAWN && view.byteLength >= 9) {
      const { value: objIdBig } = readVarintU64(view, 8);
      this.worldObjects.delete(objIdBig);
      this.worldObjects.delete(Number(objIdBig));
      return;
    }
  }

  update(dt) {
    this.clientTime += dt;
  }

  getRenderBuffer(localPlayerPos, localPlayerYaw) {
    // Layout: [guid_lo, guid_hi, x, y, z, yaw, hp_pct, cast_pct, entity_type] * N
    // GUID lo and hi halves are packed losslessly via Uint32Array bit-reinterpretation
    const entityStride = 9;
    const remoteCount = Array.from(this.entities.values()).filter(
      (e) => BigInt(e.id) !== BigInt(this.localPlayerGuid)
    ).length;
    const totalCount = 1 + remoteCount;
    const floatBuffer = new Float32Array(totalCount * entityStride);
    const uintBuffer = new Uint32Array(floatBuffer.buffer);

    let offset = 0;
    const localGuidBigInt = BigInt(this.localPlayerGuid);
    uintBuffer[offset] = Number(localGuidBigInt & 0xffffffffn) >>> 0;
    uintBuffer[offset + 1] = Number((localGuidBigInt >> 32n) & 0xffffffffn) >>> 0;
    floatBuffer[offset + 2] = localPlayerPos.x;
    floatBuffer[offset + 3] = localPlayerPos.y;
    floatBuffer[offset + 4] = localPlayerPos.z;
    floatBuffer[offset + 5] = localPlayerYaw;
    floatBuffer[offset + 6] = 1.0;
    floatBuffer[offset + 7] = 0.0;
    floatBuffer[offset + 8] = 1.0;
    offset += entityStride;

    for (const ent of this.entities.values()) {
      if (BigInt(ent.id) === BigInt(this.localPlayerGuid)) continue;
      const entGuidBigInt = BigInt(ent.id);
      uintBuffer[offset] = Number(entGuidBigInt & 0xffffffffn) >>> 0;
      uintBuffer[offset + 1] = Number((entGuidBigInt >> 32n) & 0xffffffffn) >>> 0;
      floatBuffer[offset + 2] = ent.x;
      floatBuffer[offset + 3] = ent.y;
      floatBuffer[offset + 4] = ent.z;
      floatBuffer[offset + 5] = ent.yaw;
      floatBuffer[offset + 6] = ent.hpPct;
      floatBuffer[offset + 7] = ent.castPct;
      floatBuffer[offset + 8] = ent.type;
      offset += entityStride;
    }

    return floatBuffer;
  }

  getWorldObjectsRenderBuffer() {
    // Layout: 16 floats per object. GUID lo (offset 0) and hi (offset 15) packed losslessly
    const STRIDE = 16;
    const floatBuffer = new Float32Array(this.worldObjects.size * STRIDE);
    const uintBuffer = new Uint32Array(floatBuffer.buffer);

    let offset = 0;
    for (const obj of this.worldObjects.values()) {
      const objIdBigInt = BigInt(obj.id);
      uintBuffer[offset] = Number(objIdBigInt & 0xffffffffn) >>> 0;
      floatBuffer[offset + 1] = obj.prefabId;
      floatBuffer[offset + 2] = obj.x;
      floatBuffer[offset + 3] = obj.y;
      floatBuffer[offset + 4] = obj.z;
      floatBuffer[offset + 5] = obj.qx;
      floatBuffer[offset + 6] = obj.qy;
      floatBuffer[offset + 7] = obj.qz;
      floatBuffer[offset + 8] = obj.qw;
      floatBuffer[offset + 9] = obj.sx;
      floatBuffer[offset + 10] = obj.sy;
      floatBuffer[offset + 11] = obj.sz;
      floatBuffer[offset + 12] = obj.stateFlags;
      floatBuffer[offset + 13] = obj.hpPct;
      floatBuffer[offset + 14] = obj.revision;
      uintBuffer[offset + 15] = Number((objIdBigInt >> 32n) & 0xffffffffn) >>> 0;
      offset += STRIDE;
    }
    return floatBuffer;
  }

  /**
   * Returns a reused telemetry object (no per-frame allocation or JSON round-trip).
   * The returned object is overwritten on the next call; do not retain it.
   */
  getTelemetry() {
    const t = (this._telemetry ??= {});
    t.entity_count = this.entities.size + 1;
    t.latest_server_tick = this.latestServerTick;
    t.latest_baseline_tick = this.latestBaselineTick;
    t.received_packets = this.receivedPackets;
    return t;
  }

  getTelemetryJson() {
    return JSON.stringify(this.getTelemetry());
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
  let activeTradeSessionId = 0;

  // Interaction response listener for Village HUD updates
  world.onInteractionResult((res) => {
    if (!res.data) return;

    // 1. Update player inventory if included (authoritative VillageTradeStateResponse.inventory)
    const inv = res.data.inventory || res.data.player_inventory;
    const currentWeight = res.data.current_weight ?? inv?.current_weight;
    if (inv) {
      ui.updateInventory(inv, currentWeight);
    }

    // 2. Open NPC Barter modal if merchant data present (authoritative target_merchant)
    const merchant = res.data.target_merchant || res.data.npc_merchant;
    if (merchant && nearestTarget) {
      ui.showNpcBarter(merchant, (recipeId) => {
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

    // 3. Open Player Trade modal if active trade session present (authoritative active_trade)
    const tradeSession = res.data.active_trade || res.data.trade_session;
    if (tradeSession) {
      activeTradeSessionId = tradeSession.session_id || activeTradeSessionId;
      ui.showPlayerTrade(tradeSession);
    }

    // 4. Close trade modal if trade completed
    if (res.data.trade_completed || (tradeSession && tradeSession.completed)) {
      ui.hidePlayerTrade();
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
        JSON.stringify({ session_id: activeTradeSessionId, currency: gold, items })
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
      const payload = new TextEncoder().encode(
        JSON.stringify({ session_id: activeTradeSessionId })
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
    onTradeCancel: () => {
      if (!transport.connected) return;
      const actionId = (0x0002 << 16) | 6; // Sub-opcode 6: Cancel
      const payload = new TextEncoder().encode(
        JSON.stringify({ session_id: activeTradeSessionId })
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
      world.getTelemetry(),
      transport.pingMs,
      transport.bandwidthBps,
      controls.position
    );
  }

  requestAnimationFrame(animate);
});
}
