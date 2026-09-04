use hyper::body::Bytes;
//use std::future::Future;

//use crate::server_body::H3IncomingServer;

/// Returns whether a connection ended without an HTTP/3 or transport error.
pub fn is_benign_connection_close(error: &h3::error::ConnectionError) -> bool {
    if error.is_h3_no_error() {
        return true;
    }

    #[cfg(feature = "quinn")]
    if let h3::error::ConnectionError::Remote(h3::quic::ConnectionErrorIncoming::Undefined(source)) =
        error
        && let Some(quinn_error) = source
            .as_ref()
            .downcast_ref::<h3_quinn::quinn::ConnectionError>()
    {
        return matches!(
            quinn_error,
            h3_quinn::quinn::ConnectionError::ConnectionClosed(close)
                if close.error_code == h3_quinn::quinn::TransportErrorCode::NO_ERROR
        );
    }

    false
}

pub trait H3Acceptor {
    type CONN: h3::quic::Connection<
            Bytes,
            OpenStreams = Self::OS,
            SendStream = Self::SS,
            RecvStream = Self::RS,
            BidiStream = Self::BS,
        > + Send
        + 'static;
    type OS: h3::quic::OpenStreams<Bytes, BidiStream = Self::BS> + Clone + Send; // Clone is needed for cloning send_request
    type SS: h3::quic::SendStream<Bytes> + Send;
    type RS: h3::quic::RecvStream + Send + 'static;
    type BS: h3::quic::BidiStream<Bytes, RecvStream = Self::RS, SendStream = Self::SS>
        + Send
        + 'static;

    fn accept(
        &mut self,
    ) -> impl std::future::Future<Output = Result<Option<Self::CONN>, crate::Error>> + std::marker::Send;
}
