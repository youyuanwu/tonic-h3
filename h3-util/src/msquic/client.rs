use std::sync::Arc;

use hyper::Uri;
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

impl crate::client::H3Connector for H3MsQuicConnector {
    type CONN = msquic_h3::Connection;

    type OS = msquic_h3::StreamOpener;

    type SS = msquic_h3::H3SendStream;

    type RS = msquic_h3::H3RecvStream;

    type BS = msquic_h3::H3Stream;

    async fn connect(&self) -> Result<Self::CONN, crate::Error> {
        // Maybe conn should hold a arc to reg. so that we can track how many connections are using it.
        let conn = msquic_h3::Connection::connect(
            self.reg.as_ref().unwrap().clone(),
            self.config.as_ref().unwrap(),
            self.uri.host().unwrap(),
            self.uri.port_u16().unwrap(),
        )
        .await
        .map_err(crate::Error::from)?;
        tracing::debug!("client conn start");

        Ok(conn)
    }
}

impl Drop for H3MsQuicConnector {
    fn drop(&mut self) {
        // config needs to drop before reg.
        std::mem::drop(self.config.take());
        // this drop maybe blocking since some connections are not finished.
        let reg = self.reg.take();
        if let Some(reg) = reg {
            // reg should not be dropped here.
            // user of the connector needs to keep a ref.
            let c = Arc::strong_count(&reg);
            assert_ne!(c, 1); // This may cause panic unwind but reg drop will be stuck.
        }
    }
}
