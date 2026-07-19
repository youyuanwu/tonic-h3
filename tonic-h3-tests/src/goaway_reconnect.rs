//! Regression tests for SF-3: a peer GOAWAY must NOT pin the client to a cached
//! `SendRequest` that rejects every subsequent request forever.
//!
//! In h3 0.0.8, when the peer sends GOAWAY the connection is marked closing while the
//! client driver task can keep running (its `poll_close` stays pending) to service
//! in-flight streams. So the existing driver-ended reconnect path (`driver_rx`) does NOT
//! fire, yet `SendRequest::send_request` returns `StreamError::RemoteClosing` for every
//! NEW request. The fix makes the per-request future flag a connection-level closing
//! condition so `poll_ready` retires the cached sender and the next request reconnects.
//!
//! These tests drive a real quinn + h3 server (a fake `SendRequest` cannot be injected —
//! it is a concrete type produced by `h3::client::new`). The server sends a genuine
//! GOAWAY via `h3::server::Connection::shutdown(0)` and then keeps the connection/driver
//! alive, reproducing the exact SF-3 condition (driver stays alive, `driver_rx` pending).
//!
//! A connect-counting connector (mirroring `FailOnce` in `buffered_reconnect.rs`) lets the
//! tests assert whether the client actually reconnected (connect count) rather than
//! merely "eventually succeeded".

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use h3_util::client::H3Connector;
use h3_util::quinn::h3_quinn;
use http::{Request, Uri};
use hyper::body::Bytes;
use tokio_util::sync::CancellationToken;

/// An `H3Connector` wrapper that counts how many times `connect()` is invoked, delegating
/// every call to the inner (real) connector. The shared counter lets a test assert that a
/// reconnect actually happened (count == 2) — or did NOT happen (count stays 1).
#[derive(Clone)]
struct CountConnect<C: H3Connector> {
    inner: C,
    count: Arc<AtomicUsize>,
}

impl<C: H3Connector> CountConnect<C> {
    fn new(inner: C) -> (Self, Arc<AtomicUsize>) {
        let count = Arc::new(AtomicUsize::new(0));
        (
            Self {
                inner,
                count: count.clone(),
            },
            count,
        )
    }
}

impl<C: H3Connector> H3Connector for CountConnect<C> {
    type CONN = C::CONN;
    type OS = C::OS;
    type SS = C::SS;
    type RS = C::RS;
    type BS = C::BS;

    fn connect(
        &self,
    ) -> impl std::future::Future<Output = Result<Self::CONN, h3_util::Error>> + Send {
        // Own the clones so the returned future is `Send` and does not borrow `&self`.
        let inner = self.inner.clone();
        let count = self.count.clone();
        async move {
            count.fetch_add(1, Ordering::SeqCst);
            inner.connect().await
        }
    }
}

type ServerConn = h3::server::Connection<h3_quinn::Connection, Bytes>;

/// Accept a single request on `conn` and answer it with an empty `200` response.
/// Returns `true` if a request was served, `false` if the connection ended.
async fn serve_one_ok(conn: &mut ServerConn) -> bool {
    match conn.accept().await {
        Ok(Some(resolver)) => match resolver.resolve_request().await {
            Ok((_req, mut stream)) => {
                let resp = http::Response::builder().status(200).body(()).unwrap();
                if stream.send_response(resp).await.is_err() {
                    return true;
                }
                let _ = stream.finish().await;
                true
            }
            Err(_) => true,
        },
        Ok(None) | Err(_) => false,
    }
}

/// Serve requests normally until the connection ends or the token is cancelled.
async fn serve_forever(conn: &mut ServerConn, token: &CancellationToken) {
    loop {
        tokio::select! {
            _ = token.cancelled() => break,
            served = serve_one_ok(conn) => {
                if !served {
                    break;
                }
            }
        }
    }
}

/// Spawn a quinn + h3 server whose FIRST connection serves exactly one request, then sends
/// GOAWAY (`shutdown(0)`) and keeps the connection alive so the client driver's
/// `poll_close` stays pending. Every subsequent connection (the reconnect) is served
/// normally. Returns the accept-loop join handle.
fn run_goaway_server(
    endpoint: h3_quinn::quinn::Endpoint,
    token: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut idx = 0usize;
        loop {
            let incoming = tokio::select! {
                _ = token.cancelled() => break,
                inc = endpoint.accept() => match inc {
                    Some(i) => i,
                    None => break,
                },
            };
            idx += 1;
            let is_first = idx == 1;
            let token = token.clone();
            tokio::spawn(async move {
                let quinn_conn = match incoming.await {
                    Ok(c) => c,
                    Err(_) => return,
                };
                let h3_conn = h3_quinn::Connection::new(quinn_conn);
                let mut conn = match ServerConn::new(h3_conn).await {
                    Ok(c) => c,
                    Err(_) => return,
                };
                if is_first {
                    // Serve exactly one request, then send GOAWAY and hold the connection
                    // open so the client driver stays alive (driver_rx must NOT fire).
                    let _ = serve_one_ok(&mut conn).await;
                    let _ = conn.shutdown(0).await;
                    token.cancelled().await;
                } else {
                    // The reconnect: serve normally.
                    serve_forever(&mut conn, &token).await;
                }
            });
        }
    })
}

fn empty_post(uri: &Uri) -> Request<http_body_util::Empty<Bytes>> {
    Request::builder()
        .method("POST")
        .uri(uri.clone())
        .body(http_body_util::Empty::<Bytes>::new())
        .unwrap()
}

/// SF-3 core: after a peer GOAWAY (driver kept alive), the client retires the cached
/// sender and reconnects instead of failing every request forever.
#[tokio::test]
#[test_log::test]
async fn quinn_goaway_triggers_reconnect() {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let endpoint = crate::make_quinn_server_endpoint(addr);
    let listen_addr = endpoint.local_addr().unwrap();
    let token = CancellationToken::new();
    let h_svr = run_goaway_server(endpoint, token.clone());

    let uri: Uri = format!("https://localhost:{}", listen_addr.port())
        .parse()
        .unwrap();
    let client_endpoint = crate::make_test_quinn_client_endpoint();
    let real = tonic_h3::quinn::H3QuinnConnector::new(
        uri.clone(),
        "localhost".to_string(),
        client_endpoint.clone(),
    );
    let (cc, connect_count) = CountConnect::new(real);
    let channel = h3_util::client::H3Connection::new(cc, uri.clone(), None);
    let mut client = h3_util::client::H3Client::new(channel);

    // Request #1 succeeds on connection 1.
    client
        .send(empty_post(&uri))
        .await
        .expect("first request should succeed on connection 1");
    assert_eq!(
        connect_count.load(Ordering::SeqCst),
        1,
        "exactly one connect for the first request"
    );

    // Give the GOAWAY time to reach and be processed by the client driver. The bounded
    // probe loop below is the deterministic backstop: it retries until the client has
    // observed the closing condition and reconnected.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let mut saw_closing_failure = false;
    let mut reconnected = false;
    for attempt in 0..20 {
        let res = tokio::time::timeout(Duration::from_secs(3), client.send(empty_post(&uri))).await;
        match res {
            Ok(Ok(_resp)) => {
                // Recovered via reconnection.
                reconnected = true;
                break;
            }
            Ok(Err(e)) => {
                let msg = e.to_string();
                tracing::debug!("probe attempt {attempt} failed (expected once): {msg}");
                // The first post-GOAWAY request must fail cleanly with the closing signal
                // (RemoteClosing -> "Remote is closing the connection"), never a panic.
                if msg.contains("closing") {
                    saw_closing_failure = true;
                }
            }
            Err(_timeout) => {
                tracing::debug!("probe attempt {attempt} timed out; retrying");
            }
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }

    assert!(
        saw_closing_failure,
        "expected at least one post-GOAWAY request to fail with a connection-closing error"
    );
    assert!(
        reconnected,
        "expected a subsequent request to succeed via reconnection after GOAWAY"
    );
    assert_eq!(
        connect_count.load(Ordering::SeqCst),
        2,
        "the client must have established a second connection (reconnect), not reused the closing one"
    );

    // Cleanup.
    client_endpoint.close(0_u16.into(), b"client close");
    token.cancel();
    let _ = h_svr.await;
}

/// Spawn a server for a SINGLE connection whose first request stream is reset at the
/// stream level (no GOAWAY), and whose subsequent requests are served normally. This
/// exercises the FR-006 guarantee that an ordinary per-stream error must NOT retire the
/// shared cached sender.
fn run_stream_reset_server(
    endpoint: h3_quinn::quinn::Endpoint,
    token: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let incoming = tokio::select! {
            _ = token.cancelled() => return,
            inc = endpoint.accept() => match inc {
                Some(i) => i,
                None => return,
            },
        };
        let quinn_conn = match incoming.await {
            Ok(c) => c,
            Err(_) => return,
        };
        let h3_conn = h3_quinn::Connection::new(quinn_conn);
        let mut conn = match ServerConn::new(h3_conn).await {
            Ok(c) => c,
            Err(_) => return,
        };

        // First request: reset the stream (stream-level error, connection stays healthy).
        if let Ok(Some(resolver)) = conn.accept().await
            && let Ok((_req, mut stream)) = resolver.resolve_request().await
        {
            stream.stop_stream(h3::error::Code::H3_INTERNAL_ERROR);
            // Drop the stream without sending a response -> client sees a stream error.
        }

        // Subsequent requests on the SAME connection are served normally.
        serve_forever(&mut conn, &token).await;
    })
}

/// FR-006 / SC-003: an ordinary per-stream error must NOT invalidate the shared cached
/// sender. The connect count must stay at 1 (no reconnect) and a subsequent request must
/// succeed on the SAME connection.
#[tokio::test]
#[test_log::test]
async fn quinn_stream_error_does_not_reconnect() {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let endpoint = crate::make_quinn_server_endpoint(addr);
    let listen_addr = endpoint.local_addr().unwrap();
    let token = CancellationToken::new();
    let h_svr = run_stream_reset_server(endpoint, token.clone());

    let uri: Uri = format!("https://localhost:{}", listen_addr.port())
        .parse()
        .unwrap();
    let client_endpoint = crate::make_test_quinn_client_endpoint();
    let real = tonic_h3::quinn::H3QuinnConnector::new(
        uri.clone(),
        "localhost".to_string(),
        client_endpoint.clone(),
    );
    let (cc, connect_count) = CountConnect::new(real);
    let channel = h3_util::client::H3Connection::new(cc, uri.clone(), None);
    let mut client = h3_util::client::H3Client::new(channel);

    // Request #1: the server resets the stream -> a per-stream error surfaces.
    let first = tokio::time::timeout(Duration::from_secs(5), client.send(empty_post(&uri))).await;
    assert!(
        matches!(first, Ok(Err(_))),
        "expected the first request to fail with a stream-level error (Ok(Err), got timeout or success)"
    );
    assert_eq!(
        connect_count.load(Ordering::SeqCst),
        1,
        "a stream-level error must NOT retire the sender: still exactly one connect"
    );

    // Request #2 must succeed on the SAME connection (no reconnect).
    let second = tokio::time::timeout(Duration::from_secs(5), client.send(empty_post(&uri))).await;
    assert!(
        matches!(second, Ok(Ok(_))),
        "expected a subsequent request to succeed on the same connection (got timeout or error)"
    );
    assert_eq!(
        connect_count.load(Ordering::SeqCst),
        1,
        "no reconnect should have happened for a per-stream error (connect count stays 1)"
    );

    // Cleanup.
    client_endpoint.close(0_u16.into(), b"client close");
    token.cancel();
    let _ = h_svr.await;
}
