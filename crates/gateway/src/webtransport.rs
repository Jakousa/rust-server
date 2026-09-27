//! WebTransport (HTTP/3 over QUIC/UDP) transport implementation for Edge Gateway.
//!
//! Provides self-signed ECDSA certificate generation with strict validity constraints (< 14 days),
//! SHA-256 fingerprint extraction for browser client pinning (`serverCertificateHashes`),
//! and asynchronous datagram & stream routing into `SessionRouter`.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use dashmap::DashMap;
use thiserror::Error;
use tokio::sync::mpsc;
use wtransport::tls::Identity;
use wtransport::{Endpoint, ServerConfig};

use mmo_schema::client::ClientCodecError;

use crate::router::SessionRouter;
use crate::server::{PacketDispatcher, RoutedPacket, SessionSink};

/// WebTransport specific transport errors.
#[derive(Debug, Error)]
pub enum WebTransportError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Certificate error: {0}")]
    Certificate(String),

    #[error("WTransport server error: {0}")]
    Server(String),

    #[error("Packet codec error: {0}")]
    Codec(#[from] ClientCodecError),
}

/// Generates a self-signed ECDSA certificate valid for development (< 14 days validity per W3C specification).
pub fn generate_self_signed_cert_webtransport(
    alt_names: &[&str],
    validity_days: u32,
) -> Result<(Identity, String, [u8; 32]), WebTransportError> {
    let days = validity_days.clamp(1, 13);
    let identity = Identity::self_signed_builder()
        .subject_alt_names(alt_names)
        .from_now_utc()
        .validity_days(days)
        .build()
        .map_err(|e| WebTransportError::Certificate(e.to_string()))?;

    let digest = identity.certificate_chain().as_slice()[0].hash();
    let cert_digest_bytes = *digest.as_ref();
    let cert_digest = digest.fmt(wtransport::tls::Sha256DigestFmt::DottedHex);

    Ok((identity, cert_digest, cert_digest_bytes))
}

/// High-performance WebTransport server accepting browser and native HTTP/3 connections.
pub struct WebTransportServer {
    endpoint: Endpoint<wtransport::endpoint::endpoint_side::Server>,
    router: Arc<SessionRouter>,
    dispatcher: PacketDispatcher,
    session_counter: Arc<AtomicU64>,
    session_sinks: Arc<DashMap<u64, SessionSink>>,
    packet_tx: Option<mpsc::Sender<RoutedPacket>>,
    cert_digest: String,
    cert_digest_bytes: [u8; 32],
}

impl WebTransportServer {
    /// Binds a WebTransport HTTP/3 UDP listener to `bind_addr` with self-signed development certificates.
    pub async fn bind(
        bind_addr: SocketAddr,
        router: Arc<SessionRouter>,
        session_sinks: Arc<DashMap<u64, SessionSink>>,
        session_counter: Arc<AtomicU64>,
        packet_tx: Option<mpsc::Sender<RoutedPacket>>,
    ) -> Result<Self, WebTransportError> {
        let (identity, cert_digest, cert_digest_bytes) =
            generate_self_signed_cert_webtransport(&["localhost", "127.0.0.1", "::1"], 10)?;

        let config = ServerConfig::builder()
            .with_bind_address(bind_addr)
            .with_identity(identity)
            .build();

        let endpoint =
            Endpoint::server(config).map_err(|e| WebTransportError::Server(e.to_string()))?;

        let dispatcher = PacketDispatcher::new(router.clone());

        Ok(Self {
            endpoint,
            router,
            dispatcher,
            session_counter,
            session_sinks,
            packet_tx,
            cert_digest,
            cert_digest_bytes,
        })
    }

    /// Returns the local bound socket address.
    pub fn local_addr(&self) -> Result<SocketAddr, WebTransportError> {
        self.endpoint
            .local_addr()
            .map_err(|e| WebTransportError::Server(e.to_string()))
    }

    /// Returns the SHA-256 certificate fingerprint in dotted-hex representation.
    pub fn cert_digest(&self) -> &str {
        &self.cert_digest
    }

    /// Returns the raw 32-byte SHA-256 digest of the certificate.
    pub fn cert_digest_bytes(&self) -> &[u8; 32] {
        &self.cert_digest_bytes
    }

    /// Runs the accept loop, spawning handlers for incoming WebTransport sessions until shutdown.
    pub async fn run(self, mut shutdown_rx: tokio::sync::broadcast::Receiver<()>) {
        loop {
            tokio::select! {
                incoming = self.endpoint.accept() => {
                    let session_req = match incoming.await {
                        Ok(req) => req,
                        Err(_) => continue,
                    };

                    let connection = match session_req.accept().await {
                        Ok(conn) => conn,
                        Err(_) => continue,
                    };

                    let session_id = self.session_counter.fetch_add(1, Ordering::Relaxed);
                    let router = self.router.clone();
                    let dispatcher = self.dispatcher.clone();
                    let session_sinks = self.session_sinks.clone();
                    let packet_tx = self.packet_tx.clone();

                    tokio::spawn(async move {
                        Self::handle_session(
                            connection,
                            session_id,
                            router,
                            dispatcher,
                            session_sinks,
                            packet_tx,
                        ).await;
                    });
                }
                _ = shutdown_rx.recv() => {
                    break;
                }
            }
        }
    }

    /// Spawns the server in a background task and returns a cancellation sender.
    pub fn run_background(
        self,
    ) -> (
        tokio::task::JoinHandle<()>,
        tokio::sync::broadcast::Sender<()>,
    ) {
        let (tx, rx) = tokio::sync::broadcast::channel(1);
        let handle = tokio::spawn(async move {
            self.run(rx).await;
        });
        (handle, tx)
    }

    async fn handle_session(
        connection: wtransport::Connection,
        session_id: u64,
        router: Arc<SessionRouter>,
        dispatcher: PacketDispatcher,
        session_sinks: Arc<DashMap<u64, SessionSink>>,
        packet_tx: Option<mpsc::Sender<RoutedPacket>>,
    ) {
        let (datagram_tx, mut datagram_rx) = mpsc::channel::<Bytes>(1024);
        let (stream_tx, mut stream_rx) = mpsc::unbounded_channel::<Bytes>();

        session_sinks.insert(session_id, SessionSink::new(datagram_tx, stream_tx));
        metrics::gauge!("mmo_gateway_active_sessions").set(session_sinks.len() as f64);

        struct WtSessionGuard {
            session_id: u64,
            router: Arc<SessionRouter>,
            session_sinks: Arc<DashMap<u64, SessionSink>>,
        }
        impl Drop for WtSessionGuard {
            fn drop(&mut self) {
                let _ = self.router.remove_session(self.session_id);
                self.session_sinks.remove(&self.session_id);
                metrics::gauge!("mmo_gateway_active_sessions").set(self.session_sinks.len() as f64);
            }
        }
        let _guard = WtSessionGuard {
            session_id,
            router: router.clone(),
            session_sinks: session_sinks.clone(),
        };

        // Task 1: Datagram Egress (unreliable stream: movement, deltas)
        let conn_egress = connection.clone();
        let datagram_egress_task = tokio::spawn(async move {
            while let Some(bytes) = datagram_rx.recv().await {
                if conn_egress.send_datagram(bytes).is_err() {
                    break;
                }
            }
        });

        // Task 2: Reliable Stream Egress (interactions, trades, auth)
        let conn_stream = connection.clone();
        let stream_egress_task = tokio::spawn(async move {
            let mut maybe_stream: Option<wtransport::SendStream> = None;
            while let Some(bytes) = stream_rx.recv().await {
                if maybe_stream.is_none() {
                    match conn_stream.open_bi().await {
                        Ok(opening) => match opening.await {
                            Ok((send, _)) => maybe_stream = Some(send),
                            Err(_) => break,
                        },
                        Err(_) => break,
                    }
                }
                if let Some(ref mut send) = maybe_stream {
                    if send.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
            }
        });

        // Task 3: Datagram Ingress (unreliable client movement / ACKs)
        let conn_datagram_in = connection.clone();
        let disp_datagram = dispatcher.clone();
        let ptx_datagram = packet_tx.clone();
        let datagram_ingress_task = tokio::spawn(async move {
            while let Ok(datagram) = conn_datagram_in.receive_datagram().await {
                let mut buf = BytesMut::from(&datagram[..]);
                if let Ok(routed) = disp_datagram.process_frames(session_id, &mut buf) {
                    if let Some(ref tx) = ptx_datagram {
                        for pkt in routed {
                            let _ = tx.send(pkt).await;
                        }
                    }
                }
            }
        });

        // Task 4: Stream Ingress (reliable client actions / interactions)
        let conn_stream_in = connection.clone();
        let disp_stream = dispatcher.clone();
        let ptx_stream = packet_tx.clone();
        let stream_ingress_task = tokio::spawn(async move {
            while let Ok((_, mut recv_stream)) = conn_stream_in.accept_bi().await {
                let mut buf = BytesMut::with_capacity(4096);
                let mut temp = [0u8; 1024];
                loop {
                    match recv_stream.read(&mut temp).await {
                        Ok(Some(n)) if n > 0 => {
                            buf.extend_from_slice(&temp[..n]);
                            if let Ok(routed) = disp_stream.process_frames(session_id, &mut buf) {
                                if let Some(ref tx) = ptx_stream {
                                    for pkt in routed {
                                        let _ = tx.send(pkt).await;
                                    }
                                }
                            }
                        }
                        _ => break,
                    }
                }
            }
        });

        tokio::select! {
            _ = datagram_egress_task => {}
            _ = stream_egress_task => {}
            _ = datagram_ingress_task => {}
            _ = stream_ingress_task => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_self_signed_cert_validity_and_hash() {
        let (identity, hash_str, hash_bytes) =
            generate_self_signed_cert_webtransport(&["localhost", "127.0.0.1"], 10)
                .expect("should generate certificate");

        assert_eq!(hash_bytes.len(), 32);
        assert!(!hash_str.is_empty());
        assert!(hash_str.contains(':'));
        assert_eq!(identity.certificate_chain().as_slice().len(), 1);
    }
}
