# Specification: Trade & Barter Security Hardening

## Capability: gameplay/trade-and-barter-security

### Feature: Player Trade Validation & Anti-Forgery

#### Scenario: Zero-quantity item offer rejection
- **GIVEN** an active trade session between Player A and Player B
- **WHEN** Player A submits a trade offer containing an `ItemStack` with `quantity == 0`
- **THEN** the server MUST reject the offer with an error
- **AND** the trade session offers MUST NOT be updated.

#### Scenario: Unit weight and name canonicalization from authoritative inventory
- **GIVEN** Player A possesses an item with `item_id = 1001`, `name = "Iron Ore"`, and authoritative `unit_weight = 2.0`
- **WHEN** Player A submits a trade offer for `item_id = 1001` with spoofed `unit_weight = 0.0` or `name = "Spoofed"`
- **THEN** the server MUST resolve the canonical `name` and `unit_weight` from Player A's inventory slots
- **AND** the stored trade offer MUST contain `unit_weight = 2.0` and `name = "Iron Ore"`.

#### Scenario: Multi-stack cumulative quantity validation
- **GIVEN** Player A possesses 10 Iron Ore in total
- **WHEN** Player A submits a trade offer containing two stacks of 6 Iron Ore (total 12)
- **THEN** the server MUST reject the offer during staging validation
- **AND** the offer MUST NOT be updated.

#### Scenario: Unowned item offer rejection
- **GIVEN** an active trade session between Player A and Player B
- **WHEN** Player A submits a trade offer for an `item_id` not present in Player A's inventory
- **THEN** the server MUST reject the offer with a descriptive error.

#### Scenario: Bilateral lock resetting upon offer modification
- **GIVEN** an active trade session where Player A and Player B are both locked
- **WHEN** Player A modifies their offer via sub-opcode 4
- **THEN** BOTH Player A's lock (`locked_a`) and Player B's lock (`locked_b`) MUST be reset to `false`.

### Feature: Trade Session Authorization (Anti-IDOR)

#### Scenario: Non-participant attempts to cancel trade session
- **GIVEN** an active trade session between Player A and Player B with session ID `42`
- **WHEN** Player C (not a participant) submits sub-opcode 6 with `session_id = 42`
- **THEN** the server MUST reject the request with an authorization error
- **AND** the trade session `42` MUST remain active.

#### Scenario: Participant cancels active trade session
- **GIVEN** an active trade session between Player A and Player B with session ID `42`
- **WHEN** Player A submits sub-opcode 6 with `session_id = 42`
- **THEN** the server MUST cancel the session and clear player-to-session mappings.

### Feature: Trade Distance Verification & Lifecycle Symmetry

#### Scenario: Distance exceeded at commit time
- **GIVEN** an active trade session where both players locked and accepted
- **AND** Player A and Player B are separated by more than `MAX_TRADE_DISTANCE` (5.0m)
- **WHEN** `execute_atomic_swap` is invoked
- **THEN** the swap MUST fail with an error
- **AND** the trade session MUST be canceled without modifying player inventories.

#### Scenario: Predictive movement aborts active trade session
- **GIVEN** an active trade session between Player A and Player B
- **WHEN** Player A moves to a position farther than 5.0m from Player B
- **THEN** the `TradeDistanceHook` intercepting `Event::BeforeMove` MUST detect the breach against the predicted coordinates
- **AND** the trade session MUST be canceled.

#### Scenario: Entity despawn cleans up trade session
- **GIVEN** an active trade session between Player A and Player B
- **WHEN** Player A despawns or disconnects (`Event::OnDespawn`)
- **THEN** the `TradeDistanceHook` MUST cancel the trade session and free Player B.

#### Scenario: Symmetric lock and accept progression
- **GIVEN** an active trade session between Player A and Player B
- **WHEN** Player A submits sub-opcode 5 for the first time
- **THEN** Player A's state transitions to locked (`locked_a = true`), but NOT accepted
- **WHEN** Player B submits sub-opcode 5 for the first time
- **THEN** Player B's state transitions to locked (`locked_b = true`), but NOT accepted
- **WHEN** Player A and Player B subsequently submit sub-opcode 5
- **THEN** both players transition to accepted and the swap executes.

### Feature: Safe NPC Barter Currency Arithmetic

#### Scenario: Insufficient currency during NPC barter
- **GIVEN** a player with currency `10`
- **WHEN** the player attempts a barter requiring currency `20`
- **THEN** the transaction MUST fail and currency MUST remain `10`.

#### Scenario: Currency addition overflow guard
- **GIVEN** a player with currency `u64::MAX`
- **WHEN** a barter attempts to reward positive currency
- **THEN** the transaction MUST fail cleanly with an overflow error and rollback all inventory changes.

### Feature: HUD DOM XSS Prevention

#### Scenario: Malicious item name rendering
- **GIVEN** an inventory containing an item with name `<img src=x onerror=alert(1)>`
- **WHEN** `UIController.updateInventory` renders the inventory slot
- **THEN** the element MUST render the text safely via `.textContent` without parsing HTML tags or executing scripts.
