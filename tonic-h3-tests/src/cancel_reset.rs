//! Integration tests for MF-3: interrupted HTTP/3 body sends must reset the
//! send stream so the peer observes a stream error, not a clean end-of-stream.
//!
//! These tests exercise the production `quinn` backend through the raw
//! `h3-util` client (`H3Client`) and an `axum-h3` server, which gives full
//! control over the request body and lets the server observe the terminal read
//! outcome (clean EOF vs stream reset).

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::routing::post;
use http::{Request, Uri};
use http_body_util::BodyExt;
use hyper::body::{Body, Frame};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Terminal outcome of the server reading a request body.
#[derive(Debug)]
enum ReadOutcome {
    /// The body ended with a graceful end-of-stream after `usize` frames.
    CleanEof(usize),
    /// The body read failed with a stream error (reset); the string carries the
    /// full error chain so the HTTP/3 error code can be asserted.
    Error(String),
}

type OutcomeTx = mpsc::UnboundedSender<ReadOutcome>;

/// Request body that emits one complete data frame, waits briefly so the frame
/// and request head are flushed to the server, then returns an error to simulate
/// a local body-source failure (interrupt path B) mid-stream.
struct FailAfterOneFrame {
    sent: bool,
    delay: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl Body for FailAfterOneFrame {
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
        // server, then fail so the reset is observed mid-stream (not coalesced
        // with the request head).
        let delay = self.delay.as_mut().expect("delay armed after first frame");
        match delay.as_mut().poll(cx) {
            Poll::Ready(()) => {
                Poll::Ready(Some(Err("simulated client body-source failure".into())))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Request body that emits one complete data frame and then never completes,
/// keeping the request stream open until the RPC is cancelled.
struct OneFrameThenPending {
    sent: bool,
}

impl Body for OneFrameThenPending {
    type Data = Bytes;
    type Error = h3_util::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if !self.sent {
            self.sent = true;
            Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(&[7u8; 512])))))
        } else {
            // Never yields `None` and registers no waker: the only way this send
            // can end is via cancellation, which wakes the body-send select arm.
            Poll::Pending
        }
    }
}

fn stringify_err(e: &(dyn std::error::Error + 'static)) -> String {
    let mut s = format!("{e} | {e:?}");
    let mut src = e.source();
    while let Some(inner) = src {
        s.push_str(&format!(" -> {inner} | {inner:?}"));
        src = inner.source();
    }
    s
}

/// Reads the request body inline and reports the terminal outcome, then
/// responds. Used when the response should only be produced after the request
/// body has been consumed.
async fn read_handler(
    State(tx): State<OutcomeTx>,
    req: axum::extract::Request,
) -> axum::http::StatusCode {
    let mut body = req.into_body();
    let mut frames = 0usize;
    loop {
        match body.frame().await {
            Some(Ok(_)) => frames += 1,
            Some(Err(e)) => {
                let _ = tx.send(ReadOutcome::Error(stringify_err(&e)));
                return axum::http::StatusCode::INTERNAL_SERVER_ERROR;
            }
            None => {
                let _ = tx.send(ReadOutcome::CleanEof(frames));
                return axum::http::StatusCode::OK;
            }
        }
    }
}

/// Responds immediately and reads the request body on a background task,
/// reporting the terminal outcome. Used so the client receives response headers
/// (and thus a droppable response body) while the request upload is still open.
async fn spawn_read_handler(
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
                    let _ = tx.send(ReadOutcome::Error(stringify_err(&e)));
                    return;
                }
                None => {
                    let _ = tx.send(ReadOutcome::CleanEof(frames));
                    return;
                }
            }
        }
    });
    axum::http::StatusCode::OK
}

struct TestServer {
    listen_addr: SocketAddr,
    outcomes: mpsc::UnboundedReceiver<ReadOutcome>,
    token: CancellationToken,
    handle: tokio::task::JoinHandle<()>,
}

impl TestServer {
    async fn shutdown(self) {
        self.token.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(2), self.handle).await;
    }
}

/// Start an axum-h3 server over quinn with `/read` (inline) and `/spawn`
/// (background) upload endpoints reporting their terminal read outcome.
fn start_server() -> TestServer {
    let (tx, rx) = mpsc::unbounded_channel::<ReadOutcome>();
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let ep = crate::make_quinn_server_endpoint(addr);
    let listen_addr = ep.local_addr().unwrap();
    let acceptor = h3_util::quinn::H3QuinnAcceptor::new(ep);

    let app = axum::Router::new()
        .route("/read", post(read_handler))
        .route("/spawn", post(spawn_read_handler))
        .with_state(tx);

    let token = CancellationToken::new();
    let token_cp = token.clone();
    let handle = tokio::spawn(async move {
        axum_h3::H3Router::new(app)
            .serve_with_shutdown(acceptor, async move { token_cp.cancelled().await })
            .await
            .unwrap();
    });

    TestServer {
        listen_addr,
        outcomes: rx,
        token,
        handle,
    }
}

/// Build a raw `H3Client` over quinn for the given server address.
macro_rules! make_client {
    ($listen_addr:expr) => {{
        let uri: Uri = format!("https://{}", $listen_addr).parse().unwrap();
        let client_endpoint = crate::make_test_quinn_client_endpoint();
        let cc = h3_util::quinn::H3QuinnConnector::new(
            uri.clone(),
            "localhost".to_string(),
            client_endpoint.clone(),
        );
        let channel = h3_util::client::H3Connection::new(cc, uri, None);
        (client_endpoint, h3_util::client::H3Client::new(channel))
    }};
}

/// Primary: a local body-source error mid-upload must reset the send stream so
/// the server observes a stream error (with `H3_INTERNAL_ERROR`), not clean EOF.
#[tokio::test]
#[test_log::test]
async fn client_body_error_resets_stream() {
    let mut server = start_server();
    tokio::time::sleep(Duration::from_secs(1)).await;

    let (client_endpoint, mut client) = make_client!(server.listen_addr);
    let req = Request::builder()
        .method("POST")
        .uri(format!("https://{}/spawn", server.listen_addr))
        .body(FailAfterOneFrame {
            sent: false,
            delay: None,
        })
        .unwrap();

    // Hold the response alive so cancellation (which fires when the response is
    // dropped) does NOT pre-empt the body-source error; we want to prove the
    // body-source failure path resets with `H3_INTERNAL_ERROR`.
    let resp = tokio::time::timeout(Duration::from_secs(5), client.send(req))
        .await
        .expect("send timed out")
        .expect("send failed");

    let outcome = tokio::time::timeout(Duration::from_secs(5), server.outcomes.recv())
        .await
        .expect("timed out waiting for server read outcome")
        .expect("server outcome channel closed");

    match &outcome {
        ReadOutcome::Error(s) => {
            assert!(
                s.contains("H3_INTERNAL_ERROR"),
                "expected internal-error reset code, got: {s}"
            );
        }
        ReadOutcome::CleanEof(n) => {
            panic!(
                "interrupted body must reset the stream, but server saw a clean EOF after {n} frames"
            );
        }
    }

    drop(resp);
    client_endpoint.wait_idle().await;
    server.shutdown().await;
}

/// Primary: cancelling an in-flight upload (dropping the response body) must
/// reset the send stream with `H3_REQUEST_CANCELLED`, so the server observes a
/// stream error rather than a clean EOF for the truncated request.
#[tokio::test]
#[test_log::test]
async fn client_cancel_resets_stream() {
    let mut server = start_server();
    tokio::time::sleep(Duration::from_secs(1)).await;

    let (client_endpoint, mut client) = make_client!(server.listen_addr);
    let req = Request::builder()
        .method("POST")
        .uri(format!("https://{}/spawn", server.listen_addr))
        .body(OneFrameThenPending { sent: false })
        .unwrap();

    // The server responds immediately, so `send` returns a response while the
    // request upload is still open.
    let resp = tokio::time::timeout(Duration::from_secs(5), client.send(req))
        .await
        .expect("send timed out")
        .expect("send failed");

    // Let the server begin reading the request body.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Dropping the response drops the cancellation sender, cancelling the
    // in-flight body send and resetting the request stream.
    drop(resp);

    let outcome = tokio::time::timeout(Duration::from_secs(5), server.outcomes.recv())
        .await
        .expect("timed out waiting for server read outcome")
        .expect("server outcome channel closed");

    match &outcome {
        ReadOutcome::Error(s) => {
            assert!(
                s.contains("H3_REQUEST_CANCELLED"),
                "expected request-cancelled reset code, got: {s}"
            );
        }
        ReadOutcome::CleanEof(n) => {
            panic!(
                "cancelled upload must reset the stream, but server saw a clean EOF after {n} frames"
            );
        }
    }

    client_endpoint.wait_idle().await;
    server.shutdown().await;
}

/// Control: a body that completes normally must NOT reset the stream — the
/// server sees a graceful end-of-stream (guards against spurious resets).
#[tokio::test]
#[test_log::test]
async fn normal_upload_completes_cleanly() {
    let mut server = start_server();
    tokio::time::sleep(Duration::from_secs(1)).await;

    let body = http_body_util::Full::new(Bytes::from_static(b"hello complete body"))
        .map_err(|_: std::convert::Infallible| -> h3_util::Error { unreachable!() })
        .boxed();
    let (client_endpoint, mut client) = make_client!(server.listen_addr);
    let req = Request::builder()
        .method("POST")
        .uri(format!("https://{}/read", server.listen_addr))
        .body(body)
        .unwrap();

    let resp = tokio::time::timeout(Duration::from_secs(5), client.send(req))
        .await
        .expect("send timed out")
        .expect("send failed");
    assert!(resp.status().is_success());

    let outcome = tokio::time::timeout(Duration::from_secs(5), server.outcomes.recv())
        .await
        .expect("timed out waiting for server read outcome")
        .expect("server outcome channel closed");

    match &outcome {
        ReadOutcome::CleanEof(n) => assert!(*n >= 1, "expected at least one data frame, got {n}"),
        ReadOutcome::Error(s) => {
            panic!("normal completion must not reset the stream, but server saw error: {s}")
        }
    }

    client_endpoint.wait_idle().await;
    server.shutdown().await;
}
