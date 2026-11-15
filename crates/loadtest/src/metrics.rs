//! Telemetry aggregator and HDR histogram tracking for load test swarms.

use hdrhistogram::Histogram;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

/// Aggregates performance and reliability metrics across all swarm bots.
pub struct MetricsAggregator {
    packets_sent: AtomicU64,
    packets_received: AtomicU64,
    authenticated_sessions: AtomicU64,
    entity_deltas_received: AtomicU64,
    route_handovers_observed: AtomicU64,
    total_bytes_received: AtomicU64,
    snapbacks: AtomicU64,
    connection_errors: AtomicU64,
    auth_failures: AtomicU64,
    rtt_histogram_us: Mutex<Histogram<u64>>,
    start_time: Instant,
}

/// Point-in-time metrics report.
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub duration_secs: f64,
    pub total_packets_sent: u64,
    pub total_packets_received: u64,
    pub send_rate_pps: f64,
    pub recv_rate_pps: f64,
    pub authenticated_sessions: u64,
    /// Authenticated session admission throughput in sessions per second.
    pub auth_throughput_sps: f64,
    pub entity_deltas_received: u64,
    pub route_handovers_observed: u64,
    pub total_bytes_received: u64,
    /// Downstream wire receive bandwidth in kilobits per second (SI `1000` bits/kbit).
    pub recv_bandwidth_kbps: f64,
    pub snapbacks: u64,
    pub connection_errors: u64,
    pub auth_failures: u64,
    pub min_rtt_ms: f64,
    pub p50_rtt_ms: f64,
    pub p95_rtt_ms: f64,
    pub p99_rtt_ms: f64,
    pub max_rtt_ms: f64,
}

impl MetricsAggregator {
    /// Creates a new metrics aggregator.
    pub fn new() -> Self {
        // Range: 1 us to 10 seconds (10,000,000 us), 3 significant figures
        let hist = Histogram::<u64>::new_with_bounds(1, 10_000_000, 3)
            .expect("Valid HDR histogram parameters");

        Self {
            packets_sent: AtomicU64::new(0),
            packets_received: AtomicU64::new(0),
            authenticated_sessions: AtomicU64::new(0),
            entity_deltas_received: AtomicU64::new(0),
            route_handovers_observed: AtomicU64::new(0),
            total_bytes_received: AtomicU64::new(0),
            snapbacks: AtomicU64::new(0),
            connection_errors: AtomicU64::new(0),
            auth_failures: AtomicU64::new(0),
            rtt_histogram_us: Mutex::new(hist),
            start_time: Instant::now(),
        }
    }

    pub fn record_packet_sent(&self) {
        self.packets_sent.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_packet_received(&self) {
        self.packets_received.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_authenticated_session(&self) {
        self.authenticated_sessions.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_entity_deltas(&self, count: u64) {
        self.entity_deltas_received
            .fetch_add(count, Ordering::Relaxed);
    }

    pub fn record_route_handover(&self) {
        self.route_handovers_observed
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_bytes_received(&self, bytes: u64) {
        self.total_bytes_received
            .fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn record_snapback(&self) {
        self.snapbacks.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_connection_error(&self) {
        self.connection_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_auth_failure(&self) {
        self.auth_failures.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_rtt_ms(&self, rtt_ms: f64) {
        let us = (rtt_ms * 1000.0).max(1.0) as u64;
        let mut hist = self.rtt_histogram_us.lock().unwrap();
        let _ = hist.record(us);
    }

    /// Generates a summary snapshot.
    pub fn snapshot(&self) -> MetricsSnapshot {
        self.snapshot_for_duration(self.start_time.elapsed().as_secs_f64())
    }

    fn snapshot_for_duration(&self, duration: f64) -> MetricsSnapshot {
        let sent = self.packets_sent.load(Ordering::Relaxed);
        let recv = self.packets_received.load(Ordering::Relaxed);
        let authenticated_sessions = self.authenticated_sessions.load(Ordering::Relaxed);
        let entity_deltas_received = self.entity_deltas_received.load(Ordering::Relaxed);
        let route_handovers_observed = self.route_handovers_observed.load(Ordering::Relaxed);
        let total_bytes_received = self.total_bytes_received.load(Ordering::Relaxed);
        let snapbacks = self.snapbacks.load(Ordering::Relaxed);
        let errors = self.connection_errors.load(Ordering::Relaxed);
        let auth_failures = self.auth_failures.load(Ordering::Relaxed);

        let hist = self.rtt_histogram_us.lock().unwrap();

        let to_ms = |us: u64| us as f64 / 1000.0;
        let (min_rtt_ms, p50_rtt_ms, p95_rtt_ms, p99_rtt_ms, max_rtt_ms) = if hist.is_empty() {
            (0.0, 0.0, 0.0, 0.0, 0.0)
        } else {
            (
                to_ms(hist.min()),
                to_ms(hist.value_at_quantile(0.50)),
                to_ms(hist.value_at_quantile(0.95)),
                to_ms(hist.value_at_quantile(0.99)),
                to_ms(hist.max()),
            )
        };

        let per_sec = |count: f64| {
            if duration > 0.0 {
                count / duration
            } else {
                0.0
            }
        };

        MetricsSnapshot {
            duration_secs: duration,
            total_packets_sent: sent,
            total_packets_received: recv,
            send_rate_pps: per_sec(sent as f64),
            recv_rate_pps: per_sec(recv as f64),
            authenticated_sessions,
            auth_throughput_sps: per_sec(authenticated_sessions as f64),
            entity_deltas_received,
            route_handovers_observed,
            total_bytes_received,
            recv_bandwidth_kbps: per_sec(total_bytes_received as f64 * 8.0 / 1000.0),
            snapbacks,
            connection_errors: errors,
            auth_failures,
            min_rtt_ms,
            p50_rtt_ms,
            p95_rtt_ms,
            p99_rtt_ms,
            max_rtt_ms,
        }
    }
}

impl Default for MetricsAggregator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_aggregation_and_percentiles() {
        let metrics = MetricsAggregator::new();
        metrics.record_packet_sent();
        metrics.record_packet_sent();
        metrics.record_packet_received();
        metrics.record_snapback();

        metrics.record_rtt_ms(1.0);
        metrics.record_rtt_ms(5.0);
        metrics.record_rtt_ms(10.0);
        metrics.record_rtt_ms(100.0);

        let snap = metrics.snapshot();
        assert_eq!(snap.total_packets_sent, 2);
        assert_eq!(snap.total_packets_received, 1);
        assert_eq!(snap.snapbacks, 1);
        assert!(snap.p50_rtt_ms > 0.0);
        assert!(snap.p99_rtt_ms >= snap.p50_rtt_ms);
    }

    #[test]
    fn test_empty_metrics_snapshot_returns_zero_rtt() {
        let metrics = MetricsAggregator::new();
        metrics.record_auth_failure();
        let snap = metrics.snapshot();
        assert_eq!(snap.auth_failures, 1);
        assert_eq!(snap.authenticated_sessions, 0);
        assert_eq!(snap.auth_throughput_sps, 0.0);
        assert_eq!(snap.entity_deltas_received, 0);
        assert_eq!(snap.route_handovers_observed, 0);
        assert_eq!(snap.total_bytes_received, 0);
        assert_eq!(snap.recv_bandwidth_kbps, 0.0);
        assert_eq!(snap.min_rtt_ms, 0.0);
        assert_eq!(snap.p50_rtt_ms, 0.0);
        assert_eq!(snap.p95_rtt_ms, 0.0);
        assert_eq!(snap.p99_rtt_ms, 0.0);
        assert_eq!(snap.max_rtt_ms, 0.0);
    }

    #[test]
    fn test_extended_soak_telemetry_counters_and_rates() {
        let metrics = MetricsAggregator::new();
        for _ in 0..25 {
            metrics.record_authenticated_session();
        }
        metrics.record_entity_deltas(120);
        metrics.record_entity_deltas(30);
        metrics.record_route_handover();
        metrics.record_route_handover();
        metrics.record_bytes_received(50_000);
        metrics.record_bytes_received(25_000);

        let snap = metrics.snapshot_for_duration(2.5);
        assert_eq!(snap.authenticated_sessions, 25);
        assert!((snap.auth_throughput_sps - 10.0).abs() < 1e-6);
        assert_eq!(snap.entity_deltas_received, 150);
        assert_eq!(snap.route_handovers_observed, 2);
        assert_eq!(snap.total_bytes_received, 75_000);
        // 75,000 bytes * 8 bits / 1000 / 2.5s = 240.0 kbps
        assert!((snap.recv_bandwidth_kbps - 240.0).abs() < 1e-6);

        let zero_dur = metrics.snapshot_for_duration(0.0);
        assert_eq!(zero_dur.auth_throughput_sps, 0.0);
        assert_eq!(zero_dur.recv_bandwidth_kbps, 0.0);
    }
}
