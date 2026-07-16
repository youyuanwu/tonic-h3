use std::sync::Arc;

use hyper::Uri;
use msquic_h3::msquic::{Configuration, Registration};
use tokio::sync::watch;

/// Wait for connections to finish in the connector.
/// Must be called before dropping the connector, otherwise there might be deadlock
/// when closing registration while connection is still alive. msquic will use mutex and block
/// rust runtime.
///
/// This tracks every connection created by the connector (including reconnects),
/// so `wait_shutdown` only returns once all of them are fully shut down.
#[derive(Clone)]
pub struct H3MsQuicClientWaiter {
    /// Number of connections that have not yet fully shut down.
    active: watch::Sender<usize>,
}

impl Default for H3MsQuicClientWaiter {
    fn default() -> Self {
        let (active, _rx) = watch::channel(0);
        Self { active }
    }
}

impl H3MsQuicClientWaiter {
    /// Wait for all connections created by the connector to fully shut down.
    /// Returns immediately if there are no active connections.
    pub async fn wait_shutdown(&self) {
        let mut rx = self.active.subscribe();
        // Wait until the active connection count drops back to zero.
        let _ = rx.wait_for(|&n| n == 0).await;
    }

    /// Track a connection's shutdown: increment the active count and spawn a
    /// task that decrements it once the connection is fully shut down.
    fn track(&self, waiter: msquic_h3::ConnectionShutdownWaiter) {
        self.active.send_modify(|n| *n += 1);
        let active = self.active.clone();
        tokio::spawn(async move {
            waiter.wait().await;
            active.send_modify(|n| *n -= 1);
        });
    }
}

#[derive(Clone)]
pub struct H3MsQuicConnector {
    config: Option<Arc<Configuration>>,
    reg: Option<Arc<Registration>>,
    uri: Uri,
    waiter: H3MsQuicClientWaiter,
}

impl H3MsQuicConnector {
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
        let mut conn = msquic_h3::Connection::connect(
            self.reg.as_ref().unwrap(),
            self.config.as_ref().unwrap(),
            self.uri.host().unwrap(),
            self.uri.port_u16().unwrap(),
        )
        .await
        .map_err(crate::Error::from)?;
        let waiter = conn.get_shutdown_waiter();
        self.waiter.track(waiter);
        tracing::trace!("client conn started to {}", self.uri);
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
