use std::sync::Arc;

use http::Uri;
use msquic_h3::msquic::{Configuration, Registration};

#[derive(Clone)]
pub struct H3MsQuicConnector {
    config: Option<Arc<Configuration>>,
    reg: Option<Arc<Registration>>,
    uri: Uri,
}

impl H3MsQuicConnector {
    pub fn new(config: Arc<Configuration>, reg: Arc<Registration>, uri: Uri) -> Self {
        Self {
            config: Some(config),
            reg: Some(reg),
            uri,
        }
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
            self.reg.as_ref().unwrap(),
            self.config.as_ref().unwrap(),
            self.uri.host().unwrap(),
            self.uri.port_u16().unwrap(),
        )
        .await
        .map_err(tonic_h3::Error::from)?;
        tracing::debug!("client conn start");
        Ok(conn)
    }
}

impl Drop for H3MsQuicConnector {
    fn drop(&mut self) {
        // config needs to drop before reg.
        self.config.take();
        // this drop maybe blocking since some connections are not finished.
        let reg = self.reg.take();
        // HACK: drop in another thread.
        // TODO: find another way. Or use tokio blocking task.
        std::thread::spawn(move || {
            std::mem::drop(reg);
        });
    }
}
