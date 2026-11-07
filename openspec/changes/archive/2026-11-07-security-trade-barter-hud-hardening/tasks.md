# Implementation Tasks: Security Hardening for Trade, Barter & HUD

- [x] 1. Hardened Item Validation & Safe Arithmetic (`crates/gameplay/src/inventory.rs`)
  - [x] 1.1 In `InventoryComponent::has_item_stack`, return `false` if `item.quantity == 0` and accumulate slot quantities using `u64`.
  - [x] 1.2 In `InventoryComponent::remove_item_stack`, return `false` if `item.quantity == 0`.
  - [x] 1.3 In `InventoryComponent::add_item`, return `Err("Item quantity must be greater than zero")` if `item.quantity == 0`.
  - [x] 1.4 Add unit tests in `crates/gameplay/src/inventory.rs` testing zero-quantity rejection across `has`, `remove`, and `add`, and multi-stack quantity summation.

- [x] 2. Trade Manager Hardening & Distance Abort Hook (`crates/gameplay/src/trade.rs`)
  - [x] 2.1 In `TradeSession::set_offer`, reset both `locked_a` and `locked_b` (as well as `accepted_a` and `accepted_b`) to `false` when either player modifies their offer.
  - [x] 2.2 Guard `TradeSession::lock` and `TradeSession::accept` with `if self.completed`.
  - [x] 2.3 Re-verify `MAX_TRADE_DISTANCE` in `TradeManager::execute_atomic_swap` (extracting player GUIDs first to avoid borrowck collisions) and abort session if distance $> 5.0\text{m}$.
  - [x] 2.4 Add unit tests in `crates/gameplay/src/trade.rs` covering distance abort during swap, bilateral lock reset on offer change, and `completed` guard.

- [x] 3. Village Trade & Barter Packet Handler Security (`crates/gameplay/src/village.rs`)
  - [x] 3.1 Implement `TradeDistanceHook` for `HookHandler` handling predictive distance checking on `Event::BeforeMove { entity, to, .. }` and cleanup on `Event::OnDespawn { entity }`. Register `TradeDistanceHook` in `VillagePlugin::build`.
  - [x] 3.2 In sub-opcode 2 (`NpcBarter`), use `checked_sub` and `checked_add` with full rollback on overflow.
  - [x] 3.3 In sub-opcode 4 (`PlayerTradeOffer`), reject `quantity == 0`, non-finite weights, negative weights, resolve canonical name and unit weight from authoritative inventory slots, verify currency balance, and validate staged items using a cloned test inventory.
  - [x] 3.4 In sub-opcode 5 (`PlayerTradeAccept`), verify caller is a session participant, enforce symmetric lock-then-accept transitions, and abort if distance exceeded.
  - [x] 3.5 In sub-opcode 6 (`PlayerTradeCancel`), verify caller is a session participant before canceling.
  - [x] 3.6 Add comprehensive exploit and boundary tests in `crates/gameplay/src/village.rs` (zero-quantity exploit, weight spoofing, multi-stack over-offering, bilateral lock reset, IDOR cancel, distance bypass, currency overflow, predictive movement abort, and symmetric lock/accept).

- [x] 4. Frontend DOM XSS Hardening & Test Suite (`web/src/ui.js`, `web/src/__tests__/inspector.test.js`)
  - [x] 4.1 Refactor `UIController.updateInventory` in `web/src/ui.js` to construct DOM nodes using `createElement` and `.textContent`.
  - [x] 4.2 Refactor `UIController.showNpcBarter` in `web/src/ui.js` to construct DOM nodes using `createElement` and `.textContent`.
  - [x] 4.3 Update and expand Vitest tests in `web/src/__tests__/inspector.test.js` to assert DOM tree elements and verify anti-XSS safety with script injection strings.

- [x] 5. Quality Gates & Verification
  - [x] 5.1 Run `cargo fmt --all -- --check`
  - [x] 5.2 Run `cargo clippy --workspace --all-targets -- -D warnings`
  - [x] 5.3 Run `cargo test --workspace`
  - [x] 5.4 Run `cd web && npm test && npm run build`
