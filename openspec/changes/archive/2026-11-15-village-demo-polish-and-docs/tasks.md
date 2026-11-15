# Tasks: Village & Barter Demo Polish & Showcase Documentation

- [x] 1. Scaffold OpenSpec 2.0 change `village-demo-polish-and-docs` and author `proposal.md`, `design.md`, `specs/web/visual-inspector/spec.md`, and `tasks.md`.
- [x] 2. Update `web/index.html` and `web/style.css` to add `#status-text`, `#transport-badge`, hidden `#hud-toast` banner, and polished `.controls-hint` `<kbd>` keycap onboarding card.
- [x] 3. Update `web/src/transport.js` so `notifyStatus` emits `transportMode` alongside `mode` while preserving 100% compatibility.
- [x] 4. Update `web/src/ui.js` to extend `setConnectionState(state, text, errorMessage, transportMode)` and add `showToast(message, variant, durationMs)` with safe DOM guards and timer deduplication.
- [x] 5. Update `web/src/controls.js` so `setKeyState(e, pressed)` ignores `keydown` events targeting `INPUT`, `TEXTAREA`, `SELECT`, or `contentEditable` elements while allowing `keyup` cleanup.
- [x] 6. Add comprehensive Vitest tests in `web/src/__tests__/ui_polish.test.js` covering `#status-text` / `#transport-badge`, 3-arg compatibility, `showToast` variants and timers, `notifyStatus` `transportMode`, and editable input guards.
- [x] 7. Polish `README.md` and `ARCHITECTURE.md` with accurate Docker Compose, Vite dev, `k3d`, Cloud PoC, Wire Protocol SDK, and Godot integration quickstarts and links.
- [x] 8. Run `npm --prefix web test -- --run`, `npm --prefix web run build`, and `openspec validate "village-demo-polish-and-docs"`, then mark all tasks complete.
