use h3::client::RequestStream;
use http::Uri;
use http_body_util::BodyExt;
use hyper::body::{Buf, Bytes};

pub struct H3IncomingClient<S, B>
where
    B: Buf,
    S: h3::quic::RecvStream,
{
    s: RequestStream<S, B>,
    data_done: bool,
}

impl<S, B> H3IncomingClient<S, B>
where
    B: Buf,
    S: h3::quic::RecvStream,
{
    fn new(s: RequestStream<S, B>) -> Self {
        Self {
            s,
            data_done: false,
        }
    }
}

impl<S, B> tonic::transport::Body for H3IncomingClient<S, B>
where
    B: Buf,
    S: h3::quic::RecvStream,
{
    type Data = hyper::body::Bytes;

    type Error = h3::Error;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        if !self.data_done {
            match futures_util::ready!(self.s.poll_recv_data(cx)) {
                Ok(data_opt) => match data_opt {
                    Some(mut data) => {
                        let f = hyper::body::Frame::data(data.copy_to_bytes(data.remaining()));
                        std::task::Poll::Ready(Some(Ok(f)))
                    }
                    None => {
                        self.data_done = true;
                        // try again for trailers
                        cx.waker().wake_by_ref();
                        std::task::Poll::Pending
                    }
                },
                Err(e) => std::task::Poll::Ready(Some(Err(e))),
            }
        } else {
            // TODO: need poll trailers api.
            match futures_util::ready!(self.s.poll_recv_trailers(cx))? {
                Some(tr) => std::task::Poll::Ready(Some(Ok(hyper::body::Frame::trailers(tr)))),
                None => std::task::Poll::Ready(None),
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        false
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        hyper::body::SizeHint::default()
    }
}

pub fn channel_h3(
    send_request: h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>,
    uri: Uri,
) -> impl tonic::client::GrpcService<
    tonic::body::BoxBody,
    Error = crate::Error,
    ResponseBody = H3IncomingClient<h3_quinn::RecvStream, Bytes>,
> {
    tower::service_fn(move |req: hyper::Request<tonic::body::BoxBody>| {
        let mut send_request = send_request.clone();
        let uri = uri.clone();
        async move {
            let (parts, mut body) = req.into_parts();
            let mut head_req = hyper::Request::from_parts(parts, ());
            // send header
            tracing::debug!("sending h3 req header: {:?}", head_req);

            // need to inject uri with full uri and authority
            let uri2 = Uri::builder()
                .scheme(uri.scheme().unwrap().clone())
                .authority(uri.authority().unwrap().clone())
                .path_and_query(head_req.uri().path_and_query().unwrap().clone())
                .build()
                .unwrap();

            *head_req.uri_mut() = uri2;

            // send header.
            let stream = send_request.send_request(head_req).await?;

            let (mut w, mut r) = stream.split();
            // send body in backgound
            let send_body = async move {
                loop {
                    match body.frame().await {
                        Some(Ok(f)) => {
                            if f.is_data() {
                                tracing::debug!("sending body :{:?}", f);
                                w.send_data(f.into_data().unwrap()).await.inspect_err(|e| {
                                    tracing::debug!("w send_data error: {}", e);
                                })?;
                            } else if f.is_trailers() {
                                tracing::debug!("sending trailer :{:?}", f);
                                w.send_trailers(f.into_trailers().unwrap())
                                    .await
                                    .inspect_err(|e| {
                                        tracing::debug!("w send_trailers error: {}", e);
                                    })?;
                            }
                        }
                        Some(Err(e)) => {
                            tracing::debug!("frame error: {}", e);
                            // This maybe needed for grpc streaming?
                            panic!("client tries to send a status?")
                        }
                        None => break,
                    }
                }
                w.finish().await.inspect_err(|e| {
                    tracing::debug!("w finish error: {}", e);
                })?;
                Ok::<(), h3::Error>(())
            };
            tokio::spawn(send_body);

            // return resp.
            tracing::debug!("recv header");
            let (resp, _) = r.recv_response().await?.into_parts();
            let resp_body = H3IncomingClient::new(r);
            tracing::debug!("return resp");
            Ok(hyper::Response::from_parts(resp, resp_body))
        }
    })
}

// pub struct H3Channel{

// }

// impl tonic::client::GrpcService<tonic::body::BoxBody> for H3Channel{
//     type ResponseBody;

//     type Error = crate::Error;

//     type Future = impl Future<Output = Result<http::Response<Self::ResponseBody>, Self::Error>>;

//     fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> {
//         std::task::Poll::Ready(Ok(()))
//     }

//     fn call(&mut self, request: http::Request<tonic::body::BoxBody>) -> Self::Future {
//         todo!()
//     }
// }
