use std::net::SocketAddr;

use futures::future::BoxFuture;
use http::{Request, Response, Uri};
use hyper::body::Bytes;
use tonic::body::BoxBody;

use crate::{client_body::H3IncomingClient, connection::CacheSendRequestService};

/// Grpc client channel, wrapping inner types for ease of use.
pub struct H3Channel<C>
where
    C: H3Connector,
{
    #[allow(clippy::type_complexity)]
    inner: tower::util::BoxService<
        Request<BoxBody>,
        Response<H3IncomingClient<C::RS, Bytes>>,
        crate::Error,
    >,
}

impl<C> H3Channel<C>
where
    C: H3Connector,
{
    pub fn new(connector: C, uri: Uri) -> Self {
        let cache_mk_svc = CacheSendRequestService::new(connector);

        let client_svc = ClientService {
            inner: cache_mk_svc,
            uri,
        };

        Self {
            inner: tower::util::BoxService::new(client_svc),
        }
    }
}

impl<C> tower::Service<Request<BoxBody>> for H3Channel<C>
where
    C: H3Connector,
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

    fn call(&mut self, req: Request<BoxBody>) -> Self::Future {
        self.inner.call(req)
    }
}

/// Client service that includes cache and reconnection.
pub struct ClientService<C>
where
    C: H3Connector,
{
    inner: CacheSendRequestService<C>,
    uri: Uri,
}

impl<C> tower::Service<Request<BoxBody>> for ClientService<C>
where
    C: H3Connector,
{
    type Response = Response<H3IncomingClient<C::RS, Bytes>>;

    type Error = crate::Error;

    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<BoxBody>) -> Self::Future {
        let uri = &self.uri;
        // fix up uri with full uri.
        let uri2 = Uri::builder()
            .scheme(uri.scheme().unwrap().clone())
            .authority(uri.authority().unwrap().clone())
            .path_and_query(req.uri().path_and_query().unwrap().clone())
            .build()
            .unwrap();
        *req.uri_mut() = uri2;

        let fut = self.inner.call(());
        Box::pin(async move {
            let mut send_request = fut.await?;
            send_request.call(req).await
        })
    }
}

/// Use the host:port portion of the uri and resolve to an sockaddr.
/// If uri host portion is an ip string, then directly use the ip addr without
/// dns lookup.
pub(crate) async fn dns_resolve(uri: &Uri) -> std::io::Result<Vec<SocketAddr>> {
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

pub trait H3Connector: Send + 'static + Clone {
    type CONN: h3::quic::Connection<
            Bytes,
            OpenStreams = Self::OS,
            SendStream = Self::SS,
            RecvStream = Self::RS,
            OpenError = Self::OE,
        > + Send;
    type OS: h3::quic::OpenStreams<Bytes, OpenError = Self::OE, BidiStream = Self::BS>
        + Clone
        + Send; // Clone is needed for cloning send_request
    type SS: h3::quic::SendStream<Bytes> + Send;
    type RS: h3::quic::RecvStream + Send;
    type OE: Into<Box<dyn std::error::Error>> + Send;
    type BS: h3::quic::BidiStream<Bytes, RecvStream = Self::RS, SendStream = Self::SS> + Send;

    fn connect(
        &self,
    ) -> impl std::future::Future<Output = Result<Self::CONN, crate::Error>> + std::marker::Send;
}
