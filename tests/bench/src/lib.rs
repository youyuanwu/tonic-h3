//! gRPC echo benchmark harness for tonic-h3.
//!
//! This crate provides a shared library plus two binaries (`bench-server` and
//! `bench-client`) that measure a gRPC **echo** workload across five transports:
//! a TCP+TLS (HTTP/2) baseline via `tonic-tls`, and tonic-h3 (HTTP/3) over
//! `quinn`, `msquic`, `s2n-quic`, and `quiche`.
//!
//! Only the `quinn` backend is production-supported; `msquic`, `s2n-quic`, and
//! `quiche` are experimental. See `tests/bench/README.md` for usage.

pub mod client;
pub mod load;
pub mod metrics;
pub mod server;
pub mod tls;

// Generated echo service (package `echo`): `echo_server`, `echo_client`,
// `EchoRequest`, `EchoReply`.
tonic::include_proto!("echo");

/// Convenience error type for the harness.
pub type BenchError = Box<dyn std::error::Error + Send + Sync>;

/// The transport under test. Rendered by clap as `tcp-tls`, `quinn`, `msquic`,
/// `s2n-quic`, `quiche`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Transport {
    /// tonic gRPC over TCP + TLS (HTTP/2). Baseline.
    TcpTls,
    /// tonic-h3 over quinn (HTTP/3). Production-supported.
    Quinn,
    /// tonic-h3 over msquic (HTTP/3). Experimental.
    Msquic,
    /// tonic-h3 over s2n-quic (HTTP/3). Experimental.
    S2nQuic,
    /// tonic-h3 over quiche (HTTP/3). Experimental.
    Quiche,
}

impl std::fmt::Display for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Transport::TcpTls => "tcp-tls",
            Transport::Quinn => "quinn",
            Transport::Msquic => "msquic",
            Transport::S2nQuic => "s2n-quic",
            Transport::Quiche => "quiche",
        };
        f.write_str(s)
    }
}

/// How the `bench-client` renders its result. `Text` (default) prints the
/// human-readable `=== Benchmark result ===` block; `Json` prints a single
/// machine-readable JSON object for aggregating runs across SKUs/hosts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    /// Human-readable text block (unchanged default).
    #[default]
    Text,
    /// Single-line JSON object with run metadata and metrics.
    Json,
}

/// The echo service implementation: returns the request payload unchanged.
#[derive(Default, Clone)]
pub struct EchoService;

#[tonic::async_trait]
impl echo_server::Echo for EchoService {
    async fn echo(
        &self,
        req: tonic::Request<EchoRequest>,
    ) -> Result<tonic::Response<EchoReply>, tonic::Status> {
        let payload = req.into_inner().payload;
        Ok(tonic::Response::new(EchoReply { payload }))
    }
}

/// Build the tonic `Routes` serving the echo service (used by the H3 backends).
pub fn echo_routes() -> tonic::service::Routes {
    let mut builder = tonic::service::Routes::builder();
    builder.add_service(echo_server::EchoServer::new(EchoService));
    builder.routes()
}

/// CLI argument definitions (kept in the library so they can be unit-tested).
pub mod cli {
    use super::{OutputFormat, Transport};
    use crate::load::{LoadConfig, LoadLimit};
    use clap::Parser;
    use std::time::Duration;

    const DEFAULT_ADDR: &str = "127.0.0.1:5000";

    /// `bench-server` arguments.
    #[derive(Parser, Debug)]
    #[command(name = "bench-server", about = "gRPC echo benchmark server")]
    pub struct ServerArgs {
        /// Transport to serve.
        #[arg(long, value_enum)]
        pub transport: Transport,

        /// Address to bind (host:port).
        #[arg(long, default_value = DEFAULT_ADDR)]
        pub addr: String,
    }

    /// `bench-client` arguments.
    #[derive(Parser, Debug)]
    #[command(
        name = "bench-client",
        about = "gRPC echo benchmark client / load generator"
    )]
    pub struct ClientArgs {
        /// Transport to connect with.
        #[arg(long, value_enum)]
        pub transport: Transport,

        /// Server address to target (host:port).
        #[arg(long, default_value = DEFAULT_ADDR)]
        pub addr: String,

        /// Output format for the result: `text` (default, human-readable) or
        /// `json` (single-line machine-readable object).
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        pub format: OutputFormat,

        /// Echo payload size in bytes.
        #[arg(long, default_value_t = 64)]
        pub payload_size: usize,

        /// Number of concurrent in-flight workers (must be >= 1).
        #[arg(long, default_value_t = 16, value_parser = clap::value_parser!(u32).range(1..))]
        pub concurrency: u32,

        /// Total number of requests to send (mutually exclusive with --duration).
        #[arg(long, group = "load")]
        pub count: Option<u64>,

        /// Run for this many seconds instead of a fixed count (mutually
        /// exclusive with --count).
        #[arg(long, group = "load")]
        pub duration: Option<u64>,

        /// Optional warmup duration in seconds (results discarded).
        #[arg(long)]
        pub warmup: Option<u64>,

        /// Connect/preflight timeout in seconds (fail fast when unreachable).
        #[arg(long, default_value_t = 5)]
        pub connect_timeout: u64,

        /// Per-request timeout in seconds. A request exceeding this is counted
        /// as a failure, so runs always terminate even if a backend stalls.
        #[arg(long, default_value_t = 30)]
        pub request_timeout: u64,
    }

    impl ClientArgs {
        /// Default request count when neither --count nor --duration is given.
        pub const DEFAULT_COUNT: u64 = 10_000;

        /// Resolve the load limit (defaults to a fixed count).
        pub fn limit(&self) -> LoadLimit {
            match (self.count, self.duration) {
                // clap's mutually-exclusive group prevents both being set.
                (_, Some(d)) => LoadLimit::Duration(Duration::from_secs(d)),
                (Some(n), None) => LoadLimit::Count(n),
                (None, None) => LoadLimit::Count(Self::DEFAULT_COUNT),
            }
        }

        /// Build a [`LoadConfig`] from these arguments.
        pub fn load_config(&self) -> LoadConfig {
            LoadConfig {
                payload_size: self.payload_size,
                concurrency: self.concurrency as usize,
                limit: self.limit(),
                warmup: self.warmup.map(Duration::from_secs),
                connect_timeout: Duration::from_secs(self.connect_timeout),
                request_timeout: Duration::from_secs(self.request_timeout),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn count_and_duration_are_mutually_exclusive() {
            let r = ClientArgs::try_parse_from([
                "bench-client",
                "--transport",
                "quinn",
                "--count",
                "5",
                "--duration",
                "3",
            ]);
            assert!(
                r.is_err(),
                "supplying both --count and --duration must error"
            );
        }

        #[test]
        fn defaults_apply_when_not_specified() {
            let a = ClientArgs::try_parse_from(["bench-client", "--transport", "quinn"]).unwrap();
            assert_eq!(a.payload_size, 64);
            assert_eq!(a.concurrency, 16);
            assert_eq!(a.connect_timeout, 5);
            assert!(a.count.is_none());
            assert!(a.duration.is_none());
            assert!(matches!(
                a.limit(),
                LoadLimit::Count(ClientArgs::DEFAULT_COUNT)
            ));
            // Text output is the default so existing behavior is preserved.
            assert_eq!(a.format, OutputFormat::Text);
        }

        #[test]
        fn format_json_parses() {
            let a = ClientArgs::try_parse_from([
                "bench-client",
                "--transport",
                "quinn",
                "--format",
                "json",
            ])
            .unwrap();
            assert_eq!(a.format, OutputFormat::Json);
        }

        #[test]
        fn transport_value_enum_parses_kebab_case() {
            let a =
                ClientArgs::try_parse_from(["bench-client", "--transport", "s2n-quic"]).unwrap();
            assert_eq!(a.transport, Transport::S2nQuic);
        }

        #[test]
        fn zero_concurrency_is_rejected() {
            let r = ClientArgs::try_parse_from([
                "bench-client",
                "--transport",
                "quinn",
                "--concurrency",
                "0",
            ]);
            assert!(r.is_err(), "--concurrency 0 must be rejected");
        }

        #[test]
        fn duration_limit_selected() {
            let a = ClientArgs::try_parse_from([
                "bench-client",
                "--transport",
                "quinn",
                "--duration",
                "7",
            ])
            .unwrap();
            assert!(matches!(a.limit(), LoadLimit::Duration(d) if d == Duration::from_secs(7)));
        }
    }
}

/// Install a simple tracing subscriber honoring `RUST_LOG` (defaults to info).
pub fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    // Diagnostics go to stderr so stdout carries ONLY the result output
    // (the `=== Benchmark result ===` block or, with `--format json`, a single
    // JSON line). This lets orchestration capture stdout as a clean artifact.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
