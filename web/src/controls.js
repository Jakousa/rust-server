const KEY_BINDINGS = {
  w: "w",
  a: "a",
  s: "s",
  d: "d",
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
};

/**
 * Keyboard and pointer controls for player character.
 */
export class PlayerControls {
  constructor() {
    this.keys = {
      w: false,
      a: false,
      s: false,
      d: false,
      arrowUp: false,
      arrowDown: false,
      arrowLeft: false,
      arrowRight: false,
      space: false,
    };

    this.position = { x: 0.0, y: 0.0, z: 5.0 };
    this.yaw = 0.0;
    this.speed = 7.5; // units/sec (comfortable headroom below server 10.0 m/s cap)
    this.turnSpeed = 2.5; // rad/sec
    this.sequence = 1;
    this.onCastCallback = null;

    if (typeof window !== "undefined") {
      this.initListeners();
    }
  }

  setKeyState(e, pressed) {
    const key = typeof e.key === "string" ? e.key.toLowerCase() : "";
    const bound = KEY_BINDINGS[key] || CODE_BINDINGS[e.code];
    if (bound) {
      this.keys[bound] = pressed;
    }
    if (pressed && e.code === "Space" && this.onCastCallback) {
      this.onCastCallback();
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

  onCast(callback) {
    this.onCastCallback = callback;
  }

  /**
   * Advances player position based on active keys and delta time.
   */
  update(dt) {
    // Yaw rotation with A/D and ArrowLeft/ArrowRight (E is reserved for Interact/Barter)
    if (this.keys.a || this.keys.arrowLeft) this.yaw += this.turnSpeed * dt;
    if (this.keys.d || this.keys.arrowRight) this.yaw -= this.turnSpeed * dt;

    // Movement relative to facing direction
    let moveForward = 0.0;
    let moveStrafe = 0.0;

    if (this.keys.w || this.keys.arrowUp) moveForward += 1.0;
    if (this.keys.s || this.keys.arrowDown) moveForward -= 1.0;
    if (this.keys.d) moveStrafe += 1.0;
    if (this.keys.a) moveStrafe -= 1.0;

    if (moveForward !== 0.0 || moveStrafe !== 0.0) {
      const len = Math.hypot(moveForward, moveStrafe);
      moveForward /= len;
      moveStrafe /= len;

      const cos = Math.cos(this.yaw);
      const sin = Math.sin(this.yaw);

      // Forward vector: (-sin, cos) in XZ plane
      const dx = (-sin * moveForward + cos * moveStrafe) * this.speed * dt;
      const dz = (cos * moveForward + sin * moveStrafe) * this.speed * dt;

      this.position.x += dx;
      this.position.z += dz;
    }
  }

  /**
   * Returns current kinematic snapshot for outgoing movement packet.
   */
  sample() {
    return {
      x: this.position.x,
      y: this.position.y,
      z: this.position.z,
      yaw: this.yaw,
      sequence: this.sequence++,
    };
  }

  setPosition(x, y, z) {
    this.position.x = x;
    this.position.y = y;
    this.position.z = z;
  }

  /**
   * Reconciles authoritative movement correction from server.
   */
  reconcile(sequence, x, y, z) {
    this.setPosition(x, y, z);
    if (this.sequence <= sequence) {
      this.sequence = sequence + 1;
    }
  }
}
