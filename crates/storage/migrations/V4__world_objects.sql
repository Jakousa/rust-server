-- Migration V4: Persistent World Objects, Dynamic Prefabs & Spatial Sector Streaming
-- Compatible with both SQLite and PostgreSQL.

CREATE TABLE IF NOT EXISTS world_objects (
    world_id BIGINT NOT NULL,
    object_id BIGINT NOT NULL,
    sector_x INT NOT NULL,
    sector_y INT NOT NULL,
    sector_z INT NOT NULL,
    prefab_id INT NOT NULL,
    pos_x REAL NOT NULL,
    pos_y REAL NOT NULL,
    pos_z REAL NOT NULL,
    rot_x REAL NOT NULL,
    rot_y REAL NOT NULL,
    rot_z REAL NOT NULL,
    rot_w REAL NOT NULL,
    scale_x REAL NOT NULL DEFAULT 1.0,
    scale_y REAL NOT NULL DEFAULT 1.0,
    scale_z REAL NOT NULL DEFAULT 1.0,
    bounds_min_x REAL NOT NULL,
    bounds_min_y REAL NOT NULL,
    bounds_min_z REAL NOT NULL,
    bounds_max_x REAL NOT NULL,
    bounds_max_y REAL NOT NULL,
    bounds_max_z REAL NOT NULL,
    state_flags INT NOT NULL DEFAULT 0,
    owner_type TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    health INT,
    max_health INT,
    properties_json TEXT,
    dirty_revision BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (world_id, object_id)
);

CREATE INDEX IF NOT EXISTS idx_world_objects_sector ON world_objects (world_id, sector_x, sector_y, sector_z);
CREATE INDEX IF NOT EXISTS idx_world_objects_updated ON world_objects (world_id, updated_at);
