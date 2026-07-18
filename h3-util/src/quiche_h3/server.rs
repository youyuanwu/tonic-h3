use hyper::body::Bytes;
use tokio::net::UdpSocket;

use crate::server::H3Acceptor;

// The local bridge crate's acceptor shares our wrapper's name, so alias it.
use quiche_h3::H3QuicheAcceptor as InnerAcceptor;
use quiche_h3::H3QuicheServerConfig;

/// [`H3Acceptor`] backed by the local `quiche-h3` bridge crate.
///
/// Wraps a single `quiche_h3::H3QuicheAcceptor` bound to one UDP socket and
/// forwards `accept()` to it. Construct it with an already-bound
/// [`UdpSocket`] and a [`H3QuicheServerConfig`]; capture the listening address
/// from the socket before handing it over if you need it.
pub struct H3QuicheAcceptor {
    inner: InnerAcceptor,
}

impl H3QuicheAcceptor {
    /// Bind the acceptor to a single UDP socket using the given server config.
    ///
    /// Must be called from within a Tokio runtime (the underlying listener
    /// registers with the reactor).
    pub fn new(socket: UdpSocket, config: &H3QuicheServerConfig) -> Result<Self, crate::Error> {
        let mut acceptors = InnerAcceptor::bind([socket], config)?;
        let inner = acceptors.pop().ok_or_else(|| {
            crate::Error::from("quiche_h3::H3QuicheAcceptor::bind returned no acceptor")
        })?;
        Ok(Self { inner })
    }
}

impl H3Acceptor for H3QuicheAcceptor {
    type CONN = quiche_h3::Connection<Bytes>;
    type OS = quiche_h3::StreamOpener<Bytes>;
    type SS = quiche_h3::H3SendStream<Bytes>;
    type RS = quiche_h3::H3RecvStream<Bytes>;
    type BS = quiche_h3::H3Stream<Bytes>;

    async fn accept(&mut self) -> Result<Option<Self::CONN>, crate::Error> {
        self.inner.accept().await
    }
}
