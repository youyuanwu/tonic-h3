use hyper::body::Bytes;
use tonic_h3::server::H3Acceptor;

pub struct H3S2nAcceptor {
    ep: s2n_quic::Server,
}

impl H3Acceptor for H3S2nAcceptor {
    type CONN = s2n_quic_h3::Connection;

    type OS = s2n_quic_h3::OpenStreams;

    type SS = s2n_quic_h3::SendStream<Bytes>;

    type RS = s2n_quic_h3::RecvStream;

    type OE = s2n_quic_h3::ConnectionError;

    type BS = s2n_quic_h3::BidiStream<Bytes>;

    async fn accept(&mut self) -> Result<Option<Self::CONN>, tonic_h3::Error> {
        let conn = self.ep.accept().await;
        let Some(conn) = conn else { return Ok(None) };
        Ok(Some(s2n_quic_h3::Connection::new(conn)))
    }
}
