import * as THREE from "three";
import { WorldObjectRenderer } from "./world.js";

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

    // Window resize handling
    window.addEventListener("resize", () => this.onResize());
  }

  initLighting() {
    const ambientLight = new THREE.AmbientLight(0xffffff, 0.6);
    this.scene.add(ambientLight);

    const dirLight = new THREE.DirectionalLight(0x93c5fd, 1.2);
    dirLight.position.set(50, 80, 50);
    dirLight.castShadow = true;
    this.scene.add(dirLight);
  }

  initEnvironment() {
    // Coordinate ground grid (1000m x 1000m with 10m divisions)
    const gridHelper = new THREE.GridHelper(1000, 100, 0x3b82f6, 0x1e293b);
    gridHelper.position.y = -0.01;
    this.scene.add(gridHelper);

    // Ground plane
    const groundGeo = new THREE.PlaneGeometry(1000, 1000);
    const groundMat = new THREE.MeshStandardMaterial({
      color: 0x0f172a,
      roughness: 0.9,
      metalness: 0.1,
    });
    const ground = new THREE.Mesh(groundGeo, groundMat);
    ground.rotation.x = -Math.PI / 2;
    ground.receiveShadow = true;
    this.scene.add(ground);

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
    const activeGuids = new Set();
    const entityStride = 9;
    const count = Math.floor(renderBuffer.length / entityStride);
    const localBigInt = localPlayerGuid !== null ? BigInt(localPlayerGuid) : null;
    const uint32View = new Uint32Array(
      renderBuffer.buffer,
      renderBuffer.byteOffset,
      renderBuffer.length
    );

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

    // Remove despawned entity meshes
    for (const [guid, mesh] of this.entities.entries()) {
      if (!activeGuids.has(guid)) {
        this.scene.remove(mesh);
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
   * Follows local player position with third-person camera.
   */
  updateCamera(targetPos) {
    if (!targetPos) return;

    // Center AOI ring on player
    this.aoiRing.position.x = targetPos.x;
    this.aoiRing.position.z = targetPos.z;

    // Third-person smooth follow
    const offset = new THREE.Vector3(0, 18, 26);
    this.camera.position.set(
      targetPos.x + offset.x,
      targetPos.y + offset.y,
      targetPos.z + offset.z
    );
    this.camera.lookAt(targetPos.x, targetPos.y + 1.5, targetPos.z);
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
}
