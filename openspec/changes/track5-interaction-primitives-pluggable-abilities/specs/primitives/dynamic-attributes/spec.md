# Spec: Dynamic Attributes & Modifiers

## Capability: `primitives/dynamic-attributes`

Provides a data-driven numerical property and modifier container for entities, supporting dynamic clamping, regeneration/decay curves, and temporary aura effects.

---

### Requirement: Scalar Storage & Bound Clamping

#### Scenario: Clamped attribute update
- **GIVEN** an `AttributeSet` component containing attribute `"health"` with base value $100$, min $0$, and max $100$
- **WHEN** damage reducing current value by $150$ is applied
- **THEN** the current value is clamped to $0$ without underflow
- **AND** an `AttributeChangedEvent` is emitted with `(prev: 100, current: 0)`.

---

### Requirement: Modifier Stacking

#### Scenario: Flat and percentage modifier stacking
- **GIVEN** an attribute `"speed"` with base value $10.0$
- **WHEN** a flat modifier $+2.0$ and a percentage modifier $+20\%$ are attached
- **THEN** effective value evaluates to $(10.0 + 2.0) \times (1.0 + 0.20) = 14.4$
- **AND** removing the modifier immediately recalculates the effective value deterministically.

---

### Requirement: Tick-Based Regeneration & Decay

#### Scenario: Passive regeneration per tick
- **GIVEN** an attribute `"energy"` with current value $50.0$, max $100.0$, and regen rate $+10.0 \text{ units/second}$
- **WHEN** the 20 Hz simulation ticks for duration $\Delta t = 0.05\text{s}$
- **THEN** current value increases by $10.0 \times 0.05 = 0.5$ units
- **AND** does not exceed the max bound of $100.0$.

---

### Requirement: Timed Auras & Expiration

#### Scenario: Temporary buff expiration
- **GIVEN** a temporary modifier (aura) with duration $5.0$ seconds attached at tick $T_0$
- **WHEN** simulation advances past $5.0$ seconds
- **THEN** the modifier is automatically detached
- **AND** an `AuraExpiredEvent` is emitted
- **AND** effective attribute values are updated immediately.
