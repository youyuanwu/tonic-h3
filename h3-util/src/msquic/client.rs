use std::sync::Arc;

use hyper::Uri;
use msquic_h3::Registration;
use msquic_h3::msquic::Configuration;

#[derive(Clone)]
pub struct H3MsQuicConnector {
    // Field order matters: `config` is declared before `reg` so that on drop the
    // Configuration is closed before the Registration (msquic requires
    // ConfigurationClose before RegistrationClose). Teardown is otherwise driven
    // by the owner via `Registration::shutdown` + `Registration::wait_idle`.
    config: Arc<Configuration>,
    reg: Arc<Registration>,
    uri: Uri,
}

impl H3MsQuicConnector {
    pub fn new(config: Arc<Configuration>, reg: Arc<Registration>, uri: Uri) -> Self {
        Self { config, reg, uri }
    }
}

impl crate::client::H3Connector for H3MsQuicConnector {
    type CONN = msquic_h3::Connection;

    type OS = msquic_h3::StreamOpener;

    type SS = msquic_h3::H3SendStream;

    type RS = msquic_h3::H3RecvStream;

    type BS = msquic_h3::H3Stream;

    async fn connect(&self) -> Result<Self::CONN, crate::Error> {
        let conn = msquic_h3::Connection::connect(
            &self.reg,
            &self.config,
            self.uri.host().unwrap(),
            self.uri.port_u16().unwrap(),
        )
        .await
        .map_err(crate::Error::from)?;
        tracing::trace!("client conn started to {}", self.uri);
        Ok(conn)
    }
}
