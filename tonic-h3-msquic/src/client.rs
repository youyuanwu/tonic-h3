use std::sync::Arc;

use http::Uri;
use msquic_h3::msquic::{Configuration, Registration};

#[derive(Clone)]
pub struct H3MsQuicConnector {
    config: Arc<Configuration>,
    reg: Arc<Registration>,
    uri: Uri,
}

impl H3MsQuicConnector {
    pub fn new(config: Arc<Configuration>, reg: Arc<Registration>, uri: Uri) -> Self {
        Self { config, reg, uri }
    }
}

impl h3_util::client::H3Connector for H3MsQuicConnector {
    type CONN = msquic_h3::Connection;

    type OS = msquic_h3::StreamOpener;

    type SS = msquic_h3::H3SendStream;

    type RS = msquic_h3::H3RecvStream;

    type OE = msquic_h3::H3Error;

    type BS = msquic_h3::H3Stream;

    async fn connect(&self) -> Result<Self::CONN, tonic_h3::Error> {
        let conn = msquic_h3::Connection::connect(
            &self.reg,
            &self.config,
            self.uri.host().unwrap(),
            self.uri.port_u16().unwrap(),
        )
        .await
        .map_err(tonic_h3::Error::from)?;
        tracing::debug!("client conn start");
        Ok(conn)
    }
}
