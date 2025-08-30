use crate::server::H3Acceptor;
use hyper::body::Bytes;

pub struct H3S2nAcceptor {
    ep: s2n_quic::Server,
}

impl H3S2nAcceptor {
    pub fn new(ep: s2n_quic::Server) -> Self {
        Self { ep }
    }
}

impl H3Acceptor for H3S2nAcceptor {
    type CONN = crate::s2n_quic_h3::Connection;

    type OS = crate::s2n_quic_h3::OpenStreams;

    type SS = crate::s2n_quic_h3::SendStream<Bytes>;

    type RS = crate::s2n_quic_h3::RecvStream;

    type BS = crate::s2n_quic_h3::BidiStream<Bytes>;

    async fn accept(&mut self) -> Result<Option<Self::CONN>, crate::Error> {
        let conn = self.ep.accept().await;
        let Some(conn) = conn else { return Ok(None) };
        Ok(Some(crate::s2n_quic_h3::Connection::new(conn)))
    }
}
