# ACID Item Trading and Transfer Protocol Specification

## Purpose
Specifies atomic, multi-row inventory transactions for secure item trading between players, ensuring complete elimination of item duplication, item loss, and database lock deadlocks.

## Requirements

### Requirement: Deadlock-Free Multi-Character Locking Order
When executing any transaction involving multiple characters (such as player-to-player trade or inventory transfers):
The repository MUST lock character state in strictly increasing numerical order by `guid`:
`lock(min(guid_a, guid_b))` followed by `lock(max(guid_a, guid_b))`.

- **PostgreSQL**: Transaction executes `SELECT guid FROM characters WHERE guid = $1 FOR UPDATE` on `min(guid_a, guid_b)` and then `max(guid_a, guid_b)`.
- **SQLite**: Transaction initiates with `BEGIN IMMEDIATE` to obtain exclusive write lock prior to reads, completely omitting `FOR UPDATE` to avoid SQLite parser syntax errors.

#### Scenario: Symmetrical Concurrent Trades
- **GIVEN** Player A (`guid = 100`) and Player B (`guid = 200`) trading simultaneously
- **AND** Thread 1 executes `trade(100 -> 200)` while Thread 2 concurrently executes `trade(200 -> 100)`
- **WHEN** both transactions acquire row locks
- **THEN** both acquire locks in the identical order `guid = 100`, then `guid = 200`, preventing circular wait deadlocks.

### Requirement: Atomic Item Exchange & Rollback Guarantee
The trading transaction MUST atomically transfer item ownership between characters within a single database transaction.

#### Validation Steps in Transaction:
1. Verify sender possesses item instance ID in their inventory.
2. Verify recipient inventory exists and has sufficient capacity / available slot.
3. Remove or update item instance on sender:
   - For complete stack transfer: reassign `character_guid` to recipient and update `slot` to recipient's target slot.
   - For partial stack transfer: decrement sender quantity, insert new item instance for recipient.
4. Record audit entry in `trades` table with status `"committed"`.
5. If any validation fails (e.g. sender item missing, recipient inventory full, insufficient quantity), transaction is rolled back completely with status `"rolled_back"`.

#### Scenario: Full Inventory Rollback
- **GIVEN** Player A wants to transfer an item to Player B
- **AND** Player B's inventory is at maximum capacity (no free slots)
- **WHEN** the trade execution is attempted
- **THEN** the transaction aborts, no items leave Player A's inventory, no partial rows are committed, and an error is returned.

#### Scenario: Audit Trail Durability
- **GIVEN** a successful trade between Player A and Player B
- **WHEN** the transaction commits
- **THEN** a row in `trades` table exists recording `trade_id`, `sender_guid`, `recipient_guid`, `items_json`, and `timestamp`.
