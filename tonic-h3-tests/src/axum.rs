use std::{sync::Arc, time::Duration};

use axum::{body::Bytes, routing::get};
use futures::future::poll_fn;
use h3_util::quinn::h3_quinn;
use http::{Request, Uri};
use hyper::body::Buf;
use tokio_util::sync::CancellationToken;

async fn root() -> &'static str {
    "Hello, World from axum!"
}

fn start_axum_server() -> (
    quinn::Endpoint,
    std::net::SocketAddr,
    CancellationToken,
    tokio::task::JoinHandle<()>,
) {
    let endpoint = crate::make_quinn_server_endpoint("127.0.0.1:0".parse().unwrap());
    let listen_addr = endpoint.local_addr().unwrap();
    let acceptor = h3_util::quinn::H3QuinnAcceptor::new(endpoint.clone());
    let app = axum::Router::new().route("/", get(root));
    let token = CancellationToken::new();
    let shutdown = token.clone();
    let server = tokio::spawn(async move {
        axum_h3::H3Router::new(app)
            .serve_with_shutdown(acceptor, async move { shutdown.cancelled().await })
            .await
            .unwrap();
    });
    (endpoint, listen_addr, token, server)
}

#[tokio::test]
#[test_log::test]
async fn axum_test() {
    let (endpoint, listen_addr, token, server) = start_axum_server();
    let uri: Uri = format!("https://{listen_addr}").parse().unwrap();

    let client_endpoint = crate::make_test_quinn_client_endpoint();
    // quinn client test
    {
        // client drop is required to end connection. drive will end after connection end
        let cc = h3_util::quinn::H3QuinnConnector::new(
            uri.clone(),
            "localhost".to_string(),
            client_endpoint.clone(),
        );
        let channel = h3_util::client::H3Connection::new(cc, uri.clone(), None);
        let mut client = h3_util::client::H3Client::new(channel);
        let req = Request::builder()
            .method("GET")
            .uri(uri)
            .body(http_body_util::Empty::<Bytes>::new())
            .unwrap();
        let resp = client.send(req).await.unwrap();
        use http_body_util::BodyExt;
        let data = resp.into_body().collect().await.unwrap().to_bytes();
        tracing::debug!("Resp: {data:?}");
    }

    token.cancel();
    server.await.unwrap();
    endpoint.close(0_u16.into(), b"test complete");
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
#[test_log::test]
async fn ignored_request_body_is_stopped_without_error() {
    let (endpoint, listen_addr, token, server) = start_axum_server();

    let client_endpoint = crate::make_test_quinn_client_endpoint();
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
    let request = Request::get(uri).body(()).unwrap();
    let (mut request_body, mut response) = sender.send_request(request).await.unwrap().split();
    assert_eq!(
        response.recv_response().await.unwrap().status(),
        http::StatusCode::OK
    );

    let mut body = Vec::new();
    while let Some(mut data) = response.recv_data().await.unwrap() {
        body.extend_from_slice(&data.copy_to_bytes(data.remaining()));
    }
    assert_eq!(body, b"Hello, World from axum!");

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

#[tokio::test]
#[ignore = "requires external server"]
async fn h2o_client_test() {
    // cloudflare does not work:
    // let uri =  http::Uri::from_static("https://cloudflare-quic.com:443/");
    // This works:
    // let uri = http::Uri::from_static("https://quic.tech:8443/");
    let uri = Uri::from_static("https://h2o.examp1e.net:443");
    test_client(uri).await;
}

#[tokio::test]
#[ignore = "requires external server"]
async fn apache_client_test() {
    let uri = Uri::from_static("https://docs.trafficserver.apache.org:443/");
    test_client(uri).await;
}

/// Send a get request to the uri.
async fn test_client(uri: Uri) {
    let client_endpoint = crate::make_test_quinn_client_endpoint();
    // quinn client test
    {
        // client drop is required to end connection. drive will end after connection end
        let cc = h3_util::quinn::H3QuinnConnector::new(
            uri.clone(),
            uri.host().unwrap().to_string(),
            client_endpoint.clone(),
        );
        let channel = h3_util::client::H3Connection::new(cc, uri.clone(), None);
        let mut client = h3_util::client::H3Client::new(channel);
        let req = Request::builder()
            .method("GET")
            .uri(uri)
            .body(http_body_util::Empty::<Bytes>::new())
            .unwrap();
        let resp = client.send(req).await.unwrap();
        use http_body_util::BodyExt;
        let data = resp.into_body().collect().await.unwrap().to_bytes();
        tracing::debug!("Resp: {data:?}");
    }
}
