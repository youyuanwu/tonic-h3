use std::{net::SocketAddr, path::PathBuf};

use futures::StreamExt;
use h3_util::quiche_h3::tokio_quiche::{
    ConnectionParams, ServerH3Driver,
    http3::settings::Http3Settings,
    listen,
    metrics::DefaultMetrics,
    quic::SimpleConnectionIdGenerator,
    settings::{CertificateKind, Hooks, QuicSettings, TlsCertificatePaths},
};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

use crate::quiche::server::{Server, service_fn};

#[tokio::test]
async fn test_quiche_h3() {
    crate::try_setup_tracing();

    let (cert_path, key_path) = crate::cert_gen::make_test_cert_files("test_quiche_h3", true);
    // Test implementation goes here

    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();

    let socket = UdpSocket::bind(addr)
        .await
        .expect("UDP socket should be bindable");
    let local_addr = socket.local_addr().unwrap();

    let token = CancellationToken::new();

    let svh = tokio::spawn({
        let token = token.clone();
        async move {
            run_server(token, socket, cert_path, key_path).await;
        }
    });

    // Use quinn to send a request.
    quinn_client::run_quinn_client(local_addr).await;
    token.cancel();
    svh.await.unwrap();
}

mod quinn_client {
    use axum::body::Bytes;
    use http::{Request, Uri};

    pub async fn run_quinn_client(listen_addr: std::net::SocketAddr) {
        use super::body::STREAM_BYTES;
        let uri: Uri = format!("https://{listen_addr}{STREAM_BYTES}")
            .parse()
            .unwrap();

        let client_endpoint = crate::make_test_quinn_client_endpoint();
        // quinn client test
        {
            // client drop is required to end connection. drive will end after connection end
            let cc = h3_util::quinn::H3QuinnConnector::new(
                uri.clone(),
                "localhost".to_string(),
                client_endpoint.clone(),
            );
            let channel = h3_util::client::H3Connection::new(cc, uri.clone());
            let mut client = h3_util::client::H3Client::new(channel);
            let req = Request::builder()
                .method("GET")
                .uri(uri)
                .body(http_body_util::Empty::<Bytes>::new())
                .unwrap();
            let resp = client.send(req).await.unwrap();
            use http_body_util::BodyExt;
            let data = resp.into_body().collect().await.unwrap().to_bytes();
            println!("Resp: {data:?}");
        }
    }
}

pub async fn run_server(
    token: CancellationToken,
    socket: UdpSocket,
    cert_path: PathBuf,
    key_path: PathBuf,
) {
    let mut listeners = listen(
        [socket],
        ConnectionParams::new_server(
            QuicSettings::default(),
            TlsCertificatePaths {
                cert: cert_path.as_path().to_str().unwrap(),
                private_key: key_path.as_path().to_str().unwrap(),
                kind: CertificateKind::X509,
            },
            Hooks::default(),
        ),
        SimpleConnectionIdGenerator,
        DefaultMetrics,
    )
    .expect("should be able to create a listener from a UDP socket");

    // Pull connections off the socket and serve them.
    let accepted_connection_stream = &mut listeners[0];
    loop {
        tokio::select! {
            _ = token.cancelled() => {
              tracing::info!("cancelling");
              break;
            }
            conn = accepted_connection_stream.next() => {
              match conn{
                Some(Ok(conn)) => {
                  tracing::info!("received new connection!");

                  // Create an `H3Driver` to serve the connection.
                  let (driver, mut controller) = ServerH3Driver::new(Http3Settings::default());

                  conn.start(driver);
                                // Spawn a task to process the new connection.
                  tokio::spawn(async move {
                      let mut server = Server::new(service_fn);

                      // tokio-quiche will send events to the `H3Controller`'s
                      // receiver for processing.
                      let _ = server
                          .serve_connection(controller.event_receiver_mut())
                          .await;
                  });
                }
                Some(Err(e)) => {
                  tracing::error!("could not create connection: {e:?}");
                }
                None => {
                  tracing::info!("listener closed");
                  break;
                }
              }
          }
        }
    }
}

mod server {
    use std::sync::Arc;

    use futures::SinkExt;
    use h3_util::quiche_h3::tokio_quiche::{
        QuicResult,
        http3::driver::{
            H3Event, IncomingH3Headers, OutboundFrame, OutboundFrameSender, ServerEventStream,
            ServerH3Event,
        },
        quiche::h3::{Header, NameValue},
    };
    use http::{HeaderName, HeaderValue, Request, Response, Uri, uri::PathAndQuery};

    use crate::quiche::body::ExampleBody;

    /// A simple [service function].
    ///
    /// If the request's path follows the form `/stream-bytes/<num_bytes>`, the
    /// response will come with a body that is `num_bytes` long.
    ///
    /// For example, `https://test.com/stream-bytes/57` will return a 200 response with a body that is
    /// 57 bytes long.
    ///
    /// [service function]: https://docs.rs/hyper/latest/hyper/service/index.html
    pub async fn service_fn(req: Request<()>) -> Response<ExampleBody> {
        let body = ExampleBody::new(&req);
        Response::builder().status(200).body(body).unwrap()
    }

    /// A basic asynchronous HTTP/3 server served by tokio-quiche.
    ///
    /// Note that this is simply an example, and **should not be run in
    /// production**. This merely shows how one could use tokio-quiche to write an
    /// HTTP/3 server.
    pub struct Server<S, R>
    where
        S: Fn(Request<()>) -> R + Send + Sync + 'static,
    {
        service_fn: Arc<S>,
    }

    impl<S, R> Server<S, R>
    where
        S: Fn(Request<()>) -> R + Send + Sync + 'static,
        R: Future<Output = Response<ExampleBody>> + Send + 'static,
    {
        /// Create the server by registering a [service function].
        ///
        /// [service function]: https://docs.rs/hyper/latest/hyper/service/index.html
        pub fn new(service_fn: S) -> Self {
            Server {
                service_fn: Arc::new(service_fn),
            }
        }

        /// Serve the connection.
        ///
        /// The [`Server`] will listen for [`ServerH3Event`]s, process them, and
        /// send response data back to tokio-quiche for quiche-side processing
        /// and flushing.
        ///
        /// tokio-quiche's `H3Driver` emits these events in response to data that
        /// comes off the underlying socket.
        pub async fn serve_connection(
            &mut self,
            h3_event_receiver: &mut ServerEventStream,
        ) -> QuicResult<()> {
            loop {
                match h3_event_receiver.recv().await {
                    Some(event) => self.handle_h3_event(event).await?,
                    None => return Ok(()), /* The sender was dropped, implying
                                            * connection was terminated */
                }
            }
        }

        /// Handle a [`ServerH3Event`].
        ///
        /// For simplicity's sake, we only handle a couple of events here.
        // TODO(evanrittenhouse): support POST requests
        async fn handle_h3_event(&mut self, event: ServerH3Event) -> QuicResult<()> {
            let ServerH3Event::Core(event) = event;

            match event {
                // Received an explicit connection level error. Not much to do here.
                H3Event::ConnectionError(err) => QuicResult::Err(Box::new(err)),

                // The connection has shutdown.
                H3Event::ConnectionShutdown(err) => {
                    let err = match err {
                        Some(err) => Box::new(err),
                        None => Box::new(h3_util::quiche_h3::tokio_quiche::quiche::h3::Error::Done)
                            as h3_util::quiche_h3::tokio_quiche::BoxError,
                    };

                    QuicResult::Err(err)
                }

                // Received headers for a new stream from the H3Driver.
                H3Event::IncomingHeaders(headers) => {
                    self.handle_incoming_headers(headers).await;

                    Ok(())
                }

                _ => {
                    tracing::info!("received unhandled event: {event:?}");
                    Ok(())
                }
            }
        }

        /// Respond to the request corresponding to the [`IncomingH3Headers`].
        ///
        /// This function transforms the incoming headers into a [`Request`],
        /// creating the proper response body if requested. It then spawns a
        /// Tokio task which calls the `service_fn` on the [`Request`].
        async fn handle_incoming_headers(&mut self, headers: IncomingH3Headers) {
            tracing::info!("received headers: {:?}", &headers);

            let IncomingH3Headers {
                headers: list,
                send: mut frame_sender,
                ..
            } = headers;

            let Ok((uri_builder, req_builder)) = convert_headers(list) else {
                Self::end_stream(&mut frame_sender).await;
                return;
            };

            let uri = uri_builder.build().expect("can't build uri");
            let req = req_builder.uri(uri).body(()).expect("can't build request");

            let service_fn = Arc::clone(&self.service_fn);

            tokio::spawn(async move {
                Self::handle_request(service_fn, req, frame_sender).await;
            });
        }

        /// Get a [`Response`] for a [`Request`] by calling the `service_fn`.
        ///
        /// The `frame_sender` parameter connects back to tokio-quiche `H3Driver`,
        /// which communicates the data back to quiche.
        async fn handle_request(
            service_fn: Arc<S>,
            req: Request<()>,
            mut frame_sender: OutboundFrameSender,
        ) {
            let res = service_fn(req).await;

            // Convert the result of the `service_fn` into headers and a body which
            // can be transmitted to tokio-quiche.
            let (h3_headers, body) = convert_response(res);
            let _ = frame_sender
                .send(OutboundFrame::Headers(h3_headers, None))
                .await;

            body.send(frame_sender).await;
        }

        /// End the stream.
        ///
        /// This will send  STOP_SENDING and RESET_STREAM frames to the client.
        async fn end_stream(frame_sender: &mut OutboundFrameSender) {
            let _ = frame_sender.send(OutboundFrame::PeerStreamError).await;
        }
    }

    /// Convert a list of [Header]s into a request object which can be processed by
    /// the [`Server`].
    ///
    /// This serves as an example, and does not ensure HTTP semantics by any means.
    /// For example, this will not ensure that required pseudo-headers are present,
    /// nor will it detect duplicate pseudo-headers.
    fn convert_headers(
        headers: Vec<Header>,
    ) -> QuicResult<(http::uri::Builder, http::request::Builder)> {
        let mut req_builder = Request::builder();
        let mut uri_builder = Uri::builder();

        for header in headers {
            let name = header.name();
            let value = header.value();

            let Some(first) = name
                .iter()
                .next()
                .and_then(|f| std::char::from_u32(*f as u32))
            else {
                tracing::warn!("received header with no or invalid first character");
                continue;
            };

            if first == ':' {
                match name {
                    b":method" => {
                        req_builder = req_builder.method(value);
                    }
                    b":scheme" => {
                        uri_builder = uri_builder.scheme(value);
                    }
                    b":authority" => {
                        let host = HeaderValue::from_bytes(value)?;
                        uri_builder = uri_builder.authority(host.as_bytes());
                        req_builder.headers_mut().map(|h| h.insert("host", host));
                    }
                    b":path" => {
                        let path = PathAndQuery::try_from(value)?;
                        uri_builder = uri_builder.path_and_query(path);
                    }
                    _ => {
                        tracing::warn!("received unknown pseudo-header: {name:?}");
                    }
                }
            } else {
                req_builder.headers_mut().map(|h| {
                    h.insert(
                        HeaderName::from_bytes(name).expect("invalid header name"),
                        HeaderValue::from_bytes(value).expect("invalid header value"),
                    )
                });
            }
        }

        Ok((uri_builder, req_builder))
    }

    /// Convert a [`Response`] into headers and a body, readable by tokio-quiche.
    fn convert_response<B>(res: Response<B>) -> (Vec<Header>, B) {
        let mut h3_headers = vec![Header::new(b":status", res.status().as_str().as_bytes())];

        for (name, value) in res.headers().iter() {
            h3_headers.push(Header::new(name.as_ref(), value.as_bytes()));
        }

        (h3_headers, res.into_body())
    }
}

mod body {
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };

    use axum::body::Bytes;
    use futures::{SinkExt, StreamExt};
    use h3_util::quiche_h3::tokio_quiche::http3::driver::{OutboundFrame, OutboundFrameSender};
    use http::Request;
    use http_body_util::BodyDataStream;
    use hyper::body::{Body, Frame, SizeHint};

    pub const STREAM_BYTES: &str = "/stream-bytes/";

    use h3_util::quiche_h3::tokio_quiche::buf_factory::BufFactory as BufFactoryImpl;

    /// An extremely simply response body, for example purposes only.
    pub struct ExampleBody {
        remaining: usize,
        chunk: Bytes,
    }

    impl ExampleBody {
        /// Create a new body.
        ///
        /// This takes the request's path and sees if the client has requested a
        /// body back. If so, the body with the requested size is created.
        ///
        /// We statically allocate memory at the beginning to avoid allocation costs
        /// when sending the body back.
        pub fn new(req: &Request<()>) -> Self {
            const DUMMY_CONTENT: u8 = 0x57;
            const CHUNK_SIZE: usize = 1024 * 1024; // 1MB
            static CHUNK_DATA: [u8; CHUNK_SIZE] = [DUMMY_CONTENT; CHUNK_SIZE];

            let req_path = req.uri().path();
            let size = if req_path.starts_with(STREAM_BYTES) {
                req_path
                    .split("/")
                    .last()
                    .and_then(|last| last.parse::<usize>().ok())
                    .unwrap_or(0)
            } else {
                0
            };

            Self {
                remaining: size,
                chunk: Bytes::from_static(&CHUNK_DATA),
            }
        }

        /// Use the `frame_sender` to send DATA frames to tokio-quiche.
        ///
        /// The sender is paired with the underlying tokio-quiche `H3Driver`.
        pub(crate) async fn send(self, mut frame_sender: OutboundFrameSender) -> Option<()> {
            let mut body_stream = BodyDataStream::new(self);

            while let Some(chunk) = body_stream.next().await {
                match chunk {
                    Ok(chunk) => {
                        for chunk in chunk.chunks(BufFactoryImpl::MAX_BUF_SIZE) {
                            let chunk =
                                OutboundFrame::body(BufFactoryImpl::buf_from_slice(chunk), false);
                            frame_sender.send(chunk).await.ok()?;
                        }
                    }
                    Err(error) => {
                        tracing::error!(
                            "Received error when sending or receiving HTTP body: {error:?}"
                        );

                        let fin_chunk = OutboundFrame::PeerStreamError;
                        frame_sender.send(fin_chunk).await.ok()?;

                        return None;
                    }
                }
            }

            frame_sender
                .send(OutboundFrame::Body(BufFactoryImpl::get_empty_buf(), true))
                .await
                .ok()?;

            Some(())
        }
    }

    impl Body for ExampleBody {
        type Data = Bytes;
        type Error = Box<dyn std::error::Error + Send + Sync>;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            Poll::Ready(match self.remaining {
                0 => None,

                _ => {
                    let chunk_len = std::cmp::min(self.remaining, self.chunk.len());

                    self.remaining -= chunk_len;

                    // Borrowing the slice of data and avoid copy.
                    Some(Ok(Frame::data(self.chunk.slice(..chunk_len))))
                }
            })
        }

        fn size_hint(&self) -> SizeHint {
            SizeHint::with_exact(self.remaining as u64)
        }
    }
}
