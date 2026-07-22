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
    {
        let mut c = client.clone();
        let payload = vec![0u8; cfg.payload_size];
        let fut = c.echo(tonic::Request::new(EchoRequest {
            payload: payload.clone(),
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
                if resp.into_inner().payload.len() != cfg.payload_size {
                    return Err("echo self-check failed: reply payload size mismatch".into());
                }
            }
        }
    }

    // --- Optional warmup (results discarded) ---
    if let Some(w) = cfg.warmup {
        let _ = run_duration(&client, cfg.payload_size, cfg.concurrency, w).await;
    }

    // --- Measured run ---
    let start = Instant::now();
    let rec = match cfg.limit {
        LoadLimit::Count(n) => run_count(&client, cfg.payload_size, cfg.concurrency, n).await,
        LoadLimit::Duration(d) => run_duration(&client, cfg.payload_size, cfg.concurrency, d).await,
    };
    let elapsed = start.elapsed();
    Ok(BenchSummary::from_recorder(&rec, elapsed))
}

/// Issue a single echo RPC, recording latency (success) or a failure.
async fn one_request<T>(client: &mut EchoClient<T>, payload_size: usize, rec: &mut Recorder)
where
    T: tonic::client::GrpcService<tonic::body::Body> + Clone + Send + 'static,
    T::Error: Into<StdError>,
    T::ResponseBody: Body<Data = Bytes> + Send + 'static,
    <T::ResponseBody as Body>::Error: Into<StdError> + Send,
    T::Future: Send,
{
    let payload = vec![0u8; payload_size];
    let start = Instant::now();
    let req = tonic::Request::new(EchoRequest { payload });
    match client.echo(req).await {
        Ok(resp) => {
            let lat = start.elapsed();
            if resp.into_inner().payload.len() == payload_size {
                rec.record_success(lat);
            } else {
                rec.record_failure();
            }
        }
        Err(_) => rec.record_failure(),
    }
}

/// Run `total` requests spread across `concurrency` workers pulling from a
/// shared counter, then merge the per-worker recorders.
async fn run_count<T>(
    client: &EchoClient<T>,
    payload_size: usize,
    concurrency: usize,
    total: u64,
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
        handles.push(tokio::spawn(async move {
            let mut rec = Recorder::new();
            loop {
                let n = counter.fetch_add(1, Ordering::Relaxed);
                if n >= total {
                    break;
                }
                one_request(&mut c, payload_size, &mut rec).await;
            }
            rec
        }));
    }
    let mut merged = Recorder::new();
    for h in handles {
        if let Ok(r) = h.await {
            merged.merge(&r);
        }
    }
    merged
}

/// Run for `dur` wall-clock time across `concurrency` workers.
async fn run_duration<T>(
    client: &EchoClient<T>,
    payload_size: usize,
    concurrency: usize,
    dur: Duration,
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
        handles.push(tokio::spawn(async move {
            let mut rec = Recorder::new();
            while Instant::now() < deadline {
                one_request(&mut c, payload_size, &mut rec).await;
            }
            rec
        }));
    }
    let mut merged = Recorder::new();
    for h in handles {
        if let Ok(r) = h.await {
            merged.merge(&r);
        }
    }
    merged
}
