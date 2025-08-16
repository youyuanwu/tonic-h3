use std::net::SocketAddr;

use futures::future::BoxFuture;
use hyper::body::{Body, Bytes};
use hyper::{Request, Response, Uri};

use crate::client_body::H3IncomingClient;
use crate::client_conn;

pub trait H3Connector: Send + 'static + Clone {
    type CONN: h3::quic::Connection<
            Bytes,
            OpenStreams = Self::OS,
            SendStream = Self::SS,
            RecvStream = Self::RS,
        > + Send;
    type OS: h3::quic::OpenStreams<Bytes, BidiStream = Self::BS> + Clone + Send; // Clone is needed for cloning send_request
    type SS: h3::quic::SendStream<Bytes> + Send;
    type RS: h3::quic::RecvStream + Send;
    type BS: h3::quic::BidiStream<Bytes, RecvStream = Self::RS, SendStream = Self::SS> + Send;

    fn connect(
        &self,
    ) -> impl std::future::Future<Output = Result<Self::CONN, crate::Error>> + std::marker::Send;
}

/// Use the host:port portion of the uri and resolve to an sockaddr.
/// If uri host portion is an ip string, then directly use the ip addr without
/// dns lookup.
pub async fn dns_resolve(uri: &Uri) -> std::io::Result<Vec<SocketAddr>> {
    let host_port = uri
        .authority()
        .ok_or(std::io::Error::from(std::io::ErrorKind::InvalidInput))?
        .as_str();
    match host_port.parse::<SocketAddr>() {
        Ok(addr) => Ok(vec![addr]),
        Err(_) => {
            // uri is using a dns name. try resolve it and return the first.
            tokio::net::lookup_host(host_port)
                .await
                .map(|a| a.collect::<Vec<_>>())
        }
    }
}

/// h3 client connection, wrapping inner types for ease of use.
/// All request will be sent to the connection established using the connector.
/// Currently connector can only connect to a fixed server (to support grpc use case).
/// Expand connector to do resolve different server based on uri can be added in future.
pub struct H3Connection<C, B>
where
    C: H3Connector,
    B: Body + Send + 'static + Unpin,
    B::Data: Send,
    B::Error: Into<crate::Error>,
{
    #[allow(clippy::type_complexity)]
    inner:
        tower::util::BoxService<Request<B>, Response<H3IncomingClient<C::RS, Bytes>>, crate::Error>,
}

impl<C, B> H3Connection<C, B>
where
    C: H3Connector,
    B: Body + Send + 'static + Unpin,
    B::Data: Send,
    B::Error: Into<crate::Error> + Send,
{
    pub fn new(connector: C, uri: Uri) -> Self {
        let sender = client_conn::RequestSender::new(connector, uri);
        Self {
            inner: tower::util::BoxService::new(sender),
        }
    }
}

impl<C, B> tower::Service<Request<B>> for H3Connection<C, B>
where
    C: H3Connector,
    B: Body + Send + 'static + Unpin,
    B::Data: Send,
    B::Error: Into<crate::Error>,
{
    type Response = Response<H3IncomingClient<C::RS, Bytes>>;
    type Error = crate::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        tower::Service::poll_ready(&mut self.inner, cx)
    }

    fn call(&mut self, req: Request<B>) -> Self::Future {
        self.inner.call(req)
    }
}

/// http3 client.
/// Note the client does not do dns resolve but blindly sends requests
/// using connections created by the connector.
/// Used for sending HTTP request directly.
pub struct H3Client<C, B>
where
    C: H3Connector,
    B: Body + Send + 'static + Unpin,
    B::Data: Send,
    B::Error: Into<crate::Error> + Send,
{
    channel: H3Connection<C, B>,
}

impl<C, B> H3Client<C, B>
where
    C: H3Connector,
    B: Body + Send + 'static + Unpin,
    B::Data: Send,
    B::Error: Into<crate::Error> + Send,
{
    pub fn new(inner: H3Connection<C, B>) -> Self {
        Self { channel: inner }
    }

    pub async fn send(
        &mut self,
        req: Request<B>,
    ) -> Result<Response<H3IncomingClient<C::RS, Bytes>>, crate::Error> {
        use tower::Service;
        // wait for ready
        futures::future::poll_fn(|cx| self.channel.poll_ready(cx)).await?;
        self.channel.call(req).await
    }
}
