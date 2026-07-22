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
        Transport::S2nQuic => run_s2n(addr, shutdown).await,
        Transport::Quiche => run_quiche(addr, shutdown).await,
        Transport::Msquic => run_msquic(addr, shutdown).await,
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

/// s2n-quic (HTTP/3) server. s2n has no explicit close; the serve task simply
/// ends when `shutdown` resolves.
async fn run_s2n<F>(addr: SocketAddr, shutdown: F) -> Result<(), BenchError>
where
    F: Future<Output = ()> + Send + 'static,
{
    use h3_util::s2n::s2n_quic;

    let tls =
        s2n_quic::provider::tls::rustls::server::Server::from(tls::make_server_config(&[b"h3"]));
    let server = s2n_quic::Server::builder()
        .with_tls(tls)
        .map_err(|e| format!("s2n tls provider: {e}"))?
        .with_io(addr)
        .map_err(|e| format!("s2n io: {e}"))?
        .start()
        .map_err(|e| format!("s2n start: {e}"))?;
    let local = server.local_addr()?;
    announce(Transport::S2nQuic, local);

    let acceptor = h3_util::s2n::server::H3S2nAcceptor::new(server);
    tonic_h3::server::H3Router::new(echo_routes())
        .serve_with_shutdown(acceptor, shutdown)
        .await?;
    Ok(())
}

/// quiche (HTTP/3) server. Needs cert/key FILES. Teardown: `close` + `wait_idle`.
async fn run_quiche<F>(addr: SocketAddr, shutdown: F) -> Result<(), BenchError>
where
    F: Future<Output = ()> + Send + 'static,
{
    use h3_util::quiche_h3::H3QuicheServerConfig;

    let (cert_path, key_path) = tls::make_test_cert_files("bench_quiche", true);

    // Bind via std so we can read the real local addr before handing the socket
    // to the acceptor; `from_std` requires the ambient Tokio runtime.
    let std_socket = std::net::UdpSocket::bind(addr)?;
    let local = std_socket.local_addr()?;
    std_socket.set_nonblocking(true)?;
    let socket = tokio::net::UdpSocket::from_std(std_socket)?;
    announce(Transport::Quiche, local);

    let config = H3QuicheServerConfig {
        cert_path: cert_path.to_string_lossy().into_owned(),
        key_path: key_path.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let acceptor = tonic_h3::quiche::H3QuicheAcceptor::new(socket, &config)?;
    // Take a shutdown handle before the acceptor is moved into the serve task.
    let endpoint = acceptor.endpoint();
    tonic_h3::server::H3Router::new(echo_routes())
        .serve_with_shutdown(acceptor, shutdown)
        .await?;

    // Drain live connection workers so the UDP port is released promptly.
    endpoint.close(h3::error::Code::H3_NO_ERROR, b"server shutdown");
    endpoint.wait_idle().await;
    Ok(())
}

/// msquic (HTTP/3) server. Teardown recipe: `shutdown` -> `wait_idle` ->
/// drop config -> drop registration.
async fn run_msquic<F>(addr: SocketAddr, shutdown: F) -> Result<(), BenchError>
where
    F: Future<Output = ()> + Send + 'static,
{
    use h3_util::msquic::msquic_h3::{
        Listener, Registration,
        msquic::{
            self, BufferRef, CertificateFile, Credential, CredentialConfig, CredentialFlags,
            RegistrationConfig, Settings,
        },
    };
    use h3_util::msquic::server::H3MsQuicAcceptor;

    // Non-Windows: file-based self-signed credential.
    let (cert_path, key_path) = tls::make_test_cert_files("bench_msquic", false);
    let cred = Credential::CertificateFile(CertificateFile::new(
        key_path.display().to_string(),
        cert_path.display().to_string(),
    ));

    let alpn = [BufferRef::from("h3")];
    let settings = Settings::new()
        .set_PeerBidiStreamCount(10)
        .set_PeerUnidiStreamCount(10)
        .set_IdleTimeoutMs(1000);

    let reg = Registration::new(
        &RegistrationConfig::default().set_app_name("benchapp_server".to_string()),
    )
    .map_err(|e| format!("msquic registration: {e}"))?;
    let config = reg
        .open_configuration(&alpn, Some(&settings))
        .map_err(|e| format!("msquic configuration: {e}"))?;
    let cred_config = CredentialConfig::new()
        .set_credential_flags(CredentialFlags::NO_CERTIFICATE_VALIDATION)
        .set_credential(cred);
    config
        .load_credential(&cred_config)
        .map_err(|e| format!("msquic load_credential: {e}"))?;

    let config = Arc::new(config);
    // Retry on ADDRESS_IN_USE (port may still be draining from a prior run).
    let max_retry = 30;
    let mut i = 0;
    let listener = loop {
        match Listener::new(&reg, config.clone(), &alpn, Some(addr)) {
            Ok(l) => break l,
            Err(e) => {
                let in_use = e
                    .try_as_status_code()
                    .map(|c| c == msquic::StatusCode::QUIC_STATUS_ADDRESS_IN_USE)
                    .unwrap_or(false);
                if i < max_retry && in_use {
                    std::thread::yield_now();
                } else {
                    return Err(format!("msquic listener: {e}").into());
                }
            }
        }
        i += 1;
    };
    let local = listener
        .get_ref()
        .get_local_addr()
        .map_err(|e| format!("msquic local_addr: {e}"))?
        .as_socket()
        .ok_or("msquic local_addr not a socket addr")?;
    announce(Transport::Msquic, local);

    let acceptor = H3MsQuicAcceptor::new(listener);
    let acceptor_cp = acceptor.clone();
    tonic_h3::server::H3Router::new(echo_routes())
        .serve_with_shutdown(acceptor, shutdown)
        .await?;

    // Teardown: stop the listener, shut down all connections, wait idle, then
    // drop config and registration last (order matters — see msquic-h3 docs).
    acceptor_cp.shutdown().await;
    reg.shutdown();
    std::mem::drop(acceptor_cp);
    reg.wait_idle().await;
    std::mem::drop(config);
    std::mem::drop(reg);
    Ok(())
}

/// Log/print the actual bound address (honors FR-007 for `:0` ephemeral binds).
pub(crate) fn announce(transport: Transport, local: SocketAddr) {
    tracing::info!("bench-server ({transport}) listening on {local}");
    println!("bench-server ({transport}) listening on {local}");
}
