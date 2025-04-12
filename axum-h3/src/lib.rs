use std::future::Future;

use axum::body::Bytes;
use h3_util::{server::H3Acceptor, server_body::H3IncomingServer};
use hyper::{body::Body, Request, Response};

/// Accept each connection from acceptor, then for each connection
/// accept each request. Spawn a task to handle each request.
async fn serve_inner<AC, F>(
    svc: axum::Router,
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
    tracing::debug!("loop start");
    loop {
        tracing::debug!("loop");
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
                tracing::debug!("cancellation triggered");
                return Ok(());
            }
        };

        let Some(conn) = conn else {
            tracing::debug!("acceptor end of conn");
            return Ok(());
        };

        let h_svc_cp = h_svc.clone();
        tokio::spawn(async move {
            let mut conn = match h3::server::Connection::new(conn).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::debug!("server connection failed: {}", e);
                    return;
                }
            };
            loop {
                let (request, stream) = match conn.accept().await {
                    Ok(req) => match req {
                        Some(r) => r,
                        None => {
                            tracing::debug!("server connection ended:");
                            break;
                        }
                    },
                    Err(e) => {
                        tracing::debug!("server connection accept failed: {}", e);
                        break;
                    }
                };
                let h_svc_cp = h_svc_cp.clone();
                tokio::spawn(async move {
                    if let Err(e) =
                        serve_request::<AC, _, _>(request, stream, h_svc_cp.clone()).await
                    {
                        tracing::debug!("server request failed: {}", e);
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
    tracing::debug!("serving request");
    let (parts, _) = request.into_parts();
    let (mut w, r) = stream.split();

    let req = Request::from_parts(parts, H3IncomingServer::new(r));
    tracing::debug!("serving request call service");
    let res = service.call(req).await?;

    let (res_h, res_b) = res.into_parts();

    // write header
    tracing::debug!("serving request write header");
    w.send_response(Response::from_parts(res_h, ())).await?;

    // write body or trailer.
    h3_util::server_body::send_h3_server_body::<BD, AC::BS>(&mut w, res_b).await?;

    tracing::debug!("serving request end");
    Ok(())
}

pub struct H3Router(axum::Router);

impl H3Router {
    pub fn new(inner: axum::Router) -> Self {
        Self(inner)
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
        serve_inner(self.0, acceptor, signal).await
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
