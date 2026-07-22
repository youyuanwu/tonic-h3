//! Transport-dispatching benchmark server.
//!
//! Each transport arm builds the appropriate acceptor/endpoint, serves the echo
//! service until the `shutdown` future resolves, then runs that backend's
//! documented teardown recipe so loopback runs release their port promptly
//! (avoiding QUIC idle-timeout hangs).

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use crate::{BenchError, EchoService, Transport, echo_routes, echo_server, tls};

/// Serve the echo service over `transport`, bound to `addr`, until `shutdown`.
pub async fn run_server<F>(
    transport: Transport,
    addr: SocketAddr,
    shutdown: F,
) -> Result<(), BenchError>
where
    F: Future<Output = ()> + Send + 'static,
{
    match transport {
        Transport::TcpTls => run_tcp_tls(addr, shutdown).await,
        Transport::Quinn => run_quinn(addr, shutdown).await,
        other => Err(format!(
            "server transport `{other}` is not implemented yet (added in a later phase)"
        )
        .into()),
    }
}

/// quinn (HTTP/3) server. Teardown: `close` + `wait_idle`.
async fn run_quinn<F>(addr: SocketAddr, shutdown: F) -> Result<(), BenchError>
where
    F: Future<Output = ()> + Send + 'static,
{
    use quinn::crypto::rustls::QuicServerConfig;

    let tls_config = Arc::new(tls::make_server_config(&[b"h3"]));
    let server_config =
        quinn::ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(tls_config)?));
    let endpoint = quinn::Endpoint::server(server_config, addr)?;
    let local = endpoint.local_addr()?;
    announce(Transport::Quinn, local);

    let acceptor = tonic_h3::quinn::H3QuinnAcceptor::new(endpoint.clone());
    tonic_h3::server::H3Router::new(echo_routes())
        .serve_with_shutdown(acceptor, shutdown)
        .await?;

    endpoint.close(0u16.into(), b"server shutdown");
    endpoint.wait_idle().await;
    Ok(())
}

/// TCP + TLS (HTTP/2) baseline server via `tonic-tls`.
async fn run_tcp_tls<F>(addr: SocketAddr, shutdown: F) -> Result<(), BenchError>
where
    F: Future<Output = ()> + Send + 'static,
{
    use tonic::transport::server::TcpIncoming;
    use tonic_tls::rustls::TlsIncoming;

    // gRPC over TCP negotiates HTTP/2 via ALPN "h2" (not "h3").
    let server_config = Arc::new(tls::make_server_config(&[b"h2"]));
    let tcp = TcpIncoming::bind(addr)?;
    let local = tcp.local_addr()?;
    announce(Transport::TcpTls, local);

    let incoming = TlsIncoming::new(tcp, server_config);
    tonic::transport::Server::builder()
        .add_service(echo_server::EchoServer::new(EchoService))
        .serve_with_incoming_shutdown(incoming, shutdown)
        .await?;
    Ok(())
}

/// Log/print the actual bound address (honors FR-007 for `:0` ephemeral binds).
pub(crate) fn announce(transport: Transport, local: SocketAddr) {
    tracing::info!("bench-server ({transport}) listening on {local}");
    println!("bench-server ({transport}) listening on {local}");
}
