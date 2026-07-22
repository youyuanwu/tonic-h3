//! Latency/throughput metrics collection for the benchmark client.
//!
//! Latencies are recorded into an [`hdrhistogram::Histogram`] in microseconds,
//! which is O(1) per sample with bounded memory (no per-request heap growth
//! proportional to the request count).

use std::time::Duration;

use hdrhistogram::Histogram;

/// Records latency samples and success/failure counts for one benchmark run.
pub struct Recorder {
    hist: Histogram<u64>,
    success: u64,
    failure: u64,
}

impl Recorder {
    /// Create a recorder able to record latencies from 1us up to ~120s.
    pub fn new() -> Self {
        // 1 .. 120_000_000 us (120 s), 3 significant figures.
        let hist =
            Histogram::<u64>::new_with_bounds(1, 120_000_000, 3).expect("valid histogram bounds");
        Self {
            hist,
            success: 0,
            failure: 0,
        }
    }

    /// Record a successful request with its measured latency.
    pub fn record_success(&mut self, latency: Duration) {
        self.success += 1;
        // Saturate into the recordable range instead of erroring on outliers.
        let us = latency.as_micros().min(u64::MAX as u128) as u64;
        self.hist.saturating_record(us.max(1));
    }

    /// Record a failed request (not added to the latency histogram).
    pub fn record_failure(&mut self) {
        self.failure += 1;
    }

    /// Merge another recorder's samples into this one (worker aggregation).
    pub fn merge(&mut self, other: &Recorder) {
        self.hist.add(&other.hist).expect("compatible histograms");
        self.success += other.success;
        self.failure += other.failure;
    }
}

impl Default for Recorder {
    fn default() -> Self {
        Self::new()
    }
}

/// Aggregated, human-readable result of a benchmark run.
#[derive(Debug, Clone)]
pub struct BenchSummary {
    pub total: u64,
    pub success: u64,
    pub failure: u64,
    pub elapsed: Duration,
    pub throughput_rps: f64,
    /// Latency percentiles in microseconds (None when there were no successes).
    pub p50_us: Option<u64>,
    pub p90_us: Option<u64>,
    pub p99_us: Option<u64>,
}

impl BenchSummary {
    /// Compute a summary from a recorder and the wall-clock elapsed time.
    pub fn from_recorder(rec: &Recorder, elapsed: Duration) -> Self {
        let total = rec.success + rec.failure;
        let secs = elapsed.as_secs_f64();
        // Throughput counts only successful requests over wall-clock time.
        let throughput_rps = if secs > 0.0 {
            rec.success as f64 / secs
        } else {
            0.0
        };
        // Empty-sample safety: only query percentiles when we have successes.
        let (p50, p90, p99) = if rec.success > 0 {
            (
                Some(rec.hist.value_at_quantile(0.50)),
                Some(rec.hist.value_at_quantile(0.90)),
                Some(rec.hist.value_at_quantile(0.99)),
            )
        } else {
            (None, None, None)
        };
        Self {
            total,
            success: rec.success,
            failure: rec.failure,
            elapsed,
            throughput_rps,
            p50_us: p50,
            p90_us: p90,
            p99_us: p99,
        }
    }

    /// Render this summary as a single machine-readable JSON object, combining
    /// the run's metrics with the client configuration that produced them.
    ///
    /// Latency percentiles are emitted in **milliseconds** (matching the text
    /// output) and become JSON `null` when there were no successful requests.
    /// `elapsed_s` is seconds. This shape is stable for cross-run/cross-SKU
    /// aggregation (see `docs/bench/`).
    pub fn to_json(
        &self,
        transport: &str,
        payload_size: usize,
        concurrency: u32,
    ) -> serde_json::Value {
        let ms = |v: Option<u64>| v.map(|us| us as f64 / 1000.0);
        serde_json::json!({
            "transport": transport,
            "payload_size": payload_size,
            "concurrency": concurrency,
            "count": self.total,
            "ok": self.success,
            "failed": self.failure,
            "elapsed_s": self.elapsed.as_secs_f64(),
            "throughput_rps": self.throughput_rps,
            "p50_ms": ms(self.p50_us),
            "p90_ms": ms(self.p90_us),
            "p99_ms": ms(self.p99_us),
        })
    }
}

fn fmt_us(v: Option<u64>) -> String {
    match v {
        Some(us) => format!("{:.3} ms", us as f64 / 1000.0),
        None => "n/a".to_string(),
    }
}

impl std::fmt::Display for BenchSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "=== Benchmark result ===")?;
        writeln!(
            f,
            "requests:    {} total ({} ok, {} failed)",
            self.total, self.success, self.failure
        )?;
        writeln!(f, "elapsed:     {:.3} s", self.elapsed.as_secs_f64())?;
        writeln!(f, "throughput:  {:.1} req/s", self.throughput_rps)?;
        writeln!(f, "latency p50: {}", fmt_us(self.p50_us))?;
        writeln!(f, "latency p90: {}", fmt_us(self.p90_us))?;
        write!(f, "latency p99: {}", fmt_us(self.p99_us))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_computes_percentiles_and_throughput() {
        let mut rec = Recorder::new();
        // 1000 samples of 1ms each => p50/p90/p99 all ~1ms.
        for _ in 0..1000 {
            rec.record_success(Duration::from_millis(1));
        }
        let s = BenchSummary::from_recorder(&rec, Duration::from_secs(2));
        assert_eq!(s.total, 1000);
        assert_eq!(s.success, 1000);
        assert_eq!(s.failure, 0);
        // 1000 successes over 2s => 500 req/s.
        assert!((s.throughput_rps - 500.0).abs() < 1.0);
        // ~1ms == 1000us within histogram precision.
        let p50 = s.p50_us.unwrap();
        assert!((900..=1100).contains(&p50), "p50 was {p50}");
    }

    #[test]
    fn zero_success_is_percentile_safe() {
        let mut rec = Recorder::new();
        rec.record_failure();
        rec.record_failure();
        let s = BenchSummary::from_recorder(&rec, Duration::from_secs(1));
        assert_eq!(s.total, 2);
        assert_eq!(s.success, 0);
        assert_eq!(s.failure, 2);
        assert_eq!(s.throughput_rps, 0.0);
        assert!(s.p50_us.is_none());
        assert!(s.p90_us.is_none());
        assert!(s.p99_us.is_none());
        // Display must not panic on the empty-sample case.
        let _ = format!("{s}");
    }

    #[test]
    fn merge_combines_counts() {
        let mut a = Recorder::new();
        let mut b = Recorder::new();
        a.record_success(Duration::from_millis(1));
        b.record_success(Duration::from_millis(2));
        b.record_failure();
        a.merge(&b);
        let s = BenchSummary::from_recorder(&a, Duration::from_secs(1));
        assert_eq!(s.success, 2);
        assert_eq!(s.failure, 1);
        assert_eq!(s.total, 3);
    }

    #[test]
    fn json_has_required_schema_and_units() {
        let mut rec = Recorder::new();
        for _ in 0..100 {
            rec.record_success(Duration::from_millis(2));
        }
        let s = BenchSummary::from_recorder(&rec, Duration::from_secs(2));
        let j = s.to_json("quinn", 4096, 32);

        // All required keys are present.
        for key in [
            "transport",
            "payload_size",
            "concurrency",
            "count",
            "ok",
            "failed",
            "elapsed_s",
            "throughput_rps",
            "p50_ms",
            "p90_ms",
            "p99_ms",
        ] {
            assert!(j.get(key).is_some(), "missing key {key}");
        }

        assert_eq!(j["transport"], "quinn");
        assert_eq!(j["payload_size"], 4096);
        assert_eq!(j["concurrency"], 32);
        assert_eq!(j["count"], 100);
        assert_eq!(j["ok"], 100);
        assert_eq!(j["failed"], 0);
        assert_eq!(j["elapsed_s"], 2.0);
        // 100 successes / 2s = 50 req/s.
        assert!((j["throughput_rps"].as_f64().unwrap() - 50.0).abs() < 1.0);
        // ~2ms latency, emitted in milliseconds.
        let p50 = j["p50_ms"].as_f64().unwrap();
        assert!((1.9..=2.1).contains(&p50), "p50_ms was {p50}");
        // Emits one line and round-trips as valid JSON.
        let line = j.to_string();
        assert!(!line.contains('\n'));
        let _: serde_json::Value = serde_json::from_str(&line).unwrap();
    }

    #[test]
    fn json_percentiles_null_on_zero_success() {
        let mut rec = Recorder::new();
        rec.record_failure();
        let s = BenchSummary::from_recorder(&rec, Duration::from_secs(1));
        let j = s.to_json("tcp-tls", 64, 1);
        assert_eq!(j["ok"], 0);
        assert_eq!(j["failed"], 1);
        assert_eq!(j["count"], 1);
        assert!(j["p50_ms"].is_null());
        assert!(j["p90_ms"].is_null());
        assert!(j["p99_ms"].is_null());
    }
}
