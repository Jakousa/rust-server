# Architecture & Technical Design: Trade, Barter & HUD Hardening

## Overview
This design addresses five security vulnerabilities across `crates/gameplay` and `web/`:
1. Authoritative resolution and validation of trade offers with bilateral lock resetting and multi-stack cloned inventory validation.
2. Participant authorization for trade session cancellation and accept actions (IDOR prevention).
3. Commit-time distance re-verification and predictive movement-driven trade abort hook (`Event::BeforeMove` and `Event::OnDespawn`).
4. Checked arithmetic for NPC barter currency and $u64$ quantity accumulation, with defense-in-depth in `InventoryComponent::add_item`.
5. DOM-safe rendering of inventory items and barter recipes in the web frontend.

---

## 1. Authoritative Trade Offer Resolution (`crates/gameplay/src/village.rs`, `trade.rs`)

### Vulnerability Analysis
In sub-opcode 4, the handler directly passes `req.items` (deserialized from client JSON) to `session.set_offer(entity_guid, offer)`.
If `item.quantity == 0`, checks like `has_item_stack` evaluate `total >= 0` to true even if the player has 0 items.
If a client modifies an offer while the other player is already locked, only the modifying player's lock was previously cleared, allowing an attacker to swap out an offer and execute the trade against the partner's stale lock.
Furthermore, if a player submitted multiple stacks of the same item ID (e.g. two stacks of 10 Iron Ore while only owning 10), checking each stack independently would pass.

### Remediation
1. **Quantity and Float Validation**:
   Reject any `ItemStack` in `req.items` where:
   - `quantity == 0`
   - `!unit_weight.is_finite()`
   - `unit_weight < 0.0`
2. **Canonical Slot Metadata Lookup & Deduplicated Offer Staging**:
   Look up each requested `item.item_id` in the caller's authoritative `InventoryComponent`.
   If not found, reject with an error.
   Resolve canonical `name` and `unit_weight` strictly from the player's authoritative inventory slot.
   Clone caller inventory into `let mut test_inv = caller_inv.clone();`.
   Verify `req.currency <= test_inv.currency`.
   For each canonical `ItemStack`, verify `test_inv.remove_item_stack(&canonical_stack)`. If any deduction fails, reject the entire offer.
3. **Bilateral Lock Resetting on Offer Mutation**:
   In `TradeSession::set_offer`:
   Whenever `set_offer` is called by either participant:
   ```rust
   self.locked_a = false;
   self.locked_b = false;
   self.accepted_a = false;
   self.accepted_b = false;
   ```
   Both players must re-inspect and re-lock before the trade can proceed.
4. **Defense-in-Depth in `InventoryComponent`**:
   - `has_item_stack`: return `false` if `item.quantity == 0`, and sum slot quantities as `u64`.
   - `remove_item_stack`: return `false` if `item.quantity == 0`.
   - `add_item`: return `Err("Item quantity must be greater than zero")` if `item.quantity == 0`.

---

## 2. Participant IDOR Verification (`crates/gameplay/src/village.rs`)

### Vulnerability Analysis
Sub-opcode 6 (`PlayerTradeCancel`) takes `session_id: u64` and invokes `trade_mgr.cancel_session(req.session_id)`. Any player who guesses or enumerates sequential session IDs can terminate another player's trade.

### Remediation
Before calling `cancel_session`, fetch `session` via `trade_mgr.get_session(req.session_id)`.
If not found, return `PacketHandlerError::Custom(format!("Active trade session {} not found", req.session_id))`.
Assert:
```rust
if session.player_a != entity_guid && session.player_b != entity_guid {
    return Err(PacketHandlerError::Custom(
        "Caller is not a participant in this trade session".to_string(),
    ));
}
```
Apply identical validation to sub-opcode 5 (`PlayerTradeAccept`).

---

## 3. Distance Verification & Lifecycle Symmetry (`crates/gameplay/src/trade.rs`, `village.rs`)

### Vulnerability Analysis
- `TradeManager::execute_atomic_swap` executes without checking whether players are still within `MAX_TRADE_DISTANCE` (5.0m).
- `TradeManager::check_distance_and_abort` was implemented but not registered in any hook or tick loop.
- Only `Event::BeforeMove` receives `EntityStore` via `dispatch_with_entities`. However, at `BeforeMove`, `entity.position` in the store is still the old position (`from`).
- If a player despawns/disconnects during trade, the partner remains locked in active trade state.
- Sub-opcode 5 was asymmetric, auto-accepting the second player who locks.
- `TradeSession::lock` and `TradeSession::accept` did not check `self.completed`.

### Remediation
1. **Commit-Time Distance Re-Verification**:
   In `TradeManager::execute_atomic_swap`:
   Extract `player_a` and `player_b` by value from the session first so that mutable session borrowing does not collide with `self.cancel_session`:
   ```rust
   let (p1, p2) = {
       let s = self.sessions.get(&session_id).ok_or("Trade session not found")?;
       if !s.accepted_a || !s.accepted_b {
           return Err("Trade not accepted by both participants");
       }
       (s.player_a, s.player_b)
   };
   let (e1, e2) = entities.get_two_mut(p1, p2).ok_or("Entities not found")?;
   let dx = e1.position.0 - e2.position.0;
   let dy = e1.position.1 - e2.position.1;
   let dz = e1.position.2 - e2.position.2;
   let dist = (dx * dx + dy * dy + dz * dz).sqrt();
   if dist > Self::MAX_TRADE_DISTANCE {
       self.cancel_session(session_id);
       return Err("Players moved too far apart to complete trade");
   }
   ```
2. **Predictive Movement & Despawn Hook (`TradeDistanceHook`)**:
   Implement `HookHandler` for `TradeDistanceHook`:
   - On `Event::BeforeMove { entity, to, .. }`:
     If `entity` is in an active trade session, look up the partner entity in `entities`.
     Compute distance between `*to` (predicted future position) and `partner.position`.
     If distance exceeds `MAX_TRADE_DISTANCE` (5.0m), call `trade_mgr.cancel_session(session_id)`.
   - On `Event::OnDespawn { entity }`:
     If `entity` is in an active trade session, call `trade_mgr.cancel_session(session_id)`.
   - Register `TradeDistanceHook` in `VillagePlugin::build` using its shared `trade_manager`.
3. **Completed Guard**:
   In `TradeSession::lock` and `TradeSession::accept`, return `Err("Trade session already finished")` if `self.completed`.
4. **Symmetric Lock/Accept Step Machine**:
   In sub-opcode 5:
   A call transitions the caller from unlocked to locked (`session.lock(entity_guid)`).
   If the caller is ALREADY locked: the call transitions the caller to accepted (`session.accept(entity_guid)`).
   Each player calls sub-opcode 5 once to lock, and once to accept after both are locked.

---

## 4. Checked Currency Arithmetic (`crates/gameplay/src/village.rs`, `inventory.rs`)

### Remediation
In sub-opcode 2 (`NpcBarter`):
```rust
let rollback = inv.clone();
inv.currency = inv.currency
    .checked_sub(recipe.required_currency)
    .ok_or_else(|| PacketHandlerError::Custom("Insufficient currency".to_string()))?;
for item in &recipe.required_items {
    if !inv.remove_item_stack(item) {
        *inv = rollback;
        return Err(PacketHandlerError::Custom("Failed to deduct required items".to_string()));
    }
}
inv.currency = inv.currency
    .checked_add(recipe.reward_currency)
    .ok_or_else(|| {
        *inv = rollback;
        PacketHandlerError::Custom("Currency overflow".to_string())
    })?;
```

In `InventoryComponent`:
- Aggregate slot quantities with `s.quantity as u64` and compare `total >= item.quantity as u64`.
- In `add_item`: reject `item.quantity == 0` with `Err("Item quantity must be greater than zero")`.

---

## 5. HUD DOM XSS Prevention (`web/src/ui.js`)

### Remediation
Replace template string interpolation into `.innerHTML` with programmatic DOM creation:
- Use `document.createElement("div")`, `document.createElement("span")`, `document.createElement("button")`.
- Set text via `.textContent = ...`.
- Set attributes via `.setAttribute(...)` or `.className = ...`.
- Clear containers via `.replaceChildren()` or `.innerHTML = ""`.
- Update and expand `web/src/__tests__/inspector.test.js` to assert DOM tree elements and test with malicious script payloads like `<img src=x onerror=alert(1)>`.
