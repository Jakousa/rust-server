//! Persistent Backhaul Connection Pooling and Multiplexing for Edge Gateways.
//!
//! Maintains pooled, health-checked multiplexing channels between Edge Gateways
//! and simulation cluster nodes with circuit breaker protection and backpressure isolation.

use dashmap::DashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::server::RoutedPacket;

/// Operational status of a simulation node backhaul link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BackhaulState {
    Connected = 0,
    Degraded = 1,
    Broken = 2,
}

impl From<u8> for BackhaulState {
    fn from(val: u8) -> Self {
        match val {
            0 => BackhaulState::Connected,
            1 => BackhaulState::Degraded,
            _ => BackhaulState::Broken,
        }
    }
}

/// Errors returned by backhaul operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackhaulError {
    ChannelFull,
    CircuitBreakerOpen(String),
    NodeUnreachable(String),
}

/// Managed backhaul multiplex channel to a single simulation node.
#[derive(Debug)]
pub struct BackhaulChannel {
    pub node_address: String,
    pub tx: mpsc::Sender<RoutedPacket>,
    consecutive_failures: AtomicU32,
    rtt_ms: AtomicU64,
    state: AtomicU8,
}

impl BackhaulChannel {
    pub fn new(node_address: String, tx: mpsc::Sender<RoutedPacket>) -> Self {
        Self {
            node_address,
            tx,
            consecutive_failures: AtomicU32::new(0),
            rtt_ms: AtomicU64::new(0),
            state: AtomicU8::new(BackhaulState::Connected as u8),
        }
    }

    pub fn state(&self) -> BackhaulState {
        BackhaulState::from(self.state.load(Ordering::Relaxed))
    }

    pub fn is_healthy(&self) -> bool {
        self.state() != BackhaulState::Broken
    }

    pub fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::Relaxed);
        self.state
            .store(BackhaulState::Connected as u8, Ordering::Relaxed);
    }

    pub fn record_failure(&self) {
        let failures = self.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
        if failures >= 5 {
            self.state
                .store(BackhaulState::Broken as u8, Ordering::Relaxed);
            metrics::counter!("mmo_gateway_backhaul_circuit_breaker_tripped_total").increment(1);
        } else {
            self.state
                .store(BackhaulState::Degraded as u8, Ordering::Relaxed);
        }
    }

    pub fn record_rtt(&self, rtt_ms: u64) {
        self.rtt_ms.store(rtt_ms, Ordering::Relaxed);
        metrics::gauge!("mmo_gateway_backhaul_rtt_ms").set(rtt_ms as f64);
    }

    pub fn rtt_ms(&self) -> u64 {
        self.rtt_ms.load(Ordering::Relaxed)
    }
}

/// Concurrent connection pool managing backhaul channels across simulation nodes.
#[derive(Debug)]
pub struct BackhaulPool {
    channels: DashMap<String, Arc<BackhaulChannel>>,
    channel_capacity: usize,
}

impl BackhaulPool {
    pub fn new(channel_capacity: usize) -> Self {
        Self {
            channels: DashMap::new(),
            channel_capacity,
        }
    }

    /// Gets existing channel or initializes a new bounded channel and returns receiver.
    pub fn get_or_create(
        &self,
        node_address: &str,
    ) -> (Arc<BackhaulChannel>, Option<mpsc::Receiver<RoutedPacket>>) {
        if let Some(ch) = self.channels.get(node_address) {
            return (Arc::clone(&ch), None);
        }

        let (tx, rx) = mpsc::channel(self.channel_capacity);
        let channel = Arc::new(BackhaulChannel::new(node_address.to_string(), tx));
        self.channels
            .insert(node_address.to_string(), Arc::clone(&channel));
        (channel, Some(rx))
    }

    /// Dispatches a routed packet across the node's persistent backhaul channel.
    pub fn send_packet(&self, packet: RoutedPacket) -> Result<(), BackhaulError> {
        let channel = self
            .channels
            .get(&packet.node_address)
            .ok_or_else(|| BackhaulError::NodeUnreachable(packet.node_address.clone()))?;

        if !channel.is_healthy() {
            return Err(BackhaulError::CircuitBreakerOpen(
                packet.node_address.clone(),
            ));
        }

        match channel.tx.try_send(packet) {
            Ok(_) => {
                metrics::counter!("mmo_gateway_backhaul_packets_sent_total").increment(1);
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                metrics::counter!("mmo_gateway_backhaul_dropped_packets_total").increment(1);
                Err(BackhaulError::ChannelFull)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                channel.record_failure();
                Err(BackhaulError::NodeUnreachable(channel.node_address.clone()))
            }
        }
    }

    /// Returns list of all addresses with healthy (non-broken) backhaul channels.
    pub fn healthy_channels(&self) -> Vec<String> {
        self.channels
            .iter()
            .filter(|entry| entry.value().is_healthy())
            .map(|entry| entry.key().clone())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.channels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mmo_core::guid::Guid;
    use mmo_schema::client::{ClientPacket, MovementPacket, Vec3};

    fn make_test_packet(node_address: &str) -> RoutedPacket {
        RoutedPacket {
            session_id: 1,
            guid: Some(Guid::new(mmo_core::guid::EntityType::Player, 1, 1)),
            node_address: node_address.to_string(),
            session_epoch: 0,
            packet: ClientPacket::Movement(MovementPacket {
                entity_id: 1,
                sequence: 1,
                position: Vec3::new(0.0, 0.0, 0.0),
                velocity: Vec3::new(0.0, 0.0, 0.0),
                yaw: 0.0,
                pitch: 0.0,
                timestamp: 0,
            }),
        }
    }

    #[test]
    fn test_channel_creation_and_rtt() {
        let (tx, _) = mpsc::channel(10);
        let ch = BackhaulChannel::new("127.0.0.1:50052".to_string(), tx);
        assert!(ch.is_healthy());
        assert_eq!(ch.state(), BackhaulState::Connected);

        ch.record_rtt(42);
        assert_eq!(ch.rtt_ms(), 42);
    }

    #[test]
    fn test_circuit_breaker_tripping_and_recovery() {
        let (tx, _) = mpsc::channel(10);
        let ch = BackhaulChannel::new("127.0.0.1:50052".to_string(), tx);

        for _ in 0..4 {
            ch.record_failure();
            assert_eq!(ch.state(), BackhaulState::Degraded);
            assert!(ch.is_healthy());
        }

        ch.record_failure(); // 5th failure
        assert_eq!(ch.state(), BackhaulState::Broken);
        assert!(!ch.is_healthy());

        ch.record_success();
        assert_eq!(ch.state(), BackhaulState::Connected);
        assert!(ch.is_healthy());
    }

    #[tokio::test]
    async fn test_pool_get_or_create_and_send() {
        let pool = BackhaulPool::new(10);
        let (ch, rx_opt) = pool.get_or_create("127.0.0.1:50052");
        assert_eq!(pool.len(), 1);
        let mut rx = rx_opt.expect("first get_or_create should return Receiver");

        let pkt = make_test_packet("127.0.0.1:50052");
        assert!(pool.send_packet(pkt.clone()).is_ok());

        let received = rx.recv().await.expect("should receive routed packet");
        assert_eq!(received, pkt);

        // Subsequent get_or_create returns existing channel with no Receiver
        let (ch2, rx_opt2) = pool.get_or_create("127.0.0.1:50052");
        assert!(rx_opt2.is_none());
        assert_eq!(ch2.node_address, ch.node_address);
    }

    #[test]
    fn test_pool_circuit_breaker_rejection() {
        let pool = BackhaulPool::new(10);
        let (ch, _rx) = pool.get_or_create("127.0.0.1:50052");

        for _ in 0..5 {
            ch.record_failure();
        }

        let pkt = make_test_packet("127.0.0.1:50052");
        let err = pool.send_packet(pkt).unwrap_err();
        assert_eq!(
            err,
            BackhaulError::CircuitBreakerOpen("127.0.0.1:50052".to_string())
        );
    }
}
