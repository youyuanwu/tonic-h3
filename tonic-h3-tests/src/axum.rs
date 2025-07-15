use std::time::Duration;

use axum::{body::Bytes, routing::get};
use http::{Request, Uri};
use tokio_util::sync::CancellationToken;

async fn root() -> &'static str {
    "Hello, World from axum!"
}

#[tokio::test]
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
        let channel = h3_util::client::H3Connection::new(cc, uri.clone());
        let mut client = h3_util::client::H3Client::new(channel);
        let req = Request::builder()
            .method("GET")
            .uri(uri)
            .body(http_body_util::Empty::<Bytes>::new())
            .unwrap();
        let resp = client.send(req).await.unwrap();
        use http_body_util::BodyExt;
        let data = resp.into_body().collect().await.unwrap().to_bytes();
        println!("Resp: {data:?}");
    }

    token.cancel();
    svr_h.await.unwrap();
}

#[tokio::test]
async fn h2o_client_test() {
    // cloudflare does not work:
    // let uri =  http::Uri::from_static("https://cloudflare-quic.com:443/");
    // This works:
    // let uri = http::Uri::from_static("https://quic.tech:8443/");
    let uri = Uri::from_static("https://h2o.examp1e.net:443");
    test_client(uri).await;
}

#[tokio::test]
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
        let channel = h3_util::client::H3Connection::new(cc, uri.clone());
        let mut client = h3_util::client::H3Client::new(channel);
        let req = Request::builder()
            .method("GET")
            .uri(uri)
            .body(http_body_util::Empty::<Bytes>::new())
            .unwrap();
        let resp = client.send(req).await.unwrap();
        use http_body_util::BodyExt;
        let data = resp.into_body().collect().await.unwrap().to_bytes();
        println!("Resp: {data:?}");
    }
}
