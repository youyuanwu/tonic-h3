use std::sync::Arc;

use crate::server::H3Acceptor;

#[derive(Clone)]
pub struct H3MsQuicAcceptor {
    ep: Arc<tokio::sync::Mutex<msquic_h3::Listener>>,
}

impl H3MsQuicAcceptor {
    pub fn new(inner: msquic_h3::Listener) -> Self {
        Self {
            ep: Arc::new(tokio::sync::Mutex::new(inner)),
        }
    }

    /// shutdowns the acceptor and server.
    pub async fn shutdown(&self) {
        self.ep.lock().await.shutdown().await;
    }
}

impl H3Acceptor for H3MsQuicAcceptor {
    type CONN = msquic_h3::Connection;

    type OS = msquic_h3::StreamOpener;

    type SS = msquic_h3::H3SendStream;

    type RS = msquic_h3::H3RecvStream;

    type OE = msquic_h3::H3Error;

    type BS = msquic_h3::H3Stream;

    async fn accept(&mut self) -> Result<Option<Self::CONN>, crate::Error> {
        let conn = self.ep.lock().await.accept().await?;
        Ok(conn)
    }
}
