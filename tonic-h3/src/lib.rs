use std::future::Future;

use h3::server::RequestStream;
use http::{Request, Response};
use hyper::{
    body::{Body, Bytes},
    service::Service,
};

mod client;
pub mod quinn;
pub mod server;
pub use client::{dns_resolve, H3Channel, H3Connector};
use server::H3Acceptor;
use server_body::H3IncomingServer;

pub mod client_body;
pub mod connection;
pub mod server_body;

pub type Error = Box<dyn std::error::Error + Send + Sync>;

pub async fn serve_request<S, SVC, BD>(
    request: Request<()>,
    stream: RequestStream<S, Bytes>,
    service: SVC,
) -> Result<(), crate::Error>
where
    SVC: Service<
        Request<H3IncomingServer<S::RecvStream, Bytes>>,
        Response = Response<BD>,
        Error = crate::Error,
    >,
    SVC::Future: 'static,
    BD: Body + 'static,
    BD::Error: Into<crate::Error>,
    <BD as Body>::Error: Into<crate::Error> + std::error::Error + Send + Sync,
    <BD as Body>::Data: Send + Sync,
    S: h3::quic::BidiStream<Bytes>,
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
    server_body::send_h3_server_body::<BD, S>(&mut w, res_b).await?;

    w.finish().await?;

    tracing::debug!("serving request end");
    Ok(())
}

pub async fn serve_tonic2<AC, F>(
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
                        crate::serve_request2::<AC, _, _>(request, stream, h_svc_cp.clone()).await
                    {
                        tracing::debug!("server request failed: {}", e);
                    }
                });
            }
        });
    }
}

pub async fn serve_request2<AC, SVC, BD>(
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
    server_body::send_h3_server_body::<BD, AC::BS>(&mut w, res_b).await?;

    tracing::debug!("serving request end");
    Ok(())
}
