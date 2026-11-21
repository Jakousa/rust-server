import * as THREE from "three";
import { WebTransportClient } from "./transport.js";
import { WorldRenderer } from "./renderer.js";
import { PlayerControls } from "./controls.js";
import { UIController } from "./ui.js";
import {
  AsteroidRenderer,
  HOME_STATION_SAFE_RADIUS,
} from "./games/asteroid/renderer.js";
import { SpaceFlightControls } from "./games/asteroid/controls.js";
import {
  AsteroidVoxelClient,
  VOXEL_MODULE_ID,
  VOXEL_EMPTY,
  voxelCoordToChunk,
  chunkKey,
} from "./games/asteroid/voxel_client.js";
import {
  OPCODES,
  INTERACTION_TYPES,
  DELTA_FLAGS,
  encodeAuthRequest,
  encodeInteraction,
  encodeMovement,
  encodeClientAck,
  dequantizeDeltaI16,
  dequantizeVelocityI16,
  dequantizeYawU8,
  dequantizePitchI8,
  readVarintU64,
  readVarintU32,
  readVarintString,
  readPropertyMap,
} from "./protocol.js";

export const VILLAGE_MODULE_ID = 0x0002;
export const ASTEROID_MODULE_ID = 0x0003;
export const PVP_LASER_RANGE = 45.0;

/**
 * Resolves the active game cartridge mode (`"village"` or `"asteroid"`).
 * Priority: `?game=asteroid|village` URL query -> URL subpath (`/asteroid` or `/village`) -> `envMode` (`VITE_GAME_MODE`) -> `"village"`.
 */
export function resolveGameMode(search = "", envMode = "", pathname = "") {
  const normalize = (raw) => {
    if (typeof raw !== "string") return "";
    const val = raw.trim().toLowerCase();
    if (
      val === "asteroid" ||
      val === "asteroids" ||
      val === "asteroid-miners" ||
      val === "game-b"
    ) {
      return "asteroid";
    }
    if (
      val === "village" ||
      val === "village-rpg" ||
      val === "game-a"
    ) {
      return "village";
    }
    return "";
  };

  if (typeof search === "string" && search.trim().length > 0) {
    try {
      const qs = search.startsWith("?") ? search : `?${search}`;
      const params = new URLSearchParams(qs);
      const fromQuery = normalize(params.get("game") || "");
      if (fromQuery) return fromQuery;
    } catch (_) {
      // Ignore malformed query strings
    }
  }

  if (typeof pathname === "string" && pathname.trim().length > 0) {
    const segments = pathname.split("/").filter(Boolean);
    for (const seg of segments) {
      const fromPath = normalize(seg);
      if (fromPath) return fromPath;
    }
  }

  const fromEnv = normalize(envMode);
  if (fromEnv) return fromEnv;

  return "village";
}

/**
 * Resolves per-game WebTransport, WebSocket fallback, and cert-hash discovery endpoints.
 */
export function resolveGatewayConfig(gameMode = "village", locationObj = null) {
  const loc =
    locationObj ||
    (typeof window !== "undefined" ? window.location : null);
  const host = loc?.hostname || "127.0.0.1";
  const originHost = loc?.host || host;
  const wsProto = loc?.protocol === "https:" ? "wss:" : "ws:";
  const mode = gameMode === "asteroid" ? "asteroid" : "village";

  if (mode === "asteroid") {
    return {
      mode: "asteroid",
      subpath: "/asteroid",
      wtUrl: `https://${host}:4434`,
      wsPathUrl: `${wsProto}//${originHost}/asteroid/ws`,
      wsPortUrl: `${wsProto}//${host}:8444`,
      wsPort: 8444,
      adminPort: 9092,
      certHashPaths: ["/asteroid/cert-hash"],
    };
  }

  return {
    mode: "village",
    subpath: "/village",
    wtUrl: `https://${host}:4433`,
    wsPathUrl: `${wsProto}//${originHost}/village/ws`,
    wsPortUrl: `${wsProto}//${host}:8443`,
    wsPort: 8443,
    adminPort: 9091,
    certHashPaths: ["/village/cert-hash", "/cert-hash"],
  };
}

/**
 * Shortest-arc angular interpolation in radians wrapped to [0, 2*PI).
 */
export function lerpAngle(a0, a1, t) {
  const twoPi = Math.PI * 2.0;
  const mod = (v) => ((v % twoPi) + twoPi) % twoPi;
  const diff = mod(a1 - a0);
  const shortestDiff = diff > Math.PI ? diff - twoPi : diff;
  return mod(a0 + shortestDiff * t);
}

/**
 * JS/WASM World Adapter providing unified interface for packet ingestion,
 * interpolation, and flat render buffer generation.
 */
export class ClientWorldBridge {
  constructor() {
    this.entities = new Map();
    this.worldObjects = new Map();
    this.interpHistory = new Map();
    this.renderDelaySec = 0.10;
    this.snapbackThreshold = 20.0;
    this.maxExtrapolationSec = 0.20;
    this.clampTimeoutSec = 0.25;
    this.maxInterpHistory = 32;
    this.latestServerTick = 0;
    this.latestBaselineTick = 0;
    this.receivedPackets = 0;
    this.localPlayerGuid = 1;
    this.sessionId = 0;
    this.clientTime = 0.0;
    this.onInteractionResultCallback = null;
    this.onAuthResponseCallback = null;
    this.onMovementCorrectionCallback = null;
    this.onLocalPlayerKeyframeCallback = null;
    this.onServerTickCallback = null;
  }

  onLocalPlayerKeyframe(callback) {
    this.onLocalPlayerKeyframeCallback = callback;
  }

  onServerTick(callback) {
    this.onServerTickCallback = callback;
  }

  clear() {
    this.entities.clear();
    this.worldObjects.clear();
    this.interpHistory.clear();
    this.latestServerTick = 0;
    this.latestBaselineTick = 0;
    this.receivedPackets = 0;
  }

  onMovementCorrection(callback) {
    this.onMovementCorrectionCallback = callback;
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

    // Opcode 0x0002: MovementCorrection
    if (opcode === OPCODES.MOVEMENT_CORRECTION && view.byteLength >= 45) {
      const rawEntityId = view.getBigUint64(8, true);
      const entityId = rawEntityId <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(rawEntityId) : rawEntityId;
      const x = view.getFloat32(16, true);
      const y = view.getFloat32(20, true);
      const z = view.getFloat32(24, true);
      const vx = view.getFloat32(28, true);
      const vy = view.getFloat32(32, true);
      const vz = view.getFloat32(36, true);
      const sequence = view.getUint32(40, true);
      const reason = view.getUint8(44);

      if (this.onMovementCorrectionCallback) {
        this.onMovementCorrectionCallback({
          entityId,
          rawEntityId,
          x,
          y,
          z,
          vx,
          vy,
          vz,
          sequence,
          reason,
        });
      }
      return;
    }

    // Opcode 0x0012: AuthResponse
    if (opcode === OPCODES.AUTH_RESPONSE && view.byteLength >= 33) {
      const success = view.getUint8(8) === 1;
      const errorCode = view.getUint32(9, true);
      const rawSessionId = view.getBigUint64(13, true);
      const rawPlayerGuid = view.getBigUint64(21, true);
      const sessionId = rawSessionId <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(rawSessionId) : rawSessionId;
      const playerGuid = rawPlayerGuid <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(rawPlayerGuid) : rawPlayerGuid;
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
      let payloadBytes = new Uint8Array(0);
      let payloadJson = null;
      let payloadText = "";
      if (view.byteLength >= 21 + payloadLen && payloadLen > 0) {
        payloadBytes = new Uint8Array(data.buffer, data.byteOffset + 21, payloadLen);
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
          rawBytes: payloadBytes,
        });
      }
      return;
    }

    // Opcode 0x0005: EntityState
    if (opcode === OPCODES.ENTITY_STATE && view.byteLength >= 68) {
      const rawEntityId = view.getBigUint64(8, true);
      const entityId = rawEntityId <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(rawEntityId) : rawEntityId;
      const x = view.getFloat32(16, true);
      const y = view.getFloat32(20, true);
      const z = view.getFloat32(24, true);
      const vx = view.getFloat32(28, true);
      const vy = view.getFloat32(32, true);
      const vz = view.getFloat32(36, true);
      const yaw = view.getFloat32(40, true);
      const pitch = view.getFloat32(44, true);
      const health = view.getUint32(48, true);
      const maxHealth = view.getUint32(52, true);
      const stateFlags = view.getUint32(56, true);

      const entityType = Number((rawEntityId >> 48n) & 0xffffn);
      const existing = this.entities.get(entityId);

      if (!existing) {
        this.entities.set(entityId, {
          id: entityId,
          x,
          y,
          z,
          serverX: x,
          serverY: y,
          serverZ: z,
          vx,
          vy,
          vz,
          yaw,
          serverYaw: yaw,
          pitch,
          health,
          maxHealth,
          hpPct: maxHealth > 0 ? health / maxHealth : 1.0,
          castPct: 0.0,
          stateFlags,
          type: entityType,
        });
      } else {
        existing.serverX = x;
        existing.serverY = y;
        existing.serverZ = z;
        existing.vx = vx;
        existing.vy = vy;
        existing.vz = vz;
        existing.serverYaw = yaw;
        existing.pitch = pitch;
        existing.health = health;
        existing.maxHealth = maxHealth;
        existing.hpPct = maxHealth > 0 ? health / maxHealth : 1.0;
        existing.stateFlags = stateFlags;
        existing.type = entityType;
      }

      if (BigInt(entityId) !== BigInt(this.localPlayerGuid)) {
        this._pushInterpSnapshot(
          entityId,
          this.latestServerTick,
          this.clientTime,
          x,
          y,
          z,
          vx,
          vy,
          vz,
          yaw
        );
      }
      return;
    }

    // Opcode 0x0006: EntityDespawn
    if (opcode === OPCODES.ENTITY_DESPAWN && view.byteLength >= 16) {
      const rawEntityId = view.getBigUint64(8, true);
      const entityId = rawEntityId <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(rawEntityId) : rawEntityId;
      this.entities.delete(entityId);
      this.interpHistory.delete(entityId);
      return;
    }

    // Opcode 0x0009: EntityDeltaBatch
    if (opcode === OPCODES.ENTITY_DELTA_BATCH) {
      try {
        let offset = 8;
        if (offset >= view.byteLength) return;

        // 1. server_tick & baseline_tick (LEB128 varint u64)
        const tickRes = readVarintU64(view, offset);
        offset += tickRes.bytesRead;
        const baseRes = readVarintU64(view, offset);
        offset += baseRes.bytesRead;

        const nextServerTick = Number(tickRes.value);
        const nextBaselineTick = Number(baseRes.value);
        if (
          this.latestServerTick > 0 &&
          nextServerTick > 0 &&
          (nextServerTick + 4 < this.latestServerTick ||
            (nextServerTick < this.latestServerTick && nextBaselineTick === 0))
        ) {
          this.interpHistory.clear();
        }

        this.latestServerTick = nextServerTick;
        this.latestBaselineTick = nextBaselineTick;

        if (this.onServerTickCallback) {
          this.onServerTickCallback({
            sessionId: this.sessionId,
            serverTick: this.latestServerTick,
            baselineTick: this.latestBaselineTick,
          });
        }

        // 2. despawns (LEB128 varint u32 length, then u64 entity_ids)
        if (offset >= view.byteLength) return;
        const despawnLenRes = readVarintU32(view, offset);
        offset += despawnLenRes.bytesRead;
        const despawnCount = despawnLenRes.value;

        if (despawnCount > view.byteLength) return;

        for (let i = 0; i < despawnCount; i++) {
          if (offset >= view.byteLength) break;
          const dRes = readVarintU64(view, offset);
          offset += dRes.bytesRead;
          const rawId = dRes.value;
          const entId = rawId <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(rawId) : rawId;
          this.entities.delete(entId);
          this.interpHistory.delete(entId);
        }

        // 3. deltas (LEB128 varint u32 length, then EntityDeltaState)
        if (offset >= view.byteLength) return;
        const deltaLenRes = readVarintU32(view, offset);
        offset += deltaLenRes.bytesRead;
        const deltaCount = deltaLenRes.value;

        if (deltaCount > view.byteLength) return;

        for (let i = 0; i < deltaCount; i++) {
          if (offset >= view.byteLength) break;
          const idRes = readVarintU64(view, offset);
          offset += idRes.bytesRead;
          const rawEntityId = idRes.value;
          const entityId = rawEntityId <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(rawEntityId) : rawEntityId;

          if (offset + 2 > view.byteLength) break;
          const mask = view.getUint16(offset, true);
          offset += 2;

          let posX = 0.0, posY = 0.0, posZ = 0.0;
          let hasKeyframe = false;
          let hasDeltaX = false, hasDeltaY = false, hasDeltaZ = false;
          let deltaX = 0.0, deltaY = 0.0, deltaZ = 0.0;

          if ((mask & DELTA_FLAGS.MASK_KEYFRAME) !== 0) {
            if (offset + 12 > view.byteLength) break;
            posX = view.getFloat32(offset, true);
            posY = view.getFloat32(offset + 4, true);
            posZ = view.getFloat32(offset + 8, true);
            offset += 12;
            hasKeyframe = true;
          } else {
            if ((mask & DELTA_FLAGS.MASK_DELTA_POS_X) !== 0) {
              if (offset + 2 > view.byteLength) break;
              deltaX = dequantizeDeltaI16(view.getInt16(offset, true));
              offset += 2;
              hasDeltaX = true;
            }
            if ((mask & DELTA_FLAGS.MASK_DELTA_POS_Y) !== 0) {
              if (offset + 2 > view.byteLength) break;
              deltaY = dequantizeDeltaI16(view.getInt16(offset, true));
              offset += 2;
              hasDeltaY = true;
            }
            if ((mask & DELTA_FLAGS.MASK_DELTA_POS_Z) !== 0) {
              if (offset + 2 > view.byteLength) break;
              deltaZ = dequantizeDeltaI16(view.getInt16(offset, true));
              offset += 2;
              hasDeltaZ = true;
            }
          }

          let vx = 0.0, vy = 0.0, vz = 0.0;
          let hasVelocity = false;
          if ((mask & DELTA_FLAGS.MASK_VELOCITY) !== 0) {
            if (offset + 6 > view.byteLength) break;
            vx = dequantizeVelocityI16(view.getInt16(offset, true));
            vy = dequantizeVelocityI16(view.getInt16(offset + 2, true));
            vz = dequantizeVelocityI16(view.getInt16(offset + 4, true));
            offset += 6;
            hasVelocity = true;
          }

          let yaw = null;
          if ((mask & DELTA_FLAGS.MASK_YAW) !== 0) {
            if (offset >= view.byteLength) break;
            yaw = dequantizeYawU8(view.getUint8(offset));
            offset += 1;
          }

          let pitch = null;
          if ((mask & DELTA_FLAGS.MASK_PITCH) !== 0) {
            if (offset >= view.byteLength) break;
            pitch = dequantizePitchI8(view.getInt8(offset));
            offset += 1;
          }

          let health = null;
          if ((mask & DELTA_FLAGS.MASK_HEALTH) !== 0) {
            if (offset >= view.byteLength) break;
            const hRes = readVarintU32(view, offset);
            health = hRes.value;
            offset += hRes.bytesRead;
          }

          let maxHealth = null;
          if ((mask & DELTA_FLAGS.MASK_MAX_HEALTH) !== 0) {
            if (offset >= view.byteLength) break;
            const mhRes = readVarintU32(view, offset);
            maxHealth = mhRes.value;
            offset += mhRes.bytesRead;
          }

          let stateFlags = null;
          if ((mask & DELTA_FLAGS.MASK_STATE_FLAGS) !== 0) {
            if (offset >= view.byteLength) break;
            const sfRes = readVarintU32(view, offset);
            stateFlags = sfRes.value;
            offset += sfRes.bytesRead;
          }

          let actionId = null;
          if ((mask & DELTA_FLAGS.MASK_ACTION_ID) !== 0) {
            if (offset >= view.byteLength) break;
            const actRes = readVarintU32(view, offset);
            actionId = actRes.value;
            offset += actRes.bytesRead;
          }

          if ((mask & DELTA_FLAGS.MASK_ATTRIBUTES) !== 0) {
            if (offset >= view.byteLength) break;
            const attrCountRes = readVarintU32(view, offset);
            offset += attrCountRes.bytesRead;
            const attrCount = attrCountRes.value;
            for (let a = 0; a < attrCount && offset < view.byteLength; a++) {
              const aIdRes = readVarintU32(view, offset);
              offset += aIdRes.bytesRead;
              const aValRes = readVarintU32(view, offset);
              offset += aValRes.bytesRead;
            }
          }

          // Entity replica update
          const entityType = Number((rawEntityId >> 48n) & 0xffffn);
          let ent = this.entities.get(entityId);
          const isNewEntity = !ent;
          if (!ent) {
            ent = {
              id: entityId,
              x: posX,
              y: posY,
              z: posZ,
              serverX: posX,
              serverY: posY,
              serverZ: posZ,
              vx,
              vy,
              vz,
              yaw: yaw ?? 0.0,
              serverYaw: yaw ?? 0.0,
              pitch: pitch ?? 0.0,
              health: health ?? 100,
              maxHealth: maxHealth ?? 100,
              hpPct: (maxHealth && maxHealth > 0) ? (health ?? 100) / maxHealth : 1.0,
              castPct: 0.0,
              stateFlags: stateFlags ?? 0,
              actionId: actionId ?? 0,
              type: entityType,
            };
            this.entities.set(entityId, ent);
          } else {
            if (ent.serverX === undefined) ent.serverX = ent.x;
            if (ent.serverY === undefined) ent.serverY = ent.y;
            if (ent.serverZ === undefined) ent.serverZ = ent.z;
            if (ent.serverYaw === undefined) ent.serverYaw = ent.yaw;

            // Apply authoritative updates to server state (visual state interpolates smoothly in update())
            if (hasKeyframe) {
              ent.serverX = posX;
              ent.serverY = posY;
              ent.serverZ = posZ;
            } else {
              if (hasDeltaX) ent.serverX += deltaX;
              if (hasDeltaY) ent.serverY += deltaY;
              if (hasDeltaZ) ent.serverZ += deltaZ;
            }
            if (hasVelocity) {
              ent.vx = vx;
              ent.vy = vy;
              ent.vz = vz;
            }
            if (yaw !== null) ent.serverYaw = yaw;
            if (pitch !== null) ent.pitch = pitch;
            if (health !== null) ent.health = health;
            if (maxHealth !== null) ent.maxHealth = maxHealth;
            if (ent.maxHealth > 0) {
              ent.hpPct = ent.health / ent.maxHealth;
            }
            if (stateFlags !== null) ent.stateFlags = stateFlags;
            if (actionId !== null) ent.actionId = actionId;
          }

          if (BigInt(entityId) !== BigInt(this.localPlayerGuid)) {
            this._pushInterpSnapshot(
              entityId,
              this.latestServerTick,
              this.clientTime,
              ent.serverX,
              ent.serverY,
              ent.serverZ,
              ent.vx ?? 0.0,
              ent.vy ?? 0.0,
              ent.vz ?? 0.0,
              ent.serverYaw
            );
          }

          if (
            this.onLocalPlayerKeyframeCallback &&
            BigInt(entityId) === BigInt(this.localPlayerGuid) &&
            (isNewEntity || hasKeyframe)
          ) {
            ent.x = ent.serverX;
            ent.y = ent.serverY;
            ent.z = ent.serverZ;
            ent.yaw = ent.serverYaw;
            this.onLocalPlayerKeyframeCallback({
              entityId,
              x: ent.serverX,
              y: ent.serverY,
              z: ent.serverZ,
              yaw: ent.serverYaw,
            });
          }
        }
      } catch (err) {
        console.warn("Failed to parse EntityDeltaBatch:", err);
      }
      return;
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

      let properties = {};
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
              const propRes = readPropertyMap(view, offset);
              properties = propRes.value;
              offset += propRes.bytesRead;
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
        properties,
        revision,
      });
      return;
    }

    // Opcode 0x000C: WorldObjectDelta
    if (opcode === OPCODES.WORLD_OBJECT_DELTA && view.byteLength >= 12) {
      let offset = 8;
      const { value: objIdBig, bytesRead: b1 } = readVarintU64(view, offset);
      offset += b1;
      if (offset >= view.byteLength) return;
      const { value: revBig, bytesRead: b2 } = readVarintU64(view, offset);
      offset += b2;
      const revision = Number(revBig);

      if (offset >= view.byteLength) return;
      const hasFlags = view.getUint8(offset);
      offset += 1;
      let stateFlags = null;
      if (hasFlags !== 0) {
        if (offset >= view.byteLength) return;
        const sfRes = readVarintU32(view, offset);
        stateFlags = sfRes.value;
        offset += sfRes.bytesRead;
      }

      if (offset >= view.byteLength) return;
      const hasHealth = view.getUint8(offset);
      offset += 1;
      let hpPct = null;
      if (hasHealth !== 0) {
        if (offset >= view.byteLength) return;
        const curHp = readVarintU32(view, offset);
        offset += curHp.bytesRead;
        if (offset >= view.byteLength) return;
        const maxHp = readVarintU32(view, offset);
        offset += maxHp.bytesRead;
        hpPct = maxHp.value > 0 ? Math.max(0.0, Math.min(1.0, curHp.value / maxHp.value)) : 1.0;
      }

      let properties = {};
      if (offset < view.byteLength) {
        const propRes = readPropertyMap(view, offset);
        properties = propRes.value;
      }

      const existing = this.worldObjects.get(objIdBig) ?? this.worldObjects.get(Number(objIdBig));
      if (existing && revision > existing.revision) {
        existing.revision = revision;
        if (stateFlags !== null) existing.stateFlags = stateFlags;
        if (hpPct !== null) existing.hpPct = hpPct;
        if (Object.keys(properties).length > 0) {
          existing.properties = { ...(existing.properties || {}), ...properties };
        }
      }
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

  _pushInterpSnapshot(entityId, tick, timestampSec, x, y, z, vx, vy, vz, yaw) {
    let queue = this.interpHistory.get(entityId);
    if (!queue) {
      queue = [];
      this.interpHistory.set(entityId, queue);
    }

    let effectiveTs = timestampSec;
    if (queue.length > 0) {
      const back = queue[queue.length - 1];
      if (tick > 0 && back.tick > 0) {
        if (tick < back.tick) {
          if (back.tick - tick > 4) {
            const prevBackTs = back.timestampSec;
            queue.length = 0;
            effectiveTs = Math.max(timestampSec, prevBackTs + 0.05);
          } else {
            const frontTick = queue[0].tick;
            if (tick <= frontTick || queue.some((s) => s.tick === tick)) {
              return;
            }
            const insertIdx = queue.findIndex((s) => s.tick > tick);
            const idx = insertIdx === -1 ? queue.length : insertIdx;
            const prevS = queue[idx - 1];
            const nextS = queue[idx];
            const frac = (tick - prevS.tick) / (nextS.tick - prevS.tick);
            const ts = prevS.timestampSec + frac * (nextS.timestampSec - prevS.timestampSec);
            queue.splice(idx, 0, { tick, timestampSec: ts, x, y, z, vx, vy, vz, yaw });
            while (queue.length > this.maxInterpHistory) {
              queue.shift();
            }
            return;
          }
        } else if (tick === back.tick) {
          return;
        } else {
          const tickDelta = tick - back.tick;
          const minTs = back.timestampSec + tickDelta * 0.05;
          effectiveTs = Math.max(timestampSec, minTs);
        }
      } else if (tick > 0 && back.tick === 0) {
        const minTs = back.timestampSec + 0.05;
        effectiveTs = Math.max(timestampSec, minTs);
      } else {
        if (timestampSec <= back.timestampSec) {
          return;
        }
      }
    }

    queue.push({ tick, timestampSec: effectiveTs, x, y, z, vx, vy, vz, yaw });
    while (queue.length > this.maxInterpHistory) {
      queue.shift();
    }
  }

  _sampleInterp(entityId, currentTimeSec) {
    const queue = this.interpHistory.get(entityId);
    if (!queue || queue.length === 0) return null;

    const renderTime = currentTimeSec - this.renderDelaySec;
    if (queue.length === 1) {
      const s = queue[0];
      return { x: s.x, y: s.y, z: s.z, yaw: s.yaw };
    }

    const oldest = queue[0];
    const newest = queue[queue.length - 1];

    if (renderTime <= oldest.timestampSec) {
      return { x: oldest.x, y: oldest.y, z: oldest.z, yaw: oldest.yaw };
    }

    if (renderTime >= newest.timestampSec) {
      const delta = renderTime - newest.timestampSec;
      if (delta > this.clampTimeoutSec) {
        return { x: newest.x, y: newest.y, z: newest.z, yaw: newest.yaw };
      }
      const extrapDt = Math.min(delta, this.maxExtrapolationSec);
      return {
        x: newest.x + newest.vx * extrapDt,
        y: newest.y + newest.vy * extrapDt,
        z: newest.z + newest.vz * extrapDt,
        yaw: newest.yaw,
      };
    }

    for (let i = 0; i < queue.length - 1; i++) {
      const s0 = queue[i];
      const s1 = queue[i + 1];
      if (s0.timestampSec <= renderTime && renderTime <= s1.timestampSec) {
        const span = s1.timestampSec - s0.timestampSec;
        if (span <= 1e-6) {
          return { x: s1.x, y: s1.y, z: s1.z, yaw: s1.yaw };
        }
        const dx = s1.x - s0.x;
        const dy = s1.y - s0.y;
        const dz = s1.z - s0.z;
        const distSq = dx * dx + dy * dy + dz * dz;
        if (distSq > this.snapbackThreshold * this.snapbackThreshold) {
          return { x: s1.x, y: s1.y, z: s1.z, yaw: s1.yaw };
        }
        const alpha = Math.max(0.0, Math.min(1.0, (renderTime - s0.timestampSec) / span));
        return {
          x: s0.x + dx * alpha,
          y: s0.y + dy * alpha,
          z: s0.z + dz * alpha,
          yaw: lerpAngle(s0.yaw, s1.yaw, alpha),
        };
      }
    }

    return { x: newest.x, y: newest.y, z: newest.z, yaw: newest.yaw };
  }

  update(dt) {
    this.clientTime += dt;
    const localGuidBig = BigInt(this.localPlayerGuid);
    for (const [entityId, ent] of this.entities.entries()) {
      if (BigInt(entityId) === localGuidBig) continue;
      const sampled = this._sampleInterp(entityId, this.clientTime);
      if (sampled) {
        ent.x = sampled.x;
        ent.y = sampled.y;
        ent.z = sampled.z;
        ent.yaw = sampled.yaw;
      }
    }
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
    const hasLocal = this.entities.has(this.localPlayerGuid);
    t.entity_count = this.entities.size + (hasLocal ? 0 : 1);
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
 * Returns or generates a unique session token stored in sessionStorage
 * so concurrent browser tabs authenticate as distinct characters.
 */
export function getOrCreateSessionToken(gameMode = "village") {
  if (typeof window === "undefined" || !window.sessionStorage) {
    return gameMode === "asteroid" ? "guest-poc-asteroid" : "guest-poc";
  }
  const params = new URLSearchParams(window.location.search);
  const tokenParam = params.get("token");
  let baseToken = tokenParam;
  if (tokenParam) {
    window.sessionStorage.setItem("mmo_token", tokenParam);
  } else {
    baseToken = window.sessionStorage.getItem("mmo_token");
    if (!baseToken) {
      const rand = Math.random().toString(36).substring(2, 8);
      const ts = Date.now().toString(36);
      baseToken = `guest-${ts}-${rand}`;
      window.sessionStorage.setItem("mmo_token", baseToken);
    }
  }
  return gameMode === "asteroid" ? `${baseToken}-asteroid` : baseToken;
}

/**
 * Automatically retrieves the ephemeral SHA-256 certificate digest
 * exposed by TelemetryServer GET /cert-hash for the active game mode.
 */
export async function autoFetchCertHash(gameMode = "village", force = false) {
  const certInput = document.getElementById("cert-hash");
  if (!force && certInput && certInput.value.trim().length > 0) return;
  if (force && certInput) {
    certInput.value = "";
  }
  try {
    const gwConfig = resolveGatewayConfig(gameMode);
    const proto = window.location.protocol === "https:" ? "https:" : "http:";
    const host = window.location.hostname || "127.0.0.1";
    const candidateUrls = [
      ...gwConfig.certHashPaths,
      `${proto}//${host}:${gwConfig.adminPort}/cert-hash`,
      `http://${host}:${gwConfig.adminPort}/cert-hash`,
    ];

    for (const candidateUrl of candidateUrls) {
      try {
        const res = await fetch(candidateUrl);
        if (!res || !res.ok) continue;
        const data = await res.json();
        if (data && typeof data.cert_hash === "string" && data.cert_hash.length > 0) {
          if (certInput) certInput.value = data.cert_hash;
          console.log(`[WebTransport] Auto-discovered certificate hash (${gameMode}):`, data.cert_hash);
          return;
        }
      } catch {
        // Try next candidate endpoint
      }
    }
  } catch (err) {
    console.debug("[WebTransport] Cert discovery fallback:", err);
  }
}

/**
 * Formats a transport or handshake failure into a concise HUD-safe error string.
 */
export function formatConnectionError(err) {
  if (!err) return "Connection failed.";
  const raw = typeof err === "string" ? err : err?.message || String(err);
  const trimmed = raw.trim();
  if (!trimmed) return "Connection failed.";
  return trimmed.length > 180 ? `${trimmed.slice(0, 177)}...` : trimmed;
}

// Application Bootstrap
if (typeof window !== "undefined") {
  window.addEventListener("DOMContentLoaded", async () => {
    const container = document.getElementById("canvas-container");
    const transport = new WebTransportClient();
    const ui = new UIController();
    const world = new ClientWorldBridge();

    const runtimeEnvMode =
      (typeof import.meta !== "undefined" && import.meta.env && import.meta.env.VITE_GAME_MODE) ||
      window.__GAME_MODE__ ||
      window.__MMO_GAME_MODE__ ||
      "";
    let activeGameMode = resolveGameMode(
      window.location.search,
      runtimeEnvMode,
      window.location.pathname
    );

    const controlsByMode = { village: null, asteroid: null };
    let voxelClient = null;
    let activeRenderer = null;
    let activeControls = null;

    // Dynamic gateway host resolution
    const initialGwConfig = resolveGatewayConfig(activeGameMode);
    const urlInput = document.getElementById("gateway-url");
    if (
      urlInput &&
      (!urlInput.value ||
        urlInput.value.includes("127.0.0.1") ||
        urlInput.value.endsWith(":4433") ||
        urlInput.value.endsWith(":4434"))
    ) {
      urlInput.value = initialGwConfig.wtUrl;
    }

    // Try to load cert hash from URL parameters if provided (e.g. ?cert=aa:bb:cc...)
    const params = new URLSearchParams(window.location.search);
    const certFromUrl = params.get("cert");
    if (certFromUrl) {
      const certInput = document.getElementById("cert-hash");
      if (certInput) certInput.value = certFromUrl;
    } else {
      await autoFetchCertHash(activeGameMode);
    }

    let nearestTarget = null;
    let nearestAsteroidTarget = null;
    let activeTradeSessionId = 0;
    let latestAsteroidState = null;
    let lastChunkProbeKey = "";
    let hasSyncedInitialPosition = false;
    const raycaster = new THREE.Raycaster();
    const pointerNdc = new THREE.Vector2();

    function toSafeGuid(val) {
      if (val == null) return null;
      return Number(val);
    }

    function sendAsteroidInteraction(
      subOpcode,
      targetGuid = 0,
      body = {},
      interactionType = INTERACTION_TYPES.CUSTOM
    ) {
      if (!transport.connected) return;
      const actionId = ((ASTEROID_MODULE_ID << 16) | (subOpcode & 0xffff)) >>> 0;
      const payload = new TextEncoder().encode(JSON.stringify(body));
      const pkt = encodeInteraction(
        world.localPlayerGuid,
        targetGuid ?? 0,
        actionId,
        payload,
        interactionType
      );
      transport.sendStream(pkt);
    }

    function sendVillageInteraction(
      subOpcode,
      targetGuid = 0,
      body = {},
      interactionType = INTERACTION_TYPES.TRADE
    ) {
      if (!transport.connected) return;
      const actionId = ((VILLAGE_MODULE_ID << 16) | (subOpcode & 0xffff)) >>> 0;
      const payload = new TextEncoder().encode(JSON.stringify(body));
      const pkt = encodeInteraction(
        world.localPlayerGuid,
        targetGuid ?? 0,
        actionId,
        payload,
        interactionType
      );
      transport.sendStream(pkt);
    }

    function performInspect(target) {
      if (!transport.connected || !target) return;
      if (activeGameMode === "asteroid") {
        sendAsteroidInteraction(1, target.id, {
          target_guid: target.id ? toSafeGuid(target.id) : null,
        });
      } else {
        sendVillageInteraction(1, target.id, { target_guid: toSafeGuid(target.id) });
      }
    }

    function requestVisibleAsteroidChunks(force = false) {
      if (activeGameMode !== "asteroid" || !transport.connected || !voxelClient || !activeControls) {
        return;
      }
      const { chunk } = voxelCoordToChunk(
        Math.floor(activeControls.position.x),
        Math.floor(activeControls.position.y),
        Math.floor(activeControls.position.z)
      );
      const key = chunkKey(chunk.x, chunk.y, chunk.z);
      if (!force && key === lastChunkProbeKey) return;
      lastChunkProbeKey = key;

      const candidates = [
        ...voxelClient.collectUnrequestedChunks(activeControls.position, 2, 1, 2),
        ...voxelClient.collectUnrequestedChunks({ x: 0, y: 0, z: 0 }, 2, 1, 2),
      ];
      for (const c of candidates) {
        const pkt = voxelClient.encodeChunkRequestPacket(world.localPlayerGuid, c.x, c.y, c.z);
        transport.sendStream(pkt);
      }
    }

    function fireMiningLaser(explicitTarget = null) {
      if (activeGameMode !== "asteroid" || !transport.connected || !voxelClient || !activeControls) {
        return;
      }
      const target =
        explicitTarget ||
        voxelClient.raycastFirstSolid(
          activeControls.position,
          activeControls.getForwardVector(),
          12.0
        ) ||
        voxelClient.findNearestSolidVoxel(activeControls.position, 6.5, true);

      if (!target) {
        ui.showToast("Fly near an asteroid block (<=6.5m) or click a block to mine.", "info", 2000);
        return;
      }

      const targetCenter = target.center || {
        x: target.x + 0.5,
        y: target.y + 0.5,
        z: target.z + 0.5,
      };
      activeRenderer?.spawnLaserBeam?.(activeControls.position, targetCenter, 0x38bdf8, 220);
      const pkt = voxelClient.encodeModifyRequestPacket(
        world.localPlayerGuid,
        target.x,
        target.y,
        target.z,
        VOXEL_EMPTY
      );
      transport.sendStream(pkt);
    }

    function firePvpLaser(explicitTarget = null) {
      if (activeGameMode !== "asteroid" || !transport.connected || !activeControls) return;
      const target = explicitTarget || nearestAsteroidTarget;
      if (!target) {
        ui.showToast("No rival ship within 45m laser range.", "warning", 2000);
        return;
      }
      activeRenderer?.spawnLaserBeam?.(activeControls.position, target, 0xf43f5e, 240);
      sendAsteroidInteraction(
        2,
        target.id,
        { target_guid: toSafeGuid(target.id) },
        INTERACTION_TYPES.ATTACK
      );
    }

    function bindAsteroidCanvasPicking(rendererInstance) {
      const canvas = rendererInstance?.renderer?.domElement;
      if (!canvas) return;
      let downPos = null;

      canvas.addEventListener("pointerdown", (e) => {
        if (e.button !== 0) return;
        if (
          e.target &&
          e.target.closest &&
          e.target.closest("#hud-overlay, #asteroid-hud, .modal-overlay, #game-mode-switcher")
        ) {
          return;
        }
        downPos = { x: e.clientX, y: e.clientY };
      });

      canvas.addEventListener("pointerup", (e) => {
        if (activeGameMode !== "asteroid" || !downPos || e.button !== 0) return;
        const moved = Math.hypot(e.clientX - downPos.x, e.clientY - downPos.y);
        downPos = null;
        if (moved >= 6 || !rendererInstance.camera) return;

        const rect = canvas.getBoundingClientRect();
        if (!rect.width || !rect.height) return;
        pointerNdc.x = ((e.clientX - rect.left) / rect.width) * 2 - 1;
        pointerNdc.y = -((e.clientY - rect.top) / rect.height) * 2 + 1;
        raycaster.setFromCamera(pointerNdc, rendererInstance.camera);

        // 1. Check rival ships for PvP laser targeting
        const shipHits = raycaster.intersectObjects(
          rendererInstance.getShipMeshes(world.localPlayerGuid),
          true
        );
        if (shipHits.length > 0) {
          let obj = shipHits[0].object;
          while (obj && obj.userData?.guid == null) {
            obj = obj.parent;
          }
          if (obj && obj.userData?.guid != null) {
            const targetGuid = obj.userData.guid;
            const entKey =
              targetGuid <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(targetGuid) : targetGuid;
            const ent = world.entities.get(entKey) ?? world.entities.get(targetGuid);
            if (ent && activeControls) {
              const dist = Math.hypot(
                ent.x - activeControls.position.x,
                ent.y - activeControls.position.y,
                ent.z - activeControls.position.z
              );
              if (dist <= PVP_LASER_RANGE) {
                firePvpLaser({ ...ent, distance: dist });
                return;
              }
            }
          }
        }

        // 2. Check asteroid voxel chunks for laser mining
        const voxelHits = raycaster.intersectObjects(rendererInstance.getChunkMeshObjects(), false);
        if (voxelHits.length > 0 && voxelHits[0].face) {
          const hit = voxelHits[0];
          const n = hit.face.normal;
          const vx = Math.floor(hit.point.x - n.x * 0.1);
          const vy = Math.floor(hit.point.y - n.y * 0.1);
          const vz = Math.floor(hit.point.z - n.z * 0.1);
          fireMiningLaser({
            x: vx,
            y: vy,
            z: vz,
            center: { x: vx + 0.5, y: vy + 0.5, z: vz + 0.5 },
          });
        }
      });
    }

    function mountGameMode(nextMode, updateHistory = true) {
      const prevMode = activeGameMode;
      const targetMode = nextMode === "asteroid" ? "asteroid" : "village";

      if (activeRenderer) {
        activeRenderer.clear?.();
        if (typeof activeRenderer.dispose === "function") {
          activeRenderer.dispose();
        } else {
          activeRenderer.renderer?.dispose?.();
          activeRenderer.renderer?.domElement?.remove?.();
        }
        activeRenderer = null;
      }

      if (container && typeof container.replaceChildren === "function") {
        container.replaceChildren();
      }

      activeGameMode = targetMode;
      if (activeGameMode === "asteroid") {
        activeRenderer = new AsteroidRenderer(container);
        if (!controlsByMode.asteroid) {
          const flightControls = new SpaceFlightControls();
          flightControls.onMine(() => {
            if (activeGameMode === "asteroid") fireMiningLaser();
          });
          flightControls.onFireLaser(() => {
            if (activeGameMode === "asteroid") firePvpLaser();
          });
          controlsByMode.asteroid = flightControls;
        }
        activeControls = controlsByMode.asteroid;
        voxelClient ??= new AsteroidVoxelClient();
        bindAsteroidCanvasPicking(activeRenderer);
      } else {
        activeRenderer = new WorldRenderer(container);
        if (!controlsByMode.village) {
          const groundControls = new PlayerControls();
          groundControls.onCast(() => {
            if (activeGameMode !== "village" || !transport.connected) return;
            const pkt = encodeInteraction(
              world.localPlayerGuid,
              nearestTarget ? nearestTarget.id : 0,
              42,
              new Uint8Array(0),
              INTERACTION_TYPES.ATTACK
            );
            transport.sendStream(pkt);
          });
          controlsByMode.village = groundControls;
        }
        activeControls = controlsByMode.village;
      }

      if (ui.togglePartitions) {
        activeRenderer.setPartitionsVisible(ui.togglePartitions.checked);
      }
      if (ui.toggleAoi) {
        activeRenderer.setAoiVisible(ui.toggleAoi.checked);
      }
      if (ui.toggleWireframe) {
        activeRenderer.setWireframe(ui.toggleWireframe.checked);
      }

      const gwConfig = resolveGatewayConfig(activeGameMode);
      if (
        urlInput &&
        (!urlInput.value ||
          urlInput.value.includes("127.0.0.1") ||
          urlInput.value.endsWith(":4433") ||
          urlInput.value.endsWith(":4434"))
      ) {
        urlInput.value = gwConfig.wtUrl;
      }

      ui.setGameMode(activeGameMode);
      if (updateHistory && typeof window.history?.replaceState === "function") {
        try {
          const nextUrl = new URL(window.location.href);
          nextUrl.pathname = gwConfig.subpath;
          if (nextUrl.searchParams.has("game")) {
            nextUrl.searchParams.delete("game");
          }
          window.history.replaceState(null, "", nextUrl.toString());
        } catch (_) {}
      }

      if (transport.connected && prevMode === activeGameMode) {
        performInspect({ id: 0 });
        if (activeGameMode === "asteroid") {
          lastChunkProbeKey = "";
          requestVisibleAsteroidChunks(true);
        }
      }
    }

    // Mount initial cartridge
    mountGameMode(activeGameMode, false);

    // Auto-connect (WebTransport with automatic WebSocket fallback)
    setTimeout(() => {
      ui.btnConnect?.click();
    }, 150);

    // Interaction response listener for Village, Voxel (0x00A0), and Asteroid (0x0003) HUD updates
    world.onInteractionResult((res) => {
      const moduleId = (res.actionId >>> 16) & 0xffff;

      // 1. Voxel Subsystem (0x00A0) binary chunk & modify responses
      if (moduleId === VOXEL_MODULE_ID) {
        if (!voxelClient || !res.rawBytes || res.rawBytes.byteLength === 0) return;
        const handled = voxelClient.handleInteractionResult(res.actionId, res.rawBytes);
        if (handled?.type === "modify_response") {
          if (handled.status === 0) {
            performInspect({ id: 0 });
          } else if (handled.status === 1) {
            ui.showToast("Target asteroid block is out of mining reach!", "warning", 2000);
          } else if (handled.status === 2) {
            ui.showToast("Mining laser recharging...", "info", 1200);
          }
        }
        return;
      }

      if (!res.data) return;

      function syncTradeModal(payload, completionToast) {
        const tradeSession = payload.active_trade || payload.trade_session;
        if (tradeSession) {
          activeTradeSessionId = tradeSession.session_id || activeTradeSessionId;
          ui.showPlayerTrade(tradeSession);
        }
        if (payload.trade_completed || (tradeSession && tradeSession.completed)) {
          ui.hidePlayerTrade();
          if (completionToast) {
            ui.showToast(completionToast, "success", 2800);
          }
        }
      }

      // 2. Asteroid Mining Plugin (0x0003) JSON state responses
      if (moduleId === ASTEROID_MODULE_ID) {
        const prevDeaths = latestAsteroidState?.station_vault?.deaths;
        const nextDeaths = res.data.station_vault?.deaths;
        if (
          prevDeaths !== undefined &&
          nextDeaths !== undefined &&
          nextDeaths > prevDeaths &&
          activeGameMode === "asteroid" &&
          activeControls
        ) {
          activeControls.setPosition(0.0, 0.0, 8.0);
          if (activeControls.velocity) {
            activeControls.velocity = { x: 0.0, y: 0.0, z: 0.0 };
          }
          ui.showToast("Ship destroyed! Respawned at Home Orbital Station.", "error", 3200);
        }

        latestAsteroidState = res.data;
        ui.updateAsteroidState(res.data);

        const localEnt = world.entities.get(world.localPlayerGuid);
        if (localEnt) {
          if (res.data.shininess_tier !== undefined) {
            const tier = Number(res.data.shininess_tier) & 0xff;
            localEnt.stateFlags = ((localEnt.stateFlags || 0) & 0xff00ffff) | (tier << 16);
          }
          if (res.data.hull_hp !== undefined && res.data.max_hull_hp > 0) {
            localEnt.hpPct = res.data.hull_hp / res.data.max_hull_hp;
          }
        }

        if (res.data.error) {
          ui.showToast(res.data.error, "warning", 2800);
        }

        syncTradeModal(res.data, "Station pilot trade completed!");
        return;
      }

      // 3. Village RPG (0x0002) state responses
      const inv = res.data.inventory || res.data.player_inventory;
      const currentWeight = res.data.current_weight ?? inv?.current_weight;
      if (inv) {
        ui.updateInventory(inv, currentWeight);
      }

      const merchant = res.data.target_merchant || res.data.npc_merchant;
      if (merchant && nearestTarget) {
        ui.showNpcBarter(merchant, (recipeId) => {
          sendVillageInteraction(2, nearestTarget.id, {
            target_guid: toSafeGuid(nearestTarget.id),
            recipe_id: recipeId,
          });
        });
      }

      syncTradeModal(res.data, "");
    });

    // Network Packet Ingestion
    transport.onPacket((data) => {
      world.processPacket(data);
    });

    // Authoritative server movement & initial auth hydration listeners
    world.onAuthResponse(() => {
      hasSyncedInitialPosition = false;
      lastChunkProbeKey = "";
      performInspect({ id: 0 });
      if (activeGameMode === "asteroid") {
        requestVisibleAsteroidChunks(true);
      }
    });

    world.onLocalPlayerKeyframe((state) => {
      if (!activeControls) return;
      const snapError = Math.hypot(
        activeControls.position.x - state.x,
        activeControls.position.y - state.y,
        activeControls.position.z - state.z
      );
      if (!hasSyncedInitialPosition || snapError > 10.0) {
        activeControls.setPosition(state.x, state.y, state.z);
        hasSyncedInitialPosition = true;
      }
    });

    world.onMovementCorrection((corr) => {
      if (!activeControls) return;
      if (BigInt(corr.entityId) === BigInt(world.localPlayerGuid)) {
        activeControls.reconcile(corr.sequence, corr.x, corr.y, corr.z, corr.vx, corr.vy, corr.vz);
        hasSyncedInitialPosition = true;
      }
    });

    // Baseline Ack Hook: Send ClientAck upon observing new server tick
    world.onServerTick(({ sessionId, serverTick }) => {
      if (transport.connected && sessionId) {
        const ackPkt = encodeClientAck(sessionId, serverTick, 0);
        transport.sendDatagram(ackPkt);
      }
    });

    transport.onStatus(({ status, error, transportMode }) => {
      const statusText =
        status === "connected"
          ? "Connected"
          : status === "connecting"
          ? "Connecting"
          : "Disconnected";
      const errorMsg =
        status === "disconnected" && error ? formatConnectionError(error) : "";
      ui.setConnectionState(status, statusText, errorMsg, transportMode || "");
      if (status === "disconnected") {
        hasSyncedInitialPosition = false;
        lastChunkProbeKey = "";
        world.clear();
        voxelClient?.clear();
        activeRenderer?.clear();
      }
      if (error) {
        console.warn("Transport status notice:", error);
      }
    });

    // UI Event Bindings
    ui.initEvents({
      onGameModeChange: async (nextMode) => {
        if (nextMode !== activeGameMode) {
          mountGameMode(nextMode, true);
          ui.showToast(
            nextMode === "asteroid"
              ? "Switched to Game B: 6DoF Voxel Asteroid Miners"
              : "Switched to Game A: Village RPG",
            "info",
            2200
          );
          if (transport.connected || transport._isConnecting) {
            await transport.disconnect();
            world.clear();
            voxelClient?.clear();
            activeRenderer?.clear();
          }
          nearestTarget = null;
          nearestAsteroidTarget = null;
          latestAsteroidState = null;
          hasSyncedInitialPosition = false;
          lastChunkProbeKey = "";
          await autoFetchCertHash(activeGameMode, true);
          ui.btnConnect?.click();
        }
      },
      onConnect: async (url, certHash) => {
        try {
          ui.setConnectionState("connecting", "Connecting", "");
          const gwConfig = resolveGatewayConfig(activeGameMode);
          await transport.connect(url, certHash, {
            wsUrls: [gwConfig.wsPathUrl, gwConfig.wsPortUrl],
            wsPort: gwConfig.wsPort,
          });
          const sessionToken = getOrCreateSessionToken(activeGameMode);
          const authPkt = encodeAuthRequest(sessionToken, 1);
          await transport.sendStream(authPkt);

          setTimeout(() => {
            performInspect({ id: 0 });
            if (activeGameMode === "asteroid") {
              requestVisibleAsteroidChunks(true);
            }
          }, 200);
        } catch (err) {
          const errorMsg = formatConnectionError(err);
          console.warn("Failed to connect via WebTransport:", errorMsg);
          ui.setConnectionState("disconnected", "Disconnected", errorMsg);
        }
      },
      onDisconnect: async () => {
        await transport.disconnect();
        world.clear();
        voxelClient?.clear();
        activeRenderer?.clear();
      },
      onTogglePartitions: (visible) => activeRenderer?.setPartitionsVisible(visible),
      onToggleAoi: (visible) => activeRenderer?.setAoiVisible(visible),
      onToggleWireframe: (wireframe) => activeRenderer?.setWireframe(wireframe),
      onInteract: () => {
        if (activeGameMode === "village" && nearestTarget) {
          performInspect(nearestTarget);
        }
      },
      onMineAsteroid: () => fireMiningLaser(),
      onFireLaser: () => firePvpLaser(),
      onBankLoot: () => {
        if (activeGameMode !== "asteroid" || !transport.connected) return;
        sendAsteroidInteraction(3, 0, {});
      },
      onTradePilot: () => {
        if (activeGameMode !== "asteroid" || !transport.connected || !nearestAsteroidTarget) return;
        sendAsteroidInteraction(4, nearestAsteroidTarget.id, {
          target_guid: toSafeGuid(nearestAsteroidTarget.id),
        });
      },
      onTradeOffer: (gold, items) => {
        if (!transport.connected) return;
        if (activeGameMode === "asteroid") {
          sendAsteroidInteraction(5, nearestAsteroidTarget ? nearestAsteroidTarget.id : 0, {
            session_id: activeTradeSessionId,
            currency: gold,
            items,
          });
        } else {
          sendVillageInteraction(4, nearestTarget ? nearestTarget.id : 0, {
            session_id: activeTradeSessionId,
            currency: gold,
            items,
          });
        }
      },
      onTradeAccept: () => {
        if (!transport.connected) return;
        if (activeGameMode === "asteroid") {
          sendAsteroidInteraction(6, nearestAsteroidTarget ? nearestAsteroidTarget.id : 0, {
            session_id: activeTradeSessionId,
          });
        } else {
          sendVillageInteraction(5, nearestTarget ? nearestTarget.id : 0, {
            session_id: activeTradeSessionId,
          });
        }
      },
      onTradeCancel: () => {
        if (!transport.connected) return;
        if (activeGameMode === "asteroid") {
          sendAsteroidInteraction(7, nearestAsteroidTarget ? nearestAsteroidTarget.id : 0, {
            session_id: activeTradeSessionId,
          });
        } else {
          sendVillageInteraction(6, nearestTarget ? nearestTarget.id : 0, {
            session_id: activeTradeSessionId,
          });
        }
      },
    });

    // Keyboard shortcut: E to interact with nearest target in Village mode
    window.addEventListener("keydown", (e) => {
      if (activeGameMode !== "village") return;
      if ((e.key === "e" || e.key === "E") && !e.repeat) {
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

      if (activeControls) {
        activeControls.update(dt);
      }
      world.update(dt);

      const playerPos = activeControls ? activeControls.position : { x: 0, y: 0, z: 0 };

      if (activeGameMode === "village") {
        let closest = null;
        let minDistance = Infinity;

        for (const ent of world.entities.values()) {
          if (BigInt(ent.id) === BigInt(world.localPlayerGuid)) continue;
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
              BigInt(nearestTarget.id) % 3n === 1n
                ? "Blacksmith Ilmari"
                : BigInt(nearestTarget.id) % 3n === 2n
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
      } else {
        const distToStation = Math.hypot(playerPos.x, playerPos.y, playerPos.z);
        const inSafeZone = distToStation <= HOME_STATION_SAFE_RADIUS;
        ui.setSafeZoneStatus(inSafeZone);

        let closestShip = null;
        let minShipDist = Infinity;
        for (const ent of world.entities.values()) {
          if (BigInt(ent.id) === BigInt(world.localPlayerGuid)) continue;
          const dx = ent.x - playerPos.x;
          const dy = ent.y - playerPos.y;
          const dz = ent.z - playerPos.z;
          const dist = Math.hypot(dx, dy, dz);
          if (dist < minShipDist && dist <= PVP_LASER_RANGE) {
            minShipDist = dist;
            closestShip = { ...ent, distance: dist };
          }
        }

        nearestAsteroidTarget = closestShip;
        if (nearestAsteroidTarget) {
          ui.showAsteroidTarget(nearestAsteroidTarget, inSafeZone);
        } else {
          ui.hideAsteroidTarget();
        }

        if (transport.connected) {
          requestVisibleAsteroidChunks(false);
        }
      }

      if (now - lastMoveSendTime >= MOVE_INTERVAL_MS) {
        lastMoveSendTime = now;
        if (transport.connected && activeControls) {
          const sample = activeControls.sample();
          const moveBuf = encodeMovement(world.localPlayerGuid, sample);
          transport.sendDatagram(moveBuf);
        }
      }

      if (activeRenderer && activeControls) {
        const renderBuffer = world.getRenderBuffer(activeControls.position, activeControls.yaw);
        if (activeGameMode === "asteroid") {
          activeRenderer.syncVoxelChunks(voxelClient);
          const localEnt = world.entities.get(world.localPlayerGuid);
          const localFlags =
            localEnt?.stateFlags ??
            (latestAsteroidState?.shininess_tier != null
              ? (Number(latestAsteroidState.shininess_tier) & 0xff) << 16
              : 0);
          activeRenderer.syncEntities(renderBuffer, world.localPlayerGuid, world.entities, {
            pitch: activeControls.pitch,
            stateFlags: localFlags,
          });
        } else {
          activeRenderer.syncEntities(renderBuffer, world.localPlayerGuid);
          activeRenderer.updateWorldObjects(world.getWorldObjectsRenderBuffer());
        }
        activeRenderer.updateCamera(activeControls.position);
        activeRenderer.render();
      }

      ui.updateTelemetry(
        world.getTelemetry(),
        transport.pingMs,
        transport.bandwidthBps,
        playerPos
      );
    }

    requestAnimationFrame(animate);
  });
}
