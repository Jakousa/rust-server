# Proposal: Security Hardening for Trade, NPC Barter, Distance Verification, and HUD DOM

## Problem Statement
During a security and input-validation audit of `crates/gameplay` and `web/` (Issue #154), multiple critical and high-severity security vulnerabilities were identified:
1. **Player Trade Item Forgery & Weight Spoofing (`Sub-opcode 4`)**:
   Clients can submit arbitrary `Vec<ItemStack>` in `PlayerTradeOfferRequest`. Zero-quantity items bypass `has_item_stack` check (`total >= 0` evaluates to `true`), inserting phantom items into the recipient's inventory. Furthermore, client-supplied `unit_weight` is unvalidated and can be negative, zero, or `NaN`, corrupting inventory encumbrance calculation on the recipient.
2. **Third-Party Trade Session Cancellation IDOR (`Sub-opcode 6`)**:
   `cancel_session(session_id)` is invoked without asserting that the calling entity is a participant (`player_a` or `player_b`) in the trade session. Because session IDs are sequential integers, arbitrary clients can cancel any active trade on the node.
3. **Missing Distance Re-Verification & Asymmetric Lock/Accept**:
   `TradeManager::execute_atomic_swap` does not verify the distance between trading partners at commit time, allowing players to initiate a trade within 5.0m, teleport or move far apart, and execute the swap remotely. Furthermore, `check_distance_and_abort` is not hooked into entity movement or simulation ticks, and sub-opcode 5 forces an asymmetric lock-step where one player must invoke the handler twice while the other invokes it once.
4. **Unchecked NPC Barter Currency Arithmetic & Multi-Stack Overflow**:
   NPC barter transactions use unchecked subtraction and addition (`inv.currency -= ...`, `inv.currency += ...`), which panics on underflow or wraps on overflow. Additionally, `InventoryComponent::has_item_stack` sums `u32` quantities into a `u32`, risking arithmetic overflow across multiple inventory stacks.
5. **DOM Cross-Site Scripting (XSS) in UI Controller**:
   `UIController.updateInventory` and `UIController.showNpcBarter` directly interpolate unsanitized item and recipe strings into HTML template literals assigned to `.innerHTML`.

## Proposed Solution
- **Authoritative Trade Offer Resolution**: Reject zero-quantity, negative, non-finite, or unowned items in `PlayerTradeOfferRequest`. Resolve canonical item name and unit weight strictly from the offering player's authoritative inventory slots.
- **Participant IDOR Verification**: Guard sub-opcode 6 (and sub-opcode 5) by requiring that `entity_guid == session.player_a || entity_guid == session.player_b`.
- **Atomic Distance Enforcement & Movement Hook**: Re-verify Euclidean distance ($\le 5.0\text{m}$) inside `TradeManager::execute_atomic_swap`, canceling the session if breached. Register a `TradeDistanceHook` listening to `OnMove`, `BeforeMove`, and `OnTick` to automatically abort sessions when players walk apart. Guard `lock` and `accept` against already completed sessions, and make lock-step transitions symmetric.
- **Safe Currency & Quantity Math**: Use `checked_sub` and `checked_add` for currency mutations with rollback on failure. In `InventoryComponent`, require `quantity > 0` and aggregate slot quantities using `u64`.
- **Safe DOM Rendering**: Refactor `UIController` to build DOM nodes using `document.createElement`, `setAttribute`, and `.textContent`.

## Out of Scope
- Global cross-partition trading across multiple nodes (handled via distributed 2PC containers in storage layer).
- UI styling redesign (visual appearance remains identical; only DOM construction is hardened).
