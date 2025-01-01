use h3::client::RequestStream;
use hyper::body::{Body, Buf, Bytes};

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
    pub fn new(s: RequestStream<S, B>) -> Self {
        Self {
            s,
            data_done: false,
        }
    }
}

impl<S, B> hyper::body::Body for H3IncomingClient<S, B>
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
            match futures::ready!(self.s.poll_recv_data(cx)) {
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
            match futures::ready!(self.s.poll_recv_trailers(cx))? {
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

pub async fn send_h3_client_body<S, B>(
    w: &mut h3::client::RequestStream<<S as h3::quic::BidiStream<Bytes>>::SendStream, Bytes>,
    bd: B,
) -> Result<(), crate::Error>
where
    S: h3::quic::BidiStream<hyper::body::Bytes>,
    B: Body + Send + 'static + Unpin,
    B::Data: Send,
    B::Error: Into<crate::Error>,
{
    let mut p_b = std::pin::pin!(bd);
    let mut sent_trailers = false;
    while let Some(d) = futures::future::poll_fn(|cx| p_b.as_mut().poll_frame(cx)).await {
        // send body
        let d = d.map_err(|e| e.into())?;
        if d.is_data() {
            let mut d = d.into_data().ok().unwrap();
            tracing::debug!("client write data");
            w.send_data(d.copy_to_bytes(d.remaining())).await?;
        } else if d.is_trailers() {
            let d = d.into_trailers().ok().unwrap();
            tracing::debug!("client write trailer: {:?}", d);
            w.send_trailers(d).await?;
            sent_trailers = true;
        }
    }
    if !sent_trailers {
        w.finish().await?;
    }
    Ok(())
}
