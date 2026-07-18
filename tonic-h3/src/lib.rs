//! gRPC over HTTP/3 for Rust.
//!
//! # Examples
//!
//! The examples below use the `quinn` backend (enable the `quinn` feature).
#![cfg_attr(
    feature = "quinn",
    doc = r#"
Server example:
```no_run
use tonic_h3::quinn::h3_quinn::quinn::Endpoint;

async fn run_server(endpoint: Endpoint) -> Result<(), tonic_h3::Error> {
    // Build your tonic services into `Routes`, e.g.
    // `Routes::builder().add_service(GreeterServer::new(svc)).routes()`.
    let routes = tonic::service::Routes::builder().routes();
    let acceptor = tonic_h3::quinn::H3QuinnAcceptor::new(endpoint.clone());
    tonic_h3::server::H3Router::new(routes)
        .serve(acceptor)
        .await?;
    endpoint.wait_idle().await;
    Ok(())
}
```
Client example:
```no_run
use tonic_h3::quinn::h3_quinn::quinn::Endpoint;

async fn run_client(
    uri: http::Uri,
    client_endpoint: Endpoint,
) -> Result<(), tonic_h3::Error> {
    let connector = tonic_h3::quinn::H3QuinnConnector::new(
        uri.clone(),
        "localhost".to_string(),
        client_endpoint,
    );
    let _channel = tonic_h3::H3Channel::new(connector, uri, None);
    // Pass `_channel` to your generated tonic client, e.g.
    // `GreeterClient::new(_channel)`, then call your RPCs.
    Ok(())
}
```
"#
)]

mod client;
pub mod server;
pub use {client::H3Channel, client::H3NonBufferedChannel};

pub type Error = Box<dyn std::error::Error + Send + Sync>;

// Reexport quinn implementation
#[cfg(feature = "quinn")]
pub mod quinn {
    pub use h3_util::quinn::*;
}

#[cfg(feature = "msquic")]
pub mod msquic {
    pub use h3_util::msquic::*;
}

#[cfg(feature = "s2n-quic")]
pub mod s2n {
    pub use h3_util::s2n::*;
}

#[cfg(feature = "quiche")]
pub mod quiche {
    pub use h3_util::quiche_h3::*;
}

pub use h3_util::executor::SharedExec;
