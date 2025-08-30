use std::net::SocketAddr;

use h3_util::quiche_h3::tokio_quiche::{
    ConnectionParams, listen,
    metrics::DefaultMetrics,
    quic::SimpleConnectionIdGenerator,
    settings::{CertificateKind, Hooks, QuicSettings, TlsCertificatePaths},
};
use tokio::net::UdpSocket;

#[tokio::test]
async fn test_quiche_h3() {
    // Test implementation goes here

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

    let socket = UdpSocket::bind(addr)
        .await
        .expect("UDP socket should be bindable");
    let mut _listeners = listen(
        [socket],
        ConnectionParams::new_server(
            QuicSettings::default(),
            TlsCertificatePaths {
                cert: todo!(),
                private_key: todo!(),
                kind: CertificateKind::X509,
            },
            Hooks::default(),
        ),
        SimpleConnectionIdGenerator,
        DefaultMetrics,
    )
    .expect("should be able to create a listener from a UDP socket");
}
