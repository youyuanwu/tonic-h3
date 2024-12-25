use std::{pin::Pin, task::Poll};

use futures::future::BoxFuture;
use http::{Request, Response, Uri};
use hyper::body::Bytes;
use tonic::body::BoxBody;

use crate::client_body::H3IncomingClient;

/// Send request. Lowerest layer.
// #[derive(Debug)]
pub struct SendRequest {
    inner: h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>,
}

pub struct SendRequestEntry {
    inner: SendRequest,
    h: tokio::task::JoinHandle<Result<(), crate::Error>>,
}

impl tower::Service<Request<BoxBody>> for SendRequest {
    type Response = Response<H3IncomingClient<h3_quinn::RecvStream, Bytes>>;

    type Error = crate::Error;

    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<BoxBody>) -> Self::Future {
        let send_request = self.inner.clone();
        Box::pin(async move { send_request_inner(req, send_request).await })
    }
}

pub async fn send_request_inner(
    req: hyper::Request<tonic::body::BoxBody>,
    mut send_request: h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>,
) -> Result<Response<H3IncomingClient<h3_quinn::RecvStream, Bytes>>, crate::Error> {
    let (parts, body) = req.into_parts();
    let head_req = hyper::Request::from_parts(parts, ());
    // send header
    tracing::debug!("sending h3 req header: {:?}", head_req);

    // send header.
    let stream = send_request.send_request(head_req).await?;

    let (mut w, mut r) = stream.split();
    // send body in backgound
    tokio::spawn(async move {
        crate::client_body::send_h3_client_body::<h3_quinn::BidiStream<Bytes>>(&mut w, body).await
    });

    // return resp.
    tracing::debug!("recv header");
    let (resp, _) = r.recv_response().await?.into_parts();
    let resp_body = H3IncomingClient::new(r);
    tracing::debug!("return resp");
    Ok(hyper::Response::from_parts(resp, resp_body))
}

enum SendRequestCacheState<FC> {
    Idle,
    Making(Pin<Box<FC>>),
    Ready(SendRequestEntry),
}

/// Caches a send_request client, and replace it with new one if broke.
/// h3 tonic client should reuse 1 quic connection for all requests.
pub struct CacheSendRequestService<C>
where
    C: tower::Service<Uri, Response = h3_quinn::quinn::Connection, Error = crate::Error>
        + Send
        + 'static,
    C::Future: Send,
{
    mk_svc: MakeSendRequestService<C>,
    state: SendRequestCacheState<<MakeSendRequestService<C> as tower::Service<()>>::Future>,
}

impl<C> CacheSendRequestService<C>
where
    C: tower::Service<Uri, Response = h3_quinn::quinn::Connection, Error = crate::Error>
        + Send
        + 'static,
    C::Future: Send,
{
    pub fn new(connector: C, uri: Uri) -> Self {
        let mk_svc = MakeSendRequestService {
            conn_svc: ReconnectService::new(connector, uri.clone()),
        };
        Self {
            mk_svc,
            state: SendRequestCacheState::Idle,
        }
    }
}

impl<C> tower::Service<()> for CacheSendRequestService<C>
where
    C: tower::Service<Uri, Response = h3_quinn::quinn::Connection, Error = crate::Error>
        + Send
        + 'static,
    C::Future: Send,
{
    type Response = SendRequest;

    type Error = crate::Error;

    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        loop {
            match self.state {
                SendRequestCacheState::Idle => {
                    futures::ready!(self.mk_svc.poll_ready(cx))?;
                    let fc = self.mk_svc.call(());
                    self.state = SendRequestCacheState::Making(Box::pin(fc))
                }
                SendRequestCacheState::Making(ref mut fc) => {
                    use futures_util::Future;
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
    C: tower::Service<Uri, Response = h3_quinn::quinn::Connection, Error = crate::Error>
        + Send
        + 'static,
    C::Future: Send,
{
    pub(crate) conn_svc: ReconnectService<C>,
}

impl<C> tower::Service<()> for MakeSendRequestService<C>
where
    C: tower::Service<Uri, Response = h3_quinn::quinn::Connection, Error = crate::Error>
        + Send
        + 'static,
    C::Future: Send,
{
    type Response = SendRequestEntry;

    type Error = crate::Error;

    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.conn_svc.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, _: ()) -> Self::Future {
        let fut = self.conn_svc.call(());
        Box::pin(async move {
            let conn = fut.await.map_err(crate::Error::from)?;

            tracing::debug!("making new send_request");
            let (mut driver, send_request) =
                h3::client::new(h3_quinn::Connection::new(conn.clone())).await?;
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

enum State<FC> {
    Idle,
    Connecting(Pin<Box<FC>>), // future of making connection.
    ConnectionReady(h3_quinn::quinn::Connection),
}

/// Keep and cache a good connection and reconnect.
pub struct ReconnectService<C>
where
    C: tower::Service<Uri, Response = h3_quinn::quinn::Connection, Error = crate::Error>
        + Send
        + 'static,
    C::Future: Send,
{
    connector: C,
    state: State<C::Future>,
    uri: Uri,
}

impl<C> ReconnectService<C>
where
    C: tower::Service<Uri, Response = h3_quinn::quinn::Connection, Error = crate::Error>
        + Send
        + 'static,
    C::Future: Send,
{
    pub fn new(connector: C, uri: Uri) -> Self {
        Self {
            connector,
            state: State::Idle,
            uri,
        }
    }
}

impl<C> tower::Service<()> for ReconnectService<C>
where
    C: tower::Service<Uri, Response = h3_quinn::quinn::Connection, Error = crate::Error>
        + Send
        + 'static,
    C::Future: Send,
{
    type Response = h3_quinn::quinn::Connection;

    type Error = crate::Error;

    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        loop {
            match self.state {
                State::Idle => {
                    futures::ready!(self.connector.poll_ready(cx))?;
                    let fc = self.connector.call(self.uri.clone());
                    self.state = State::Connecting(Box::pin(fc))
                }
                State::Connecting(ref mut fc) => {
                    use futures_util::Future;
                    let pfc = std::pin::Pin::new(fc);
                    match futures::ready!(pfc.poll(cx)) {
                        Ok(c) => {
                            self.state = State::ConnectionReady(c);
                            tracing::debug!("client connection established.");
                            return Poll::Ready(Ok(()));
                        }
                        Err(e) => {
                            self.state = State::Idle;
                            return Poll::Ready(Err(e));
                        }
                    }
                }
                State::ConnectionReady(ref mut connection) => {
                    if let Some(e) = connection.close_reason() {
                        tracing::debug!("client connection broken. Needs reconnect. {}", e);
                        self.state = State::Idle
                    } else {
                        tracing::debug!("client connection cache hit.");
                        return Poll::Ready(Ok(()));
                    }
                }
            }
        }
    }

    fn call(&mut self, _: ()) -> Self::Future {
        let State::ConnectionReady(conn) = &mut self.state else {
            panic!("service not ready; poll_ready must be called first");
        };
        let conn = conn.clone();
        Box::pin(async move { Ok(conn) })
    }
}
