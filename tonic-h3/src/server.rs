use http::{Request, Response};
use hyper::{
    body::{Body, Bytes},
    service::Service,
};
use std::future::Future;

use crate::server_body::H3IncomingServer;

pub trait H3Acceptor {
    type CONN: h3::quic::Connection<
            Bytes,
            OpenStreams = Self::OS,
            SendStream = Self::SS,
            RecvStream = Self::RS,
            OpenError = Self::OE,
            BidiStream = Self::BS,
        > + Send
        + 'static;
    type OS: h3::quic::OpenStreams<Bytes, OpenError = Self::OE, BidiStream = Self::BS>
        + Clone
        + Send; // Clone is needed for cloning send_request
    type SS: h3::quic::SendStream<Bytes> + Send;
    type RS: h3::quic::RecvStream + Send + 'static;
    type OE: Into<Box<dyn std::error::Error>> + Send;
    type BS: h3::quic::BidiStream<Bytes, RecvStream = Self::RS, SendStream = Self::SS>
        + Send
        + 'static;

    fn accept(
        &mut self,
    ) -> impl std::future::Future<Output = Result<Option<Self::CONN>, crate::Error>> + std::marker::Send;
}

/// Accept each connection from acceptor, then for each connection
/// accept each request. Spawn a task to handle each request.
async fn serve_tonic_inner<AC, F>(
    svc: tonic::service::Routes,
    mut acceptor: AC,
    signal: F,
) -> Result<(), crate::Error>
where
    AC: H3Acceptor,
    F: Future<Output = ()>,
{
    let svc = svc.prepare();
    let svc = tower::ServiceBuilder::new()
        //.add_extension(Arc::new(ConnInfo { addr, certificates }))
        .service(svc);
    use tower::ServiceExt;
    let h_svc =
        hyper_util::service::TowerToHyperService::new(svc.map_request(|req: http::Request<_>| {
            req.map(tonic::body::boxed::<crate::H3IncomingServer<AC::RS, Bytes>>)
        }));

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

pub async fn serve_request<AC, SVC, BD>(
    request: Request<()>,
    stream: h3::server::RequestStream<
        <<AC as H3Acceptor>::CONN as h3::quic::OpenStreams<hyper::body::Bytes>>::BidiStream,
        Bytes,
    >,
    service: SVC,
) -> Result<(), crate::Error>
where
    AC: H3Acceptor,
    SVC: Service<
        Request<H3IncomingServer<AC::RS, Bytes>>,
        Response = Response<BD>,
        Error = crate::Error,
    >,
    SVC::Future: 'static,
    BD: Body + 'static,
    BD::Error: Into<crate::Error>,
    <BD as Body>::Error: Into<crate::Error> + std::error::Error + Send + Sync,
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
    crate::server_body::send_h3_server_body::<BD, AC::BS>(&mut w, res_b).await?;

    tracing::debug!("serving request end");
    Ok(())
}

/// Router for running tonic services.
///
pub struct H3Router(tonic::service::Routes);

/// Converts from router.
/// TODO: maybe more options from tonic routers needs to be applied here.
impl From<tonic::transport::server::Router> for H3Router {
    fn from(value: tonic::transport::server::Router) -> Self {
        Self::new(value.into_service())
    }
}

impl H3Router {
    pub fn new(routes: tonic::service::Routes) -> Self {
        Self(routes)
    }
}

impl H3Router {
    /// Runs the service on acceptor until shutdown.
    pub async fn serve_with_shutdown<AC, F>(
        self,
        acceptor: AC,
        signal: F,
    ) -> Result<(), crate::Error>
    where
        AC: H3Acceptor,
        F: Future<Output = ()>,
    {
        serve_tonic_inner(self.0, acceptor, signal).await
    }

    /// Runs all services on acceptor
    pub async fn serve<AC>(self, acceptor: AC) -> Result<(), crate::Error>
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
