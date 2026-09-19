//! Telemetry aggregator and HDR histogram tracking for load test swarms.

use hdrhistogram::Histogram;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

/// Aggregates performance and reliability metrics across all swarm bots.
pub struct MetricsAggregator {
    packets_sent: AtomicU64,
    packets_received: AtomicU64,
    snapbacks: AtomicU64,
    connection_errors: AtomicU64,
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
    pub snapbacks: u64,
    pub connection_errors: u64,
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
            snapbacks: AtomicU64::new(0),
            connection_errors: AtomicU64::new(0),
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

    pub fn record_snapback(&self) {
        self.snapbacks.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_connection_error(&self) {
        self.connection_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_rtt_ms(&self, rtt_ms: f64) {
        let us = (rtt_ms * 1000.0).max(1.0) as u64;
        let mut hist = self.rtt_histogram_us.lock().unwrap();
        let _ = hist.record(us);
    }

    /// Generates a summary snapshot.
    pub fn snapshot(&self) -> MetricsSnapshot {
        let duration = self.start_time.elapsed().as_secs_f64();
        let sent = self.packets_sent.load(Ordering::Relaxed);
        let recv = self.packets_received.load(Ordering::Relaxed);
        let snapbacks = self.snapbacks.load(Ordering::Relaxed);
        let errors = self.connection_errors.load(Ordering::Relaxed);

        let hist = self.rtt_histogram_us.lock().unwrap();

        let to_ms = |us: u64| us as f64 / 1000.0;

        MetricsSnapshot {
            duration_secs: duration,
            total_packets_sent: sent,
            total_packets_received: recv,
            send_rate_pps: if duration > 0.0 { sent as f64 / duration } else { 0.0 },
            recv_rate_pps: if duration > 0.0 { recv as f64 / duration } else { 0.0 },
            snapbacks,
            connection_errors: errors,
            min_rtt_ms: to_ms(hist.min()),
            p50_rtt_ms: to_ms(hist.value_at_quantile(0.50)),
            p95_rtt_ms: to_ms(hist.value_at_quantile(0.95)),
            p99_rtt_ms: to_ms(hist.value_at_quantile(0.99)),
            max_rtt_ms: to_ms(hist.max()),
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
}
