use std::net::SocketAddr;

use futures::future::BoxFuture;
use hyper::body::{Body, Bytes};
use hyper::{Request, Response, Uri};

use crate::client_body::H3IncomingClient;

pub trait H3Connector: Send + 'static + Clone {
    type CONN: h3::quic::Connection<
            Bytes,
            OpenStreams = Self::OS,
            SendStream = Self::SS,
            RecvStream = Self::RS,
        > + Send;
    type OS: h3::quic::OpenStreams<Bytes, BidiStream = Self::BS> + Clone + Send; // Clone is needed for cloning send_request
    type SS: h3::quic::SendStream<Bytes> + Send;
    type RS: h3::quic::RecvStream + Send;
    type BS: h3::quic::BidiStream<Bytes, RecvStream = Self::RS, SendStream = Self::SS> + Send;

    fn connect(
        &self,
    ) -> impl std::future::Future<Output = Result<Self::CONN, crate::Error>> + std::marker::Send;
}

/// Use the host:port portion of the uri and resolve to an sockaddr.
/// If uri host portion is an ip string, then directly use the ip addr without
/// dns lookup.
pub async fn dns_resolve(uri: &Uri) -> std::io::Result<Vec<SocketAddr>> {
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

/// h3 client connection, wrapping inner types for ease of use.
/// All request will be sent to the connection established using the connector.
/// Currently connector can only connect to a fixed server (to support grpc use case).
/// Expand connector to do resolve different server based on uri can be added in future.
pub struct H3Connection<C, B>
where
    C: H3Connector,
    B: Body + Send + 'static + Unpin,
    B::Data: Send,
    B::Error: Into<crate::Error>,
{
    #[allow(clippy::type_complexity)]
    inner:
        tower::util::BoxService<Request<B>, Response<H3IncomingClient<C::RS, Bytes>>, crate::Error>,
}

impl<C, B> H3Connection<C, B>
where
    C: H3Connector,
    B: Body + Send + 'static + Unpin,
    B::Data: Send,
    B::Error: Into<crate::Error> + Send,
{
    pub fn new(connector: C, uri: Uri) -> Self {
        let sender = exp2::RequestSender::new(connector, uri);
        Self {
            inner: tower::util::BoxService::new(sender),
        }
    }
}

impl<C, B> tower::Service<Request<B>> for H3Connection<C, B>
where
    C: H3Connector,
    B: Body + Send + 'static + Unpin,
    B::Data: Send,
    B::Error: Into<crate::Error>,
{
    type Response = Response<H3IncomingClient<C::RS, Bytes>>;
    type Error = crate::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        tower::Service::poll_ready(&mut self.inner, cx)
    }

    fn call(&mut self, req: Request<B>) -> Self::Future {
        self.inner.call(req)
    }
}

// /// Client service that includes cache and reconnection.
// pub struct ClientService<C>
// where
//     C: H3Connector,
// {
//     inner: CacheSendRequestService<C>,
//     uri: Uri,
// }

// impl<C, B> tower::Service<Request<B>> for ClientService<C>
// where
//     C: H3Connector,
//     B: Body + Send + 'static + Unpin,
//     B::Data: Send,
//     B::Error: Into<crate::Error> + Send,
// {
//     type Response = Response<H3IncomingClient<C::RS, Bytes>>;

//     type Error = crate::Error;

//     type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

//     fn poll_ready(
//         &mut self,
//         cx: &mut std::task::Context<'_>,
//     ) -> std::task::Poll<Result<(), Self::Error>> {
//         self.inner.poll_ready(cx)
//     }

//     fn call(&mut self, mut req: Request<B>) -> Self::Future {
//         let uri = &self.uri;
//         // fix up uri with full uri.
//         let uri2 = Uri::builder()
//             .scheme(uri.scheme().unwrap().clone())
//             .authority(uri.authority().unwrap().clone())
//             .path_and_query(req.uri().path_and_query().unwrap().clone())
//             .build()
//             .unwrap();
//         *req.uri_mut() = uri2;

//         let fut = self.inner.call(());
//         Box::pin(async move {
//             let mut send_request = fut.await?;
//             send_request.call(req).await
//         })
//     }
// }

/// http3 client.
/// Note the client does not do dns resolve but blindly sends requests
/// using connections created by the connector.
pub struct H3Client<C, B>
where
    C: H3Connector,
    B: Body + Send + 'static + Unpin,
    B::Data: Send,
    B::Error: Into<crate::Error> + Send,
{
    channel: H3Connection<C, B>,
}

impl<C, B> H3Client<C, B>
where
    C: H3Connector,
    B: Body + Send + 'static + Unpin,
    B::Data: Send,
    B::Error: Into<crate::Error> + Send,
{
    pub fn new(inner: H3Connection<C, B>) -> Self {
        Self { channel: inner }
    }

    pub async fn send(
        &mut self,
        req: Request<B>,
    ) -> Result<Response<H3IncomingClient<C::RS, Bytes>>, crate::Error> {
        use tower::Service;
        // wait for ready
        futures::future::poll_fn(|cx| self.channel.poll_ready(cx)).await?;
        self.channel.call(req).await
    }
}

pub mod exp2 {
    use futures::{FutureExt, future::BoxFuture};
    use hyper::{
        Request, Response, Uri,
        body::{Body, Bytes},
    };

    use crate::{client::H3Connector, client_body::H3IncomingClient};
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

        fn poll_ready(
            &mut self,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            if let Some(rx) = &mut self.driver_rx {
                // check if the driver is still running
                match rx.try_recv() {
                    Ok(()) => {
                        tracing::debug!("driver is closed, reconnecting.");
                        self.send_request = None;
                        self.driver_rx = None;
                    }
                    Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                        // driver is still running
                    }
                    Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                        tracing::debug!("driver is closed, reconnecting.");
                        self.send_request = None;
                        self.driver_rx = None;
                    }
                }
            }

            // ready for send.
            if self.send_request.is_some() {
                tracing::debug!("exp poll_ready cache hit.");
                assert!(self.make_send_request_fut.is_none());
                assert!(self.driver_rx.is_some());
                return std::task::Poll::Ready(Ok(()));
            }

            if self.make_send_request_fut.is_none() {
                // start the driver in the background
                let conn = self.conn.clone();

                self.make_send_request_fut = Some(Box::pin(async move {
                    let conn = conn.connect().await?;
                    let (mut driver, send_request) = h3::client::new(conn).await?;
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    tokio::spawn(async move {
                        let res = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
                        tracing::debug!("h3 driver ended: {res:?}");
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
            Box::pin(async move {
                crate::connection::send_request_inner::<CONN, B>(req, send_request).await
            })
        }
    }
}
