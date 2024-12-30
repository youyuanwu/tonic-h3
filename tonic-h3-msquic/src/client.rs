use http::Uri;
use msquic::{
    core::{config::QConfiguration, conn::QConnection, reg::QRegistration},
    msh3::H3Conn,
};

#[derive(Clone)]
pub struct H3MsQuicConnector {
    config: QConfiguration,
    reg: QRegistration,
    uri: Uri,
}

impl H3MsQuicConnector {
    pub fn new(config: QConfiguration, reg: QRegistration, uri: Uri) -> Self {
        Self { config, reg, uri }
    }
}

impl tonic_h3::H3Connector for H3MsQuicConnector {
    type CONN = msquic::msh3::H3Conn;

    type OS = msquic::msh3::H3OpenStream;

    type SS = msquic::msh3::H3Stream;

    type RS = msquic::msh3::H3Stream;

    type OE = msquic::msh3::H3Error;

    type BS = msquic::msh3::H3Stream;

    async fn connect(&self) -> Result<Self::CONN, tonic_h3::Error> {
        let mut conn = QConnection::open(&self.reg);
        tracing::debug!("client conn start");
        conn.start(
            &self.config,
            self.uri.host().unwrap(),
            self.uri.port_u16().unwrap(),
        )
        .await
        .map_err(tonic_h3::Error::from)?;
        Ok(H3Conn::new(conn))
    }
}
