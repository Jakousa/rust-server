/**
 * Protocol encoders and decoders for MMO schema packets in browser environment.
 */

export const OPCODES = {
  MOVEMENT: 0x0001,
  MOVEMENT_CORRECTION: 0x0002,
  INTERACTION: 0x0003,
  INTERACTION_RESULT: 0x0004,
  ENTITY_STATE: 0x0005,
  ENTITY_DESPAWN: 0x0006,
  PING: 0x0007,
  PONG: 0x0008,
  ENTITY_DELTA_BATCH: 0x0009,
  CLIENT_ACK: 0x000a,
  WORLD_OBJECT_SPAWN: 0x000b,
  WORLD_OBJECT_DELTA: 0x000c,
  WORLD_OBJECT_DESPAWN: 0x000d,
  WORLD_OBJECT_RESYNC: 0x000e,
  PLACE_OBJECT: 0x000f,
  INTERACT_OBJECT: 0x0010,
  AUTH_REQUEST: 0x0011,
  AUTH_RESPONSE: 0x0012,
};

export const INTERACTION_TYPES = {
  TALK: 0,
  ATTACK: 1,
  USE_ITEM: 2,
  LOOT: 3,
  TRADE: 4,
  CUSTOM: 5,
};

/**
 * Encodes ClientPacket::AuthRequest (Opcode 0x0011).
 */
export function encodeAuthRequest(token = "guest", clientVersion = 1) {
  const encoder = new TextEncoder();
  const tokenBytes = encoder.encode(token);
  const payloadLen = 2 + tokenBytes.length + 4;
  const frame = new Uint8Array(8 + payloadLen);
  const view = new DataView(frame.buffer);

  frame[0] = 0x4d; // 'M'
  frame[1] = 0x4d; // 'M'
  view.setUint16(2, OPCODES.AUTH_REQUEST, true);
  view.setUint32(4, payloadLen, true);

  view.setUint16(8, tokenBytes.length, true);
  frame.set(tokenBytes, 10);
  view.setUint32(10 + tokenBytes.length, clientVersion, true);

  return frame;
}

/**
 * Encodes ClientPacket::Interaction (Opcode 0x0003).
 */
export function encodeInteraction(
  sourceGuid,
  targetGuid,
  actionId,
  payloadBytes = new Uint8Array(0),
  interactionType = INTERACTION_TYPES.TRADE
) {
  const payloadLen = payloadBytes.byteLength;
  const totalPayloadSize = 25 + payloadLen;
  const frame = new Uint8Array(8 + totalPayloadSize);
  const view = new DataView(frame.buffer);

  frame[0] = 0x4d;
  frame[1] = 0x4d;
  view.setUint16(2, OPCODES.INTERACTION, true);
  view.setUint32(4, totalPayloadSize, true);

  view.setBigUint64(8, BigInt(sourceGuid), true);
  view.setBigUint64(16, BigInt(targetGuid), true);
  frame[24] = interactionType;
  view.setUint32(25, actionId, true);
  view.setUint32(29, payloadLen, true);
  frame.set(payloadBytes, 33);

  return frame;
}

/**
 * Decodes an unsigned LEB128 64-bit integer from a DataView at given byte offset.
 */
export function readVarintU64(view, offset) {
  let result = 0n;
  let shift = 0n;
  let bytesRead = 0;
  while (offset + bytesRead < view.byteLength) {
    const byte = view.getUint8(offset + bytesRead);
    bytesRead++;
    result |= BigInt(byte & 0x7f) << shift;
    if ((byte & 0x80) === 0) {
      break;
    }
    shift += 7n;
    if (shift >= 64n) break;
  }
  return { value: result, bytesRead };
}

/**
 * Decodes an unsigned LEB128 32-bit integer from a DataView at given byte offset.
 */
export function readVarintU32(view, offset) {
  let result = 0;
  let shift = 0;
  let bytesRead = 0;
  while (offset + bytesRead < view.byteLength) {
    const byte = view.getUint8(offset + bytesRead);
    bytesRead++;
    result |= (byte & 0x7f) << shift;
    if ((byte & 0x80) === 0) {
      break;
    }
    shift += 7;
    if (shift >= 35) break;
  }
  return { value: result >>> 0, bytesRead };
}

/**
 * Reads a length-prefixed UTF-8 string from a DataView at given byte offset.
 */
export function readVarintString(view, offset) {
  const { value: len, bytesRead: lenBytes } = readVarintU32(view, offset);
  const start = offset + lenBytes;
  if (start + len > view.byteLength) {
    return { value: "", bytesRead: lenBytes };
  }
  const bytes = new Uint8Array(view.buffer, view.byteOffset + start, len);
  const value = new TextDecoder().decode(bytes);
  return { value, bytesRead: lenBytes + len };
}

/**
 * Skips a serialized PropertyMap in a DataView at given byte offset.
 */
export function skipPropertyMap(view, offset) {
  let curOffset = offset;
  if (curOffset >= view.byteLength) return 0;
  const { value: count, bytesRead } = readVarintU32(view, curOffset);
  curOffset += bytesRead;
  for (let i = 0; i < count; i++) {
    if (curOffset >= view.byteLength) break;
    const keyRes = readVarintString(view, curOffset);
    curOffset += keyRes.bytesRead;
    if (curOffset >= view.byteLength) break;
    const tag = view.getUint8(curOffset);
    curOffset += 1;
    if (tag === 0) {
      // Null
    } else if (tag === 1) {
      // Bool
      curOffset += 1;
    } else if (tag === 2) {
      // Int
      const v = readVarintU64(view, curOffset);
      curOffset += v.bytesRead;
    } else if (tag === 3) {
      // Float
      curOffset += 8;
    } else if (tag === 4 || tag === 5) {
      // String or Bytes
      const s = readVarintString(view, curOffset);
      curOffset += s.bytesRead;
    }
  }
  return curOffset - offset;
}

/**
 * Encodes ClientPacket::Movement (Opcode 0x0001).
 * Payload size: 52 bytes (60 bytes frame total) matching MovementPacket::decode.
 */
export function encodeMovement(entityId, sample) {
  const payloadLen = 52;
  const moveBuf = new Uint8Array(8 + payloadLen);
  moveBuf[0] = 0x4d;
  moveBuf[1] = 0x4d;
  const view = new DataView(moveBuf.buffer);
  view.setUint16(2, OPCODES.MOVEMENT, true);
  view.setUint32(4, payloadLen, true);

  view.setBigUint64(8, BigInt(entityId), true);
  view.setFloat32(16, sample.x ?? 0.0, true);
  view.setFloat32(20, sample.y ?? 0.0, true);
  view.setFloat32(24, sample.z ?? 0.0, true);
  view.setFloat32(28, sample.vx ?? 0.0, true);
  view.setFloat32(32, sample.vy ?? 0.0, true);
  view.setFloat32(36, sample.vz ?? 0.0, true);
  view.setFloat32(40, sample.yaw ?? 0.0, true);
  view.setFloat32(44, sample.pitch ?? 0.0, true);
  const timestamp = sample.timestamp != null ? BigInt(sample.timestamp) : BigInt(Date.now());
  view.setBigUint64(48, timestamp, true);
  const sequence = (sample.sequence ?? 0) >>> 0;
  view.setUint32(56, sequence, true);

  return moveBuf;
}
