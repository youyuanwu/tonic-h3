use std::net::SocketAddr;

use h3_util::client::H3Connector;
use http::Uri;
use tokio_util::sync::CancellationToken;

#[tokio::test]
#[test_log::test]
#[serial_test::serial(reconnect)]
async fn h3_quinn_test() {
    reconnect_test(crate::run_test_quinn_hello_server, crate::run_quinn_client).await;
}

#[tokio::test]
#[test_log::test]
#[serial_test::serial(reconnect)]
#[ignore = "s2n does not support acceptor close"]
async fn h3_s2n_test() {
    reconnect_test(crate::run_test_s2n_server, crate::run_s2n_client).await;
}

#[tokio::test]
#[test_log::test]
#[serial_test::serial(reconnect)] // serial is used because sometimes msquic test can stuck.
async fn h3_msquic_test() {
    reconnect_test(
        crate::msquic_util::run_test_msquic_server,
        crate::msquic_util::run_msquic_client,
    )
    .await;
}

async fn reconnect_test<T: H3Connector>(
    run_server: fn(SocketAddr, CancellationToken) -> (tokio::task::JoinHandle<()>, SocketAddr),
    run_client: fn(Uri, CancellationToken) -> (tokio::task::JoinHandle<()>, T),
) {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let token = CancellationToken::new();
    let (h_svr, listen_addr) = run_server(addr, token.clone());
    tracing::debug!("listenaddr : {}", listen_addr);

    // send client request
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    tracing::debug!("connecting quic client.");

    let uri: Uri = format!("https://{listen_addr}").parse().unwrap();

    let client_token = CancellationToken::new();
    let (h_cli, cc) = run_client(uri.clone(), client_token.clone());
    let channel = tonic_h3::H3Channel::new(cc, uri);
    // Client drop is required to end connection. drive will end after connection end.
    // Or client endpoint needs to be closed.
    let mut client = crate::greeter_client::GreeterClient::new(channel);
    {
        let request = tonic::Request::new(crate::HelloRequest {
            name: "Tonic".into(),
        });
        let response = client.say_hello(request).await.unwrap();

        tracing::debug!("RESPONSE={:?}", response);
    }

    // stop and restart the server.
    token.cancel();
    h_svr.await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let token2 = CancellationToken::new();
    let (h_svr2, listen_addr2) = run_server(listen_addr, token2.clone());
    assert_eq!(listen_addr2.port(), listen_addr.port());

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    // check client can send again.
    {
        let request = tonic::Request::new(crate::HelloRequest {
            name: "Tonic2".into(),
        });
        let response = client.say_hello(request).await.unwrap();

        tracing::debug!("RESPONSE={:?}", response);
    }

    // close client
    client_token.cancel();
    // drop client so that reg can be cleaned up in task while loop
    drop(client);
    h_cli.await.unwrap();
    tracing::debug!("client finished.");
    // close server
    token2.cancel();
    h_svr2.await.unwrap();
}
