//! HTTP/3 server adapter for [axum], built on the [h3] crate.
//!
//! Developed as part of [`tonic-h3`]. This crate serves an [`axum::Router`]
//! over HTTP/3 by accepting QUIC connections from any [`h3-util`] backend
//! (via the [`h3_util::server::H3Acceptor`] trait) and dispatching each request
//! to the router.
//!
//! The entry point is [`H3Router`], which wraps an [`axum::Router`] and drives
//! it with [`H3Router::serve`] or [`H3Router::serve_with_shutdown`]. Each
//! accepted connection is served on its own task, and each request on that
//! connection is spawned as a separate task using a [`SharedExec`] executor
//! (Tokio by default).
//!
//! [`h3-util`] itself selects the concrete QUIC backend behind
//! [`h3_util::server::H3Acceptor`] via cargo features (`quinn`, `msquic`,
//! `s2n-quic`, `quiche`). **Only the `quinn` backend is supported for
//! production use;** the others are experimental.
//!
//! # Example
//!
//! The server is generic over the QUIC backend — provide any
//! [`h3_util::server::H3Acceptor`]:
//!
//! ```no_run
//! use axum::{routing::get, Router};
//! use axum_h3::H3Router;
//! use h3_util::server::H3Acceptor;
//!
//! async fn serve<A: H3Acceptor>(acceptor: A) -> Result<(), h3_util::Error> {
//!     let app = Router::new().route("/", get(|| async { "hello over h3" }));
//!     H3Router::from(app).serve(acceptor).await
//! }
//! ```
//!
//! [axum]: https://github.com/tokio-rs/axum
//! [h3]: https://github.com/hyperium/h3
//! [`tonic-h3`]: https://github.com/youyuanwu/tonic-h3
//! [`h3-util`]: https://crates.io/crates/h3-util
//! [`SharedExec`]: h3_util::executor::SharedExec

use std::future::Future;

use axum::body::Bytes;
use h3_util::{server::H3Acceptor, server_body::H3IncomingServer};
use hyper::{Request, Response, body::Body, rt::Executor};

fn is_benign_connection_close(error: &h3::error::ConnectionError) -> bool {
    if error.is_h3_no_error() {
        return true;
    }

    // h3-quinn 0.0.10 erases QUIC transport close codes into an `Undefined`
    // h3 error. Treat only QUIC NO_ERROR as benign; other transport and
    // application errors remain warnings.
    error.to_string()
        == "Remote error: Error undefined by h3: aborted by peer: the connection is being closed abruptly in the absence of any error"
}

/// Accept each connection from acceptor, then for each connection
/// accept each request. Spawn a task to handle each request.
async fn serve_inner<AC, F>(
    svc: axum::Router,
    executor: &h3_util::executor::SharedExec,
    mut acceptor: AC,
    signal: F,
) -> Result<(), h3_util::Error>
where
    AC: H3Acceptor,
    F: Future<Output = ()>,
{
    let svc = tower::ServiceBuilder::new()
        //.add_extension(Arc::new(ConnInfo { addr, certificates }))
        .service(svc);

    // TODO: tonic body is wrapped? Is it for error to status conversion?
    // use tower::ServiceExt;
    // let h_svc =
    //     hyper_util::service::TowerToHyperService::new(svc.map_request(|req: http::Request<_>| {
    //         req.map(tonic::body::boxed::<crate::H3IncomingServer<AC::RS, Bytes>>)
    //     }));

    let h_svc = hyper_util::service::TowerToHyperService::new(svc);

    let mut sig = std::pin::pin!(signal);
    tracing::trace!("loop start");
    loop {
        tracing::trace!("loop");
        // get the next stream to run http on
        let conn = tokio::select! {
            res = acceptor.accept() =>{
                match res{
                Ok(x) => x,
                Err(e) => {
                    tracing::error!("accept error : {e}");
                    return Err(e);
                }
            }
            }
            _ = &mut sig =>{
                tracing::trace!("cancellation triggered");
                return Ok(());
            }
        };

        let Some(conn) = conn else {
            tracing::trace!("acceptor end of conn");
            return Ok(());
        };

        // server each connection in the background
        let h_svc_cp = h_svc.clone();
        let executor_clone = executor.clone();
        executor.execute(async move {
            let mut conn = match h3::server::Connection::new(conn).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("server connection failed: {}", e);
                    return;
                }
            };
            loop {
                let resolver = match conn.accept().await {
                    Ok(req) => match req {
                        Some(r) => r,
                        None => {
                            tracing::trace!("server connection ended:");
                            break;
                        }
                    },
                    Err(e) => {
                        if is_benign_connection_close(&e) {
                            tracing::trace!("server connection ended without error: {e}");
                        } else {
                            tracing::warn!("server connection accept failed: {}", e);
                        }
                        break;
                    }
                };
                let h_svc_cp = h_svc_cp.clone();
                executor_clone.execute(async move {
                    let (req, stream) = match resolver.resolve_request().await {
                        Ok(req) => req,
                        Err(e) => {
                            tracing::warn!("fail resolve request {e:#?}");
                            return;
                        }
                    };
                    if let Err(e) = serve_request::<AC, _, _>(req, stream, h_svc_cp.clone()).await {
                        tracing::warn!("server request failed: {}", e);
                    }
                });
            }
        });
    }
}

async fn serve_request<AC, SVC, BD>(
    request: Request<()>,
    stream: h3::server::RequestStream<
        <<AC as H3Acceptor>::CONN as h3::quic::OpenStreams<Bytes>>::BidiStream,
        Bytes,
    >,
    service: SVC,
) -> Result<(), h3_util::Error>
where
    AC: H3Acceptor,
    SVC: hyper::service::Service<
            Request<H3IncomingServer<AC::RS, Bytes>>,
            Response = Response<BD>,
            Error = std::convert::Infallible,
        >,
    SVC::Future: 'static,
    BD: Body + 'static,
    BD::Error: Into<h3_util::Error>,
    <BD as Body>::Error: Into<h3_util::Error> + std::error::Error + Send + Sync,
    <BD as Body>::Data: Send + Sync,
{
    tracing::trace!("serving request");
    let (parts, _) = request.into_parts();
    let (mut w, r) = stream.split();

    let req = Request::from_parts(parts, H3IncomingServer::new(r));
    tracing::trace!("serving request call service");
    let res = service.call(req).await?;

    let (res_h, res_b) = res.into_parts();

    // write header
    tracing::trace!("serving request write header");
    w.send_response(Response::from_parts(res_h, ())).await?;

    // write body or trailer.
    h3_util::server_body::send_h3_server_body::<BD, AC::BS>(&mut w, res_b).await?;

    tracing::trace!("serving request end");
    Ok(())
}

pub struct H3Router {
    inner: axum::Router,
    executor: h3_util::executor::SharedExec, // expose this for the user.
}

impl H3Router {
    pub fn new(inner: axum::Router) -> Self {
        Self {
            inner,
            executor: h3_util::executor::SharedExec::tokio(),
        }
    }
}

impl From<axum::Router> for H3Router {
    fn from(value: axum::Router) -> Self {
        Self::new(value)
    }
}

impl H3Router {
    /// Runs the service on acceptor until shutdown.
    pub async fn serve_with_shutdown<AC, F>(
        self,
        acceptor: AC,
        signal: F,
    ) -> Result<(), h3_util::Error>
    where
        AC: H3Acceptor,
        F: Future<Output = ()>,
    {
        serve_inner(self.inner, &self.executor, acceptor, signal).await
    }

    /// Runs all services on acceptor
    pub async fn serve<AC>(self, acceptor: AC) -> Result<(), h3_util::Error>
    where
        AC: H3Acceptor,
    {
        self.serve_with_shutdown(acceptor, async {
            // never returns
            futures::future::pending().await
        })
        .await
    }
}
