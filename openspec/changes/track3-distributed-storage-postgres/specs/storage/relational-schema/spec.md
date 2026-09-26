# Relational Schema Specification

## Purpose
Defines the relational data model for accounts, characters, inventories, items, snapshots, and trade logs, ensuring cross-dialect compatibility between PostgreSQL and SQLite.

## Requirements

### Requirement: Account and Character Schema
The database MUST persist player accounts and characters with foreign key referential integrity.

#### Schema Definitions:
- `accounts`:
  - `account_id` (TEXT PRIMARY KEY)
  - `username` (TEXT NOT NULL UNIQUE)
  - `created_at` (BIGINT NOT NULL)
- `characters`:
  - `guid` (BIGINT PRIMARY KEY)
  - `account_id` (TEXT NOT NULL REFERENCES accounts(account_id) ON DELETE CASCADE)
  - `name` (TEXT NOT NULL)
  - `level` (INTEGER NOT NULL DEFAULT 1)
  - `experience` (BIGINT NOT NULL DEFAULT 0)
  - `pos_x` (REAL NOT NULL)
  - `pos_y` (REAL NOT NULL)
  - `pos_z` (REAL NOT NULL)
  - `rotation` (REAL NOT NULL DEFAULT 0.0)
  - `attributes_json` (TEXT NOT NULL DEFAULT '{}')
  - `updated_at` (BIGINT NOT NULL)
  - Index on `account_id`

#### Scenario: Creating Account and Character
- **GIVEN** a newly registered account `acc_42`
- **WHEN** character `guid = 1001`, `name = "Aric"`, `pos = (0, 0, 0)` is inserted
- **THEN** both records are durably committed and querying character 1001 resolves the associated account.

### Requirement: Inventory and Item Slot Schema
The database MUST maintain character inventory metadata and discrete item instances located in distinct slots.

#### Schema Definitions:
- `inventories`:
  - `character_guid` (BIGINT PRIMARY KEY REFERENCES characters(guid) ON DELETE CASCADE)
  - `capacity` (INTEGER NOT NULL DEFAULT 30)
  - `updated_at` (BIGINT NOT NULL)
- `items`:
  - `item_instance_id` (TEXT PRIMARY KEY)
  - `character_guid` (BIGINT NOT NULL REFERENCES characters(guid) ON DELETE CASCADE)
  - `item_def_id` (INTEGER NOT NULL)
  - `slot` (INTEGER NOT NULL)
  - `quantity` (INTEGER NOT NULL DEFAULT 1)
  - `durability` (INTEGER NOT NULL DEFAULT 100)
  - `attributes_json` (TEXT NOT NULL DEFAULT '{}')
  - `created_at` (BIGINT NOT NULL)
  - `updated_at` (BIGINT NOT NULL)
  - Constraint: `UNIQUE(character_guid, slot)` to guarantee slot exclusivity.
  - Index on `(character_guid, slot)`

#### Scenario: Enforcing Slot Exclusivity
- **GIVEN** character `1001` already has item `item_sword` in slot `0`
- **WHEN** an attempt is made to insert `item_shield` into character `1001` at slot `0`
- **THEN** the database rejects the insertion with a unique constraint violation.
