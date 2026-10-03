//! MMO State Persistence and Storage Tier.

#![allow(clippy::double_must_use)]

pub mod cache;
pub mod db;
pub mod models;
pub mod repository;
pub mod social;

pub use cache::{MemoryCache, RedisCache, SessionCacheBackend, SessionData};
pub use db::{sql_for_dialect, Database, DbBackend, StorageError};
pub use models::{
    AccountRecord, CharacterRecord, EntitySnapshot, InventoryRecord, ItemRecord, TradeRecord,
    TradeStatus,
};
pub use repository::CharacterRepository;
pub use social::{
    ChatChannel, ChatMessage, GuildSyncAction, MemorySocialBus, PartyAction, RedisSocialBus,
    SocialBus, SocialBusError, SocialEnvelope,
};
