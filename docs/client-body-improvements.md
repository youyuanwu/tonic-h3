# Client Body Streaming Redesign

Redesigned client body sending to support eager polling, error logging, and cancellation.

## Previous Design

```rust
executor.execute(async move {
    let _ = send_h3_client_body::<CONN::BS, _>(&mut w, body).await;
});
let resp_body = H3IncomingClient::new(r);
```

Problems: unconditional spawn, silent error discard, no cancellation.

## New Design

```rust
let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();

let mut body_fut = Box::pin(
    send_h3_client_body::<CONN::BS, _>(w, body, cancel_rx),
);

// Eager poll: try to complete without spawning.
match poll_once(&mut body_fut).await {
    Some(res) => { res?; }
    None => {
        executor.execute(async move {
            if let Err(e) = body_fut.await {
                tracing::warn!("h3 client body send failed: {e}");
            }
        });
    }
};

let resp_body = H3IncomingClient::new(r, Some(cancel_tx));
```

### Eager Poll

`Box::pin` makes the future movable — poll once inline, spawn only if `Pending`. Stack-pinning would prevent the move. Follows hyper's H2 pattern (`PipeToSendStream`).

`Body::is_end_stream()` cannot be used instead: gRPC always has trailers, so it never returns `true`.

### Error Logging

Errors are logged at `warn` level in the spawned task. Hyper doesn't propagate body send errors to the caller either — response-side errors will surface naturally via the QUIC stream.

### Cancellation

`cancel_tx` is stored in `H3IncomingClient`. When dropped, the oneshot closes, and `send_h3_client_body` exits via `tokio::select! { biased; }`:

```rust
let frame = tokio::select! {
    biased;
    _ = &mut cancel => { return Ok(()); }
    frame = poll_fn(|cx| body.poll_frame(cx)) => frame,
};
```

### Owned `SendStream`

`send_h3_client_body` takes owned `w` (not `&mut w`) so the future can be moved into a spawned task after the eager poll.

## Files Changed

| File | Changes |
|---|---|
| `h3-util/src/client_body.rs` | `send_h3_client_body` takes owned `w` + cancel receiver; `H3IncomingClient` gains `_cancel_body_send` |
| `h3-util/src/client_conn.rs` | Eager poll, cancel/error wiring in `send_request_inner` |
