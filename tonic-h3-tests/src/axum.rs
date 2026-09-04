use std::time::Duration;

use axum::{body::Bytes, routing::get};
use futures::future::poll_fn;
use http::{Request, Uri};
use hyper::body::Buf;
use tokio_util::sync::CancellationToken;

async fn root() -> &'static str {
    "Hello, World from axum!"
}

#[tokio::test]
#[test_log::test]
async fn axum_test() {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();

    let ep = crate::make_quinn_server_endpoint(addr);
    let listen_addr = ep.local_addr().unwrap();

    let acceptor = h3_util::quinn::H3QuinnAcceptor::new(ep);

    let app = axum::Router::new().route("/", get(root));

    let token = CancellationToken::new();
    let token_cp = token.clone();

    let svr_h = tokio::spawn(async move {
        axum_h3::H3Router::new(app)
            .serve_with_shutdown(acceptor, async move { token_cp.cancelled().await })
            .await
            .unwrap();
    });

    tokio::time::sleep(Duration::from_secs(1)).await;

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

    #[tokio::test]
    #[test_log::test]
    async fn ignored_request_body_is_stopped_without_error() {
        let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
        let endpoint = crate::make_quinn_server_endpoint(addr);
        let listen_addr = endpoint.local_addr().unwrap();
        let acceptor = h3_util::quinn::H3QuinnAcceptor::new(endpoint.clone());
        let app = axum::Router::new().route("/", get(root));
        let token = CancellationToken::new();
        let token_cp = token.clone();
        let server = tokio::spawn(async move {
            axum_h3::H3Router::new(app)
                .serve_with_shutdown(acceptor, async move { token_cp.cancelled().await })
                .await
                .unwrap();
        });

        let uri: Uri = format!("https://{listen_addr}").parse().unwrap();
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

        let request = Request::builder()
            .method("GET")
            .uri(uri)
            .body(())
            .unwrap();
        let stream = sender.send_request(request).await.unwrap();
        let (mut request_body, mut response) = stream.split();
        let response_head = response.recv_response().await.unwrap();
        assert_eq!(response_head.status(), http::StatusCode::OK);

        let mut response_body = Vec::new();
        while let Some(mut data) = response.recv_data().await.unwrap() {
            response_body.extend_from_slice(&data.copy_to_bytes(data.remaining()));
        }
        assert_eq!(response_body, b"Hello, World from axum!");

        let stop = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match request_body.send_data(Bytes::from_static(b"still sending")).await {
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
        driver.await.unwrap().unwrap_err();
        endpoint.close(0_u16.into(), b"test complete");
    }

    token.cancel();
    svr_h.await.unwrap();
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
