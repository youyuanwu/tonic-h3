use std::{sync::Arc, time::Duration};

use axum::{Router, body::Bytes, routing::get};
use futures::future::poll_fn;
use h3_util::quinn::{H3QuinnAcceptor, h3_quinn};
use hyper::body::Buf;
use rcgen::generate_simple_self_signed;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use tokio_util::sync::CancellationToken;

fn server_endpoint() -> (quinn::Endpoint, CertificateDer<'static>) {
    let certified = generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let certificate = CertificateDer::from(certified.cert);
    let key = PrivateKeyDer::from_pem(
        rustls::pki_types::pem::SectionKind::PrivateKey,
        certified.signing_key.serialize_der(),
    )
    .unwrap();
    let mut tls = rustls::ServerConfig::builder_with_provider(
        rustls::crypto::aws_lc_rs::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![certificate.clone()], key)
    .unwrap();
    tls.alpn_protocols = vec![b"h3".to_vec()];
    let config = quinn::ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(tls).unwrap(),
    ));
    (
        quinn::Endpoint::server(config, "127.0.0.1:0".parse().unwrap()).unwrap(),
        certificate,
    )
}

fn client_endpoint(certificate: CertificateDer<'static>) -> quinn::Endpoint {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(certificate).unwrap();
    let mut tls = rustls::ClientConfig::builder_with_provider(
        rustls::crypto::aws_lc_rs::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    tls.alpn_protocols = vec![b"h3".to_vec()];
    let mut endpoint = quinn::Endpoint::client("[::]:0".parse().unwrap()).unwrap();
    endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(tls).unwrap(),
    )));
    endpoint
}

fn transport_close(code: quinn::TransportErrorCode) -> h3::error::ConnectionError {
    let close = quinn::ConnectionError::ConnectionClosed(quinn::ConnectionClose {
        error_code: code,
        frame_type: None,
        reason: Bytes::new(),
    });
    h3::error::ConnectionError::Remote(h3::quic::ConnectionErrorIncoming::Undefined(Arc::new(
        close,
    )))
}

#[test]
fn only_transport_no_error_is_benign() {
    assert!(h3_util::server::is_benign_connection_close(
        &transport_close(quinn::TransportErrorCode::NO_ERROR)
    ));
    assert!(!h3_util::server::is_benign_connection_close(
        &transport_close(quinn::TransportErrorCode::INTERNAL_ERROR)
    ));
}

#[tokio::test]
async fn ignored_request_body_is_stopped_without_error() {
    let (endpoint, certificate) = server_endpoint();
    let listen_addr = endpoint.local_addr().unwrap();
    let acceptor = H3QuinnAcceptor::new(endpoint.clone());
    let token = CancellationToken::new();
    let shutdown = token.clone();
    let server = tokio::spawn(async move {
        let app = Router::new().route("/", get(|| async { "Hello, HTTP/3!" }));
        axum_h3::H3Router::new(app)
            .serve_with_shutdown(acceptor, async move { shutdown.cancelled().await })
            .await
            .unwrap();
    });

    let client_endpoint = client_endpoint(certificate);
    let connection = client_endpoint
        .connect(listen_addr, "localhost")
        .unwrap()
        .await
        .unwrap();
    let (mut driver, mut sender) = h3::client::new(h3_quinn::Connection::new(connection))
        .await
        .unwrap();
    let driver = tokio::spawn(async move { poll_fn(|cx| driver.poll_close(cx)).await });

    let uri = format!("https://{listen_addr}/");
    let request = hyper::Request::get(uri).body(()).unwrap();
    let (mut request_body, mut response) = sender.send_request(request).await.unwrap().split();
    assert_eq!(
        response.recv_response().await.unwrap().status(),
        hyper::StatusCode::OK
    );

    let mut body = Vec::new();
    while let Some(mut data) = response.recv_data().await.unwrap() {
        body.extend_from_slice(&data.copy_to_bytes(data.remaining()));
    }
    assert_eq!(body, b"Hello, HTTP/3!");

    let stop = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            match request_body
                .send_data(Bytes::from_static(b"still sending"))
                .await
            {
                Ok(()) => tokio::task::yield_now().await,
                Err(error) => break error,
            }
        }
    })
    .await
    .expect("server did not stop the ignored request body");
    assert_eq!(stop.to_string(), "Remote reset: H3_NO_ERROR");

    drop(sender);
    client_endpoint.close(0_u16.into(), b"test complete");
    token.cancel();
    server.await.unwrap();
    let _ = driver.await.unwrap();
    endpoint.close(0_u16.into(), b"test complete");
}
