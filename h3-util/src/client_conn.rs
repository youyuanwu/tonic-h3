use crate::{client::H3Connector, executor::SharedExec};
use futures::{FutureExt, future::BoxFuture};
use hyper::{
    Request, Response, Uri,
    body::{Body, Bytes},
    http::uri::{Authority, PathAndQuery, Scheme},
    rt::Executor,
};
use std::future::Future;

use crate::client_body::H3IncomingClient;

pub async fn send_request_inner<CONN, B>(
    req: hyper::Request<B>,
    mut send_request: h3::client::SendRequest<CONN::OS, Bytes>,
    executor: &SharedExec,
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
    tracing::trace!("sending h3 req header: {:?}", head_req);

    // send header.
    let stream = send_request.send_request(head_req).await?;

    let (w, mut r) = stream.split();

    // Cancellation: cancel_tx is stored in H3IncomingClient.
    // When the response body is dropped, cancel_tx drops, triggering cancellation.
    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();

    // Build the body send future with owned w and cancel support.
    let mut body_fut = Box::pin(crate::client_body::send_h3_client_body::<CONN::BS, _>(
        w, body, cancel_rx,
    ));

    // Eager poll: try to complete body send without spawning a task.
    match futures::future::poll_fn(|cx| match body_fut.as_mut().poll(cx) {
        std::task::Poll::Ready(res) => std::task::Poll::Ready(Some(res)),
        std::task::Poll::Pending => std::task::Poll::Ready(None),
    })
    .await
    {
        Some(res) => {
            // Body completed synchronously — no spawn needed.
            res?;
        }
        None => {
            // Body still pending — move to background task.
            executor.execute(async move {
                if let Err(e) = body_fut.await {
                    tracing::warn!("h3 client body send failed: {e}");
                }
            });
        }
    };

    // return resp.
    tracing::trace!("recv header");
    let (resp, _) = r
        .recv_response()
        .await
        .inspect_err(|e| {
            tracing::error!("recv header error: {e}");
        })?
        .into_parts();
    let resp_body = H3IncomingClient::new(r, Some(cancel_tx));
    tracing::trace!("return resp");
    Ok(hyper::Response::from_parts(resp, resp_body))
}

/// Sender that can do reconnection.
#[allow(clippy::type_complexity)]
pub struct RequestSender<CONN: H3Connector> {
    conn: CONN,
    send_request: Option<h3::client::SendRequest<CONN::OS, Bytes>>,
    driver_rx: Option<tokio::sync::oneshot::Receiver<()>>,
    make_send_request_fut: Option<
        BoxFuture<
            'static,
            Result<
                (
                    h3::client::SendRequest<CONN::OS, Bytes>,
                    tokio::sync::oneshot::Receiver<()>,
                ),
                crate::Error,
            >,
        >,
    >,
    executor: SharedExec,
    // Precomputed base URI parts (SF-1). The public constructors accept any `http::Uri`,
    // whose scheme and authority are `Option`s. We validate/clone them once here so the
    // request hot path (`call`) never unwraps a user-supplied `Option` and panics. `None`
    // records that the base URI lacked that component; `call` surfaces this as a
    // per-request error future rather than panicking (mirroring `connect_error`).
    base_scheme: Option<Scheme>,
    base_authority: Option<Authority>,
    // Stores a connect/handshake error from `poll_ready` so it can be surfaced as a
    // per-request failure from `call` instead of a terminal readiness error. Returning
    // an error from `poll_ready` would cause `tower::buffer::Buffer` to treat the channel
    // as permanently failed (it closes the request channel and replays the stored error
    // to every subsequent request and cloned handle), which would defeat reconnection.
    connect_error: Option<crate::Error>,
}

impl<CONN> RequestSender<CONN>
where
    CONN: H3Connector,
{
    pub fn new(conn: CONN, uri: Uri, executor: SharedExec) -> Self {
        // SF-1: precompute/validate the base URI's scheme and authority once. Do NOT
        // panic here on a missing component — the public constructors must stay
        // infallible (return `Self`). Absence is recorded and surfaced per-request
        // from `call` as a clean error future.
        let base_scheme = uri.scheme().cloned();
        let base_authority = uri.authority().cloned();
        Self {
            conn,
            send_request: None,
            driver_rx: None,
            make_send_request_fut: None,
            executor,
            base_scheme,
            base_authority,
            connect_error: None,
        }
    }
}

impl<CONN, B> tower::Service<Request<B>> for RequestSender<CONN>
where
    CONN: H3Connector,
    B: Body + Send + 'static + Unpin,
    B::Data: Send,
    B::Error: Into<crate::Error> + Send,
{
    type Response = Response<H3IncomingClient<CONN::RS, Bytes>>;
    type Error = crate::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    /// This handles connection creation and reconnection.
    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        if let Some(rx) = &mut self.driver_rx {
            // check if the driver is still running
            match rx.try_recv() {
                Ok(()) => {
                    tracing::trace!("driver is closed, reconnecting.");
                    self.send_request = None;
                    self.driver_rx = None;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                    // driver is still running
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    tracing::trace!("driver is closed, reconnecting.");
                    self.send_request = None;
                    self.driver_rx = None;
                }
            }
        }

        // Idempotency guard (tower `Service` contract): if a previous connect attempt
        // failed, we are "ready" — the stored error will be surfaced by the next `call`.
        // Returning ready here (before any connect logic) ensures repeated `poll_ready`
        // calls stay ready and do NOT start a second connect attempt or clear/overwrite
        // the pending error. The error is cleared only when `call` consumes it, after
        // which the next `poll_ready` (guard sees `None`) starts a fresh connect attempt.
        if self.connect_error.is_some() {
            return std::task::Poll::Ready(Ok(()));
        }

        // ready for send.
        if self.send_request.is_some() {
            tracing::trace!("exp poll_ready cache hit.");
            assert!(self.make_send_request_fut.is_none());
            assert!(self.driver_rx.is_some());
            return std::task::Poll::Ready(Ok(()));
        }

        if self.make_send_request_fut.is_none() {
            // start the driver in the background
            let conn = self.conn.clone();
            let executor = self.executor.clone();
            self.make_send_request_fut = Some(Box::pin(async move {
                let conn = conn.connect().await?;
                let (mut driver, send_request) = h3::client::new(conn).await?;
                let (tx, rx) = tokio::sync::oneshot::channel();
                executor.execute(async move {
                    let res = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
                    tracing::trace!("h3 driver ended: {res:?}");
                    let _ = tx.send(());
                });
                Ok((send_request, rx))
            }));
        }
        self.make_send_request_fut
            .as_mut()
            .unwrap()
            .poll_unpin(cx)
            .map(|res| match res {
                Ok((send_request, rx)) => {
                    self.send_request = Some(send_request);
                    self.driver_rx = Some(rx);
                    self.make_send_request_fut = None;
                    Ok(())
                }
                Err(e) => {
                    // Defer the connect/handshake error to the next `call` instead of
                    // returning it here. Surfacing it through `poll_ready` would make
                    // `tower::Buffer` permanently fail the channel (see `connect_error`).
                    // `send_request` stays `None` and `make_send_request_fut` is cleared,
                    // so once `call` consumes the error the next `poll_ready` reconnects.
                    self.make_send_request_fut = None;
                    self.connect_error = Some(e);
                    Ok(())
                }
            })
    }

    /// Gets the send_request from the cache and send the request.
    fn call(&mut self, mut req: Request<B>) -> Self::Future {
        // Surface any deferred connect/handshake error as a per-request failure. `take`
        // moves the error out, so the next `poll_ready` will start a fresh connect.
        if let Some(e) = self.connect_error.take() {
            return Box::pin(async move { Err(e) });
        }

        // SF-1: rebuild the request URI from the validated base scheme+authority and the
        // request's path-and-query, surfacing any missing/invalid component as a
        // per-request error future (mirroring the `connect_error` shape) instead of
        // panicking. Done BEFORE cloning the cached sender so an invalid base URI never
        // consumes a connection.
        let (scheme, authority) = match (self.base_scheme.clone(), self.base_authority.clone()) {
            (Some(scheme), Some(authority)) => (scheme, authority),
            (None, _) => {
                return Box::pin(async move {
                    Err(crate::Error::from("h3 client base URI is missing a scheme"))
                });
            }
            (_, None) => {
                return Box::pin(async move {
                    Err(crate::Error::from(
                        "h3 client base URI is missing an authority",
                    ))
                });
            }
        };
        // A request target without a path-and-query defaults to origin-form "/".
        let path_and_query = req
            .uri()
            .path_and_query()
            .cloned()
            .unwrap_or_else(|| PathAndQuery::from_static("/"));
        let uri2 = match Uri::builder()
            .scheme(scheme)
            .authority(authority)
            .path_and_query(path_and_query)
            .build()
        {
            Ok(uri2) => uri2,
            Err(e) => {
                return Box::pin(async move { Err(crate::Error::from(e)) });
            }
        };

        // Defensive: under the tower `Service` contract `call` is only reached after a
        // `poll_ready` that returned `Ready(Ok)`, which means either a cached sender or a
        // pending connect error existed. If neither holds (contract violation), return an
        // error future rather than panicking.
        let send_request = match &self.send_request {
            Some(sr) => sr.clone(),
            None => {
                return Box::pin(async move {
                    Err(crate::Error::from(
                        "h3 request sender is not ready: poll_ready must return Ready(Ok) before call",
                    ))
                });
            }
        };

        *req.uri_mut() = uri2;
        let executor = self.executor.clone();
        Box::pin(async move {
            crate::client_conn::send_request_inner::<CONN, B>(req, send_request, &executor).await
        })
    }
}
