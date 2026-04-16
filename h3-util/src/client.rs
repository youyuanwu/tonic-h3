use std::{
    fmt,
    future::Future,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};

use futures::future::BoxFuture;
use hyper::{
    Request, Response, Uri,
    body::{Body, Bytes},
    rt::Executor,
};
use tower::{
    Service,
    buffer::{Buffer, future::ResponseFuture as BufferResponseFuture},
    util::BoxService,
};

use crate::client_conn;
use crate::{client_body::H3IncomingClient, executor::SharedExec};

const DEFAULT_BUFFER_SIZE: usize = 1024;

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

/// Cloneable http3 client channel, which can be used to enable multiplexing requests.
pub struct H3Channel<C, B>
where
    C: H3Connector,
    B: Body + Send + 'static + Unpin,
    B::Data: Send,
    B::Error: Into<crate::Error> + Send,
{
    #[allow(clippy::type_complexity)]
    svc: Buffer<
        Request<B>,
        BoxFuture<'static, Result<Response<H3IncomingClient<C::RS, Bytes>>, crate::Error>>,
    >,
}

impl<C, B> Clone for H3Channel<C, B>
where
    C: H3Connector,
    B: Body + Send + 'static + Unpin,
    B::Data: Send,
    B::Error: Into<crate::Error> + Send,
{
    fn clone(&self) -> Self {
        Self {
            svc: self.svc.clone(),
        }
    }
}

pub struct ResponseFuture<C>
where
    C: H3Connector,
{
    #[allow(clippy::type_complexity)]
    inner: BufferResponseFuture<
        BoxFuture<'static, Result<Response<H3IncomingClient<C::RS, Bytes>>, crate::Error>>,
    >,
}

impl<C, B> H3Channel<C, B>
where
    C: H3Connector,
    B: Body + Send + 'static + Unpin,
    B::Data: Send,
    B::Error: Into<crate::Error> + Send,
{
    pub fn new(connector: C, uri: Uri, executor: Option<SharedExec>) -> Self {
        let executor = executor.unwrap_or_else(SharedExec::tokio);
        let svc = H3Connection::new(connector, uri, Some(executor.clone()));
        let (svc, worker) = Buffer::pair(svc, DEFAULT_BUFFER_SIZE);
        executor.execute(worker);
        Self { svc }
    }
}

impl<C, B> Service<Request<B>> for H3Channel<C, B>
where
    C: H3Connector,
    B: Body + Send + 'static + Unpin,
    B::Data: Send,
    B::Error: Into<crate::Error> + Send,
{
    type Response = Response<H3IncomingClient<C::RS, Bytes>>;
    type Error = crate::Error;
    type Future = ResponseFuture<C>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Service::poll_ready(&mut self.svc, cx).map_err(crate::Error::from)
    }

    fn call(&mut self, req: Request<B>) -> Self::Future {
        let inner = Service::call(&mut self.svc, req);
        ResponseFuture { inner }
    }
}

impl<C> Future for ResponseFuture<C>
where
    C: H3Connector,
{
    type Output = Result<Response<H3IncomingClient<C::RS, Bytes>>, crate::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.inner)
            .poll(cx)
            .map_err(crate::Error::from)
    }
}

impl<C, B> fmt::Debug for H3Channel<C, B>
where
    C: H3Connector,
    B: Body + Send + 'static + Unpin,
    B::Data: Send,
    B::Error: Into<crate::Error> + Send,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("H3Channel").finish()
    }
}

impl<C> fmt::Debug for ResponseFuture<C>
where
    C: H3Connector,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResponseFuture").finish()
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
    inner: BoxService<Request<B>, Response<H3IncomingClient<C::RS, Bytes>>, crate::Error>,
}

impl<C, B> H3Connection<C, B>
where
    C: H3Connector,
    B: Body + Send + 'static + Unpin,
    B::Data: Send,
    B::Error: Into<crate::Error> + Send,
{
    pub fn new(connector: C, uri: Uri, executor: Option<SharedExec>) -> Self {
        let executor = executor.unwrap_or_else(SharedExec::tokio);
        let sender = client_conn::RequestSender::new(connector, uri, executor);
        Self {
            inner: BoxService::new(sender),
        }
    }
}

impl<C, B> Service<Request<B>> for H3Connection<C, B>
where
    C: H3Connector,
    B: Body + Send + 'static + Unpin,
    B::Data: Send,
    B::Error: Into<crate::Error>,
{
    type Response = Response<H3IncomingClient<C::RS, Bytes>>;
    type Error = crate::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Service::poll_ready(&mut self.inner, cx)
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
        // wait for ready
        futures::future::poll_fn(|cx| self.channel.poll_ready(cx)).await?;
        self.channel.call(req).await
    }
}
