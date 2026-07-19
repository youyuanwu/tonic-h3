use hyper::Uri;
use hyper::body::Bytes;

use crate::client::H3Connector;

#[derive(Clone)]
pub struct H3QuinnConnector {
    uri: Uri,
    server_name: String,
    ep: h3_quinn::quinn::Endpoint,
}

impl H3QuinnConnector {
    pub fn new(uri: Uri, server_name: String, ep: h3_quinn::quinn::Endpoint) -> Self {
        Self {
            uri,
            server_name,
            ep,
        }
    }
}

impl H3Connector for H3QuinnConnector {
    type CONN = h3_quinn::Connection;
    type OS = h3_quinn::OpenStreams;
    type SS = h3_quinn::SendStream<Bytes>;
    type RS = h3_quinn::RecvStream;
    type BS = h3_quinn::BidiStream<Bytes>;
    async fn connect(&self) -> Result<Self::CONN, crate::Error> {
        // connect to dns resolved addr.
        let mut conn_err = std::io::Error::from(std::io::ErrorKind::AddrNotAvailable).into();
        let addrs = crate::client::dns_resolve(&self.uri).await?;
        tracing::trace!("connecting to server: {:?}", addrs);
        for addr in addrs {
            match self
                .ep
                .connect(addr, &self.server_name)
                .map_err(Into::<crate::Error>::into)
            {
                // `Endpoint::connect` only validates config synchronously; the real
                // handshake completes on the awaited future below.
                Ok(conn) => match conn.await.map_err(Into::<crate::Error>::into) {
                    Ok(x) => return Ok(h3_quinn::Connection::new(x)),
                    // The async handshake failed for this address. Record the error and
                    // continue to the next resolved address (e.g. IPv6 then IPv4) rather
                    // than aborting the whole connect attempt on the first failure.
                    Err(e) => {
                        tracing::trace!("handshake failed for {addr:?}: {e}");
                        conn_err = e;
                    }
                },
                Err(e) => conn_err = e,
            }
        }
        Err(conn_err)
    }
}
