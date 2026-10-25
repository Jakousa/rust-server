//! MMO State Persistence and Storage Tier.

#![allow(clippy::double_must_use)]

pub mod cache;
pub mod container_repo;
pub mod datastore;
pub mod db;
pub mod escrow_coord;
pub mod marketplace;
pub mod models;
pub mod repository;
pub mod social;
#[cfg(any(test, feature = "test-helpers"))]
pub mod test_helpers;
#[cfg(any(test, feature = "test-helpers"))]
pub use test_helpers::{connect_test_databases, reset_postgres_tables};
pub mod transaction_coord;
pub mod world_repo;

pub use cache::{MemoryCache, RedisCache, SessionCacheBackend, SessionData};
pub use container_repo::ContainerRepository;
pub use datastore::{DataDocument, DataStore, DataStoreError, DocumentStore, LoadReport};
pub use db::{sql_for_dialect, Database, DbBackend, StorageError};
pub use escrow_coord::{hash_secret, EscrowCoordinator};
pub use marketplace::MarketplaceRepository;
pub use models::{
    AccountRecord, CharacterRecord, ContainerRecord, ContainerRecordBuilder,
    ContainerTransactionRecord, EntitySnapshot, EscrowRecord, EscrowStatus, InventoryRecord,
    ItemRecord, ItemRecordBuilder, ListingStatus, MarketListingRecord, ReservationRecord,
    TradeRecord, TradeStatus, WorldObjectRecord,
};
pub use repository::{CharacterRepository, StorageRepository};
pub use social::{
    ChatChannel, ChatMessage, ContainerSyncAction, GuildSyncAction, MemorySocialBus, PartyAction,
    RedisSocialBus, SocialBus, SocialBusError, SocialEnvelope,
};
pub use transaction_coord::TransactionCoordinator;
pub use world_repo::{WorldObjectRepository, WorldRepository};
