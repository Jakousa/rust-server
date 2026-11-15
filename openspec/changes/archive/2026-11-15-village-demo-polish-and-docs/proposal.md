# Proposal: Village & Barter Demo Polish & Showcase Documentation

## Why

First-time operators and playtesters launching the **Village & Barter Vertical Slice** in the browser inspector lack clear visual feedback on connection state, active transport protocol (HTTP/3 WebTransport vs. WebSocket fallback), and in-world barter/trade outcomes, while typing inside HUD inputs (gateway URL, certificate hash, or trade gold) accidentally triggers player movement and ability casts. In addition, the repository's top-level documentation (`README.md` and `ARCHITECTURE.md`) needs cohesive, accurate quickstart paths covering single-command Docker Compose, local Vite development, `k3d` cluster deployment, cloud PoC operations, and external client/Godot SDK integration.

## What Changes

- **Web HUD Connection & Transport Visibility (`web/index.html`, `web/style.css`, `web/src/ui.js`, `web/src/transport.js`)**:
  - Add `#status-text` (`Disconnected` / `Connecting...` / `Connected`) and `#transport-badge` (`HTTP/3` / `WebSocket` / `Offline` / `Online`) badges inside the `.connection-panel` header.
  - Extend `WebTransportClient.notifyStatus` to emit `transportMode` alongside `mode` (`{ status, error, mode, transportMode }`) for full backward compatibility.
  - Extend `UIController.setConnectionState(state, text, errorMessage, transportMode)` while keeping 3-argument callers backward-compatible.
- **Non-Intrusive Toast Feedback & Onboarding Polish (`web/index.html`, `web/style.css`, `web/src/ui.js`)**:
  - Add a hidden-by-default `#hud-toast` notification banner and `UIController.showToast(message, variant, durationMs)` with XSS-safe text rendering and overlapping-timer cancellation.
  - Polish `.controls-hint` with crisp `<kbd>` keycap badges (`WASD`, `Arrows`, `Drag`, `Wheel`, `E`, `Space`) and Village & Barter onboarding guidance without altering existing HUD panel dimensions or IDs.
- **Editable Input Isolation (`web/src/controls.js`)**:
  - Guard `PlayerControls.setKeyState(e, pressed)` so keydown events targeting `INPUT`, `TEXTAREA`, `SELECT`, or `contentEditable` elements do not mutate movement keys or trigger `Space` casts, while still allowing `keyup` to clear held keys.
- **Unit Test Coverage & Showcase Documentation (`web/src/__tests__/ui_polish.test.js`, `README.md`, `ARCHITECTURE.md`)**:
  - Add targeted Vitest coverage for HUD connection/transport badges, toast notifications, `transportMode` status payloads, and input focus guards.
  - Polish `README.md` and `ARCHITECTURE.md` with accurate Docker Compose, Vite dev, `k3d`, cloud deployment, wire protocol SDK, and Godot client integration guides.

## Capabilities

### New Capabilities

### Modified Capabilities
- `web/visual-inspector`: Add connection status text badge, transport protocol badge, timed HUD toast notifications, `<kbd>` onboarding hints, `transportMode` status emission, and editable DOM input isolation in player keyboard controls.

## Impact

- **Affected Files**: Leaf browser frontend (`web/index.html`, `web/style.css`, `web/src/ui.js`, `web/src/controls.js`, `web/src/transport.js`, `web/src/__tests__/ui_polish.test.js`) and repository documentation (`README.md`, `ARCHITECTURE.md`).
- **Architectural Invariants**: Zero changes to core server crates (`mmo-core`, `mmo-node`, `mmo-schema`, `mmo-storage`, `mmo-gateway`, `mmo-director`, `mmo-gameplay`, `mmo-plugin-voxel`); core remains strictly genre-agnostic with zero server pollution.
- **Compatibility**: All existing `UIController`, `PlayerControls`, and `WebTransportClient` exports, DOM IDs, and 3-argument method signatures remain 100% backward-compatible.
