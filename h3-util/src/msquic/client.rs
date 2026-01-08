use std::sync::Arc;

use hyper::Uri;
use msquic_h3::msquic::{Configuration, Registration};

/// Wait for connection to finish in the connector.
/// Must be called before dropping the connector, otherwise there might be deadlock
/// when closing registration while connection is still alive. msquic will use mutex and block
/// rust runtime.
#[derive(Clone, Default)]
pub struct H3MsQuicClientWaiter {
    waiter: Arc<std::sync::Mutex<Option<msquic_h3::ConnectionShutdownWaiter>>>,
}

impl H3MsQuicClientWaiter {
    /// If not connection it returns immediately..
    pub async fn wait_shutdown(&self) {
        let w = self.waiter.lock().unwrap().take();
        if let Some(w) = w {
            w.wait().await;
        }
    }

    fn replace(&self, w: msquic_h3::ConnectionShutdownWaiter) {
        let mut waiter = self.waiter.lock().unwrap();
        let prev = waiter.replace(w);
        // The url is unique, so there is at most one connection. The prev can safely drop because we can wait for the new one.
        if prev.is_some() {
            tracing::debug!("replace existing msquic shutdown waiter");
        }
    }
}

#[derive(Clone)]
pub struct H3MsQuicConnector {
    config: Option<Arc<Configuration>>,
    reg: Option<Arc<Registration>>,
    uri: Uri,
    server_name: Option<String>,
    waiter: H3MsQuicClientWaiter,
}

impl H3MsQuicConnector {
    /// Create a new connector that uses the URI host as the server name (SNI).
    pub fn new(
        config: Arc<Configuration>,
        reg: Arc<Registration>,
        uri: Uri,
        waiter: H3MsQuicClientWaiter,
    ) -> Self {
        Self {
            config: Some(config),
            reg: Some(reg),
            uri,
            server_name: None,
            waiter,
        }
    }

    /// Create a new connector with a custom server name (SNI) for TLS.
    /// This is useful when connecting to an IP address but need to send
    /// a hostname as SNI (e.g., for virtual hosting).
    pub fn with_server_name(
        config: Arc<Configuration>,
        reg: Arc<Registration>,
        uri: Uri,
        server_name: String,
        waiter: H3MsQuicClientWaiter,
    ) -> Self {
        Self {
            config: Some(config),
            reg: Some(reg),
            uri,
            server_name: Some(server_name),
            waiter,
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
        // Use custom server_name if provided, otherwise use URI host
        let server_name = self
            .server_name
            .as_deref()
            .unwrap_or_else(|| self.uri.host().unwrap());
        let mut conn = msquic_h3::Connection::connect(
            self.reg.as_ref().unwrap(),
            self.config.as_ref().unwrap(),
            server_name,
            self.uri.port_u16().unwrap(),
        )
        .await
        .map_err(crate::Error::from)?;
        let waiter = conn.get_shutdown_waiter();
        self.waiter.replace(waiter);
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
