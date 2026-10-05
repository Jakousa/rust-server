//! Edge Gateway for Distributed MMO Engine.
//!
//! Provides client transport termination, binary packet framing via FlatBuffers,
//! dynamic session routing tables, and seamless simulation node handovers.

pub mod auth;
pub mod backhaul;
pub mod handover;
pub mod probe;
pub mod quic;
pub mod router;
pub mod server;
pub mod webtransport;

pub use auth::{AuthError, AuthResult, EdgeAuthenticator, JwtClaims, TokenValidator};
pub use backhaul::{BackhaulChannel, BackhaulError, BackhaulPool, BackhaulState};
pub use handover::{HandoverError, HandoverRateLimiter, HandoverTicket};

pub use probe::{
    encode_probe_response, parse_probe_request, parse_probe_response, run_probe_server,
    ProbeRequest, ProbeResponse, MIN_REQUEST_SIZE, PROBE_MAGIC, RESPONSE_SIZE,
};
pub use quic::{
    create_quic_server, generate_self_signed_cert, make_insecure_client_config, make_server_config,
    QuicClientSession, QuicTransportError,
};
pub use router::SessionRouter;
pub use server::{
    extract_acting_guid, GatewayError, GatewayServer, GatewayServiceImpl, PacketDispatcher,
    RoutedPacket,
};
