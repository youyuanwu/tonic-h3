use std::net::SocketAddr;

use futures::future::BoxFuture;
use http::{Request, Response, Uri};
use hyper::body::Bytes;
use tonic::body::BoxBody;

use crate::{client_body::H3IncomingClient, connection::CacheSendRequestService};

/// Grpc client channel, wrapping inner types for ease of use.
pub struct H3Channel {
    inner: tower::util::BoxService<
        Request<BoxBody>,
        Response<H3IncomingClient<h3_quinn::RecvStream, Bytes>>,
        crate::Error,
    >,
}

impl H3Channel {
    pub fn new<C>(connector: C, uri: Uri) -> Self
    where
        C: tower::Service<Uri, Response = h3_quinn::quinn::Connection, Error = crate::Error>
            + Send
            + 'static,
        C::Future: Send,
    {
        let cache_mk_svc = CacheSendRequestService::new(connector, uri.clone());

        let client_svc = ClientService {
            inner: cache_mk_svc,
            uri,
        };

        Self {
            inner: tower::util::BoxService::new(client_svc),
        }
    }

    pub fn new_quinn(uri: Uri, ep: h3_quinn::quinn::Endpoint) -> Self {
        let connector = connector(uri.clone(), "localhost".to_string(), ep);
        Self::new(connector, uri)
    }
}

impl tower::Service<Request<BoxBody>> for H3Channel {
    type Response = Response<H3IncomingClient<h3_quinn::RecvStream, Bytes>>;
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
    C: tower::Service<Uri, Response = h3_quinn::quinn::Connection, Error = crate::Error>
        + Send
        + 'static,
    C::Future: Send,
{
    inner: CacheSendRequestService<C>,
    uri: Uri,
}

impl<C> tower::Service<Request<BoxBody>> for ClientService<C>
where
    C: tower::Service<Uri, Response = h3_quinn::quinn::Connection, Error = crate::Error>
        + Send
        + 'static,
    C::Future: Send,
{
    type Response = Response<H3IncomingClient<h3_quinn::RecvStream, Bytes>>;

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

/// Make connections from endpoint.
pub fn connector(
    uri: Uri,
    server_name: String,
    ep: h3_quinn::quinn::Endpoint,
) -> impl tower::Service<
    Uri,
    Response = h3_quinn::quinn::Connection,
    Future = impl Send + 'static,
    Error = crate::Error,
> {
    tower::service_fn(move |_: Uri| {
        let ep = ep.clone();
        let uri = uri.clone();
        let server_name = server_name.clone();
        async move {
            // connect to dns resolved addr.
            let mut conn_err = std::io::Error::from(std::io::ErrorKind::AddrNotAvailable).into();
            let addrs = dns_resolve(&uri).await?;

            for addr in addrs {
                match ep
                    .connect(addr, &server_name)
                    .map_err(Into::<crate::Error>::into)
                {
                    Ok(conn) => return conn.await.map_err(Into::<crate::Error>::into),
                    Err(e) => conn_err = e,
                }
            }
            Err(conn_err)
        }
    })
}

/// Use the host:port portion of the uri and resolve to an sockaddr.
/// If uri host portion is an ip string, then directly use the ip addr without
/// dns lookup.
async fn dns_resolve(uri: &Uri) -> std::io::Result<Vec<SocketAddr>> {
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
