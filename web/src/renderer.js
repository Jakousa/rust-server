import * as THREE from "three";
import { WorldObjectRenderer } from "./world.js";

// Pre-allocated color constants for per-frame health bar color interpolation.
const HP_COLOR_EMERALD = new THREE.Color(0x22c55e);
const HP_COLOR_AMBER = new THREE.Color(0xf59e0b);
const HP_COLOR_CRIMSON = new THREE.Color(0xef4444);

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
 * Builds an articulated medieval humanoid silhouette inside `bodyGroup` with directional `+Z` facing cues.
 */
function buildHumanoidBody(bodyGroup, palette, options = {}) {
  const {
    hasCloak = false,
    hasBackpack = false,
    hasPauldrons = false,
    hasSatchel = false,
    hasLantern = false,
    hatStyle = "hood",
    hasForwardChevron = false,
  } = options;

  const tunicMat = new THREE.MeshStandardMaterial({
    color: palette.tunic,
    roughness: 0.6,
    metalness: 0.12,
  });
  const trimMat = new THREE.MeshStandardMaterial({
    color: palette.trim,
    roughness: 0.4,
    metalness: 0.45,
  });
  const skinMat = new THREE.MeshStandardMaterial({
    color: 0xf1c7a6,
    roughness: 0.72,
  });
  const bootMat = new THREE.MeshStandardMaterial({
    color: 0x292524,
    roughness: 0.85,
  });
  const hatMat = new THREE.MeshStandardMaterial({
    color: palette.hat,
    roughness: 0.65,
    metalness: 0.15,
  });

  // Boots / lower legs
  const legGeo = new THREE.CylinderGeometry(0.13, 0.15, 0.56, 8);
  for (const side of [-1, 1]) {
    const leg = new THREE.Mesh(legGeo, bootMat);
    leg.position.set(side * 0.17, 0.28, 0);
    leg.castShadow = true;
    bodyGroup.add(leg);
  }

  // Tapered tunic torso + lower skirt hem
  const torso = new THREE.Mesh(new THREE.CylinderGeometry(0.31, 0.42, 0.84, 10), tunicMat);
  torso.position.y = 0.94;
  torso.castShadow = true;
  bodyGroup.add(torso);

  // Waist belt + front +Z buckle cue
  const belt = new THREE.Mesh(new THREE.CylinderGeometry(0.38, 0.40, 0.11, 10), bootMat);
  belt.position.y = 0.74;
  bodyGroup.add(belt);

  const buckle = new THREE.Mesh(new THREE.BoxGeometry(0.14, 0.12, 0.08), trimMat);
  buckle.position.set(0, 0.74, 0.37);
  bodyGroup.add(buckle);

  // Forward collar V-trim facing +Z
  const collar = new THREE.Mesh(new THREE.BoxGeometry(0.24, 0.14, 0.08), trimMat);
  collar.position.set(0, 1.28, 0.28);
  bodyGroup.add(collar);

  // Arms & hands
  const armGeo = new THREE.CylinderGeometry(0.095, 0.085, 0.54, 8);
  const handGeo = new THREE.SphereGeometry(0.085, 8, 8);
  for (const side of [-1, 1]) {
    const arm = new THREE.Mesh(armGeo, tunicMat);
    arm.position.set(side * 0.41, 1.02, 0.04);
    arm.rotation.z = side * -0.18;
    arm.castShadow = true;
    bodyGroup.add(arm);

    const hand = new THREE.Mesh(handGeo, skinMat);
    hand.position.set(side * 0.47, 0.74, 0.07);
    bodyGroup.add(hand);
  }

  // Optional shoulder pauldrons
  if (hasPauldrons) {
    const pauldronGeo = new THREE.SphereGeometry(0.17, 8, 8);
    for (const side of [-1, 1]) {
      const pauldron = new THREE.Mesh(pauldronGeo, trimMat);
      pauldron.scale.set(1.1, 0.75, 1.0);
      pauldron.position.set(side * 0.39, 1.30, 0);
      pauldron.castShadow = true;
      bodyGroup.add(pauldron);
    }
  }

  // Optional rear cloak
  if (hasCloak) {
    const cloakMat = new THREE.MeshStandardMaterial({
      color: palette.cloak,
      roughness: 0.78,
    });
    const cloak = new THREE.Mesh(new THREE.BoxGeometry(0.56, 0.82, 0.10), cloakMat);
    cloak.position.set(0, 0.92, -0.34);
    cloak.rotation.x = 0.14;
    cloak.castShadow = true;
    bodyGroup.add(cloak);
  }

  // Optional traveler backpack
  if (hasBackpack) {
    const packMat = new THREE.MeshStandardMaterial({ color: 0x78350f, roughness: 0.85 });
    const pack = new THREE.Mesh(new THREE.BoxGeometry(0.40, 0.48, 0.22), packMat);
    pack.position.set(0, 1.04, -0.36);
    pack.castShadow = true;
    bodyGroup.add(pack);

    const bedroll = new THREE.Mesh(new THREE.CylinderGeometry(0.08, 0.08, 0.42, 8), trimMat);
    bedroll.rotation.z = Math.PI / 2;
    bedroll.position.set(0, 1.32, -0.36);
    bodyGroup.add(bedroll);
  }

  // Optional merchant leather satchel on right hip
  if (hasSatchel) {
    const satchelMat = new THREE.MeshStandardMaterial({ color: 0x92400e, roughness: 0.82 });
    const satchel = new THREE.Mesh(new THREE.BoxGeometry(0.20, 0.28, 0.22), satchelMat);
    satchel.position.set(0.38, 0.72, 0.04);
    satchel.rotation.z = -0.15;
    satchel.castShadow = true;
    bodyGroup.add(satchel);

    const clasp = new THREE.Mesh(new THREE.BoxGeometry(0.06, 0.06, 0.06), trimMat);
    clasp.position.set(0.47, 0.76, 0.08);
    bodyGroup.add(clasp);
  }

  // Optional handheld warm lantern prop on left hand (+Z forward offset)
  if (hasLantern) {
    const cageMat = new THREE.MeshStandardMaterial({ color: 0x1e293b, metalness: 0.8, roughness: 0.35 });
    const glowMat = new THREE.MeshStandardMaterial({
      color: 0xfbbf24,
      emissive: 0xf59e0b,
      emissiveIntensity: 1.2,
    });
    const rod = new THREE.Mesh(new THREE.CylinderGeometry(0.02, 0.02, 0.16, 6), cageMat);
    rod.position.set(-0.48, 0.66, 0.22);
    bodyGroup.add(rod);

    const lanternBody = new THREE.Mesh(new THREE.BoxGeometry(0.15, 0.19, 0.15), glowMat);
    lanternBody.position.set(-0.48, 0.54, 0.22);
    bodyGroup.add(lanternBody);

    const lanternCap = new THREE.Mesh(new THREE.ConeGeometry(0.12, 0.08, 4), cageMat);
    lanternCap.rotation.y = Math.PI / 4;
    lanternCap.position.set(-0.48, 0.66, 0.22);
    bodyGroup.add(lanternCap);
  }

  // Head & +Z facial directional cue
  const head = new THREE.Mesh(new THREE.SphereGeometry(0.26, 12, 10), skinMat);
  head.position.set(0, 1.62, 0);
  head.castShadow = true;
  bodyGroup.add(head);

  const nose = new THREE.Mesh(new THREE.BoxGeometry(0.07, 0.07, 0.09), skinMat);
  nose.position.set(0, 1.60, 0.25);
  bodyGroup.add(nose);

  const eyeMat = new THREE.MeshBasicMaterial({ color: 0x1e293b });
  for (const side of [-1, 1]) {
    const eye = new THREE.Mesh(new THREE.BoxGeometry(0.045, 0.035, 0.03), eyeMat);
    eye.position.set(side * 0.09, 1.65, 0.235);
    bodyGroup.add(eye);
  }

  // Headgear silhouette by character archetype
  if (hatStyle === "helm") {
    const hood = new THREE.Mesh(
      new THREE.SphereGeometry(0.285, 12, 10, 0, Math.PI * 2, 0, Math.PI * 0.62),
      hatMat
    );
    hood.position.set(0, 1.64, -0.01);
    hood.rotation.x = -0.12;
    bodyGroup.add(hood);

    const visor = new THREE.Mesh(new THREE.BoxGeometry(0.32, 0.055, 0.14), trimMat);
    visor.position.set(0, 1.72, 0.22);
    bodyGroup.add(visor);
  } else if (hatStyle === "traveler") {
    const brim = new THREE.Mesh(new THREE.CylinderGeometry(0.42, 0.44, 0.045, 14), hatMat);
    brim.position.set(0, 1.78, 0);
    bodyGroup.add(brim);

    const crown = new THREE.Mesh(new THREE.ConeGeometry(0.24, 0.34, 10), hatMat);
    crown.position.set(0, 1.94, 0);
    bodyGroup.add(crown);
  } else if (hatStyle === "merchant") {
    const cap = new THREE.Mesh(new THREE.SphereGeometry(0.30, 12, 8), hatMat);
    cap.scale.set(1.12, 0.68, 1.12);
    cap.position.set(0, 1.82, -0.02);
    bodyGroup.add(cap);

    const band = new THREE.Mesh(new THREE.CylinderGeometry(0.28, 0.28, 0.06, 12), trimMat);
    band.position.set(0, 1.74, 0);
    bodyGroup.add(band);
  }

  // Forward directional chevron indicator for local player
  if (hasForwardChevron) {
    const chevronMat = new THREE.MeshBasicMaterial({ color: 0x60a5fa });
    const chevron = new THREE.Mesh(new THREE.ConeGeometry(0.18, 0.42, 4), chevronMat);
    chevron.rotation.x = Math.PI / 2;
    chevron.rotation.z = Math.PI / 4;
    chevron.position.set(0, 1.05, 0.62);
    bodyGroup.add(chevron);
  }
}

/**
 * Three.js 3D Viewport Renderer for OpenSpec MMO (Game A: Village RPG).
 */
export class WorldRenderer {
  constructor(containerElement) {
    this.container = containerElement;
    this.entities = new Map();
    this.lanternHalos = [];
    this.scene = new THREE.Scene();
    this.scene.background = new THREE.Color(0x162032);
    this.scene.fog = new THREE.FogExp2(0x162032, 0.0042);
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
    this.renderer.shadowMap.type = THREE.PCFSoftShadowMap;
    this.renderer.toneMapping = THREE.ACESFilmicToneMapping;
    this.renderer.toneMappingExposure = 1.06;
    this.container.appendChild(this.renderer.domElement);

    // Lighting
    this.initLighting();

    // Environment & Partition Boundaries
    this.initEnvironment();

    // Camera Controls
    this.initCameraControls();

    // Window resize handling
    this._onResizeBound = () => this.onResize();
    window.addEventListener("resize", this._onResizeBound);
  }

  initLighting() {
    const ambientLight = new THREE.AmbientLight(0xffedd5, 0.6);
    this.scene.add(ambientLight);

    const dirLight = new THREE.DirectionalLight(0xffe4bd, 1.55);
    dirLight.position.set(42, 68, 38);
    dirLight.castShadow = true;
    dirLight.shadow.mapSize.set(1024, 1024);
    dirLight.shadow.camera.left = -42;
    dirLight.shadow.camera.right = 42;
    dirLight.shadow.camera.top = 42;
    dirLight.shadow.camera.bottom = -42;
    dirLight.shadow.camera.near = 5;
    dirLight.shadow.camera.far = 160;
    dirLight.shadow.bias = -0.0005;
    this.scene.add(dirLight);

    const rimLight = new THREE.DirectionalLight(0x93c5fd, 0.38);
    rimLight.position.set(-45, 28, -42);
    this.scene.add(rimLight);

    const hemiLight = new THREE.HemisphereLight(0x93c5fd, 0x1e3a24, 0.55);
    this.scene.add(hemiLight);

    // Warm central square lantern fill light above the well
    const plazaWarmLight = new THREE.PointLight(0xffd28a, 0.85, 28, 1.5);
    plazaWarmLight.position.set(0, 4.8, 0);
    this.scene.add(plazaWarmLight);
  }

  initCameraControls() {
    const canvas = this.renderer?.domElement;
    if (!canvas) return;

    const onPointerDown = (e) => {
      if (e.target && e.target.closest && e.target.closest("#hud-overlay, .modal-overlay")) return;
      this.isDragging = true;
      this.previousPointerPosition = { x: e.clientX, y: e.clientY };
      try {
        canvas.setPointerCapture(e.pointerId);
      } catch (_) {}
    };

    const onPointerMove = (e) => {
      if (!this.isDragging) return;
      const deltaX = e.clientX - this.previousPointerPosition.x;
      const deltaY = e.clientY - this.previousPointerPosition.y;
      this.previousPointerPosition = { x: e.clientX, y: e.clientY };

      const sensitivity = 0.005;
      this.orbitYaw += deltaX * sensitivity;
      this.orbitPitch -= deltaY * sensitivity;

      // Clamp pitch to prevent gimbal lock singularities and ground plane clipping
      this.orbitPitch = Math.max(0.05, Math.min(1.45, this.orbitPitch));
    };

    const stopDrag = (e) => {
      if (this.isDragging) {
        this.isDragging = false;
        try {
          if (e && e.pointerId) canvas.releasePointerCapture(e.pointerId);
        } catch (_) {}
      }
    };

    const onWheel = (e) => {
      if (e.target && e.target.closest && e.target.closest("#hud-overlay, .modal-overlay")) return;
      e.preventDefault();
      const zoomFactor = 0.03;
      this.orbitDistance += e.deltaY * zoomFactor;
      this.orbitDistance = Math.max(5.0, Math.min(60.0, this.orbitDistance));
    };

    canvas.addEventListener("pointerdown", onPointerDown);
    canvas.addEventListener("pointermove", onPointerMove);
    canvas.addEventListener("pointerup", stopDrag);
    canvas.addEventListener("pointercancel", stopDrag);
    canvas.addEventListener("wheel", onWheel, { passive: false });

    this._cleanupCameraControls = () => {
      canvas.removeEventListener("pointerdown", onPointerDown);
      canvas.removeEventListener("pointermove", onPointerMove);
      canvas.removeEventListener("pointerup", stopDrag);
      canvas.removeEventListener("pointercancel", stopDrag);
      canvas.removeEventListener("wheel", onWheel);
    };
  }

  initEnvironment() {
    // Coordinate ground grid (hidden by default so the village reads as a polished game world)
    this.gridHelper = new THREE.GridHelper(1000, 100, 0x3b82f6, 0x1e293b);
    this.gridHelper.position.y = 0.005;
    if (this.gridHelper.material) {
      this.gridHelper.material.transparent = true;
      this.gridHelper.material.opacity = 0.16;
    }
    this.gridHelper.visible = false;
    this.scene.add(this.gridHelper);

    // Meadow ground plane + organic grassy tone patches
    const groundGeo = new THREE.PlaneGeometry(1000, 1000);
    const groundMat = new THREE.MeshStandardMaterial({
      color: 0x1b3822,
      roughness: 0.95,
      metalness: 0.02,
    });
    const ground = new THREE.Mesh(groundGeo, groundMat);
    ground.rotation.x = -Math.PI / 2;
    ground.receiveShadow = true;
    this.scene.add(ground);

    const meadowPatchSpecs = [
      [-15, -14, 9.5, 0x224429],
      [16, -12, 8.5, 0x224429],
      [-20, 14, 10.0, 0x162e1c],
      [20, 14, 10.0, 0x224429],
      [0, 24, 11.0, 0x224429],
      [0, -22, 9.0, 0x162e1c],
    ];
    for (const [mx, mz, mr, col] of meadowPatchSpecs) {
      const patch = new THREE.Mesh(
        new THREE.CircleGeometry(mr, 24),
        new THREE.MeshStandardMaterial({ color: col, roughness: 0.95 })
      );
      patch.rotation.x = -Math.PI / 2;
      patch.position.set(mx, 0.004, mz);
      patch.receiveShadow = true;
      this.scene.add(patch);
    }

    this._buildPlazaAndPaths();
    this._buildLanternPosts();
    this._buildSignposts();
    this._buildFlowerAndBushClusters();
    this._buildPerimeterFences();
    this._buildVariedTrees();

    // Partition boundary planes (representing server cluster nodes)
    this.partitionGroup = new THREE.Group();
    const planeGeo = new THREE.PlaneGeometry(200, 40);
    const planeMatX = new THREE.MeshBasicMaterial({
      color: 0x38bdf8,
      transparent: true,
      opacity: 0.05,
      side: THREE.DoubleSide,
      depthWrite: false,
    });

    const planeX = new THREE.Mesh(planeGeo, planeMatX);
    planeX.position.set(0, 20, 0);
    this.partitionGroup.add(planeX);

    const planeZ = new THREE.Mesh(planeGeo, planeMatX.clone());
    planeZ.rotation.y = Math.PI / 2;
    planeZ.position.set(0, 20, 0);
    this.partitionGroup.add(planeZ);

    this.scene.add(this.partitionGroup);

    // AOI Radius Circle (80m radius ring)
    const ringGeo = new THREE.RingGeometry(79.5, 80.0, 64);
    const ringMat = new THREE.MeshBasicMaterial({
      color: 0xf59e0b,
      side: THREE.DoubleSide,
      transparent: true,
      opacity: 0.35,
      depthWrite: false,
    });
    this.aoiRing = new THREE.Mesh(ringGeo, ringMat);
    this.aoiRing.rotation.x = -Math.PI / 2;
    this.aoiRing.position.y = 0.05;
    this.scene.add(this.aoiRing);
  }

  _buildPlazaAndPaths() {
    // Central cobblestone plaza disc
    const plazaMat = new THREE.MeshStandardMaterial({ color: 0x57534e, roughness: 0.88 });
    const plaza = new THREE.Mesh(new THREE.CircleGeometry(14.2, 48), plazaMat);
    plaza.rotation.x = -Math.PI / 2;
    plaza.position.y = 0.01;
    plaza.receiveShadow = true;
    this.scene.add(plaza);

    // Concentric stone rings (well hearth apron, mid-plaza inlay, outer curb)
    const ringSpecs = [
      { inner: 0.0, outer: 3.3, y: 0.015, color: 0x6b6560, segs: 32 },
      { inner: 3.3, outer: 3.7, y: 0.02, color: 0x78716c, segs: 32 },
      { inner: 6.5, outer: 6.9, y: 0.016, color: 0x44403c, segs: 40 },
      { inner: 6.9, outer: 7.45, y: 0.02, color: 0x78716c, segs: 40 },
      { inner: 13.6, outer: 14.3, y: 0.02, color: 0x78716c, segs: 48 },
      { inner: 14.3, outer: 14.8, y: 0.018, color: 0x44403c, segs: 48 },
    ];
    for (const r of ringSpecs) {
      const geo = r.inner === 0.0
        ? new THREE.CircleGeometry(r.outer, r.segs)
        : new THREE.RingGeometry(r.inner, r.outer, r.segs);
      const mesh = new THREE.Mesh(
        geo,
        new THREE.MeshStandardMaterial({ color: r.color, roughness: 0.84 })
      );
      mesh.rotation.x = -Math.PI / 2;
      mesh.position.y = r.y;
      mesh.receiveShadow = true;
      this.scene.add(mesh);
    }

    // Individual radial cobblestone paver accents
    const cobbleShades = [0x6c6660, 0x5c5752, 0x807972].map(
      (c) => new THREE.MeshStandardMaterial({ color: c, roughness: 0.88 })
    );
    const cobbleGeo = new THREE.CylinderGeometry(0.42, 0.46, 0.03, 6);
    const cobbleRings = [
      { radius: 4.9, count: 16 },
      { radius: 9.8, count: 24 },
      { radius: 12.6, count: 30 },
    ];
    let cobbleIdx = 0;
    for (const cr of cobbleRings) {
      for (let i = 0; i < cr.count; i++) {
        const a = (i / cr.count) * Math.PI * 2 + (cr.radius * 0.1);
        const stone = new THREE.Mesh(cobbleGeo, cobbleShades[cobbleIdx % cobbleShades.length]);
        stone.position.set(Math.cos(a) * cr.radius, 0.015, Math.sin(a) * cr.radius);
        stone.rotation.y = a;
        stone.scale.set(1.0 + (cobbleIdx % 3) * 0.15, 1, 0.8 + (cobbleIdx % 2) * 0.2);
        stone.receiveShadow = true;
        this.scene.add(stone);
        cobbleIdx++;
      }
    }

    // Radial dirt & flagstone pathways to buildings and south road
    const pathMat = new THREE.MeshStandardMaterial({ color: 0x6b4f35, roughness: 0.92 });
    const createPath = (x1, z1, x2, z2, width = 3.2) => {
      const dx = x2 - x1;
      const dz = z2 - z1;
      const length = Math.hypot(dx, dz);
      const angle = Math.atan2(dx, dz);
      const path = new THREE.Mesh(new THREE.PlaneGeometry(width, length), pathMat);
      path.rotation.x = -Math.PI / 2;
      path.rotation.z = -angle;
      path.position.set((x1 + x2) / 2, 0.013, (z1 + z2) / 2);
      path.receiveShadow = true;
      this.scene.add(path);

      // Stepping flagstones along pathway center
      const steps = Math.max(2, Math.floor(length / 2.6));
      for (let s = 1; s < steps; s++) {
        const t = s / steps;
        const sx = x1 + dx * t + (s % 2 === 0 ? 0.25 : -0.25);
        const sz = z1 + dz * t;
        const flagstone = new THREE.Mesh(cobbleGeo, cobbleShades[s % cobbleShades.length]);
        flagstone.position.set(sx, 0.017, sz);
        flagstone.scale.set(1.3, 1, 1.0);
        flagstone.receiveShadow = true;
        this.scene.add(flagstone);
      }
    };

    createPath(0, 0, -12, -8);
    createPath(0, 0, 12, -8);
    createPath(0, 0, -18, 10);
    createPath(0, 0, 18, 10);
    createPath(0, 14, 0, 28, 3.6);
  }

  _buildLanternPosts() {
    const lanternPositions = [
      [7.5, 7.5],
      [-7.5, 7.5],
      [7.5, -7.5],
      [-7.5, -7.5],
      [-3.6, 15.2],
      [3.6, 15.2],
    ];
    const plinthMat = new THREE.MeshStandardMaterial({ color: 0x57534e, roughness: 0.9 });
    const postMat = new THREE.MeshStandardMaterial({ color: 0x451a03, roughness: 0.84 });
    const ironMat = new THREE.MeshStandardMaterial({ color: 0x1e293b, metalness: 0.8, roughness: 0.35 });

    lanternPositions.forEach(([lx, lz], idx) => {
      const lanternGroup = new THREE.Group();
      lanternGroup.position.set(lx, 0, lz);

      const plinth = new THREE.Mesh(new THREE.CylinderGeometry(0.24, 0.28, 0.28, 8), plinthMat);
      plinth.position.y = 0.14;
      plinth.castShadow = true;
      lanternGroup.add(plinth);

      const post = new THREE.Mesh(new THREE.CylinderGeometry(0.10, 0.13, 2.75, 8), postMat);
      post.position.y = 1.52;
      post.castShadow = true;
      lanternGroup.add(post);

      // Bracket arm pointing toward plaza center
      const inwardAngle = Math.atan2(-lx, -lz);
      const armPivot = new THREE.Group();
      armPivot.rotation.y = inwardAngle;
      armPivot.position.y = 2.82;

      const arm = new THREE.Mesh(new THREE.BoxGeometry(0.08, 0.08, 0.56), postMat);
      arm.position.set(0, 0, 0.24);
      armPivot.add(arm);

      const lampCoreMat = new THREE.MeshStandardMaterial({
        color: 0xfbbf24,
        emissive: 0xf59e0b,
        emissiveIntensity: 1.25,
      });
      const lamp = new THREE.Mesh(new THREE.BoxGeometry(0.28, 0.38, 0.28), lampCoreMat);
      lamp.position.set(0, -0.24, 0.45);
      armPivot.add(lamp);

      const cap = new THREE.Mesh(new THREE.ConeGeometry(0.24, 0.16, 4), ironMat);
      cap.rotation.y = Math.PI / 4;
      cap.position.set(0, -0.01, 0.45);
      armPivot.add(cap);

      // Translucent warm lantern halo glow
      const haloMat = new THREE.MeshBasicMaterial({
        color: 0xfde68a,
        transparent: true,
        opacity: 0.24,
        depthWrite: false,
      });
      const halo = new THREE.Mesh(new THREE.SphereGeometry(0.68, 14, 10), haloMat);
      halo.position.set(0, -0.24, 0.45);
      armPivot.add(halo);

      // Soft warm ground light-pool disc
      const pool = new THREE.Mesh(
        new THREE.CircleGeometry(2.3, 18),
        new THREE.MeshBasicMaterial({
          color: 0xfbbf24,
          transparent: true,
          opacity: 0.08,
          depthWrite: false,
        })
      );
      pool.rotation.x = -Math.PI / 2;
      pool.position.set(0, -2.795, 0.45);
      armPivot.add(pool);

      const light = new THREE.PointLight(0xffd28a, 1.35, 16, 1.6);
      light.position.set(0, -0.24, 0.45);
      armPivot.add(light);

      this.lanternHalos.push({
        halo,
        light,
        baseOpacity: 0.24,
        baseIntensity: 1.35,
        phase: idx * 1.37,
      });

      lanternGroup.add(armPivot);
      this.scene.add(lanternGroup);
    });
  }

  _buildSignposts() {
    const postMat = new THREE.MeshStandardMaterial({ color: 0x5c3a1e, roughness: 0.86 });
    const plankMat = new THREE.MeshStandardMaterial({ color: 0x92400e, roughness: 0.78 });
    const trimMat = new THREE.MeshStandardMaterial({ color: 0xfde68a, roughness: 0.65 });
    const stoneMat = new THREE.MeshStandardMaterial({ color: 0x64748b, roughness: 0.9 });

    const signposts = [
      { x: -5.4, z: -1.6, arms: [{ y: 1.75, rotY: 0.65 }, { y: 1.42, rotY: -2.4 }] },
      { x: 5.4, z: -1.6, arms: [{ y: 1.75, rotY: -0.65 }, { y: 1.42, rotY: 2.4 }] },
      { x: -3.9, z: 12.6, arms: [{ y: 1.75, rotY: 0.2 }, { y: 1.42, rotY: -1.3 }] },
    ];

    for (const sp of signposts) {
      const group = new THREE.Group();
      group.position.set(sp.x, 0, sp.z);

      const base = new THREE.Mesh(new THREE.CylinderGeometry(0.18, 0.22, 0.20, 8), stoneMat);
      base.position.y = 0.10;
      group.add(base);

      const pole = new THREE.Mesh(new THREE.CylinderGeometry(0.08, 0.10, 2.05, 8), postMat);
      pole.position.y = 1.05;
      pole.castShadow = true;
      group.add(pole);

      const cap = new THREE.Mesh(new THREE.ConeGeometry(0.18, 0.14, 4), postMat);
      cap.rotation.y = Math.PI / 4;
      cap.position.y = 2.12;
      group.add(cap);

      for (const arm of sp.arms) {
        const boardGroup = new THREE.Group();
        boardGroup.position.y = arm.y;
        boardGroup.rotation.y = arm.rotY;

        const board = new THREE.Mesh(new THREE.BoxGeometry(0.85, 0.22, 0.06), plankMat);
        board.position.x = 0.34;
        board.castShadow = true;
        boardGroup.add(board);

        const tip = new THREE.Mesh(new THREE.ConeGeometry(0.15, 0.18, 4), plankMat);
        tip.rotation.z = -Math.PI / 2;
        tip.rotation.x = Math.PI / 4;
        tip.position.x = 0.85;
        boardGroup.add(tip);

        const labelStrip = new THREE.Mesh(new THREE.BoxGeometry(0.62, 0.05, 0.07), trimMat);
        labelStrip.position.x = 0.34;
        boardGroup.add(labelStrip);

        group.add(boardGroup);
      }

      this.scene.add(group);
    }
  }

  _buildFlowerAndBushClusters() {
    const bushShades = [0x15803d, 0x166534, 0x14532d].map(
      (c) => new THREE.MeshStandardMaterial({ color: c, roughness: 0.88 })
    );
    const flowerColors = [0xfacc15, 0xf472b6, 0x60a5fa, 0xfb923c].map(
      (c) => new THREE.MeshStandardMaterial({ color: c, roughness: 0.65 })
    );

    const clusterCoords = [
      [-15.5, -6.5], [-14.8, -11.5], [-9.2, -11.5],
      [15.2, -6.8], [14.6, -11.4], [9.2, -11.4],
      [-21.8, 7.6], [-14.4, 13.4], [-21.4, 13.2],
      [21.8, 7.6], [14.4, 13.4], [21.4, 13.2],
      [-11.8, 1.2], [11.8, 1.2], [-4.8, 14.4], [4.8, 14.4],
      [-3.4, -13.6], [3.4, -13.6],
    ];

    clusterCoords.forEach(([cx, cz], idx) => {
      const cluster = new THREE.Group();
      cluster.position.set(cx, 0, cz);

      for (let b = 0; b < 3; b++) {
        const radius = 0.36 + ((idx + b) % 3) * 0.11;
        const puff = new THREE.Mesh(
          new THREE.DodecahedronGeometry(radius, 0),
          bushShades[(idx + b) % bushShades.length]
        );
        const angle = (b / 3) * Math.PI * 2 + idx;
        puff.position.set(Math.cos(angle) * 0.28, radius * 0.72, Math.sin(angle) * 0.28);
        puff.castShadow = true;
        cluster.add(puff);
      }

      for (let f = 0; f < 5; f++) {
        const fa = (f / 5) * Math.PI * 2 + idx * 0.7;
        const fr = 0.22 + (f % 3) * 0.16;
        const blossom = new THREE.Mesh(
          new THREE.SphereGeometry(0.08, 6, 6),
          flowerColors[(idx + f) % flowerColors.length]
        );
        blossom.position.set(Math.cos(fa) * fr, 0.36 + (f % 2) * 0.12, Math.sin(fa) * fr);
        cluster.add(blossom);
      }

      this.scene.add(cluster);
    });
  }

  _buildPerimeterFences() {
    const woodMat = new THREE.MeshStandardMaterial({ color: 0x6b3e1b, roughness: 0.88 });
    const postGeo = new THREE.BoxGeometry(0.18, 0.95, 0.18);

    const addFenceArc = (points) => {
      for (let i = 0; i < points.length - 1; i++) {
        const [x1, z1] = points[i];
        const [x2, z2] = points[i + 1];
        const dx = x2 - x1;
        const dz = z2 - z1;
        const dist = Math.hypot(dx, dz);
        const angle = Math.atan2(dx, dz);
        const postsCount = Math.max(2, Math.round(dist / 2.6));

        for (let p = 0; p <= postsCount; p++) {
          if (i > 0 && p === 0) continue;
          const t = p / postsCount;
          const post = new THREE.Mesh(postGeo, woodMat);
          post.position.set(x1 + dx * t, 0.475, z1 + dz * t);
          post.castShadow = true;
          this.scene.add(post);
        }

        const railGeo = new THREE.BoxGeometry(0.08, 0.09, dist);
        for (const ry of [0.38, 0.72]) {
          const rail = new THREE.Mesh(railGeo, woodMat);
          rail.position.set((x1 + x2) / 2, ry, (z1 + z2) / 2);
          rail.rotation.y = angle;
          rail.castShadow = true;
          this.scene.add(rail);
        }
      }
    };

    addFenceArc([
      [-4.6, 22.5], [-12.5, 22.0], [-20.5, 18.5], [-25.0, 12.0],
      [-25.5, 2.0], [-23.0, -9.0], [-18.0, -17.0], [-9.5, -21.5], [-3.6, -22.5],
    ]);
    addFenceArc([
      [4.6, 22.5], [12.5, 22.0], [20.5, 18.5], [25.0, 12.0],
      [25.5, 2.0], [23.0, -9.0], [18.0, -17.0], [9.5, -21.5], [3.6, -22.5],
    ]);
  }

  _buildVariedTrees() {
    const trunkMat = new THREE.MeshStandardMaterial({ color: 0x3d2817, roughness: 0.9 });
    const pineMats = [0x14532d, 0x166534, 0x114726].map(
      (c) => new THREE.MeshStandardMaterial({ color: c, roughness: 0.84 })
    );
    const oakMats = [0x15803d, 0x3f6212, 0x166534].map(
      (c) => new THREE.MeshStandardMaterial({ color: c, roughness: 0.82 })
    );

    const treePositions = [
      [-28, -20], [-34, -5], [-32, 18], [-24, 28],
      [28, -20], [35, -4], [30, 20], [22, 28],
      [-10, -28], [10, -28], [-9, 31], [9, 31],
      [-29, 6], [29, 6], [-22, -26], [22, -26],
      [-35, -16], [35, 12],
    ];

    treePositions.forEach(([tx, tz], idx) => {
      const treeGroup = new THREE.Group();
      const scale = 0.88 + (idx % 4) * 0.14;
      const isOak = idx % 3 === 1;

      if (isOak) {
        const trunk = new THREE.Mesh(new THREE.CylinderGeometry(0.32, 0.50, 2.1, 8), trunkMat);
        trunk.position.y = 1.05;
        trunk.castShadow = true;
        treeGroup.add(trunk);

        const canopyOffsets = [
          [0, 3.0, 0, 1.55],
          [-0.95, 2.55, 0.55, 1.15],
          [0.95, 2.6, -0.45, 1.18],
          [0.2, 2.7, 0.9, 1.05],
        ];
        canopyOffsets.forEach(([cx, cy, cz, cr], cIdx) => {
          const puff = new THREE.Mesh(
            new THREE.DodecahedronGeometry(cr, 1),
            oakMats[(idx + cIdx) % oakMats.length]
          );
          puff.position.set(cx, cy, cz);
          puff.castShadow = true;
          treeGroup.add(puff);
        });
      } else {
        const trunk = new THREE.Mesh(new THREE.CylinderGeometry(0.28, 0.45, 1.8, 8), trunkMat);
        trunk.position.y = 0.9;
        trunk.castShadow = true;
        treeGroup.add(trunk);

        const tiers = [
          { r: 2.15, h: 2.4, y: 2.35 },
          { r: 1.65, h: 2.1, y: 3.45 },
          { r: 1.10, h: 1.75, y: 4.50 },
        ];
        tiers.forEach((tier, tIdx) => {
          const cone = new THREE.Mesh(
            new THREE.ConeGeometry(tier.r, tier.h, 8),
            pineMats[(idx + tIdx) % pineMats.length]
          );
          cone.position.y = tier.y;
          cone.rotation.y = tIdx * 0.45;
          cone.castShadow = true;
          treeGroup.add(cone);
        });
      }

      treeGroup.position.set(tx, 0, tz);
      treeGroup.rotation.y = idx * 0.9;
      treeGroup.scale.setScalar(scale);
      this.scene.add(treeGroup);
    });
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
        group.userData.bobPhase = Number(guid % 997n) * 0.13;
        this.entities.set(guid, group);
        this.scene.add(group);
      }

      // Update position & rotation
      group.position.set(x, y, z);
      group.rotation.y = yaw;

      // Update health bar scale and color-shift (emerald -> amber -> crimson)
      if (group.userData.hpBar) {
        const clampedHp = Math.max(0.0, Math.min(1.0, Number.isFinite(hpPct) ? hpPct : 1.0));
        group.userData.hpBar.scale.x = Math.max(0.01, clampedHp);

        const hpMat = group.userData.hpBar.material;
        if (hpMat && hpMat.color) {
          if (clampedHp >= 0.6) {
            const t = (clampedHp - 0.6) / 0.4;
            hpMat.color.lerpColors(HP_COLOR_AMBER, HP_COLOR_EMERALD, t);
          } else if (clampedHp >= 0.25) {
            const t = (clampedHp - 0.25) / 0.35;
            hpMat.color.lerpColors(HP_COLOR_CRIMSON, HP_COLOR_AMBER, t);
          } else {
            hpMat.color.copy(HP_COLOR_CRIMSON);
          }
        }
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
   * Factory creating articulated character meshes based on entity type.
   * Safe to invoke directly on `WorldRenderer.prototype` without an instance.
   */
  createEntityMesh(entityType, isLocalPlayer) {
    const group = new THREE.Group();
    const bodyGroup = new THREE.Group();
    group.add(bodyGroup);
    group.userData.bodyGroup = bodyGroup;

    // Ground contact shadow disc
    const shadow = new THREE.Mesh(
      new THREE.CircleGeometry(0.65, 20),
      new THREE.MeshBasicMaterial({
        color: 0x05080f,
        transparent: true,
        opacity: 0.45,
        depthWrite: false,
      })
    );
    shadow.rotation.x = -Math.PI / 2;
    shadow.position.y = 0.015;
    group.add(shadow);

    let ringColor = 0x10b981;
    let ringInner = 0.68;
    let ringOuter = 0.78;
    let ringOpacity = 0.45;

    if (isLocalPlayer) {
      buildHumanoidBody(
        bodyGroup,
        { tunic: 0x2563eb, trim: 0x93c5fd, cloak: 0x1d4ed8, hat: 0x1e40af },
        { hasCloak: true, hasPauldrons: true, hatStyle: "helm", hasForwardChevron: true }
      );
      ringColor = 0x38bdf8;
      ringInner = 0.72;
      ringOuter = 0.86;
      ringOpacity = 0.78;

      // Ground directional facing triangle pointer on local selection ring
      const dirTip = new THREE.Mesh(
        new THREE.ConeGeometry(0.18, 0.34, 3),
        new THREE.MeshBasicMaterial({ color: 0x60a5fa })
      );
      dirTip.rotation.x = Math.PI / 2;
      dirTip.position.set(0, 0.03, 0.98);
      group.add(dirTip);
    } else if (entityType === 1) {
      buildHumanoidBody(
        bodyGroup,
        { tunic: 0x10b981, trim: 0xd97706, hat: 0x065f46 },
        { hasBackpack: true, hasPauldrons: true, hatStyle: "traveler" }
      );
      ringColor = 0x10b981;
    } else if (entityType === 2) {
      // Villager NPC: purple merchant robe, satchel, handheld lantern, warm aura ring & golden marker
      buildHumanoidBody(
        bodyGroup,
        { tunic: 0x8b5cf6, trim: 0xf59e0b, hat: 0x6d28d9 },
        { hasSatchel: true, hasLantern: true, hatStyle: "merchant" }
      );
      ringColor = 0xf59e0b;
      ringInner = 0.82;
      ringOuter = 0.98;
      ringOpacity = 0.65;

      // Floating golden merchant diamond marker
      const markerGeo = new THREE.OctahedronGeometry(0.32);
      const markerMat = new THREE.MeshStandardMaterial({
        color: 0xf59e0b,
        metalness: 0.8,
        roughness: 0.2,
        emissive: 0xd97706,
        emissiveIntensity: 0.55,
      });
      const marker = new THREE.Mesh(markerGeo, markerMat);
      marker.position.y = 3.25;
      group.add(marker);
      group.userData.marker = marker;
    } else {
      // Hostile Mob / Creature: armored crimson golem/raider silhouette
      const armorMat = new THREE.MeshStandardMaterial({ color: 0xef4444, roughness: 0.5, metalness: 0.25 });
      const darkIronMat = new THREE.MeshStandardMaterial({ color: 0x450a0a, roughness: 0.6, metalness: 0.4 });
      const eyeMat = new THREE.MeshBasicMaterial({ color: 0xfbbf24 });

      const core = new THREE.Mesh(new THREE.BoxGeometry(1.1, 1.1, 0.9), armorMat);
      core.position.y = 0.95;
      core.castShadow = true;
      bodyGroup.add(core);

      const helm = new THREE.Mesh(new THREE.BoxGeometry(0.62, 0.52, 0.62), darkIronMat);
      helm.position.set(0, 1.72, 0.05);
      bodyGroup.add(helm);

      for (const side of [-1, 1]) {
        const horn = new THREE.Mesh(new THREE.ConeGeometry(0.10, 0.36, 6), darkIronMat);
        horn.position.set(side * 0.28, 2.05, 0.05);
        horn.rotation.z = side * -0.4;
        bodyGroup.add(horn);

        const eye = new THREE.Mesh(new THREE.BoxGeometry(0.11, 0.07, 0.05), eyeMat);
        eye.position.set(side * 0.14, 1.74, 0.36);
        bodyGroup.add(eye);
      }

      ringColor = 0xef4444;
    }

    // Ground selection / interaction aura ring
    const groundRing = new THREE.Mesh(
      new THREE.RingGeometry(ringInner, ringOuter, 28),
      new THREE.MeshBasicMaterial({
        color: ringColor,
        side: THREE.DoubleSide,
        transparent: true,
        opacity: ringOpacity,
        depthWrite: false,
      })
    );
    groundRing.rotation.x = -Math.PI / 2;
    groundRing.position.y = 0.022;
    group.add(groundRing);
    if (entityType === 2 && !isLocalPlayer) {
      group.userData.auraRing = groundRing;
    }

    // Floating overhead health bar + backing frame
    const bgBar = new THREE.Mesh(
      new THREE.BoxGeometry(1.44, 0.16, 0.03),
      new THREE.MeshBasicMaterial({ color: 0x0f172a })
    );
    bgBar.position.set(0, 2.72, -0.02);
    group.add(bgBar);

    const barGeo = new THREE.BoxGeometry(1.4, 0.12, 0.05);
    const barMat = new THREE.MeshBasicMaterial({ color: 0x22c55e });
    const hpBar = new THREE.Mesh(barGeo, barMat);
    hpBar.position.y = 2.72;
    group.add(hpBar);
    group.userData.hpBar = hpBar;

    return group;
  }

  /**
   * Follows local player position with interactive spherical orbit camera.
   */
  updateCamera(targetPos) {
    if (!targetPos || !this.camera) return;

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
    if (this.partitionGroup) {
      this.partitionGroup.visible = Boolean(visible);
    }
  }

  setGridVisible(visible) {
    if (this.gridHelper) {
      this.gridHelper.visible = Boolean(visible);
    }
  }

  setAoiVisible(visible) {
    if (this.aoiRing) {
      this.aoiRing.visible = Boolean(visible);
    }
  }

  setWireframe(wireframe) {
    this.scene.traverse((child) => {
      if (child.isMesh && child.material) {
        if (Array.isArray(child.material)) {
          child.material.forEach((m) => {
            if (m) m.wireframe = wireframe;
          });
        } else {
          child.material.wireframe = wireframe;
        }
      }
    });
  }

  render() {
    const now = performance.now();
    const angle = now * 0.002;

    // Animate smooth idle bobbing, NPC interaction aura pulses, and rotating merchant markers
    for (const group of this.entities.values()) {
      const phase = group.userData.bobPhase || 0;
      if (group.userData.bodyGroup) {
        group.userData.bodyGroup.position.y = Math.sin(now * 0.0032 + phase) * 0.055;
      }
      if (group.userData.marker) {
        group.userData.marker.rotation.y = angle;
        group.userData.marker.position.y = 3.25 + Math.sin(now * 0.003 + phase) * 0.09;
      }
      if (group.userData.auraRing && group.userData.auraRing.material) {
        group.userData.auraRing.material.opacity = 0.52 + 0.18 * Math.sin(now * 0.004 + phase);
      }
    }

    // Subtle warm candle flicker for village plaza lanterns
    for (const entry of this.lanternHalos) {
      const wave = Math.sin(now * 0.005 + entry.phase) * 0.5 + Math.sin(now * 0.011 + entry.phase) * 0.5;
      if (entry.halo?.material) {
        entry.halo.material.opacity = entry.baseOpacity + wave * 0.04;
      }
      if (entry.light) {
        entry.light.intensity = entry.baseIntensity + wave * 0.15;
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

  /**
   * Releases all event listeners, scene GPU resources, and the WebGL renderer.
   */
  dispose() {
    this.clear();
    if (typeof window !== "undefined" && this._onResizeBound) {
      window.removeEventListener("resize", this._onResizeBound);
    }
    this._cleanupCameraControls?.();
    this._cleanupCameraControls = null;
    this.lanternHalos.length = 0;
    disposeHierarchy(this.scene);
    this.renderer?.dispose?.();
    this.renderer?.domElement?.remove?.();
  }
}
