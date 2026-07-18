# s2n-quic Reconnect: Why the UDP Socket Isn't Released on Shutdown

Why `reconnect::h3_s2n_test` is `#[ignore]`d, why a retry-with-backoff on the
rebind does **not** fix it, and why — unlike the other backends — s2n-quic offers
*no application API at all* to close a server endpoint.

This is the s2n counterpart to
[quiche-reconnect-socket-release.md](./quiche-reconnect-socket-release.md); the
symptom is identical (`AddrInUse` on rebind) but the root cause is more
fundamental.

## Symptom

The reconnect test stops the server and immediately starts a new one bound to the
**same** UDP port ([`tonic-h3-tests/src/reconnect.rs`](../tonic-h3-tests/src/reconnect.rs)):

```rust
token.cancel();
h_svr.await.unwrap();
tokio::time::sleep(Duration::from_secs(3)).await;

let (h_svr2, listen_addr2) = run_server(listen_addr, token2.clone());
assert_eq!(listen_addr2.port(), listen_addr.port());
```

For the s2n backend the second bind fails with `AddrInUse`, so the test carries:

```rust
#[ignore = "s2n does not support acceptor close"]
```

The server helper already records that waiting is futile
([`tonic-h3-tests/src/lib.rs`](../tonic-h3-tests/src/lib.rs), `run_test_s2n_server`):

```rust
tracing::debug!("test server ended");
// s2n does not support close so wait a bit to let server release listening port.
// tokio::time::sleep(std::time::Duration::from_secs(2)).await;
// This does not work.
```

## Retry + backoff does not help

As with quiche, wrapping the rebind in an `AddrInUse`-only retry loop cannot
succeed. The socket is not lagging behind teardown — it is held open by
background endpoint tasks that the server side has **no way to stop**. Waiting
longer changes nothing.

## Root cause: `s2n_quic::Server` is only an *acceptor handle*

Unlike quinn (where the `Endpoint` owns the socket and exposes `close()`),
`s2n_quic::Server` owns neither the socket nor a way to close the endpoint. It is
a thin handle over an `Acceptor` (`s2n-quic-1.83.0/src/server.rs`):

```rust
pub struct Server {
    acceptor: Acceptor,
    local_addr: s2n_quic_core::inet::SocketAddress,
}
```

and the only thing it can do with it is `poll_accept`:

```rust
pub fn poll_accept(&mut self, cx: &mut Context) -> Poll<Option<Connection>> {
    match self.acceptor.poll_accept(cx) { … }
}
```

### The socket lives in detached tasks, not in `Server`

`.start()` builds the endpoint and hands it to the IO provider
(`s2n-quic-1.83.0/src/server/providers.rs`):

```rust
let (endpoint, acceptor) = endpoint::Endpoint::new_server(endpoint_config);
…
let local_addr = io.start(endpoint)?;   // only local_addr is kept
```

The tokio IO provider then **spawns** the socket-owning work as independent tasks
and returns (`s2n-quic-platform-0.83.0/src/io/tokio.rs`):

```rust
handle.spawn(task::rx( … rx_socket … ));   // owns the UDP socket
handle.spawn(task::tx( … tx_socket … ));   // owns a clone of the socket
let task = handle.spawn(EventLoop { endpoint, … }.start(rx_addr.into()));
…
Ok((task, rx_addr.into()))                 // `task` handle is dropped by the caller
```

So after `start()`:

- the UDP socket is owned by detached `rx`/`tx`/`EventLoop` tasks, and
- `Server` holds only the `Acceptor` receiver end.

Dropping the `Server` (which is what happens when `serve_with_shutdown` returns
and the acceptor is dropped) therefore drops only the accept channel — it does
**not** abort those tasks and does **not** release the socket.

### The `Acceptor` cannot request close — only the `Connector` can

The endpoint's event loop only tears down (dropping the endpoint and releasing the
socket) when both conditions hold (`s2n-quic-transport-0.83.0/src/endpoint/mod.rs`):

```rust
if self.close_handle.poll_interest().is_ready() // someone asked to close
    && self.connections.is_empty()              // all connections closed
    && self.connections.is_open()
{
    self.close_handle.close();
    self.connections.close();
}
if !self.connections.is_open() {
    return Poll::Ready(Err(CloseError));         // <- endpoint task ends here
}
```

"Close interest" is delivered through a `CloseSender`/`Closer`. Crucially, the two
application handles split their capabilities
(`s2n-quic-transport-0.83.0/src/endpoint/handle.rs`):

```rust
pub struct Acceptor { acceptor: AcceptorReceiver }          // poll_accept ONLY

pub struct Connector {
    connector: ConnectorSender,
    closer: close::Closer,                                   // poll_close lives here
}
```

`s2n_quic::Server` wraps the **`Acceptor`**, which has no `Closer` and no
`poll_close`. The close machinery is reachable only from the **`Connector`**,
which `s2n_quic::Client` holds — not the server. So a server application can never
signal close interest to its own endpoint.

That is precisely what the test's `#[ignore]` reason means by *"s2n does not
support acceptor close"*.

### Why the socket is never freed during the test

Both gates fail on the server side:

1. **No close signal.** The server holds only the `Acceptor`; there is no API to
   set close interest, so `poll_interest()` never becomes ready.
2. **Connections not empty.** The reconnect test keeps the client connected across
   the rebind (the client isn't cancelled until the end), so
   `connections.is_empty()` is false regardless.

With the endpoint event loop still running and still owning the socket, rebinding
the same port is impossible — not slow, impossible.

## Comparison with the other backends

| Backend | Who owns the socket | Server-side close API | Reconnect test |
| --- | --- | --- | --- |
| quinn | `Endpoint` (held by app) | `endpoint.close()` + `wait_idle()` | passes |
| quiche | detached `tokio-quiche` I/O task | none on the acceptor; socket freed only once the stream is dropped **and** all yielded connections close | ignored |
| s2n | detached `rx`/`tx`/`EventLoop` tasks | none — close lives on the `Connector`/`Client`, not the `Acceptor`/`Server` | ignored |

quinn passes because it force-closes the endpoint and its socket after the serve
future ends, independent of open connections
([`tonic-h3-tests/src/lib.rs`](../tonic-h3-tests/src/lib.rs), quinn helper):

```rust
endpoint.close(0_u16.into(), b"svr shutdown");
endpoint.wait_idle().await;
```

s2n has no equivalent, and — unlike quiche, where dropping the stream at least
satisfies one of the two teardown conditions — s2n's server side can satisfy
*neither*.

## What an actual fix requires

A retry is the wrong layer. To release the socket, the old server endpoint must be
told to close. Options, roughly in order of correctness:

1. **Expose the `Connector`/close handle on the server (needs upstream support).**
   `s2n_quic::Server` would need to surface the endpoint `Closer` (or a
   `close()`/`wait_idle()` pair like quinn) so the shutdown path can request close
   after the serve future ends. Today the capability exists in
   `s2n-quic-transport` but is only wired to the `Connector`.

2. **Give `H3S2nAcceptor` a shutdown hook that force-closes the endpoint.** Once
   upstream exposes a close, the acceptor wrapper
   ([`h3-util/src/s2n/server.rs`](../h3-util/src/s2n/server.rs)) could call it on
   drop/cancel, analogous to quinn's `endpoint.close()`.

3. **Track and abort the detached endpoint/IO tasks.** In principle the spawned
   `rx`/`tx`/`EventLoop` tasks could be tracked and aborted, but s2n-quic does not
   hand those `JoinHandle`s back to the application (the `task` handle is dropped
   inside `io.start`), so this is not reachable without upstream changes.

4. **`SO_REUSEPORT` on the bind — rejected.** It lets the second bind succeed while
   the first socket lingers, but the kernel then load-balances UDP datagrams across
   both sockets, delivering client packets to the dead endpoint. This breaks
   correctness and must not be used.

Until upstream exposes a server-side close, the test stays `#[ignore]`d. A backoff
retry on the rebind should **not** be added: the server can neither signal close
nor is the connection set empty, so no amount of waiting frees the port — it would
only turn a fast, clearly-labelled skip into a slow, confusing timeout.

## References

- Ignored test: [`tonic-h3-tests/src/reconnect.rs`](../tonic-h3-tests/src/reconnect.rs)
- s2n server helper + the "does not work" note: [`tonic-h3-tests/src/lib.rs`](../tonic-h3-tests/src/lib.rs) (`run_test_s2n_server`)
- Acceptor wrapper: [`h3-util/src/s2n/server.rs`](../h3-util/src/s2n/server.rs)
- `Server` = acceptor handle: `s2n-quic` 1.83.0 `src/server.rs`
- Endpoint spawn / socket ownership: `s2n-quic` 1.83.0 `src/server/providers.rs`, `s2n-quic-platform` 0.83.0 `src/io/tokio.rs`
- Acceptor vs. Connector capabilities: `s2n-quic-transport` 0.83.0 `src/endpoint/handle.rs`
- Endpoint teardown gate: `s2n-quic-transport` 0.83.0 `src/endpoint/mod.rs`, `src/endpoint/close.rs`
