//! Generic load generator for the benchmark client.
//!
//! [`drive_load`] is generic over the tonic channel type, so the same worker
//! loop and metrics collection run for both the HTTP/3 `H3Channel` and the
//! HTTP/2 `tonic::transport::Channel`. The generic `where` clause mirrors the
//! bound set that generated tonic clients declare.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

// Brings `Body`, `Bytes`, and `StdError` into scope (same imports the generated
// client `impl` block uses).
use tonic::codegen::*;

use crate::echo_client::EchoClient;
use crate::metrics::{BenchSummary, Recorder};
use crate::{BenchError, EchoRequest};

/// How much load to apply: a fixed request count or a wall-clock duration.
#[derive(Debug, Clone, Copy)]
pub enum LoadLimit {
    Count(u64),
    Duration(Duration),
}

/// Configuration for one benchmark run.
#[derive(Debug, Clone)]
pub struct LoadConfig {
    pub payload_size: usize,
    pub concurrency: usize,
    pub limit: LoadLimit,
    pub warmup: Option<Duration>,
    pub connect_timeout: Duration,
    /// Per-request timeout. A request that exceeds this is counted as a failure
    /// so count- and duration-based runs always terminate (guards against a
    /// stalled backend, e.g. the experimental quiche path under concurrency).
    pub request_timeout: Duration,
}

/// Build a deterministic, non-uniform payload of `size` bytes.
///
/// A non-constant pattern lets the preflight self-check validate payload
/// *content* (byte-for-byte), not merely length (FR-009).
pub(crate) fn make_payload(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 251) as u8).collect()
}

/// Drive load against `client` and return the aggregated summary.
///
/// Performs a bounded preflight probe first so a missing/unreachable server
/// fails fast (returning `Err`) instead of hanging.
pub async fn drive_load<T>(
    client: EchoClient<T>,
    cfg: LoadConfig,
) -> Result<BenchSummary, BenchError>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Clone + Send + 'static,
    T::Error: Into<StdError>,
    T::ResponseBody: Body<Data = Bytes> + Send + 'static,
    <T::ResponseBody as Body>::Error: Into<StdError> + Send,
    T::Future: Send,
{
    // --- Preflight probe (fail fast) ---
    // Full byte-for-byte self-check (FR-009): a non-uniform payload is sent and
    // the reply must match exactly. The measured hot path below only checks
    // length, to keep per-request overhead O(1) (metrics NFR).
    let payload = Arc::new(make_payload(cfg.payload_size));
    {
        let mut c = client.clone();
        let fut = c.echo(tonic::Request::new(EchoRequest {
            payload: payload.as_ref().clone(),
        }));
        match tokio::time::timeout(cfg.connect_timeout, fut).await {
            Err(_) => {
                return Err(format!(
                    "connect probe timed out after {:?}; is the server running?",
                    cfg.connect_timeout
                )
                .into());
            }
            Ok(Err(e)) => {
                return Err(format!("connect probe failed: {e}").into());
            }
            Ok(Ok(resp)) => {
                if resp.into_inner().payload != *payload {
                    return Err(
                        "echo self-check failed: reply payload does not match request \
                                (byte-for-byte comparison)"
                            .into(),
                    );
                }
            }
        }
    }

    // --- Optional warmup (results discarded) ---
    if let Some(w) = cfg.warmup {
        let _ = run_duration(&client, &payload, cfg.concurrency, w, cfg.request_timeout).await;
    }

    // --- Measured run ---
    let start = Instant::now();
    let rec = match cfg.limit {
        LoadLimit::Count(n) => {
            run_count(&client, &payload, cfg.concurrency, n, cfg.request_timeout).await
        }
        LoadLimit::Duration(d) => {
            run_duration(&client, &payload, cfg.concurrency, d, cfg.request_timeout).await
        }
    };
    let elapsed = start.elapsed();
    Ok(BenchSummary::from_recorder(&rec, elapsed))
}

/// Issue a single echo RPC, recording latency (success) or a failure.
///
/// The RPC is bounded by `request_timeout`; a timed-out request is a failure so
/// the run cannot hang on a stalled backend.
async fn one_request<T>(
    client: &mut EchoClient<T>,
    payload: &[u8],
    request_timeout: Duration,
    rec: &mut Recorder,
) where
    T: tonic::client::GrpcService<tonic::body::Body> + Clone + Send + 'static,
    T::Error: Into<StdError>,
    T::ResponseBody: Body<Data = Bytes> + Send + 'static,
    <T::ResponseBody as Body>::Error: Into<StdError> + Send,
    T::Future: Send,
{
    let start = Instant::now();
    let req = tonic::Request::new(EchoRequest {
        payload: payload.to_vec(),
    });
    match tokio::time::timeout(request_timeout, client.echo(req)).await {
        Ok(Ok(resp)) => {
            let lat = start.elapsed();
            // Length-only check keeps per-request overhead O(1) (metrics NFR);
            // content correctness is validated by the preflight self-check.
            if resp.into_inner().payload.len() == payload.len() {
                rec.record_success(lat);
            } else {
                rec.record_failure();
            }
        }
        // RPC error or timeout: both count as a failure.
        _ => rec.record_failure(),
    }
}

/// Run `total` requests spread across `concurrency` workers pulling from a
/// shared counter, then merge the per-worker recorders.
async fn run_count<T>(
    client: &EchoClient<T>,
    payload: &Arc<Vec<u8>>,
    concurrency: usize,
    total: u64,
    request_timeout: Duration,
) -> Recorder
where
    T: tonic::client::GrpcService<tonic::body::Body> + Clone + Send + 'static,
    T::Error: Into<StdError>,
    T::ResponseBody: Body<Data = Bytes> + Send + 'static,
    <T::ResponseBody as Body>::Error: Into<StdError> + Send,
    T::Future: Send,
{
    let counter = Arc::new(AtomicU64::new(0));
    let mut handles = Vec::new();
    for _ in 0..concurrency.max(1) {
        let mut c = client.clone();
        let counter = counter.clone();
        let payload = payload.clone();
        handles.push(tokio::spawn(async move {
            let mut rec = Recorder::new();
            loop {
                let n = counter.fetch_add(1, Ordering::Relaxed);
                if n >= total {
                    break;
                }
                one_request(&mut c, &payload, request_timeout, &mut rec).await;
            }
            rec
        }));
    }
    let mut merged = Recorder::new();
    for h in handles {
        match h.await {
            Ok(r) => merged.merge(&r),
            // A panicked/cancelled worker is accounted as one failed request so
            // lost work is not silently dropped from the totals.
            Err(_) => merged.record_failure(),
        }
    }
    merged
}

/// Run for `dur` wall-clock time across `concurrency` workers.
async fn run_duration<T>(
    client: &EchoClient<T>,
    payload: &Arc<Vec<u8>>,
    concurrency: usize,
    dur: Duration,
    request_timeout: Duration,
) -> Recorder
where
    T: tonic::client::GrpcService<tonic::body::Body> + Clone + Send + 'static,
    T::Error: Into<StdError>,
    T::ResponseBody: Body<Data = Bytes> + Send + 'static,
    <T::ResponseBody as Body>::Error: Into<StdError> + Send,
    T::Future: Send,
{
    let deadline = Instant::now() + dur;
    let mut handles = Vec::new();
    for _ in 0..concurrency.max(1) {
        let mut c = client.clone();
        let payload = payload.clone();
        handles.push(tokio::spawn(async move {
            let mut rec = Recorder::new();
            while Instant::now() < deadline {
                one_request(&mut c, &payload, request_timeout, &mut rec).await;
            }
            rec
        }));
    }
    let mut merged = Recorder::new();
    for h in handles {
        match h.await {
            Ok(r) => merged.merge(&r),
            Err(_) => merged.record_failure(),
        }
    }
    merged
}
