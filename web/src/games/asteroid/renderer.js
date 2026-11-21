import * as THREE from "three";
import { disposeHierarchy } from "../../renderer.js";
import { CHUNK_SIZE, chunkKey } from "./voxel_client.js";

export const HOME_STATION_POS = Object.freeze({ x: 0, y: 0, z: 0 });
export const HOME_STATION_SAFE_RADIUS = 15.0;

/**
 * Extracts the 8-bit loot-shininess tier (0..255) from Entity::state_flags bits 16..23.
 */
export function extractShininessTier(stateFlags = 0) {
  const flags = Number(stateFlags) || 0;
  return (flags >>> 16) & 0xff;
}

/**
 * Computes ship material emissive, metalness, roughness, and aura properties
 * scaled dynamically by the replicated loot-shininess tier (0..255).
 */
export function computeShipShaderProps(stateFlags = 0) {
  const tier = extractShininessTier(stateFlags);
  const normalized = tier / 255;

  const metalness = 0.35 + normalized * 0.63;
  const roughness = Math.max(0.08, 0.55 - normalized * 0.45);
  const emissiveIntensity = tier === 0 ? 0.05 : 0.25 + normalized * 2.15;

  let emissiveColor = 0x1e293b;
  let auraColor = 0x38bdf8;
  if (tier >= 180) {
    emissiveColor = 0xfef08a; // Radiant Void/Solar crown
    auraColor = 0xc084fc;
  } else if (tier >= 60) {
    emissiveColor = 0xfacc15; // Solar Gold gleam
    auraColor = 0xfde047;
  } else if (tier > 0) {
    emissiveColor = 0x38bdf8; // Titanium glint
    auraColor = 0x7dd3fc;
  }

  const auraIntensity = tier === 0 ? 0.0 : 0.4 + normalized * 3.0;
  const auraDistance = tier === 0 ? 0.0 : 4.0 + normalized * 14.0;
  const auraScale = tier === 0 ? 0.0 : 1.0 + normalized * 1.1;

  return {
    tier,
    normalized,
    emissiveColor,
    emissiveIntensity,
    metalness,
    roughness,
    auraColor,
    auraIntensity,
    auraDistance,
    auraScale,
  };
}

/**
 * Three.js 3D Space Renderer for Game B (6DoF Voxel Asteroid Miners).
 * Renders a starfield, the Home Orbital Station safe zone at (0,0,0) radius 15,
 * face-culled voxel asteroid chunk meshes, animated laser beams, and loot-shiny ships.
 */
export class AsteroidRenderer {
  constructor(containerElement) {
    this.container = containerElement;
    this.entities = new Map();
    this.voxelChunkMeshes = new Map();
    this.laserBeams = [];

    this.scene = new THREE.Scene();
    this.scene.background = new THREE.Color(0x030712);
    this.scene.fog = new THREE.FogExp2(0x030712, 0.002);

    this.voxelMaterial = new THREE.MeshStandardMaterial({
      vertexColors: true,
      roughness: 0.55,
      metalness: 0.25,
    });

    // Orbit camera state around the active ship
    this.orbitYaw = 0.0;
    this.orbitPitch = 0.32;
    this.orbitDistance = 20.0;
    this.isDragging = false;
    this.previousPointerPosition = { x: 0, y: 0 };

    if (typeof window === "undefined" || !containerElement) {
      return;
    }

    this.camera = new THREE.PerspectiveCamera(
      65,
      window.innerWidth / window.innerHeight,
      0.1,
      1200
    );
    this.camera.position.set(0, 10, 28);
    this.camera.lookAt(0, 0, 0);

    this.renderer = new THREE.WebGLRenderer({ antialias: true });
    this.renderer.setSize(window.innerWidth, window.innerHeight);
    this.renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    this.container.appendChild(this.renderer.domElement);

    this.initLighting();
    this.initEnvironment();
    this.initCameraControls();

    this._onResizeBound = () => this.onResize();
    window.addEventListener("resize", this._onResizeBound);
  }

  initLighting() {
    const ambient = new THREE.AmbientLight(0xdbeafe, 0.55);
    this.scene.add(ambient);

    const sunLight = new THREE.DirectionalLight(0xfef9c3, 1.5);
    sunLight.position.set(80, 120, 60);
    this.scene.add(sunLight);

    const rimLight = new THREE.DirectionalLight(0x818cf8, 0.65);
    rimLight.position.set(-70, -40, -90);
    this.scene.add(rimLight);
  }

  initEnvironment() {
    // 1. Deep-space procedural starfield
    const starCount = 1500;
    const starPos = new Float32Array(starCount * 3);
    const starColors = new Float32Array(starCount * 3);
    const palette = [
      new THREE.Color(0xffffff),
      new THREE.Color(0xbae6fd),
      new THREE.Color(0xfef08a),
      new THREE.Color(0xe9d5ff),
    ];

    for (let i = 0; i < starCount; i++) {
      // Deterministic pseudo-random spherical distribution
      const u = ((i * 73856093) % 10007) / 10007;
      const v = ((i * 19349663) % 10007) / 10007;
      const theta = u * Math.PI * 2;
      const phi = Math.acos(2 * v - 1);
      const r = 280 + (i % 240);

      starPos[i * 3] = r * Math.sin(phi) * Math.cos(theta);
      starPos[i * 3 + 1] = r * Math.cos(phi);
      starPos[i * 3 + 2] = r * Math.sin(phi) * Math.sin(theta);

      const c = palette[i % palette.length];
      starColors[i * 3] = c.r;
      starColors[i * 3 + 1] = c.g;
      starColors[i * 3 + 2] = c.b;
    }

    const starGeo = new THREE.BufferGeometry();
    starGeo.setAttribute("position", new THREE.BufferAttribute(starPos, 3));
    starGeo.setAttribute("color", new THREE.BufferAttribute(starColors, 3));
    const starMat = new THREE.PointsMaterial({
      size: 1.5,
      vertexColors: true,
      transparent: true,
      opacity: 0.9,
      sizeAttenuation: true,
    });
    this.starfield = new THREE.Points(starGeo, starMat);
    this.scene.add(this.starfield);

    // 2. Home Orbital Station & Safe-Zone Sphere/Ring at (0, 0, 0) radius 15
    this.stationGroup = new THREE.Group();
    this.stationGroup.position.set(HOME_STATION_POS.x, HOME_STATION_POS.y, HOME_STATION_POS.z);

    const coreGeo = new THREE.OctahedronGeometry(2.8, 1);
    const coreMat = new THREE.MeshStandardMaterial({
      color: 0x0ea5e9,
      emissive: 0x0284c7,
      emissiveIntensity: 0.8,
      metalness: 0.8,
      roughness: 0.2,
    });
    const coreMesh = new THREE.Mesh(coreGeo, coreMat);
    this.stationGroup.add(coreMesh);
    this.stationCore = coreMesh;

    const torusGeo = new THREE.TorusGeometry(5.5, 0.45, 12, 48);
    const torusMat = new THREE.MeshStandardMaterial({
      color: 0x94a3b8,
      metalness: 0.85,
      roughness: 0.25,
      emissive: 0x0369a1,
      emissiveIntensity: 0.35,
    });
    const stationRing = new THREE.Mesh(torusGeo, torusMat);
    stationRing.rotation.x = Math.PI / 2;
    this.stationGroup.add(stationRing);
    this.stationDockRing = stationRing;

    const beaconLight = new THREE.PointLight(0x38bdf8, 2.2, 40);
    this.stationGroup.add(beaconLight);

    // Safe-zone wireframe sphere (radius 15)
    const safeSphereGeo = new THREE.SphereGeometry(HOME_STATION_SAFE_RADIUS, 28, 18);
    const safeSphereMat = new THREE.MeshBasicMaterial({
      color: 0x22d3ee,
      transparent: true,
      opacity: 0.1,
      wireframe: true,
    });
    this.safeZoneSphere = new THREE.Mesh(safeSphereGeo, safeSphereMat);
    this.stationGroup.add(this.safeZoneSphere);

    // Glowing equatorial safe-zone ring (radius 15)
    const safeRingGeo = new THREE.RingGeometry(
      HOME_STATION_SAFE_RADIUS - 0.3,
      HOME_STATION_SAFE_RADIUS,
      64
    );
    const safeRingMat = new THREE.MeshBasicMaterial({
      color: 0x22d3ee,
      side: THREE.DoubleSide,
      transparent: true,
      opacity: 0.55,
    });
    this.safeZoneRing = new THREE.Mesh(safeRingGeo, safeRingMat);
    this.safeZoneRing.rotation.x = -Math.PI / 2;
    this.stationGroup.add(this.safeZoneRing);

    this.scene.add(this.stationGroup);

    // 3. Sector Partition Planes (for visual debug toggle parity)
    this.partitionGroup = new THREE.Group();
    const pGeo = new THREE.PlaneGeometry(200, 200);
    const pMat = new THREE.MeshBasicMaterial({
      color: 0x38bdf8,
      transparent: true,
      opacity: 0.08,
      side: THREE.DoubleSide,
    });
    const px = new THREE.Mesh(pGeo, pMat);
    const pz = new THREE.Mesh(pGeo, pMat.clone());
    pz.rotation.y = Math.PI / 2;
    this.partitionGroup.add(px);
    this.partitionGroup.add(pz);
    this.scene.add(this.partitionGroup);

    // 4. AOI Radius Circle (80m radius)
    const aoiGeo = new THREE.RingGeometry(79.5, 80.0, 64);
    const aoiMat = new THREE.MeshBasicMaterial({
      color: 0xf59e0b,
      side: THREE.DoubleSide,
      transparent: true,
      opacity: 0.3,
    });
    this.aoiRing = new THREE.Mesh(aoiGeo, aoiMat);
    this.aoiRing.rotation.x = -Math.PI / 2;
    this.scene.add(this.aoiRing);
  }

  initCameraControls() {
    const canvas = this.renderer?.domElement;
    if (!canvas) return;

    canvas.addEventListener("pointerdown", (e) => {
      if (e.target && e.target.closest && e.target.closest("#hud-overlay, #asteroid-hud, .modal-overlay, #game-mode-switcher")) {
        return;
      }
      this.isDragging = true;
      this.previousPointerPosition = { x: e.clientX, y: e.clientY };
      try {
        canvas.setPointerCapture(e.pointerId);
      } catch (_) {}
    });

    canvas.addEventListener("pointermove", (e) => {
      if (!this.isDragging) return;
      const dx = e.clientX - this.previousPointerPosition.x;
      const dy = e.clientY - this.previousPointerPosition.y;
      this.previousPointerPosition = { x: e.clientX, y: e.clientY };

      const sensitivity = 0.005;
      this.orbitYaw += dx * sensitivity;
      this.orbitPitch -= dy * sensitivity;
      this.orbitPitch = Math.max(-1.4, Math.min(1.4, this.orbitPitch));
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

    canvas.addEventListener(
      "wheel",
      (e) => {
        if (e.target && e.target.closest && e.target.closest("#hud-overlay, #asteroid-hud, .modal-overlay, #game-mode-switcher")) {
          return;
        }
        e.preventDefault();
        this.orbitDistance = Math.max(6.0, Math.min(75.0, this.orbitDistance + e.deltaY * 0.03));
      },
      { passive: false }
    );
  }

  /**
   * Creates a 3D spacecraft mesh with dynamic shininess hull material and aura light.
   */
  createShipMesh(isLocalPlayer, stateFlags = 0) {
    const group = new THREE.Group();
    group.rotation.order = "YXZ";

    const props = computeShipShaderProps(stateFlags);
    const baseColor = isLocalPlayer ? 0x3b82f6 : 0x10b981;

    const hullMat = new THREE.MeshStandardMaterial({
      color: baseColor,
      metalness: props.metalness,
      roughness: props.roughness,
      emissive: props.emissiveColor,
      emissiveIntensity: props.emissiveIntensity,
    });

    // Sleek fuselage cone pointing along +Z
    const hullGeo = new THREE.ConeGeometry(0.85, 3.0, 8);
    hullGeo.rotateX(Math.PI / 2);
    const hullMesh = new THREE.Mesh(hullGeo, hullMat);
    group.add(hullMesh);

    // Swept delta wings
    const wingGeo = new THREE.BoxGeometry(3.2, 0.14, 1.1);
    const wingMesh = new THREE.Mesh(wingGeo, hullMat);
    wingMesh.position.set(0, 0, -0.45);
    group.add(wingMesh);

    // Cockpit canopy
    const canopyGeo = new THREE.SphereGeometry(0.42, 12, 8);
    const canopyMat = new THREE.MeshStandardMaterial({
      color: 0x38bdf8,
      emissive: 0x0284c7,
      emissiveIntensity: 0.7,
      roughness: 0.1,
      metalness: 0.9,
    });
    const canopy = new THREE.Mesh(canopyGeo, canopyMat);
    canopy.position.set(0, 0.35, 0.35);
    group.add(canopy);

    // Rear thruster glow
    const engineGeo = new THREE.CylinderGeometry(0.32, 0.2, 0.4, 8);
    engineGeo.rotateX(Math.PI / 2);
    const engineMat = new THREE.MeshBasicMaterial({ color: 0x38bdf8 });
    const engine = new THREE.Mesh(engineGeo, engineMat);
    engine.position.set(0, 0, -1.55);
    group.add(engine);

    // Loot-shininess translucent aura halo
    const auraGeo = new THREE.SphereGeometry(1.9, 16, 12);
    const auraMat = new THREE.MeshBasicMaterial({
      color: props.auraColor,
      transparent: true,
      opacity: props.tier > 0 ? 0.15 + props.normalized * 0.35 : 0.0,
      wireframe: true,
    });
    const auraHalo = new THREE.Mesh(auraGeo, auraMat);
    auraHalo.visible = props.tier > 0;
    if (props.auraScale > 0) {
      auraHalo.scale.setScalar(props.auraScale);
    }
    group.add(auraHalo);

    // Loot-shininess point light aura
    const auraLight = new THREE.PointLight(
      props.auraColor,
      props.auraIntensity,
      props.auraDistance
    );
    group.add(auraLight);

    // Overhead hull integrity bar
    const hpGeo = new THREE.BoxGeometry(2.0, 0.14, 0.08);
    const hpMat = new THREE.MeshBasicMaterial({ color: 0x22c55e });
    const hpBar = new THREE.Mesh(hpGeo, hpMat);
    hpBar.position.y = 1.85;
    group.add(hpBar);

    group.userData.hullMat = hullMat;
    group.userData.auraHalo = auraHalo;
    group.userData.auraLight = auraLight;
    group.userData.hpBar = hpBar;
    group.userData.shininessTier = props.tier;
    group.userData.stateFlags = stateFlags;

    return group;
  }

  /**
   * Updates an existing ship mesh's shader material and aura light from `state_flags`.
   */
  applyShipShininess(group, stateFlags = 0) {
    if (!group || !group.userData) return;
    const props = computeShipShaderProps(stateFlags);
    group.userData.shininessTier = props.tier;
    group.userData.stateFlags = stateFlags;

    const hullMat = group.userData.hullMat;
    if (hullMat) {
      hullMat.metalness = props.metalness;
      hullMat.roughness = props.roughness;
      if (hullMat.emissive && typeof hullMat.emissive.setHex === "function") {
        hullMat.emissive.setHex(props.emissiveColor);
      }
      hullMat.emissiveIntensity = props.emissiveIntensity;
    }

    const auraHalo = group.userData.auraHalo;
    if (auraHalo) {
      auraHalo.visible = props.tier > 0;
      if (props.tier > 0) {
        auraHalo.scale.setScalar(props.auraScale);
        if (auraHalo.material) {
          auraHalo.material.color?.setHex?.(props.auraColor);
          auraHalo.material.opacity = 0.15 + props.normalized * 0.35;
        }
      }
    }

    const auraLight = group.userData.auraLight;
    if (auraLight) {
      auraLight.color?.setHex?.(props.auraColor);
      auraLight.intensity = props.auraIntensity;
      auraLight.distance = props.auraDistance;
    }
  }

  /**
   * Synchronizes ship meshes from the flat 9-float render buffer and optional entity metadata Map.
   */
  syncEntities(renderBuffer, localPlayerGuid = null, entitiesMap = null, localOptions = {}) {
    if (!renderBuffer || renderBuffer.length === 0) return;
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
      let hpPct = renderBuffer[offset + 6];

      activeGuids.add(guid);

      const isLocal = localBigInt !== null && guid === localBigInt;
      const entKey = guid <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(guid) : guid;
      const meta = entitiesMap ? (entitiesMap.get(entKey) ?? entitiesMap.get(guid)) : null;
      const stateFlags = meta?.stateFlags ?? (isLocal ? (localOptions.stateFlags ?? 0) : 0);
      const pitch = isLocal ? (localOptions.pitch ?? meta?.pitch ?? 0.0) : (meta?.pitch ?? 0.0);
      if (meta && typeof meta.hpPct === "number") {
        hpPct = meta.hpPct;
      }

      let group = this.entities.get(guid);
      if (!group) {
        group = this.createShipMesh(isLocal, stateFlags);
        group.userData.guid = guid;
        this.entities.set(guid, group);
        this.scene.add(group);
      } else if (group.userData.stateFlags !== stateFlags) {
        this.applyShipShininess(group, stateFlags);
      }

      group.position.set(x, y, z);
      group.rotation.y = yaw;
      group.rotation.x = -pitch;

      if (group.userData.hpBar) {
        group.userData.hpBar.scale.x = Math.max(0.01, Math.min(1.0, hpPct));
      }
    }

    for (const [guid, mesh] of this.entities.entries()) {
      if (!activeGuids.has(guid)) {
        this.scene.remove(mesh);
        disposeHierarchy(mesh);
        this.entities.delete(guid);
      }
    }
  }

  /**
   * Rebuilds any dirty voxel asteroid chunk meshes from `AsteroidVoxelClient`.
   */
  syncVoxelChunks(voxelClient) {
    if (!voxelClient || voxelClient.dirtyMeshes.size === 0) return;

    for (const key of Array.from(voxelClient.dirtyMeshes)) {
      voxelClient.dirtyMeshes.delete(key);
      const entry = voxelClient.chunks.get(key);
      const existingMesh = this.voxelChunkMeshes.get(key);

      if (!entry) {
        if (existingMesh) {
          this.scene.remove(existingMesh);
          existingMesh.geometry?.dispose?.();
          this.voxelChunkMeshes.delete(key);
        }
        continue;
      }

      const geometry = voxelClient.buildChunkGeometry(entry.x, entry.y, entry.z);
      const posAttr = geometry?.getAttribute("position");
      if (!geometry || !posAttr || posAttr.count === 0) {
        geometry?.dispose?.();
        if (existingMesh) {
          this.scene.remove(existingMesh);
          existingMesh.geometry?.dispose?.();
          this.voxelChunkMeshes.delete(key);
        }
        continue;
      }

      if (existingMesh) {
        existingMesh.geometry?.dispose?.();
        existingMesh.geometry = geometry;
      } else {
        const mesh = new THREE.Mesh(geometry, this.voxelMaterial);
        mesh.position.set(entry.x * CHUNK_SIZE, entry.y * CHUNK_SIZE, entry.z * CHUNK_SIZE);
        mesh.userData.chunkKey = key;
        this.voxelChunkMeshes.set(key, mesh);
        this.scene.add(mesh);
      }
    }
  }

  /**
   * Returns active voxel chunk meshes for Three.js raycast picking.
   */
  getChunkMeshObjects() {
    return Array.from(this.voxelChunkMeshes.values());
  }

  /**
   * Returns active remote ship mesh groups for Three.js raycast picking.
   */
  getShipMeshes(localPlayerGuid = null) {
    const localBig = localPlayerGuid !== null ? BigInt(localPlayerGuid) : null;
    const ships = [];
    for (const [guid, group] of this.entities.entries()) {
      if (localBig !== null && guid === localBig) continue;
      ships.push(group);
    }
    return ships;
  }

  /**
   * Spawns a transient glowing laser beam mesh between `fromPos` and `toPos`.
   */
  spawnLaserBeam(fromPos, toPos, color = 0x38bdf8, durationMs = 240) {
    const start = new THREE.Vector3(fromPos.x, fromPos.y, fromPos.z);
    const end = new THREE.Vector3(toPos.x, toPos.y, toPos.z);
    const delta = new THREE.Vector3().subVectors(end, start);
    const length = delta.length();
    if (length < 0.05) return null;

    const geo = new THREE.CylinderGeometry(0.08, 0.08, length, 6);
    geo.translate(0, length * 0.5, 0);
    geo.rotateX(Math.PI / 2);

    const mat = new THREE.MeshBasicMaterial({
      color,
      transparent: true,
      opacity: 0.95,
    });
    const beam = new THREE.Mesh(geo, mat);
    beam.position.copy(start);
    beam.lookAt(end);

    this.scene.add(beam);
    this.laserBeams.push({
      mesh: beam,
      expiresAt: performance.now() + durationMs,
      durationMs,
    });
    return beam;
  }

  updateWorldObjects(_buffer) {
    // Space sector uses Orbital Station and procedural voxel asteroids
  }

  /**
   * Follows the player ship in 3D with interactive spherical orbit camera.
   */
  updateCamera(targetPos) {
    if (!targetPos || !this.camera) return;

    if (this.aoiRing) {
      this.aoiRing.position.set(targetPos.x, targetPos.y, targetPos.z);
    }

    const cosPitch = Math.cos(this.orbitPitch);
    const sinPitch = Math.sin(this.orbitPitch);
    const sinYaw = Math.sin(this.orbitYaw);
    const cosYaw = Math.cos(this.orbitYaw);

    const cx = targetPos.x + this.orbitDistance * sinYaw * cosPitch;
    const cy = targetPos.y + 2.0 + this.orbitDistance * sinPitch;
    const cz = targetPos.z + this.orbitDistance * cosYaw * cosPitch;

    this.camera.position.set(cx, cy, cz);
    this.camera.lookAt(targetPos.x, targetPos.y + 0.5, targetPos.z);
  }

  setPartitionsVisible(visible) {
    if (this.partitionGroup) {
      this.partitionGroup.visible = visible;
    }
  }

  setAoiVisible(visible) {
    if (this.aoiRing) {
      this.aoiRing.visible = visible;
    }
  }

  setWireframe(wireframe) {
    this.voxelMaterial.wireframe = wireframe;
    this.scene.traverse((child) => {
      if (child.isMesh && child.material && child !== this.safeZoneSphere) {
        child.material.wireframe = wireframe;
      }
    });
  }

  render() {
    const now = performance.now();
    if (this.stationDockRing) {
      this.stationDockRing.rotation.z = now * 0.0005;
    }
    if (this.stationCore) {
      this.stationCore.rotation.y = now * 0.001;
    }

    // Animate & expire laser beams
    for (let i = this.laserBeams.length - 1; i >= 0; i--) {
      const beam = this.laserBeams[i];
      if (now >= beam.expiresAt) {
        this.scene.remove(beam.mesh);
        disposeHierarchy(beam.mesh);
        this.laserBeams.splice(i, 1);
      } else if (beam.mesh.material) {
        const remaining = (beam.expiresAt - now) / beam.durationMs;
        beam.mesh.material.opacity = Math.max(0.1, remaining);
      }
    }

    if (this.renderer && this.camera) {
      this.renderer.render(this.scene, this.camera);
    }
  }

  onResize() {
    if (!this.camera || !this.renderer) return;
    this.camera.aspect = window.innerWidth / window.innerHeight;
    this.camera.updateProjectionMatrix();
    this.renderer.setSize(window.innerWidth, window.innerHeight);
  }

  clear() {
    for (const mesh of this.entities.values()) {
      this.scene.remove(mesh);
      disposeHierarchy(mesh);
    }
    this.entities.clear();

    for (const chunkMesh of this.voxelChunkMeshes.values()) {
      this.scene.remove(chunkMesh);
      chunkMesh.geometry?.dispose?.();
    }
    this.voxelChunkMeshes.clear();

    for (const beam of this.laserBeams) {
      this.scene.remove(beam.mesh);
      disposeHierarchy(beam.mesh);
    }
    this.laserBeams.length = 0;
  }

  dispose() {
    this.clear();
    if (typeof window !== "undefined" && this._onResizeBound) {
      window.removeEventListener("resize", this._onResizeBound);
    }
    disposeHierarchy(this.scene);
    this.voxelMaterial?.dispose?.();
    this.renderer?.dispose?.();
    this.renderer?.domElement?.remove?.();
  }
}
