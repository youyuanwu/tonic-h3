use std::{net::SocketAddr, time::Duration};

use serial_test::serial;
use tokio_util::sync::CancellationToken;
use tonic::transport::Uri;

#[tokio::test]
#[serial(gm_quic)]
async fn h3_quinn_test() {
    h3_test(crate::run_test_quinn_hello_server).await;
}

#[tokio::test]
#[serial(gm_quic)]
async fn h3_s2n_test() {
    h3_test(crate::run_test_s2n_server).await;
}

#[tokio::test]
async fn msquic_test() {
    // Use h3_test_without_gm_quic for msquic server because gm-quic client
    // has ALPN negotiation issues with msquic server (Crypto error 120).
    h3_test_without_gm_quic(crate::msquic_util::run_test_msquic_server).await;
}

#[tokio::test]
#[serial(gm_quic)]
async fn gm_quic_test() {
    // Use a separate test function for gm-quic server that skips msquic client.
    // The gm-quic server uses virtual hosting based on SNI, and msquic's API
    // uses the same host parameter for both DNS resolution and SNI, which causes
    // issues when trying to connect to an IP address with a different SNI.
    h3_test_without_msquic(crate::gm_quic_util::run_test_gm_quic_server).await;
}

// takes in the fn to start the server and then send request to the server.
#[allow(clippy::type_complexity)]
async fn h3_test(
    run_server: fn(SocketAddr, CancellationToken) -> (tokio::task::JoinHandle<()>, SocketAddr),
) {
    crate::try_setup_tracing();

    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let token = CancellationToken::new();
    let (h_svr, listen_addr) = run_server(addr, token.clone());
    tracing::debug!("listenaddr : {}", listen_addr);

    // send client request
    tokio::time::sleep(Duration::from_secs(1)).await;

    tracing::debug!("connecting quic client.");

    let uri: Uri = format!("https://{listen_addr}").parse().unwrap();

    let client_endpoint = crate::make_test_quinn_client_endpoint();
    // quinn client test
    {
        // client drop is required to end connection. drive will end after connection end
        let cc = tonic_h3::quinn::H3QuinnConnector::new(
            uri.clone(),
            "localhost".to_string(),
            client_endpoint.clone(),
        );
        let channel = tonic_h3::H3Channel::new(cc, uri.clone());
        let mut client = crate::greeter_client::GreeterClient::new(channel);

        {
            let request = tonic::Request::new(crate::HelloRequest {
                name: "Tonic".into(),
            });
            let response = client.say_hello(request).await.unwrap();

            tracing::debug!("RESPONSE={:?}", response);
        }
        {
            let request = tonic::Request::new(crate::HelloRequest {
                name: "Tonic2".into(),
            });
            let response = client.say_hello(request).await.unwrap();

            tracing::debug!("RESPONSE={:?}", response);
        }
    }
    tracing::debug!("client wait idle");
    client_endpoint.wait_idle().await;

    // test s2n client
    let mut s2n_ep = crate::make_test_s2n_client_endpoint();
    {
        let cc = tonic_h3::s2n::client::H3S2nConnector::new(
            uri.clone(),
            "localhost".to_string(),
            s2n_ep.clone(),
        );
        let channel = tonic_h3::H3Channel::new(cc, uri.clone());
        let mut client = crate::greeter_client::GreeterClient::new(channel);
        {
            let request = tonic::Request::new(crate::HelloRequest {
                name: "Tonic-S2n".into(),
            });
            let response = client.say_hello(request).await.unwrap();
            tracing::debug!("RESPONSE={:?}", response);
        }
    }
    s2n_ep.wait_idle().await.unwrap();

    // test msquic client
    // reg should be the last thing to drop, otherwise it will wait for other handle to drop and deadlock.
    let (reg, config) = crate::make_test_msquic_client_parts();
    let msquic_waiter = h3_util::msquic::client::H3MsQuicClientWaiter::default();
    {
        let channel = tonic_h3::H3Channel::new(
            h3_util::msquic::client::H3MsQuicConnector::new(
                config,
                reg.clone(),
                uri.clone(),
                msquic_waiter.clone(),
            ),
            uri.clone(),
        );
        let mut client = crate::greeter_client::GreeterClient::new(channel);
        {
            let request = tonic::Request::new(crate::HelloRequest {
                name: "Tonic-MsQuic".into(),
            });
            let response = client.say_hello(request).await.unwrap();
            tracing::debug!("RESPONSE={:?}", response);
        }
        // trigger reg shutdown and wait for connection to close.
        reg.shutdown();
        msquic_waiter.wait_shutdown().await;
    }

    // test gm-quic client
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
                name: "Tonic-GmQuic".into(),
            });
            let response = client.say_hello(request).await.unwrap();
            tracing::debug!("RESPONSE={:?}", response);
        }
    }

    token.cancel();
    h_svr.await.unwrap();
}

/// Test function without gm-quic client, for servers that have ALPN issues with gm-quic
/// (like msquic server which gets Crypto error 120 with gm-quic client).
#[allow(clippy::type_complexity)]
async fn h3_test_without_gm_quic(
    run_server: fn(SocketAddr, CancellationToken) -> (tokio::task::JoinHandle<()>, SocketAddr),
) {
    crate::try_setup_tracing();

    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let token = CancellationToken::new();
    let (h_svr, listen_addr) = run_server(addr, token.clone());
    tracing::debug!("listenaddr : {}", listen_addr);

    // send client request
    tokio::time::sleep(Duration::from_secs(1)).await;

    tracing::debug!("connecting quic client.");

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
                name: "Tonic".into(),
            });
            let response = client.say_hello(request).await.unwrap();

            tracing::debug!("RESPONSE={:?}", response);
        }
        {
            let request = tonic::Request::new(crate::HelloRequest {
                name: "Tonic2".into(),
            });
            let response = client.say_hello(request).await.unwrap();

            tracing::debug!("RESPONSE={:?}", response);
        }
    }
    tracing::debug!("client wait idle");
    client_endpoint.wait_idle().await;

    // test s2n client
    let mut s2n_ep = crate::make_test_s2n_client_endpoint();
    {
        let cc = tonic_h3::s2n::client::H3S2nConnector::new(
            uri.clone(),
            "localhost".to_string(),
            s2n_ep.clone(),
        );
        let channel = tonic_h3::H3Channel::new(cc, uri.clone());
        let mut client = crate::greeter_client::GreeterClient::new(channel);
        {
            let request = tonic::Request::new(crate::HelloRequest {
                name: "Tonic-S2n".into(),
            });
            let response = client.say_hello(request).await.unwrap();
            tracing::debug!("RESPONSE={:?}", response);
        }
    }
    s2n_ep.wait_idle().await.unwrap();

    // test msquic client
    // reg should be the last thing to drop, otherwise it will wait for other handle to drop and deadlock.
    let (reg, config) = crate::make_test_msquic_client_parts();
    let msquic_waiter = h3_util::msquic::client::H3MsQuicClientWaiter::default();
    {
        let channel = tonic_h3::H3Channel::new(
            h3_util::msquic::client::H3MsQuicConnector::new(
                config,
                reg.clone(),
                uri.clone(),
                msquic_waiter.clone(),
            ),
            uri.clone(),
        );
        let mut client = crate::greeter_client::GreeterClient::new(channel);
        {
            let request = tonic::Request::new(crate::HelloRequest {
                name: "Tonic-MsQuic".into(),
            });
            let response = client.say_hello(request).await.unwrap();
            tracing::debug!("RESPONSE={:?}", response);
        }
        // trigger reg shutdown and wait for connection to close.
        reg.shutdown();
        msquic_waiter.wait_shutdown().await;
    }

    // Skip gm-quic client test for msquic server due to ALPN negotiation issues
    tracing::debug!("Skipping gm-quic client test for msquic server (ALPN interop issue)");

    token.cancel();
    h_svr.await.unwrap();
}

/// Test function without msquic client, for servers that use virtual hosting
/// (like gm-quic) which don't work well with msquic's SNI handling.
#[allow(clippy::type_complexity)]
async fn h3_test_without_msquic(
    run_server: fn(SocketAddr, CancellationToken) -> (tokio::task::JoinHandle<()>, SocketAddr),
) {
    crate::try_setup_tracing();

    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let token = CancellationToken::new();
    let (h_svr, listen_addr) = run_server(addr, token.clone());
    tracing::debug!("listenaddr : {}", listen_addr);

    // send client request
    tokio::time::sleep(Duration::from_secs(1)).await;

    tracing::debug!("connecting quic client.");

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
                name: "Tonic".into(),
            });
            let response = client.say_hello(request).await.unwrap();

            tracing::debug!("RESPONSE={:?}", response);
        }
        {
            let request = tonic::Request::new(crate::HelloRequest {
                name: "Tonic2".into(),
            });
            let response = client.say_hello(request).await.unwrap();

            tracing::debug!("RESPONSE={:?}", response);
        }
    }
    tracing::debug!("client wait idle");
    client_endpoint.wait_idle().await;

    // test s2n client
    let mut s2n_ep = crate::make_test_s2n_client_endpoint();
    {
        let cc = tonic_h3::s2n::client::H3S2nConnector::new(
            uri.clone(),
            "localhost".to_string(),
            s2n_ep.clone(),
        );
        let channel = tonic_h3::H3Channel::new(cc, uri.clone());
        let mut client = crate::greeter_client::GreeterClient::new(channel);
        {
            let request = tonic::Request::new(crate::HelloRequest {
                name: "Tonic-S2n".into(),
            });
            let response = client.say_hello(request).await.unwrap();
            tracing::debug!("RESPONSE={:?}", response);
        }
    }
    s2n_ep.wait_idle().await.unwrap();

    // Skip msquic client test for virtual hosting servers like gm-quic.
    // The msquic API uses the same host for DNS resolution and SNI, which
    // doesn't work with gm-quic's virtual hosting when connecting to IP addresses.
    tracing::debug!("Skipping msquic client test for virtual hosting server");

    // test gm-quic client
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
                name: "Tonic-GmQuic".into(),
            });
            let response = client.say_hello(request).await.unwrap();
            tracing::debug!("RESPONSE={:?}", response);
        }
    }

    token.cancel();
    h_svr.await.unwrap();
}
