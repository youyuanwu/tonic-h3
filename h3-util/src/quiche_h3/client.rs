use hyper::Uri;
use hyper::body::Bytes;

use crate::client::H3Connector;

// The local bridge crate's connector shares our wrapper's name, so alias it.
use quiche_h3::H3QuicheClientConfig;
use quiche_h3::H3QuicheConnector as InnerConnector;

/// [`H3Connector`] backed by the local `quiche-h3` bridge crate.
///
/// Mirrors the quinn/s2n connectors: it owns the target [`Uri`], the server
/// name used for TLS/SNI, and a [`H3QuicheClientConfig`]. DNS resolution is
/// performed on each `connect()` because the underlying
/// `quiche_h3::H3QuicheConnector` is constructed from a resolved
/// [`std::net::SocketAddr`].
#[derive(Clone)]
pub struct H3QuicheConnector {
    uri: Uri,
    server_name: String,
    config: H3QuicheClientConfig,
}

impl H3QuicheConnector {
    pub fn new(uri: Uri, server_name: String, config: H3QuicheClientConfig) -> Self {
        Self {
            uri,
            server_name,
            config,
        }
    }
}

impl H3Connector for H3QuicheConnector {
    type CONN = quiche_h3::Connection<Bytes>;
    type OS = quiche_h3::StreamOpener<Bytes>;
    type SS = quiche_h3::H3SendStream<Bytes>;
    type RS = quiche_h3::H3RecvStream<Bytes>;
    type BS = quiche_h3::H3Stream<Bytes>;

    async fn connect(&self) -> Result<Self::CONN, crate::Error> {
        // connect to dns resolved addr.
        let mut conn_err = std::io::Error::from(std::io::ErrorKind::AddrNotAvailable).into();
        let addrs = crate::client::dns_resolve(&self.uri).await?;
        tracing::trace!("connecting to server: {:?}", addrs);
        for addr in addrs {
            let connector =
                match InnerConnector::new(addr, self.server_name.clone(), self.config.clone()) {
                    Ok(c) => c,
                    Err(e) => {
                        conn_err = e;
                        continue;
                    }
                };
            match connector.connect().await {
                Ok(conn) => return Ok(conn),
                Err(e) => conn_err = e,
            }
        }
        Err(conn_err)
    }
}
