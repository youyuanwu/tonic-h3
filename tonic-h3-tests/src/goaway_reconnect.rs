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

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::extract::State;
use axum::routing::post;
use h3_util::client::H3Connector;
use h3_util::quinn::h3_quinn;
use http::{Request, Uri};
use http_body_util::BodyExt;
use hyper::body::{Body, Bytes, Frame};
use tokio::sync::mpsc;
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

// ---------------------------------------------------------------------------
// FR-006 negative case: a *per-stream* error must NOT retire the cached sender.
//
// This complements `quinn_goaway_triggers_reconnect` (the connection-level
// GOAWAY *does* reconnect, count -> 2). Here a stream-level disruption on one
// request must NOT reconnect (count stays 1), and a subsequent normal request
// must reuse the same connection (count still 1).
//
// Per the SF-3 reliability lesson, the server is built on the proven `axum-h3`
// harness (mirroring `cancel_reset.rs`), NOT a hand-rolled raw-`h3` server.
// ---------------------------------------------------------------------------

/// Terminal outcome of the server draining a request body on `/drain`.
#[derive(Debug)]
enum DrainOutcome {
    /// The body ended with a graceful end-of-stream after `usize` frames.
    CleanEof(usize),
    /// The body read failed with a stream error (reset); the string carries the
    /// full error chain so the HTTP/3 error code can be asserted.
    StreamError(String),
}

/// Flatten an error and its `source` chain into one string so the HTTP/3 reset
/// code (e.g. `H3_INTERNAL_ERROR`) can be asserted regardless of nesting.
fn stringify_err(e: &(dyn std::error::Error + 'static)) -> String {
    let mut s = format!("{e} | {e:?}");
    let mut src = e.source();
    while let Some(inner) = src {
        s.push_str(&format!(" -> {inner} | {inner:?}"));
        src = inner.source();
    }
    s
}

/// Request body that emits one complete data frame, waits briefly so the frame
/// and request head are flushed to the server, then returns an error to simulate
/// a local body-source failure mid-stream. This resets the *request* stream
/// (a per-stream disruption) without closing the connection — mirroring the
/// proven `FailAfterOneFrame` in `cancel_reset.rs`.
struct BodyErrsAfterOneFrame {
    sent: bool,
    delay: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl Body for BodyErrsAfterOneFrame {
    type Data = Bytes;
    type Error = h3_util::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if !self.sent {
            self.sent = true;
            self.delay = Some(Box::pin(tokio::time::sleep(Duration::from_millis(300))));
            return Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(&[0u8; 512])))));
        }
        // Wait until the first frame has certainly been flushed and read by the
        // server, then fail so the reset is observed mid-stream.
        let delay = self.delay.as_mut().expect("delay armed after first frame");
        match delay.as_mut().poll(cx) {
            Poll::Ready(()) => {
                Poll::Ready(Some(Err("simulated client body-source failure".into())))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Body type used by the counting client. Both requests (the erroring upload and
/// the subsequent normal request) must share one body type because `H3Client<C, B>`
/// fixes `B` per client, so both are boxed into this common type.
type BoxedReqBody = http_body_util::combinators::UnsyncBoxBody<Bytes, h3_util::Error>;

type OutcomeTx = mpsc::UnboundedSender<DrainOutcome>;

/// Normal route: always responds `200 OK` (no body read needed).
async fn ok_handler() -> axum::http::StatusCode {
    axum::http::StatusCode::OK
}

/// Responds `200 OK` immediately and drains the request body on a background
/// task, reporting the terminal read outcome (clean EOF vs stream reset). This
/// lets the client receive response headers while the request upload is still
/// open, so a mid-upload body-source error resets the request stream.
async fn drain_handler(
    State(tx): State<OutcomeTx>,
    req: axum::extract::Request,
) -> axum::http::StatusCode {
    let mut body = req.into_body();
    tokio::spawn(async move {
        let mut frames = 0usize;
        loop {
            match body.frame().await {
                Some(Ok(_)) => frames += 1,
                Some(Err(e)) => {
                    let _ = tx.send(DrainOutcome::StreamError(stringify_err(&e)));
                    return;
                }
                None => {
                    let _ = tx.send(DrainOutcome::CleanEof(frames));
                    return;
                }
            }
        }
    });
    axum::http::StatusCode::OK
}

/// An `axum-h3` server over quinn with a normal `/ok` route and a `/drain` route
/// that reports its terminal read outcome. Reuses the proven harness pattern from
/// `cancel_reset.rs` rather than a bespoke raw-`h3` server.
struct OkServer {
    listen_addr: SocketAddr,
    outcomes: mpsc::UnboundedReceiver<DrainOutcome>,
    token: CancellationToken,
    handle: tokio::task::JoinHandle<()>,
}

impl OkServer {
    async fn shutdown(self) {
        self.token.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(2), self.handle).await;
    }
}

/// Start the `axum-h3` server (`/ok` normal, `/drain` reports read outcome).
fn start_ok_server() -> OkServer {
    let (tx, rx) = mpsc::unbounded_channel::<DrainOutcome>();
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let ep = crate::make_quinn_server_endpoint(addr);
    let listen_addr = ep.local_addr().unwrap();
    let acceptor = h3_util::quinn::H3QuinnAcceptor::new(ep);

    let app = axum::Router::new()
        .route("/ok", post(ok_handler))
        .route("/drain", post(drain_handler))
        .with_state(tx);

    let token = CancellationToken::new();
    let token_cp = token.clone();
    let handle = tokio::spawn(async move {
        axum_h3::H3Router::new(app)
            .serve_with_shutdown(acceptor, async move { token_cp.cancelled().await })
            .await
            .unwrap();
    });

    OkServer {
        listen_addr,
        outcomes: rx,
        token,
        handle,
    }
}

/// Build an `H3Client` over quinn whose connector is wrapped in `CountConnect`,
/// returning the shared connect counter. This is the connect-counting variant of
/// `cancel_reset.rs`'s `make_client!` (which hardcodes an uncounted connector).
macro_rules! make_counting_client {
    ($listen_addr:expr) => {{
        let uri: Uri = format!("https://{}", $listen_addr).parse().unwrap();
        let client_endpoint = crate::make_test_quinn_client_endpoint();
        let real = tonic_h3::quinn::H3QuinnConnector::new(
            uri.clone(),
            "localhost".to_string(),
            client_endpoint.clone(),
        );
        let (cc, connect_count) = CountConnect::new(real);
        let channel = h3_util::client::H3Connection::new(cc, uri, None);
        let client = h3_util::client::H3Client::<_, BoxedReqBody>::new(channel);
        (client_endpoint, client, connect_count)
    }};
}

/// Boxed empty request body (`200 OK` probe) as the shared `BoxedReqBody` type.
fn empty_boxed_body() -> BoxedReqBody {
    http_body_util::Empty::<Bytes>::new()
        .map_err(|_: std::convert::Infallible| -> h3_util::Error { unreachable!() })
        .boxed_unsync()
}

/// SF-3 / FR-006 negative case: a per-stream error (a request-body-source failure
/// that resets one request stream) must NOT retire the cached `SendRequest`, so
/// the client must NOT reconnect (connect count stays 1). A subsequent normal
/// request must then succeed on the SAME connection (count still 1).
#[tokio::test]
#[test_log::test]
#[serial_test::serial(goaway_reconnect)]
async fn quinn_stream_error_does_not_reconnect() {
    let mut server = start_ok_server();
    tokio::time::sleep(Duration::from_secs(1)).await;

    let (client_endpoint, mut client, connect_count) = make_counting_client!(server.listen_addr);

    // Request #1: induce a per-stream body-source error mid-upload against `/drain`.
    // `/drain` responds `200 OK` immediately (reading the body on a background task),
    // so `send` returns a response while the upload is still open; the body-source
    // error then resets the request stream mid-upload. Hold the response so its drop
    // does not pre-empt the body-source error.
    let req = Request::builder()
        .method("POST")
        .uri(format!("https://{}/drain", server.listen_addr))
        .body(
            BodyErrsAfterOneFrame {
                sent: false,
                delay: None,
            }
            .boxed_unsync(),
        )
        .unwrap();
    let resp = tokio::time::timeout(Duration::from_secs(5), client.send(req))
        .await
        .expect("first request send timed out")
        .expect("first request send failed");
    assert!(
        resp.status().is_success(),
        "the `/drain` route responds 200 before the body error"
    );
    assert_eq!(
        connect_count.load(Ordering::SeqCst),
        1,
        "exactly one connect for the first request"
    );

    // Wait until the server actually observes the per-stream reset (H3_INTERNAL_ERROR),
    // so the disruption is a confirmed fact before we assert no reconnect happened.
    let outcome = tokio::time::timeout(Duration::from_secs(5), server.outcomes.recv())
        .await
        .expect("timed out waiting for the drain outcome")
        .expect("server outcome channel closed");
    match &outcome {
        DrainOutcome::StreamError(s) => assert!(
            s.contains("H3_INTERNAL_ERROR"),
            "expected a stream-level reset (H3_INTERNAL_ERROR), got: {s}"
        ),
        DrainOutcome::CleanEof(n) => panic!(
            "interrupted body must reset the request stream, but server saw a clean EOF after {n} frames"
        ),
    }

    // FR-006 core assertion: a per-stream error must NOT retire the cached sender.
    assert_eq!(
        connect_count.load(Ordering::SeqCst),
        1,
        "a per-stream error must NOT trigger a reconnect (connect count must stay 1)"
    );

    // The stream is already reset; dropping the held response is now harmless.
    drop(resp);

    // A subsequent normal request must succeed on the SAME connection (count still 1).
    let ok_req = Request::builder()
        .method("POST")
        .uri(format!("https://{}/ok", server.listen_addr))
        .body(empty_boxed_body())
        .unwrap();
    let ok_resp = tokio::time::timeout(Duration::from_secs(5), client.send(ok_req))
        .await
        .expect("second request send timed out")
        .expect("second request send failed");
    assert!(
        ok_resp.status().is_success(),
        "a subsequent normal request should succeed on the same connection"
    );
    assert_eq!(
        connect_count.load(Ordering::SeqCst),
        1,
        "the subsequent request must reuse the same connection (connect count still 1)"
    );

    // Cleanup. Close the client endpoint immediately (like the sibling positive
    // test) rather than `wait_idle()`, so teardown does not wait out the ~30s idle
    // timeout — the per-stream reset was already confirmed via the outcome channel.
    drop(ok_resp);
    client_endpoint.close(0_u16.into(), b"client close");
    server.shutdown().await;
}
