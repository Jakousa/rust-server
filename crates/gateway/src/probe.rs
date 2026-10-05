//! Anycast latency & health UDP probing with reflection amplification defense.
//!
//! Enforces a minimum 64-byte request size to guarantee an amplification factor <= 1.0,
//! neutralizing UDP DDoS reflection attacks.

use std::convert::TryInto;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::net::UdpSocket;

pub const PROBE_MAGIC: u32 = 0x4D4D4F50; // "MMOP"
pub const MIN_REQUEST_SIZE: usize = 64;
pub const RESPONSE_SIZE: usize = 64;

/// Incoming probe request parsed from client datagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeRequest {
    pub client_timestamp_ms: u64,
    pub client_nonce: u32,
}

/// Outgoing probe response with server telemetry and region metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResponse {
    pub client_timestamp_ms: u64,
    pub client_nonce: u32,
    pub server_timestamp_ms: u64,
    pub active_sessions: u32,
    pub capacity: u32,
    pub region: String,
}

/// Parses a raw probe request buffer.
///
/// Returns `None` if `buf.len() < MIN_REQUEST_SIZE` (64 bytes) or magic is invalid.
pub fn parse_probe_request(buf: &[u8]) -> Option<ProbeRequest> {
    if buf.len() < MIN_REQUEST_SIZE {
        return None;
    }

    let magic = u32::from_be_bytes(buf[0..4].try_into().ok()?);
    if magic != PROBE_MAGIC {
        return None;
    }

    let client_timestamp_ms = u64::from_be_bytes(buf[4..12].try_into().ok()?);
    let client_nonce = u32::from_be_bytes(buf[12..16].try_into().ok()?);

    Some(ProbeRequest {
        client_timestamp_ms,
        client_nonce,
    })
}

/// Encodes a probe response into a fixed 64-byte buffer.
pub fn encode_probe_response(resp: &ProbeResponse) -> [u8; RESPONSE_SIZE] {
    let mut buf = [0u8; RESPONSE_SIZE];
    buf[0..4].copy_from_slice(&PROBE_MAGIC.to_be_bytes());
    buf[4..12].copy_from_slice(&resp.client_timestamp_ms.to_be_bytes());
    buf[12..16].copy_from_slice(&resp.client_nonce.to_be_bytes());
    buf[16..24].copy_from_slice(&resp.server_timestamp_ms.to_be_bytes());
    buf[24..28].copy_from_slice(&resp.active_sessions.to_be_bytes());
    buf[28..32].copy_from_slice(&resp.capacity.to_be_bytes());

    let region_bytes = resp.region.as_bytes();
    let region_len = region_bytes.len().min(31);
    buf[32] = region_len as u8;
    buf[33..33 + region_len].copy_from_slice(&region_bytes[..region_len]);

    buf
}

/// Parses a 64-byte probe response buffer.
pub fn parse_probe_response(buf: &[u8]) -> Option<ProbeResponse> {
    if buf.len() != RESPONSE_SIZE {
        return None;
    }

    let magic = u32::from_be_bytes(buf[0..4].try_into().ok()?);
    if magic != PROBE_MAGIC {
        return None;
    }

    let client_timestamp_ms = u64::from_be_bytes(buf[4..12].try_into().ok()?);
    let client_nonce = u32::from_be_bytes(buf[12..16].try_into().ok()?);
    let server_timestamp_ms = u64::from_be_bytes(buf[16..24].try_into().ok()?);
    let active_sessions = u32::from_be_bytes(buf[24..28].try_into().ok()?);
    let capacity = u32::from_be_bytes(buf[28..32].try_into().ok()?);
    let region_len = (buf[32] as usize).min(31);
    let region = String::from_utf8(buf[33..33 + region_len].to_vec()).ok()?;

    Some(ProbeResponse {
        client_timestamp_ms,
        client_nonce,
        server_timestamp_ms,
        active_sessions,
        capacity,
        region,
    })
}

/// Runs the UDP probe responder task.
pub async fn run_probe_server(
    socket: UdpSocket,
    region: String,
    capacity: u32,
    active_sessions: Arc<AtomicU32>,
) {
    let mut recv_buf = [0u8; 512];
    loop {
        let (len, src) = match socket.recv_from(&mut recv_buf).await {
            Ok(res) => res,
            Err(_) => continue,
        };

        if let Some(req) = parse_probe_request(&recv_buf[..len]) {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);

            let resp = ProbeResponse {
                client_timestamp_ms: req.client_timestamp_ms,
                client_nonce: req.client_nonce,
                server_timestamp_ms: now_ms,
                active_sessions: active_sessions.load(Ordering::Relaxed),
                capacity,
                region: region.clone(),
            };

            let encoded = encode_probe_response(&resp);
            let _ = socket.send_to(&encoded, src).await;
            metrics::counter!("mmo_gateway_probe_responses_total").increment(1);
        } else {
            metrics::counter!("mmo_gateway_probe_dropped_unpadded_total").increment(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_probe_request_drops_short_packet() {
        let buf = [0u8; 63];
        assert_eq!(parse_probe_request(&buf), None);
    }

    #[test]
    fn test_parse_probe_request_drops_invalid_magic() {
        let mut buf = [0u8; 64];
        buf[0..4].copy_from_slice(&0x12345678u32.to_be_bytes());
        assert_eq!(parse_probe_request(&buf), None);
    }

    #[test]
    fn test_parse_probe_request_valid() {
        let mut buf = [0u8; 64];
        buf[0..4].copy_from_slice(&PROBE_MAGIC.to_be_bytes());
        buf[4..12].copy_from_slice(&0x1234567890abcdefu64.to_be_bytes());
        buf[12..16].copy_from_slice(&0xabcdef12u32.to_be_bytes());

        let req = parse_probe_request(&buf).unwrap();
        assert_eq!(req.client_timestamp_ms, 0x1234567890abcdef);
        assert_eq!(req.client_nonce, 0xabcdef12);
    }

    #[test]
    fn test_encode_probe_response_roundtrip() {
        let resp = ProbeResponse {
            client_timestamp_ms: 0x1234567890abcdef,
            client_nonce: 0xabcdef12,
            server_timestamp_ms: 0xabcdef1234567890,
            active_sessions: 100,
            capacity: 200,
            region: "eu-central".to_string(),
        };

        let encoded = encode_probe_response(&resp);
        assert_eq!(encoded.len(), 64);
        let decoded = parse_probe_response(&encoded).unwrap();
        assert_eq!(decoded, resp);
    }

    #[tokio::test]
    async fn test_udp_client_server_echo() {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let server_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let server_addr = server_socket.local_addr().unwrap();

        let region = "us-east".to_string();
        let capacity = 500;
        let active_sessions = Arc::new(AtomicU32::new(42));

        let s_active = Arc::clone(&active_sessions);
        tokio::spawn(async move {
            run_probe_server(server_socket, region, capacity, s_active).await;
        });

        let mut req_buf = [0u8; 64];
        req_buf[0..4].copy_from_slice(&PROBE_MAGIC.to_be_bytes());
        req_buf[4..12].copy_from_slice(&123456789u64.to_be_bytes());
        req_buf[12..16].copy_from_slice(&98765u32.to_be_bytes());

        socket.send_to(&req_buf, server_addr).await.unwrap();

        let mut resp_buf = [0u8; 64];
        let (len, _) = socket.recv_from(&mut resp_buf).await.unwrap();
        assert_eq!(len, 64);

        let decoded = parse_probe_response(&resp_buf[..len]).unwrap();
        assert_eq!(decoded.client_timestamp_ms, 123456789);
        assert_eq!(decoded.client_nonce, 98765);
        assert_eq!(decoded.active_sessions, 42);
        assert_eq!(decoded.capacity, 500);
        assert_eq!(decoded.region, "us-east");
    }
}
