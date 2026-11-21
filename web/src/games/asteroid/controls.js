const KEY_BINDINGS = {
  w: "w",
  a: "a",
  s: "s",
  d: "d",
  q: "q",
  e: "e",
  r: "r",
  f: "f",
  c: "c",
  j: "j",
  x: "x",
  b: "b",
  arrowup: "arrowUp",
  arrowdown: "arrowDown",
  arrowleft: "arrowLeft",
  arrowright: "arrowRight",
};

const CODE_BINDINGS = {
  ArrowUp: "arrowUp",
  ArrowDown: "arrowDown",
  ArrowLeft: "arrowLeft",
  ArrowRight: "arrowRight",
  Space: "space",
  ShiftLeft: "shift",
  ShiftRight: "shift",
};

function isEditableEventTarget(target) {
  const tag = typeof target?.tagName === "string" ? target.tagName.toUpperCase() : "";
  return (
    tag === "INPUT" ||
    tag === "TEXTAREA" ||
    tag === "SELECT" ||
    Boolean(target?.isContentEditable)
  );
}

/**
 * 6DoF Newtonian space flight controller for Game B (Asteroid Miners).
 * Supports 3D thrust along yaw/pitch, lateral strafing, vertical thrust,
 * velocity damping, and mining/PvP laser trigger hooks while respecting server
 * kinematic speed and acceleration caps.
 */
export class SpaceFlightControls {
  constructor(options = {}) {
    this.keys = {
      w: false,
      a: false,
      s: false,
      d: false,
      q: false,
      e: false,
      r: false,
      f: false,
      c: false,
      j: false,
      x: false,
      b: false,
      arrowUp: false,
      arrowDown: false,
      arrowLeft: false,
      arrowRight: false,
      space: false,
      shift: false,
    };

    this.position = {
      x: options.x ?? 0.0,
      y: options.y ?? 0.0,
      z: options.z ?? 8.0,
    };
    this.velocity = { x: 0.0, y: 0.0, z: 0.0 };
    // Default yaw PI faces -Z toward Home Orbital Station (0,0,0) and asteroid field
    this.yaw = options.yaw ?? Math.PI;
    this.pitch = options.pitch ?? 0.0;

    // Comfortable headroom below server DEFAULT_MAX_SPEED (10.0 m/s) and max_acceleration (250.0 m/s^2)
    this.maxSpeed = options.maxSpeed ?? 8.5;
    this.thrustAcceleration = options.thrustAcceleration ?? 28.0;
    this.linearDamping = options.linearDamping ?? 2.0;
    this.turnSpeed = options.turnSpeed ?? 2.4;
    this.pitchSpeed = options.pitchSpeed ?? 1.8;
    this.bankAngle = 0.0;
    this.maxBankAngle = options.maxBankAngle ?? 0.42;
    this.bankSmoothRate = options.bankSmoothRate ?? 8.0;
    this.sequence = 1;

    this.onMineCallback = null;
    this.onFireLaserCallback = null;
    this.onBankCallback = null;
    this.autopilotTarget = null;
    this.enabled = true;

    if (typeof window !== "undefined") {
      this.initListeners();
    }
  }

  /**
   * Registers a callback invoked when the player triggers the mining laser via keyboard.
   */
  onMine(callback) {
    this.onMineCallback = callback;
  }

  /**
   * Registers a callback invoked when the player triggers the PvP laser via keyboard.
   */
  onFireLaser(callback) {
    this.onFireLaserCallback = callback;
  }

  /**
   * Registers a callback invoked when the player presses `B` to dock & bank cargo.
   */
  onBank(callback) {
    this.onBankCallback = callback;
  }

  /**
   * Zeroes linear velocity, levels pitch, and cancels any active autopilot target.
   */
  brake() {
    this.velocity = { x: 0.0, y: 0.0, z: 0.0 };
    this.pitch = 0.0;
    this.autopilotTarget = null;
  }

  /**
   * Orients ship nose toward `targetPos` and optionally engages gentle approach autopilot.
   */
  orientToward(targetPos, engageAutopilot = false, stopDistance = 10.0) {
    if (!targetPos) return;
    const dx = targetPos.x - this.position.x;
    const dy = targetPos.y - this.position.y;
    const dz = targetPos.z - this.position.z;
    const horiz = Math.hypot(dx, dz);
    const total = Math.hypot(dx, dy, dz);
    if (total < 1e-4) return;
    this.yaw = Math.atan2(dx, dz);
    this.pitch = Math.max(-1.45, Math.min(1.45, Math.atan2(dy, Math.max(1e-4, horiz))));
    if (engageAutopilot) {
      this.autopilotTarget = {
        x: targetPos.x,
        y: targetPos.y,
        z: targetPos.z,
        stopDistance: Math.max(2.0, Number(stopDistance) || 10.0),
      };
    }
  }

  /**
   * Updates key state from DOM keyboard events, ignoring editable input elements on keydown.
   */
  setKeyState(e, pressed) {
    if (pressed && isEditableEventTarget(e?.target)) {
      return;
    }

    const key = typeof e?.key === "string" ? e.key.toLowerCase() : "";
    const bound = KEY_BINDINGS[key] || CODE_BINDINGS[e?.code];
    if (bound && Object.prototype.hasOwnProperty.call(this.keys, bound)) {
      this.keys[bound] = pressed;
      if (
        pressed &&
        bound !== "j" &&
        bound !== "b" &&
        bound !== "x"
      ) {
        this.autopilotTarget = null;
      }
    }

    if (pressed && this.enabled) {
      if ((key === "j" || e?.code === "KeyJ") && !e?.repeat && this.onMineCallback) {
        this.onMineCallback();
      } else if (e?.code === "Enter" && !e?.repeat && this.onFireLaserCallback) {
        this.onFireLaserCallback();
      } else if ((key === "b" || e?.code === "KeyB") && !e?.repeat && this.onBankCallback) {
        this.onBankCallback();
      } else if ((key === "x" || e?.code === "KeyX") && !e?.repeat) {
        this.brake();
      }
    }
  }

  initListeners() {
    window.addEventListener("keydown", (e) => {
      this.setKeyState(e, true);
    });

    window.addEventListener("keyup", (e) => {
      this.setKeyState(e, false);
    });
  }

  /**
   * Applies pointer-look yaw and pitch increments.
   */
  applyAimDelta(deltaYaw, deltaPitch) {
    this.yaw += deltaYaw;
    this.pitch = Math.max(-1.45, Math.min(1.45, this.pitch + deltaPitch));
  }

  /**
   * Returns the normalized 3D unit vector along the ship's current nose direction.
   */
  getForwardVector() {
    const cosPitch = Math.cos(this.pitch);
    return {
      x: Math.sin(this.yaw) * cosPitch,
      y: Math.sin(this.pitch),
      z: Math.cos(this.yaw) * cosPitch,
    };
  }

  /**
   * Advances 6DoF ship orientation, thruster velocity, and 3D position by `dt` seconds.
   */
  update(dt) {
    if (!Number.isFinite(dt) || dt <= 0) return;

    let turnInput = 0.0;
    if (this.keys.q || this.keys.arrowLeft) {
      this.yaw += this.turnSpeed * dt;
      turnInput += 1.0;
    }
    if (this.keys.e || this.keys.arrowRight) {
      this.yaw -= this.turnSpeed * dt;
      turnInput -= 1.0;
    }
    if (this.keys.arrowUp) this.pitch += this.pitchSpeed * dt;
    if (this.keys.arrowDown) this.pitch -= this.pitchSpeed * dt;

    this.pitch = Math.max(-1.45, Math.min(1.45, this.pitch));

    const cosPitch = Math.cos(this.pitch);
    const sinPitch = Math.sin(this.pitch);
    const sinYaw = Math.sin(this.yaw);
    const cosYaw = Math.cos(this.yaw);

    // 3D forward nose vector (+Z rotated by yaw around Y and -pitch around X)
    const fx = sinYaw * cosPitch;
    const fy = sinPitch;
    const fz = cosYaw * cosPitch;

    // Lateral right vector in XZ plane (forwardXZ x up)
    const rx = -cosYaw;
    const rz = sinYaw;

    let thrustForward = 0.0;
    let thrustRight = 0.0;
    let thrustUp = 0.0;

    if (this.keys.w) thrustForward += 1.0;
    if (this.keys.s) thrustForward -= 1.0;
    if (this.keys.d) thrustRight += 1.0;
    if (this.keys.a) thrustRight -= 1.0;
    if (this.keys.space || this.keys.r) thrustUp += 1.0;
    if (this.keys.shift || this.keys.c || this.keys.f) thrustUp -= 1.0;

    let usedAutopilot = false;
    if (
      this.autopilotTarget &&
      thrustForward === 0.0 &&
      thrustRight === 0.0 &&
      thrustUp === 0.0
    ) {
      const dist = Math.hypot(
        this.autopilotTarget.x - this.position.x,
        this.autopilotTarget.y - this.position.y,
        this.autopilotTarget.z - this.position.z
      );
      if (dist <= this.autopilotTarget.stopDistance) {
        this.brake();
      } else {
        usedAutopilot = true;
        this.orientToward(this.autopilotTarget, false);
        const cosP = Math.cos(this.pitch);
        const sinP = Math.sin(this.pitch);
        const sinY = Math.sin(this.yaw);
        const cosY = Math.cos(this.yaw);
        const apScale = Math.min(1.0, (dist - this.autopilotTarget.stopDistance) / 6.0);
        const targetSpeed = Math.max(2.5, this.maxSpeed * apScale);
        this.velocity.x = sinY * cosP * targetSpeed;
        this.velocity.y = sinP * targetSpeed;
        this.velocity.z = cosY * cosP * targetSpeed;
      }
    }

    if (!usedAutopilot) {
      let ax = fx * thrustForward + rx * thrustRight;
      let ay = fy * thrustForward + thrustUp;
      let az = fz * thrustForward + rz * thrustRight;

      const aMag = Math.hypot(ax, ay, az);
      if (aMag > 1e-6) {
        const scale = this.thrustAcceleration / aMag;
        this.velocity.x += ax * scale * dt;
        this.velocity.y += ay * scale * dt;
        this.velocity.z += az * scale * dt;
      } else if (this.linearDamping > 0.0) {
        const damp = Math.max(0.0, 1.0 - this.linearDamping * dt);
        this.velocity.x *= damp;
        this.velocity.y *= damp;
        this.velocity.z *= damp;
        if (Math.hypot(this.velocity.x, this.velocity.y, this.velocity.z) < 0.01) {
          this.velocity.x = 0.0;
          this.velocity.y = 0.0;
          this.velocity.z = 0.0;
        }
      }
    }

    const speed = Math.hypot(this.velocity.x, this.velocity.y, this.velocity.z);
    if (speed > this.maxSpeed) {
      const clampScale = this.maxSpeed / speed;
      this.velocity.x *= clampScale;
      this.velocity.y *= clampScale;
      this.velocity.z *= clampScale;
    }

    this.position.x += this.velocity.x * dt;
    this.position.y += this.velocity.y * dt;
    this.position.z += this.velocity.z * dt;

    // Smooth visual roll/bank angle during lateral strafing and turning (positive = bank right)
    const bankInput = Math.max(-1.0, Math.min(1.0, thrustRight - turnInput));
    const targetBank = bankInput * this.maxBankAngle;
    const bankAlpha = 1.0 - Math.exp(-this.bankSmoothRate * dt);
    this.bankAngle += (targetBank - this.bankAngle) * bankAlpha;
    if (targetBank === 0.0 && Math.abs(this.bankAngle) < 1e-5) {
      this.bankAngle = 0.0;
    }
  }

  /**
   * Returns current 6DoF kinematic snapshot for outgoing MovementPacket.
   */
  sample() {
    return {
      x: this.position.x,
      y: this.position.y,
      z: this.position.z,
      vx: this.velocity.x,
      vy: this.velocity.y,
      vz: this.velocity.z,
      yaw: this.yaw,
      pitch: this.pitch,
      sequence: this.sequence++,
    };
  }

  setPosition(x, y, z) {
    this.position.x = x;
    this.position.y = y;
    this.position.z = z;
  }

  setOrientation(yaw, pitch = 0.0) {
    this.yaw = yaw;
    this.pitch = Math.max(-1.45, Math.min(1.45, pitch));
  }

  /**
   * Reconciles authoritative server position/velocity correction.
   */
  reconcile(sequence, x, y, z, vx = 0.0, vy = 0.0, vz = 0.0) {
    this.setPosition(x, y, z);
    this.velocity.x = vx;
    this.velocity.y = vy;
    this.velocity.z = vz;
    if (this.sequence <= sequence) {
      this.sequence = sequence + 1;
    }
  }
}
