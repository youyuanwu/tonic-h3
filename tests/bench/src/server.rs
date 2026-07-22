//! Transport-dispatching benchmark server.
//!
//! Each transport arm builds the appropriate acceptor/endpoint, serves the echo
//! service until the `shutdown` future resolves, then runs that backend's
//! documented teardown recipe so loopback runs release their port promptly
//! (avoiding QUIC idle-timeout hangs).

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use crate::{BenchError, Transport, echo_routes, tls};

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

/// Log/print the actual bound address (honors FR-007 for `:0` ephemeral binds).
pub(crate) fn announce(transport: Transport, local: SocketAddr) {
    tracing::info!("bench-server ({transport}) listening on {local}");
    println!("bench-server ({transport}) listening on {local}");
}
