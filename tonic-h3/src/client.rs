use futures::future::BoxFuture;
use h3_util::{
    client::{H3Connection, H3Connector},
    client_body::H3IncomingClient,
};
use http::{Request, Response, Uri};
use hyper::body::Bytes;

/// Wrap around h3 channel but change the error type
pub struct H3Channel<C>
where
    C: H3Connector,
{
    inner: H3Connection<C, tonic::body::Body>,
}

impl<C> H3Channel<C>
where
    C: H3Connector,
{
    pub fn new(connector: C, uri: Uri) -> Self {
        Self {
            inner: H3Connection::new(connector, uri),
        }
    }
}

impl<C> tower::Service<Request<tonic::body::Body>> for H3Channel<C>
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

    fn call(&mut self, req: Request<tonic::body::Body>) -> Self::Future {
        // change the body error type to be generic
        // let (header, body) = req.into_parts();
        // use http_body_util::BodyExt;
        // let b = H3BoxBody::new(body.map_err(|e| h3_util::Error::from(e)));
        // let mapped_req = http::Request::from_parts(header, b);
        self.inner.call(req)
    }
}
