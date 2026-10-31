//! Generic edge packet interception substrate.
//!
//! Lets domain modules (marketplace, auctions, crafting, ...) claim client packets at the
//! gateway edge without the core gateway loop hardcoding any game-specific packet handling.

use std::future::Future;
use std::pin::Pin;

use mmo_schema::client::ClientPacket;

/// Result of offering a packet to an [`EdgePacketInterceptor`].
#[derive(Debug, Clone, PartialEq)]
pub enum EdgeInterceptOutcome {
    /// The interceptor does not handle this packet; try the next interceptor / forward to the node.
    Pass,
    /// The packet was consumed; nothing is sent back (e.g. dropped client-forged packets).
    Handled,
    /// The packet was consumed and the given packet is returned to the originating session.
    Respond(ClientPacket),
}

/// Boxed future returned by [`EdgePacketInterceptor::intercept`].
pub type InterceptFuture<'a> = Pin<Box<dyn Future<Output = EdgeInterceptOutcome> + Send + 'a>>;

/// Domain-agnostic hook allowing edge services to intercept client packets.
///
/// `player_guid` is the authoritative GUID bound to the session by the gateway router
/// (`None` for unauthenticated sessions); interceptors must never trust GUIDs in the payload.
pub trait EdgePacketInterceptor: Send + Sync {
    fn intercept<'a>(
        &'a self,
        player_guid: Option<u64>,
        packet: &'a ClientPacket,
    ) -> InterceptFuture<'a>;
}
