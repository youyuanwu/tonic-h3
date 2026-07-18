# Quiche Reconnect: Why the UDP Socket Isn't Released on Shutdown

Why `reconnect::h3_quiche_test` is `#[ignore]`d, why a retry-with-backoff on the
rebind does **not** fix it, and what an actual fix requires.

## Symptom

The reconnect test stops the server and immediately starts a new one bound to the
**same** UDP port, asserting the client can talk to the fresh server
([`tonic-h3-tests/src/reconnect.rs`](../tonic-h3-tests/src/reconnect.rs)):

```rust
// stop and restart the server on the same address
token.cancel();
h_svr.await.unwrap();
tokio::time::sleep(Duration::from_secs(3)).await;

let (h_svr2, listen_addr2) = run_server(listen_addr, token2.clone());
assert_eq!(listen_addr2.port(), listen_addr.port());
```

For the quiche backend the second bind fails:

```
bind udp socket: Os { code: 98, kind: AddrInUse, message: "Address already in use" }
```

so the test carries:

```rust
#[ignore = "quiche listener does not release the UDP socket promptly on shutdown (AddrInUse on rebind)"]
```

The bind happens synchronously in
[`run_test_quiche_server`](../tonic-h3-tests/src/lib.rs) via
`std::net::UdpSocket::bind`, which is where `AddrInUse` surfaces.

## Retry + backoff does not help

An obvious first idea is to wrap the rebind in a loop that retries only the
`AddrInUse` error with exponential backoff. This was implemented and measured: it
retried for **~9 seconds** across 10 attempts (50 ms → 1 s backoff) and **still**
failed with `AddrInUse` every time.

The reason is that this is **not a transient, timing-related lag** in socket
teardown. The socket is *deliberately held open* for as long as the old client
connection is alive — which, by the very design of the reconnect test, is until
after the rebind. No amount of waiting frees it.

## Root cause: `tokio-quiche`'s socket lifecycle

`tokio-quiche` owns the UDP socket in a background I/O task, not in the acceptor
handle we hold. Its `listen` documentation is explicit
(`tokio-quiche-0.19.1/src/lib.rs`):

> Each socket starts a separate tokio task to process and route inbound packets.
> … **The task shuts down when the returned stream is closed (or dropped) and all
> previously-yielded connections are closed.**

So releasing the socket requires **two** conditions:

1. the `QuicConnectionStream` is dropped, **and**
2. every connection the stream already yielded is closed.

Our acceptor (`quiche-h3` 0.0.1, wrapped by
[`h3-util/src/quiche_h3/server.rs`](../h3-util/src/quiche_h3/server.rs)) only holds
the stream. It has no handle to the already-accepted connections, so it can satisfy
(1) but not (2).

### Why condition (2) is never met during the test

The generic serve loop in
[`axum-h3/src/lib.rs`](../axum-h3/src/lib.rs) drives it:

1. On the shutdown signal, `serve_with_shutdown` returns and the acceptor (stream)
   is dropped — condition (1) is satisfied:

   ```rust
   _ = &mut sig => {
       tracing::trace!("cancellation triggered");
       return Ok(());
   }
   ```

2. **But** each accepted connection is served on a *detached* background task:

   ```rust
   // serve each connection in the background
   executor.execute(async move {
       let mut conn = h3::server::Connection::new(conn).await?;
       loop {
           let resolver = match conn.accept().await { … };
           …
       }
   });
   ```

   This task is not tracked or aborted on shutdown. Its inner loop only breaks when
   the **client** ends the connection (`conn.accept()` returns `None`/error).

3. In the reconnect test the client stays connected across the rebind (it is not
   cancelled until the end of the test). So the server-side connection never
   closes, condition (2) is never met, the `tokio-quiche` I/O task keeps running,
   and the UDP socket stays bound. Rebinding the same port is therefore impossible
   — not slow, impossible — until the client disconnects.

## Why the other backends work

quinn passes the same test because its server *force-closes* the endpoint (and its
socket) after the server future ends, regardless of open connections
([`tonic-h3-tests/src/lib.rs`](../tonic-h3-tests/src/lib.rs), quinn helper):

```rust
h_sv.await…;
endpoint.close(0_u16.into(), b"svr shutdown");
endpoint.wait_idle().await;
```

There is no equivalent for quiche: the acceptor exposes no "close the listener /
close all connections" operation, so the shutdown path cannot force condition (2).

(s2n is `#[ignore]`d on the same test for a related reason — "s2n does not support
acceptor close" — analysed in
[s2n-reconnect-socket-release.md](./s2n-reconnect-socket-release.md).)

## What an actual fix requires

A retry is the wrong layer. Any real fix must make the *old* server release its
socket on shutdown, i.e. satisfy condition (2). Options, roughly in order of
correctness:

1. **Force-close the quiche listener/connections on shutdown** (preferred, needs
   upstream support). Give the `quiche-h3` acceptor a `close()` that shuts down the
   underlying `tokio-quiche` listener and its live connections — the quiche analog
   of quinn's `endpoint.close()` + `wait_idle()`. The server helper would call it
   after the serve future ends.

2. **Track and abort the detached connection tasks in `axum-h3`.** Replace the
   fire-and-forget `executor.execute(...)` with tracked handles (e.g. a `JoinSet`)
   and abort/await them when the shutdown signal fires. Dropping the connections
   drops their quiche `Connection`s, satisfying condition (2) so `tokio-quiche`
   releases the socket. This is a cross-backend change to the serve loop and needs
   care not to regress graceful-shutdown behavior for the other backends.

3. **`SO_REUSEPORT` on the bind — rejected.** It would let the second bind succeed
   while the first socket lingers, but for UDP the kernel then load-balances
   datagrams across both sockets, so client packets could be delivered to the dead
   old socket. This breaks correctness and must not be used here.

Until option 1 or 2 lands, the test stays `#[ignore]`d. A backoff retry on the
rebind should **not** be added: it cannot succeed while the client connection is
open, and it would only turn a fast, clearly-labelled skip into a slow, confusing
timeout.

## References

- Ignored test: [`tonic-h3-tests/src/reconnect.rs`](../tonic-h3-tests/src/reconnect.rs)
- Quiche server + bind: [`tonic-h3-tests/src/lib.rs`](../tonic-h3-tests/src/lib.rs) (`run_test_quiche_server`)
- Acceptor wrapper: [`h3-util/src/quiche_h3/server.rs`](../h3-util/src/quiche_h3/server.rs)
- Generic serve loop / detached connection tasks: [`axum-h3/src/lib.rs`](../axum-h3/src/lib.rs)
- Socket lifecycle contract: `tokio-quiche` 0.19.1 `listen` docs (`src/lib.rs`)
