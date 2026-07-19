use h3::client::RequestStream;
use hyper::body::{Body, Buf, Bytes};

pub struct H3IncomingClient<S, B>
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
    // Dropping this sender cancels the background body send task.
    _cancel_body_send: Option<tokio::sync::oneshot::Sender<()>>,
}

impl<S, B> H3IncomingClient<S, B>
where
    B: Buf,
    S: h3::quic::RecvStream,
{
    pub fn new(
        s: RequestStream<S, B>,
        cancel_body_send: Option<tokio::sync::oneshot::Sender<()>>,
    ) -> Self {
        Self {
            s,
            data_done: false,
            finished: false,
            _cancel_body_send: cancel_body_send,
        }
    }
}

impl<S, B> Drop for H3IncomingClient<S, B>
where
    B: Buf,
    S: h3::quic::RecvStream,
{
    fn drop(&mut self) {
        if !self.finished {
            // The incoming response body was dropped before it was fully
            // consumed. Tell the peer to stop sending with a proper HTTP/3 code
            // (quinn would otherwise send STOP_SENDING with code 0). Best-effort:
            // ignored if the stream is already finished or reset.
            self.s.stop_sending(h3::error::Code::H3_REQUEST_CANCELLED);
        }
    }
}

impl<S, B> hyper::body::Body for H3IncomingClient<S, B>
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
                Err(e) => {
                    self.finished = true;
                    std::task::Poll::Ready(Some(Err(e)))
                }
            }
        } else {
            // TODO: need poll trailers api.
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

/// Stream the client request body to the peer over the send-side
/// `RequestStream`.
///
/// The send stream is wrapped in a reset-on-drop guard: if the body send is
/// interrupted — by cancellation, a local body-source error, a transport send
/// error, or a hard drop/abort of this future — the stream is reset so the peer
/// observes an HTTP/3 stream error instead of a graceful end-of-stream.
/// Cancellation resets with `H3_REQUEST_CANCELLED`; body-source and transport
/// failures reset with `H3_INTERNAL_ERROR`. On normal completion the guard is
/// disarmed after `finish()` succeeds, preserving the clean FIN.
pub async fn send_h3_client_body<S, B>(
    w: h3::client::RequestStream<<S as h3::quic::BidiStream<Bytes>>::SendStream, Bytes>,
    bd: B,
    mut cancel: tokio::sync::oneshot::Receiver<()>,
) -> Result<(), crate::Error>
where
    S: h3::quic::BidiStream<hyper::body::Bytes>,
    B: Body + Send + 'static + Unpin,
    B::Data: Send,
    B::Error: Into<crate::Error>,
{
    // Reset-on-drop guard: if this future is cancelled, errors, or is dropped/
    // aborted before `finish()` succeeds, the send stream is reset so the peer
    // observes an HTTP/3 stream error instead of a graceful end-of-stream.
    let mut w = crate::send_guard::SendResetGuard::new(w);
    let mut p_b = std::pin::pin!(bd);
    loop {
        let frame = tokio::select! {
            biased;
            _ = &mut cancel => {
                tracing::trace!("client body send cancelled");
                // Guard stays armed with H3_REQUEST_CANCELLED; drop resets.
                return Ok(());
            }
            frame = futures::future::poll_fn(|cx| p_b.as_mut().poll_frame(cx)) => frame,
        };

        let Some(d) = frame else { break };
        let d = match d {
            Ok(d) => d,
            Err(e) => {
                // Local body-source failure: reset with an internal error.
                w.set_error_code(h3::error::Code::H3_INTERNAL_ERROR);
                return Err(e.into());
            }
        };
        if d.is_data() {
            let mut d = d.into_data().ok().unwrap();
            tracing::trace!("client write data");
            if let Err(e) = w.send_data(d.copy_to_bytes(d.remaining())).await {
                w.set_error_code(h3::error::Code::H3_INTERNAL_ERROR);
                return Err(e.into());
            }
        } else if d.is_trailers() {
            let d = d.into_trailers().ok().unwrap();
            tracing::trace!("client write trailer: {:?}", d);
            if let Err(e) = w.send_trailers(d).await {
                w.set_error_code(h3::error::Code::H3_INTERNAL_ERROR);
                return Err(e.into());
            }
        }
    }
    if let Err(e) = w.finish().await {
        w.set_error_code(h3::error::Code::H3_INTERNAL_ERROR);
        return Err(e.into());
    }
    // Normal completion: keep the graceful FIN, do not reset on drop.
    w.disarm();
    Ok(())
}
