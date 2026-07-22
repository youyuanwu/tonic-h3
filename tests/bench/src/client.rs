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
        other => Err(format!(
            "client transport `{other}` is not implemented yet (added in a later phase)"
        )
        .into()),
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
