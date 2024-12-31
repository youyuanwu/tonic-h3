use http::Uri;
use hyper::body::Bytes;

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

impl h3_util::client::H3Connector for H3S2nConnector {
    type CONN = s2n_quic_h3::Connection;

    type OS = s2n_quic_h3::OpenStreams;

    type SS = s2n_quic_h3::SendStream<Bytes>;

    type RS = s2n_quic_h3::RecvStream;

    type OE = s2n_quic_h3::ConnectionError;

    type BS = s2n_quic_h3::BidiStream<Bytes>;

    async fn connect(&self) -> Result<Self::CONN, tonic_h3::Error> {
        // connect to dns resolved addr.
        let mut conn_err = std::io::Error::from(std::io::ErrorKind::AddrNotAvailable).into();
        let addrs = h3_util::client::dns_resolve(&self.uri).await?;
        tracing::debug!("connecting to server: {:?}", addrs);
        for addr in addrs {
            let connect =
                s2n_quic::client::Connect::new(addr).with_server_name(self.server_name.as_str());
            match self
                .ep
                .connect(connect)
                .await
                .map_err(Into::<tonic_h3::Error>::into)
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

pub fn new_s2n_h3_channel(uri: Uri, ep: s2n_quic::Client) -> tonic_h3::H3Channel<H3S2nConnector> {
    let connector = H3S2nConnector::new(uri.clone(), "localhost".to_string(), ep);
    tonic_h3::H3Channel::new(connector, uri)
}
