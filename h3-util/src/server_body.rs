use h3::server::RequestStream;
use hyper::body::{Body, Buf, Bytes};

pub struct H3IncomingServer<S, B>
where
    B: Buf,
    S: h3::quic::RecvStream,
{
    s: RequestStream<S, B>,
    data_done: bool,
    // Set once the stream reaches its terminal state (clean end-of-stream or a
    // stream error). Used by `Drop` to decide whether an early drop should reset
    // the receive side.
    finished: bool,
}

impl<S, B> H3IncomingServer<S, B>
where
    B: Buf,
    S: h3::quic::RecvStream,
{
    pub fn new(s: RequestStream<S, B>) -> Self {
        Self {
            s,
            data_done: false,
            finished: false,
        }
    }
}

impl<S, B> Drop for H3IncomingServer<S, B>
where
    B: Buf,
    S: h3::quic::RecvStream,
{
    fn drop(&mut self) {
        if !self.finished {
            // The incoming request body was dropped before it was fully
            // consumed. Tell the peer to stop sending with a proper HTTP/3 code
            // (quinn would otherwise send STOP_SENDING with code 0). Best-effort:
            // ignored if the stream is already finished or reset.
            self.s.stop_sending(h3::error::Code::H3_REQUEST_CANCELLED);
        }
    }
}

impl<S, B> hyper::body::Body for H3IncomingServer<S, B>
where
    B: Buf,
    S: h3::quic::RecvStream,
{
    type Data = hyper::body::Bytes;

    type Error = h3::error::StreamError;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        if !self.data_done {
            tracing::trace!("server incomming poll_frame recv_data");
            match futures::ready!(self.s.poll_recv_data(cx)) {
                Ok(data_opt) => match data_opt {
                    Some(mut data) => {
                        let f = hyper::body::Frame::data(data.copy_to_bytes(data.remaining()));
                        std::task::Poll::Ready(Some(Ok(f)))
                    }
                    None => {
                        self.data_done = true;
                        // try again to get trailers
                        cx.waker().wake_by_ref();
                        std::task::Poll::Pending
                    }
                },
                Err(e) => {
                    self.finished = true;
                    std::task::Poll::Ready(Some(Err(e)))
                }
            }
        } else {
            tracing::trace!("server incomming poll_frame recv_trailers");
            match futures::ready!(self.s.poll_recv_trailers(cx)) {
                Ok(Some(tr)) => std::task::Poll::Ready(Some(Ok(hyper::body::Frame::trailers(tr)))),
                Ok(None) => {
                    self.finished = true;
                    std::task::Poll::Ready(None)
                }
                Err(e) => {
                    self.finished = true;
                    std::task::Poll::Ready(Some(Err(e)))
                }
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

/// Stream the server response body to the peer over the send-side
/// `RequestStream`.
///
/// Response headers have already been sent by the caller, so an interrupted body
/// must reset the stream rather than let the peer see a graceful end-of-stream
/// for a truncated response. The send stream is wrapped in a reset-on-drop
/// guard: a body-source or transport failure resets with `H3_INTERNAL_ERROR`,
/// and a hard drop/abort of this future resets with the default
/// `H3_REQUEST_CANCELLED`. On normal completion the guard is disarmed after
/// `finish()` succeeds.
pub async fn send_h3_server_body<BD, S>(
    w: &mut h3::server::RequestStream<<S as h3::quic::BidiStream<Bytes>>::SendStream, Bytes>,
    bd: BD,
) -> Result<(), crate::Error>
where
    BD: Body + 'static,
    BD::Error: Into<crate::Error>,
    <BD as Body>::Error: Into<crate::Error> + std::error::Error + Send + Sync,
    <BD as Body>::Data: Send + Sync,
    S: h3::quic::BidiStream<hyper::body::Bytes>,
{
    // Reset-on-drop guard: response headers may already have been sent, so an
    // interrupted body must reset the stream rather than let the peer see a
    // graceful end-of-stream for a truncated response.
    let mut w = crate::send_guard::SendResetGuard::new(w);
    let mut p_b = std::pin::pin!(bd);
    while let Some(d) = futures::future::poll_fn(|cx| p_b.as_mut().poll_frame(cx)).await {
        // send body
        let d = match d {
            Ok(d) => d,
            Err(e) => {
                // Local body-source failure: reset with an internal error.
                w.set_error_code(h3::error::Code::H3_INTERNAL_ERROR);
                return Err(crate::Error::from(e));
            }
        };
        if d.is_data() {
            let mut d = d.into_data().ok().unwrap();
            tracing::trace!("serving request write data");
            // Bytes optimizes the shallow copy.
            if let Err(e) = w.send_data(d.copy_to_bytes(d.remaining())).await {
                w.set_error_code(h3::error::Code::H3_INTERNAL_ERROR);
                return Err(e.into());
            }
        } else if d.is_trailers() {
            let d = d.into_trailers().ok().unwrap();
            tracing::trace!("serving request write trailer: {:?}", d);
            if let Err(e) = w.send_trailers(d).await {
                w.set_error_code(h3::error::Code::H3_INTERNAL_ERROR);
                return Err(e.into());
            }
        }
    }
    // Close the stream gracefully.
    // This is technically only needed when not writing trailers.
    // But msquic-h3 requires stream be gracefully closed all the time.
    if let Err(e) = w.finish().await {
        w.set_error_code(h3::error::Code::H3_INTERNAL_ERROR);
        return Err(e.into());
    }
    // Normal completion: keep the graceful FIN, do not reset on drop.
    w.disarm();
    Ok(())
}
