import * as THREE from "three";
import { WorldObjectRenderer } from "./world.js";

// Third-person camera follow offset (module constants avoid a per-frame Vector3 allocation).
const CAMERA_OFFSET_X = 0;
const CAMERA_OFFSET_Y = 18;
const CAMERA_OFFSET_Z = 26;

/**
 * Recursively disposes GPU resources (geometries, materials, textures) in a Three.js hierarchy.
 */
export function disposeHierarchy(node) {
  if (!node) return;
  node.traverse((child) => {
    if (child.geometry) {
      child.geometry.dispose();
    }
    if (child.material) {
      const materials = Array.isArray(child.material) ? child.material : [child.material];
      for (const m of materials) {
        if (!m) continue;
        for (const key of Object.keys(m)) {
          const value = m[key];
          if (value && typeof value.dispose === "function" && value.isTexture) {
            value.dispose();
          }
        }
        m.dispose();
      }
    }
  });
}

/**
 * Three.js 3D Viewport Renderer for OpenSpec MMO.
 */
export class WorldRenderer {
  constructor(containerElement) {
    this.container = containerElement;
    this.entities = new Map();
    this.scene = new THREE.Scene();
    this.scene.background = new THREE.Color(0x0b0f19);
    this.scene.fog = new THREE.FogExp2(0x0b0f19, 0.005);
    this.worldObjects = new WorldObjectRenderer(this.scene);

    // Orbit Camera State
    this.orbitYaw = 0.0;
    this.orbitPitch = 0.45;
    this.orbitDistance = 24.0;
    this.isDragging = false;
    this.previousPointerPosition = { x: 0, y: 0 };

    if (typeof window === "undefined" || !containerElement) {
      return;
    }

    // Camera
    this.camera = new THREE.PerspectiveCamera(
      60,
      window.innerWidth / window.innerHeight,
      0.1,
      1000
    );
    this.camera.position.set(0, 25, 35);
    this.camera.lookAt(0, 0, 0);

    // WebGL Renderer
    this.renderer = new THREE.WebGLRenderer({ antialias: true });
    this.renderer.setSize(window.innerWidth, window.innerHeight);
    this.renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    this.renderer.shadowMap.enabled = true;
    this.container.appendChild(this.renderer.domElement);

    // Lighting
    this.initLighting();

    // Environment & Partition Boundaries
    this.initEnvironment();

    // Camera Controls
    this.initCameraControls();

    // Window resize handling
    window.addEventListener("resize", () => this.onResize());
  }

  initLighting() {
    const ambientLight = new THREE.AmbientLight(0xffedd5, 0.55);
    this.scene.add(ambientLight);

    const dirLight = new THREE.DirectionalLight(0xfff7ed, 1.3);
    dirLight.position.set(50, 80, 50);
    dirLight.castShadow = true;
    this.scene.add(dirLight);

    const hemiLight = new THREE.HemisphereLight(0x93c5fd, 0x162c1d, 0.4);
    this.scene.add(hemiLight);
  }

  initCameraControls() {
    const canvas = this.renderer?.domElement;
    if (!canvas) return;

    canvas.addEventListener("pointerdown", (e) => {
      if (e.target && e.target.closest && e.target.closest("#hud-overlay, .modal-overlay")) return;
      this.isDragging = true;
      this.previousPointerPosition = { x: e.clientX, y: e.clientY };
      try {
        canvas.setPointerCapture(e.pointerId);
      } catch (_) {}
    });

    canvas.addEventListener("pointermove", (e) => {
      if (!this.isDragging) return;
      const deltaX = e.clientX - this.previousPointerPosition.x;
      const deltaY = e.clientY - this.previousPointerPosition.y;
      this.previousPointerPosition = { x: e.clientX, y: e.clientY };

      const sensitivity = 0.005;
      this.orbitYaw += deltaX * sensitivity;
      this.orbitPitch -= deltaY * sensitivity;

      // Clamp pitch to prevent gimbal lock singularities and ground plane clipping
      this.orbitPitch = Math.max(0.05, Math.min(1.45, this.orbitPitch));
    });

    const stopDrag = (e) => {
      if (this.isDragging) {
        this.isDragging = false;
        try {
          if (e && e.pointerId) canvas.releasePointerCapture(e.pointerId);
        } catch (_) {}
      }
    };

    canvas.addEventListener("pointerup", stopDrag);
    canvas.addEventListener("pointercancel", stopDrag);

    canvas.addEventListener("wheel", (e) => {
      if (e.target && e.target.closest && e.target.closest("#hud-overlay, .modal-overlay")) return;
      e.preventDefault();
      const zoomFactor = 0.03;
      this.orbitDistance += e.deltaY * zoomFactor;
      this.orbitDistance = Math.max(5.0, Math.min(60.0, this.orbitDistance));
    }, { passive: false });
  }

  initEnvironment() {
    // Coordinate ground grid (1000m x 1000m with 10m divisions)
    const gridHelper = new THREE.GridHelper(1000, 100, 0x3b82f6, 0x1e293b);
    gridHelper.position.y = -0.01;
    this.scene.add(gridHelper);

    // Meadow ground plane
    const groundGeo = new THREE.PlaneGeometry(1000, 1000);
    const groundMat = new THREE.MeshStandardMaterial({
      color: 0x162c1d, // Deep meadow green
      roughness: 0.95,
      metalness: 0.05,
    });
    const ground = new THREE.Mesh(groundGeo, groundMat);
    ground.rotation.x = -Math.PI / 2;
    ground.receiveShadow = true;
    this.scene.add(ground);

    // Village central cobblestone plaza around the well (radius 14m)
    const plazaGeo = new THREE.CircleGeometry(14, 32);
    const plazaMat = new THREE.MeshStandardMaterial({
      color: 0x475569, // Slate cobblestone
      roughness: 0.85,
    });
    const plaza = new THREE.Mesh(plazaGeo, plazaMat);
    plaza.rotation.x = -Math.PI / 2;
    plaza.position.y = 0.01;
    plaza.receiveShadow = true;
    this.scene.add(plaza);

    // Cobblestone plaza outer border rim
    const rimGeo = new THREE.RingGeometry(13.5, 14.2, 32);
    const rimMat = new THREE.MeshStandardMaterial({
      color: 0x64748b,
      roughness: 0.8,
    });
    const rim = new THREE.Mesh(rimGeo, rimMat);
    rim.rotation.x = -Math.PI / 2;
    rim.position.y = 0.02;
    this.scene.add(rim);

    // Radial dirt pathways connecting plaza to the 4 village building locations:
    // 1. Forge: (-12, 0, -8), 2. Stall: (12, 0, -8), 3. House West: (-18, 0, 10), 4. House East: (18, 0, 10)
    const pathMat = new THREE.MeshStandardMaterial({
      color: 0x574332, // Beaten dirt pathway
      roughness: 0.9,
    });

    const createPath = (x1, z1, x2, z2, width = 3.2) => {
      const dx = x2 - x1;
      const dz = z2 - z1;
      const length = Math.hypot(dx, dz);
      const angle = Math.atan2(dx, dz);
      const pathGeo = new THREE.PlaneGeometry(width, length);
      const path = new THREE.Mesh(pathGeo, pathMat);
      path.rotation.x = -Math.PI / 2;
      path.rotation.z = -angle;
      path.position.set((x1 + x2) / 2, 0.015, (z1 + z2) / 2);
      path.receiveShadow = true;
      this.scene.add(path);
    };

    createPath(0, 0, -12, -8);
    createPath(0, 0, 12, -8);
    createPath(0, 0, -18, 10);
    createPath(0, 0, 18, 10);

    // 4 Village Lantern Posts around the plaza perimeter
    const lanternPositions = [
      [7.5, 7.5],
      [-7.5, 7.5],
      [7.5, -7.5],
      [-7.5, -7.5],
    ];
    const postMat = new THREE.MeshStandardMaterial({ color: 0x451a03, roughness: 0.8 });
    const lanternMat = new THREE.MeshStandardMaterial({
      color: 0xf59e0b,
      emissive: 0xf59e0b,
      emissiveIntensity: 0.9,
    });

    lanternPositions.forEach(([lx, lz]) => {
      const postGeo = new THREE.CylinderGeometry(0.12, 0.15, 2.8, 8);
      const post = new THREE.Mesh(postGeo, postMat);
      post.position.set(lx, 1.4, lz);
      this.scene.add(post);

      const lampGeo = new THREE.BoxGeometry(0.35, 0.45, 0.35);
      const lamp = new THREE.Mesh(lampGeo, lanternMat);
      lamp.position.set(lx, 2.9, lz);
      this.scene.add(lamp);

      const light = new THREE.PointLight(0xffedd5, 1.2, 14);
      light.position.set(lx, 3.0, lz);
      this.scene.add(light);
    });

    // Perimeter stylized conifer trees around village outskirts
    const treeTrunkMat = new THREE.MeshStandardMaterial({ color: 0x3d2817, roughness: 0.9 });
    const foliageMat = new THREE.MeshStandardMaterial({ color: 0x14532d, roughness: 0.8 });
    const treePositions = [
      [-28, -20], [-34, -5], [-32, 18], [-24, 28],
      [28, -20], [35, -4], [30, 20], [22, 28],
      [-10, -28], [10, -28], [-4, 32], [8, 32],
    ];

    treePositions.forEach(([tx, tz]) => {
      const treeGroup = new THREE.Group();
      const trunkGeo = new THREE.CylinderGeometry(0.3, 0.45, 1.6, 8);
      const trunk = new THREE.Mesh(trunkGeo, treeTrunkMat);
      trunk.position.y = 0.8;
      treeGroup.add(trunk);

      const cone1 = new THREE.Mesh(new THREE.ConeGeometry(2.0, 2.4, 7), foliageMat);
      cone1.position.y = 2.4;
      treeGroup.add(cone1);

      const cone2 = new THREE.Mesh(new THREE.ConeGeometry(1.4, 2.0, 7), foliageMat);
      cone2.position.y = 3.6;
      treeGroup.add(cone2);

      treeGroup.position.set(tx, 0, tz);
      this.scene.add(treeGroup);
    });

    // Partition boundary planes (representing server cluster nodes)
    this.partitionGroup = new THREE.Group();
    const planeGeo = new THREE.PlaneGeometry(200, 40);
    const planeMatX = new THREE.MeshBasicMaterial({
      color: 0x38bdf8,
      transparent: true,
      opacity: 0.15,
      side: THREE.DoubleSide,
      wireframe: false,
    });

    const planeX = new THREE.Mesh(planeGeo, planeMatX);
    planeX.position.set(0, 20, 0);
    this.partitionGroup.add(planeX);

    const planeZ = new THREE.Mesh(planeGeo, planeMatX.clone());
    planeZ.rotation.y = Math.PI / 2;
    planeZ.position.set(0, 20, 0);
    this.partitionGroup.add(planeZ);

    this.scene.add(this.partitionGroup);

    // AOI Radius Circle (80m radius wireframe circle)
    const ringGeo = new THREE.RingGeometry(79.5, 80.0, 64);
    const ringMat = new THREE.MeshBasicMaterial({
      color: 0xf59e0b,
      side: THREE.DoubleSide,
      transparent: true,
      opacity: 0.4,
    });
    this.aoiRing = new THREE.Mesh(ringGeo, ringMat);
    this.aoiRing.rotation.x = -Math.PI / 2;
    this.aoiRing.position.y = 0.05;
    this.scene.add(this.aoiRing);
  }

  /**
   * Synchronizes Three.js scene meshes from the flat WASM render buffer.
   * Format per entity (9 floats):
   * [guid_lo, guid_hi, x, y, z, yaw, hp_pct, cast_pct, entity_type]
   */
  syncEntities(renderBuffer, localPlayerGuid = null) {
    if (!renderBuffer || renderBuffer.length === 0) return;
    // Reuse the same Set / typed-array view across frames to avoid per-frame garbage.
    const activeGuids = (this._activeGuids ??= new Set());
    activeGuids.clear();
    const entityStride = 9;
    const count = Math.floor(renderBuffer.length / entityStride);
    const localBigInt = localPlayerGuid !== null ? BigInt(localPlayerGuid) : null;
    let uint32View = this._entityU32View;
    if (
      !uint32View ||
      uint32View.buffer !== renderBuffer.buffer ||
      uint32View.byteOffset !== renderBuffer.byteOffset ||
      uint32View.length !== renderBuffer.length
    ) {
      uint32View = new Uint32Array(
        renderBuffer.buffer,
        renderBuffer.byteOffset,
        renderBuffer.length
      );
      this._entityU32View = uint32View;
    }

    for (let i = 0; i < count; i++) {
      const offset = i * entityStride;
      const guidLo = BigInt(uint32View[offset]);
      const guidHi = BigInt(uint32View[offset + 1]);
      const guid = (guidHi << 32n) | guidLo;
      const x = renderBuffer[offset + 2];
      const y = renderBuffer[offset + 3];
      const z = renderBuffer[offset + 4];
      const yaw = renderBuffer[offset + 5];
      const hpPct = renderBuffer[offset + 6];
      const _castPct = renderBuffer[offset + 7];
      const entityType = Math.floor(renderBuffer[offset + 8]);

      activeGuids.add(guid);

      let group = this.entities.get(guid);
      if (!group) {
        group = this.createEntityMesh(entityType, localBigInt !== null && guid === localBigInt);
        this.entities.set(guid, group);
        this.scene.add(group);
      }

      // Update position & rotation
      group.position.set(x, y, z);
      group.rotation.y = yaw;

      // Update health bar scale
      if (group.userData.hpBar) {
        group.userData.hpBar.scale.x = Math.max(0.01, hpPct);
      }
    }

    // Remove despawned entity meshes and release GPU memory
    for (const [guid, mesh] of this.entities.entries()) {
      if (!activeGuids.has(guid)) {
        this.scene.remove(mesh);
        disposeHierarchy(mesh);
        this.entities.delete(guid);
      }
    }
  }

  /**
   * Updates world objects from flat WASM render buffer (16 floats per object).
   */
  updateWorldObjects(buffer) {
    this.worldObjects.updateFromBuffer(buffer);
  }

  /**
   * Factory creating mesh geometries based on entity type.
   */
  createEntityMesh(entityType, isLocalPlayer) {
    const group = new THREE.Group();

    let bodyMesh;
    if (isLocalPlayer) {
      // Local player: vibrant blue capsule with forward directional pointer
      const geo = new THREE.CapsuleGeometry(0.8, 1.8, 8, 16);
      const mat = new THREE.MeshStandardMaterial({
        color: 0x2563eb,
        roughness: 0.3,
        metalness: 0.2,
      });
      bodyMesh = new THREE.Mesh(geo, mat);
      bodyMesh.position.y = 1.3;

      // Nose / facing cone
      const coneGeo = new THREE.ConeGeometry(0.3, 0.8, 8);
      const coneMat = new THREE.MeshBasicMaterial({ color: 0x60a5fa });
      const cone = new THREE.Mesh(coneGeo, coneMat);
      cone.rotation.x = Math.PI / 2;
      cone.position.set(0, 1.6, 0.9);
      group.add(cone);
    } else if (entityType === 1) {
      // Remote player: green capsule
      const geo = new THREE.CapsuleGeometry(0.8, 1.8, 8, 16);
      const mat = new THREE.MeshStandardMaterial({
        color: 0x10b981,
        roughness: 0.4,
      });
      bodyMesh = new THREE.Mesh(geo, mat);
      bodyMesh.position.y = 1.3;
    } else if (entityType === 2) {
      // Villager NPC: purple/indigo robe capsule with golden merchant marker diamond
      const geo = new THREE.CapsuleGeometry(0.8, 1.8, 8, 16);
      const mat = new THREE.MeshStandardMaterial({
        color: 0x8b5cf6, // Purple villager attire
        roughness: 0.5,
      });
      bodyMesh = new THREE.Mesh(geo, mat);
      bodyMesh.position.y = 1.3;

      // Floating golden merchant diamond marker
      const markerGeo = new THREE.OctahedronGeometry(0.35);
      const markerMat = new THREE.MeshStandardMaterial({
        color: 0xf59e0b,
        metalness: 0.8,
        roughness: 0.2,
        emissive: 0xd97706,
        emissiveIntensity: 0.5,
      });
      const marker = new THREE.Mesh(markerGeo, markerMat);
      marker.position.y = 3.3;
      group.add(marker);
      group.userData.marker = marker;
    } else {
      // Hostile Mob / Other: red cube
      const geo = new THREE.BoxGeometry(1.6, 1.6, 1.6);
      const mat = new THREE.MeshStandardMaterial({
        color: 0xef4444,
        roughness: 0.5,
      });
      bodyMesh = new THREE.Mesh(geo, mat);
      bodyMesh.position.y = 0.8;
    }

    bodyMesh.castShadow = true;
    group.add(bodyMesh);

    // Floating overhead health bar
    const barGeo = new THREE.BoxGeometry(1.4, 0.12, 0.05);
    const barMat = new THREE.MeshBasicMaterial({ color: 0x22c55e });
    const hpBar = new THREE.Mesh(barGeo, barMat);
    hpBar.position.y = 2.8;
    group.add(hpBar);
    group.userData.hpBar = hpBar;

    return group;
  }

  /**
   * Follows local player position with interactive spherical orbit camera.
   */
  updateCamera(targetPos) {
    if (!targetPos) return;

    // Center AOI ring on player
    if (this.aoiRing) {
      this.aoiRing.position.x = targetPos.x;
      this.aoiRing.position.z = targetPos.z;
    }

    const targetOffsetY = 1.5;
    const cosPitch = Math.cos(this.orbitPitch);
    const sinPitch = Math.sin(this.orbitPitch);
    const sinYaw = Math.sin(this.orbitYaw);
    const cosYaw = Math.cos(this.orbitYaw);

    const cx = targetPos.x + this.orbitDistance * sinYaw * cosPitch;
    const cy = targetPos.y + targetOffsetY + this.orbitDistance * sinPitch;
    const cz = targetPos.z + this.orbitDistance * cosYaw * cosPitch;

    this.camera.position.set(cx, cy, cz);
    this.camera.lookAt(targetPos.x, targetPos.y + targetOffsetY, targetPos.z);
  }

  setPartitionsVisible(visible) {
    this.partitionGroup.visible = visible;
  }

  setAoiVisible(visible) {
    this.aoiRing.visible = visible;
  }

  setWireframe(wireframe) {
    this.scene.traverse((child) => {
      if (child.isMesh && child.material) {
        child.material.wireframe = wireframe;
      }
    });
  }

  render() {
    // Animate rotating golden merchant markers
    const angle = performance.now() * 0.002;
    for (const group of this.entities.values()) {
      if (group.userData.marker) {
        group.userData.marker.rotation.y = angle;
      }
    }
    this.renderer.render(this.scene, this.camera);
  }

  onResize() {
    this.camera.aspect = window.innerWidth / window.innerHeight;
    this.camera.updateProjectionMatrix();
    this.renderer.setSize(window.innerWidth, window.innerHeight);
  }

  /**
   * Removes all entity meshes and disposes their GPU resources.
   */
  clear() {
    for (const mesh of this.entities.values()) {
      this.scene.remove(mesh);
      disposeHierarchy(mesh);
    }
    this.entities.clear();
    if (this.worldObjects) {
      this.worldObjects.clear();
    }
  }
}
