import * as THREE from "three";
import { encodeInteraction, INTERACTION_TYPES } from "../../protocol.js";

export const VOXEL_MODULE_ID = 0x00a0;

export const SUB_OPCODE_CHUNK_REQUEST = 0x0001;
export const SUB_OPCODE_CHUNK_DATA = 0x0002;
export const SUB_OPCODE_DELTA_BATCH = 0x0003;
export const SUB_OPCODE_MODIFY_REQUEST = 0x0004;
export const SUB_OPCODE_MODIFY_RESPONSE = 0x0005;

export const CHUNK_SIZE = 16;
export const CHUNK_VOXELS = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE; // 4096
export const MAX_RLE_PAYLOAD_SIZE = 64 * 1024;
export const MAGIC_VXR1 = "VXR1";

export const VOXEL_EMPTY = 0;
export const VOXEL_CRUST_ROCK = 1;
export const VOXEL_TITANIUM = 2;
export const VOXEL_SOLAR_GOLD = 3;
export const VOXEL_VOID_CRYSTAL = 4;

export const VOXEL_MATERIALS = Object.freeze({
  [VOXEL_CRUST_ROCK]: Object.freeze({
    id: VOXEL_CRUST_ROCK,
    name: "Silicate Crust",
    color: "#64748b",
    hex: 0x64748b,
    unitValue: 0,
  }),
  [VOXEL_TITANIUM]: Object.freeze({
    id: VOXEL_TITANIUM,
    name: "Titanium",
    color: "#38bdf8",
    hex: 0x38bdf8,
    unitValue: 10,
  }),
  [VOXEL_SOLAR_GOLD]: Object.freeze({
    id: VOXEL_SOLAR_GOLD,
    name: "Solar Gold",
    color: "#facc15",
    hex: 0xfacc15,
    unitValue: 30,
  }),
  [VOXEL_VOID_CRYSTAL]: Object.freeze({
    id: VOXEL_VOID_CRYSTAL,
    name: "Void Crystal",
    color: "#c084fc",
    hex: 0xc084fc,
    unitValue: 75,
  }),
});

export const SEEDED_ASTEROID_CHUNKS = Object.freeze([
  Object.freeze({ x: 0, y: 0, z: -2 }),
  Object.freeze({ x: 1, y: 0, z: -1 }),
  Object.freeze({ x: -2, y: 0, z: 0 }),
  Object.freeze({ x: 0, y: 1, z: -1 }),
  Object.freeze({ x: 1, y: 0, z: 1 }),
  Object.freeze({ x: -1, y: -1, z: -2 }),
  Object.freeze({ x: -1, y: 0, z: -2 }),
  Object.freeze({ x: 0, y: -1, z: -2 }),
  Object.freeze({ x: -1, y: 0, z: 1 }),
  Object.freeze({ x: 1, y: -1, z: -1 }),
]);

const SEEDED_CHUNK_KEYS = new Set(SEEDED_ASTEROID_CHUNKS.map((c) => `${c.x},${c.y},${c.z}`));

/**
 * Returns metadata (`id`, `name`, `color`, `hex`, `unitValue`) for a voxel ID.
 */
export function getVoxelMaterialInfo(voxelId) {
  return (
    VOXEL_MATERIALS[voxelId] || {
      id: voxelId,
      name: voxelId === VOXEL_EMPTY ? "Empty Space" : `Voxel #${voxelId}`,
      color: "#94a3b8",
      hex: 0x94a3b8,
      unitValue: 0,
    }
  );
}

/**
 * Deterministic procedural 16x16x16 asteroid generator mirroring
 * `crates/gameplay/src/asteroid.rs::generate_asteroid_chunk`.
 */
export function generateProceduralAsteroidChunk(cx, cy, cz) {
  const voxels = new Uint16Array(CHUNK_VOXELS);
  const seedSalt = Math.abs(cx) * 7 + Math.abs(cy) * 13 + Math.abs(cz) * 19;

  for (let lz = 1; lz < CHUNK_SIZE - 1; lz++) {
    for (let ly = 1; ly < CHUNK_SIZE - 1; ly++) {
      for (let lx = 1; lx < CHUNK_SIZE - 1; lx++) {
        const dx = lx - 7.5;
        const dy = ly - 7.5;
        const dz = lz - 7.5;
        const dist = Math.hypot(dx, dy, dz);
        const bump = ((lx * 31 + ly * 17 + lz * 13 + seedSalt) % 5) * 0.2;

        let voxel = VOXEL_EMPTY;
        if (dist <= 1.8) {
          voxel = VOXEL_VOID_CRYSTAL;
        } else if (dist <= 3.1) {
          voxel = VOXEL_SOLAR_GOLD;
        } else if (dist <= 4.5) {
          voxel = VOXEL_TITANIUM;
        } else if (dist <= 5.6 + bump) {
          const surfaceHash = (lx * 3 + ly * 5 + lz * 7 + seedSalt) % 11;
          if (surfaceHash === 0) {
            voxel = VOXEL_SOLAR_GOLD;
          } else if (surfaceHash <= 2) {
            voxel = VOXEL_TITANIUM;
          } else {
            voxel = VOXEL_CRUST_ROCK;
          }
        }
        voxels[indexFromLocal(lx, ly, lz)] = voxel;
      }
    }
  }

  return voxels;
}

/**
 * Computes the 32-bit modular action_id for a voxel sub-opcode.
 */
export function voxelActionId(subOpcode) {
  return ((VOXEL_MODULE_ID << 16) | (subOpcode & 0xffff)) >>> 0;
}

export function chunkKey(cx, cy, cz) {
  return `${cx},${cy},${cz}`;
}

export function indexFromLocal(lx, ly, lz) {
  return lx + ly * CHUNK_SIZE + lz * CHUNK_SIZE * CHUNK_SIZE;
}

export function localFromIndex(idx) {
  const lx = idx % CHUNK_SIZE;
  const ly = Math.floor(idx / CHUNK_SIZE) % CHUNK_SIZE;
  const lz = Math.floor(idx / (CHUNK_SIZE * CHUNK_SIZE));
  return [lx, ly, lz];
}

export function worldPosToVoxelCoord(x, y, z) {
  return {
    x: Math.floor(x),
    y: Math.floor(y),
    z: Math.floor(z),
  };
}

export function voxelCoordToChunk(vx, vy, vz) {
  const cx = Math.floor(vx / CHUNK_SIZE);
  const cy = Math.floor(vy / CHUNK_SIZE);
  const cz = Math.floor(vz / CHUNK_SIZE);
  const lx = ((vx % CHUNK_SIZE) + CHUNK_SIZE) % CHUNK_SIZE;
  const ly = ((vy % CHUNK_SIZE) + CHUNK_SIZE) % CHUNK_SIZE;
  const lz = ((vz % CHUNK_SIZE) + CHUNK_SIZE) % CHUNK_SIZE;
  return {
    chunk: { x: cx, y: cy, z: cz },
    lx,
    ly,
    lz,
    index: indexFromLocal(lx, ly, lz),
  };
}

function toUint8View(bytes) {
  if (bytes instanceof Uint8Array) return bytes;
  if (bytes instanceof ArrayBuffer) return new Uint8Array(bytes);
  if (ArrayBuffer.isView(bytes)) {
    return new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }
  return new Uint8Array(bytes || 0);
}

/**
 * Encodes a 12-byte `VoxelChunkRequestPacket` payload (chunk x, y, z as i32 LE).
 */
export function encodeVoxelChunkRequest(cx, cy, cz) {
  const buf = new Uint8Array(12);
  const view = new DataView(buf.buffer);
  view.setInt32(0, cx, true);
  view.setInt32(4, cy, true);
  view.setInt32(8, cz, true);
  return buf;
}

/**
 * Wraps a `VoxelChunkRequestPacket` inside a 0x4D4D `INTERACTION` frame (`0x00A0` | `0x0001`).
 */
export function encodeVoxelChunkRequestInteraction(sourceGuid, cx, cy, cz) {
  return encodeInteraction(
    sourceGuid,
    0,
    voxelActionId(SUB_OPCODE_CHUNK_REQUEST),
    encodeVoxelChunkRequest(cx, cy, cz),
    INTERACTION_TYPES.CUSTOM
  );
}

/**
 * Encodes a 26-byte `VoxelModifyRequestPacket` payload (x, y, z as i64 LE + new_voxel u16 LE).
 */
export function encodeVoxelModifyRequest(vx, vy, vz, newVoxel = VOXEL_EMPTY) {
  const buf = new Uint8Array(26);
  const view = new DataView(buf.buffer);
  view.setBigInt64(0, BigInt(Math.trunc(vx)), true);
  view.setBigInt64(8, BigInt(Math.trunc(vy)), true);
  view.setBigInt64(16, BigInt(Math.trunc(vz)), true);
  view.setUint16(24, newVoxel & 0xffff, true);
  return buf;
}

/**
 * Wraps a `VoxelModifyRequestPacket` inside a 0x4D4D `INTERACTION` frame (`0x00A0` | `0x0004`).
 */
export function encodeVoxelModifyInteraction(sourceGuid, vx, vy, vz, newVoxel = VOXEL_EMPTY) {
  return encodeInteraction(
    sourceGuid,
    0,
    voxelActionId(SUB_OPCODE_MODIFY_REQUEST),
    encodeVoxelModifyRequest(vx, vy, vz, newVoxel),
    INTERACTION_TYPES.CUSTOM
  );
}

/**
 * Decodes a `VXR1` RLE compressed chunk stream into 4,096 Uint16 voxels + revision.
 * Strictly enforces magic header, 64KB payload limit, and exact 4,096 voxel sum.
 */
export function decodeRleVoxelChunk(rawBytes) {
  const bytes = toUint8View(rawBytes);
  if (bytes.byteLength > MAX_RLE_PAYLOAD_SIZE) {
    throw new Error(`RLE payload exceeds max size (${bytes.byteLength} > ${MAX_RLE_PAYLOAD_SIZE})`);
  }
  if (bytes.byteLength < 16) {
    throw new Error(`RLE payload too small (${bytes.byteLength} < 16)`);
  }

  // Verify magic "VXR1"
  if (
    bytes[0] !== 0x56 || // 'V'
    bytes[1] !== 0x58 || // 'X'
    bytes[2] !== 0x52 || // 'R'
    bytes[3] !== 0x31    // '1'
  ) {
    throw new Error("Invalid VXR1 magic header");
  }

  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const revision = Number(view.getBigUint64(4, true));
  const spanCount = view.getUint32(12, true);

  const voxels = new Uint16Array(CHUNK_VOXELS);
  let offset = 16;
  let totalCount = 0;

  for (let i = 0; i < spanCount; i++) {
    if (offset + 4 > bytes.byteLength) {
      throw new Error(`Unexpected EOF reading RLE span ${i}`);
    }
    const count = view.getUint16(offset, true);
    const voxelId = view.getUint16(offset + 2, true);
    offset += 4;

    if (totalCount + count > CHUNK_VOXELS) {
      throw new Error(`Malformed RLE count: ${totalCount + count} exceeds ${CHUNK_VOXELS}`);
    }
    if (voxelId !== 0) {
      voxels.fill(voxelId, totalCount, totalCount + count);
    }
    totalCount += count;
  }

  if (totalCount !== CHUNK_VOXELS) {
    throw new Error(`Malformed RLE count: sum is ${totalCount}, expected ${CHUNK_VOXELS}`);
  }

  return { revision, voxels };
}

/**
 * Decodes `VoxelChunkDataPacket` (12B chunk i32 LE + VXR1 RLE payload).
 */
export function decodeVoxelChunkData(rawBytes) {
  const bytes = toUint8View(rawBytes);
  if (bytes.byteLength < 12) {
    throw new Error(`VoxelChunkData payload truncated (${bytes.byteLength} < 12)`);
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const x = view.getInt32(0, true);
  const y = view.getInt32(4, true);
  const z = view.getInt32(8, true);
  const { revision, voxels } = decodeRleVoxelChunk(bytes.subarray(12));
  return {
    chunk: { x, y, z },
    revision,
    voxels,
  };
}

/**
 * Decodes `VoxelModifyResponsePacket` (status u8 + x,y,z i64 LE + new_voxel u16 LE).
 */
export function decodeVoxelModifyResponse(rawBytes) {
  const bytes = toUint8View(rawBytes);
  if (bytes.byteLength < 27) {
    throw new Error(`VoxelModifyResponse payload truncated (${bytes.byteLength} < 27)`);
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const status = bytes[0];
  const x = Number(view.getBigInt64(1, true));
  const y = Number(view.getBigInt64(9, true));
  const z = Number(view.getBigInt64(17, true));
  const newVoxel = view.getUint16(25, true);
  return { status, x, y, z, newVoxel };
}

/**
 * Decodes `VoxelDeltaBatchPacket` (12B chunk + 8B revision + 2B count + N*5B entries).
 */
export function decodeVoxelDeltaBatch(rawBytes) {
  const bytes = toUint8View(rawBytes);
  if (bytes.byteLength < 22) {
    throw new Error(`VoxelDeltaBatch payload truncated (${bytes.byteLength} < 22)`);
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const x = view.getInt32(0, true);
  const y = view.getInt32(4, true);
  const z = view.getInt32(8, true);
  const revision = Number(view.getBigUint64(12, true));
  const count = view.getUint16(20, true);
  if (bytes.byteLength < 22 + count * 5) {
    throw new Error(`VoxelDeltaBatch truncated for ${count} deltas`);
  }
  const deltas = [];
  let offset = 22;
  for (let i = 0; i < count; i++) {
    const lx = bytes[offset];
    const ly = bytes[offset + 1];
    const lz = bytes[offset + 2];
    const newVoxel = view.getUint16(offset + 3, true);
    offset += 5;
    if (lx < CHUNK_SIZE && ly < CHUNK_SIZE && lz < CHUNK_SIZE) {
      deltas.push({ lx, ly, lz, newVoxel });
    }
  }
  return { chunk: { x, y, z }, revision, deltas };
}

const FACE_DEFS = [
  // +X (Right)
  {
    dx: 1,
    dy: 0,
    dz: 0,
    normal: [1, 0, 0],
    corners: [
      [1, 0, 0],
      [1, 1, 0],
      [1, 1, 1],
      [1, 0, 1],
    ],
  },
  // -X (Left)
  {
    dx: -1,
    dy: 0,
    dz: 0,
    normal: [-1, 0, 0],
    corners: [
      [0, 0, 1],
      [0, 1, 1],
      [0, 1, 0],
      [0, 0, 0],
    ],
  },
  // +Y (Top)
  {
    dx: 0,
    dy: 1,
    dz: 0,
    normal: [0, 1, 0],
    corners: [
      [0, 1, 0],
      [0, 1, 1],
      [1, 1, 1],
      [1, 1, 0],
    ],
  },
  // -Y (Bottom)
  {
    dx: 0,
    dy: -1,
    dz: 0,
    normal: [0, -1, 0],
    corners: [
      [0, 0, 1],
      [0, 0, 0],
      [1, 0, 0],
      [1, 0, 1],
    ],
  },
  // +Z (Front)
  {
    dx: 0,
    dy: 0,
    dz: 1,
    normal: [0, 0, 1],
    corners: [
      [0, 0, 1],
      [1, 0, 1],
      [1, 1, 1],
      [0, 1, 1],
    ],
  },
  // -Z (Back)
  {
    dx: 0,
    dy: 0,
    dz: -1,
    normal: [0, 0, -1],
    corners: [
      [1, 0, 0],
      [0, 0, 0],
      [0, 1, 0],
      [1, 1, 0],
    ],
  },
];

const _colorHelper = new THREE.Color();

function getMaterialRgb(voxelId) {
  const mat = VOXEL_MATERIALS[voxelId];
  _colorHelper.setHex(mat ? mat.hex : 0x94a3b8);
  return [_colorHelper.r, _colorHelper.g, _colorHelper.b];
}

function isNeighborEmpty(lx, ly, lz, dx, dy, dz, voxels, neighbors) {
  const nx = lx + dx;
  const ny = ly + dy;
  const nz = lz + dz;

  if (nx >= 0 && nx < CHUNK_SIZE && ny >= 0 && ny < CHUNK_SIZE && nz >= 0 && nz < CHUNK_SIZE) {
    return voxels[indexFromLocal(nx, ny, nz)] === VOXEL_EMPTY;
  }

  if (!neighbors) {
    // Unloaded space around floating asteroid chunks is vacuum
    return true;
  }

  let neighborArr = null;
  let nLx = nx;
  let nLy = ny;
  let nLz = nz;

  if (nx >= CHUNK_SIZE) {
    neighborArr = neighbors.posX?.voxels || neighbors.posX;
    nLx = 0;
  } else if (nx < 0) {
    neighborArr = neighbors.negX?.voxels || neighbors.negX;
    nLx = CHUNK_SIZE - 1;
  } else if (ny >= CHUNK_SIZE) {
    neighborArr = neighbors.posY?.voxels || neighbors.posY;
    nLy = 0;
  } else if (ny < 0) {
    neighborArr = neighbors.negY?.voxels || neighbors.negY;
    nLy = CHUNK_SIZE - 1;
  } else if (nz >= CHUNK_SIZE) {
    neighborArr = neighbors.posZ?.voxels || neighbors.posZ;
    nLz = 0;
  } else if (nz < 0) {
    neighborArr = neighbors.negZ?.voxels || neighbors.negZ;
    nLz = CHUNK_SIZE - 1;
  }

  if (!neighborArr) {
    return true;
  }
  return neighborArr[indexFromLocal(nLx, nLy, nLz)] === VOXEL_EMPTY;
}

/**
 * Builds a face-culled `THREE.BufferGeometry` for a 16x16x16 voxel chunk with
 * material vertex colors (`1=Silicate Crust`, `2=Titanium`, `3=Solar Gold`, `4=Void Crystal`).
 */
export function buildChunkBufferGeometry(voxels, neighbors = null) {
  const positions = [];
  const normals = [];
  const colors = [];

  for (let idx = 0; idx < CHUNK_VOXELS; idx++) {
    const voxelId = voxels[idx];
    if (voxelId === VOXEL_EMPTY) continue;

    const [lx, ly, lz] = localFromIndex(idx);
    const [r, g, b] = getMaterialRgb(voxelId);

    for (let f = 0; f < FACE_DEFS.length; f++) {
      const face = FACE_DEFS[f];
      if (!isNeighborEmpty(lx, ly, lz, face.dx, face.dy, face.dz, voxels, neighbors)) {
        continue;
      }

      const [c0, c1, c2, c3] = face.corners;
      const [nx, ny, nz] = face.normal;

      // Two triangles: (c0, c1, c2) and (c0, c2, c3)
      const triVerts = [c0, c1, c2, c0, c2, c3];
      for (let v = 0; v < 6; v++) {
        const c = triVerts[v];
        positions.push(lx + c[0], ly + c[1], lz + c[2]);
        normals.push(nx, ny, nz);
        colors.push(r, g, b);
      }
    }
  }

  const geometry = new THREE.BufferGeometry();
  geometry.setAttribute("position", new THREE.Float32BufferAttribute(positions, 3));
  geometry.setAttribute("normal", new THREE.Float32BufferAttribute(normals, 3));
  geometry.setAttribute("color", new THREE.Float32BufferAttribute(colors, 3));
  if (positions.length > 0) {
    geometry.computeBoundingBox();
    geometry.computeBoundingSphere();
  }
  return geometry;
}

/**
 * Client-side voxel cache, 0x00A0 packet handler, mesh builder, and raycast picker.
 */
export class AsteroidVoxelClient {
  constructor() {
    this.chunks = new Map();
    this.dirtyMeshes = new Set();
    this.requestedChunks = new Set();
    this.pendingChunkRequests = new Map();
    this.requestRetryTtlMs = 2000;
    this.maxEmptyChunkRetries = 3;
    this.maxSeededChunkRetries = 10;
  }

  clear() {
    this.chunks.clear();
    this.dirtyMeshes.clear();
    this.requestedChunks.clear();
    this.pendingChunkRequests.clear();
  }

  /**
   * Clears in-flight request tracking so chunks (including preview chunks) are
   * cleanly requested after authentication succeeds.
   */
  resetRequestTracking() {
    this.requestedChunks.clear();
    this.pendingChunkRequests.clear();
  }

  /**
   * Populates deterministic procedural preview asteroids (`isPreview = true`) so the
   * 3D belt is immediately visible offline or prior to server sync.
   */
  seedLocalPreviewChunks(overwriteAuthoritative = false) {
    let seededCount = 0;
    for (const c of SEEDED_ASTEROID_CHUNKS) {
      const existing = this.getChunk(c.x, c.y, c.z);
      if (!existing || overwriteAuthoritative) {
        const voxels = generateProceduralAsteroidChunk(c.x, c.y, c.z);
        this.setChunk(c.x, c.y, c.z, voxels, 0, { isPreview: true });
        seededCount++;
      }
    }
    return seededCount;
  }

  /**
   * Stores decoded chunk voxels at `(cx, cy, cz)` and marks self + adjacent loaded chunks dirty.
   */
  setChunk(cx, cy, cz, voxels, revision = 0, options = {}) {
    const key = chunkKey(cx, cy, cz);
    const data = voxels instanceof Uint16Array ? voxels : new Uint16Array(voxels);
    const isPreview = Boolean(options?.isPreview);

    let solidCount = 0;
    let oreCount = 0;
    for (let i = 0; i < data.length; i++) {
      const v = data[i];
      if (v !== VOXEL_EMPTY) {
        solidCount++;
        if (v >= VOXEL_TITANIUM) oreCount++;
      }
    }

    this.chunks.set(key, {
      x: cx,
      y: cy,
      z: cz,
      revision,
      voxels: data,
      isPreview,
      solidCount,
      oreCount,
    });
    this.dirtyMeshes.add(key);

    if (!isPreview) {
      this.pendingChunkRequests.delete(key);
      this.requestedChunks.add(key);
    }

    // Flag loaded cardinal neighbors dirty for seamless cross-chunk boundary culling
    const neighbors = [
      [cx + 1, cy, cz],
      [cx - 1, cy, cz],
      [cx, cy + 1, cz],
      [cx, cy - 1, cz],
      [cx, cy, cz + 1],
      [cx, cy, cz - 1],
    ];
    for (const [nx, ny, nz] of neighbors) {
      const nKey = chunkKey(nx, ny, nz);
      if (this.chunks.has(nKey)) {
        this.dirtyMeshes.add(nKey);
      }
    }
  }

  getChunk(cx, cy, cz) {
    return this.chunks.get(chunkKey(cx, cy, cz)) || null;
  }

  getVoxel(vx, vy, vz) {
    const { chunk, index } = voxelCoordToChunk(vx, vy, vz);
    const entry = this.chunks.get(chunkKey(chunk.x, chunk.y, chunk.z));
    return entry ? entry.voxels[index] : VOXEL_EMPTY;
  }

  /**
   * Updates a single voxel at world voxel coordinate `(vx, vy, vz)` if its chunk is loaded.
   */
  setVoxel(vx, vy, vz, newVoxel) {
    const { chunk, lx, ly, lz, index } = voxelCoordToChunk(vx, vy, vz);
    const key = chunkKey(chunk.x, chunk.y, chunk.z);
    const entry = this.chunks.get(key);
    if (!entry) return null;

    const prev = entry.voxels[index];
    entry.voxels[index] = newVoxel;
    entry.revision += 1;
    if (prev === VOXEL_EMPTY && newVoxel !== VOXEL_EMPTY) {
      entry.solidCount = (entry.solidCount ?? 0) + 1;
      if (newVoxel >= VOXEL_TITANIUM) entry.oreCount = (entry.oreCount ?? 0) + 1;
    } else if (prev !== VOXEL_EMPTY && newVoxel === VOXEL_EMPTY) {
      entry.solidCount = Math.max(0, (entry.solidCount ?? 1) - 1);
      if (prev >= VOXEL_TITANIUM) entry.oreCount = Math.max(0, (entry.oreCount ?? 1) - 1);
    }
    this.dirtyMeshes.add(key);

    if (lx === 0) this._markDirtyIfLoaded(chunk.x - 1, chunk.y, chunk.z);
    if (lx === CHUNK_SIZE - 1) this._markDirtyIfLoaded(chunk.x + 1, chunk.y, chunk.z);
    if (ly === 0) this._markDirtyIfLoaded(chunk.x, chunk.y - 1, chunk.z);
    if (ly === CHUNK_SIZE - 1) this._markDirtyIfLoaded(chunk.x, chunk.y + 1, chunk.z);
    if (lz === 0) this._markDirtyIfLoaded(chunk.x, chunk.y, chunk.z - 1);
    if (lz === CHUNK_SIZE - 1) this._markDirtyIfLoaded(chunk.x, chunk.y, chunk.z + 1);

    return prev;
  }

  _markDirtyIfLoaded(cx, cy, cz) {
    const key = chunkKey(cx, cy, cz);
    if (this.chunks.has(key)) {
      this.dirtyMeshes.add(key);
    }
  }

  encodeChunkRequestPacket(sourceGuid, cx, cy, cz) {
    return encodeVoxelChunkRequestInteraction(sourceGuid, cx, cy, cz);
  }

  encodeModifyRequestPacket(sourceGuid, vx, vy, vz, newVoxel = VOXEL_EMPTY) {
    return encodeVoxelModifyInteraction(sourceGuid, vx, vy, vz, newVoxel);
  }

  /**
   * Handles incoming `OPCODES.INTERACTION_RESULT` for `VOXEL_MODULE_ID` (`0x00A0`).
   */
  handleInteractionResult(actionId, rawBytes) {
    const moduleId = (actionId >>> 16) & 0xffff;
    if (moduleId !== VOXEL_MODULE_ID) return null;

    const subOpcode = actionId & 0xffff;
    const bytes = toUint8View(rawBytes);

    if (subOpcode === SUB_OPCODE_CHUNK_REQUEST || subOpcode === SUB_OPCODE_CHUNK_DATA) {
      if (bytes.byteLength < 12) return null;
      const decoded = decodeVoxelChunkData(bytes);
      this.setChunk(decoded.chunk.x, decoded.chunk.y, decoded.chunk.z, decoded.voxels, decoded.revision, {
        isPreview: false,
      });
      return { type: "chunk_data", ...decoded };
    }

    if (subOpcode === SUB_OPCODE_DELTA_BATCH) {
      if (bytes.byteLength < 22) return null;
      const batch = decodeVoxelDeltaBatch(bytes);
      const entry = this.getChunk(batch.chunk.x, batch.chunk.y, batch.chunk.z);
      if (entry) {
        for (const d of batch.deltas) {
          const idx = indexFromLocal(d.lx, d.ly, d.lz);
          const prev = entry.voxels[idx];
          entry.voxels[idx] = d.newVoxel;
          if (prev === VOXEL_EMPTY && d.newVoxel !== VOXEL_EMPTY) {
            entry.solidCount = (entry.solidCount ?? 0) + 1;
            if (d.newVoxel >= VOXEL_TITANIUM) entry.oreCount = (entry.oreCount ?? 0) + 1;
          } else if (prev !== VOXEL_EMPTY && d.newVoxel === VOXEL_EMPTY) {
            entry.solidCount = Math.max(0, (entry.solidCount ?? 1) - 1);
            if (prev >= VOXEL_TITANIUM) entry.oreCount = Math.max(0, (entry.oreCount ?? 1) - 1);
          }
        }
        entry.revision = batch.revision;
        entry.isPreview = false;
        this.pendingChunkRequests.delete(chunkKey(batch.chunk.x, batch.chunk.y, batch.chunk.z));
        this.dirtyMeshes.add(chunkKey(batch.chunk.x, batch.chunk.y, batch.chunk.z));
      }
      return { type: "delta_batch", ...batch };
    }

    if (subOpcode === SUB_OPCODE_MODIFY_REQUEST || subOpcode === SUB_OPCODE_MODIFY_RESPONSE) {
      if (bytes.byteLength < 27) return null;
      const resp = decodeVoxelModifyResponse(bytes);
      if (resp.status === 0) {
        this.setVoxel(resp.x, resp.y, resp.z, resp.newVoxel);
      }
      return { type: "modify_response", ...resp };
    }

    return null;
  }

  /**
   * Builds a face-culled `THREE.BufferGeometry` for chunk `(cx, cy, cz)` using loaded neighbors.
   */
  buildChunkGeometry(cx, cy, cz) {
    const entry = this.getChunk(cx, cy, cz);
    if (!entry) return null;
    const neighbors = {
      posX: this.getChunk(cx + 1, cy, cz),
      negX: this.getChunk(cx - 1, cy, cz),
      posY: this.getChunk(cx, cy + 1, cz),
      negY: this.getChunk(cx, cy - 1, cz),
      posZ: this.getChunk(cx, cy, cz + 1),
      negZ: this.getChunk(cx, cy, cz - 1),
    };
    return buildChunkBufferGeometry(entry.voxels, neighbors);
  }

  /**
   * Returns unrequested (or expired-pending / preview-only) chunk coordinates around `pos`,
   * prioritizing known seeded asteroid chunks first.
   */
  collectUnrequestedChunks(pos, radiusX = 2, radiusY = 1, radiusZ = 2, nowMs = Date.now()) {
    const center = voxelCoordToChunk(
      Math.floor(pos?.x ?? 0),
      Math.floor(pos?.y ?? 0),
      Math.floor(pos?.z ?? 0)
    ).chunk;

    const toRequest = [];
    const seen = new Set();
    const addCandidate = (cx, cy, cz) => {
      const key = chunkKey(cx, cy, cz);
      if (seen.has(key)) return;
      seen.add(key);

      const existing = this.chunks.get(key);
      if (existing && !existing.isPreview) return;

      const pending = this.pendingChunkRequests.get(key);
      const isSeeded = SEEDED_CHUNK_KEYS.has(key);
      const maxAttempts = isSeeded ? this.maxSeededChunkRetries : this.maxEmptyChunkRetries;
      if (pending) {
        if (nowMs - pending.sentAt < this.requestRetryTtlMs) return;
        if (pending.attempts >= maxAttempts) return;
      }

      const nextAttempts = (pending?.attempts ?? 0) + 1;
      this.pendingChunkRequests.set(key, { sentAt: nowMs, attempts: nextAttempts });
      this.requestedChunks.add(key);
      toRequest.push({ x: cx, y: cy, z: cz });
    };

    for (const c of SEEDED_ASTEROID_CHUNKS) {
      addCandidate(c.x, c.y, c.z);
    }

    for (let dx = -radiusX; dx <= radiusX; dx++) {
      for (let dy = -radiusY; dy <= radiusY; dy++) {
        for (let dz = -radiusZ; dz <= radiusZ; dz++) {
          addCandidate(center.x + dx, center.y + dy, center.z + dz);
        }
      }
    }

    return toRequest;
  }

  /**
   * Summarizes loaded non-empty asteroid clusters sorted by distance to `shipPos`.
   */
  getAsteroidClusterSummaries(shipPos = { x: 0, y: 0, z: 0 }) {
    const sx = shipPos?.x ?? 0;
    const sy = shipPos?.y ?? 0;
    const sz = shipPos?.z ?? 0;
    const clusters = [];

    for (const chunk of this.chunks.values()) {
      const solidCount = chunk.solidCount ?? 0;
      if (solidCount === 0) continue;
      const cx = chunk.x * CHUNK_SIZE + 8.0;
      const cy = chunk.y * CHUNK_SIZE + 8.0;
      const cz = chunk.z * CHUNK_SIZE + 8.0;
      const distance = Math.hypot(cx - sx, cy - sy, cz - sz);
      clusters.push({
        key: chunkKey(chunk.x, chunk.y, chunk.z),
        cx: chunk.x,
        cy: chunk.y,
        cz: chunk.z,
        center: { x: cx, y: cy, z: cz },
        distance,
        solidCount,
        oreCount: chunk.oreCount ?? 0,
        isPreview: Boolean(chunk.isPreview),
      });
    }

    clusters.sort((a, b) => a.distance - b.distance);
    return clusters;
  }

  /**
   * Returns aggregate loaded chunk & voxel statistics for diagnostics.
   */
  getLoadedStats() {
    let authoritativeCount = 0;
    let previewCount = 0;
    let totalSolidVoxels = 0;
    let totalOreVoxels = 0;
    for (const chunk of this.chunks.values()) {
      if (chunk.isPreview) previewCount++;
      else authoritativeCount++;
      totalSolidVoxels += chunk.solidCount ?? 0;
      totalOreVoxels += chunk.oreCount ?? 0;
    }
    return {
      chunkCount: this.chunks.size,
      authoritativeCount,
      previewCount,
      totalSolidVoxels,
      totalOreVoxels,
      pendingRequests: this.pendingChunkRequests.size,
    };
  }

  /**
   * 3D DDA raycast returning the first non-empty voxel hit along `direction` within `maxDistance`.
   */
  raycastFirstSolid(origin, direction, maxDistance = 12.0) {
    const len = Math.hypot(direction.x, direction.y, direction.z);
    if (len < 1e-6) return null;
    const dx = direction.x / len;
    const dy = direction.y / len;
    const dz = direction.z / len;

    let vx = Math.floor(origin.x);
    let vy = Math.floor(origin.y);
    let vz = Math.floor(origin.z);

    const stepX = dx >= 0 ? 1 : -1;
    const stepY = dy >= 0 ? 1 : -1;
    const stepZ = dz >= 0 ? 1 : -1;

    const tDeltaX = dx !== 0 ? Math.abs(1 / dx) : Infinity;
    const tDeltaY = dy !== 0 ? Math.abs(1 / dy) : Infinity;
    const tDeltaZ = dz !== 0 ? Math.abs(1 / dz) : Infinity;

    let tMaxX = dx !== 0 ? ((dx > 0 ? vx + 1 - origin.x : origin.x - vx) * tDeltaX) : Infinity;
    let tMaxY = dy !== 0 ? ((dy > 0 ? vy + 1 - origin.y : origin.y - vy) * tDeltaY) : Infinity;
    let tMaxZ = dz !== 0 ? ((dz > 0 ? vz + 1 - origin.z : origin.z - vz) * tDeltaZ) : Infinity;

    let dist = 0;
    while (dist <= maxDistance) {
      const voxelId = this.getVoxel(vx, vy, vz);
      if (voxelId !== VOXEL_EMPTY) {
        return {
          x: vx,
          y: vy,
          z: vz,
          voxelId,
          distance: dist,
          center: { x: vx + 0.5, y: vy + 0.5, z: vz + 0.5 },
        };
      }

      if (tMaxX < tMaxY) {
        if (tMaxX < tMaxZ) {
          dist = tMaxX;
          vx += stepX;
          tMaxX += tDeltaX;
        } else {
          dist = tMaxZ;
          vz += stepZ;
          tMaxZ += tDeltaZ;
        }
      } else {
        if (tMaxY < tMaxZ) {
          dist = tMaxY;
          vy += stepY;
          tMaxY += tDeltaY;
        } else {
          dist = tMaxZ;
          vz += stepZ;
          tMaxZ += tDeltaZ;
        }
      }
    }

    return null;
  }

  /**
   * Finds the nearest solid voxel within `maxRadius` of `pos` (optionally prioritizing ore blocks).
   */
  findNearestSolidVoxel(pos, maxRadius = 6.5, preferOre = true) {
    let bestOre = null;
    let bestOreDistSq = Infinity;
    let bestAny = null;
    let bestAnyDistSq = Infinity;
    const maxDistSq = maxRadius * maxRadius;

    for (const chunk of this.chunks.values()) {
      const baseX = chunk.x * CHUNK_SIZE;
      const baseY = chunk.y * CHUNK_SIZE;
      const baseZ = chunk.z * CHUNK_SIZE;

      // Coarse chunk AABB reject
      const closestCx = Math.max(baseX, Math.min(pos.x, baseX + CHUNK_SIZE));
      const closestCy = Math.max(baseY, Math.min(pos.y, baseY + CHUNK_SIZE));
      const closestCz = Math.max(baseZ, Math.min(pos.z, baseZ + CHUNK_SIZE));
      const cDx = closestCx - pos.x;
      const cDy = closestCy - pos.y;
      const cDz = closestCz - pos.z;
      if (cDx * cDx + cDy * cDy + cDz * cDz > maxDistSq) {
        continue;
      }

      for (let idx = 0; idx < CHUNK_VOXELS; idx++) {
        const voxelId = chunk.voxels[idx];
        if (voxelId === VOXEL_EMPTY) continue;
        const [lx, ly, lz] = localFromIndex(idx);
        const wx = baseX + lx;
        const wy = baseY + ly;
        const wz = baseZ + lz;
        const ddx = wx + 0.5 - pos.x;
        const ddy = wy + 0.5 - pos.y;
        const ddz = wz + 0.5 - pos.z;
        const distSq = ddx * ddx + ddy * ddy + ddz * ddz;
        if (distSq > maxDistSq) continue;

        if (preferOre && voxelId >= VOXEL_TITANIUM && distSq < bestOreDistSq) {
          bestOreDistSq = distSq;
          bestOre = {
            x: wx,
            y: wy,
            z: wz,
            voxelId,
            distance: Math.sqrt(distSq),
            center: { x: wx + 0.5, y: wy + 0.5, z: wz + 0.5 },
          };
        }
        if (distSq < bestAnyDistSq) {
          bestAnyDistSq = distSq;
          bestAny = {
            x: wx,
            y: wy,
            z: wz,
            voxelId,
            distance: Math.sqrt(distSq),
            center: { x: wx + 0.5, y: wy + 0.5, z: wz + 0.5 },
          };
        }
      }
    }

    return bestOre || bestAny;
  }
}
