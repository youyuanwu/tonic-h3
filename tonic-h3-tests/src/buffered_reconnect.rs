//! Regression test for MF-2: a transient connect/DNS failure must NOT permanently
//! brick a buffered `H3Channel` (nor its clones), and the non-buffered path must keep
//! surfacing connect failures per-request while remaining able to reconnect.
//!
//! `H3Channel` wraps the reconnecting `RequestSender` in `tower::buffer::Buffer`. Before
//! the fix, `RequestSender::poll_ready` returned a transient connect error as a readiness
//! error; the Buffer worker treats a readiness error as terminal — it closes the request
//! channel and replays the stored error to every later request and every cloned handle
//! forever. The fix defers connect errors to per-request failures (`connect_error`), so
//! the channel stays alive and reconnects on the next request.
//!
//! These tests are deterministic: a `FailOnce` connector fails its first `connect()` and
//! then delegates to a real quinn connector, so "server is briefly unreachable then
//! reachable" is reproduced without any timing/port races.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use h3_util::client::H3Connector;
use http::{Request, Uri};
use tokio_util::sync::CancellationToken;

/// An `H3Connector` wrapper whose first `connect()` call fails with a simulated transient
/// error and whose subsequent calls delegate to the inner (real) connector. All
/// associated stream types are delegated to the inner connector.
#[derive(Clone)]
struct FailOnce<C: H3Connector> {
    inner: C,
    failed: Arc<AtomicBool>,
}

impl<C: H3Connector> FailOnce<C> {
    fn new(inner: C) -> Self {
        Self {
            inner,
            failed: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl<C: H3Connector> H3Connector for FailOnce<C> {
    type CONN = C::CONN;
    type OS = C::OS;
    type SS = C::SS;
    type RS = C::RS;
    type BS = C::BS;

    fn connect(
        &self,
    ) -> impl std::future::Future<Output = Result<Self::CONN, h3_util::Error>> + Send {
        // Move owned clones into the future so it is `Send` without requiring `Sync`
        // (the future must not borrow `&self` across the await point).
        let failed = self.failed.clone();
        let inner = self.inner.clone();
        async move {
            // `swap` returns the previous value: the first call sees `false` and fails;
            // every later call sees `true` and delegates to the real connector.
            if !failed.swap(true, Ordering::SeqCst) {
                tracing::debug!("FailOnce: injecting simulated transient connect failure");
                return Err("simulated transient connect failure".into());
            }
            inner.connect().await
        }
    }
}

/// SC-002 / SC-003: a buffered `H3Channel` survives a transient connect failure — the
/// first RPC fails, a later RPC on the same channel succeeds, and clones are not bricked.
#[tokio::test]
#[test_log::test]
async fn quinn_buffered_channel_survives_transient_failure() {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let token = CancellationToken::new();
    let (h_svr, listen_addr) = crate::run_test_quinn_hello_server(addr, token.clone());

    // Let the server come up.
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let uri: Uri = format!("https://localhost:{}", listen_addr.port())
        .parse()
        .unwrap();
    let client_endpoint = crate::make_test_quinn_client_endpoint();
    let real = tonic_h3::quinn::H3QuinnConnector::new(
        uri.clone(),
        "localhost".to_string(),
        client_endpoint.clone(),
    );
    let cc = FailOnce::new(real);
    let channel = tonic_h3::H3Channel::new(cc, uri, None);
    let mut client = crate::greeter_client::GreeterClient::new(channel.clone());

    // First RPC: the injected transient connect failure surfaces as a per-request error.
    let first = client
        .say_hello(tonic::Request::new(crate::HelloRequest {
            name: "one".into(),
        }))
        .await;
    assert!(
        first.is_err(),
        "expected first RPC to fail with the transient connect error, got {first:?}"
    );
    tracing::debug!("first RPC error (expected): {:?}", first.unwrap_err());

    // Second RPC on the SAME channel: the channel is not bricked and reconnects.
    let second = client
        .say_hello(tonic::Request::new(crate::HelloRequest {
            name: "two".into(),
        }))
        .await
        .expect("expected second RPC on the same channel to succeed after reconnect");
    tracing::debug!("second RPC response: {second:?}");

    // SC-003: a clone taken after the transient failure is also healthy.
    let mut cloned_client = crate::greeter_client::GreeterClient::new(channel.clone());
    let via_clone = cloned_client
        .say_hello(tonic::Request::new(crate::HelloRequest {
            name: "clone".into(),
        }))
        .await
        .expect("expected a cloned channel handle to serve requests (not bricked)");
    tracing::debug!("clone RPC response: {via_clone:?}");

    // Cleanup.
    client_endpoint.close(0_u16.into(), b"client close");
    token.cancel();
    h_svr.await.unwrap();
}

/// FR-008: the non-buffered `H3Connection` path (shared `RequestSender`, no Buffer) keeps
/// surfacing connect failures per-request and recovers on a subsequent request. This
/// confirms the MF-2 change does not regress the non-buffered path's observable behavior.
#[tokio::test]
#[test_log::test]
async fn quinn_non_buffered_survives_transient_failure() {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let token = CancellationToken::new();
    let (h_svr, listen_addr) = crate::run_test_quinn_hello_server(addr, token.clone());

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let uri: Uri = format!("https://localhost:{}", listen_addr.port())
        .parse()
        .unwrap();
    let client_endpoint = crate::make_test_quinn_client_endpoint();
    let real = tonic_h3::quinn::H3QuinnConnector::new(
        uri.clone(),
        "localhost".to_string(),
        client_endpoint.clone(),
    );
    let cc = FailOnce::new(real);
    let channel = h3_util::client::H3Connection::new(cc, uri.clone(), None);
    let mut client = h3_util::client::H3Client::new(channel);

    let build_req = || {
        Request::builder()
            .method("POST")
            .uri(uri.clone())
            .body(http_body_util::Empty::<hyper::body::Bytes>::new())
            .unwrap()
    };

    // First request: transient connect failure surfaces as a per-request error.
    let first = client.send(build_req()).await;
    assert!(
        first.is_err(),
        "expected first non-buffered request to fail with the transient connect error"
    );
    tracing::debug!("non-buffered first error (expected): {:?}", first.err());

    // Second request: the shared RequestSender reconnects and the HTTP/3 exchange
    // completes (a transport-level response is returned regardless of gRPC status).
    let second = client.send(build_req()).await;
    assert!(
        second.is_ok(),
        "expected second non-buffered request to succeed after reconnect, got {:?}",
        second.err()
    );
    tracing::debug!("non-buffered second request succeeded");

    // Cleanup.
    client_endpoint.close(0_u16.into(), b"client close");
    token.cancel();
    h_svr.await.unwrap();
}
