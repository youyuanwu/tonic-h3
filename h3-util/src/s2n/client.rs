use hyper::body::Bytes;
use hyper::http::Uri;

use super::s2n_quic_h3;

#[derive(Clone)]
pub struct H3S2nConnector {
    uri: Uri,
    server_name: String,
    ep: s2n_quic::Client,
}

impl H3S2nConnector {
    pub fn new(uri: Uri, server_name: String, ep: s2n_quic::Client) -> Self {
        Self {
            uri,
            server_name,
            ep,
        }
    }
}

impl crate::client::H3Connector for H3S2nConnector {
    type CONN = s2n_quic_h3::Connection;

    type OS = s2n_quic_h3::OpenStreams;

    type SS = s2n_quic_h3::SendStream<Bytes>;

    type RS = s2n_quic_h3::RecvStream;

    type BS = s2n_quic_h3::BidiStream<Bytes>;

    async fn connect(&self) -> Result<Self::CONN, crate::Error> {
        // connect to dns resolved addr.
        let mut conn_err = std::io::Error::from(std::io::ErrorKind::AddrNotAvailable).into();
        let addrs = crate::client::dns_resolve(&self.uri).await?;
        tracing::debug!("connecting to server: {:?}", addrs);
        for addr in addrs {
            let connect =
                s2n_quic::client::Connect::new(addr).with_server_name(self.server_name.as_str());
            match self
                .ep
                .connect(connect)
                .await
                .map_err(Into::<crate::Error>::into)
            {
                Ok(mut conn) => {
                    conn.keep_alive(true)?;
                    return Ok(s2n_quic_h3::Connection::new(conn));
                }
                Err(e) => conn_err = e,
            }
        }
        Err(conn_err)
    }
}
