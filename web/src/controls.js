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
      q: false,
      e: false,
      space: false,
    };

    this.position = { x: 0.0, y: 0.0, z: 0.0 };
    this.yaw = 0.0;
    this.speed = 10.0; // units/sec
    this.turnSpeed = 2.5; // rad/sec
    this.sequence = 1;
    this.onCastCallback = null;

    if (typeof window !== "undefined") {
      this.initListeners();
    }
  }

  initListeners() {
    window.addEventListener("keydown", (e) => {
      const k = e.key.toLowerCase();
      if (k === "w") this.keys.w = true;
      if (k === "a") this.keys.a = true;
      if (k === "s") this.keys.s = true;
      if (k === "d") this.keys.d = true;
      if (k === "q") this.keys.q = true;
      if (k === "e") this.keys.e = true;
      if (e.code === "Space") {
        this.keys.space = true;
        if (this.onCastCallback) {
          this.onCastCallback();
        }
      }
    });

    window.addEventListener("keyup", (e) => {
      const k = e.key.toLowerCase();
      if (k === "w") this.keys.w = false;
      if (k === "a") this.keys.a = false;
      if (k === "s") this.keys.s = false;
      if (k === "d") this.keys.d = false;
      if (k === "q") this.keys.q = false;
      if (k === "e") this.keys.e = false;
      if (e.code === "Space") this.keys.space = false;
    });
  }

  onCast(callback) {
    this.onCastCallback = callback;
  }

  /**
   * Advances player position based on active keys and delta time.
   */
  update(dt) {
    // Yaw rotation with Q/E
    if (this.keys.q) this.yaw += this.turnSpeed * dt;
    if (this.keys.e) this.yaw -= this.turnSpeed * dt;

    // Movement relative to facing direction
    let moveForward = 0.0;
    let moveStrafe = 0.0;

    if (this.keys.w) moveForward += 1.0;
    if (this.keys.s) moveForward -= 1.0;
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
}
