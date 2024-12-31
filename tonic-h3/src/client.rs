use futures::future::BoxFuture;
use h3_util::client::H3Connector;
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
