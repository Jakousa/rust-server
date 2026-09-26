//! Edge Gateway for Distributed MMO Engine.
//!
//! Provides client transport termination, binary packet framing via FlatBuffers,
//! dynamic session routing tables, and seamless simulation node handovers.

pub mod quic;
pub mod router;
pub mod server;

pub use quic::{
    create_quic_server, generate_self_signed_cert, make_insecure_client_config, make_server_config,
    QuicClientSession, QuicTransportError,
};
pub use router::SessionRouter;
pub use server::{
    extract_acting_guid, GatewayError, GatewayServer, GatewayServiceImpl, PacketDispatcher,
    RoutedPacket,
};
