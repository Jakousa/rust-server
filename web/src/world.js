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
      // Village Structures (Prefabs 10..=13)
      well: new THREE.MeshStandardMaterial({
        color: 0x78716c, // Weathered stone masonry
        roughness: 0.88,
        metalness: 0.08,
      }),
      forge: new THREE.MeshStandardMaterial({
        color: 0x475569, // Dark hearth masonry stone
        roughness: 0.9,
        metalness: 0.12,
      }),
      stall: new THREE.MeshStandardMaterial({
        color: 0x854d0e, // Seasoned oak timber counter
        roughness: 0.78,
        metalness: 0.08,
      }),
      house: new THREE.MeshStandardMaterial({
        color: 0xe7dcc8, // Warm wattle-and-daub plaster
        roughness: 0.85,
        metalness: 0.05,
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

  createObjectMesh(prefabId, _sx, _sy, _sz) {
    const group = new THREE.Group();
    const mat = this.getMaterialForPrefab(prefabId);

    let hpBarY = 1.45;
    if (prefabId === 10) {
      hpBarY = this._buildWell(group, mat);
    } else if (prefabId === 11) {
      hpBarY = this._buildForge(group, mat);
    } else if (prefabId === 12) {
      hpBarY = this._buildMarketStall(group, mat);
    } else if (prefabId === 13) {
      hpBarY = this._buildHouse(group, mat);
    } else {
      const mainGeo = new THREE.BoxGeometry(1.0, 1.0, 1.0);
      const mesh = new THREE.Mesh(mainGeo, mat);
      mesh.position.y = 0.5;
      mesh.castShadow = true;
      mesh.receiveShadow = true;
      group.add(mesh);
      group.userData.mesh = mesh;
    }

    // Overhead health bar for destructible objects
    const barGeo = new THREE.BoxGeometry(1.0, 0.08, 0.04);
    const barMat = new THREE.MeshBasicMaterial({ color: 0x10b981 });
    const hpBar = new THREE.Mesh(barGeo, barMat);
    hpBar.position.y = hpBarY;
    hpBar.visible = false;
    group.add(hpBar);
    group.userData.hpBar = hpBar;

    return group;
  }

  /**
   * Prefab 10: Village Well — masonry stone rim, crossbeam winch with rope & bucket,
   * shingled roof eaves, and shimmering water disc.
   */
  _buildWell(group, mainMat) {
    const baseStoneMat = new THREE.MeshStandardMaterial({ color: 0x57534e, roughness: 0.9 });
    const copingMat = new THREE.MeshStandardMaterial({ color: 0xa8a29e, roughness: 0.8 });
    const timberMat = new THREE.MeshStandardMaterial({ color: 0x6b3e1b, roughness: 0.82 });
    const shingleMat = new THREE.MeshStandardMaterial({ color: 0x9a3412, roughness: 0.72 });
    const ropeMat = new THREE.MeshStandardMaterial({ color: 0xd6b88d, roughness: 0.9 });
    const ironMat = new THREE.MeshStandardMaterial({ color: 0x334155, metalness: 0.8, roughness: 0.35 });
    const waterMat = new THREE.MeshStandardMaterial({
      color: 0x0284c7,
      emissive: 0x0369a1,
      emissiveIntensity: 0.3,
      roughness: 0.1,
      metalness: 0.85,
    });

    // Primary circular masonry body
    const mainMesh = new THREE.Mesh(new THREE.CylinderGeometry(0.45, 0.49, 0.50, 16), mainMat);
    mainMesh.position.y = 0.25;
    mainMesh.castShadow = true;
    mainMesh.receiveShadow = true;
    group.add(mainMesh);
    group.userData.mesh = mainMesh;

    // Stepped stone foundation plinth
    const plinth = new THREE.Mesh(new THREE.CylinderGeometry(0.57, 0.61, 0.07, 16), baseStoneMat);
    plinth.position.y = 0.035;
    plinth.receiveShadow = true;
    group.add(plinth);

    // Dressed stone coping rim + segmented masonry stones
    const rim = new THREE.Mesh(new THREE.TorusGeometry(0.43, 0.065, 8, 20), copingMat);
    rim.rotation.x = Math.PI / 2;
    rim.position.y = 0.50;
    rim.castShadow = true;
    group.add(rim);

    for (let i = 0; i < 8; i++) {
      const a = (i / 8) * Math.PI * 2;
      const capBlock = new THREE.Mesh(new THREE.BoxGeometry(0.14, 0.05, 0.12), copingMat);
      capBlock.position.set(Math.cos(a) * 0.43, 0.535, Math.sin(a) * 0.43);
      capBlock.rotation.y = -a;
      capBlock.castShadow = true;
      group.add(capBlock);
    }

    // Shimmering water disc + inner ripple highlight
    const water = new THREE.Mesh(new THREE.CircleGeometry(0.38, 20), waterMat);
    water.rotation.x = -Math.PI / 2;
    water.position.y = 0.43;
    group.add(water);

    const rippleMat = new THREE.MeshBasicMaterial({
      color: 0x7dd3fc,
      transparent: true,
      opacity: 0.38,
      side: THREE.DoubleSide,
    });
    const ripple = new THREE.Mesh(new THREE.RingGeometry(0.16, 0.21, 18), rippleMat);
    ripple.rotation.x = -Math.PI / 2;
    ripple.position.y = 0.435;
    group.add(ripple);

    // Timber upright posts + diagonal eaves braces
    const postGeo = new THREE.BoxGeometry(0.075, 0.92, 0.075);
    const braceGeo = new THREE.BoxGeometry(0.045, 0.24, 0.045);
    for (const side of [-1, 1]) {
      const post = new THREE.Mesh(postGeo, timberMat);
      post.position.set(side * 0.38, 0.88, 0);
      post.castShadow = true;
      group.add(post);

      const brace = new THREE.Mesh(braceGeo, timberMat);
      brace.position.set(side * 0.30, 1.24, 0);
      brace.rotation.z = side * -0.7;
      group.add(brace);
    }

    // Crossbeam winch roller + iron crank + coiled hemp rope + wooden bucket
    const winch = new THREE.Mesh(new THREE.CylinderGeometry(0.04, 0.04, 0.78, 10), timberMat);
    winch.rotation.z = Math.PI / 2;
    winch.position.set(0, 0.95, 0);
    winch.castShadow = true;
    group.add(winch);

    const crankArm = new THREE.Mesh(new THREE.BoxGeometry(0.03, 0.14, 0.03), ironMat);
    crankArm.position.set(0.43, 0.90, 0.04);
    group.add(crankArm);

    const crankHandle = new THREE.Mesh(new THREE.CylinderGeometry(0.018, 0.018, 0.10, 6), timberMat);
    crankHandle.rotation.z = Math.PI / 2;
    crankHandle.position.set(0.48, 0.84, 0.04);
    group.add(crankHandle);

    const ropeCoil = new THREE.Mesh(new THREE.CylinderGeometry(0.056, 0.056, 0.18, 10), ropeMat);
    ropeCoil.rotation.z = Math.PI / 2;
    ropeCoil.position.set(0, 0.95, 0);
    group.add(ropeCoil);

    const ropeLine = new THREE.Mesh(new THREE.CylinderGeometry(0.012, 0.012, 0.32, 6), ropeMat);
    ropeLine.position.set(0, 0.77, 0);
    group.add(ropeLine);

    const bucket = new THREE.Mesh(new THREE.CylinderGeometry(0.105, 0.08, 0.16, 10), timberMat);
    bucket.position.set(0, 0.58, 0);
    bucket.castShadow = true;
    group.add(bucket);

    const bucketBand = new THREE.Mesh(new THREE.CylinderGeometry(0.10, 0.095, 0.025, 10), ironMat);
    bucketBand.position.set(0, 0.62, 0);
    group.add(bucketBand);

    // Gabled shingled roof canopy with eaves & ridge beam
    const roofSlopeGeo = new THREE.BoxGeometry(0.64, 0.045, 0.96);
    for (const side of [-1, 1]) {
      const slope = new THREE.Mesh(roofSlopeGeo, shingleMat);
      slope.position.set(side * 0.25, 1.44, 0);
      slope.rotation.z = side * -0.56;
      slope.castShadow = true;
      group.add(slope);

      // Eave trim fascia board
      const eave = new THREE.Mesh(new THREE.BoxGeometry(0.04, 0.06, 0.98), timberMat);
      eave.position.set(side * 0.51, 1.30, 0);
      group.add(eave);
    }

    const ridge = new THREE.Mesh(new THREE.BoxGeometry(0.075, 0.075, 1.00), timberMat);
    ridge.position.set(0, 1.59, 0);
    ridge.castShadow = true;
    group.add(ridge);

    return 1.88;
  }

  /**
   * Prefab 11: Blacksmith Forge — stone hearth arch, glowing ember core + warm point light,
   * chimney stack, iron anvil on timber block, and weapon rack accent.
   */
  _buildForge(group, mainMat) {
    const darkStoneMat = new THREE.MeshStandardMaterial({ color: 0x334155, roughness: 0.92 });
    const timberMat = new THREE.MeshStandardMaterial({ color: 0x5c3a1e, roughness: 0.85 });
    const ironMat = new THREE.MeshStandardMaterial({ color: 0x1e293b, metalness: 0.85, roughness: 0.28 });
    const steelMat = new THREE.MeshStandardMaterial({ color: 0xcbd5e1, metalness: 0.9, roughness: 0.2 });
    const emberMat = new THREE.MeshStandardMaterial({
      color: 0xff6b00,
      emissive: 0xff4500,
      emissiveIntensity: 1.75,
      roughness: 0.4,
    });
    const leatherMat = new THREE.MeshStandardMaterial({ color: 0x7c2d12, roughness: 0.8 });

    // Primary stone hearth platform
    const mainMesh = new THREE.Mesh(new THREE.BoxGeometry(0.84, 0.44, 0.74), mainMat);
    mainMesh.position.set(-0.06, 0.22, -0.08);
    mainMesh.castShadow = true;
    mainMesh.receiveShadow = true;
    group.add(mainMesh);
    group.userData.mesh = mainMesh;

    // Stone floor flagstone apron
    const floorApron = new THREE.Mesh(new THREE.BoxGeometry(1.06, 0.04, 1.06), darkStoneMat);
    floorApron.position.set(0, 0.02, 0.02);
    floorApron.receiveShadow = true;
    group.add(floorApron);

    // Hearth back wall and arched side pillars
    const backWall = new THREE.Mesh(new THREE.BoxGeometry(0.76, 0.64, 0.20), darkStoneMat);
    backWall.position.set(-0.08, 0.62, -0.34);
    backWall.castShadow = true;
    group.add(backWall);

    for (const side of [-1, 1]) {
      const pillar = new THREE.Mesh(new THREE.BoxGeometry(0.16, 0.44, 0.34), darkStoneMat);
      pillar.position.set(-0.08 + side * 0.27, 0.62, -0.11);
      pillar.castShadow = true;
      group.add(pillar);
    }

    // Hearth arch hood + keystone lintel
    const hood = new THREE.Mesh(new THREE.BoxGeometry(0.72, 0.16, 0.38), darkStoneMat);
    hood.position.set(-0.08, 0.88, -0.12);
    hood.castShadow = true;
    group.add(hood);

    const archKeystone = new THREE.Mesh(new THREE.BoxGeometry(0.14, 0.14, 0.08), mainMat);
    archKeystone.position.set(-0.08, 0.86, 0.08);
    group.add(archKeystone);

    // Tapered stone chimney stack + crown collar
    const chimney = new THREE.Mesh(
      new THREE.CylinderGeometry(0.14, 0.23, 0.68, 4),
      darkStoneMat
    );
    chimney.rotation.y = Math.PI / 4;
    chimney.position.set(-0.08, 1.26, -0.22);
    chimney.castShadow = true;
    group.add(chimney);

    const chimneyCap = new THREE.Mesh(new THREE.BoxGeometry(0.28, 0.07, 0.28), darkStoneMat);
    chimneyCap.position.set(-0.08, 1.60, -0.22);
    chimneyCap.castShadow = true;
    group.add(chimneyCap);

    // Glowing ember core + coal chunks + warm hearth point light
    const emberBed = new THREE.Mesh(new THREE.BoxGeometry(0.36, 0.09, 0.28), emberMat);
    emberBed.position.set(-0.08, 0.47, -0.10);
    group.add(emberBed);

    const coalOffsets = [
      [-0.14, 0.52, -0.08],
      [-0.02, 0.53, -0.12],
      [-0.08, 0.54, -0.04],
    ];
    for (const [cx, cy, cz] of coalOffsets) {
      const coal = new THREE.Mesh(new THREE.DodecahedronGeometry(0.06, 0), emberMat);
      coal.position.set(cx, cy, cz);
      group.add(coal);
    }

    const hearthLight = new THREE.PointLight(0xff7b29, 2.2, 16, 1.5);
    hearthLight.position.set(-0.08, 0.64, 0.08);
    group.add(hearthLight);

    // Bellows beside hearth
    const bellows = new THREE.Mesh(new THREE.CylinderGeometry(0.08, 0.12, 0.22, 6), leatherMat);
    bellows.rotation.z = Math.PI / 2.4;
    bellows.position.set(-0.52, 0.52, -0.12);
    group.add(bellows);

    // Iron anvil mounted on oak timber block (front right)
    const stump = new THREE.Mesh(new THREE.CylinderGeometry(0.135, 0.155, 0.28, 10), timberMat);
    stump.position.set(0.28, 0.16, 0.26);
    stump.castShadow = true;
    group.add(stump);

    const anvilBase = new THREE.Mesh(new THREE.BoxGeometry(0.17, 0.06, 0.12), ironMat);
    anvilBase.position.set(0.28, 0.33, 0.26);
    group.add(anvilBase);

    const anvilWaist = new THREE.Mesh(new THREE.BoxGeometry(0.11, 0.06, 0.09), ironMat);
    anvilWaist.position.set(0.28, 0.38, 0.26);
    group.add(anvilWaist);

    const anvilTop = new THREE.Mesh(new THREE.BoxGeometry(0.22, 0.075, 0.12), ironMat);
    anvilTop.position.set(0.28, 0.43, 0.26);
    anvilTop.castShadow = true;
    group.add(anvilTop);

    const anvilHorn = new THREE.Mesh(new THREE.ConeGeometry(0.05, 0.14, 8), ironMat);
    anvilHorn.rotation.z = -Math.PI / 2;
    anvilHorn.position.set(0.43, 0.43, 0.26);
    group.add(anvilHorn);

    // Quench barrel beside hearth
    const barrel = new THREE.Mesh(new THREE.CylinderGeometry(0.11, 0.10, 0.24, 10), timberMat);
    barrel.position.set(-0.46, 0.14, 0.16);
    barrel.castShadow = true;
    group.add(barrel);

    const quenchWater = new THREE.Mesh(
      new THREE.CircleGeometry(0.095, 10),
      new THREE.MeshStandardMaterial({ color: 0x0ea5e9, roughness: 0.15, metalness: 0.7 })
    );
    quenchWater.rotation.x = -Math.PI / 2;
    quenchWater.position.set(-0.46, 0.265, 0.16);
    group.add(quenchWater);

    // Weapon rack accent (front left)
    const rackGroup = new THREE.Group();
    rackGroup.position.set(-0.30, 0.04, 0.38);
    const uprightGeo = new THREE.BoxGeometry(0.032, 0.38, 0.032);
    for (const rx of [-0.14, 0.14]) {
      const upright = new THREE.Mesh(uprightGeo, timberMat);
      upright.position.set(rx, 0.19, 0);
      upright.castShadow = true;
      rackGroup.add(upright);
    }
    const rackBar = new THREE.Mesh(new THREE.BoxGeometry(0.34, 0.03, 0.03), timberMat);
    rackBar.position.set(0, 0.31, 0);
    rackGroup.add(rackBar);

    for (const wx of [-0.07, 0.03, 0.10]) {
      const blade = new THREE.Mesh(new THREE.BoxGeometry(0.024, 0.34, 0.01), steelMat);
      blade.position.set(wx, 0.22, 0.04);
      blade.rotation.x = -0.22;
      rackGroup.add(blade);

      const guard = new THREE.Mesh(new THREE.BoxGeometry(0.075, 0.016, 0.02), ironMat);
      guard.position.set(wx, 0.34, 0.015);
      rackGroup.add(guard);
    }
    group.add(rackGroup);

    return 1.85;
  }

  /**
   * Prefab 12: Herbalist Market Stall — dual-color striped awning, timber counter,
   * colorful potion flasks, herb baskets, and coin scale props.
   */
  _buildMarketStall(group, mainMat) {
    const counterTopMat = new THREE.MeshStandardMaterial({ color: 0xa16207, roughness: 0.72 });
    const postMat = new THREE.MeshStandardMaterial({ color: 0x6b3e1b, roughness: 0.84 });
    const awningGreenMat = new THREE.MeshStandardMaterial({ color: 0x15803d, roughness: 0.58 });
    const awningCreamMat = new THREE.MeshStandardMaterial({ color: 0xfef3c7, roughness: 0.62 });
    const basketMat = new THREE.MeshStandardMaterial({ color: 0xd97706, roughness: 0.86 });
    const brassMat = new THREE.MeshStandardMaterial({ color: 0xf59e0b, metalness: 0.85, roughness: 0.22 });
    const corkMat = new THREE.MeshStandardMaterial({ color: 0x92400e, roughness: 0.9 });

    // Primary timber counter base
    const mainMesh = new THREE.Mesh(new THREE.BoxGeometry(0.92, 0.42, 0.56), mainMat);
    mainMesh.position.set(0, 0.21, 0);
    mainMesh.castShadow = true;
    mainMesh.receiveShadow = true;
    group.add(mainMesh);
    group.userData.mesh = mainMesh;

    // Overhanging wooden countertop plank
    const counterTop = new THREE.Mesh(new THREE.BoxGeometry(1.00, 0.055, 0.64), counterTopMat);
    counterTop.position.set(0, 0.445, 0.01);
    counterTop.castShadow = true;
    counterTop.receiveShadow = true;
    group.add(counterTop);

    // 4 Corner awning posts
    const postGeo = new THREE.CylinderGeometry(0.032, 0.036, 1.06, 8);
    const posts = [
      [-0.44, 0.53, 0.25],
      [0.44, 0.53, 0.25],
      [-0.44, 0.58, -0.25],
      [0.44, 0.58, -0.25],
    ];
    for (const [px, py, pz] of posts) {
      const post = new THREE.Mesh(postGeo, postMat);
      post.position.set(px, py, pz);
      post.castShadow = true;
      group.add(post);
    }

    // Dual-color striped awning canopy (6 alternating emerald & cream fabric panels)
    const awningGroup = new THREE.Group();
    awningGroup.position.set(0, 1.09, 0.01);
    awningGroup.rotation.x = 0.18;
    const stripeCount = 6;
    const stripeW = 1.14 / stripeCount;
    for (let i = 0; i < stripeCount; i++) {
      const sx = -0.57 + stripeW * (i + 0.5);
      const stripeMat = i % 2 === 0 ? awningGreenMat : awningCreamMat;
      const panel = new THREE.Mesh(new THREE.BoxGeometry(stripeW, 0.05, 0.80), stripeMat);
      panel.position.set(sx, 0, 0);
      panel.castShadow = true;
      awningGroup.add(panel);

      // Scalloped front awning valance flap
      const flap = new THREE.Mesh(new THREE.BoxGeometry(stripeW * 0.94, 0.11, 0.03), stripeMat);
      flap.position.set(sx, -0.055, 0.39);
      awningGroup.add(flap);
    }
    group.add(awningGroup);

    // Colorful glass potion flasks on counter left-center
    const potions = [
      { x: -0.34, z: 0.12, r: 0.048, h: 0.075, color: 0xef4444, emissive: 0xb91c1c },
      { x: -0.24, z: 0.06, r: 0.042, h: 0.09, color: 0x10b981, emissive: 0x047857 },
      { x: -0.15, z: 0.14, r: 0.045, h: 0.065, color: 0x3b82f6, emissive: 0x1d4ed8 },
      { x: -0.28, z: -0.10, r: 0.038, h: 0.08, color: 0xf59e0b, emissive: 0xb45309 },
    ];
    for (const p of potions) {
      const glassMat = new THREE.MeshStandardMaterial({
        color: p.color,
        emissive: p.emissive,
        emissiveIntensity: 0.45,
        roughness: 0.15,
        metalness: 0.25,
      });
      const bulb = new THREE.Mesh(new THREE.SphereGeometry(p.r, 10, 8), glassMat);
      bulb.position.set(p.x, 0.475 + p.r, p.z);
      group.add(bulb);

      const neck = new THREE.Mesh(new THREE.CylinderGeometry(0.016, 0.02, p.h * 0.55, 8), glassMat);
      neck.position.set(p.x, 0.475 + p.r * 1.8 + p.h * 0.25, p.z);
      group.add(neck);

      const cork = new THREE.Mesh(new THREE.CylinderGeometry(0.015, 0.013, 0.022, 8), corkMat);
      cork.position.set(p.x, 0.475 + p.r * 1.8 + p.h * 0.55, p.z);
      group.add(cork);
    }

    // Herb baskets (one on counter, one beside front apron)
    const addHerbBasket = (bx, by, bz, scale, herbColor) => {
      const basket = new THREE.Mesh(
        new THREE.CylinderGeometry(0.12 * scale, 0.09 * scale, 0.13 * scale, 10),
        basketMat
      );
      basket.position.set(bx, by + 0.065 * scale, bz);
      basket.castShadow = true;
      group.add(basket);

      const herbMat = new THREE.MeshStandardMaterial({ color: herbColor, roughness: 0.85 });
      for (const [ox, oz] of [[-0.03, 0.02], [0.03, -0.02], [0, 0.03]]) {
        const tuft = new THREE.Mesh(new THREE.DodecahedronGeometry(0.055 * scale, 0), herbMat);
        tuft.position.set(bx + ox * scale, by + 0.13 * scale, bz + oz * scale);
        group.add(tuft);
      }
    };

    addHerbBasket(0.04, 0.472, -0.04, 1.0, 0x22c55e);
    addHerbBasket(-0.26, 0.0, 0.36, 1.15, 0x16a34a);
    addHerbBasket(0.24, 0.0, 0.36, 1.0, 0xa855f7);

    // Merchant brass coin scale on right side of counter
    const scaleBase = new THREE.Mesh(new THREE.CylinderGeometry(0.05, 0.06, 0.025, 10), brassMat);
    scaleBase.position.set(0.29, 0.485, 0.08);
    group.add(scaleBase);

    const scaleStem = new THREE.Mesh(new THREE.CylinderGeometry(0.012, 0.014, 0.17, 8), brassMat);
    scaleStem.position.set(0.29, 0.57, 0.08);
    group.add(scaleStem);

    const scaleBeam = new THREE.Mesh(new THREE.BoxGeometry(0.18, 0.016, 0.016), brassMat);
    scaleBeam.position.set(0.29, 0.64, 0.08);
    scaleBeam.rotation.z = 0.08;
    group.add(scaleBeam);

    for (const [px, py] of [[0.21, 0.56], [0.37, 0.575]]) {
      const pan = new THREE.Mesh(new THREE.CylinderGeometry(0.04, 0.03, 0.014, 10), brassMat);
      pan.position.set(px, py, 0.08);
      group.add(pan);
    }

    // Small stack of gold coins beside scale
    const coinStack = new THREE.Mesh(new THREE.CylinderGeometry(0.026, 0.026, 0.032, 10), brassMat);
    coinStack.position.set(0.38, 0.49, 0.18);
    group.add(coinStack);

    return 1.48;
  }

  /**
   * Prefab 13: Village Houses — timber-framed walls, pitched roof with overhang beams,
   * glowing warm window panes, stone chimney, and wooden porch step.
   */
  _buildHouse(group, mainMat) {
    const stoneMat = new THREE.MeshStandardMaterial({ color: 0x64748b, roughness: 0.9 });
    const timberMat = new THREE.MeshStandardMaterial({ color: 0x4a2c14, roughness: 0.85 });
    const porchMat = new THREE.MeshStandardMaterial({ color: 0x78451e, roughness: 0.82 });
    const roofMat = new THREE.MeshStandardMaterial({ color: 0x9a3412, roughness: 0.72 });
    const doorMat = new THREE.MeshStandardMaterial({ color: 0x3b1d0a, roughness: 0.88 });
    const windowGlowMat = new THREE.MeshStandardMaterial({
      color: 0xfde68a,
      emissive: 0xf59e0b,
      emissiveIntensity: 1.1,
      roughness: 0.25,
    });

    // Primary plaster cottage walls
    const mainMesh = new THREE.Mesh(new THREE.BoxGeometry(0.86, 0.58, 0.78), mainMat);
    mainMesh.position.set(0, 0.37, 0);
    mainMesh.castShadow = true;
    mainMesh.receiveShadow = true;
    group.add(mainMesh);
    group.userData.mesh = mainMesh;

    // Stone foundation skirt
    const foundation = new THREE.Mesh(new THREE.BoxGeometry(0.92, 0.09, 0.84), stoneMat);
    foundation.position.set(0, 0.045, 0);
    foundation.castShadow = true;
    foundation.receiveShadow = true;
    group.add(foundation);

    // Half-timbered vertical corner & center posts
    const cornerGeo = new THREE.BoxGeometry(0.065, 0.60, 0.065);
    for (const sx of [-1, 1]) {
      for (const sz of [-1, 1]) {
        const post = new THREE.Mesh(cornerGeo, timberMat);
        post.position.set(sx * 0.415, 0.38, sz * 0.375);
        group.add(post);
      }
    }

    // Horizontal half-timber wall belt beams
    const beltTop = new THREE.Mesh(new THREE.BoxGeometry(0.89, 0.055, 0.81), timberMat);
    beltTop.position.set(0, 0.65, 0);
    group.add(beltTop);

    const beltMid = new THREE.Mesh(new THREE.BoxGeometry(0.885, 0.04, 0.805), timberMat);
    beltMid.position.set(0, 0.35, 0);
    group.add(beltMid);

    // Triangular gable end walls (front +Z and back -Z)
    const gableShape = new THREE.Shape();
    gableShape.moveTo(-0.43, 0);
    gableShape.lineTo(0.43, 0);
    gableShape.lineTo(0, 0.32);
    gableShape.closePath();
    const gableGeo = new THREE.ExtrudeGeometry(gableShape, {
      depth: 0.76,
      bevelEnabled: false,
    });
    gableGeo.translate(0, 0, -0.38);
    const gable = new THREE.Mesh(gableGeo, mainMat);
    gable.position.set(0, 0.66, 0);
    gable.castShadow = true;
    group.add(gable);

    // Pitched shingle roof with overhanging eaves + exposed purlin beams
    const roofSlopeGeo = new THREE.BoxGeometry(0.68, 0.055, 0.96);
    for (const side of [-1, 1]) {
      const slope = new THREE.Mesh(roofSlopeGeo, roofMat);
      slope.position.set(side * 0.25, 0.82, 0);
      slope.rotation.z = side * -0.62;
      slope.castShadow = true;
      group.add(slope);

      // Protruding timber overhang beam under eaves
      const overhangBeam = new THREE.Mesh(new THREE.BoxGeometry(0.05, 0.05, 0.98), timberMat);
      overhangBeam.position.set(side * 0.45, 0.66, 0);
      group.add(overhangBeam);
    }

    const ridgeBeam = new THREE.Mesh(new THREE.BoxGeometry(0.075, 0.075, 1.00), timberMat);
    ridgeBeam.position.set(0, 0.985, 0);
    ridgeBeam.castShadow = true;
    group.add(ridgeBeam);

    // Stone chimney rising through roof
    const chimney = new THREE.Mesh(new THREE.BoxGeometry(0.17, 0.72, 0.17), stoneMat);
    chimney.position.set(0.28, 0.88, -0.18);
    chimney.castShadow = true;
    group.add(chimney);

    const chimneyCap = new THREE.Mesh(new THREE.BoxGeometry(0.22, 0.055, 0.22), stoneMat);
    chimneyCap.position.set(0.28, 1.24, -0.18);
    group.add(chimneyCap);

    // Front oak door + wooden porch steps + porch overhang beam
    const doorFrame = new THREE.Mesh(new THREE.BoxGeometry(0.26, 0.42, 0.05), timberMat);
    doorFrame.position.set(0, 0.29, 0.39);
    group.add(doorFrame);

    const door = new THREE.Mesh(new THREE.BoxGeometry(0.20, 0.38, 0.06), doorMat);
    door.position.set(0, 0.28, 0.395);
    group.add(door);

    const porchUpper = new THREE.Mesh(new THREE.BoxGeometry(0.36, 0.05, 0.14), porchMat);
    porchUpper.position.set(0, 0.07, 0.45);
    porchUpper.receiveShadow = true;
    group.add(porchUpper);

    const porchStep = new THREE.Mesh(new THREE.BoxGeometry(0.40, 0.045, 0.12), porchMat);
    porchStep.position.set(0, 0.0225, 0.53);
    porchStep.receiveShadow = true;
    group.add(porchStep);

    const porchCanopy = new THREE.Mesh(new THREE.BoxGeometry(0.32, 0.035, 0.18), roofMat);
    porchCanopy.position.set(0, 0.53, 0.45);
    porchCanopy.rotation.x = 0.22;
    group.add(porchCanopy);

    // Glowing warm window panes with timber frames & cross mullions (front, back, and sides)
    const addWindow = (wx, wy, wz, rotY = 0) => {
      const winGroup = new THREE.Group();
      winGroup.position.set(wx, wy, wz);
      winGroup.rotation.y = rotY;

      const frame = new THREE.Mesh(new THREE.BoxGeometry(0.18, 0.18, 0.03), timberMat);
      winGroup.add(frame);

      const pane = new THREE.Mesh(new THREE.BoxGeometry(0.135, 0.135, 0.036), windowGlowMat);
      winGroup.add(pane);

      const mullionH = new THREE.Mesh(new THREE.BoxGeometry(0.14, 0.018, 0.042), timberMat);
      winGroup.add(mullionH);

      const mullionV = new THREE.Mesh(new THREE.BoxGeometry(0.018, 0.14, 0.042), timberMat);
      winGroup.add(mullionV);

      group.add(winGroup);
    };

    // Front windows flanking door + back windows + side windows
    addWindow(-0.25, 0.43, 0.395, 0);
    addWindow(0.25, 0.43, 0.395, 0);
    addWindow(-0.22, 0.43, -0.395, Math.PI);
    addWindow(0.22, 0.43, -0.395, Math.PI);
    addWindow(-0.435, 0.43, 0, -Math.PI / 2);
    addWindow(0.435, 0.43, 0, Math.PI / 2);

    return 1.48;
  }

  clear() {
    for (const group of this.objects.values()) {
      this.scene.remove(group);
      disposeHierarchy(group);
    }
    this.objects.clear();
  }
}
