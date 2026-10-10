// SPDX-FileCopyrightText: 2026 Free Mobile
// SPDX-License-Identifier: AGPL-3.0-only

//! Per-exporter rate limiter. Port of `outlet/core/rate.go`.

use std::collections::HashMap;
use std::net::Ipv6Addr;
use std::sync::Mutex;

#[derive(Debug)]
struct PerExporter {
    /// Dropped during the current second.
    dropped: u64,
    /// Received during the current second.
    total: u64,
    /// Sampling rate correction computed from the last second.
    factor: f64,
    /// Second the counters apply to.
    current_second: u64,
}

/// Rate limiter state for all exporters.
#[derive(Debug, Default)]
pub struct RateLimiter {
    exporters: Mutex<HashMap<Ipv6Addr, PerExporter>>,
}

impl RateLimiter {
    pub fn new() -> RateLimiter {
        RateLimiter::default()
    }

    /// Check if a flow from `exporter` is allowed with `rate_limit` flows/s.
    /// `time_received` is the second the inlet got the flow. Also returns the
    /// factor to apply to the sampling rate to account for dropped flows.
    /// `rate_limit` should be positive.
    pub fn allow_one_message(&self, exporter: Ipv6Addr, rate_limit: u64, time_received: u64) -> (bool, f64) {
        let mut exporters = self.exporters.lock().unwrap();
        let value = exporters.entry(exporter).or_insert(PerExporter {
            dropped: 0,
            total: 0,
            factor: 1.0,
            current_second: time_received,
        });
        if time_received > value.current_second {
            // The accepted flows stand for the dropped ones.
            let accepted = value.total - value.dropped;
            if accepted > 0 {
                value.factor = value.total as f64 / accepted as f64;
            }
            value.dropped = 0;
            value.total = 0;
            value.current_second = time_received;
        }
        // A flow older than the current second counts with it: Kafka
        // partitions are read in parallel, so flows come out of order.
        value.total += 1;
        let mut verdict = true;
        if value.total > rate_limit {
            value.dropped += 1;
            verdict = false;
        }
        (verdict, value.factor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exporter(last: u8) -> Ipv6Addr {
        std::net::Ipv4Addr::new(192, 0, 2, last).to_ipv6_mapped()
    }

    fn count_allowed(rl: &RateLimiter, e: Ipv6Addr, n: usize, second: u64) -> usize {
        (0..n).filter(|_| rl.allow_one_message(e, 100, second).0).count()
    }

    #[test]
    fn enforced() {
        let rl = RateLimiter::new();
        assert_eq!(count_allowed(&rl, exporter(1), 150, 1000), 100);
        assert_eq!(count_allowed(&rl, exporter(1), 150, 1001), 100);
    }

    #[test]
    fn sampling_rate_factor() {
        let rl = RateLimiter::new();
        for second in 0..5u64 {
            let expected = if second == 0 { 1.0 } else { 1.5 };
            for _ in 0..150 {
                assert_eq!(rl.allow_one_message(exporter(1), 100, 1000 + second).1, expected, "second {second}");
            }
        }
    }

    #[test]
    fn late_flow() {
        let rl = RateLimiter::new();
        count_allowed(&rl, exporter(1), 60, 1001);
        assert_eq!(count_allowed(&rl, exporter(1), 50, 1000), 40);
    }

    #[test]
    fn per_exporter() {
        let rl = RateLimiter::new();
        count_allowed(&rl, exporter(1), 150, 1000);
        assert_eq!(count_allowed(&rl, exporter(2), 100, 1000), 100);
    }

    #[test]
    fn below_limit() {
        let rl = RateLimiter::new();
        for second in 0..5u64 {
            for _ in 0..10 {
                assert_eq!(rl.allow_one_message(exporter(1), 100, 1000 + second), (true, 1.0));
            }
        }
    }
}
