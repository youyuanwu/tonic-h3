use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();
    let addr: std::net::SocketAddr = "127.0.0.1:5047".parse().unwrap();

    let token = CancellationToken::new();
    let (h_svr, listen_addr) = tonic_h3_test::run_test_quinn_hello_server(addr, token.clone());
    tracing::debug!("listenaddr : {}", listen_addr);
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for event");
    token.cancel();
    h_svr.await.unwrap();
}
