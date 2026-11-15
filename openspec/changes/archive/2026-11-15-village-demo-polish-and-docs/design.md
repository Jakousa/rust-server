# Design: Village & Barter Demo Polish & Showcase Documentation

## 1. Architectural Scope & Invariants

All runtime changes are confined strictly to the browser visual inspector leaf package (`web/`) and root markdown documentation (`README.md`, `ARCHITECTURE.md`). Core server simulation crates (`crates/core`, `crates/node`, `crates/director`, `crates/gateway`, `crates/schema`, `crates/storage`, `crates/gameplay`, `crates/plugins/voxel`) remain untouched, preserving genre-agnostic core substrates, 20 Hz server authority, and zero server pollution.

## 2. Connection Status & Transport Protocol Badges

### DOM Structure (`web/index.html`, `web/style.css`)
Inside `.connection-panel > .panel-header`, alongside `#status-dot`:
- `#status-text` (`class="status-badge disconnected"`): Displays human-readable connection state (`Disconnected`, `Connecting...`, `Connected`).
- `#transport-badge` (`class="transport-badge offline"`): Displays active transport layer (`HTTP/3`, `WebSocket`, `Online`, `Negotiating`, or `Offline`).

### Status Event Emission (`web/src/transport.js`)
`WebTransportClient.notifyStatus(status, error = null)` emits:
```js
{ status, error, mode: this.transportMode, transportMode: this.transportMode }
```
Preserving `mode` keeps existing callers and `web/src/__tests__/inspector.test.js` 100% compatible while exposing `transportMode` cleanly.

### UI Synchronization (`web/src/ui.js`)
`UIController.setConnectionState(state, text = "", errorMessage = "", transportMode = "")`:
- Updates `#status-dot` and button `disabled` states and `#connection-error` identically to prior behavior.
- Updates `#status-text` using `text` when provided, otherwise defaulting cleanly (`"Connected"`, `"Connecting..."`, `"Disconnected"`).
- Updates `#transport-badge` cleanly:
  - `"webtransport"` / `"http3"` / `"http/3"` -> `"HTTP/3"` (`.webtransport`)
  - `"websocket"` / `"ws"` / `"wss"` -> `"WebSocket"` (`.websocket`)
  - Omitted/empty `transportMode`:
    - `state === "disconnected"` -> clears remembered mode and shows `"Offline"` (`.offline`)
    - `state === "connecting"` -> clears remembered mode and shows `"Negotiating"` (`.connecting`)
    - `state === "connected"` -> keeps previously remembered active mode on the current connection if set, otherwise shows `"Online"` (`.online`).

## 3. Non-Intrusive HUD Toast Banner (`#hud-toast`)

- Placed as a top-center fixed banner (`#hud-toast`, `role="status"`, `aria-live="polite"`, `pointer-events: none`, `z-index: 150`) so it never collides with `.connection-panel` (top-left), `#village-hud` (top-right), or blocks clicks on modals.
- Starts with `.hud-toast.hidden` (`display: none`).
- `UIController.showToast(message, variant = "info", durationMs = 3000)`:
  - Gracefully no-ops if `#hud-toast` is absent (for unit tests with minimal DOM stubs).
  - Clears any prior `this._toastTimer` before setting a new timeout so rapid calls reset the display window cleanly.
  - Assigns `textContent` (never `innerHTML`) to prevent XSS, applies `hud-toast toast-<variant>` (`info`, `success`, `warning`, `error`), removes `.hidden`, and schedules `hideToast()` when `durationMs > 0`.

## 4. Onboarding Controls Card & Editable Input Isolation

- `.controls-hint` (`web/index.html`, `web/style.css`): Styled as a compact two-column keycap grid using semantic `<kbd>` badges (`WASD`, `Arrows`, `Drag`, `Wheel`, `E`, `Space`) plus a short Village & Barter prompt so the left HUD stack fits comfortably inside `1280x800` viewports without overlapping `.debug-panel`.
- `PlayerControls.setKeyState(e, pressed)` (`web/src/controls.js`):
  - Checks `e?.target?.tagName` (case-insensitive) for `"INPUT"`, `"TEXTAREA"`, `"SELECT"`, or `e?.target?.isContentEditable`.
  - When `pressed` is `true` and an editable element is focused, returns immediately so typing into Gateway URL, Cert Hash, or Trade Gold inputs does not move the player or fire `onCastCallback`.
  - Keyup (`pressed === false`) continues to execute so releasing a held key while focusing an input never leaves movement stuck.

## 5. Showcase Documentation (`README.md`, `ARCHITECTURE.md`)

- `README.md`: Clarify and verify commands/ports for Docker Compose (`docker compose up -d`, `:3000`, `:4433/udp`, `:8080/tcp`, `:9091`), Vite dev (`npm --prefix web run dev`), local `k3d` (`./scripts/cluster.sh all`), Cloud PoC (`docs/CLOUD_POC_DEPLOYMENT.md`), Wire Protocol & Client SDK (`docs/WIRE_PROTOCOL_AND_CLIENT_SDK.md`), and Godot integration (`docs/GODOT_CLIENT_INTEGRATION.md`), plus an interactive Village & Barter Demo guide.
- `ARCHITECTURE.md`: Add concise cross-references to the transport fallback, Village & Barter showcase topology, deployment guides, and external client SDK / Godot integration documentation while preserving all existing Mermaid architecture diagrams and invariants.
