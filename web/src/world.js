import * as THREE from "three";

/**
 * Three.js World Object Renderer for OpenSpec MMO.
 * Decodes flat 16-float WASM buffers and maintains 3D meshes for persistent objects and prefabs.
 */
export class WorldObjectRenderer {
  constructor(scene) {
    this.scene = scene;
    this.objects = new Map(); // objectId -> THREE.Group
    this.materials = this.initMaterials();
  }

  initMaterials() {
    return {
      defaultStructure: new THREE.MeshStandardMaterial({
        color: 0xd97706, // Amber/Gold
        roughness: 0.6,
        metalness: 0.2,
      }),
      chest: new THREE.MeshStandardMaterial({
        color: 0xb45309,
        roughness: 0.4,
        metalness: 0.4,
      }),
      resourceNode: new THREE.MeshStandardMaterial({
        color: 0x059669, // Emerald
        roughness: 0.8,
        metalness: 0.1,
      }),
      damaged: new THREE.MeshStandardMaterial({
        color: 0xb91c1c, // Crimson
        roughness: 0.7,
        metalness: 0.1,
      }),
    };
  }

  /**
   * Updates world objects from a flat Float32Array buffer (16 floats per object).
   * Memory layout:
   * [objectId, prefabId, x, y, z, qx, qy, qz, qw, sx, sy, sz, stateFlags, hpPct, revision, padding]
   */
  updateFromBuffer(buffer) {
    if (!buffer || buffer.length === 0) {
      this.clear();
      return;
    }

    const STRIDE = 16;
    const count = Math.floor(buffer.length / STRIDE);
    const activeIds = new Set();

    for (let i = 0; i < count; i++) {
      const offset = i * STRIDE;
      const objectId = Number(buffer[offset]);
      const prefabId = Number(buffer[offset + 1]);
      const x = buffer[offset + 2];
      const y = buffer[offset + 3];
      const z = buffer[offset + 4];
      const qx = buffer[offset + 5];
      const qy = buffer[offset + 6];
      const qz = buffer[offset + 7];
      const qw = buffer[offset + 8];
      const sx = buffer[offset + 9];
      const sy = buffer[offset + 10];
      const sz = buffer[offset + 11];
      const stateFlags = Number(buffer[offset + 12]);
      const hpPct = buffer[offset + 13];

      activeIds.add(objectId);

      let group = this.objects.get(objectId);
      if (!group) {
        group = this.createObjectMesh(prefabId, sx, sy, sz);
        this.objects.set(objectId, group);
        this.scene.add(group);
      }

      // Update transform
      group.position.set(x, y, z);
      group.quaternion.set(qx, qy, qz, qw);
      group.scale.set(sx, sy, sz);

      // Update state appearance
      const isDamaged = (stateFlags & 16) !== 0; // DAMAGED flag
      if (group.userData.mesh) {
        group.userData.mesh.material = isDamaged
          ? this.materials.damaged
          : this.materials.defaultStructure;
      }

      // Update health bar if present
      if (group.userData.hpBar) {
        if (hpPct >= 0.0) {
          group.userData.hpBar.visible = true;
          group.userData.hpBar.scale.x = Math.max(0.01, hpPct);
        } else {
          group.userData.hpBar.visible = false;
        }
      }
    }

    // Remove despawned object meshes
    for (const [id, group] of this.objects.entries()) {
      if (!activeIds.has(id)) {
        this.scene.remove(group);
        this.objects.delete(id);
      }
    }
  }

  createObjectMesh(prefabId, sx, sy, sz) {
    const group = new THREE.Group();

    // Base geometry: Unit cube scaled by transform
    const geo = new THREE.BoxGeometry(1.0, 1.0, 1.0);
    const mesh = new THREE.Mesh(geo, this.materials.defaultStructure);
    mesh.castShadow = true;
    mesh.receiveShadow = true;
    group.add(mesh);
    group.userData.mesh = mesh;

    // Overhead health bar for destructible objects
    const barGeo = new THREE.BoxGeometry(1.0, 0.08, 0.04);
    const barMat = new THREE.MeshBasicMaterial({ color: 0x10b981 });
    const hpBar = new THREE.Mesh(barGeo, barMat);
    hpBar.position.y = 0.8;
    hpBar.visible = false;
    group.add(hpBar);
    group.userData.hpBar = hpBar;

    return group;
  }

  clear() {
    for (const group of this.objects.values()) {
      this.scene.remove(group);
    }
    this.objects.clear();
  }
}
