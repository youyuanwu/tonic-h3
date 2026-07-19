//! Regression tests for SF-1: `RequestSender::call` must never panic on
//! boundary-shaped URIs. The client constructors accept any `http::Uri`, whose
//! `scheme`/`authority` are `Option`s, and the per-request URI's `path_and_query`
//! is likewise optional. Previously `call` unwrapped all of these (plus the rebuilt
//! URI), so a base URI missing scheme/authority, or a request URI missing a path,
//! panicked deep inside the async `tower::Service::call` task.
//!
//! The fix precomputes/validates the base scheme+authority once in
//! `RequestSender::new` (without panicking) and surfaces any missing/invalid
//! component as a per-request error future, and defaults a request URI without a
//! `path_and_query` to origin-form "/".
//!
//! These tests drive the raw `H3Connection` + `H3Client` path (as in
//! `buffered_reconnect.rs` / `cancel_reset.rs`) so the test controls the base URI
//! and the per-request URI directly. The connector always targets the real running
//! server (so `poll_ready` reaches readiness and `call` runs the rewrite), while the
//! boundary-shaped URI is passed as the *base* URI to `H3Connection::new`.

use http::{Request, Uri};
use tokio_util::sync::CancellationToken;

type EmptyBody = http_body_util::Empty<hyper::body::Bytes>;

fn empty_body() -> EmptyBody {
    EmptyBody::new()
}

/// Build a raw non-buffered client whose connector targets `server_uri` (valid) but
/// whose base URI is `base_uri` (possibly boundary-shaped).
fn make_client(
    server_uri: &Uri,
    base_uri: Uri,
) -> h3_util::client::H3Client<tonic_h3::quinn::H3QuinnConnector, EmptyBody> {
    let client_endpoint = crate::make_test_quinn_client_endpoint();
    let cc = tonic_h3::quinn::H3QuinnConnector::new(
        server_uri.clone(),
        "localhost".to_string(),
        client_endpoint,
    );
    let conn = h3_util::client::H3Connection::new(cc, base_uri, None);
    h3_util::client::H3Client::new(conn)
}

/// SF-1: a base URI with an authority but no scheme (e.g. `localhost:PORT`) must yield
/// a clean per-request error rather than panicking at `scheme().unwrap()`.
#[tokio::test]
#[test_log::test]
#[serial_test::serial(uri_boundary)]
async fn quinn_base_uri_missing_scheme_no_panic() {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let token = CancellationToken::new();
    let (h_svr, listen_addr) = crate::run_test_quinn_hello_server(addr, token.clone());
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let server_uri: Uri = format!("https://localhost:{}", listen_addr.port())
        .parse()
        .unwrap();
    // Authority present, scheme absent.
    let base_uri: Uri = format!("localhost:{}", listen_addr.port()).parse().unwrap();
    assert!(
        base_uri.scheme().is_none(),
        "test setup: base must lack scheme"
    );
    assert!(
        base_uri.authority().is_some(),
        "test setup: base should carry an authority"
    );

    let mut client = make_client(&server_uri, base_uri);
    let req = Request::builder()
        .method("POST")
        .uri(&server_uri)
        .body(empty_body())
        .unwrap();
    let res = client.send(req).await;
    assert!(
        res.is_err(),
        "expected a clean error for a base URI missing a scheme, got Ok"
    );
    tracing::debug!("missing-scheme base URI error (expected): {:?}", res.err());

    token.cancel();
    h_svr.await.unwrap();
}

/// SF-1: a base URI with neither scheme nor authority (e.g. `/path`) must yield a clean
/// per-request error rather than panicking.
#[tokio::test]
#[test_log::test]
#[serial_test::serial(uri_boundary)]
async fn quinn_base_uri_missing_authority_no_panic() {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let token = CancellationToken::new();
    let (h_svr, listen_addr) = crate::run_test_quinn_hello_server(addr, token.clone());
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let server_uri: Uri = format!("https://localhost:{}", listen_addr.port())
        .parse()
        .unwrap();
    // Neither scheme nor authority.
    let base_uri: Uri = "/some/path".parse().unwrap();
    assert!(
        base_uri.authority().is_none(),
        "test setup: base must lack authority"
    );

    let mut client = make_client(&server_uri, base_uri);
    let req = Request::builder()
        .method("POST")
        .uri(&server_uri)
        .body(empty_body())
        .unwrap();
    let res = client.send(req).await;
    assert!(
        res.is_err(),
        "expected a clean error for a base URI missing an authority, got Ok"
    );
    tracing::debug!(
        "missing-authority base URI error (expected): {:?}",
        res.err()
    );

    token.cancel();
    h_svr.await.unwrap();
}

/// SF-1: a request URI with no `path_and_query` (authority-form target) must not panic;
/// the path defaults to origin-form "/", so the request completes at the transport level.
#[tokio::test]
#[test_log::test]
#[serial_test::serial(uri_boundary)]
async fn quinn_request_uri_missing_path_defaults_to_root() {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let token = CancellationToken::new();
    let (h_svr, listen_addr) = crate::run_test_quinn_hello_server(addr, token.clone());
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let server_uri: Uri = format!("https://localhost:{}", listen_addr.port())
        .parse()
        .unwrap();
    // Valid absolute base URI.
    let mut client = make_client(&server_uri, server_uri.clone());

    // Authority-form request target has no path_and_query.
    let req_uri: Uri = format!("localhost:{}", listen_addr.port()).parse().unwrap();
    assert!(
        req_uri.path_and_query().is_none(),
        "test setup: request URI must lack path_and_query"
    );
    let req = Request::builder()
        .method("POST")
        .uri(req_uri)
        .body(empty_body())
        .unwrap();
    let res = client.send(req).await;
    assert!(
        res.is_ok(),
        "expected the request to succeed with a defaulted '/' path, got {:?}",
        res.err()
    );

    token.cancel();
    h_svr.await.unwrap();
}
