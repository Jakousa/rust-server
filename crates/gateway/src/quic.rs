//! Native QUIC UDP transport implementation for Edge Gateway.
//!
//! Provides TLS 1.3 certificate generation, QUIC endpoint listeners,
//! hybrid datagram & stream processing, and client connection helpers using `quinn`.

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use quinn::VarInt;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use thiserror::Error;

use mmo_schema::client::{ClientCodecError, ClientPacket};

/// Errors that can occur during QUIC transport operations.
#[derive(Debug, Error)]
pub enum QuicTransportError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("TLS configuration error: {0}")]
    Tls(#[from] rustls::Error),

    #[error("Certificate generation error: {0}")]
    Certificate(#[from] rcgen::Error),

    #[error("Crypto error: {0:?}")]
    Crypto(quinn::crypto::rustls::NoInitialCipherSuite),

    #[error("Quinn connection error: {0}")]
    Connection(#[from] quinn::ConnectionError),

    #[error("Quinn connect error: {0}")]
    Connect(#[from] quinn::ConnectError),

    #[error("Quinn send datagram error: {0}")]
    SendDatagram(#[from] quinn::SendDatagramError),

    #[error("Quinn write error: {0}")]
    Write(#[from] quinn::WriteError),

    #[error("Quinn read error: {0}")]
    Read(#[from] quinn::ReadError),

    #[error("Packet codec error: {0}")]
    Codec(#[from] ClientCodecError),

    #[error("Stream closed unexpectedly")]
    StreamClosed,

    #[error("Incomplete packet frame received in datagram")]
    IncompleteFrame,
}

/// Generates a self-signed TLS 1.3 certificate and private key in DER format.
pub fn generate_self_signed_cert(
    subject_alt_names: &[&str],
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>), rcgen::Error> {
    let names: Vec<String> = subject_alt_names.iter().map(|s| s.to_string()).collect();
    let cert = rcgen::generate_simple_self_signed(names)?;
    let cert_der = cert.cert.der().to_vec();
    let key_der = cert.key_pair.serialized_der().to_vec();
    Ok((
        vec![CertificateDer::from(cert_der)],
        PrivateKeyDer::Pkcs8(key_der.into()),
    ))
}

/// Creates a `quinn::ServerConfig` configured with TLS 1.3 and datagram support.
pub fn make_server_config(
    certs: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> Result<quinn::ServerConfig, QuicTransportError> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let server_crypto = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;

    let quic_crypto = quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)
        .map_err(QuicTransportError::Crypto)?;
    let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(quic_crypto));

    let mut transport_config = quinn::TransportConfig::default();
    transport_config.datagram_receive_buffer_size(Some(1024 * 1024));
    transport_config.datagram_send_buffer_size(1024 * 1024);
    transport_config.max_idle_timeout(Some(quinn::IdleTimeout::from(VarInt::from_u32(30_000))));
    server_config.transport_config(Arc::new(transport_config));

    Ok(server_config)
}

/// Dummy certificate verifier that skips validation for local testing.
#[derive(Debug)]
pub struct SkipServerVerification(Arc<rustls::crypto::CryptoProvider>);

impl Default for SkipServerVerification {
    fn default() -> Self {
        Self::new()
    }
}

impl SkipServerVerification {
    pub fn new() -> Self {
        Self(Arc::new(rustls::crypto::ring::default_provider()))
    }
}

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// Creates a `quinn::ClientConfig` configured with datagrams and a skip-verification verifier for tests.
pub fn make_insecure_client_config() -> quinn::ClientConfig {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client_crypto = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(SkipServerVerification::new()))
        .with_no_client_auth();

    let mut client_config = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto).unwrap(),
    ));

    let mut transport_config = quinn::TransportConfig::default();
    transport_config.datagram_receive_buffer_size(Some(1024 * 1024));
    transport_config.datagram_send_buffer_size(1024 * 1024);
    transport_config.max_idle_timeout(Some(quinn::IdleTimeout::from(VarInt::from_u32(30_000))));
    client_config.transport_config(Arc::new(transport_config));

    client_config
}

/// Creates a QUIC server endpoint bound to the specified UDP address.
pub fn create_quic_server(
    addr: SocketAddr,
    certs: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> Result<quinn::Endpoint, QuicTransportError> {
    let server_config = make_server_config(certs, key)?;
    let endpoint = quinn::Endpoint::server(server_config, addr)?;
    Ok(endpoint)
}

/// Helper client session for connecting and communicating with a QUIC Gateway endpoint.
pub struct QuicClientSession {
    pub endpoint: quinn::Endpoint,
    pub connection: quinn::Connection,
    pub send_stream: Option<quinn::SendStream>,
    pub recv_stream: Option<quinn::RecvStream>,
    pub read_buf: BytesMut,
}

impl QuicClientSession {
    /// Connects to a QUIC gateway server at `server_addr`.
    pub async fn connect(
        server_addr: SocketAddr,
        server_name: &str,
    ) -> Result<Self, QuicTransportError> {
        let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse().unwrap())?;
        endpoint.set_default_client_config(make_insecure_client_config());
        let connection = endpoint.connect(server_addr, server_name)?.await?;
        Ok(Self {
            endpoint,
            connection,
            send_stream: None,
            recv_stream: None,
            read_buf: BytesMut::with_capacity(8192),
        })
    }

    /// Ensures a primary bidirectional stream is open on the connection.
    pub async fn ensure_bi_stream(&mut self) -> Result<(), QuicTransportError> {
        if self.send_stream.is_none() {
            let (send, recv) = self.connection.open_bi().await?;
            self.send_stream = Some(send);
            self.recv_stream = Some(recv);
        }
        Ok(())
    }

    /// Sends a high-frequency packet as an unreliable, unordered QUIC Datagram.
    pub fn send_datagram(&self, pkt: &ClientPacket) -> Result<(), QuicTransportError> {
        let encoded = Bytes::from(pkt.encode());
        self.connection.send_datagram(encoded)?;
        Ok(())
    }

    /// Reads an unreliable packet from an incoming QUIC Datagram.
    pub async fn read_datagram(&self) -> Result<ClientPacket, QuicTransportError> {
        let datagram = self.connection.read_datagram().await?;
        let mut buf = BytesMut::from(datagram.as_ref());
        match ClientPacket::decode_frame(&mut buf)? {
            Some(pkt) => Ok(pkt),
            None => Err(QuicTransportError::IncompleteFrame),
        }
    }

    /// Sends a reliable packet across the bidirectional stream.
    pub async fn send_packet_stream(
        &mut self,
        pkt: &ClientPacket,
    ) -> Result<(), QuicTransportError> {
        self.ensure_bi_stream().await?;
        let encoded = pkt.encode();
        if let Some(ref mut send) = self.send_stream {
            send.write_all(&encoded).await?;
        }
        Ok(())
    }

    /// Reads a reliable packet from the bidirectional stream.
    pub async fn recv_packet_stream(&mut self) -> Result<ClientPacket, QuicTransportError> {
        self.ensure_bi_stream().await?;
        loop {
            if let Some(pkt) = ClientPacket::decode_frame(&mut self.read_buf)? {
                return Ok(pkt);
            }
            let mut chunk = [0u8; 1024];
            if let Some(ref mut recv) = self.recv_stream {
                let n = match recv.read(&mut chunk).await? {
                    Some(n) if n > 0 => n,
                    _ => return Err(QuicTransportError::StreamClosed),
                };
                self.read_buf.extend_from_slice(&chunk[..n]);
            } else {
                return Err(QuicTransportError::StreamClosed);
            }
        }
    }

    /// Closes the QUIC connection cleanly.
    pub fn close(&self, error_code: u32, reason: &str) {
        self.connection
            .close(VarInt::from_u32(error_code), reason.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_schema::client::{MovementPacket, PingPacket, Vec3};

    #[tokio::test]
    async fn test_quic_self_signed_cert_and_endpoint_roundtrip() {
        let (certs, key) =
            generate_self_signed_cert(&["localhost", "127.0.0.1"]).expect("generate cert");
        assert!(!certs.is_empty());

        let server_addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let server_endpoint =
            create_quic_server(server_addr, certs, key).expect("create server endpoint");
        let bound_addr = server_endpoint.local_addr().expect("local addr");

        // Spawn a basic echo server task
        let srv = server_endpoint.clone();
        let server_task = tokio::spawn(async move {
            let incoming = srv.accept().await.expect("incoming conn");
            let conn = incoming.await.expect("handshake");

            // Echo a datagram
            let datagram = conn.read_datagram().await.expect("read datagram");
            conn.send_datagram(datagram).expect("echo datagram");

            // Echo a stream
            let (mut send, mut recv) = conn.accept_bi().await.expect("accept bi");
            let mut buf = vec![0u8; 64];
            let n = recv.read(&mut buf).await.unwrap().unwrap();
            send.write_all(&buf[..n]).await.expect("echo stream");
            let _ = send.finish();

            // Keep connection alive until client closes
            let _ = conn.closed().await;
        });

        // Connect client
        let mut client = QuicClientSession::connect(bound_addr, "localhost")
            .await
            .expect("client connects");

        // 1. Test Datagram flow
        let move_pkt = ClientPacket::Movement(MovementPacket {
            entity_id: 42,
            position: Vec3::new(1.0, 2.0, 3.0),
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            timestamp: 100,
            sequence: 1,
        });

        client.send_datagram(&move_pkt).expect("send datagram");
        let echoed_pkt = client.read_datagram().await.expect("recv datagram");
        assert_eq!(echoed_pkt, move_pkt);

        // 2. Test Stream flow
        let ping_pkt = ClientPacket::Ping(PingPacket { timestamp: 999 });
        client
            .send_packet_stream(&ping_pkt)
            .await
            .expect("send stream");
        let echoed_ping = client.recv_packet_stream().await.expect("recv stream");
        assert_eq!(echoed_ping, ping_pkt);

        client.close(0, "normal shutdown");
        server_endpoint.close(0u32.into(), b"done");
        let _ = server_task.await;
    }
}
