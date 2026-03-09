use crate::{client::H3Connector, executor::SharedExec};
use futures::{FutureExt, future::BoxFuture};
use hyper::{
    Request, Response, Uri,
    body::{Body, Bytes},
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
    uri: Uri,
    executor: SharedExec,
}

impl<CONN> RequestSender<CONN>
where
    CONN: H3Connector,
{
    pub fn new(conn: CONN, uri: Uri) -> Self {
        Self {
            conn,
            send_request: None,
            driver_rx: None,
            make_send_request_fut: None,
            uri,
            executor: SharedExec::tokio(), // TODO: expose the executor for user.
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
                Err(e) => Err(e),
            })
    }

    /// Gets the send_request from the cache and send the request.
    fn call(&mut self, mut req: Request<B>) -> Self::Future {
        let send_request = self.send_request.clone().unwrap();

        // replace the uri
        let uri = &self.uri;
        // fix up uri with full uri.
        let uri2 = Uri::builder()
            .scheme(uri.scheme().unwrap().clone())
            .authority(uri.authority().unwrap().clone())
            .path_and_query(req.uri().path_and_query().unwrap().clone())
            .build()
            .unwrap();
        *req.uri_mut() = uri2;
        let executor = self.executor.clone();
        Box::pin(async move {
            crate::client_conn::send_request_inner::<CONN, B>(req, send_request, &executor).await
        })
    }
}
