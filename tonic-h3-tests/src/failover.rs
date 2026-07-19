//! Regression test for MF-1: quinn address failover must try every DNS-resolved
//! address before giving up, rather than aborting on the first asynchronous
//! handshake failure.
//!
//! This is a best-effort, environment-guarded full-stack test. The quinn connector
//! sources its address list from `dns_resolve` (there is no injection seam without
//! refactoring it, which is out of scope), so the only way to exercise the real
//! `H3QuinnConnector::connect` loop is via a host name that resolves to multiple
//! addresses. We bind the server to `127.0.0.1` only and connect via `localhost`.
//! When `localhost` resolves to a dual-stack set (e.g. `[::1, 127.0.0.1]`) with the
//! non-loopback-v4 address ordered first, the first handshake fails and the fix must
//! continue to `127.0.0.1`. If the environment resolves `localhost` to a single
//! address, the failover path cannot be exercised and the test self-skips (logs and
//! returns) instead of producing a flaky failure.

use std::sync::Arc;

use h3_util::quinn::h3_quinn::quinn;

#[tokio::test]
#[test_log::test]
async fn quinn_multi_address_failover() {
    let bind_addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let token = tokio_util::sync::CancellationToken::new();

    // Server listens on 127.0.0.1 only.
    let (h_svr, listen_addr) = crate::run_test_quinn_hello_server(bind_addr, token.clone());
    let port = listen_addr.port();

    // Give the server a moment to come up.
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // Resolve `localhost:<port>` to inspect what the connector's `dns_resolve` will see.
    let resolved: Vec<std::net::SocketAddr> = tokio::net::lookup_host(format!("localhost:{port}"))
        .await
        .expect("lookup_host localhost failed")
        .collect();
    tracing::info!("localhost:{port} resolved to: {resolved:?}");

    let exercises_failover = resolved.len() >= 2
        && resolved
            .iter()
            .any(|a| a.ip() != std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));

    if !exercises_failover {
        tracing::warn!(
            "skipping MF-1 failover assertion: `localhost` resolved to {resolved:?}, \
             which cannot exercise a failing-first address on this host"
        );
        // Clean up server and pass — the environment cannot exercise failover.
        token.cancel();
        h_svr.await.unwrap();
        return;
    }

    // Build a client endpoint with a short idle timeout so a failing-first address
    // (e.g. `::1` where nothing listens) fails fast instead of stalling the test.
    let client_endpoint = make_short_idle_quinn_client_endpoint();
    let uri: http::Uri = format!("https://localhost:{port}").parse().unwrap();
    let cc = tonic_h3::quinn::H3QuinnConnector::new(
        uri.clone(),
        "localhost".to_string(),
        client_endpoint.clone(),
    );
    let channel = tonic_h3::H3Channel::new(cc, uri, None);
    let mut client = crate::greeter_client::GreeterClient::new(channel);

    // A single call must succeed by failing over to the healthy 127.0.0.1 address.
    let request = tonic::Request::new(crate::HelloRequest {
        name: "Failover".into(),
    });
    let response = client
        .say_hello(request)
        .await
        .expect("expected failover to a healthy address to succeed");
    tracing::debug!("failover RESPONSE={response:?}");

    // Cleanup.
    client_endpoint.close(0_u16.into(), b"client close");
    token.cancel();
    h_svr.await.unwrap();
}

/// A dual-stack quinn client endpoint (skips cert verification for tests) with a short
/// idle timeout, so a handshake to a dead address fails promptly.
fn make_short_idle_quinn_client_endpoint() -> quinn::Endpoint {
    let tls_config = crate::make_danger_rustls_client_config();
    let mut client_endpoint = quinn::Endpoint::client("[::]:0".parse().unwrap()).unwrap();
    let mut client_config = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(tls_config).unwrap(),
    ));
    let mut transport = quinn::TransportConfig::default();
    transport.max_idle_timeout(Some(std::time::Duration::from_secs(2).try_into().unwrap()));
    client_config.transport_config(Arc::new(transport));
    client_endpoint.set_default_client_config(client_config);
    client_endpoint
}
