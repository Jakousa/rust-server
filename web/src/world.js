import * as THREE from "three";
import { disposeHierarchy } from "./renderer.js";

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
      // Village PoC Structures (Prefabs 10..=13)
      well: new THREE.MeshStandardMaterial({
        color: 0x0284c7, // Deep Water Cyan / Stone
        roughness: 0.4,
        metalness: 0.2,
      }),
      forge: new THREE.MeshStandardMaterial({
        color: 0xea580c, // Hearth Flame Orange / Iron
        roughness: 0.6,
        metalness: 0.5,
      }),
      stall: new THREE.MeshStandardMaterial({
        color: 0x16a34a, // Herbal Green / Timber
        roughness: 0.5,
        metalness: 0.1,
      }),
      house: new THREE.MeshStandardMaterial({
        color: 0x78350f, // Warm Timber / Thatch
        roughness: 0.7,
        metalness: 0.1,
      }),
    };
  }

  /**
   * Updates world objects from a flat Float32Array buffer (16 floats per object).
   * Memory layout:
   * [objectId_lo, prefabId, x, y, z, qx, qy, qz, qw, sx, sy, sz, stateFlags, hpPct, revision, objectId_hi]
   */
  updateFromBuffer(buffer) {
    if (!buffer || buffer.length === 0) {
      this.clear();
      return;
    }

    const STRIDE = 16;
    const count = Math.floor(buffer.length / STRIDE);
    const activeIds = (this._activeIds ??= new Set());
    activeIds.clear();
    const uint32View = new Uint32Array(buffer.buffer, buffer.byteOffset, buffer.length);

    for (let i = 0; i < count; i++) {
      const offset = i * STRIDE;
      const lo = BigInt(uint32View[offset]);
      const hi = BigInt(uint32View[offset + 15]);
      const objectId = (hi << 32n) | lo;
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
          : this.getMaterialForPrefab(prefabId);
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

    // Remove despawned object meshes and release GPU memory
    for (const [id, group] of this.objects.entries()) {
      if (!activeIds.has(id)) {
        this.scene.remove(group);
        disposeHierarchy(group);
        this.objects.delete(id);
      }
    }
  }

  getMaterialForPrefab(prefabId) {
    switch (prefabId) {
      case 10:
        return this.materials.well;
      case 11:
        return this.materials.forge;
      case 12:
        return this.materials.stall;
      case 13:
        return this.materials.house;
      default:
        return this.materials.defaultStructure;
    }
  }

  createObjectMesh(prefabId, sx, sy, sz) {
    const group = new THREE.Group();
    const mat = this.getMaterialForPrefab(prefabId);

    let mainGeo;
    if (prefabId === 10) {
      // Well: stone circular base
      mainGeo = new THREE.CylinderGeometry(0.5, 0.55, 0.6, 16);
    } else if (prefabId === 11) {
      // Forge: stone masonry hearth block
      mainGeo = new THREE.BoxGeometry(1.0, 0.6, 1.0);
    } else if (prefabId === 12) {
      // Stall: market counter table
      mainGeo = new THREE.BoxGeometry(1.0, 0.5, 0.7);
    } else if (prefabId === 13) {
      // House: log/stone cabin body
      mainGeo = new THREE.BoxGeometry(1.0, 0.7, 0.9);
    } else {
      mainGeo = new THREE.BoxGeometry(1.0, 1.0, 1.0);
    }

    const mesh = new THREE.Mesh(mainGeo, mat);
    mesh.position.y = (prefabId === 10 || prefabId === 11 || prefabId === 12 || prefabId === 13) ? 0.35 : 0.5;
    mesh.castShadow = true;
    mesh.receiveShadow = true;
    group.add(mesh);
    group.userData.mesh = mesh;

    // Prefab-specific structural composite additions
    if (prefabId === 10) {
      // Well: 2 wooden support posts + pitched roof canopy + rope/water
      const timberMat = new THREE.MeshStandardMaterial({ color: 0x78350f, roughness: 0.8 });
      const postGeo = new THREE.CylinderGeometry(0.04, 0.04, 0.8, 8);
      const postL = new THREE.Mesh(postGeo, timberMat);
      postL.position.set(-0.35, 0.7, 0);
      group.add(postL);

      const postR = new THREE.Mesh(postGeo, timberMat);
      postR.position.set(0.35, 0.7, 0);
      group.add(postR);

      // Peaked roof canopy
      const roofGeo = new THREE.ConeGeometry(0.55, 0.35, 4);
      const roofMat = new THREE.MeshStandardMaterial({ color: 0x92400e, roughness: 0.7 });
      const roof = new THREE.Mesh(roofGeo, roofMat);
      roof.position.set(0, 1.2, 0);
      roof.rotation.y = Math.PI / 4;
      group.add(roof);

      // Water surface inside
      const waterGeo = new THREE.CircleGeometry(0.42, 16);
      const waterMat = new THREE.MeshStandardMaterial({ color: 0x0ea5e9, roughness: 0.1, metalness: 0.8 });
      const water = new THREE.Mesh(waterGeo, waterMat);
      water.rotation.x = -Math.PI / 2;
      water.position.set(0, 0.45, 0);
      group.add(water);
    } else if (prefabId === 11) {
      // Forge: tall chimney + glowing hearth basin + anvil
      const stoneMat = new THREE.MeshStandardMaterial({ color: 0x334155, roughness: 0.9 });
      const chimneyGeo = new THREE.BoxGeometry(0.3, 0.8, 0.3);
      const chimney = new THREE.Mesh(chimneyGeo, stoneMat);
      chimney.position.set(0.3, 0.9, -0.3);
      group.add(chimney);

      // Glowing coal basin
      const coalGeo = new THREE.BoxGeometry(0.45, 0.15, 0.45);
      const coalMat = new THREE.MeshStandardMaterial({
        color: 0xea580c,
        emissive: 0xf97316,
        emissiveIntensity: 0.8,
        roughness: 0.5,
      });
      const coals = new THREE.Mesh(coalGeo, coalMat);
      coals.position.set(-0.15, 0.68, 0.05);
      group.add(coals);

      // Anvil block
      const ironMat = new THREE.MeshStandardMaterial({ color: 0x1e293b, metalness: 0.8, roughness: 0.3 });
      const anvilGeo = new THREE.BoxGeometry(0.22, 0.22, 0.35);
      const anvil = new THREE.Mesh(anvilGeo, ironMat);
      anvil.position.set(-0.45, 0.2, 0.45);
      group.add(anvil);
    } else if (prefabId === 12) {
      // Stall: wooden poles + striped awning canopy + produce crates
      const timberMat = new THREE.MeshStandardMaterial({ color: 0x854d0e, roughness: 0.8 });
      const poleGeo = new THREE.CylinderGeometry(0.03, 0.03, 0.8, 8);
      [[-0.42, 0.25], [0.42, 0.25], [-0.42, -0.25], [0.42, -0.25]].forEach(([px, pz]) => {
        const pole = new THREE.Mesh(poleGeo, timberMat);
        pole.position.set(px, 0.75, pz);
        group.add(pole);
      });

      // Striped awning
      const awningGeo = new THREE.BoxGeometry(1.1, 0.08, 0.8);
      const awningMat = new THREE.MeshStandardMaterial({ color: 0x15803d, roughness: 0.5 });
      const awning = new THREE.Mesh(awningGeo, awningMat);
      awning.position.set(0, 1.15, 0);
      awning.rotation.x = 0.15;
      group.add(awning);

      // Display crates on counter
      const crateGeo = new THREE.BoxGeometry(0.25, 0.15, 0.2);
      const crateMat = new THREE.MeshStandardMaterial({ color: 0xa16207, roughness: 0.7 });
      const crate1 = new THREE.Mesh(crateGeo, crateMat);
      crate1.position.set(-0.25, 0.65, 0);
      group.add(crate1);

      const crate2 = new THREE.Mesh(crateGeo, crateMat);
      crate2.position.set(0.2, 0.65, 0);
      group.add(crate2);
    } else if (prefabId === 13) {
      // House: pitched roof with eaves + stone chimney + timber door
      const roofGeo = new THREE.ConeGeometry(0.85, 0.55, 4);
      const roofMat = new THREE.MeshStandardMaterial({ color: 0x9a3412, roughness: 0.7 });
      const roof = new THREE.Mesh(roofGeo, roofMat);
      roof.position.set(0, 1.0, 0);
      roof.rotation.y = Math.PI / 4;
      group.add(roof);

      // Chimney
      const chimneyGeo = new THREE.BoxGeometry(0.18, 0.6, 0.18);
      const stoneMat = new THREE.MeshStandardMaterial({ color: 0x64748b, roughness: 0.9 });
      const chimney = new THREE.Mesh(chimneyGeo, stoneMat);
      chimney.position.set(0.32, 1.2, -0.25);
      group.add(chimney);

      // Door
      const doorGeo = new THREE.BoxGeometry(0.24, 0.45, 0.06);
      const doorMat = new THREE.MeshStandardMaterial({ color: 0x451a03, roughness: 0.9 });
      const door = new THREE.Mesh(doorGeo, doorMat);
      door.position.set(0, 0.35, 0.46);
      group.add(door);
    }

    // Overhead health bar for destructible objects
    const barGeo = new THREE.BoxGeometry(1.0, 0.08, 0.04);
    const barMat = new THREE.MeshBasicMaterial({ color: 0x10b981 });
    const hpBar = new THREE.Mesh(barGeo, barMat);
    hpBar.position.y = 1.4;
    hpBar.visible = false;
    group.add(hpBar);
    group.userData.hpBar = hpBar;

    return group;
  }

  clear() {
    for (const group of this.objects.values()) {
      this.scene.remove(group);
      disposeHierarchy(group);
    }
    this.objects.clear();
  }
}
