import { WebTransportClient } from "./transport.js";
import { WorldRenderer } from "./renderer.js";
import { PlayerControls } from "./controls.js";
import { UIController } from "./ui.js";
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
      const rawEntityId = view.getBigUint64(8, true);
      const entityId = rawEntityId <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(rawEntityId) : rawEntityId;
      const x = view.getFloat32(16, true);
      const y = view.getFloat32(20, true);
      const z = view.getFloat32(24, true);
      const vx = view.getFloat32(28, true);
      const vy = view.getFloat32(32, true);
      const vz = view.getFloat32(36, true);
      const yaw = view.getFloat32(40, true);
      const health = view.getUint32(48, true);
      const maxHealth = view.getUint32(52, true);

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
          hpPct: maxHealth > 0 ? health / maxHealth : 1.0,
          castPct: 0.0,
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
        existing.hpPct = maxHealth > 0 ? health / maxHealth : 1.0;
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

        this.latestServerTick = Number(tickRes.value);
        this.latestBaselineTick = Number(baseRes.value);

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
export function getOrCreateSessionToken() {
  if (typeof window === "undefined" || !window.sessionStorage) {
    return "guest-poc";
  }
  const params = new URLSearchParams(window.location.search);
  const tokenParam = params.get("token");
  if (tokenParam) {
    window.sessionStorage.setItem("mmo_token", tokenParam);
    return tokenParam;
  }
  let token = window.sessionStorage.getItem("mmo_token");
  if (!token) {
    const rand = Math.random().toString(36).substring(2, 8);
    const ts = Date.now().toString(36);
    token = `guest-${ts}-${rand}`;
    window.sessionStorage.setItem("mmo_token", token);
  }
  return token;
}

/**
 * Automatically retrieves the ephemeral SHA-256 certificate digest
 * exposed by TelemetryServer GET /cert-hash.
 */
export async function autoFetchCertHash() {
  const certInput = document.getElementById("cert-hash");
  if (certInput && certInput.value.trim().length > 0) return;
  try {
    // 1. First try relative /cert-hash (works over both HTTP and HTTPS via ingress reverse proxy)
    let res = null;
    try {
      res = await fetch("/cert-hash");
    } catch {
      // Fallback below
    }

    if (!res || !res.ok) {
      // 2. Fall back to port 9091 matching active protocol or HTTP
      const proto = window.location.protocol === "https:" ? "https:" : "http:";
      const adminUrl = `${proto}//${window.location.hostname || "127.0.0.1"}:9091/cert-hash`;
      try {
        res = await fetch(adminUrl);
      } catch {
        // Fall back to plain http
        res = await fetch(`http://${window.location.hostname || "127.0.0.1"}:9091/cert-hash`);
      }
    }

    if (res && res.ok) {
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
  const renderer = new WorldRenderer(container);
  const controls = new PlayerControls();
  const transport = new WebTransportClient();
  const ui = new UIController();
  const world = new ClientWorldBridge();

  // Dynamic gateway host resolution
  const host = window.location.hostname || "127.0.0.1";
  const defaultGatewayUrl = `https://${host}:4433`;
  const urlInput = document.getElementById("gateway-url");
  if (urlInput && (!urlInput.value || urlInput.value.includes("127.0.0.1"))) {
    urlInput.value = defaultGatewayUrl;
  }

  // Try to load cert hash from URL parameters if provided (e.g. ?cert=aa:bb:cc...)
  const params = new URLSearchParams(window.location.search);
  const certFromUrl = params.get("cert");
  if (certFromUrl) {
    const certInput = document.getElementById("cert-hash");
    if (certInput) certInput.value = certFromUrl;
  } else {
    await autoFetchCertHash();
  }

  // Auto-connect (WebTransport with automatic WebSocket fallback)
  setTimeout(() => {
    ui.btnConnect?.click();
  }, 150);

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

  // Authoritative server movement correction listener
  let hasSyncedInitialPosition = false;
  world.onAuthResponse(() => {
    hasSyncedInitialPosition = false;
  });
  world.onLocalPlayerKeyframe((state) => {
    if (!hasSyncedInitialPosition) {
      controls.setPosition(state.x, state.y, state.z);
      hasSyncedInitialPosition = true;
    }
  });

  world.onMovementCorrection((corr) => {
    if (BigInt(corr.entityId) === BigInt(world.localPlayerGuid)) {
      controls.reconcile(corr.sequence, corr.x, corr.y, corr.z);
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

  transport.onStatus(({ status, error }) => {
    const statusText =
      status === "connected"
        ? "Connected"
        : status === "connecting"
        ? "Connecting"
        : "Disconnected";
    const errorMsg =
      status === "disconnected" && error ? formatConnectionError(error) : "";
    ui.setConnectionState(status, statusText, errorMsg);
    if (status === "disconnected") {
      hasSyncedInitialPosition = false;
      world.clear();
      renderer.clear();
    }
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
        ui.setConnectionState("connecting", "Connecting", "");
        await transport.connect(url, certHash);
        // Send AuthRequest handshake upon connection with isolated session token
        const sessionToken = getOrCreateSessionToken();
        const authPkt = encodeAuthRequest(sessionToken, 1);
        await transport.sendStream(authPkt);

        // Initial inventory inspection query
        setTimeout(() => {
          performInspect({ id: 0 });
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
      renderer.clear();
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
