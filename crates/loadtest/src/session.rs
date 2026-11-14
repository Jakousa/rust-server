//! Virtual bot client connection session.
//!
//! Manages an asynchronous TCP connection to the Edge Gateway, framing outgoing
//! `ClientPacket` binary requests (movement, ping) and decoding incoming server responses.

use bytes::BytesMut;
use mmo_core::guid::Guid;
use mmo_schema::client::{
    delta_flags, AuthRequestPacket, AuthResponsePacket, ClientAckPacket, ClientCodecError,
    ClientPacket, MovementPacket, PingPacket, Vec3,
};
use std::collections::HashMap;
use std::time::Instant;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Errors that can occur during bot session lifecycle.
#[derive(Debug, Error)]
pub enum SessionError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Codec error: {0}")]
    Codec(#[from] ClientCodecError),

    #[error("Not connected")]
    NotConnected,

    #[error("Authentication rejected (code {error_code}): {error_message}")]
    AuthRejected {
        error_code: u32,
        error_message: String,
    },

    #[error("Authentication timed out")]
    AuthTimeout,
}

/// Active virtual bot session.
pub struct BotSession {
    pub guid: Guid,
    pub session_id: u64,
    pub sequence: u32,
    stream: Option<TcpStream>,
    read_buf: BytesMut,
    pub current_position: Vec3,
    pub current_velocity: Vec3,
    pub last_ping_sent: Option<(u64, Instant)>,
    pub snapback_count: u64,
    pub last_observed_tick: u64,
    pub total_bytes_received: u64,
    pub known_entities: HashMap<u64, Vec3>,
}

impl BotSession {
    /// Creates a new bot session with initial position and unique GUID.
    pub fn new(guid: Guid, initial_pos: Vec3) -> Self {
        Self {
            guid,
            session_id: 0,
            sequence: 0,
            stream: None,
            read_buf: BytesMut::with_capacity(4096),
            current_position: initial_pos,
            current_velocity: Vec3::ZERO,
            last_ping_sent: None,
            snapback_count: 0,
            last_observed_tick: 0,
            total_bytes_received: 0,
            known_entities: HashMap::new(),
        }
    }

    /// Connects to the Edge Gateway at `target_addr`.
    pub async fn connect(&mut self, target_addr: &str) -> Result<(), SessionError> {
        let stream = TcpStream::connect(target_addr).await?;
        stream.set_nodelay(true)?;
        self.stream = Some(stream);
        Ok(())
    }

    /// Checks if the session is currently connected.
    pub fn is_connected(&self) -> bool {
        self.stream.is_some()
    }

    /// Encodes and sends a client packet to the gateway.
    pub async fn send_packet(&mut self, packet: &ClientPacket) -> Result<(), SessionError> {
        let stream = self.stream.as_mut().ok_or(SessionError::NotConnected)?;
        let encoded = packet.encode();
        stream.write_all(&encoded).await?;
        Ok(())
    }

    /// Sends a movement packet with current position and velocity.
    pub async fn send_movement(&mut self, timestamp_ms: u64) -> Result<(), SessionError> {
        self.sequence = self.sequence.wrapping_add(1);
        let packet = ClientPacket::Movement(MovementPacket {
            entity_id: self.guid.raw(),
            position: self.current_position,
            velocity: self.current_velocity,
            yaw: 0.0,
            pitch: 0.0,
            timestamp: timestamp_ms,
            sequence: self.sequence,
        });
        self.send_packet(&packet).await
    }

    /// Sends a ping packet and records the dispatched timestamp for RTT calculation.
    pub async fn send_ping(&mut self, timestamp_ms: u64) -> Result<(), SessionError> {
        let packet = ClientPacket::Ping(PingPacket {
            timestamp: timestamp_ms,
        });
        self.send_packet(&packet).await?;
        self.last_ping_sent = Some((timestamp_ms, Instant::now()));
        Ok(())
    }

    /// Sends an `AuthRequestPacket` with `token` and awaits the server's `AuthResponsePacket`.
    ///
    /// On success, stores the server-assigned `session_id` (and updates `self.guid` when
    /// `player_guid != 0`). Returns `SessionError::AuthRejected` if the server denies admission
    /// or `SessionError::AuthTimeout` if no response arrives within `timeout`.
    pub async fn authenticate(
        &mut self,
        token: impl Into<String>,
        timeout: std::time::Duration,
    ) -> Result<AuthResponsePacket, SessionError> {
        if !self.is_connected() {
            return Err(SessionError::NotConnected);
        }
        let auth_packet = ClientPacket::AuthRequest(AuthRequestPacket {
            token: token.into(),
            client_version: 1,
        });
        self.send_packet(&auth_packet).await?;

        let deadline = Instant::now() + timeout;
        loop {
            // Decode one frame at a time so any trailing frames coalesced behind AuthResponse
            // remain intact in `self.read_buf` for subsequent tick polling.
            while let Some(pkt) = ClientPacket::decode_frame(&mut self.read_buf)? {
                let _ = self.handle_decoded_packet(&pkt);
                if let ClientPacket::AuthResponse(resp) = pkt {
                    return if resp.success {
                        Ok(resp)
                    } else {
                        Err(SessionError::AuthRejected {
                            error_code: resp.error_code,
                            error_message: resp.error_message,
                        })
                    };
                }
            }

            let now = Instant::now();
            if now >= deadline {
                return Err(SessionError::AuthTimeout);
            }
            let remaining = deadline.saturating_duration_since(now);
            match tokio::time::timeout(remaining, self.read_more_bytes()).await {
                Ok(Ok(_)) => {}
                Ok(Err(err)) => return Err(err),
                Err(_) => return Err(SessionError::AuthTimeout),
            }
        }
    }

    /// Consumes the session and returns its active `TcpStream` and unparsed read buffer.
    pub fn into_parts(self) -> Option<(TcpStream, BytesMut)> {
        self.stream.map(|stream| (stream, self.read_buf))
    }

    async fn read_more_bytes(&mut self) -> Result<usize, SessionError> {
        let stream = self.stream.as_mut().ok_or(SessionError::NotConnected)?;
        let bytes_read = stream.read_buf(&mut self.read_buf).await?;
        if bytes_read == 0 {
            self.stream = None;
            return Err(SessionError::Io(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "Connection closed by server",
            )));
        }
        self.total_bytes_received += bytes_read as u64;
        Ok(bytes_read)
    }

    fn handle_decoded_packet(&mut self, packet: &ClientPacket) -> Option<f64> {
        let mut rtt_ms = None;
        match packet {
            ClientPacket::AuthResponse(resp) => {
                if resp.success {
                    self.session_id = resp.session_id;
                    if resp.player_guid != 0 {
                        self.guid = Guid::from_raw(resp.player_guid);
                    }
                }
            }
            ClientPacket::Pong(pong) => {
                if let Some((ts, sent_time)) = self.last_ping_sent {
                    if ts == pong.client_timestamp {
                        let elapsed = sent_time.elapsed().as_secs_f64() * 1000.0;
                        rtt_ms = Some(elapsed);
                        self.last_ping_sent = None;
                    }
                }
            }
            ClientPacket::MovementCorrection(corr) if corr.entity_id == self.guid.raw() => {
                self.snapback_count += 1;
                self.current_position = corr.corrected_position;
                self.current_velocity = Vec3::ZERO;
            }
            ClientPacket::EntityDeltaBatch(batch) => {
                self.last_observed_tick = self.last_observed_tick.max(batch.server_tick);
                for &despawn_id in &batch.despawns {
                    self.known_entities.remove(&despawn_id);
                }
                for delta in &batch.deltas {
                    if delta.field_mask & delta_flags::MASK_KEYFRAME != 0 {
                        self.known_entities.insert(
                            delta.entity_id,
                            Vec3::new(delta.position.0, delta.position.1, delta.position.2),
                        );
                    } else if let Some(ent) = self.known_entities.get_mut(&delta.entity_id) {
                        if delta.field_mask & delta_flags::MASK_DELTA_POS_X != 0 {
                            ent.x += delta.position.0;
                        }
                        if delta.field_mask & delta_flags::MASK_DELTA_POS_Y != 0 {
                            ent.y += delta.position.1;
                        }
                        if delta.field_mask & delta_flags::MASK_DELTA_POS_Z != 0 {
                            ent.z += delta.position.2;
                        }
                    }
                }
            }
            ClientPacket::EntityState(state) => {
                self.known_entities.insert(state.entity_id, state.position);
            }
            ClientPacket::EntityDespawn(despawn) => {
                self.known_entities.remove(&despawn.entity_id);
            }
            _ => {}
        }
        rtt_ms
    }

    fn decode_buffered_packets(
        &mut self,
    ) -> Result<(Vec<ClientPacket>, Option<f64>), SessionError> {
        let mut rtt_ms = None;
        let mut packets = Vec::new();
        while let Some(packet) = ClientPacket::decode_frame(&mut self.read_buf)? {
            if let Some(rtt) = self.handle_decoded_packet(&packet) {
                rtt_ms = Some(rtt);
            }
            packets.push(packet);
        }

        Ok((packets, rtt_ms))
    }

    /// Reads incoming bytes from the gateway socket and parses all completed frames.
    ///
    /// Returns decoded packets and any observed ping RTT latency in milliseconds.
    pub async fn poll_incoming(
        &mut self,
    ) -> Result<(Vec<ClientPacket>, Option<f64>), SessionError> {
        self.read_more_bytes().await?;
        self.decode_buffered_packets()
    }

    /// Dispatches a baseline acknowledgement confirming the latest observed tick.
    pub async fn send_ack(&mut self, session_id: u64) -> Result<(), SessionError> {
        let packet = ClientPacket::ClientAck(ClientAckPacket {
            session_id,
            observed_tick: self.last_observed_tick,
            ack_mask: 0,
        });
        self.send_packet(&packet).await
    }

    /// Disconnects the session by dropping the TCP stream.
    pub fn disconnect(&mut self) {
        self.stream = None;
    }

    /// Reads incoming bytes with a timeout, returning an empty packet vector if the timeout elapses.
    pub async fn poll_incoming_timeout(
        &mut self,
        duration: std::time::Duration,
    ) -> Result<(Vec<ClientPacket>, Option<f64>), SessionError> {
        if self.stream.is_none() {
            return Err(SessionError::NotConnected);
        }
        match tokio::time::timeout(duration, self.poll_incoming()).await {
            Ok(res) => res,
            Err(_) => Ok((Vec::new(), None)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::guid::EntityType;
    use std::time::Duration;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn test_bot_session_send_and_receive() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = BytesMut::with_capacity(1024);
            socket.read_buf(&mut buf).await.unwrap();

            let pkt = ClientPacket::decode_frame(&mut buf).unwrap().unwrap();
            if let ClientPacket::Ping(ping) = pkt {
                let pong = ClientPacket::Pong(mmo_schema::client::PongPacket {
                    client_timestamp: ping.timestamp,
                    server_timestamp: 9999,
                });
                let out = pong.encode();
                socket.write_all(&out).await.unwrap();
            }
        });

        let guid = Guid::new(EntityType::Player, 1, 42);
        let mut session = BotSession::new(guid, Vec3::new(10.0, 0.0, 10.0));
        session.connect(&local_addr.to_string()).await.unwrap();
        assert!(session.is_connected());

        session.send_ping(12345).await.unwrap();

        tokio::time::sleep(Duration::from_millis(10)).await;
        let (packets, rtt) = session.poll_incoming().await.unwrap();
        assert_eq!(packets.len(), 1);
        assert!(rtt.is_some());
        assert!(rtt.unwrap() >= 0.0);

        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn test_bot_session_authenticate_success_and_poll_incoming() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();
        let assigned_guid = Guid::new(EntityType::Player, 1, 9001);

        let server_task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = BytesMut::with_capacity(1024);
            socket.read_buf(&mut buf).await.unwrap();
            let pkt = ClientPacket::decode_frame(&mut buf).unwrap().unwrap();
            match pkt {
                ClientPacket::AuthRequest(req) => {
                    assert_eq!(req.token, "valid-token");
                    let resp = ClientPacket::AuthResponse(AuthResponsePacket {
                        success: true,
                        error_code: 0,
                        session_id: 777,
                        player_guid: assigned_guid.raw(),
                        assigned_node: "127.0.0.1:50052".to_string(),
                        error_message: String::new(),
                    });
                    socket.write_all(&resp.encode()).await.unwrap();
                }
                other => panic!("Expected AuthRequest, got {other:?}"),
            }
        });

        let initial_guid = Guid::new(EntityType::Player, 1, 1);
        let mut session = BotSession::new(initial_guid, Vec3::ZERO);
        session.connect(&local_addr.to_string()).await.unwrap();
        let resp = session
            .authenticate("valid-token", Duration::from_secs(2))
            .await
            .unwrap();
        assert!(resp.success);
        assert_eq!(session.session_id, 777);
        assert_eq!(session.guid, assigned_guid);
        server_task.await.unwrap();

        // Also verify poll_incoming updates session_id on AuthResponse directly
        let listener2 = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr2 = listener2.local_addr().unwrap();
        let server_task2 = tokio::spawn(async move {
            let (mut socket, _) = listener2.accept().await.unwrap();
            let resp = ClientPacket::AuthResponse(AuthResponsePacket {
                success: true,
                error_code: 0,
                session_id: 888,
                player_guid: 0,
                assigned_node: "127.0.0.1:50052".to_string(),
                error_message: String::new(),
            });
            socket.write_all(&resp.encode()).await.unwrap();
        });
        let mut session2 = BotSession::new(initial_guid, Vec3::ZERO);
        session2.connect(&local_addr2.to_string()).await.unwrap();
        let (pkts, _) = session2
            .poll_incoming_timeout(Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(pkts.len(), 1);
        assert_eq!(session2.session_id, 888);
        assert_eq!(session2.guid, initial_guid);
        server_task2.await.unwrap();
    }

    #[tokio::test]
    async fn test_bot_session_authenticate_rejected_and_timeout() {
        // Rejection path
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = BytesMut::with_capacity(1024);
            socket.read_buf(&mut buf).await.unwrap();
            let _ = ClientPacket::decode_frame(&mut buf).unwrap().unwrap();
            let resp = ClientPacket::AuthResponse(AuthResponsePacket {
                success: false,
                error_code: 3,
                session_id: 0,
                player_guid: 0,
                assigned_node: String::new(),
                error_message: "invalid signature".to_string(),
            });
            socket.write_all(&resp.encode()).await.unwrap();
        });

        let mut session = BotSession::new(Guid::new(EntityType::Player, 1, 2), Vec3::ZERO);
        session.connect(&local_addr.to_string()).await.unwrap();
        let err = session
            .authenticate("bad-token", Duration::from_secs(2))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            SessionError::AuthRejected { error_code: 3, .. }
        ));
        assert_eq!(session.session_id, 0);
        server_task.await.unwrap();

        // Timeout path
        let listener2 = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr2 = listener2.local_addr().unwrap();
        let _hold_server = tokio::spawn(async move {
            let (_socket, _) = listener2.accept().await.unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;
        });

        let mut session2 = BotSession::new(Guid::new(EntityType::Player, 1, 3), Vec3::ZERO);
        session2.connect(&local_addr2.to_string()).await.unwrap();
        let err2 = session2
            .authenticate("any-token", Duration::from_millis(40))
            .await
            .unwrap_err();
        assert!(matches!(err2, SessionError::AuthTimeout));
    }
}
