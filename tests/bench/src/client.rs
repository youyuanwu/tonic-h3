//! Transport-dispatching benchmark client.
//!
//! Each arm builds the appropriate channel, wraps it in the generated
//! `EchoClient`, and runs the shared [`crate::load::drive_load`] loop.

use std::sync::Arc;

use http::Uri;

use crate::cli::ClientArgs;
use crate::echo_client::EchoClient;
use crate::load::drive_load;
use crate::metrics::BenchSummary;
use crate::{BenchError, Transport, tls};

/// Connect with the selected transport and drive the configured load.
pub async fn run_client(args: &ClientArgs) -> Result<BenchSummary, BenchError> {
    let uri: Uri = format!("https://{}", args.addr).parse()?;
    let cfg = args.load_config();

    match args.transport {
        Transport::TcpTls => {
            let channel = connect_tcp_tls(&args.addr).await?;
            let client = EchoClient::new(channel);
            let summary = drive_load(client, cfg).await?;
            Ok(summary)
        }
        Transport::Quinn => {
            let endpoint = make_quinn_client_endpoint()?;
            let connector = tonic_h3::quinn::H3QuinnConnector::new(
                uri.clone(),
                "localhost".to_string(),
                endpoint.clone(),
            );
            let channel = tonic_h3::H3Channel::new(connector, uri.clone(), None);
            let client = EchoClient::new(channel);
            let summary = drive_load(client, cfg).await?;
            // Client teardown: explicit close is preferred over wait_idle.
            endpoint.close(0u16.into(), b"client done");
            Ok(summary)
        }
        Transport::S2nQuic => {
            let s2n_ep = make_s2n_client_endpoint()?;
            let connector = h3_util::s2n::client::H3S2nConnector::new(
                uri.clone(),
                "localhost".to_string(),
                s2n_ep,
            );
            let channel = tonic_h3::H3Channel::new(connector, uri.clone(), None);
            let client = EchoClient::new(channel);
            let summary = drive_load(client, cfg).await?;
            // s2n has no close; the client process exits immediately after.
            Ok(summary)
        }
        Transport::Quiche => {
            use h3_util::quiche_h3::H3QuicheClientConfig;
            let connector = tonic_h3::quiche::H3QuicheConnector::new(
                uri.clone(),
                "localhost".to_string(),
                H3QuicheClientConfig {
                    verify_peer: false,
                    ..Default::default()
                },
            );
            let channel = tonic_h3::H3Channel::new(connector, uri.clone(), None);
            let client = EchoClient::new(channel);
            let summary = drive_load(client, cfg).await?;
            // The quiche connector holds no persistent endpoint; nothing to close.
            Ok(summary)
        }
        Transport::Msquic => {
            let (reg, config) = make_msquic_client_parts()?;
            let connector =
                h3_util::msquic::client::H3MsQuicConnector::new(config, reg.clone(), uri.clone());
            let channel = tonic_h3::H3Channel::new(connector, uri.clone(), None);
            let client = EchoClient::new(channel);
            let summary = drive_load(client, cfg).await?;
            // Teardown: shut down the registration, wait idle, then drop so the
            // final RegistrationClose does not block the runtime.
            reg.shutdown();
            reg.wait_idle().await;
            std::mem::drop(reg);
            Ok(summary)
        }
    }
}

/// Build a quinn client endpoint using the dangerous (no-verify) h3 config.
fn make_quinn_client_endpoint() -> Result<quinn::Endpoint, BenchError> {
    use quinn::crypto::rustls::QuicClientConfig;

    let tls_config = tls::make_danger_client_config(&[b"h3"]);
    let mut endpoint = quinn::Endpoint::client("[::]:0".parse()?)?;
    let client_config = quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(tls_config)?));
    endpoint.set_default_client_config(client_config);
    Ok(endpoint)
}

/// Build a tonic `Channel` over TCP+TLS (HTTP/2, ALPN "h2") via `tonic-tls`.
async fn connect_tcp_tls(addr: &str) -> Result<tonic::transport::Channel, BenchError> {
    use rustls::pki_types::ServerName;
    use tonic::transport::Endpoint;

    let endpoint = Endpoint::from_shared(format!("https://{addr}"))?;
    let dnsname = ServerName::try_from("localhost")?.to_owned();
    let client_config = Arc::new(tls::make_danger_client_config(&[b"h2"]));
    let transport = tonic_tls::TcpTransport::from_endpoint(&endpoint);
    let channel = endpoint
        .connect_with_connector(tonic_tls::rustls::TlsConnector::new(
            transport,
            client_config,
            dnsname,
        ))
        .await?;
    Ok(channel)
}

/// Build an s2n-quic client endpoint using the dangerous (no-verify) h3 config.
fn make_s2n_client_endpoint() -> Result<h3_util::s2n::s2n_quic::Client, BenchError> {
    use h3_util::s2n::s2n_quic;

    let tls =
        s2n_quic::provider::tls::rustls::Client::from(tls::make_danger_client_config(&[b"h3"]));
    let client = s2n_quic::Client::builder()
        .with_tls(tls)
        .map_err(|e| format!("s2n tls provider: {e}"))?
        .with_io("0.0.0.0:0")
        .map_err(|e| format!("s2n io: {e}"))?
        .start()
        .map_err(|e| format!("s2n start: {e}"))?;
    Ok(client)
}

/// Build msquic client registration + configuration (no-verify h3).
#[allow(clippy::type_complexity)]
fn make_msquic_client_parts() -> Result<
    (
        Arc<h3_util::msquic::msquic_h3::Registration>,
        Arc<h3_util::msquic::msquic_h3::msquic::Configuration>,
    ),
    BenchError,
> {
    use h3_util::msquic::msquic_h3::{self, msquic};

    let reg = msquic_h3::Registration::new(
        &msquic::RegistrationConfig::new().set_app_name("benchapp_client".to_string()),
    )
    .map_err(|e| format!("msquic registration: {e}"))?;

    let alpn = msquic::BufferRef::from("h3");
    let client_settings = msquic::Settings::new()
        .set_IdleTimeoutMs(1000)
        .set_PeerBidiStreamCount(10)
        .set_PeerUnidiStreamCount(10);
    let client_config = reg
        .open_configuration(&[alpn], Some(&client_settings))
        .map_err(|e| format!("msquic configuration: {e}"))?;
    let cred_config = msquic::CredentialConfig::new_client()
        .set_credential_flags(msquic::CredentialFlags::NO_CERTIFICATE_VALIDATION);
    client_config
        .load_credential(&cred_config)
        .map_err(|e| format!("msquic load_credential: {e}"))?;
    Ok((reg.into(), client_config.into()))
}
