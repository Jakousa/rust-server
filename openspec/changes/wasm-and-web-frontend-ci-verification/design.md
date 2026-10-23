# Technical Design: WASM Target Check, Frontend Build Verification, and Vitest Unit Suite

## 1. Vitest Unit Test Suite (`web/package.json` & `web/src/__tests__/inspector.test.js`)

1. In `web/package.json`:
   - Add `"test": "vitest run"` to `"scripts"`.
   - Add `"vitest": "^2.1.0"` (or compatible version) to `"devDependencies"` and update `web/package-lock.json`.
2. In `web/src/controls.js`:
   - Guard `if (typeof window !== "undefined") { this.initListeners(); }` in `PlayerControls.constructor` so `PlayerControls` can be instantiated in a pure Node.js Vitest environment without requiring a DOM `window` global.
3. Create `web/src/__tests__/inspector.test.js`:
   - **`PlayerControls` tests**:
     - Verify `update(dt)` with `w + d` diagonal input normalizes movement vector (`Math.hypot(dx, dz)` equals `speed * dt`).
     - Verify `q` and `e` keys rotate `yaw` and `sample()` increments `sequence` monotonically (`1, 2, 3...`).
   - **`WorldObjectRenderer` tests**:
     - Construct a `THREE.Scene()` and `new WorldObjectRenderer(scene)`.
     - Build a 16-float `Float32Array` buffer encoding a 64-bit object ID with `lo = 0xDEADBEEF` (`3735928559`) and `hi = 0x12345678` (`305419896`), `stateFlags = 16` (damaged), and `hpPct = 0.5`.
     - Call `renderer.updateFromBuffer(buf)` and assert `renderer.objects.has((BigInt(0x12345678) << 32n) | BigInt(0xDEADBEEF))` is `true`, the mesh uses `materials.damaged`, and `hpBar.scale.x === 0.5`.
     - Call `renderer.updateFromBuffer(new Float32Array(0))` and assert `renderer.objects.size === 0`.

---

## 2. GitHub Actions CI Integration (`.github/workflows/rust.yml`)

In `.github/workflows/rust.yml` under `jobs.lint`:
1. Add `targets: wasm32-unknown-unknown` to `dtolnay/rust-toolchain@stable`.
2. Add step:
   ```yaml
      - name: Check WASM Target (mmo-client-wasm)
        run: cargo clippy --package mmo-client-wasm --target wasm32-unknown-unknown -- -D warnings
   ```
3. Add Node.js setup and Web Frontend test & build verification steps:
   ```yaml
      - name: Setup Node.js
        uses: actions/setup-node@v4
        with:
          node-version: '20'
          cache: 'npm'
          cache-dependency-path: web/package-lock.json

      - name: Web Frontend Unit Tests & Build
        working-directory: web
        run: |
          npm ci
          npm test
          npm run build
   ```
