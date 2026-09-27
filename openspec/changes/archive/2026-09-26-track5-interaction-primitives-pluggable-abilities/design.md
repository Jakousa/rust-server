# Architecture & Design: Track 5 - Interaction Primitives & Pluggable Abilities

## 1. Architectural Philosophy & Layering

The central design goal is to strictly separate **Engine Simulation Primitives** (`crates/core`) from **Game Rules & Abilities** (`crates/gameplay`).

```
┌────────────────────────────────────────────────────────────────────────┐
│                        GAMEPLAY PLUGINS (crates/gameplay)              │
│   • AbilityPlugin (Cooldowns, Mana/Energy costs, Instant/Casted)      │
│   • CombatPlugin (Armor mitigation formulas, damage events)           │
│   • ThreatPlugin & MonsterAiPlugin (Aggro scoring, FSM triggers)      │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │ registers & consumes
┌───────────────────────────────────▼────────────────────────────────────┐
│                        ENGINE PRIMITIVES (crates/core)                 │
│                                                                        │
│   • TimedAction / ActionQueue     • AttributeSet & Modifiers           │
│     - Cancellable durations         - Numeric properties & bounds      │
│     - Move/Damage/Stun interrupts   - Tick-based regen/decay curves    │
│     - Action lifecycle events       - Timed auras (Buffs/Debuffs)      │
│                                                                        │
│   • SpatialTargetQuery            • Interaction Pipeline               │
│     - Point, Raycast, Cone, Sphere  - Hook validation (BeforeInteract) │
│     - 3D Octree spatial indexing    - Cross-boundary node dispatch     │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Core Data Structures (`crates/core`)

### 2.1. `TimedAction` & `ActionQueue`
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionInterruptFlags {
    pub cancel_on_movement: bool,
    pub cancel_on_damage: bool,
    pub cancel_on_stun: bool,
    pub cancel_on_target_lost: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimedAction {
    pub action_id: u32,
    pub duration_sec: f32,
    pub elapsed_sec: f32,
    pub interrupt_flags: ActionInterruptFlags,
    pub target_guid: Option<Guid>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActionQueue {
    pub current: Option<TimedAction>,
}
```

### 2.2. `AttributeSet` & Modifiers
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttributeValue {
    pub base: f32,
    pub current: f32,
    pub min: f32,
    pub max: f32,
    pub regen_per_sec: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ModifierType {
    Flat(f32),
    Percent(f32), // e.g. 0.20 = +20%
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimedModifier {
    pub id: String,
    pub attribute_name: String,
    pub modifier: ModifierType,
    pub remaining_duration_sec: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AttributeSet {
    pub attributes: HashMap<String, AttributeValue>,
    pub modifiers: Vec<TimedModifier>,
}
```

### 2.3. Spatial Target Query Primitives
```rust
pub enum SpatialTargetQuery {
    Point { max_range: f32 },
    Cone { range: f32, half_angle_rad: f32, facing: Vec3 },
    Sphere { center: Vec3, radius: f32, max_targets: usize },
    Raycast { origin: Vec3, direction: Vec3, max_range: f32 },
}
```

---

## 3. Concurrency & Tick Execution Lifecycle

### 3.1. Strict Order of Operations for Attributes
To guarantee determinism regardless of attachment/detachment order:
$$\text{Effective} = (\text{Base} + \sum \text{FlatModifiers}) \times \left(1.0 + \sum \text{PercentModifiers}\right)$$
Clamped strictly: $\min \le \text{Current} \le \text{Max}$.

### 3.2. Sequential Tick Phase Ordering in `sim.rs`
During each 20 Hz simulation tick:
1. **Phase 1: Ingress Movement & Kinematics**:
   - Client movement steps are validated with sequence monotonicity and $\Delta t$ bounds.
   - Synchronous `BeforeMove` hook dispatch: if non-zero displacement occurs and entity has an active `TimedAction` with `cancel_on_movement: true`, the action is immediately aborted with `ActionInterruptedEvent` before the position is committed.
2. **Phase 2: Action Progress System (`ActionTickSystem`)**:
   - Ticks all active `TimedAction` instances by advancing `elapsed_sec += dt`.
   - Completed actions emit `ActionCompletedEvent` into `EventQueue` and are removed from `ActionQueue`.
3. **Phase 3: Attribute & Aura System (`AttributeTickSystem`)**:
   - Applies passive regen/decay: $\text{current} = \text{clamp}(\text{current} + \text{regen} \times \Delta t, \min, \max)$.
   - Decrements aura modifier durations; emits `AuraExpiredEvent` upon timeout and recalculates effective bounds.
4. **Phase 4: Gameplay Plugins Execution**:
   - Plugins drain `EventQueue` (handling `ActionCompletedEvent`, `DamageEvent`, `ActionInterruptedEvent`).
   - Abilities, combat mitigation, and threat score adjustments execute.
5. **Phase 5: Octree Spatial Re-indexing & AoI Replication**:
   - Positions updated, dynamic density throttling evaluated, AoI packets broadcast.
6. **Phase 6: Persistence Flush**:
   - Async write-behind snapshotter enqueues dirty states.

---

## 4. Failure Modes & Edge Cases

| Scenario | Risk | Mitigation |
| :--- | :--- | :--- |
| **Movement spoofing mid-cast** | Client sends cast command, then moves while claiming cast completed | Movement pipeline checks active action flags synchronously; vetoes or cancels action before completing |
| **Cross-partition target despawn** | Target crosses boundary mid-cast | `ActionQueue` with `cancel_on_target_lost` queries Octree / AoI each tick; cancels if target is absent |
| **Attribute underflow/overflow** | Damage > health or regen > max | Strict clamp in `AttributeValue::apply_delta` ensuring `min <= current <= max` |
| **Modifier stacking blowup** | Endless buffs causing integer/float overflow | Bounded modifiers list per attribute, capped total multiplier |
| **Re-entrancy in event queue** | Action completion triggers another action immediately | Events are queued deferred into `EventQueue` and processed cleanly at tick boundary |
