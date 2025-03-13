use std::{pin::Pin, task::Poll};

use crate::client::H3Connector;
use futures::future::BoxFuture;
use http::{Request, Response};
use hyper::body::{Body, Bytes};

use crate::client_body::H3IncomingClient;

/// Send request. Lowerest layer.
#[derive(Clone)]
pub struct SendRequest<CONN>
where
    CONN: H3Connector,
{
    inner: h3::client::SendRequest<CONN::OS, Bytes>,
}

pub struct SendRequestEntry<CONN>
where
    CONN: H3Connector,
{
    inner: SendRequest<CONN>,
    h: tokio::task::JoinHandle<Result<(), crate::Error>>,
}

impl<CONN, B> tower::Service<Request<B>> for SendRequest<CONN>
where
    CONN: H3Connector,
    B: Body + Send + 'static + Unpin,
    B::Data: Send,
    B::Error: Into<crate::Error> + Send,
{
    type Response = Response<H3IncomingClient<CONN::RS, Bytes>>;

    type Error = crate::Error;

    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<B>) -> Self::Future {
        let send_request = self.inner.clone();
        Box::pin(async move { send_request_inner::<CONN, B>(req, send_request).await })
    }
}

pub async fn send_request_inner<CONN, B>(
    req: hyper::Request<B>,
    mut send_request: h3::client::SendRequest<CONN::OS, Bytes>,
) -> Result<Response<H3IncomingClient<CONN::RS, Bytes>>, crate::Error>
where
    CONN: H3Connector,
    B: Body + Send + 'static + Unpin,
    B::Data: Send,
    B::Error: Into<crate::Error> + Send,
{
    let (parts, body) = req.into_parts();
    let head_req = hyper::Request::from_parts(parts, ());
    // send header
    tracing::debug!("sending h3 req header: {:?}", head_req);

    // send header.
    let stream = send_request.send_request(head_req).await?;

    let (mut w, mut r) = stream.split();
    // send body in backgound
    tokio::spawn(async move {
        crate::client_body::send_h3_client_body::<CONN::BS, _>(&mut w, body).await
    });

    // return resp.
    tracing::debug!("recv header");
    let (resp, _) = r.recv_response().await?.into_parts();
    let resp_body = H3IncomingClient::new(r);
    tracing::debug!("return resp");
    Ok(hyper::Response::from_parts(resp, resp_body))
}

enum SendRequestCacheState<FC, CONN>
where
    CONN: H3Connector,
{
    Idle,
    Making(Pin<Box<FC>>),
    Ready(SendRequestEntry<CONN>),
}

/// Caches a send_request client, and replace it with new one if broke.
/// h3 client should reuse 1 quic connection for all requests.
pub struct CacheSendRequestService<C>
where
    C: H3Connector,
{
    mk_svc: MakeSendRequestService<C>,
    state: SendRequestCacheState<<MakeSendRequestService<C> as tower::Service<()>>::Future, C>,
}

impl<C> CacheSendRequestService<C>
where
    C: H3Connector,
{
    pub fn new(connector: C) -> Self {
        let mk_svc = MakeSendRequestService { connector };
        Self {
            mk_svc,
            state: SendRequestCacheState::Idle,
        }
    }
}

impl<C> tower::Service<()> for CacheSendRequestService<C>
where
    C: H3Connector,
{
    type Response = SendRequest<C>;

    type Error = crate::Error;

    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        loop {
            match self.state {
                SendRequestCacheState::Idle => {
                    tracing::debug!("send_request idle.");
                    futures::ready!(self.mk_svc.poll_ready(cx))?;
                    let fc = self.mk_svc.call(());
                    self.state = SendRequestCacheState::Making(Box::pin(fc))
                }
                SendRequestCacheState::Making(ref mut fc) => {
                    use futures::Future;
                    let pfc = std::pin::Pin::new(fc);
                    match futures::ready!(pfc.poll(cx)) {
                        Ok(c) => {
                            self.state = SendRequestCacheState::Ready(c);
                            tracing::debug!("send_request entry established.");
                            return Poll::Ready(Ok(()));
                        }
                        Err(e) => {
                            self.state = SendRequestCacheState::Idle;
                            return Poll::Ready(Err(e));
                        }
                    }
                }
                SendRequestCacheState::Ready(ref mut entry) => {
                    if entry.h.is_finished() {
                        tracing::debug!("send_request entry broken. Needs make.");
                        self.state = SendRequestCacheState::Idle
                    } else {
                        tracing::debug!("send_request entry cache hit.");
                        return Poll::Ready(Ok(()));
                    }
                }
            }
        }
    }

    fn call(&mut self, _: ()) -> Self::Future {
        let SendRequestCacheState::Ready(send_request_entry) = &mut self.state else {
            panic!("service not ready; poll_ready must be called first");
        };
        // get the cache.
        let send_request = SendRequest {
            inner: send_request_entry.inner.inner.clone(),
        };
        Box::pin(async move { Ok(send_request) })
    }
}

/// Constructs send request, and keep the driver in the background task.
pub struct MakeSendRequestService<C>
where
    C: H3Connector,
{
    connector: C,
}

// TODO: rewrite this using trait?
impl<C> tower::Service<()> for MakeSendRequestService<C>
where
    C: H3Connector,
{
    type Response = SendRequestEntry<C>;

    type Error = crate::Error;

    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, _: ()) -> Self::Future {
        let connector = self.connector.clone();
        Box::pin(async move {
            let conn = connector
                .connect()
                .await
                .inspect_err(|e| tracing::debug!("connector error: {e}"))?;

            tracing::debug!("making new send_request");
            let (mut driver, send_request) = h3::client::new(conn).await?;
            let h = tokio::spawn(async move {
                // run in background to maintain h3 connection until end.
                // Drive the connection
                std::future::poll_fn(|cx| driver.poll_close(cx))
                    .await
                    .inspect_err(|e| {
                        tracing::debug!("poll_close failed: {}", e);
                    })
                    .map_err(crate::Error::from)
            });
            Ok(SendRequestEntry {
                inner: SendRequest {
                    inner: send_request,
                },
                h,
            })
        })
    }
}
