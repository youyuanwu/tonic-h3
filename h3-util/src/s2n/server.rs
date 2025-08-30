use super::s2n_quic_h3;
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
    type CONN = s2n_quic_h3::Connection;

    type OS = s2n_quic_h3::OpenStreams;

    type SS = s2n_quic_h3::SendStream<Bytes>;

    type RS = s2n_quic_h3::RecvStream;

    type BS = s2n_quic_h3::BidiStream<Bytes>;

    async fn accept(&mut self) -> Result<Option<Self::CONN>, crate::Error> {
        let conn = self.ep.accept().await;
        let Some(conn) = conn else { return Ok(None) };
        Ok(Some(s2n_quic_h3::Connection::new(conn)))
    }
}
