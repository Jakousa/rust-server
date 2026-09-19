//! Edge Gateway for Distributed MMO Engine.
//!
//! Provides client transport termination, binary packet framing via FlatBuffers,
//! dynamic session routing tables, and seamless simulation node handovers.

pub mod router;
pub mod server;

pub use router::SessionRouter;
pub use server::{extract_acting_guid, GatewayError, GatewayServer, PacketDispatcher, RoutedPacket};
