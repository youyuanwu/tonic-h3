# Client Reconnect & Failover Fixes

Two client-path reliability fixes: quinn address failover no longer aborts on the first
async handshake failure (MF-1), and a transient connect failure no longer permanently
bricks a buffered `H3Channel` (MF-2).

## MF-1: Try all resolved addresses

`dns_resolve` can return several addresses (e.g. IPv6 + IPv4). `Endpoint::connect()` only
validates config synchronously; the real handshake completes on the awaited future.

### Previous Design

```rust
Ok(conn) => {
    // `?` returns from the whole function on the FIRST async handshake failure,
    // so later resolved addresses are never tried.
    let x = conn.await.map_err(Into::<crate::Error>::into)?;
    return Ok(h3_quinn::Connection::new(x));
}
Err(e) => conn_err = e,
```

### New Design

```rust
Ok(conn) => match conn.await.map_err(Into::<crate::Error>::into) {
    Ok(x) => return Ok(h3_quinn::Connection::new(x)),
    // Record the error and try the next address instead of aborting.
    Err(e) => conn_err = e,
},
Err(e) => conn_err = e,
```

`Err(conn_err)` is returned only after all addresses are exhausted. The synchronous-error
arm already continued and is unchanged; the placeholder `conn_err` (`AddrNotAvailable`)
surfaces only when the resolved list is empty.

## MF-2: Don't brick the buffered channel on a transient failure

`H3Channel` wraps the reconnecting `RequestSender` in `tower::buffer::Buffer`. When the
inner `poll_ready` returns `Poll::Ready(Err(_))`, the Buffer worker treats it as
**terminal** — it closes the request channel and replays the stored error to every later
request and every cloned handle (tower-0.5.3 `src/buffer/worker.rs`). `RequestSender`
returned connect errors this way, so one transient failure (server not up yet, DNS blip,
failed reconnect) permanently disabled the channel.

### Fix: defer connect errors to per-request failures (Option A)

`RequestSender` gains `connect_error: Option<crate::Error>`.

`poll_ready` keeps the driver-staleness check, adds an idempotency guard, and stores
(rather than returns) a connect error:

```rust
// After the driver-staleness check, before any connect logic:
if self.connect_error.is_some() {
    return Poll::Ready(Ok(())); // stay ready; error is surfaced by `call`
}
// ...
Err(e) => {
    self.make_send_request_fut = None;
    self.connect_error = Some(e);
    Ok(()) // NOT Err(e)
}
```

`call` surfaces the stored error per-request and never panics on a missing sender:

```rust
if let Some(e) = self.connect_error.take() {
    return Box::pin(async move { Err(e) });
}
let send_request = match &self.send_request {
    Some(sr) => sr.clone(),
    None => return Box::pin(async move { Err(/* not-ready error */) }),
};
```

The idempotency guard is required by the tower `Service` contract: repeated `poll_ready`
calls must stay ready without starting a second connect or dropping the pending error.
Because `connect_error` is only set when no sender was cached, the invariant
`connect_error.is_some() ⟹ send_request.is_none()` holds and the cache-hit assertions
stay valid. After a failure `send_request` stays `None` and `make_send_request_fut` is
cleared, so once `call` consumes the error the next `poll_ready` starts a fresh connect —
the channel stays alive and reconnects.

The same shared `RequestSender` backs the non-buffered `H3Connection`; its per-RPC
behavior is unchanged (connect errors now surface from `call` rather than `poll_ready`).

## Tests

- `tonic-h3-tests/src/buffered_reconnect.rs` — deterministic: a `FailOnce` connector fails
  its first `connect()` then delegates to a real quinn connector. Asserts a buffered
  `H3Channel` recovers on a later RPC and its clones are not bricked, and that the
  non-buffered path recovers too. Without the MF-2 fix the first failure bricks the Buffer
  and the later requests fail.
- `tonic-h3-tests/src/failover.rs` — best-effort, environment-guarded: binds the server to
  `127.0.0.1` and connects via `localhost`; asserts failover when the host resolves to a
  dual-stack set that can present a failing-first address, and self-skips otherwise (there
  is no DNS-injection seam without refactoring `dns_resolve`).

## Files Changed

| File | Changes |
|---|---|
| `h3-util/src/quinn/client.rs` | `connect` loops over all resolved addresses; async handshake errors continue instead of returning |
| `h3-util/src/client_conn.rs` | `RequestSender` gains `connect_error`; `poll_ready` idempotency guard + non-terminal connect errors; `call` surfaces the error and drops the `unwrap()` panic |
| `tonic-h3-tests/src/buffered_reconnect.rs` | New deterministic MF-2 regression tests (buffered + clone + non-buffered) |
| `tonic-h3-tests/src/failover.rs` | New environment-guarded MF-1 failover test |
