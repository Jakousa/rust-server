import { describe, it, expect } from "vitest";
import * as THREE from "three";
import { PlayerControls } from "../controls.js";
import { WorldObjectRenderer } from "../world.js";

describe("PlayerControls", () => {
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
    const lo = 3735928559; // 0xDEADBEEF
    const hi = 305419896;  // 0x12345678
    buf[0] = lo;
    buf[1] = 42; // prefabId
    buf[2] = 10.0; // x
    buf[3] = 0.0;  // y
    buf[4] = 20.0; // z
    buf[5] = 0.0;  // qx
    buf[6] = 0.0;  // qy
    buf[7] = 0.0;  // qz
    buf[8] = 1.0;  // qw
    buf[9] = 1.0;  // sx
    buf[10] = 1.0; // sy
    buf[11] = 1.0; // sz
    buf[12] = 16;  // stateFlags (DAMAGED)
    buf[13] = 0.5; // hpPct
    buf[14] = 1;   // revision
    buf[15] = hi;

    renderer.updateFromBuffer(buf);

    const expectedId = (BigInt(Math.round(buf[15])) << 32n) | BigInt(Math.round(buf[0]));
    expect(renderer.objects.has(expectedId)).toBe(true);

    const group = renderer.objects.get(expectedId);
    expect(group.userData.mesh.material).toBe(renderer.materials.damaged);
    expect(group.userData.hpBar.scale.x).toBeCloseTo(0.5, 5);

    // Empty buffer clears / despawns all objects
    renderer.updateFromBuffer(new Float32Array(0));
    expect(renderer.objects.size).toBe(0);
  });
});
