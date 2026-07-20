# Client URI & GOAWAY Reliability Fixes

Two client-path reliability fixes in `h3-util/src/client_conn.rs`, following up on the
MF-1/MF-2 reconnect/failover work: `RequestSender::call` no longer panics on URIs missing
components (SF-1), and a peer GOAWAY no longer pins the client to a cached sender that
rejects every new request forever (SF-3). Both changes are non-breaking — the public
constructors (`H3Channel::new`, `H3Connection::new`, `H3Client::new`) still return `Self`.

## SF-1: No panic on boundary URIs

`RequestSender::call` rebuilds each outgoing request URI from the channel's base URI
(scheme + authority, supplied to the public constructors as any `http::Uri`) and the
request's path-and-query (user/tonic-supplied). The previous code unwrapped every optional
component, so a base URI missing a scheme/authority, a request URI missing a
path-and-query, or an invalid rebuilt URI panicked deep inside `tower::Service::call`,
aborting the request task.

### Previous Design

```rust
let uri2 = Uri::builder()
    .scheme(uri.scheme().unwrap().clone())                       // panics: no scheme
    .authority(uri.authority().unwrap().clone())                 // panics: no authority
    .path_and_query(req.uri().path_and_query().unwrap().clone()) // panics: no path
    .build()
    .unwrap();                                                   // panics: invalid uri
```

### New Design

The base URI's scheme and authority are precomputed **once** in `RequestSender::new`,
which stays infallible (it must, to keep the constructors returning `Self`):

```rust
let base_scheme = uri.scheme().cloned();       // Option<Scheme>
let base_authority = uri.authority().cloned(); // Option<Authority>
```

`call` then surfaces every boundary condition as a clean per-request error future
(mirroring the MF-2 `connect_error` deferral pattern) instead of panicking:

- Missing base scheme or authority → per-request error future, returned **before** the
  cached sender is cloned or used (so no request is sent on an invalid base URI).
- Missing request path-and-query → defaults to origin-form `"/"`
  (`PathAndQuery::from_static("/")`). This is the correct HTTP/3 request target for an
  authority-form request URI (e.g. `localhost:443`, whose `path_and_query()` is `None`);
  the `:path` pseudo-header must be a non-empty path, and `/` is the origin-form default.
- `Uri::builder().build()` failure → per-request error future rather than `.unwrap()`.

Net effect: no reachable `panic!`/`unwrap`/`expect` on user-influenced URI values in the
request path. `Scheme`/`Authority`/`PathAndQuery` come from `hyper::http::uri` (hyper
re-exports the `http` crate), so no new dependency is added.

## SF-3: Reconnect after a peer GOAWAY

When the peer sends GOAWAY, h3 0.0.8 marks the connection closing but the client **driver
task keeps running** (its `poll_close` stays pending) to service in-flight streams — so
the existing driver-ended reconnect path (`driver_rx` firing) never triggers. Meanwhile
`h3::client::SendRequest::send_request` calls `check_peer_connection_closing()` and returns
`StreamError::RemoteClosing` for any **new** request. The result: `RequestSender::call`
kept cloning the same closing cached sender and every subsequent RPC failed permanently,
even though a fresh connection would succeed.

### Fix: a per-generation closing signal

`RequestSender` gains a `closing: Arc<AtomicBool>` field. The per-request future
(`send_request_inner`) captures a clone of the *current* flag and sets it when it observes
a **connection-level** closing/closed error (at the `send_request` and `recv_response`
sites). `poll_ready` checks the flag — alongside the existing `driver_rx` check — and
retires the cached sender when it is set, so the next request reconnects:

```rust
if self.closing.load(Ordering::SeqCst) {
    // driver stayed alive, but the peer is going away: retire and reconnect.
    self.retire_connection();
}
```

The request that observes the closing error fails cleanly; the **next** `poll_ready`
reconnects (no in-place retry — consistent with the MF-2 per-request-error model). The
first post-GOAWAY request failing, then the next reconnecting, is the intended behavior.

#### Generation tagging (race safety)

The `closing` flag's `Arc` identity is a **connection generation tag**. All retire paths go
through one helper:

```rust
fn retire_connection(&mut self) {
    self.send_request = None;
    self.driver_rx = None;
    self.closing = Arc::new(AtomicBool::new(false)); // fresh generation
}
```

Installing a *fresh* `Arc` on every retire (both the driver-ended and closing-flag paths)
is essential: an in-flight request from the retired connection still holds the previous
`Arc`, so its late error sets the old flag and cannot invalidate the healthy new sender a
subsequent `poll_ready` establishes. `call` clones `self.closing` at call time, so requests
on the new connection observe the new generation.

#### Connection-level vs. per-stream classification

Only **connection-level** conditions retire the shared cached sender — it multiplexes
other concurrent requests, so tearing it down on an ordinary per-stream error would
needlessly kill healthy connections. The classifier is conservative:

```rust
fn is_connection_closing(err: &h3::error::StreamError) -> bool {
    let msg = err.to_string();
    msg.starts_with("Remote is closing the connection") // RemoteClosing (GOAWAY)
        || msg.starts_with("Connection error:")         // ConnectionError(_)
}
```

Detection goes through `Display` deliberately. In h3 0.0.8 each `StreamError` variant is
individually `#[non_exhaustive]`, so downstream crates **cannot** name `RemoteClosing` or
`ConnectionError(_)` in a pattern (they are reported as private — E0603), and there is no
public predicate for the closing/GOAWAY case (`is_h3_no_error()` returns `false` for
`RemoteClosing`). The `Display` strings are the only stable public surface distinguishing
connection-level from stream-level errors, and `h3` is pinned to `0.0.8` in `Cargo.lock`,
so those strings are fixed. Stream-level variants render with other prefixes ("Stream
error:", "Remote reset:", "Header too big:", "Undefined error:") and return `false`.

## Tests

- `tonic-h3-tests/src/uri_boundary.rs` (SF-1) — asserts no panic for boundary URIs, each
  yielding a clean error or the documented `/` default: a base URI missing a scheme, a
  base URI missing an authority, and a request URI with no path-and-query.
- `tonic-h3-tests/src/goaway_reconnect.rs` (SF-3) — a real quinn + h3 server (a fake
  `SendRequest` cannot be injected) sends a genuine GOAWAY via
  `h3::server::Connection::shutdown(0)` and **keeps the connection/driver alive** so the
  client's `driver_rx` stays pending, exercising the new closing-flag path rather than the
  pre-existing driver-ended path. A connect-counting connector proves recovery:
  - `quinn_goaway_triggers_reconnect` — request #1 succeeds; a bounded probe loop then sees
    a request fail cleanly with the closing error and a subsequent request succeed via
    reconnection, with the connect count reaching **2**.
  - `quinn_stream_error_does_not_reconnect` — a stream-level reset must NOT retire the
    sender: the connect count stays **1** and a later request succeeds on the same
    connection (FR-006).

The existing `reconnect`, `cert_error`, `failover`, `buffered_reconnect`, `cancel_reset`,
and `mix` suites remain green and unweakened.

## Files Changed

| File | Changes |
|---|---|
| `h3-util/src/client_conn.rs` | SF-1: precompute base scheme/authority in `new`; `call` surfaces missing/invalid URI components as per-request errors and defaults missing path to `/`. SF-3: `closing` generation flag set by `send_request_inner` on connection-level errors and observed by `poll_ready`; `retire_connection` helper installs a fresh flag on every retire; `is_connection_closing` Display-based classifier. |
| `tonic-h3-tests/src/uri_boundary.rs` | New SF-1 boundary-URI no-panic tests. |
| `tonic-h3-tests/src/goaway_reconnect.rs` | New SF-3 GOAWAY-reconnect and per-stream-retention tests. |
| `tonic-h3-tests/src/lib.rs` | Register the new `uri_boundary` and `goaway_reconnect` modules. |
| `tonic-h3-tests/Cargo.toml` | Add `h3` dependency (server-side test harness). |
