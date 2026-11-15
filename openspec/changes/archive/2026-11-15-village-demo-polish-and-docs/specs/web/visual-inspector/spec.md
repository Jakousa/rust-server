# Delta Specification: Web Visual Inspector

## ADDED Requirements

### Requirement: Connection Status, Transport Badge & Toast Feedback
The web visual inspector HUD MUST surface human-readable connection state, active transport protocol (`HTTP/3` vs `WebSocket` vs `Offline`), and timed non-intrusive toast notifications without breaking 3-argument callers or missing DOM nodes.

#### Scenario: Transport Mode Emission and Badge Updates
- GIVEN a `WebTransportClient` transitioning through `connecting`, `connected`, and `disconnected` states
- WHEN `notifyStatus(status, error)` fires
- THEN the status callback payload MUST include `transportMode` alongside `mode` (`"webtransport"`, `"websocket"`, or `null`)
- AND `UIController.setConnectionState(state, text, errorMessage, transportMode)` MUST update `#status-text` (`"Disconnected"`, `"Connecting..."`, `"Connected"`) and `#transport-badge` (`"HTTP/3"` for `"webtransport"`, `"WebSocket"` for `"websocket"`, `"Offline"` when disconnected, and `"Online"` when connected without a specified `transportMode`).

#### Scenario: Backward Compatibility for Three-Argument Callers
- GIVEN an existing caller invoking `UIController.setConnectionState(state, text, errorMessage)` with three arguments
- WHEN `transportMode` is omitted
- THEN `UIController` MUST update `#status-dot`, `#connection-error`, button states, `#status-text`, and `#transport-badge` without errors.

#### Scenario: Timed HUD Toast Notifications
- GIVEN `UIController.showToast(message, variant, durationMs)` is invoked
- WHEN `#hud-toast` is present in the DOM
- THEN `#hud-toast` MUST become visible with `textContent` set to `message` and class `toast-<variant>`
- AND any previously active dismiss timer MUST be cancelled before scheduling the new auto-dismiss timeout
- AND when `#hud-toast` is absent from the DOM, `showToast` MUST return gracefully without throwing.

---

### Requirement: Onboarding Keycap Hints & Editable Input Isolation
The web visual inspector MUST present keyboard/mouse Village & Barter onboarding hints with `<kbd>` keycap badges and MUST isolate character controls from editable HTML form fields.

#### Scenario: Keycap Onboarding Card
- GIVEN the visual inspector page loads
- WHEN the HUD renders
- THEN `.controls-hint` MUST display `<kbd>` keycap badges for movement (`WASD` / `Arrows`), camera (`Drag` / `Wheel`), interaction (`E`), and action cast (`Space`) alongside Village & Barter instructions without overlapping existing HUD panels.

#### Scenario: Editable Element Focus Guard
- GIVEN the user has focused an `INPUT`, `TEXTAREA`, or `contentEditable` element (such as `#gateway-url`, `#cert-hash`, or `#trade-offer-gold`)
- WHEN a `keydown` event fires for `W`, `A`, `S`, `D`, arrow keys, or `Space`
- THEN `PlayerControls.setKeyState(e, true)` MUST ignore the event without updating `controls.keys` or triggering `onCastCallback`
- AND `keyup` events MUST still clear previously held keys to prevent stuck movement.
