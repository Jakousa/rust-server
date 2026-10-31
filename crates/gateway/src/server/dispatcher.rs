use std::sync::Arc;

use bytes::BytesMut;
use mmo_core::guid::Guid;
use mmo_schema::client::ClientPacket;

use super::{GatewayError, RoutedPacket};
use crate::router::SessionRouter;

/// Default bounded capacity for reliable session stream queues.
pub const DEFAULT_STREAM_CHANNEL_CAPACITY: usize = 1024;

/// Helper to determine if a packet frame is ephemeral (high-frequency movement or sync)
/// suited for unreliable QUIC Datagrams.
pub fn is_ephemeral_packet(bytes: &[u8]) -> bool {
    if bytes.len() >= 4 && bytes[0] == 0x4D && bytes[1] == 0x4D {
        let opcode_raw = u16::from_le_bytes([bytes[2], bytes[3]]);
        if let Ok(opcode) = mmo_schema::client::Opcode::from_u16(opcode_raw) {
            return matches!(
                opcode,
                mmo_schema::client::Opcode::Movement
                    | mmo_schema::client::Opcode::MovementCorrection
                    | mmo_schema::client::Opcode::EntityState
                    | mmo_schema::client::Opcode::EntityDespawn
                    | mmo_schema::client::Opcode::EntityDeltaBatch
                    | mmo_schema::client::Opcode::Ping
                    | mmo_schema::client::Opcode::Pong
            );
        }
    }
    false
}

/// Helper function extracting the acting player or entity GUID from a decoded client packet, if present.
pub fn extract_acting_guid(packet: &ClientPacket) -> Option<Guid> {
    match packet {
        ClientPacket::Movement(p) => Some(Guid::from_raw(p.entity_id)),
        ClientPacket::MovementCorrection(p) => Some(Guid::from_raw(p.entity_id)),
        ClientPacket::Interaction(p) => Some(Guid::from_raw(p.source_guid)),
        ClientPacket::EntityState(p) => Some(Guid::from_raw(p.entity_id)),
        ClientPacket::EntityDespawn(p) => Some(Guid::from_raw(p.entity_id)),
        ClientPacket::SocialMessage(p) => Some(Guid::from_raw(p.sender_guid)),
        ClientPacket::InteractionResult(_)
        | ClientPacket::Ping(_)
        | ClientPacket::Pong(_)
        | ClientPacket::EntityDeltaBatch(_)
        | ClientPacket::ClientAck(_)
        | ClientPacket::WorldObjectSpawn(_)
        | ClientPacket::WorldObjectDelta(_)
        | ClientPacket::WorldObjectDespawn(_)
        | ClientPacket::WorldObjectResync(_)
        | ClientPacket::PlaceObject(_)
        | ClientPacket::InteractObject(_)
        | ClientPacket::AuthRequest(_)
        | ClientPacket::AuthResponse(_)
        | ClientPacket::SocialSubscribe(_)
        | ClientPacket::SocialUnsubscribe(_)
        | ClientPacket::MarketListRequest(_)
        | ClientPacket::MarketBuyRequest(_)
        | ClientPacket::MarketCancelRequest(_)
        | ClientPacket::MarketQueryRequest(_)
        | ClientPacket::MarketResponse(_) => None,
    }
}

/// Packet framer and dispatcher responsible for decoding streaming client buffers
/// and dynamically routing packets based on `SessionRouter` tables.
#[derive(Clone)]
pub struct PacketDispatcher {
    router: Arc<SessionRouter>,
}

impl PacketDispatcher {
    /// Creates a new `PacketDispatcher` backed by a shared lock-free `SessionRouter`.
    pub fn new(router: Arc<SessionRouter>) -> Self {
        Self { router }
    }

    /// Accesses the underlying shared router.
    pub fn router(&self) -> &Arc<SessionRouter> {
        &self.router
    }

    /// Dispatches a single client packet by resolving its destination node address.
    ///
    /// Resolution checks the acting `Guid` first, falling back to the `session_id`.
    pub fn dispatch_packet(
        &self,
        session_id: u64,
        packet: ClientPacket,
    ) -> Result<RoutedPacket, GatewayError> {
        Self::dispatch_with_router(&self.router, session_id, packet)
    }

    /// Dispatches a single client packet using a direct reference to a `SessionRouter`.
    pub fn dispatch_with_router(
        router: &SessionRouter,
        session_id: u64,
        packet: ClientPacket,
    ) -> Result<RoutedPacket, GatewayError> {
        let guid = extract_acting_guid(&packet);

        // Anti-Spoofing: If the packet contains an acting GUID, it must strictly match
        // the session's authenticated GUID in the routing table (if bound).
        if let Some(acting) = guid {
            if let Some(session_guid) = router.get_guid_by_session(session_id) {
                if acting != session_guid {
                    metrics::counter!("mmo_gateway_spoofed_packets_dropped_total").increment(1);
                    return Err(GatewayError::IdentityMismatch {
                        session_id,
                        expected: session_guid,
                        actual: acting,
                    });
                }
            }
        }

        let node_address = guid
            .and_then(|g| router.get_route_by_guid(g))
            .or_else(|| router.get_route_by_session(session_id));

        match node_address {
            Some(addr) => Ok(RoutedPacket {
                session_id,
                guid,
                node_address: addr,
                packet,
            }),
            None => Err(GatewayError::NoRouteFound { session_id, guid }),
        }
    }

    /// Frames and decodes all complete client packets currently in `buffer` using
    /// `ClientPacket::decode_frame`, and dispatches each to its destination node address.
    ///
    /// Incomplete frames remain in `buffer` for subsequent reads.
    pub fn process_frames(
        &self,
        session_id: u64,
        buffer: &mut BytesMut,
    ) -> Result<Vec<RoutedPacket>, GatewayError> {
        Self::process_frames_with_router(&self.router, session_id, buffer)
    }

    /// Frames and decodes client packets directly against a given `SessionRouter`.
    pub fn process_frames_with_router(
        router: &SessionRouter,
        session_id: u64,
        buffer: &mut BytesMut,
    ) -> Result<Vec<RoutedPacket>, GatewayError> {
        let mut routed = Vec::new();
        while let Some(packet) = ClientPacket::decode_frame(buffer).map_err(GatewayError::Codec)? {
            let dispatched = Self::dispatch_with_router(router, session_id, packet)?;
            routed.push(dispatched);
        }
        Ok(routed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_schema::client::Opcode;

    #[test]
    fn test_ephemeral_valid_opcodes() {
        let opcodes = [
            Opcode::Movement as u16,
            Opcode::MovementCorrection as u16,
            Opcode::EntityState as u16,
            Opcode::EntityDespawn as u16,
            Opcode::EntityDeltaBatch as u16,
            Opcode::Ping as u16,
            Opcode::Pong as u16,
        ];

        for opcode in opcodes {
            let [b1, b2] = opcode.to_le_bytes();
            let buf = [0x4D, 0x4D, b1, b2];
            assert!(
                is_ephemeral_packet(&buf),
                "Opcode {:#06x} should be ephemeral",
                opcode
            );
        }
    }

    #[test]
    fn test_ephemeral_non_ephemeral_opcodes() {
        let opcodes = [
            Opcode::Interaction as u16,
            Opcode::InteractionResult as u16,
            Opcode::ClientAck as u16,
            Opcode::AuthRequest as u16,
            Opcode::AuthResponse as u16,
        ];

        for opcode in opcodes {
            let [b1, b2] = opcode.to_le_bytes();
            let buf = [0x4D, 0x4D, b1, b2];
            assert!(
                !is_ephemeral_packet(&buf),
                "Opcode {:#06x} should NOT be ephemeral",
                opcode
            );
        }
    }

    #[test]
    fn test_ephemeral_invalid_header() {
        let [b1, b2] = (Opcode::Movement as u16).to_le_bytes();

        // Invalid first byte
        assert!(!is_ephemeral_packet(&[0x4C, 0x4D, b1, b2]));

        // Invalid second byte
        assert!(!is_ephemeral_packet(&[0x4D, 0x4E, b1, b2]));

        // Both invalid
        assert!(!is_ephemeral_packet(&[0x00, 0x00, b1, b2]));
    }

    #[test]
    fn test_ephemeral_buffer_too_short() {
        assert!(!is_ephemeral_packet(&[]));
        assert!(!is_ephemeral_packet(&[0x4D]));
        assert!(!is_ephemeral_packet(&[0x4D, 0x4D]));
        assert!(!is_ephemeral_packet(&[0x4D, 0x4D, 0x01]));
    }

    #[test]
    fn test_ephemeral_invalid_opcode() {
        // Unknown opcode (e.g., 0xFFFF)
        let [b1, b2] = 0xFFFFu16.to_le_bytes();
        let buf = [0x4D, 0x4D, b1, b2];
        assert!(!is_ephemeral_packet(&buf));
    }

    #[test]
    fn test_ephemeral_longer_buffer() {
        let [b1, b2] = (Opcode::Movement as u16).to_le_bytes();
        let buf = [0x4D, 0x4D, b1, b2, 0x00, 0xFF, 0xAA]; // Extra payload
        assert!(is_ephemeral_packet(&buf));
    }
}
