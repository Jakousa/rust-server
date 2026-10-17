# Lag Compensation, History Buffers & Client Prediction Reconciliation Specification

## Purpose
Specifies the server-authoritative tick history ring buffer, anti-cheat rewind window clamping, historical spatial raycasts, and client-side prediction replay reconciliation across the distributed MMO engine.

---

## Requirements

### Requirement: Server-Side Tick History Ring Buffer
Simulation nodes SHALL record a historical snapshot of all active entity transforms, positions, velocities, and 3D bounding boxes at every authoritative 20 Hz simulation tick into a fixed-capacity circular ring buffer without performing per-tick heap allocations.

#### Scenario: Tick frame recording with zero heap reallocations
- **GIVEN** an active `TickSimulation` instance with `SpatialHistoryBuffer` configured with capacity $N \ge 32$
- **WHEN** `TickSimulation::step()` executes
- **THEN** the simulation SHALL record a `HistoricalTickFrame` containing all active entities in `EntityStore`
- **AND** the buffer SHALL reuse preallocated memory vectors from evicted oldest slots without allocating new heap buffers
- **AND** the recorded frame SHALL be retrievable by tick counter or monotonic timestamp.

#### Scenario: Buffer circular eviction on capacity overflow
- **GIVEN** a history buffer with capacity of 32 ticks
- **WHEN** 33 ticks have executed
- **THEN** the oldest tick frame (tick 1) SHALL be evicted and overwritten by tick 33
- **AND** the buffer length SHALL remain clamped at 32 ticks.

---

### Requirement: Anti-Cheat Rewind Window Clamping & Latency Bounds
The simulation node SHALL authoritatively validate and clamp any client-requested interaction or ability timestamp before reconstructing the historical spatial scene. The client-provided target timestamp SHALL be server-synchronized (adjusted via Ping/Pong RTT estimation and clock offset, or expressed as a target server tick/monotonic timestamp). The server SHALL reject future timestamps and strictly clamp the rewind window to a maximum of $250\text{ms}$.

#### Scenario: Valid client timestamp within allowable rewind window
- **GIVEN** a server monotonic time $t_{\text{now}} = 1000\text{ms}$
- **WHEN** a client submits an interaction with server-synchronized target timestamp $t_{\text{client}} = 900\text{ms}$ ($100\text{ms}$ rewind)
- **THEN** the server SHALL accept the rewind request
- **AND** reconstruct the spatial scene at $t = 900\text{ms}$.

#### Scenario: Malicious or excessive rewind clamped to 250ms ceiling
- **GIVEN** a server monotonic time $t_{\text{now}} = 1000\text{ms}$
- **WHEN** a client submits an interaction with target timestamp $t_{\text{client}} = 400\text{ms}$ ($600\text{ms}$ rewind)
- **THEN** the server SHALL clamp the effective query timestamp to $t = 750\text{ms}$ ($t_{\text{now}} - 250\text{ms}$)
- **AND** increment the `mmo_lag_compensation_rewind_clamped_total` metric counter.

#### Scenario: Future timestamp spoofing prevented
- **GIVEN** a server monotonic time $t_{\text{now}} = 1000\text{ms}$
- **WHEN** a client submits an interaction with target timestamp $t_{\text{client}} = 1200\text{ms}$ (in the future)
- **THEN** the server SHALL clamp the effective query timestamp to $t_{\text{now}} = 1000\text{ms}$
- **AND** prevent any future-peeking advantage.

---

### Requirement: Historical Scene Reconstruction & Interpolation
When evaluating a clamped historical timestamp between two discrete simulation ticks, the server SHALL linearly interpolate entity positions and bounding boxes between the two surrounding frames.

#### Scenario: Fractional tick linear interpolation
- **GIVEN** historical frames at $t_0 = 100\text{ms}$ with entity at $(0.0, 0.0, 0.0)$ and $t_1 = 150\text{ms}$ with entity at $(10.0, 0.0, 0.0)$
- **WHEN** a query is executed for target timestamp $t = 125\text{ms}$
- **THEN** the reconstructed entity position SHALL be $(5.0, 0.0, 0.0)$
- **AND** the reconstructed bounding box center SHALL correspond to the interpolated position.

#### Scenario: Target timestamp older than buffer capacity
- **GIVEN** a buffer containing frames from $t = 800\text{ms}$ to $t = 1000\text{ms}$
- **WHEN** a query is executed for target timestamp $t = 700\text{ms}$
- **THEN** the query SHALL clamp to the oldest available frame at $t = 800\text{ms}$
- **AND** log or report clamping telemetry.

---

### Requirement: Authoritative Lag-Compensated Raycasts & Vector Normalization
The server SHALL execute raycast hit testing against historical entity bounding boxes to validate hits for hitscan abilities or targeted interactions. The server SHALL re-normalize the client-provided ray direction vector and reject degenerate (non-finite or near-zero) vectors. If a hit target entity despawned between the historical tick and the current tick, the server SHALL return the historical hit result, but interaction handlers SHALL gracefully handle entity non-existence in `EntityStore`.

#### Scenario: Degenerate or unnormalized direction vector handled safely
- **GIVEN** an incoming raycast with unnormalized direction vector $(0.1, 0.0, 0.0)$ or degenerate vector $(0.0, 0.0, 0.0)$
- **WHEN** `raycast_historical` processes the query
- **THEN** degenerate vectors (< 1e-6 length or non-finite) SHALL be rejected with `None`
- **AND** valid non-zero vectors SHALL be authoritatively normalized to unit length before executing the slab intersection test.

#### Scenario: Direct hit on moving target at historical position
- **GIVEN** a moving target entity at historical position $(10.0, 0.0, 0.0)$ with bounding box half-extents $(0.5, 1.0, 0.5)$ at $t_{\text{target}}$
- **WHEN** an attacker fires a ray from origin $(0.0, 0.0, 0.0)$ in direction $(1.0, 0.0, 0.0)$ with max distance $20.0$ at $t_{\text{target}}$
- **THEN** `raycast_historical` SHALL register a hit on the target entity
- **AND** report hit distance $9.5$ and impact coordinates $(9.5, 0.0, 0.0)$.

#### Scenario: Target despawned between historical tick and current execution
- **GIVEN** an entity was present at $t_{\text{target}}$ but despawned prior to the current tick
- **WHEN** `raycast_historical` hits the historical bounding box
- **THEN** the query SHALL return the hit entity GUID
- **AND** downstream action application SHALL handle `entity_store.get_by_guid(target) == None` without panicking.

#### Scenario: Raycast misses target that has moved
- **GIVEN** an attacker fires a ray toward the target's current position $(15.0, 0.0, 0.0)$, but target was at $(10.0, 5.0, 0.0)$ at $t_{\text{target}}$
- **WHEN** the raycast is executed against historical bounding boxes at $t_{\text{target}}$
- **THEN** the raycast SHALL return `None` (miss)
- **AND** prevent erroneous hit registration.

---

### Requirement: Client-Side Prediction & Reconciliation
The WebAssembly client SHALL predict local player movement immediately upon input and store unacknowledged inputs in a FIFO queue. Upon receiving authoritative server state updates or corrections, the client SHALL reconcile divergence: if the positional error exceeds $\epsilon = 0.05\text{m}$, it SHALL replay unacknowledged inputs and smoothly decay the visual error offset.

#### Scenario: Prediction matches authoritative server state
- **GIVEN** a client has predicted movement up to sequence 10, arriving at $(10.0, 0.0, 5.0)$
- **WHEN** the server acknowledges sequence 10 with position $(10.02, 0.0, 5.01)$ (error $\approx 0.022\text{m} \le 0.05\text{m}$)
- **THEN** the client SHALL evict inputs $\le 10$
- **AND** maintain current predicted trajectory without triggering replay or visual correction.

#### Scenario: Divergence triggers replay and smooth convergence
- **GIVEN** a client has predicted movement up to sequence 15, with unacknowledged inputs 11 through 15
- **WHEN** the server corrects sequence 10 with position $(9.0, 0.0, 5.0)$ (divergence $1.0\text{m} > 0.05\text{m}$)
- **THEN** the client SHALL reset base state to the server authoritative position
- **AND** replay inputs 11 through 15 sequentially to calculate the new predicted position
- **AND** smoothly decay the visual error offset over a convergence window without instantaneous camera snapping.

#### Scenario: Severe divergence triggers hard snapback
- **GIVEN** a client experiences a server correction with divergence $> 20.0\text{m}$ (e.g. anti-cheat teleport rollback)
- **WHEN** the correction is processed
- **THEN** the client SHALL instantly snap both simulation and visual positions to the server corrected state.
