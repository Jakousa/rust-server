//! Data models for persistence tier.

use serde::{Deserialize, Serialize};

/// Account persistent record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountRecord {
    pub account_id: String,
    pub username: String,
    pub created_at: i64,
}

/// Character persistent record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CharacterRecord {
    pub guid: u64,
    pub account_id: String,
    pub name: String,
    pub level: i32,
    pub position: (f32, f32, f32),
    pub rotation: f32,
    pub attributes: serde_json::Value,
    pub updated_at: i64,
}

/// Decoupled entity snapshot representing in-memory state captured for persistence or handover.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntitySnapshot {
    pub guid: u64,
    pub position: (f32, f32, f32),
    pub rotation: f32,
    pub attributes: serde_json::Value,
    pub version: u64,
    pub timestamp: u64,
}
