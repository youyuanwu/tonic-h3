use hyper::body::Bytes;

pub trait H3Acceptor {
    type CONN: h3::quic::Connection<
            Bytes,
            OpenStreams = Self::OS,
            SendStream = Self::SS,
            RecvStream = Self::RS,
            OpenError = Self::OE,
            BidiStream = Self::BS,
        > + Send
        + 'static;
    type OS: h3::quic::OpenStreams<Bytes, OpenError = Self::OE, BidiStream = Self::BS>
        + Clone
        + Send; // Clone is needed for cloning send_request
    type SS: h3::quic::SendStream<Bytes> + Send;
    type RS: h3::quic::RecvStream + Send + 'static;
    type OE: Into<Box<dyn std::error::Error>> + Send;
    type BS: h3::quic::BidiStream<Bytes, RecvStream = Self::RS, SendStream = Self::SS>
        + Send
        + 'static;

    fn accept(
        &mut self,
    ) -> impl std::future::Future<Output = Result<Option<Self::CONN>, crate::Error>> + std::marker::Send;
}
