use std::{net::SocketAddr, path::PathBuf};

use futures::StreamExt;
use h3_util::quiche_h3::tokio_quiche::{
    ConnectionParams, ServerH3Driver,
    http3::settings::Http3Settings,
    listen,
    metrics::DefaultMetrics,
    quic::SimpleConnectionIdGenerator,
    settings::{CertificateKind, Hooks, QuicSettings, TlsCertificatePaths},
};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

use crate::quiche::server::{Server, service_fn};

pub mod body;
pub mod server;

#[tokio::test]
async fn test_quiche_h3() {
    crate::try_setup_tracing();

    let (cert_path, key_path) = crate::cert_gen::make_test_cert_files("test_quiche_h3", true);
    // Test implementation goes here

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

    let socket = UdpSocket::bind(addr)
        .await
        .expect("UDP socket should be bindable");
    let local_addr = socket.local_addr().unwrap();

    let token = CancellationToken::new();

    let svh = tokio::spawn({
        let token = token.clone();
        async move {
            run_server(token, socket, cert_path, key_path).await;
        }
    });

    // Use quinn to send a request.
    quinn_client::run_quinn_client(local_addr).await;
    token.cancel();
    svh.await.unwrap();
}

mod quinn_client {
    use axum::body::Bytes;
    use http::{Request, Uri};

    pub async fn run_quinn_client(listen_addr: std::net::SocketAddr) {
        use super::body::STREAM_BYTES;
        let uri: Uri = format!("https://{listen_addr}{STREAM_BYTES}")
            .parse()
            .unwrap();

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
    }
}

pub async fn run_server(
    token: CancellationToken,
    socket: UdpSocket,
    cert_path: PathBuf,
    key_path: PathBuf,
) {
    let mut listeners = listen(
        [socket],
        ConnectionParams::new_server(
            QuicSettings::default(),
            TlsCertificatePaths {
                cert: cert_path.as_path().to_str().unwrap(),
                private_key: key_path.as_path().to_str().unwrap(),
                kind: CertificateKind::X509,
            },
            Hooks::default(),
        ),
        SimpleConnectionIdGenerator,
        DefaultMetrics,
    )
    .expect("should be able to create a listener from a UDP socket");

    // Pull connections off the socket and serve them.
    let accepted_connection_stream = &mut listeners[0];
    loop {
        tokio::select! {
            _ = token.cancelled() => {
              tracing::info!("cancelling");
              break;
            }
            conn = accepted_connection_stream.next() => {
              match conn{
                Some(Ok(conn)) => {
                  tracing::info!("received new connection!");

                  // Create an `H3Driver` to serve the connection.
                  let (driver, mut controller) = ServerH3Driver::new(Http3Settings::default());

                  conn.start(driver);
                                // Spawn a task to process the new connection.
                  tokio::spawn(async move {
                      let mut server = Server::new(service_fn);

                      // tokio-quiche will send events to the `H3Controller`'s
                      // receiver for processing.
                      let _ = server
                          .serve_connection(controller.event_receiver_mut())
                          .await;
                  });
                }
                Some(Err(e)) => {
                  tracing::error!("could not create connection: {e:?}");
                }
                None => {
                  tracing::info!("listener closed");
                  break;
                }
              }
          }
        }
    }
}
