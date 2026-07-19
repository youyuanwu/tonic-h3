//! Reset-on-drop guard for HTTP/3 send-side [`RequestStream`]s.
//!
//! When a request/response body send is interrupted — by cancellation, a local
//! body-source error, a transport send error, or a hard drop/abort of the
//! body-send future — the send-side stream must be *reset* so the peer observes
//! an HTTP/3 stream error rather than a graceful end-of-stream. Without an
//! explicit reset, dropping an unfinished Quinn `SendStream` implicitly calls
//! `finish()`, which the peer sees as a clean FIN, allowing a truncated message
//! to be processed as if it were complete.
//!
//! [`SendResetGuard`] wraps the send half and calls
//! [`stop_stream`](h3::client::RequestStream::stop_stream) on drop while it is
//! *armed*. Callers disarm it only after a normal `finish()` succeeds, so a
//! completed stream is never spuriously reset.
//!
//! [`RequestStream`]: h3::client::RequestStream

use h3::error::Code;
use hyper::body::Buf;

/// Send-side HTTP/3 streams that can be reset with an error code.
///
/// Implemented for both the client and server `RequestStream` send halves (and
/// for `&mut` references to them) so a single guard type works for both.
pub(crate) trait StopSendStream {
    fn stop_send_stream(&mut self, code: Code);
}

impl<S, B> StopSendStream for h3::client::RequestStream<S, B>
where
    S: h3::quic::SendStream<B>,
    B: Buf,
{
    fn stop_send_stream(&mut self, code: Code) {
        self.stop_stream(code);
    }
}

impl<S, B> StopSendStream for h3::server::RequestStream<S, B>
where
    S: h3::quic::SendStream<B>,
    B: Buf,
{
    fn stop_send_stream(&mut self, code: Code) {
        self.stop_stream(code);
    }
}

impl<T: StopSendStream> StopSendStream for &mut T {
    fn stop_send_stream(&mut self, code: Code) {
        (**self).stop_send_stream(code);
    }
}

/// RAII guard that resets the wrapped send stream on drop unless disarmed.
///
/// The guard [`Deref`](std::ops::Deref)s to the wrapped stream, so
/// `send_data`/`send_trailers`/`finish` are called through it directly.
///
/// - Default state is *armed* with [`Code::H3_REQUEST_CANCELLED`].
/// - Call [`set_error_code`](Self::set_error_code) before returning an error to
///   reset with a different code (e.g. [`Code::H3_INTERNAL_ERROR`]).
/// - Call [`disarm`](Self::disarm) after a successful `finish()` so normal
///   completion leaves the graceful FIN in place.
pub(crate) struct SendResetGuard<W: StopSendStream> {
    w: W,
    armed: bool,
    code: Code,
}

impl<W: StopSendStream> SendResetGuard<W> {
    /// Wrap a send stream, armed to reset with [`Code::H3_REQUEST_CANCELLED`].
    pub(crate) fn new(w: W) -> Self {
        Self {
            w,
            armed: true,
            code: Code::H3_REQUEST_CANCELLED,
        }
    }

    /// Disarm the guard so drop leaves the stream untouched (normal completion).
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }

    /// Set the error code used when the guard resets the stream on drop.
    pub(crate) fn set_error_code(&mut self, code: Code) {
        self.code = code;
    }
}

impl<W: StopSendStream> std::ops::Deref for SendResetGuard<W> {
    type Target = W;

    fn deref(&self) -> &Self::Target {
        &self.w
    }
}

impl<W: StopSendStream> std::ops::DerefMut for SendResetGuard<W> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.w
    }
}

impl<W: StopSendStream> Drop for SendResetGuard<W> {
    fn drop(&mut self) {
        if self.armed {
            // Best-effort: a no-op if the stream is already finished or reset.
            self.w.stop_send_stream(self.code);
        }
    }
}
