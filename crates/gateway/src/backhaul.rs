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
    HalfOpen = 3,
}

impl From<u8> for BackhaulState {
    fn from(val: u8) -> Self {
        match val {
            0 => BackhaulState::Connected,
            1 => BackhaulState::Degraded,
            2 => BackhaulState::Broken,
            3 => BackhaulState::HalfOpen,
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

/// Default probe interval in milliseconds before probing a broken channel in half-open state.
pub const DEFAULT_PROBE_INTERVAL_MS: u64 = 5_000;

#[inline]
fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Managed backhaul multiplex channel to a single simulation node.
#[derive(Debug)]
pub struct BackhaulChannel {
    pub node_address: String,
    pub tx: mpsc::Sender<RoutedPacket>,
    consecutive_failures: AtomicU32,
    rtt_ms: AtomicU64,
    state: AtomicU8,
    last_state_change_ms: AtomicU64,
    probe_interval_ms: AtomicU64,
}

impl BackhaulChannel {
    pub fn new(node_address: String, tx: mpsc::Sender<RoutedPacket>) -> Self {
        Self::with_probe_interval(node_address, tx, DEFAULT_PROBE_INTERVAL_MS)
    }

    pub fn with_probe_interval(
        node_address: String,
        tx: mpsc::Sender<RoutedPacket>,
        probe_interval_ms: u64,
    ) -> Self {
        let now = current_time_ms();
        Self {
            node_address,
            tx,
            consecutive_failures: AtomicU32::new(0),
            rtt_ms: AtomicU64::new(0),
            state: AtomicU8::new(BackhaulState::Connected as u8),
            last_state_change_ms: AtomicU64::new(now),
            probe_interval_ms: AtomicU64::new(probe_interval_ms),
        }
    }

    pub fn probe_interval_ms(&self) -> u64 {
        self.probe_interval_ms.load(Ordering::Relaxed)
    }

    pub fn set_probe_interval_ms(&self, interval_ms: u64) {
        self.probe_interval_ms.store(interval_ms, Ordering::Relaxed);
    }

    pub fn state(&self) -> BackhaulState {
        let raw = self.state.load(Ordering::Acquire);
        let s = BackhaulState::from(raw);
        if s == BackhaulState::Broken {
            let last = self.last_state_change_ms.load(Ordering::Relaxed);
            let now = current_time_ms();
            let interval = self.probe_interval_ms.load(Ordering::Relaxed);
            if now.saturating_sub(last) >= interval
                && self
                    .state
                    .compare_exchange(
                        BackhaulState::Broken as u8,
                        BackhaulState::HalfOpen as u8,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_ok()
            {
                self.last_state_change_ms.store(now, Ordering::Release);
                metrics::counter!("mmo_gateway_backhaul_circuit_breaker_half_open_total")
                    .increment(1);
                return BackhaulState::HalfOpen;
            }
        }
        BackhaulState::from(self.state.load(Ordering::Acquire))
    }

    /// Explicitly transitions the circuit breaker to HalfOpen state if it is currently Broken.
    pub fn transition_to_half_open(&self) -> bool {
        let now = current_time_ms();
        if self
            .state
            .compare_exchange(
                BackhaulState::Broken as u8,
                BackhaulState::HalfOpen as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            self.last_state_change_ms.store(now, Ordering::Release);
            metrics::counter!("mmo_gateway_backhaul_circuit_breaker_half_open_total").increment(1);
            true
        } else {
            self.state.load(Ordering::Acquire) == BackhaulState::HalfOpen as u8
        }
    }

    /// Whether this channel is considered healthy for routing (Connected or Degraded).
    pub fn is_healthy(&self) -> bool {
        let s = self.state();
        s == BackhaulState::Connected || s == BackhaulState::Degraded
    }

    /// Whether a packet or probe transmission is permitted through this channel.
    pub fn can_send(&self) -> bool {
        let s = self.state();
        s != BackhaulState::Broken
    }

    pub fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::Relaxed);
        let prev = self
            .state
            .swap(BackhaulState::Connected as u8, Ordering::Release);
        if prev != BackhaulState::Connected as u8 {
            self.last_state_change_ms
                .store(current_time_ms(), Ordering::Release);
            metrics::counter!("mmo_gateway_backhaul_circuit_breaker_recovered_total").increment(1);
        }
    }

    pub fn record_failure(&self) {
        let failures = self.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
        let now = current_time_ms();
        if failures >= 5 || self.state.load(Ordering::Acquire) == BackhaulState::HalfOpen as u8 {
            self.state
                .store(BackhaulState::Broken as u8, Ordering::Release);
            self.last_state_change_ms.store(now, Ordering::Release);
            metrics::counter!("mmo_gateway_backhaul_circuit_breaker_tripped_total").increment(1);
        } else {
            self.state
                .store(BackhaulState::Degraded as u8, Ordering::Release);
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
    probe_interval_ms: u64,
}

impl BackhaulPool {
    pub fn new(channel_capacity: usize) -> Self {
        Self::with_probe_interval(channel_capacity, DEFAULT_PROBE_INTERVAL_MS)
    }

    pub fn with_probe_interval(channel_capacity: usize, probe_interval_ms: u64) -> Self {
        Self {
            channels: DashMap::new(),
            channel_capacity,
            probe_interval_ms,
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
        let channel = Arc::new(BackhaulChannel::with_probe_interval(
            node_address.to_string(),
            tx,
            self.probe_interval_ms,
        ));
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

        if !channel.can_send() {
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
            session_epoch: 1,
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

    #[tokio::test]
    async fn test_circuit_breaker_half_open_probe_recovery() {
        let pool = BackhaulPool::with_probe_interval(10, 20); // 20ms probe interval
        let (ch, rx_opt) = pool.get_or_create("127.0.0.1:50052");
        let mut rx = rx_opt.expect("receiver");

        for _ in 0..5 {
            ch.record_failure();
        }
        assert_eq!(ch.state(), BackhaulState::Broken);
        assert!(!ch.can_send());

        // Wait for probe interval to elapse
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;

        // Channel should transition to HalfOpen
        assert_eq!(ch.state(), BackhaulState::HalfOpen);
        assert!(ch.can_send());

        // Probe packet send should succeed
        let pkt = make_test_packet("127.0.0.1:50052");
        assert!(pool.send_packet(pkt.clone()).is_ok());
        let received = rx.recv().await.expect("received probe packet");
        assert_eq!(received, pkt);

        // Success records recovery back to Connected
        ch.record_success();
        assert_eq!(ch.state(), BackhaulState::Connected);
        assert!(ch.is_healthy());
    }

    #[tokio::test]
    async fn test_circuit_breaker_half_open_probe_failure_trips_immediately() {
        let pool = BackhaulPool::with_probe_interval(10, 20);
        let (ch, _rx) = pool.get_or_create("127.0.0.1:50052");

        for _ in 0..5 {
            ch.record_failure();
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert_eq!(ch.state(), BackhaulState::HalfOpen);

        // A failure during HalfOpen immediately trips back to Broken without requiring 5 failures
        ch.record_failure();
        assert_eq!(ch.state(), BackhaulState::Broken);
        assert!(!ch.can_send());
    }

    #[tokio::test]
    async fn test_backhaul_pool_healthy_channels_and_automatic_reassignment() {
        let pool = BackhaulPool::with_probe_interval(10, 30);
        let (ch1, _rx1) = pool.get_or_create("node1:50052");
        let (_ch2, rx2_opt) = pool.get_or_create("node2:50052");
        let mut rx2 = rx2_opt.expect("receiver");

        let initial_healthy = pool.healthy_channels();
        assert_eq!(initial_healthy.len(), 2);

        // Trip node1 circuit breaker
        for _ in 0..5 {
            ch1.record_failure();
        }

        let healthy_after_trip = pool.healthy_channels();
        assert_eq!(healthy_after_trip, vec!["node2:50052".to_string()]);

        // Sending to node1 fails with CircuitBreakerOpen
        let mut pkt = make_test_packet("node1:50052");
        let err = pool.send_packet(pkt.clone()).unwrap_err();
        assert_eq!(
            err,
            BackhaulError::CircuitBreakerOpen("node1:50052".to_string())
        );

        // Automatic route reassignment fallback: pick healthy node
        let fallback = pool
            .healthy_channels()
            .into_iter()
            .find(|a| a != "node1:50052")
            .unwrap();
        pkt.node_address = fallback;
        assert!(pool.send_packet(pkt.clone()).is_ok());

        let received = rx2.recv().await.expect("received reassigned packet");
        assert_eq!(received.node_address, "node2:50052");

        // Wait for probe interval to recover node1
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        assert_eq!(ch1.state(), BackhaulState::HalfOpen);
        ch1.record_success();
        assert_eq!(ch1.state(), BackhaulState::Connected);
        assert_eq!(pool.healthy_channels().len(), 2);
    }
}
