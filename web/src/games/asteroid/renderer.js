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

function resolveBeaconAppearance(props) {
  let outerColor = 0x38bdf8;
  let innerColor = 0x7dd3fc;
  if (props.tier >= 180) {
    outerColor = 0xc084fc; // Void Crystal outer halo
    innerColor = 0xfef08a; // Radiant Solar Gold inner crystal crown
  } else if (props.tier >= 60) {
    outerColor = 0xfacc15; // Solar Gold ring
    innerColor = 0xfde047;
  }
  const ringScale = props.tier > 0 ? 0.88 + props.normalized * 0.48 : 0.01;
  const emissiveIntensity = props.tier > 0 ? 0.65 + props.normalized * 1.45 : 0.0;
  return { outerColor, innerColor, ringScale, emissiveIntensity };
}

function applyBeaconState(group, props) {
  const lootBeaconGroup = group?.userData?.lootBeaconGroup;
  if (!lootBeaconGroup) return;

  const { outerColor, innerColor, ringScale, emissiveIntensity } = resolveBeaconAppearance(props);
  lootBeaconGroup.visible = props.tier > 0;
  if (props.tier > 0) {
    lootBeaconGroup.scale.setScalar(ringScale);
  }

  const updateMat = (mat, hex) => {
    if (!mat) return;
    mat.color?.setHex?.(hex);
    mat.emissive?.setHex?.(hex);
    mat.emissiveIntensity = emissiveIntensity;
  };
  updateMat(group.userData.beaconOuterMat, outerColor);
  updateMat(group.userData.beaconInnerMat, innerColor);
}

function createDeltaWingGeometry() {
  const shape = new THREE.Shape();
  shape.moveTo(0, 0.68);
  shape.lineTo(0.54, 0.45);
  shape.lineTo(2.16, -0.95);
  shape.lineTo(2.04, -1.42);
  shape.lineTo(0.56, -1.16);
  shape.lineTo(0.36, -1.44);
  shape.lineTo(-0.36, -1.44);
  shape.lineTo(-0.56, -1.16);
  shape.lineTo(-2.04, -1.42);
  shape.lineTo(-2.16, -0.95);
  shape.lineTo(-0.54, 0.45);
  shape.closePath();

  const geo = new THREE.ExtrudeGeometry(shape, {
    depth: 0.1,
    bevelEnabled: true,
    bevelThickness: 0.025,
    bevelSize: 0.035,
    bevelSegments: 1,
  });
  geo.rotateX(Math.PI / 2);
  geo.translate(0, 0.05, 0);
  return geo;
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

    const sunLight = new THREE.DirectionalLight(0xfef9c3, 1.75);
    sunLight.position.set(80, 120, 60);
    this.scene.add(sunLight);

    const rimLight = new THREE.DirectionalLight(0x818cf8, 0.75);
    rimLight.position.set(-70, -40, -90);
    this.scene.add(rimLight);

    const bounceLight = new THREE.HemisphereLight(0x93c5fd, 0x0f172a, 0.35);
    this.scene.add(bounceLight);
  }

  initEnvironment() {
    // 1. Deep-space procedural starfield
    const starCount = 2200;
    const starPos = new Float32Array(starCount * 3);
    const starColors = new Float32Array(starCount * 3);
    const palette = [
      new THREE.Color(0xffffff),
      new THREE.Color(0xbae6fd),
      new THREE.Color(0xfef08a),
      new THREE.Color(0xe9d5ff),
      new THREE.Color(0xfecdd3),
    ];

    for (let i = 0; i < starCount; i++) {
      // Deterministic pseudo-random spherical distribution
      const u = ((i * 73856093) % 10007) / 10007;
      const v = ((i * 19349663) % 10007) / 10007;
      const theta = u * Math.PI * 2;
      const phi = Math.acos(2 * v - 1);
      const r = 280 + (i % 260);

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
      size: 1.55,
      vertexColors: true,
      transparent: true,
      opacity: 0.92,
      sizeAttenuation: true,
    });
    this.starfield = new THREE.Points(starGeo, starMat);
    this.scene.add(this.starfield);

    // 2. Home Orbital Station & Safe-Zone Sphere/Ring at (0, 0, 0) radius 15
    this.stationGroup = new THREE.Group();
    this.stationGroup.position.set(HOME_STATION_POS.x, HOME_STATION_POS.y, HOME_STATION_POS.z);

    const alloyMat = new THREE.MeshStandardMaterial({
      color: 0x94a3b8,
      metalness: 0.85,
      roughness: 0.25,
    });
    const darkTrimMat = new THREE.MeshStandardMaterial({
      color: 0x334155,
      metalness: 0.78,
      roughness: 0.35,
    });

    // Central luminous reactor core
    const coreGeo = new THREE.OctahedronGeometry(2.3, 1);
    const coreMat = new THREE.MeshStandardMaterial({
      color: 0x38bdf8,
      emissive: 0x0284c7,
      emissiveIntensity: 0.95,
      metalness: 0.8,
      roughness: 0.18,
    });
    const coreMesh = new THREE.Mesh(coreGeo, coreMat);
    this.stationGroup.add(coreMesh);
    this.stationCore = coreMesh;

    // Reactor containment spire (upper & lower command towers + antennas)
    for (const yDir of [1, -1]) {
      const tower = new THREE.Mesh(new THREE.CylinderGeometry(0.95, 1.65, 2.0, 8), darkTrimMat);
      tower.position.y = yDir * 2.75;
      this.stationGroup.add(tower);

      const cap = new THREE.Mesh(new THREE.CylinderGeometry(0.42, 0.95, 0.9, 8), alloyMat);
      cap.position.y = yDir * 4.15;
      this.stationGroup.add(cap);

      const mast = new THREE.Mesh(new THREE.CylinderGeometry(0.06, 0.14, 2.2, 6), alloyMat);
      mast.position.y = yDir * 5.65;
      this.stationGroup.add(mast);

      const tip = new THREE.Mesh(
        new THREE.SphereGeometry(0.18, 8, 6),
        new THREE.MeshBasicMaterial({ color: 0x22d3ee })
      );
      tip.position.y = yDir * 6.8;
      this.stationGroup.add(tip);
    }

    // Main horizontal docking ring with 4 clamp bay modules (makes ring rotation clearly visible)
    const torusGeo = new THREE.TorusGeometry(5.5, 0.45, 14, 48);
    const torusMat = new THREE.MeshStandardMaterial({
      color: 0x94a3b8,
      metalness: 0.85,
      roughness: 0.25,
      emissive: 0x0369a1,
      emissiveIntensity: 0.35,
    });
    const stationRing = new THREE.Mesh(torusGeo, torusMat);
    stationRing.rotation.x = Math.PI / 2;

    const bayGlowMat = new THREE.MeshStandardMaterial({
      color: 0x38bdf8,
      emissive: 0x0ea5e9,
      emissiveIntensity: 0.9,
    });
    for (let i = 0; i < 4; i++) {
      const angle = (i * Math.PI) / 2;
      const bay = new THREE.Mesh(new THREE.BoxGeometry(1.15, 0.72, 1.05), darkTrimMat);
      bay.position.set(Math.cos(angle) * 5.5, Math.sin(angle) * 5.5, 0);
      bay.rotation.z = angle;
      stationRing.add(bay);

      const bayStrip = new THREE.Mesh(new THREE.BoxGeometry(0.24, 0.42, 1.12), bayGlowMat);
      bayStrip.position.set(Math.cos(angle) * 5.5, Math.sin(angle) * 5.5, 0);
      bayStrip.rotation.z = angle;
      stationRing.add(bayStrip);
    }
    this.stationGroup.add(stationRing);
    this.stationDockRing = stationRing;

    // Counter-rotating upper & lower habitat spokes with pressurized crew pods
    this.stationHabitatSpokes = new THREE.Group();
    const podWindowMat = new THREE.MeshStandardMaterial({
      color: 0xfef08a,
      emissive: 0xf59e0b,
      emissiveIntensity: 0.85,
    });
    for (const yLevel of [1.68, -1.68]) {
      const hubBand = new THREE.Mesh(new THREE.TorusGeometry(4.6, 0.16, 8, 40), alloyMat);
      hubBand.rotation.x = Math.PI / 2;
      hubBand.position.y = yLevel;
      this.stationHabitatSpokes.add(hubBand);

      for (let i = 0; i < 4; i++) {
        const angle = (i * Math.PI) / 2 + Math.PI / 4;
        const spokeGroup = new THREE.Group();
        spokeGroup.rotation.y = angle;
        spokeGroup.position.y = yLevel;

        const spokeBeam = new THREE.Mesh(new THREE.CylinderGeometry(0.14, 0.14, 3.6, 8), darkTrimMat);
        spokeBeam.rotation.z = Math.PI / 2;
        spokeBeam.position.x = 3.1;
        spokeGroup.add(spokeBeam);

        const habPod = new THREE.Mesh(new THREE.BoxGeometry(1.25, 0.62, 0.92), alloyMat);
        habPod.position.x = 4.75;
        spokeGroup.add(habPod);

        const windowStrip = new THREE.Mesh(new THREE.BoxGeometry(1.3, 0.16, 0.65), podWindowMat);
        windowStrip.position.x = 4.75;
        spokeGroup.add(windowStrip);

        this.stationHabitatSpokes.add(spokeGroup);
      }
    }
    this.stationGroup.add(this.stationHabitatSpokes);

    // Articulated east/west solar panel arrays (clear of +-Z spawn/docking lane & inside 15m safe radius)
    this.stationSolarArrays = new THREE.Group();
    const solarCellMat = new THREE.MeshStandardMaterial({
      color: 0x1e3a8a,
      emissive: 0x1d4ed8,
      emissiveIntensity: 0.28,
      metalness: 0.82,
      roughness: 0.18,
    });
    const solarFrameMat = new THREE.MeshStandardMaterial({
      color: 0xd97706,
      metalness: 0.78,
      roughness: 0.3,
    });

    for (const xSign of [1, -1]) {
      const arrayMount = new THREE.Group();
      arrayMount.position.set(0, 2.55, 0);

      // Structural truss boom from tower out to 11.8m
      const boom = new THREE.Mesh(new THREE.CylinderGeometry(0.12, 0.16, 10.6, 8), alloyMat);
      boom.rotation.z = Math.PI / 2;
      boom.position.x = xSign * 6.5;
      arrayMount.add(boom);

      // Dual tilted photovoltaic wings on each side
      for (const zOffset of [-1.05, 1.05]) {
        const wingFrame = new THREE.Mesh(new THREE.BoxGeometry(5.6, 0.08, 1.56), solarFrameMat);
        wingFrame.position.set(xSign * 9.1, 0, zOffset);
        wingFrame.rotation.x = 0.32;
        arrayMount.add(wingFrame);

        const wingCells = new THREE.Mesh(new THREE.BoxGeometry(5.35, 0.1, 1.38), solarCellMat);
        wingCells.position.set(xSign * 9.1, 0, zOffset);
        wingCells.rotation.x = 0.32;
        arrayMount.add(wingCells);
      }
      this.stationSolarArrays.add(arrayMount);
    }
    this.stationGroup.add(this.stationSolarArrays);

    // Docking approach runway guide lights along +-Z corridor
    this.stationDockLights = [];
    const dockZSteps = [6.6, 8.9, 11.2, 13.5];
    for (const zDir of [1, -1]) {
      dockZSteps.forEach((zDist, idx) => {
        for (const xOffset of [-2.1, 2.1]) {
          const pylon = new THREE.Mesh(new THREE.CylinderGeometry(0.08, 0.12, 0.45, 6), darkTrimMat);
          pylon.position.set(xOffset, -1.25, zDir * zDist);
          this.stationGroup.add(pylon);

          const lightMat = new THREE.MeshBasicMaterial({
            color: idx === dockZSteps.length - 1 ? 0xf59e0b : 0x22d3ee,
            transparent: true,
            opacity: 0.85,
          });
          const bulb = new THREE.Mesh(new THREE.SphereGeometry(0.14, 8, 6), lightMat);
          bulb.position.set(xOffset, -0.95, zDir * zDist);
          this.stationGroup.add(bulb);
          this.stationDockLights.push({
            mesh: bulb,
            material: lightMat,
            stepIndex: dockZSteps.length - 1 - idx,
          });
        }
      });
    }

    const beaconLight = new THREE.PointLight(0x38bdf8, 2.5, 45);
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
    this.safeZoneSphere.userData.preserveWireframe = true;
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

    // Animated safe-zone boundary accents (tilted orbital shield bands + perimeter buoys)
    this.safeZoneAccents = new THREE.Group();
    const bandGeo = new THREE.TorusGeometry(HOME_STATION_SAFE_RADIUS, 0.045, 6, 72);
    const bandMat1 = new THREE.MeshBasicMaterial({
      color: 0x38bdf8,
      transparent: true,
      opacity: 0.24,
    });
    const band1 = new THREE.Mesh(bandGeo, bandMat1);
    band1.rotation.set(0.55, 0, 0.35);
    this.safeZoneAccents.add(band1);

    const band2 = new THREE.Mesh(bandGeo.clone(), bandMat1.clone());
    band2.rotation.set(-0.55, 0, -0.35);
    this.safeZoneAccents.add(band2);

    this.safeZoneBuoys = new THREE.Group();
    const buoyMat = new THREE.MeshBasicMaterial({
      color: 0x22d3ee,
      transparent: true,
      opacity: 0.85,
    });
    for (let i = 0; i < 8; i++) {
      const a = (i * Math.PI) / 4;
      const buoy = new THREE.Mesh(new THREE.OctahedronGeometry(0.24, 0), buoyMat);
      buoy.position.set(
        Math.cos(a) * HOME_STATION_SAFE_RADIUS,
        0,
        Math.sin(a) * HOME_STATION_SAFE_RADIUS
      );
      this.safeZoneBuoys.add(buoy);
    }
    this.safeZoneAccents.add(this.safeZoneBuoys);
    this.stationGroup.add(this.safeZoneAccents);

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
    const thrusterHex = isLocalPlayer ? 0x38bdf8 : 0x34d399;

    const hullMat = new THREE.MeshStandardMaterial({
      color: baseColor,
      metalness: props.metalness,
      roughness: props.roughness,
      emissive: props.emissiveColor,
      emissiveIntensity: props.emissiveIntensity,
    });

    const trimMat = new THREE.MeshStandardMaterial({
      color: 0x1e293b,
      metalness: 0.78,
      roughness: 0.35,
    });

    // Forward nose cone (+Z) + mid fuselage + dorsal spine fairing
    const noseGeo = new THREE.ConeGeometry(0.56, 1.9, 6);
    noseGeo.rotateX(Math.PI / 2);
    const noseMesh = new THREE.Mesh(noseGeo, hullMat);
    noseMesh.position.set(0, 0, 0.92);
    group.add(noseMesh);

    const fuselageGeo = new THREE.CylinderGeometry(0.56, 0.76, 1.85, 6);
    fuselageGeo.rotateX(Math.PI / 2);
    const fuselageMesh = new THREE.Mesh(fuselageGeo, hullMat);
    fuselageMesh.position.set(0, -0.02, -0.55);
    group.add(fuselageMesh);

    const spineMesh = new THREE.Mesh(new THREE.BoxGeometry(0.26, 0.22, 1.6), trimMat);
    spineMesh.position.set(0, 0.42, -0.55);
    group.add(spineMesh);

    // Sculpted beveled delta wings
    const wingMesh = new THREE.Mesh(createDeltaWingGeometry(), hullMat);
    group.add(wingMesh);

    // Tinted glass cockpit canopy + metallic frame arches
    const canopyGeo = new THREE.SphereGeometry(0.38, 14, 10);
    const canopyMat = new THREE.MeshStandardMaterial({
      color: 0x38bdf8,
      emissive: 0x0284c7,
      emissiveIntensity: 0.7,
      roughness: 0.08,
      metalness: 0.92,
    });
    const canopy = new THREE.Mesh(canopyGeo, canopyMat);
    canopy.scale.set(0.85, 0.72, 1.48);
    canopy.position.set(0, 0.4, 0.32);
    group.add(canopy);

    const foreAftArch = new THREE.Mesh(
      new THREE.TorusGeometry(0.38, 0.04, 6, 16, Math.PI),
      trimMat
    );
    foreAftArch.rotation.y = Math.PI / 2;
    foreAftArch.scale.set(1.52, 0.76, 0.9);
    foreAftArch.position.set(0, 0.4, 0.32);
    group.add(foreAftArch);

    const crossArch = new THREE.Mesh(
      new THREE.TorusGeometry(0.36, 0.038, 6, 14, Math.PI),
      trimMat
    );
    crossArch.scale.set(0.92, 0.78, 1.0);
    crossArch.position.set(0, 0.4, 0.28);
    group.add(crossArch);

    // Symmetric wingtip fins, dual rear engine nacelles with glowing thruster cones,
    // and forward mining/laser emitter pods
    const thrusterCones = [];
    const emitterCrystalMat = new THREE.MeshBasicMaterial({ color: 0x22d3ee });
    const thrusterCoreMat = new THREE.MeshBasicMaterial({ color: 0xe0f2fe });

    for (const sideSign of [-1, 1]) {
      // Canted wingtip stabilizer fin
      const fin = new THREE.Mesh(new THREE.BoxGeometry(0.08, 0.48, 0.62), hullMat);
      fin.position.set(sideSign * 2.06, 0.22, -1.14);
      fin.rotation.z = -sideSign * 0.34;
      fin.rotation.x = -0.22;
      group.add(fin);

      // Rear engine nacelle + nozzle + plasma cones
      const nacelle = new THREE.Mesh(
        new THREE.CylinderGeometry(0.22, 0.27, 1.25, 10).rotateX(Math.PI / 2),
        trimMat
      );
      nacelle.position.set(sideSign * 0.74, -0.02, -1.04);
      group.add(nacelle);

      const nozzle = new THREE.Mesh(
        new THREE.CylinderGeometry(0.21, 0.16, 0.22, 10).rotateX(Math.PI / 2),
        hullMat
      );
      nozzle.position.set(sideSign * 0.74, -0.02, -1.68);
      group.add(nozzle);

      const coreDisc = new THREE.Mesh(
        new THREE.CylinderGeometry(0.14, 0.14, 0.06, 10).rotateX(Math.PI / 2),
        thrusterCoreMat
      );
      coreDisc.position.set(sideSign * 0.74, -0.02, -1.78);
      group.add(coreDisc);

      const outerConeMat = new THREE.MeshBasicMaterial({
        color: thrusterHex,
        transparent: true,
        opacity: 0.78,
      });
      const outerCone = new THREE.Mesh(
        new THREE.ConeGeometry(0.16, 0.74, 10).rotateX(-Math.PI / 2),
        outerConeMat
      );
      outerCone.position.set(sideSign * 0.74, -0.02, -2.14);
      group.add(outerCone);

      const innerConeMat = new THREE.MeshBasicMaterial({
        color: 0xe0f2fe,
        transparent: true,
        opacity: 0.92,
      });
      const innerCone = new THREE.Mesh(
        new THREE.ConeGeometry(0.085, 0.45, 8).rotateX(-Math.PI / 2),
        innerConeMat
      );
      innerCone.position.set(sideSign * 0.74, -0.02, -2.0);
      group.add(innerCone);
      thrusterCones.push({ outer: outerCone, inner: innerCone });

      // Forward mining / laser emitter pod
      const podBody = new THREE.Mesh(
        new THREE.CylinderGeometry(0.095, 0.125, 0.84, 8).rotateX(Math.PI / 2),
        trimMat
      );
      podBody.position.set(sideSign * 0.74, -0.14, 0.62);
      group.add(podBody);

      const barrel = new THREE.Mesh(
        new THREE.CylinderGeometry(0.055, 0.075, 0.36, 8).rotateX(Math.PI / 2),
        hullMat
      );
      barrel.position.set(sideSign * 0.74, -0.14, 1.16);
      group.add(barrel);

      const emitterTip = new THREE.Mesh(new THREE.SphereGeometry(0.068, 8, 6), emitterCrystalMat);
      emitterTip.position.set(sideSign * 0.74, -0.14, 1.36);
      group.add(emitterTip);
    }

    // Rotating crystal/gold loot-beacon rings scaled by shininessTier
    const lootBeaconGroup = new THREE.Group();
    lootBeaconGroup.position.set(0, 0, -0.1);

    const beaconOuterMat = new THREE.MeshStandardMaterial({
      color: 0x38bdf8,
      emissive: 0x38bdf8,
      emissiveIntensity: 0.9,
      metalness: 0.85,
      roughness: 0.15,
    });
    const beaconRingOuter = new THREE.Mesh(
      new THREE.TorusGeometry(1.72, 0.036, 8, 36),
      beaconOuterMat
    );
    for (let i = 0; i < 4; i++) {
      const a = (i * Math.PI) / 2;
      const crystal = new THREE.Mesh(new THREE.OctahedronGeometry(0.11, 0), beaconOuterMat);
      crystal.position.set(Math.cos(a) * 1.72, Math.sin(a) * 1.72, 0);
      beaconRingOuter.add(crystal);
    }
    lootBeaconGroup.add(beaconRingOuter);

    const beaconInnerMat = new THREE.MeshStandardMaterial({
      color: 0xfacc15,
      emissive: 0xfacc15,
      emissiveIntensity: 0.9,
      metalness: 0.9,
      roughness: 0.12,
    });
    const beaconRingInner = new THREE.Mesh(
      new THREE.TorusGeometry(1.42, 0.028, 6, 30),
      beaconInnerMat
    );
    beaconRingInner.rotation.x = Math.PI / 3;
    for (let i = 0; i < 4; i++) {
      const a = (i * Math.PI) / 2 + Math.PI / 4;
      const stud = new THREE.Mesh(new THREE.OctahedronGeometry(0.09, 0), beaconInnerMat);
      stud.position.set(Math.cos(a) * 1.42, Math.sin(a) * 1.42, 0);
      beaconRingInner.add(stud);
    }
    lootBeaconGroup.add(beaconRingInner);
    group.add(lootBeaconGroup);

    // Loot-shininess translucent aura halo
    const auraGeo = new THREE.SphereGeometry(1.9, 16, 12);
    const auraMat = new THREE.MeshBasicMaterial({
      color: props.auraColor,
      transparent: true,
      opacity: props.tier > 0 ? 0.15 + props.normalized * 0.35 : 0.0,
      wireframe: true,
    });
    const auraHalo = new THREE.Mesh(auraGeo, auraMat);
    auraHalo.userData.preserveWireframe = true;
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
    hpBar.position.y = 2.0;
    group.add(hpBar);

    group.userData.hullMat = hullMat;
    group.userData.auraHalo = auraHalo;
    group.userData.auraLight = auraLight;
    group.userData.hpBar = hpBar;
    group.userData.shininessTier = props.tier;
    group.userData.stateFlags = stateFlags;
    group.userData.thrusterCones = thrusterCones;
    group.userData.lootBeaconGroup = lootBeaconGroup;
    group.userData.beaconRingOuter = beaconRingOuter;
    group.userData.beaconRingInner = beaconRingInner;
    group.userData.beaconOuterMat = beaconOuterMat;
    group.userData.beaconInnerMat = beaconInnerMat;

    applyBeaconState(group, props);

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

    applyBeaconState(group, props);
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
    const now = performance.now();

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
      const explicitBank = isLocal
        ? (localOptions.bankAngle ?? meta?.bankAngle)
        : meta?.bankAngle;
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

      if (explicitBank !== undefined && explicitBank !== null) {
        group.rotation.z = explicitBank;
        group.userData._inferredBank = explicitBank;
      } else if (group.userData._prevYaw !== undefined && group.userData._prevPos) {
        const dtMs = now - group.userData._prevSyncTime;
        if (dtMs >= 1 && dtMs <= 250) {
          const dt = dtMs / 1000;
          let dYaw = yaw - group.userData._prevYaw;
          while (dYaw > Math.PI) dYaw -= Math.PI * 2;
          while (dYaw < -Math.PI) dYaw += Math.PI * 2;

          const dx = x - group.userData._prevPos.x;
          const dz = z - group.userData._prevPos.z;
          const rx = -Math.cos(yaw);
          const rz = Math.sin(yaw);
          const lateralVel = (dx * rx + dz * rz) / dt;
          const yawRate = dYaw / dt;
          const targetBank = Math.max(-0.42, Math.min(0.42, lateralVel * 0.045 - yawRate * 0.18));
          const alpha = 1.0 - Math.exp(-8.0 * dt);
          group.userData._inferredBank =
            (group.userData._inferredBank ?? 0.0) +
            (targetBank - (group.userData._inferredBank ?? 0.0)) * alpha;
          group.rotation.z = group.userData._inferredBank;
        }
      } else {
        group.userData._inferredBank = 0.0;
        group.rotation.z = 0.0;
      }

      group.userData._prevYaw = yaw;
      if (!group.userData._prevPos) {
        group.userData._prevPos = { x, y, z };
      } else {
        group.userData._prevPos.x = x;
        group.userData._prevPos.y = y;
        group.userData._prevPos.z = z;
      }
      group.userData._prevSyncTime = now;

      if (group.userData.hpBar) {
        const clampedHp = Math.max(0.01, Math.min(1.0, hpPct));
        group.userData.hpBar.scale.x = clampedHp;
        if (group.userData.hpBar.material?.color) {
          const hpColor = clampedHp > 0.5 ? 0x22c55e : clampedHp > 0.25 ? 0xf59e0b : 0xef4444;
          group.userData.hpBar.material.color.setHex(hpColor);
        }
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
   * Spawns a transient dual-layer glowing laser beam and target impact flash between `fromPos` and `toPos`.
   */
  spawnLaserBeam(fromPos, toPos, color = 0x38bdf8, durationMs = 240) {
    const start = new THREE.Vector3(fromPos.x, fromPos.y, fromPos.z);
    const end = new THREE.Vector3(toPos.x, toPos.y, toPos.z);
    const delta = new THREE.Vector3().subVectors(end, start);
    const length = delta.length();
    if (length < 0.05) return null;

    const beamGroup = new THREE.Group();
    beamGroup.position.copy(start);
    beamGroup.lookAt(end);

    const coreColor = new THREE.Color(color).lerp(new THREE.Color(0xffffff), 0.72);

    // Bright inner core beam
    const coreGeo = new THREE.CylinderGeometry(0.045, 0.045, length, 8);
    coreGeo.translate(0, length * 0.5, 0);
    coreGeo.rotateX(Math.PI / 2);
    const coreMat = new THREE.MeshBasicMaterial({
      color: coreColor,
      transparent: true,
      opacity: 0.98,
      blending: THREE.AdditiveBlending,
      depthWrite: false,
    });
    const coreMesh = new THREE.Mesh(coreGeo, coreMat);
    beamGroup.add(coreMesh);

    // Translucent outer energy sheath
    const glowGeo = new THREE.CylinderGeometry(0.16, 0.14, length, 10);
    glowGeo.translate(0, length * 0.5, 0);
    glowGeo.rotateX(Math.PI / 2);
    const glowMat = new THREE.MeshBasicMaterial({
      color,
      transparent: true,
      opacity: 0.45,
      blending: THREE.AdditiveBlending,
      depthWrite: false,
    });
    const glowMesh = new THREE.Mesh(glowGeo, glowMat);
    beamGroup.add(glowMesh);

    // Transient target-point impact flash sphere & expanding shockwave ring
    const flashMat = new THREE.MeshBasicMaterial({
      color: coreColor,
      transparent: true,
      opacity: 0.95,
      blending: THREE.AdditiveBlending,
      depthWrite: false,
    });
    const flashMesh = new THREE.Mesh(new THREE.SphereGeometry(0.42, 10, 8), flashMat);
    flashMesh.position.set(0, 0, length);
    beamGroup.add(flashMesh);

    const ringMat = new THREE.MeshBasicMaterial({
      color,
      side: THREE.DoubleSide,
      transparent: true,
      opacity: 0.85,
      blending: THREE.AdditiveBlending,
      depthWrite: false,
    });
    const ringMesh = new THREE.Mesh(new THREE.RingGeometry(0.24, 0.48, 20), ringMat);
    ringMesh.position.set(0, 0, length);
    beamGroup.add(ringMesh);

    beamGroup.userData.coreMesh = coreMesh;
    beamGroup.userData.glowMesh = glowMesh;
    beamGroup.userData.flashMesh = flashMesh;
    beamGroup.userData.ringMesh = ringMesh;

    const safeDurationMs = Math.max(1, Number(durationMs) || 240);
    this.scene.add(beamGroup);
    this.laserBeams.push({
      mesh: beamGroup,
      coreMat,
      glowMat,
      flashMesh,
      flashMat,
      ringMesh,
      ringMat,
      expiresAt: performance.now() + safeDurationMs,
      durationMs: safeDurationMs,
    });
    return beamGroup;
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
      if (
        child.isMesh &&
        child.material &&
        child !== this.safeZoneSphere &&
        !child.userData?.preserveWireframe
      ) {
        child.material.wireframe = wireframe;
      }
    });
  }

  render() {
    const now = performance.now();
    if (this.stationDockRing) {
      this.stationDockRing.rotation.z = now * 0.00045;
    }
    if (this.stationCore) {
      this.stationCore.rotation.y = now * 0.001;
    }
    if (this.stationHabitatSpokes) {
      this.stationHabitatSpokes.rotation.y = -now * 0.00038;
    }
    if (this.safeZoneAccents) {
      this.safeZoneAccents.rotation.y = now * 0.00025;
      if (this.safeZoneAccents.children[0]) {
        this.safeZoneAccents.children[0].rotation.y = now * 0.0004;
      }
      if (this.safeZoneAccents.children[1]) {
        this.safeZoneAccents.children[1].rotation.y = -now * 0.00035;
      }
    }
    if (this.safeZoneBuoys) {
      this.safeZoneBuoys.rotation.y = -now * 0.0005;
    }
    if (this.safeZoneRing?.material) {
      this.safeZoneRing.material.opacity = 0.44 + 0.16 * Math.sin(now * 0.0024);
    }
    if (this.safeZoneSphere?.material) {
      this.safeZoneSphere.material.opacity = 0.08 + 0.035 * Math.sin(now * 0.0018);
    }
    if (this.stationDockLights) {
      for (const light of this.stationDockLights) {
        const wave = 0.5 + 0.5 * Math.sin(now * 0.008 - light.stepIndex * 1.15);
        light.material.opacity = 0.25 + 0.75 * wave;
      }
    }

    // Animate ship loot-beacon rings and engine thruster flames
    let shipIdx = 0;
    for (const group of this.entities.values()) {
      const ud = group.userData;
      if (ud?.lootBeaconGroup?.visible) {
        if (ud.beaconRingOuter) {
          ud.beaconRingOuter.rotation.z = now * 0.0018 + shipIdx;
          ud.beaconRingOuter.rotation.x = Math.sin(now * 0.0009 + shipIdx) * 0.25;
        }
        if (ud.beaconRingInner) {
          ud.beaconRingInner.rotation.y = -now * 0.0024 + shipIdx;
          ud.beaconRingInner.rotation.z = -now * 0.0014;
        }
      }
      if (ud?.thrusterCones) {
        const flicker = 0.9 + 0.15 * Math.sin(now * 0.024 + shipIdx * 1.7);
        for (const cone of ud.thrusterCones) {
          cone.outer.scale.set(1.0, 1.0, flicker);
          cone.inner.scale.set(1.0, 1.0, 0.92 + (flicker - 0.9) * 1.2);
        }
      }
      shipIdx++;
    }

    // Animate & expire dual-layer laser beams + impact flashes
    for (let i = this.laserBeams.length - 1; i >= 0; i--) {
      const beam = this.laserBeams[i];
      if (now >= beam.expiresAt) {
        this.scene.remove(beam.mesh);
        disposeHierarchy(beam.mesh);
        this.laserBeams.splice(i, 1);
      } else {
        const remaining = Math.max(0.0, (beam.expiresAt - now) / beam.durationMs);
        const elapsed = 1.0 - remaining;
        if (beam.coreMat) beam.coreMat.opacity = Math.max(0.12, remaining);
        if (beam.glowMat) beam.glowMat.opacity = Math.max(0.05, remaining * 0.48);
        if (beam.flashMat) beam.flashMat.opacity = Math.max(0.05, remaining * 0.95);
        if (beam.flashMesh) {
          beam.flashMesh.scale.setScalar(0.65 + Math.sin(elapsed * Math.PI) * 0.85);
        }
        if (beam.ringMat) beam.ringMat.opacity = Math.max(0.02, remaining * 0.85);
        if (beam.ringMesh) {
          beam.ringMesh.scale.setScalar(0.8 + elapsed * 1.9);
        }
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
