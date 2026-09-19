//! Edge Gateway Server and Packet Dispatcher.
//!
//! Handles client connection listening, binary packet framing via FlatBuffers /
//! `mmo_schema::client::ClientPacket::decode_frame`, extracting acting GUIDs / session IDs,
//! and dynamically dispatching packets to authoritative simulation nodes via `SessionRouter`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use bytes::BytesMut;
use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use mmo_core::guid::Guid;
use mmo_schema::client::{ClientCodecError, ClientPacket};

use crate::router::SessionRouter;

/// Errors that can occur within the Edge Gateway server or packet dispatcher.
#[derive(Debug, Error)]
pub enum GatewayError {
    #[error("Packet codec error: {0}")]
    Codec(#[from] ClientCodecError),

    #[error("No authoritative route found for session {session_id}, guid: {guid:?}")]
    NoRouteFound {
        session_id: u64,
        guid: Option<Guid>,
    },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Session {0} not found")]
    SessionNotFound(u64),

    #[error("Router lock poisoned")]
    LockPoisoned,
}

/// Represents a client packet that has been resolved and routed to an authoritative simulation node.
#[derive(Debug, Clone, PartialEq)]
pub struct RoutedPacket {
    /// Client transport session ID.
    pub session_id: u64,
    /// Acting player/entity GUID extracted from packet, if present.
    pub guid: Option<Guid>,
    /// Destination authoritative simulation node network address (e.g. "127.0.0.1:50051").
    pub node_address: String,
    /// The decoded client packet.
    pub packet: ClientPacket,
}

impl RoutedPacket {
    pub fn is_movement(&self) -> bool {
        matches!(self.packet, ClientPacket::Movement(_))
    }

    pub fn is_interaction(&self) -> bool {
        matches!(self.packet, ClientPacket::Interaction(_))
    }
}

/// Extracts the primary acting entity `Guid` from a `ClientPacket`, if applicable.
pub fn extract_acting_guid(packet: &ClientPacket) -> Option<Guid> {
    match packet {
        ClientPacket::Movement(p) => Some(Guid::from_raw(p.entity_id)),
        ClientPacket::MovementCorrection(p) => Some(Guid::from_raw(p.entity_id)),
        ClientPacket::Interaction(p) => Some(Guid::from_raw(p.source_guid)),
        ClientPacket::EntityState(p) => Some(Guid::from_raw(p.entity_id)),
        ClientPacket::EntityDespawn(p) => Some(Guid::from_raw(p.entity_id)),
        ClientPacket::InteractionResult(_) | ClientPacket::Ping(_) | ClientPacket::Pong(_) => None,
    }
}

/// Packet framer and dispatcher responsible for decoding streaming client buffers
/// and dynamically routing packets based on `SessionRouter` tables.
#[derive(Clone)]
pub struct PacketDispatcher {
    router: Arc<RwLock<SessionRouter>>,
}

impl PacketDispatcher {
    /// Creates a new `PacketDispatcher` backed by a shared `SessionRouter`.
    pub fn new(router: Arc<RwLock<SessionRouter>>) -> Self {
        Self { router }
    }

    /// Accesses the underlying shared router.
    pub fn router(&self) -> &Arc<RwLock<SessionRouter>> {
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
        let router = self.router.read().map_err(|_| GatewayError::LockPoisoned)?;
        Self::dispatch_with_router(&router, session_id, packet)
    }

    /// Dispatches a single client packet using a direct reference to a `SessionRouter`.
    pub fn dispatch_with_router(
        router: &SessionRouter,
        session_id: u64,
        packet: ClientPacket,
    ) -> Result<RoutedPacket, GatewayError> {
        let guid = extract_acting_guid(&packet);

        let node_address = guid
            .and_then(|g| router.get_route_by_guid(g))
            .or_else(|| router.get_route_by_session(session_id));

        match node_address {
            Some(addr) => Ok(RoutedPacket {
                session_id,
                guid,
                node_address: addr.to_string(),
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
        let router = self.router.read().map_err(|_| GatewayError::LockPoisoned)?;
        Self::process_frames_with_router(&router, session_id, buffer)
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

/// Gateway server managing connection lifecycles, streaming packet framing,
/// and dynamic routing to authoritative simulation nodes.
pub struct GatewayServer {
    router: Arc<RwLock<SessionRouter>>,
    dispatcher: PacketDispatcher,
    session_counter: Arc<AtomicU64>,
    packet_tx: Option<mpsc::Sender<RoutedPacket>>,
}

impl GatewayServer {
    /// Creates a new `GatewayServer` with the given routing table.
    pub fn new(router: Arc<RwLock<SessionRouter>>) -> Self {
        let dispatcher = PacketDispatcher::new(router.clone());
        Self {
            router,
            dispatcher,
            session_counter: Arc::new(AtomicU64::new(1)),
            packet_tx: None,
        }
    }

    /// Creates a new `GatewayServer` that emits all routed packets into an `mpsc::Sender`.
    pub fn with_packet_channel(
        router: Arc<RwLock<SessionRouter>>,
        packet_tx: mpsc::Sender<RoutedPacket>,
    ) -> Self {
        let dispatcher = PacketDispatcher::new(router.clone());
        Self {
            router,
            dispatcher,
            session_counter: Arc::new(AtomicU64::new(1)),
            packet_tx: Some(packet_tx),
        }
    }

    /// Accesses the underlying `SessionRouter`.
    pub fn router(&self) -> &Arc<RwLock<SessionRouter>> {
        &self.router
    }

    /// Accesses the packet dispatcher.
    pub fn dispatcher(&self) -> &PacketDispatcher {
        &self.dispatcher
    }

    /// Generates the next monotonic unique session ID.
    pub fn next_session_id(&self) -> u64 {
        self.session_counter.fetch_add(1, Ordering::Relaxed)
    }

    /// Registers a session in the gateway routing table.
    pub fn register_session(
        &self,
        session_id: u64,
        guid: Guid,
        node_address: String,
    ) -> Result<(), GatewayError> {
        let mut router = self.router.write().map_err(|_| GatewayError::LockPoisoned)?;
        router.register_session(session_id, guid, node_address);
        Ok(())
    }

    /// Updates the destination route for a player during seamless node handovers.
    pub fn update_route(&self, guid: Guid, new_node_address: String) -> Result<bool, GatewayError> {
        let mut router = self.router.write().map_err(|_| GatewayError::LockPoisoned)?;
        Ok(router.update_route(guid, new_node_address))
    }

    /// Resolves the current authoritative route for a player GUID.
    pub fn get_route_by_guid(&self, guid: Guid) -> Result<Option<String>, GatewayError> {
        let router = self.router.read().map_err(|_| GatewayError::LockPoisoned)?;
        Ok(router.get_route_by_guid(guid).map(|s| s.to_string()))
    }

    /// Resolves the current authoritative route for a session ID.
    pub fn get_route_by_session(&self, session_id: u64) -> Result<Option<String>, GatewayError> {
        let router = self.router.read().map_err(|_| GatewayError::LockPoisoned)?;
        Ok(router.get_route_by_session(session_id).map(|s| s.to_string()))
    }

    /// Removes a session from the routing table.
    pub fn remove_session(&self, session_id: u64) -> Result<Option<Guid>, GatewayError> {
        let mut router = self.router.write().map_err(|_| GatewayError::LockPoisoned)?;
        Ok(router.remove_session(session_id))
    }

    /// Binds a TCP connection listener to the specified address.
    pub async fn bind(&self, addr: &str) -> Result<TcpListener, GatewayError> {
        TcpListener::bind(addr).await.map_err(GatewayError::Io)
    }

    /// Handles an active client connection socket:
    /// Streams incoming bytes into a framing buffer, decodes client packets,
    /// resolves routing destination, and dispatches to the internal channel / node.
    pub async fn handle_connection(
        &self,
        mut socket: TcpStream,
        session_id: u64,
    ) -> Result<(), GatewayError> {
        let mut buf = BytesMut::with_capacity(8192);

        loop {
            let bytes_read = socket.read_buf(&mut buf).await.map_err(GatewayError::Io)?;
            if bytes_read == 0 {
                // Client cleanly closed connection
                break;
            }

            let routed_packets = self.dispatcher.process_frames(session_id, &mut buf)?;
            if let Some(ref tx) = self.packet_tx {
                for pkt in routed_packets {
                    let _ = tx.send(pkt).await;
                }
            }
        }

        // Clean up session route on disconnect
        let _ = self.remove_session(session_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::guid::EntityType;
    use mmo_schema::client::{InteractionPacket, InteractionType, MovementPacket, Vec3};
    use tokio::io::AsyncWriteExt;

    #[test]
    fn test_framing_and_dispatch_mock_client_packets() {
        let mut router = SessionRouter::new();
        let player_guid = Guid::new(EntityType::Player, 1, 100);
        let session_id = 1;
        let node_addr = "127.0.0.1:50051".to_string();

        router.register_session(session_id, player_guid, node_addr.clone());

        // 1. Build a MovementPacket
        let move_pkt = ClientPacket::Movement(MovementPacket {
            entity_id: player_guid.raw(),
            position: Vec3::new(10.0, 20.0, 30.0),
            velocity: Vec3::new(1.0, 0.0, -1.0),
            yaw: 1.57,
            pitch: 0.25,
            timestamp: 1700000000,
            sequence: 1,
        });

        // 2. Build an InteractionPacket
        let target_guid = Guid::new(EntityType::Npc, 1, 200);
        let interact_pkt = ClientPacket::Interaction(InteractionPacket {
            source_guid: player_guid.raw(),
            target_guid: target_guid.raw(),
            interaction_type: InteractionType::Attack,
            action_id: 42,
            payload: vec![1, 2, 3, 4],
        });

        // 3. Serialize both into wire format bytes
        let move_bytes = move_pkt.encode();
        let interact_bytes = interact_pkt.encode();

        let mut stream_buffer = BytesMut::new();

        // Feed partial bytes of the first packet (packet framer should return empty)
        stream_buffer.extend_from_slice(&move_bytes[..10]);
        let partial_res =
            PacketDispatcher::process_frames_with_router(&router, session_id, &mut stream_buffer)
                .expect("Framing partial bytes should not fail");
        assert!(partial_res.is_empty());
        assert_eq!(stream_buffer.len(), 10);

        // Feed remainder of movement packet and all of interaction packet
        stream_buffer.extend_from_slice(&move_bytes[10..]);
        stream_buffer.extend_from_slice(&interact_bytes);

        let routed =
            PacketDispatcher::process_frames_with_router(&router, session_id, &mut stream_buffer)
                .expect("Should frame and dispatch all complete packets");

        assert_eq!(routed.len(), 2);
        assert!(stream_buffer.is_empty());

        // Verify MovementPacket routing
        assert_eq!(routed[0].session_id, session_id);
        assert_eq!(routed[0].guid, Some(player_guid));
        assert_eq!(routed[0].node_address, "127.0.0.1:50051");
        assert!(routed[0].is_movement());
        assert_eq!(routed[0].packet, move_pkt);

        // Verify InteractionPacket routing
        assert_eq!(routed[1].session_id, session_id);
        assert_eq!(routed[1].guid, Some(player_guid));
        assert_eq!(routed[1].node_address, "127.0.0.1:50051");
        assert!(routed[1].is_interaction());
        assert_eq!(routed[1].packet, interact_pkt);
    }

    #[test]
    fn test_dynamic_route_handoff_seamless_packet_routing() {
        let router = Arc::new(RwLock::new(SessionRouter::new()));
        let dispatcher = PacketDispatcher::new(router.clone());

        let player_guid = Guid::new(EntityType::Player, 1, 555);
        let session_id = 99;
        let node1 = "127.0.0.1:50051".to_string();
        let node2 = "127.0.0.1:50052".to_string();

        // Step 1: Initial registration to Node 1
        {
            let mut r = router.write().unwrap();
            r.register_session(session_id, player_guid, node1.clone());
        }

        // Step 2: Client sends movement packet #1 while in Node 1's domain
        let pkt1 = ClientPacket::Movement(MovementPacket {
            entity_id: player_guid.raw(),
            position: Vec3::new(10.0, 0.0, 10.0),
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            timestamp: 1000,
            sequence: 1,
        });

        let mut buf = BytesMut::from(&pkt1.encode()[..]);
        let routed1 = dispatcher
            .process_frames(session_id, &mut buf)
            .expect("Packet 1 routing failed");
        assert_eq!(routed1.len(), 1);
        assert_eq!(routed1[0].node_address, node1);

        // Step 3: Client sends movement packet #2 still on Node 1
        let pkt2 = ClientPacket::Movement(MovementPacket {
            entity_id: player_guid.raw(),
            position: Vec3::new(15.0, 0.0, 10.0),
            velocity: Vec3::new(5.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            timestamp: 1050,
            sequence: 2,
        });
        buf.extend_from_slice(&pkt2.encode());
        let routed2 = dispatcher
            .process_frames(session_id, &mut buf)
            .expect("Packet 2 routing failed");
        assert_eq!(routed2.len(), 1);
        assert_eq!(routed2[0].node_address, node1);

        // Step 4: Player crosses boundary! Simulation triggers seamless handover to Node 2.
        {
            let mut r = router.write().unwrap();
            let updated = r.update_route(player_guid, node2.clone());
            assert!(updated, "Route should exist and be updated");
        }

        // Step 5: Client sends movement packet #3 and #4 immediately without reconnecting
        let pkt3 = ClientPacket::Movement(MovementPacket {
            entity_id: player_guid.raw(),
            position: Vec3::new(20.0, 0.0, 10.0),
            velocity: Vec3::new(5.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            timestamp: 1100,
            sequence: 3,
        });
        let pkt4 = ClientPacket::Movement(MovementPacket {
            entity_id: player_guid.raw(),
            position: Vec3::new(25.0, 0.0, 10.0),
            velocity: Vec3::new(5.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            timestamp: 1150,
            sequence: 4,
        });

        buf.extend_from_slice(&pkt3.encode());
        buf.extend_from_slice(&pkt4.encode());

        let routed_handoff = dispatcher
            .process_frames(session_id, &mut buf)
            .expect("Handoff packets routing failed");

        // Both packets routed seamlessly to Node 2 without packet loss
        assert_eq!(routed_handoff.len(), 2);
        assert_eq!(routed_handoff[0].node_address, node2);
        assert_eq!(routed_handoff[0].packet, pkt3);
        assert_eq!(routed_handoff[1].node_address, node2);
        assert_eq!(routed_handoff[1].packet, pkt4);
    }

    #[test]
    fn test_unrouted_packet_returns_error() {
        let router = Arc::new(RwLock::new(SessionRouter::new()));
        let dispatcher = PacketDispatcher::new(router);

        let unk_guid = Guid::new(EntityType::Player, 9, 999);
        let pkt = ClientPacket::Movement(MovementPacket {
            entity_id: unk_guid.raw(),
            position: Vec3::ZERO,
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            timestamp: 0,
            sequence: 1,
        });

        let mut buf = BytesMut::from(&pkt.encode()[..]);
        let res = dispatcher.process_frames(12345, &mut buf);
        assert!(res.is_err());
        match res.unwrap_err() {
            GatewayError::NoRouteFound { session_id, guid } => {
                assert_eq!(session_id, 12345);
                assert_eq!(guid, Some(unk_guid));
            }
            other => panic!("Expected NoRouteFound, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_gateway_server_tcp_listener_and_packet_stream() {
        let router = Arc::new(RwLock::new(SessionRouter::new()));
        let (packet_tx, mut packet_rx) = mpsc::channel(16);

        let server = Arc::new(GatewayServer::with_packet_channel(router.clone(), packet_tx));

        // Bind listener on an ephemeral port
        let listener = server.bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let player_guid = Guid::new(EntityType::Player, 1, 777);
        let session_id = server.next_session_id();
        server
            .register_session(session_id, player_guid, "127.0.0.1:50051".into())
            .unwrap();

        // Spawn server accept task for 1 client
        let server_clone = server.clone();
        let server_handle = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            server_clone.handle_connection(socket, session_id).await
        });

        // Client connects
        let mut client_stream = TcpStream::connect(local_addr).await.unwrap();

        let move_pkt = ClientPacket::Movement(MovementPacket {
            entity_id: player_guid.raw(),
            position: Vec3::new(1.0, 2.0, 3.0),
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            timestamp: 100,
            sequence: 1,
        });

        // Write packet across TCP socket
        client_stream.write_all(&move_pkt.encode()).await.unwrap();

        // Receive routed packet from server channel
        let routed = packet_rx.recv().await.expect("Expected routed packet");
        assert_eq!(routed.session_id, session_id);
        assert_eq!(routed.guid, Some(player_guid));
        assert_eq!(routed.node_address, "127.0.0.1:50051");
        assert_eq!(routed.packet, move_pkt);

        // Close connection and wait for server task
        drop(client_stream);
        let _ = server_handle.await;

        // Session removed on disconnect
        assert_eq!(server.get_route_by_session(session_id).unwrap(), None);
    }
}
