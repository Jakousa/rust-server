# Proposal: Lag Compensation, Server-Authoritative History Buffers & Client Prediction Reconciliation

## Problem Statement & Motivation
Following the deployment of geo-distributed Anycast edge gateways, clients worldwide connect over WAN links with variable round-trip times (20ms – 180ms). While the simulation node cluster enforces authoritative 20 Hz kinematics and rejects client-provided delta time, players over WAN connections experience two major latency friction points:

1. **Sluggish Combat & Target Inaccuracy Without Server Lag Compensation**:
   When a client with 100ms ping fires a hitscan projectile or initiates an interaction against an active player or mobile NPC, the server simulation has already advanced 2–3 ticks into the future relative to what the player observed on their screen. Without server-side historical scene reconstruction, high-latency players routinely miss moving targets that were squarely centered in their crosshairs at fire time.

2. **Visual Snapping & Jitter Without Client Prediction Reconciliation**:
   The WebAssembly client (`crates/client-wasm`) predicts player movement locally on user input. However, when the server returns movement corrections or authoritative entity state updates, the client either abruptly snaps the player's position (`replica.position = correction.corrected_position`) or suffers desync without replaying unacknowledged inputs. This produces camera jarring, micro-teleports, and jerky motion.

To resolve these challenges while maintaining the engine's prime directive—**unopinionated, genre-agnostic MMO engine core with zero game-specific assumptions**—this change implements:
- A high-performance, zero-allocation server-side tick history ring buffer.
- Server-authoritative lag-compensated spatial queries with strictly enforced anti-cheat rewind bounds ($t_{\text{rewind}} \le 250\text{ms}$).
- Client-side prediction buffers with unacknowledged input queuing, divergence reconciliation ($\epsilon > 0.05\text{m}$), and smooth error convergence.

---

## Technical Scope

### 1. Server-Side Tick History Ring Buffer (`crates/node`)
- Implement `SpatialHistoryBuffer` in `crates/node/src/history.rs` storing entity transforms, bounding boxes, and kinematic states for the past $N$ ticks (e.g. 32 ticks at 20 Hz = 1.6s).
- Preallocate a contiguous circular ring buffer of tick frame slabs. When recording a tick, reuse existing entity record vectors via `.clear()`, achieving zero per-tick heap allocations.
- Integrate history recording into `TickSimulation::step()` immediately following kinematic updates and system executions.

### 2. Authoritative Lag-Compensated Raycasts & Interactions (`crates/node`)
- Provide historical spatial scene reconstruction:
  - Interpolate entity positions and bounding boxes between adjacent frames for fractional target timestamps.
  - Strictly validate and clamp rewind duration: reject future timestamps ($t > t_{\text{now}}$) and clamp $t_{\text{rewind}} \le 250\text{ms}$ to prevent backward time-travel exploits or spoofed latency claims.
- Implement `raycast_historical`:
  - Execute ray-AABB intersection tests against reconstructed historical bounding boxes using the slab method.
  - Return the closest hit entity, hit distance, and impact point.
- Implement historical point-to-point range and line-of-sight validation.

### 3. Schema & Protocol Support (`crates/schema`)
- Define typed `LagCompensationPayload` for serializing client timestamp, ray origin, direction, and max range within `InteractionPacket.payload`.
- Define `MovementCorrectionReason` enum distinguishing kinematic anti-cheat violations, reconciliation adjustments, and obstacle vetos.
- Support sequence acknowledgment roundtripping.

### 4. Client-Side Prediction & Reconciliation (`crates/client-wasm`)
- Implement `PredictionBuffer` in `crates/client-wasm/src/prediction.rs` maintaining a FIFO queue of unacknowledged movement inputs with monotonic sequence tags.
- On receiving authoritative server movement updates or corrections:
  - Discard acknowledged inputs $\le \text{seq}_{\text{ack}}$.
  - Detect positional divergence: if $\|P_{\text{server}} - P_{\text{predicted}}\| > 0.05\text{m}$, replay all unacknowledged inputs forward from $P_{\text{server}}$.
  - Apply exponential smoothing to positional error offsets to avoid sudden camera or character snapbacks, while snapping immediately only for severe rollbacks ($> 20.0\text{m}$).

---

## Non-Goals
- Weapon-specific recoil patterns, projectile ballistic physics, or hit-box bone deformation (core operates strictly on mathematical 3D bounding boxes and geometric volumes).
- Client-authoritative damage calculation (the server authoritatively runs spatial raycasts, checks range, and dispatches gameplay events).
- Allowing client-requested rewinds beyond the 250ms anti-cheat ceiling under any circumstance.
