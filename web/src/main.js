import { WebTransportClient } from "./transport.js";
import { WorldRenderer } from "./renderer.js";
import { PlayerControls } from "./controls.js";
import { UIController } from "./ui.js";

/**
 * JS/WASM World Adapter providing unified interface for packet ingestion,
 * interpolation, and flat render buffer generation.
 */
class ClientWorldBridge {
  constructor() {
    this.entities = new Map();
    this.worldObjects = new Map();
    this.latestServerTick = 0;
    this.latestBaselineTick = 0;
    this.receivedPackets = 0;
    this.localPlayerGuid = 1;
    this.clientTime = 0.0;
  }

  processPacket(data) {
    const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
    if (view.byteLength < 4) return;

    // Check MM magic
    if (view.getUint8(0) !== 0x4d || view.getUint8(1) !== 0x4d) return;
    const opcode = view.getUint16(2, true);

    this.receivedPackets++;

    // Opcode 0x0004: EntityState
    if (opcode === 0x0004 && view.byteLength >= 50) {
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
    }

    // Opcode 0x0005: EntityDespawn
    if (opcode === 0x0005 && view.byteLength >= 16) {
      const entityId = Number(view.getBigUint64(8, true));
      this.entities.delete(entityId);
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

// Application Bootstrap
window.addEventListener("DOMContentLoaded", () => {
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
    document.getElementById("cert-hash").value = certFromUrl;
  }

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

  // UI Event Bindings
  ui.initEvents({
    onConnect: async (url, certHash) => {
      try {
        await transport.connect(url, certHash);
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
  });

  // Action Cast Hook
  controls.onCast(() => {
    if (!transport.connected) return;
    // Cast packet: Magic [0x4D, 0x4D], Opcode 0x0003 (Interaction), Source 1, Target 0, Type 1 (Attack)
    const buf = new Uint8Array(32);
    buf[0] = 0x4d;
    buf[1] = 0x4d;
    buf[2] = 0x03; // Opcode Interaction
    buf[3] = 0x00;
    buf[4] = 0x18; // Length 24
    buf[5] = 0x00;
    buf[6] = 0x00;
    buf[7] = 0x00;
    const view = new DataView(buf.buffer);
    view.setBigUint64(8, 1n, true); // Source GUID
    view.setBigUint64(16, 0n, true); // Target GUID
    buf[24] = 0x01; // Attack
    view.setUint32(25, 42, true); // Action ID
    transport.sendStream(buf);
  });

  // Fixed 20 Hz movement update interval (50ms)
  let lastMoveSendTime = 0;
  const MOVE_INTERVAL_MS = 50;

  let lastFrameTime = performance.now();

  function animate(now) {
    requestAnimationFrame(animate);

    const dt = Math.min((now - lastFrameTime) / 1000, 0.1);
    lastFrameTime = now;

    // 1. Advance controls and world interpolation
    controls.update(dt);
    world.update(dt);

    // 2. Transmit 20 Hz movement updates over WebTransport datagrams
    if (now - lastMoveSendTime >= MOVE_INTERVAL_MS) {
      lastMoveSendTime = now;
      if (transport.connected) {
        const sample = controls.sample();

        // MovementPacket: Magic 0x4D, 0x4D, Opcode 0x0001
        // Length: 40 bytes payload
        const moveBuf = new Uint8Array(48);
        moveBuf[0] = 0x4d;
        moveBuf[1] = 0x4d;
        moveBuf[2] = 0x01; // Opcode Movement
        moveBuf[3] = 0x00;
        moveBuf[4] = 0x28; // Payload length 40
        moveBuf[5] = 0x00;
        moveBuf[6] = 0x00;
        moveBuf[7] = 0x00;

        const view = new DataView(moveBuf.buffer);
        view.setBigUint64(8, 1n, true); // Entity ID
        view.setFloat32(16, sample.x, true);
        view.setFloat32(20, sample.y, true);
        view.setFloat32(24, sample.z, true);
        view.setFloat32(28, 0.0, true); // vx
        view.setFloat32(32, 0.0, true); // vy
        view.setFloat32(36, 0.0, true); // vz
        view.setFloat32(40, sample.yaw, true);
        view.setFloat32(44, 0.0, true); // pitch

        transport.sendDatagram(moveBuf);
      }
    }

    // 3. Render synchronized 3D scene
    const renderBuffer = world.getRenderBuffer(controls.position, controls.yaw);
    renderer.syncEntities(renderBuffer, world.localPlayerGuid);
    renderer.updateWorldObjects(world.getWorldObjectsRenderBuffer());
    renderer.updateCamera(controls.position);
    renderer.render();

    // 4. Update HUD Telemetry
    ui.updateTelemetry(
      world.getTelemetryJson(),
      transport.pingMs,
      transport.bandwidthBps,
      controls.position
    );
  }

  requestAnimationFrame(animate);
});
