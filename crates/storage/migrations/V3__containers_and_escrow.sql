-- Migration V3: Distributed Transactional Containers & Generic Escrow
-- Compatible with both SQLite and PostgreSQL.

-- 1. Create containers table
CREATE TABLE IF NOT EXISTS containers (
    container_id BIGINT PRIMARY KEY,
    owner_type TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    name TEXT NOT NULL,
    capacity INTEGER NOT NULL,
    access_policy TEXT NOT NULL,
    version BIGINT NOT NULL DEFAULT 1,
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_containers_owner ON containers (owner_type, owner_id);

-- 2. Create container reservations table
CREATE TABLE IF NOT EXISTS container_reservations (
    reservation_id TEXT PRIMARY KEY,
    tx_id TEXT NOT NULL,
    container_id BIGINT NOT NULL,
    slot INTEGER NOT NULL,
    item_instance_id TEXT,
    reservation_type TEXT NOT NULL,
    expires_at_epoch_ms BIGINT NOT NULL,
    created_at_epoch_ms BIGINT NOT NULL,
    FOREIGN KEY (container_id) REFERENCES containers(container_id) ON DELETE CASCADE,
    CONSTRAINT uq_container_slot_res UNIQUE(container_id, slot)
);

CREATE INDEX IF NOT EXISTS idx_reservations_tx ON container_reservations(tx_id);
CREATE INDEX IF NOT EXISTS idx_reservations_expiry ON container_reservations(expires_at_epoch_ms);

-- 3. Create escrows table
CREATE TABLE IF NOT EXISTS escrows (
    escrow_id TEXT PRIMARY KEY,
    depositor_container_id BIGINT NOT NULL,
    beneficiary_container_id BIGINT NOT NULL,
    escrow_container_id BIGINT NOT NULL,
    status TEXT NOT NULL,
    secret_hash TEXT NOT NULL,
    expires_at_epoch_ms BIGINT NOT NULL,
    created_at_epoch_ms BIGINT NOT NULL,
    updated_at_epoch_ms BIGINT NOT NULL,
    FOREIGN KEY (depositor_container_id) REFERENCES containers(container_id) ON DELETE CASCADE,
    FOREIGN KEY (beneficiary_container_id) REFERENCES containers(container_id) ON DELETE CASCADE,
    FOREIGN KEY (escrow_container_id) REFERENCES containers(container_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_escrows_status_expiry ON escrows(status, expires_at_epoch_ms);

-- 4. Create container transactions ledger table
CREATE TABLE IF NOT EXISTS container_transactions (
    tx_id TEXT PRIMARY KEY,
    src_container_id BIGINT NOT NULL,
    dst_container_id BIGINT NOT NULL,
    item_instance_id TEXT NOT NULL,
    status TEXT NOT NULL,
    timestamp_epoch_ms BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_container_tx_time ON container_transactions(timestamp_epoch_ms);

-- 5. Backfill items container_id column
ALTER TABLE items ADD COLUMN IF NOT EXISTS container_id BIGINT DEFAULT 0;
CREATE INDEX IF NOT EXISTS idx_items_container_slot ON items(container_id, slot);
