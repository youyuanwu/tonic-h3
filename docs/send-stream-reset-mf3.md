# Resetting HTTP/3 Streams on Interrupted Body Sends

Interrupted request/response body sends now reset the HTTP/3 stream so the peer
observes a stream error instead of a graceful end-of-stream.

## The Problem

`send_h3_client_body` / `send_h3_server_body` own (client) or borrow (server) the
send half of an h3 `RequestStream`. When a body send was interrupted, the send
half was simply dropped without being reset.

On the production `quinn` backend, dropping an unfinished `SendStream` implicitly
calls `finish()` (quinn `send_stream.rs`, `impl Drop for SendStream`). The peer
therefore sees a clean `FIN` — "message complete" — instead of a cancel/reset.
A **truncated** request or response could be processed as if it were complete.
This is dangerous for client-streaming / bidirectional RPCs and for cancellation:

- **Cancellation** (interrupt path A): the client body-send future is eagerly
  polled and then detached as a background task (`client_conn.rs`). Cancelling
  the RPC dropped that task; the un-reset stream FIN-ed.
- **Local body-source error** (path B): `body.poll_frame` yields `Err(..)`.
- **Transport send error** (path C): `send_data` / `send_trailers` / `finish`
  fails.

The send-side `RequestStream` exposes a synchronous
`stop_stream(error_code: h3::error::Code)` that resets the sending part — but
explicit `?`/`select!` branches cannot cover the case where the whole future is
dropped or its task aborted.

## The Fix: A Reset-on-Drop Guard

`h3-util/src/send_guard.rs` introduces `SendResetGuard<W>`, an RAII guard around
the send-side stream:

- It is created **armed** with the default code `H3_REQUEST_CANCELLED`.
- It `Deref`s to the wrapped stream, so `send_data` / `send_trailers` / `finish`
  go through it unchanged.
- On `Drop`, if still armed, it calls `stop_stream(code)` (best-effort; a no-op
  if the stream is already finished or reset).
- On **normal completion**, the caller disarms it *after* `finish()` succeeds, so
  a completed stream is never spuriously reset.

Because the guard owns (client) or mutably borrows (server) the stream, a hard
drop or task abort still runs `Drop` and resets the stream — the crux that
branch-only handling misses.

### Error-Code Mapping

The two failure classes are deliberately **not** collapsed:

| Interrupt                                   | Reset code             |
| ------------------------------------------- | ---------------------- |
| Deliberate cancellation (path A)            | `H3_REQUEST_CANCELLED` |
| Local body-source / transport send (B, C)   | `H3_INTERNAL_ERROR`    |

Cancellation returns `Ok(())` with the guard still armed at its default code.
Error paths call `set_error_code(H3_INTERNAL_ERROR)` before returning `Err`.

## Receive-Side Reset on Incomplete Drop

The incoming bodies (`H3IncomingClient`, `H3IncomingServer`) had the mirror-image
problem: dropping one before the stream was fully consumed left quinn to send
`STOP_SENDING` with code `0`, which is not a valid HTTP/3 error code.

Each now tracks a `finished` flag, set when the body reaches a terminal state
(clean end-of-stream or a stream error). A `Drop` impl calls
`stop_sending(H3_REQUEST_CANCELLED)` only when the body was **not** fully
consumed. Fully-read streams — the normal unary/streaming case, where gRPC always
reads trailers — are never reset, so there is no behavior change for completed
RPCs.

## Tests

`tonic-h3-tests/src/cancel_reset.rs` exercises the production quinn path through
the raw `h3-util` client and an `axum-h3` server that reports the terminal
outcome of reading the request body (clean EOF vs. stream error):

- `client_body_error_resets_stream` — a body-source failure mid-upload → the
  server observes a reset carrying `H3_INTERNAL_ERROR`.
- `client_cancel_resets_stream` — cancelling an in-flight upload (dropping the
  response) → the server observes a reset carrying `H3_REQUEST_CANCELLED`.
- `normal_upload_completes_cleanly` — a body that completes normally → the server
  sees a graceful end-of-stream (guards against spurious resets).

## Files Changed

- `h3-util/src/send_guard.rs` (new) — `SendResetGuard` and the `StopSendStream`
  trait.
- `h3-util/src/client_body.rs` — wire the guard into `send_h3_client_body`;
  recv-side `Drop` on `H3IncomingClient`.
- `h3-util/src/server_body.rs` — wire the guard into `send_h3_server_body`;
  recv-side `Drop` on `H3IncomingServer`.
- `h3-util/src/lib.rs` — register the `send_guard` module.
- `tonic-h3-tests/src/cancel_reset.rs` (new) — integration tests.
