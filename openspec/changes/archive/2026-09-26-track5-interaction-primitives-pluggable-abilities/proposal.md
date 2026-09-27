# Proposal: Track 5 - Generalized Interaction Primitives & Pluggable Ability System

## 1. Context & Motivation

Following our architectural realignment, the MMO server engine is formally defined as a **genre-agnostic, distributed spatial simulation substrate**. In earlier iterations, gameplay mechanics in `crates/gameplay` (e.g. `CombatPlugin`, `ThreatPlugin`) made ad-hoc assumptions about entity health, damage calculation, and direct manipulation.

To support diverse MMO genres (fantasy RPGs, space simulators, survival sandboxes, social worlds), the engine core (`crates/core`, `crates/node`) must provide high-performance, generalized simulation primitives:
1. **Cancellable Timed Actions**: Abstract stateful tasks over time with deterministic interruption guards.
2. **Dynamic Attributes & Modifiers**: Generalized scalar properties with modifiers, clamps, regeneration curves, and aura timers.
3. **Authoritative 3D Spatial Queries**: Raycast/Line-of-Sight, Cone/Cleave, and Sphere/AoE targeting against the 3D Octree.
4. **Cross-Partition Interaction Dispatch**: Enabling actions between entities located on different cluster simulation nodes.

Specific gameplay implementations (spells, weapons, mana/energy, threat tables, crafting bars) will live exclusively in decoupled plugins in `crates/gameplay` that consume these core primitives.

---

## 2. Proposed Capabilities

### In `crates/core`:
- **`TimedAction` & `ActionQueue`**: Generational action tracking, progress calculation, and interruption conditions (`CancelOnMovement`, `CancelOnDamage`, `CancelOnStun`, `CancelOnTargetLost`).
- **`AttributeSet`**: Dynamic key-value scalar attributes with base values, current values, min/max bounds, flat & percentage modifiers, passive tick-rate regeneration/decay, and temporary timed auras.
- **`SpatialTargetQuery`**: Pure geometric spatial queries (Point, Raycast/Line-of-Sight, Cone, Sphere) evaluated against the authoritative 3D `Octree`.
- **`InteractionEventBus`**: Event pipeline for requesting, validating, and executing interactions locally and across partition boundaries.

### In `crates/gameplay`:
- **Upgraded `CombatPlugin`**: Migrated to utilize `AttributeSet` (e.g. `health`, `armor`, `stamina`) and `TimedAction` rather than monolithic hardcoded structs.
- **Reference `AbilityPlugin`**: Cooldown management (`CooldownTracker`), resource costs, instant casts vs. casted actions, and ability effect execution (damage, healing, auras).
- **Decoupled AI & Threat Integration**: Clean wiring between action completion, combat events, and existing `ThreatPlugin` / `MonsterAiPlugin`.

---

## 3. Non-Goals

- Hardcoding RPG-specific class hierarchies, talent trees, or fantasy lore into `crates/core`.
- Client-side animation frame interpolation or particle VFX.
- Heavy continuous rigid-body collision meshes (targeting authoritative bounding volume and octree queries).

---

## 4. Architectural Invariants

1. **Zero Fantasy Bias in Core**: `crates/core` contains zero references to "spells", "mana", "threat", or "GCD".
2. **Deterministic Tick Evaluation**: All action progress, cooldowns, attribute regeneration, and aura expirations are evaluated deterministically in the 20 Hz fixed-step loop.
3. **Anti-Cheat & Interruption Guarantees**: Any movement violating action constraints instantly aborts the action server-side and raises an `ActionInterruptedEvent`.
4. **Cross-Partition Safety**: When entity A interacts with entity B on a neighboring simulation node, no entity state is mutated without atomic inter-node validation.
