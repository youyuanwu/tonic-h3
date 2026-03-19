use std::sync::Arc;

use h3_util::quinn::h3_quinn::quinn;

/// Test that a TLS certificate error returns a proper error instead of panicking.
///
/// Regression test for: `async fn` resumed after completion panic in client_conn.rs
/// when the QUIC handshake fails (e.g. CaUsedAsEndEntity or untrusted cert).
/// The fix ensures `make_send_request_fut` is cleared on error so that
/// subsequent `poll_ready` calls don't poll a completed future.
#[tokio::test]
#[test_log::test]
async fn quinn_cert_error_no_panic() {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let token = tokio_util::sync::CancellationToken::new();

    // Start server with a self-signed cert.
    let (h_svr, listen_addr) = crate::run_test_quinn_hello_server(addr, token.clone());

    // Build a client that does NOT skip cert verification.
    // The server's self-signed cert will be rejected by rustls.
    let strict_client_endpoint = make_strict_quinn_client_endpoint();
    let uri: http::Uri = format!("https://localhost:{}", listen_addr.port())
        .parse()
        .unwrap();
    let cc = tonic_h3::quinn::H3QuinnConnector::new(
        uri.clone(),
        "localhost".to_string(),
        strict_client_endpoint.clone(),
    );
    let channel = tonic_h3::H3Channel::new(cc, uri);
    let mut client = crate::greeter_client::GreeterClient::new(channel);

    // First call should fail with a TLS error, not a panic.
    let request = tonic::Request::new(crate::HelloRequest {
        name: "Tonic".into(),
    });
    let result = client.say_hello(request).await;
    assert!(result.is_err(), "expected TLS cert error, got Ok");
    tracing::debug!("first call error (expected): {:?}", result.unwrap_err());

    // Second call should also return an error, not panic with
    // "`async fn` resumed after completion".
    let request2 = tonic::Request::new(crate::HelloRequest {
        name: "Tonic2".into(),
    });
    let result2 = client.say_hello(request2).await;
    assert!(result2.is_err(), "expected TLS cert error on retry, got Ok");
    tracing::debug!("second call error (expected): {:?}", result2.unwrap_err());

    // Cleanup
    strict_client_endpoint.close(0_u16.into(), b"client close");
    token.cancel();
    h_svr.await.unwrap();
}

/// Create a quinn client endpoint with default (strict) certificate verification.
/// This will reject self-signed server certs, which is exactly what we want for this test.
fn make_strict_quinn_client_endpoint() -> quinn::Endpoint {
    let provider = rustls::crypto::aws_lc_rs::default_provider();
    let mut tls_config = rustls::ClientConfig::builder_with_provider(provider.into())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    tls_config.alpn_protocols = vec![b"h3".to_vec()];

    let mut client_endpoint = quinn::Endpoint::client("[::]:0".parse().unwrap()).unwrap();
    let mut client_config = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(tls_config).unwrap(),
    ));
    // Use a short idle timeout so the handshake fails fast instead of retrying for 30s.
    let mut transport = quinn::TransportConfig::default();
    transport.max_idle_timeout(Some(std::time::Duration::from_secs(2).try_into().unwrap()));
    client_config.transport_config(Arc::new(transport));
    client_endpoint.set_default_client_config(client_config);
    client_endpoint
}
