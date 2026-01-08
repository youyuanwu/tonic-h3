//! gm-quic integration tests

use std::time::Duration;

use serial_test::serial;
use tokio_util::sync::CancellationToken;
use tonic::transport::Uri;

/// Test gm-quic server with quinn client
#[tokio::test]
#[serial(gm_quic)]
async fn gm_quic_server_quinn_client_test() {
    crate::try_setup_tracing();

    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let token = CancellationToken::new();
    let (h_svr, listen_addr) = crate::gm_quic_util::run_test_gm_quic_server(addr, token.clone());
    tracing::debug!("listenaddr : {}", listen_addr);

    // send client request
    tokio::time::sleep(Duration::from_secs(1)).await;

    tracing::debug!("connecting quinn client to gm-quic server.");

    let uri: Uri = format!("https://{listen_addr}").parse().unwrap();

    let client_endpoint = crate::make_test_quinn_client_endpoint();
    // quinn client test
    {
        let cc = tonic_h3::quinn::H3QuinnConnector::new(
            uri.clone(),
            "localhost".to_string(),
            client_endpoint.clone(),
        );
        let channel = tonic_h3::H3Channel::new(cc, uri.clone());
        let mut client = crate::greeter_client::GreeterClient::new(channel);

        {
            let request = tonic::Request::new(crate::HelloRequest {
                name: "Tonic-GmQuic-Quinn".into(),
            });
            let response = client.say_hello(request).await.unwrap();

            tracing::debug!("RESPONSE={:?}", response);
        }
    }
    tracing::debug!("client wait idle");
    client_endpoint.wait_idle().await;

    token.cancel();
    h_svr.await.unwrap();
}

/// Test gm-quic server with gm-quic client
#[tokio::test]
#[serial(gm_quic)]
async fn gm_quic_server_gm_quic_client_test() {
    crate::try_setup_tracing();

    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let token = CancellationToken::new();
    let (h_svr, listen_addr) = crate::gm_quic_util::run_test_gm_quic_server(addr, token.clone());
    tracing::debug!("listenaddr : {}", listen_addr);

    // send client request
    tokio::time::sleep(Duration::from_secs(1)).await;

    tracing::debug!("connecting gm-quic client to gm-quic server.");

    let uri: Uri = format!("https://{listen_addr}").parse().unwrap();

    // gm-quic client test
    {
        let quic_client = crate::gm_quic_util::make_test_gm_quic_client();
        let server_addr = format!("localhost:{}", listen_addr.port());
        let connection = quic_client.connect(&server_addr).await.unwrap();
        let cc = tonic_h3::gm_quic::H3GmQuicConnector::new(
            uri.clone(),
            "localhost".to_string(),
            std::sync::Arc::new(connection),
        );
        let channel = tonic_h3::H3Channel::new(cc, uri.clone());
        let mut client = crate::greeter_client::GreeterClient::new(channel);

        {
            let request = tonic::Request::new(crate::HelloRequest {
                name: "Tonic-GmQuic-GmQuic".into(),
            });
            let response = client.say_hello(request).await.unwrap();

            tracing::debug!("RESPONSE={:?}", response);
        }
    }

    token.cancel();
    h_svr.await.unwrap();
}

/// Test quinn server with gm-quic client
#[tokio::test]
#[serial(gm_quic)]
async fn quinn_server_gm_quic_client_test() {
    crate::try_setup_tracing();

    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let token = CancellationToken::new();
    let (h_svr, listen_addr) = crate::run_test_quinn_hello_server(addr, token.clone());
    tracing::debug!("listenaddr : {}", listen_addr);

    // send client request
    tokio::time::sleep(Duration::from_secs(1)).await;

    tracing::debug!("connecting gm-quic client to quinn server.");

    let uri: Uri = format!("https://{listen_addr}").parse().unwrap();

    // gm-quic client test
    {
        let quic_client = crate::gm_quic_util::make_test_gm_quic_client();
        let server_addr = format!("localhost:{}", listen_addr.port());
        let connection = quic_client.connect(&server_addr).await.unwrap();
        let cc = tonic_h3::gm_quic::H3GmQuicConnector::new(
            uri.clone(),
            "localhost".to_string(),
            std::sync::Arc::new(connection),
        );
        let channel = tonic_h3::H3Channel::new(cc, uri.clone());
        let mut client = crate::greeter_client::GreeterClient::new(channel);

        {
            let request = tonic::Request::new(crate::HelloRequest {
                name: "Tonic-Quinn-GmQuic".into(),
            });
            let response = client.say_hello(request).await.unwrap();

            tracing::debug!("RESPONSE={:?}", response);
        }
    }

    token.cancel();
    h_svr.await.unwrap();
}
