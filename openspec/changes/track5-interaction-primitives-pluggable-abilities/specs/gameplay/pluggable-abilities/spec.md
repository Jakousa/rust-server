# Spec: Pluggable Ability & Combat Systems

## Capability: `gameplay/pluggable-abilities`

Provides pluggable gameplay ability handling, cooldown tracking, resource cost validation, and combat resolution consuming the core interaction primitives.

---

### Requirement: Cooldown Management

#### Scenario: Global Cooldown (GCD) & Ability Cooldown
- **GIVEN** an entity with `CooldownTracker`
- **WHEN** ability $X$ with cooldown $10.0$ seconds and GCD $1.5$ seconds is triggered
- **THEN** ability $X$ is placed on $10.0$s cooldown
- **AND** all GCD-restricted abilities are locked for $1.5$s
- **AND** subsequent activation attempts within $1.5$s return `Err(OnCooldown)`.

---

### Requirement: Resource Cost Deduction

#### Scenario: Resource validation before action
- **GIVEN** an entity with an `AttributeSet` having attribute `"energy" = 20.0`
- **WHEN** an ability requiring $30.0$ energy is executed
- **THEN** execution fails with `Err(InsufficientResource)`
- **AND** no action or cooldown is triggered.

#### Scenario: Resource deduction on success
- **GIVEN** an entity with `"energy" = 50.0`
- **WHEN** an ability requiring $30.0$ energy completes
- **THEN** `"energy"` is reduced to $20.0$ atomically.

---

### Requirement: Decoupled Damage & Healing Resolution

#### Scenario: Armor mitigation and combat event
- **GIVEN** target entity $T$ with `health` $100$ and `armor` $20$
- **WHEN** incoming physical damage $50$ is processed
- **THEN** damage formula mitigates damage based on armor
- **AND** remaining health is decremented in `AttributeSet`
- **AND** a `DamageTakenEvent` is pushed to `EventQueue` for consumption by threat and UI plugins.
