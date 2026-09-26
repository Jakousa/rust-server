//! Virtual bot client connection session.
//!
//! Manages an asynchronous TCP connection to the Edge Gateway, framing outgoing
//! `ClientPacket` binary requests (movement, ping) and decoding incoming server responses.

use bytes::BytesMut;
use mmo_core::guid::Guid;
use mmo_schema::client::{ClientCodecError, ClientPacket, MovementPacket, PingPacket, Vec3};
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
}

/// Active virtual bot session.
pub struct BotSession {
    pub guid: Guid,
    stream: Option<TcpStream>,
    read_buf: BytesMut,
    pub current_position: Vec3,
    pub current_velocity: Vec3,
    pub last_ping_sent: Option<(u64, Instant)>,
    pub snapback_count: u64,
}

impl BotSession {
    /// Creates a new bot session with initial position and unique GUID.
    pub fn new(guid: Guid, initial_pos: Vec3) -> Self {
        Self {
            guid,
            stream: None,
            read_buf: BytesMut::with_capacity(4096),
            current_position: initial_pos,
            current_velocity: Vec3::ZERO,
            last_ping_sent: None,
            snapback_count: 0,
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
        let packet = ClientPacket::Movement(MovementPacket {
            entity_id: self.guid.raw(),
            position: self.current_position,
            velocity: self.current_velocity,
            yaw: 0.0,
            pitch: 0.0,
            timestamp: timestamp_ms,
            sequence: 1,
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

    /// Reads incoming bytes from the gateway socket and parses all completed frames.
    ///
    /// Returns decoded packets and any observed ping RTT latency in milliseconds.
    pub async fn poll_incoming(
        &mut self,
    ) -> Result<(Vec<ClientPacket>, Option<f64>), SessionError> {
        let stream = self.stream.as_mut().ok_or(SessionError::NotConnected)?;
        let mut rtt_ms = None;

        let bytes_read = stream.read_buf(&mut self.read_buf).await?;
        if bytes_read == 0 {
            self.stream = None;
            return Err(SessionError::Io(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "Connection closed by server",
            )));
        }

        let mut packets = Vec::new();
        while let Some(packet) = ClientPacket::decode_frame(&mut self.read_buf)? {
            match &packet {
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
                _ => {}
            }
            packets.push(packet);
        }

        Ok((packets, rtt_ms))
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
}
