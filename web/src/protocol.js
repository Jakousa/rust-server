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
 * Encodes ClientPacket::Movement (Opcode 0x0001).
 */
export function encodeMovement(entityId, sample) {
  const moveBuf = new Uint8Array(48);
  moveBuf[0] = 0x4d;
  moveBuf[1] = 0x4d;
  moveBuf[2] = 0x01;
  moveBuf[3] = 0x00;
  moveBuf[4] = 0x28; // Payload len 40
  moveBuf[5] = 0x00;
  moveBuf[6] = 0x00;
  moveBuf[7] = 0x00;

  const view = new DataView(moveBuf.buffer);
  view.setBigUint64(8, BigInt(entityId), true);
  view.setFloat32(16, sample.x, true);
  view.setFloat32(20, sample.y, true);
  view.setFloat32(24, sample.z, true);
  view.setFloat32(28, 0.0, true);
  view.setFloat32(32, 0.0, true);
  view.setFloat32(36, 0.0, true);
  view.setFloat32(40, sample.yaw, true);
  view.setFloat32(44, 0.0, true);

  return moveBuf;
}
