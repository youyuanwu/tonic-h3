use std::sync::Arc;

use gm_quic::prelude::Connection;
use hyper::Uri;
use hyper::body::Bytes;

use crate::client::H3Connector;

/// Connector for gm-quic based h3 connections.
///
/// This connector wraps a gm-quic `Connection` and implements the `H3Connector` trait
/// to provide h3 connection capabilities.
#[derive(Clone)]
pub struct H3GmQuicConnector {
    uri: Uri,
    server_name: String,
    connection: Arc<Connection>,
}

impl H3GmQuicConnector {
    /// Create a new gm-quic connector from an existing QUIC connection.
    ///
    /// # Arguments
    /// * `uri` - The URI to connect to
    /// * `server_name` - The server name for TLS
    /// * `connection` - The underlying gm-quic connection
    pub fn new(uri: Uri, server_name: String, connection: Arc<Connection>) -> Self {
        Self {
            uri,
            server_name,
            connection,
        }
    }

    /// Get the URI this connector is configured for
    pub fn uri(&self) -> &Uri {
        &self.uri
    }

    /// Get the server name
    pub fn server_name(&self) -> &str {
        &self.server_name
    }
}

impl H3Connector for H3GmQuicConnector {
    type CONN = h3_shim::QuicConnection;
    type OS = h3_shim::conn::OpenStreams;
    type SS = h3_shim::streams::SendStream<Bytes>;
    type RS = h3_shim::streams::RecvStream;
    type BS = h3_shim::streams::BidiStream<Bytes>;

    async fn connect(&self) -> Result<Self::CONN, crate::Error> {
        tracing::debug!(uri = %self.uri, server_name = %self.server_name, "connecting to gm-quic server");
        // Create the h3-shim QuicConnection wrapper
        let conn = h3_shim::QuicConnection::new(self.connection.clone());
        tracing::debug!("gm-quic connection established");
        Ok(conn)
    }
}
