use std::future::Future;

use h3::server::RequestStream;
use http::{Request, Response};
use hyper::{
    body::{Body, Bytes},
    service::Service,
};

mod client;
pub use client::H3Channel;
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

type H3Conn = h3::server::Connection<h3_quinn::Connection, Bytes>;

/// incomming connections.
pub fn incoming_conn(
    mut acceptor: h3_quinn::Endpoint,
) -> impl futures::Stream<Item = Result<H3Conn, crate::Error>> {
    async_stream::try_stream! {

        let mut tasks = tokio::task::JoinSet::<Result<H3Conn, crate::Error>>::new();

        loop {
            match select_conn(&mut acceptor, &mut tasks).await {
                SelectOutputConn::NewIncoming(incoming) => {
                    tracing::debug!("poll conn new incoming");
                    tasks.spawn(async move {
                        let conn = incoming.await?;
                        let conn = h3::server::Connection::new(h3_quinn::Connection::new(conn))
                            .await
                            .map_err(crate::Error::from)?;
                        tracing::debug!("New incoming conn.");
                        Ok(conn)
                    });
                },
                SelectOutputConn::NewConn(connection) => {
                    yield connection;
                },
                SelectOutputConn::ConnErr(error) => {
                    // continue on error
                    tracing::debug!("conn error, ignore: {}" , error);
                },
                SelectOutputConn::Done => {
                    break;
                },
            }

        }

    }
}

// #[tracing::instrument(level = "debug")]
async fn select_conn(
    incoming: &mut h3_quinn::Endpoint,
    tasks: &mut tokio::task::JoinSet<Result<H3Conn, crate::Error>>,
) -> SelectOutputConn {
    tracing::debug!("select_conn");

    let incoming_stream_future = async {
        tracing::debug!("endpoint waiting accept");
        match incoming.accept().await {
            Some(i) => {
                tracing::debug!("endpoint accept incoming conn");
                SelectOutputConn::NewIncoming(i)
            }
            None => SelectOutputConn::Done, // shutdown.
        }
    };
    if tasks.is_empty() {
        tracing::debug!("endpoint wait for new incoming");
        return incoming_stream_future.await;
    }
    tokio::select! {
        stream = incoming_stream_future => stream,
        accept = tasks.join_next() => {
            match accept.expect("JoinSet should never end") {
                Ok(conn) => {
                    match conn {
                        Ok(conn2) => {
                                SelectOutputConn::NewConn(conn2)
                        },
                        Err(e) => SelectOutputConn::ConnErr(e)
                    }
                },
                Err(e) => SelectOutputConn::ConnErr(e.into()),
            }
        }
    }
}

enum SelectOutputConn {
    NewIncoming(h3_quinn::quinn::Incoming),
    NewConn(H3Conn),
    ConnErr(crate::Error),
    Done,
}

type H3Req = (
    Request<()>,
    RequestStream<h3_quinn::BidiStream<Bytes>, Bytes>,
);

pub fn incoming_req(
    incoming: impl futures::Stream<Item = Result<H3Conn, crate::Error>>,
) -> impl futures::Stream<Item = Result<H3Req, crate::Error>> {
    async_stream::try_stream! {
        let mut incoming = std::pin::pin!(incoming);
            let mut tasks = tokio::task::JoinSet::<Result<Option<(H3Req, H3Conn)>, crate::Error>>::new();
            loop {
                match select_req(&mut incoming, &mut tasks).await {
                SelectOutputReq::NewConn(mut conn) => {
                    tasks.spawn(async move {
                        let req = conn.accept().await.map_err(crate::Error::from)?;
                        tracing::debug!("conn accept h3 req new conn");
                        match req {
                            // got a new request.
                            Some(r) => Ok(Some((r, conn))),
                            // no more request.
                            None => Ok(None),
                        }
                    });
                },
                SelectOutputReq::ConnErr(error) => {
                    tracing::debug!("conn req error: {}", error);
                },
                SelectOutputReq::NewReq(req) => {
                    match req {
                        Some(r) => {
                            let (res, mut conn) = r;
                            // accept the next req on the same conn
                            tasks.spawn(async move {
                                let req = conn.accept().await.map_err(crate::Error::from)?;
                                tracing::debug!("conn accept h3 req");
                                match req {
                                    // got a new request.
                                    Some(r) => Ok(Some((r, conn))),
                                    // no more request.
                                    None => Ok(None),
                                }
                            });
                            yield res;
                        }
                        None => {
                            tracing::debug!("conn has no more reqs.");
                        }
                    }
                },
                SelectOutputReq::ReqErr(error) => {
                    tracing::debug!("incoming_req reqErr: {}", error);
                },
                SelectOutputReq::Done => break,
            }
        }
    }
}

// #[tracing::instrument(level = "debug")]
async fn select_req(
    mut incoming: impl futures::Stream<Item = Result<H3Conn, crate::Error>> + Unpin,
    tasks: &mut tokio::task::JoinSet<Result<Option<(H3Req, H3Conn)>, crate::Error>>,
) -> SelectOutputReq {
    tracing::debug!("select_req");
    use futures_util::StreamExt;
    let incoming_stream_future = async {
        match incoming.next().await {
            Some(Ok(c)) => SelectOutputReq::NewConn(c),
            Some(Err(e)) => SelectOutputReq::ConnErr(e),
            None => SelectOutputReq::Done,
        }
    };
    if tasks.is_empty() {
        return incoming_stream_future.await;
    }
    tokio::select! {
        stream = incoming_stream_future => stream,
        accept = tasks.join_next() => {
            match accept.expect("JoinSet should never end") {
                Ok(req) => {
                    match req {
                        Ok(reqq) => SelectOutputReq::NewReq(reqq),
                        Err(e) => SelectOutputReq::ReqErr(e)
                    }
                },
                Err(e) => SelectOutputReq::ReqErr(e.into()),
            }
        }
    }
}

enum SelectOutputReq {
    NewConn(H3Conn),
    ConnErr(crate::Error),
    NewReq(Option<(H3Req, H3Conn)>),
    ReqErr(crate::Error),
    Done,
}

pub async fn serve_tonic<I, F>(
    svc: tonic::service::Routes,
    mut incoming: I,
    signal: F,
) -> Result<(), crate::Error>
where
    I: tokio_stream::Stream<
        Item = Result<
            (
                Request<()>,
                RequestStream<h3_quinn::BidiStream<Bytes>, Bytes>,
            ),
            crate::Error,
        >,
    >,
    F: Future<Output = ()>,
{
    let svc = svc.prepare();
    let svc = tower::ServiceBuilder::new()
        //.add_extension(Arc::new(ConnInfo { addr, certificates }))
        .service(svc);
    use tower::ServiceExt;
    let h_svc =
        hyper_util::service::TowerToHyperService::new(svc.map_request(|req: http::Request<_>| {
            req.map(tonic::body::boxed::<crate::H3IncomingServer<_, Bytes>>)
        }));

    use tokio_stream::StreamExt;
    let mut sig = std::pin::pin!(signal);
    let mut incoming = std::pin::pin!(incoming);
    tracing::debug!("loop start");
    loop {
        tracing::debug!("loop");
        // get the next stream to run http on
        let (request, stream) = tokio::select! {
            res = incoming.next() => {
                tracing::debug!("tonic server next request");
                match res {
                    Some(s) => {
                        match s{
                            Ok(ss) => {
                                ss
                            },
                            Err(e) => {
                                tracing::debug!("incoming has error, skip. {:?}", e);
                                continue;
                            },
                        }
                    },
                    None => {
                        tracing::debug!("incoming ended");
                        return Ok(());
                    }
                }
            }
            _ = &mut sig =>{
                tracing::debug!("cancellation triggered");
                return Ok(());
            }
        };

        let h_svc_cp = h_svc.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::serve_request(request, stream, h_svc_cp).await {
                tracing::debug!("server request failed: {}", e);
            }
        });
    }
}
