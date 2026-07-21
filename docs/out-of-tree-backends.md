# Out-of-Tree, Third-Party QUIC Backends

Status: **Design / exploration** (no product code changes)

This document evaluates whether a **third-party QUIC backend crate can integrate
with `tonic-h3` / `axum-h3` without taking a hard dependency on `h3-util`**, and,
if feasible, designs the cleanest additive API to make it work. It is a design
artifact only; the API sketches below are illustrative and are validated
conceptually against the real traits (with file/line citations).

---

## 1. Problem statement, goals & non-goals

The in-tree backends — quinn, msquic, s2n-quic, quiche — all live inside
`h3-util` behind cargo features (`h3-util/src/lib.rs:41-53`). Each one supplies a
concrete `H3Connector` (client) and `H3Acceptor` (server) implementation, e.g.
`H3QuinnConnector` (`h3-util/src/quinn/client.rs:23-57`) and `H3QuinnAcceptor`
(`h3-util/src/quinn/server.rs:65-97`).

The maintainers want to enable **out-of-tree** backends: a hypothetical
`foo-quic-h3` crate, living in its own repo, that plugs into `tonic-h3` /
`axum-h3` just like an in-tree backend. The central question is a coupling one:

> Can `foo-quic-h3` avoid depending on `h3-util` entirely, while `h3-util` still
> provides adaptors that seamlessly turn the third party's QUIC connection type
> into something the client `H3Channel` and the server driver accept?

### Goals

- **G1 — No forced `h3-util` dependency for backend authors.** A third-party
  backend crate should be able to depend only on `h3` (+ its own QUIC library)
  and still be usable with `tonic-h3` / `axum-h3`.
- **G2 — In-tree backends unchanged.** quinn/msquic/s2n/quiche keep their current
  `H3Connector` / `H3Acceptor` impls with no source changes and no behavior
  regression (quinn is the only production backend and must not regress).
- **G3 — Consumers unaffected.** `tonic_h3::H3Channel`, `tonic_h3::server::H3Router`,
  and `axum_h3::H3Router` keep their current generic public API.
- **G4 — Additive only.** The solution is new public surface; nothing existing is
  removed or changed in a breaking way.

### Non-goals

- Redefining or relocating the `h3::quic::*` transport traits (they are upstream,
  owned by the `h3` crate — out of scope).
- Removing the `H3Connector` / `H3Acceptor` traits, or making the consumers
  dynamic (`dyn`)/object-safe. Consumers stay generic.
- Building the `foo-quic-h3` crate itself, or shipping a concrete new backend.
- Runtime backend selection / plugin discovery.

---

## 2. Current coupling analysis

### 2.1 The two integration traits

Client side (`h3-util/src/client.rs:26-41`):

```rust
pub trait H3Connector: Send + 'static + Clone {
    type CONN: h3::quic::Connection<Bytes, OpenStreams = Self::OS,
                                    SendStream = Self::SS, RecvStream = Self::RS> + Send;
    type OS: h3::quic::OpenStreams<Bytes, BidiStream = Self::BS> + Clone + Send;
    type SS: h3::quic::SendStream<Bytes> + Send;
    type RS: h3::quic::RecvStream + Send;
    type BS: h3::quic::BidiStream<Bytes, RecvStream = Self::RS, SendStream = Self::SS> + Send;

    fn connect(&self) -> impl Future<Output = Result<Self::CONN, crate::Error>> + Send;
}
```

Server side (`h3-util/src/server.rs:6-25`):

```rust
pub trait H3Acceptor {
    type CONN: h3::quic::Connection<Bytes, OpenStreams = Self::OS, SendStream = Self::SS,
                                    RecvStream = Self::RS, BidiStream = Self::BS> + Send + 'static;
    type OS: h3::quic::OpenStreams<Bytes, BidiStream = Self::BS> + Clone + Send;
    type SS: h3::quic::SendStream<Bytes> + Send;
    type RS: h3::quic::RecvStream + Send + 'static;
    type BS: h3::quic::BidiStream<Bytes, RecvStream = Self::RS, SendStream = Self::SS> + Send + 'static;

    fn accept(&mut self) -> impl Future<Output = Result<Option<Self::CONN>, crate::Error>> + Send;
}
```

### 2.2 What forces a dependency on `h3-util` today

**Only one thing: implementing `H3Connector` / `H3Acceptor`.** These two traits
are defined in `h3-util`. Under Rust's orphan rule a crate may implement a
*foreign* trait for a *local* type — so `foo-quic-h3` *could* write
`impl h3_util::client::H3Connector for FooConnector { … }` — but doing so
requires naming `h3_util::client::H3Connector`, i.e. a direct `h3-util`
dependency. That is exactly the coupling G1 wants to remove.

Everything the consumers actually *touch* is generic over these traits:

- `H3Channel<C, B>` / `H3Connection` / `H3Client` are generic over `C: H3Connector`
  and wrap it into a `tower::Service` via `Buffer` + `BoxService`, reconnecting
  through `client_conn::RequestSender` (`h3-util/src/client.rs:63-115, 176-256`;
  `h3-util/src/client_conn.rs`).
- The server driver `serve_inner` and `serve_request` are generic over
  `AC: H3Acceptor` (`axum-h3/src/lib.rs:49-190`); `tonic_h3::server::H3Router`
  and `axum_h3::H3Router::serve_with_shutdown` just forward the `AC` type
  parameter (`tonic-h3/src/server.rs:15-33`; `axum-h3/src/lib.rs:209-229`).

### 2.3 What does **not** force a dependency on `h3-util`

**The connection/stream types themselves.** Every associated type on both traits
is bound to an **upstream `h3::quic::*` trait**, not to anything in `h3-util`:

- `h3::quic::Connection<Bytes>`, `OpenStreams<Bytes>`, `SendStream<Bytes>`,
  `RecvStream`, `BidiStream<Bytes>` are all defined in the external `h3` crate
  (`h3-0.0.8/src/quic.rs:121-231`).

A third-party QUIC backend already implements these `h3::quic::*` traits — that
is precisely what the in-tree bridge crates (`h3_quinn`, `msquic-h3`,
`quiche-h3`) do, and what `H3QuinnConnector::connect` returns:
`h3_quinn::Connection`, which implements `h3::quic::Connection<Bytes>`
(`h3-util/src/quinn/client.rs:24-28, 42-43`).

**Key consequence:** the *raw connection object* a third party produces is
already `h3-util`-agnostic. Only the two wrapper traits are `h3-util`-owned. So
the integration problem reduces to: *provide an `h3-util`-owned adaptor that
turns "a thing that yields an `h3::quic::Connection<Bytes>`" into an
`H3Connector` / `H3Acceptor`, without the third party naming an `h3-util` type.*

### 2.4 The associated-type derivation that makes this clean

The trait bounds look like they need five independent associated types, but they
are all derivable from a **single** bound `C: h3::quic::Connection<Bytes>`,
because the upstream trait hierarchy already ties them together
(`h3-0.0.8/src/quic.rs:121-231`):

```text
Connection<B>: OpenStreams<B>            // Connection IS an OpenStreams
Connection<B>::RecvStream: RecvStream
Connection<B>::OpenStreams: OpenStreams<B, SendStream = ::SendStream, BidiStream = ::BidiStream>
OpenStreams<B>::SendStream: SendStream<B>
OpenStreams<B>::BidiStream: SendStream<B> + RecvStream
```

So from just `C: h3::quic::Connection<Bytes>` we can name all five:

| `H3Connector` / `H3Acceptor` assoc type | Derived from `C` |
|---|---|
| `CONN` | `C` |
| `OS`   | `<C as h3::quic::Connection<Bytes>>::OpenStreams` |
| `SS`   | `<C as h3::quic::OpenStreams<Bytes>>::SendStream` |
| `RS`   | `<C as h3::quic::Connection<Bytes>>::RecvStream` |
| `BS`   | `<C as h3::quic::OpenStreams<Bytes>>::BidiStream` |

The `SendStream = Self::SS` / `BidiStream = Self::BS` cross-equalities the two
traits demand are already guaranteed by `Connection::OpenStreams`'s own bound
(`h3-0.0.8/src/quic.rs:125`).

**One caveat verified by compiling the sketch (§ verification):** upstream bounds
`OpenStreams::BidiStream` only as `SendStream<B> + RecvStream`
(`h3-0.0.8/src/quic.rs:150`), *not* as `h3::quic::BidiStream<B>`, whereas
`H3Connector::BS` / `H3Acceptor::BS` require `h3::quic::BidiStream<Bytes>`
(`h3-util/src/client.rs:36`, `h3-util/src/server.rs:18-20`). So the adaptor must
add one explicit bound:

```rust
<C as h3::quic::OpenStreams<Bytes>>::BidiStream:
    h3::quic::BidiStream<Bytes,
        RecvStream = C::RecvStream,
        SendStream = <C as h3::quic::OpenStreams<Bytes>>::SendStream> + Send,
```

Every real backend's bidi-stream type already implements `h3::quic::BidiStream`
(that is what makes it usable with `h3`), so the bound is always satisfiable — it
just has to be stated. The other four types come "for free" from `C`.

### 2.5 Error and executor coupling

- `crate::Error` is a boxed trait object: `pub type Error = Box<dyn
  std::error::Error + Send + Sync>` (`h3-util/src/lib.rs:56`). Any third-party
  error `E: Into<Box<dyn Error + Send + Sync>>` (which includes any
  `E: std::error::Error + Send + Sync + 'static`) converts with `.into()`.
  So error plumbing needs **no** shared error type.
- The executor (`SharedExec`, `h3-util/src/executor.rs`/`client_conn.rs:9-36`)
  is constructed by the consumer, not the backend, so it introduces no backend
  coupling.

**Conclusion of §2: the decoupling is feasible.** The only thing preventing G1
is that the *impl* of `H3Connector`/`H3Acceptor` must live somewhere; if
`h3-util` provides a generic adaptor type that carries that impl, the third
party never names an `h3-util` type.

---

## 3. Design options

### Option A — Function / factory adaptors owned by `h3-util` (recommended)

`h3-util` ships generic adaptor structs plus ergonomic constructors:

- `ConnectorFn<F>` + `connector_fn(f)` for the client.
- `AcceptorFn<F>` + `acceptor_fn(f)` for the server.

The user supplies a closure/factory that returns a future resolving to a value
that implements `h3::quic::Connection<Bytes>`. `h3-util` implements its **own**
`H3Connector`/`H3Acceptor` trait for its **own** `ConnectorFn`/`AcceptorFn`
type — a textbook coherent impl (local trait, local type). The third-party crate
depends only on `h3` (+ its QUIC lib); the glue closure lives in the user's
binary (or a thin optional shim).

**Pros**

- Fully satisfies G1: third party depends only on `h3`.
- Coherent by construction (see §5); no orphan-rule tricks.
- Minimal new surface (two structs + two fns), additive (G4).
- In-tree backends untouched (G2) and consumers untouched (G3).
- Zero-cost: monomorphized generic wrapper, no `dyn`, no boxing of the
  connection.

**Cons**

- The five `h3::quic::*` bounds must be restated on the adaptor's impl (verbose,
  but written once inside `h3-util`).
- `Clone` requirement on the client side pushes a `Clone` bound onto the user's
  closure (acceptable; closures capturing `Clone` state are `Clone`).

### Option B — Blanket impls for arbitrary `Fn`/factory types

Instead of a named wrapper, blanket-impl the traits directly:

```rust
impl<F, Fut, C, E> H3Connector for F
where F: Fn() -> Fut + Send + 'static + Clone, /* … */ {}
```

**Orphan-rule reality:** this is *coherent* — `h3-util` owns `H3Connector`, so it
may add any impl it likes, including a blanket impl over `F: Fn`. (The thing the
orphan rule forbids is a *downstream* crate impl'ing `h3-util`'s trait for a
foreign type — which is not what happens here.)

**Pros**

- Most ergonomic call site: pass a bare closure, no wrapper constructor.

**Cons**

- **Coherence fragility / future-proofing:** a blanket impl for *all* `F: Fn()`
  permanently claims that shape. If `h3-util` ever wanted another blanket impl
  (e.g. for a different factory shape) they could overlap. A named wrapper
  (Option A) keeps the impl surface controllable.
- **Worse type errors** and possible inference ambiguity where a value is both a
  closure and something else.
- **`Clone` on closures** is still required and is easy to trip over with no
  wrapper to hang documentation/bounds on.

Option B is viable and coherent, but Option A's named wrapper is safer and
clearer. (Nothing stops us adding a blanket-ish convenience later.)

### Option C — Optional thin integration crate / feature-gated module

Put the adaptors in a separate `h3-util-adaptors` crate, or behind a feature
like `adaptors`.

**Pros**

- Keeps the core lean if the adaptors ever grow heavy dependencies (they don't —
  they need nothing beyond `h3` + `hyper::body::Bytes`, already core deps).

**Cons**

- The adaptor **must** be defined in the crate that owns the trait to be coherent
  (see §5). A *separate* crate could not impl `H3Connector` for a generic `F`
  unless `F` is local to it — so a standalone `h3-util-adaptors` crate would have
  to define its *own* wrapper type *and* still depend on `h3-util` for the trait,
  which is fine, but adds a crate for no coherence benefit.
- A feature gate adds a config axis for a tiny, dependency-free API.

Recommendation: keep the adaptors in `h3-util` core (they are essentially
free), optionally gate behind a default-on `adaptors` feature only if the
maintainers prefer opt-in surface.

### Option D — Object-safe / `dyn` connection adaptor

Erase the connection behind a boxed `dyn` and expose a non-generic
`BoxConnector`.

**Cons**

- `h3::quic::Connection` is not object-safe (generic methods / associated types),
  so this needs a hand-written vtable/newtype per stream type — large, intrusive,
  and it would regress the zero-cost generic path. Rejected.

---

## 4. Recommended design

Adopt **Option A**: two generic adaptor types in `h3-util`, plus constructor
functions, implementing `h3-util`'s own traits. Nothing else changes.

### 4.1 Client adaptor — `ConnectorFn`

```rust
// h3-util/src/client.rs (additive)
use std::future::Future;
use hyper::body::Bytes;

/// Adaptor that turns a cloneable async connection factory into an
/// [`H3Connector`], so a third-party QUIC backend only needs to depend on `h3`.
#[derive(Clone)]
pub struct ConnectorFn<F> {
    f: F,
}

/// Build an [`H3Connector`] from a factory `F: Fn() -> impl Future<Output =
/// Result<C, E>>` where `C: h3::quic::Connection<Bytes>`.
pub fn connector_fn<F, Fut, C, E>(f: F) -> ConnectorFn<F>
where
    F: Fn() -> Fut + Clone + Send + 'static,
    Fut: Future<Output = Result<C, E>> + Send,
    C: h3::quic::Connection<Bytes> + Send + 'static,
    C::OpenStreams: Clone + Send,
    <C as h3::quic::OpenStreams<Bytes>>::SendStream: Send,
    C::RecvStream: Send,
    // Upstream only guarantees `BidiStream: SendStream + RecvStream`; the trait
    // needs the full `h3::quic::BidiStream<Bytes>` (verified by compiling).
    <C as h3::quic::OpenStreams<Bytes>>::BidiStream: h3::quic::BidiStream<
            Bytes,
            RecvStream = C::RecvStream,
            SendStream = <C as h3::quic::OpenStreams<Bytes>>::SendStream,
        > + Send,
    E: Into<crate::Error>,
{
    ConnectorFn { f }
}

impl<F, Fut, C, E> H3Connector for ConnectorFn<F>
where
    F: Fn() -> Fut + Clone + Send + 'static,
    Fut: Future<Output = Result<C, E>> + Send,
    C: h3::quic::Connection<Bytes> + Send + 'static,
    C::OpenStreams: Clone + Send,
    <C as h3::quic::OpenStreams<Bytes>>::SendStream: Send,
    C::RecvStream: Send,
    <C as h3::quic::OpenStreams<Bytes>>::BidiStream: h3::quic::BidiStream<
            Bytes,
            RecvStream = C::RecvStream,
            SendStream = <C as h3::quic::OpenStreams<Bytes>>::SendStream,
        > + Send,
    E: Into<crate::Error>,
{
    type CONN = C;
    type OS = <C as h3::quic::Connection<Bytes>>::OpenStreams;
    type SS = <C as h3::quic::OpenStreams<Bytes>>::SendStream;
    type RS = <C as h3::quic::Connection<Bytes>>::RecvStream;
    type BS = <C as h3::quic::OpenStreams<Bytes>>::BidiStream;

    fn connect(&self) -> impl Future<Output = Result<Self::CONN, crate::Error>> + Send {
        let fut = (self.f)();
        async move { fut.await.map_err(Into::into) }
    }
}
```

Notes:

- All five associated types are derived from `C` per the table in §2.4; the
  `SendStream = Self::SS` / `BidiStream = Self::BS` equalities the trait requires
  hold automatically because `Connection::OpenStreams` re-exports the same
  associated types (`h3-0.0.8/src/quic.rs:125`).
- The `Clone` on `ConnectorFn<F>` (needed because `H3Connector: Clone`, see
  `h3-util/src/client.rs:26`) is satisfied by `#[derive(Clone)]` given
  `F: Clone`.
- Error conversion is a single `.into()` into the boxed `crate::Error`
  (`h3-util/src/lib.rs:56`).

### 4.2 Server adaptor — `AcceptorFn`

The acceptor is stateful (`&mut self`, `accept()` returns `Option`,
`h3-util/src/server.rs:22-24`), so the adaptor wraps a `FnMut` state machine that
yields the next connection (or `None` to stop).

```rust
// h3-util/src/server.rs (additive)
use std::future::Future;
use hyper::body::Bytes;

/// Adaptor that turns a `FnMut` accept-loop into an [`H3Acceptor`].
pub struct AcceptorFn<F> {
    f: F,
}

/// Build an [`H3Acceptor`] from `F: FnMut() -> impl Future<Output =
/// Result<Option<C>, E>>` where `C: h3::quic::Connection<Bytes>`.
pub fn acceptor_fn<F, Fut, C, E>(f: F) -> AcceptorFn<F>
where
    F: FnMut() -> Fut + Send,
    Fut: Future<Output = Result<Option<C>, E>> + Send,
    C: h3::quic::Connection<Bytes> + Send + 'static,
    C::OpenStreams: Clone + Send,
    <C as h3::quic::OpenStreams<Bytes>>::SendStream: Send,
    C::RecvStream: Send + 'static,
    <C as h3::quic::OpenStreams<Bytes>>::BidiStream: h3::quic::BidiStream<
            Bytes,
            RecvStream = C::RecvStream,
            SendStream = <C as h3::quic::OpenStreams<Bytes>>::SendStream,
        > + Send + 'static,
    E: Into<crate::Error>,
{
    AcceptorFn { f }
}

impl<F, Fut, C, E> H3Acceptor for AcceptorFn<F>
where
    F: FnMut() -> Fut + Send,
    Fut: Future<Output = Result<Option<C>, E>> + Send,
    C: h3::quic::Connection<Bytes> + Send + 'static,
    C::OpenStreams: Clone + Send,
    <C as h3::quic::OpenStreams<Bytes>>::SendStream: Send,
    C::RecvStream: Send + 'static,
    <C as h3::quic::OpenStreams<Bytes>>::BidiStream: h3::quic::BidiStream<
            Bytes,
            RecvStream = C::RecvStream,
            SendStream = <C as h3::quic::OpenStreams<Bytes>>::SendStream,
        > + Send + 'static,
    E: Into<crate::Error>,
{
    type CONN = C;
    type OS = <C as h3::quic::Connection<Bytes>>::OpenStreams;
    type SS = <C as h3::quic::OpenStreams<Bytes>>::SendStream;
    type RS = <C as h3::quic::Connection<Bytes>>::RecvStream;
    type BS = <C as h3::quic::OpenStreams<Bytes>>::BidiStream;

    fn accept(&mut self) -> impl Future<Output = Result<Option<Self::CONN>, crate::Error>> + Send {
        let fut = (self.f)();
        async move { fut.await.map_err(Into::into) }
    }
}
```

For richer acceptors that own a listening endpoint plus an in-flight
`JoinSet` (as `H3QuinnAcceptor` does, `h3-util/src/quinn/server.rs:51-97`), a
struct-based variant can be offered too:

```rust
/// State-carrying variant: `poll`-style accept over owned state `S`.
pub struct AcceptorState<S, F> { state: S, accept: F }
```

but the `FnMut` form above already covers the common "loop and await the next
connection" shape and keeps the surface small.

### 4.3 End-to-end usage: a hypothetical `foo-quic-h3`

`foo-quic-h3` (external repo) — depends only on `h3` + `foo-quic`, **not** on
`h3-util`:

```rust
// foo-quic-h3/src/lib.rs
use h3::quic; // upstream traits
use hyper::body::Bytes;

/// Wraps a `foo-quic` connection and implements the upstream h3 traits.
pub struct FooConnection(/* foo_quic::Connection */);

impl quic::Connection<Bytes> for FooConnection { /* … */ }
impl quic::OpenStreams<Bytes> for FooConnection { /* … */ }
// … SendStream / RecvStream / BidiStream on the stream types …

pub async fn connect(addr: std::net::SocketAddr, sni: &str)
    -> Result<FooConnection, foo_quic::Error> { /* … */ }

pub async fn accept(listener: &mut foo_quic::Listener)
    -> Result<Option<FooConnection>, foo_quic::Error> { /* … */ }
```

User binary — the only place `h3-util`/`tonic-h3` and `foo-quic-h3` meet:

```rust
use tonic_h3::H3Channel;                 // client
use tonic_h3::server::H3Router;          // server
use h3_util::client::connector_fn;
use h3_util::server::acceptor_fn;

// ---- client ----
let ep = foo_quic::Endpoint::client()?;
let uri: http::Uri = "https://example.com:443".parse()?;
let connector = connector_fn({
    let ep = ep.clone();
    move || {
        let ep = ep.clone();
        async move { foo_quic_h3::connect(resolve(&uri)?, "example.com").await }
    }
});
let channel: H3Channel<_> = H3Channel::new(connector, uri, None);
// … build a tonic client on `channel` exactly as with the quinn backend …

// ---- server ----
let mut listener = foo_quic::Listener::bind("0.0.0.0:443").await?;
let acceptor = acceptor_fn(move || {
    // NOTE: capture-by-&mut in a real impl; shown simplified
    async move { foo_quic_h3::accept(&mut listener).await }
});
H3Router::from(router)
    .serve_with_shutdown(acceptor, shutdown_signal())
    .await?;
```

Neither `foo-quic-h3` nor the user binary implements an `h3-util` trait by hand;
`connector_fn` / `acceptor_fn` carry the impls. `foo-quic-h3` has **no**
`h3-util` dependency (G1 met). `tonic_h3::H3Channel::new` and
`H3Router::serve_with_shutdown` are called with their existing signatures (G3
met).

### 4.4 Verification

The `ConnectorFn` sketch above was compiled against the **real** `h3-util` traits
and a **real** `h3::quic::Connection<Bytes>` type (`h3_quinn::Connection`), and
wired end-to-end into the real `tonic_h3::H3Channel::new` in a throwaway crate
(no product code changed). It compiles once the explicit
`BidiStream<Bytes>` bound from §2.4 is present; without that bound the compiler
rejects it with:

```text
error[E0277]: the trait bound
  `<C as OpenStreams<Bytes>>::BidiStream: BidiStream<Bytes>` is not satisfied
  note: required by a bound in `H3Connector::BS`
        (h3-util/src/client.rs:36)
```

This confirms both feasibility and coherence (§5) on the actual codebase rather
than by inspection alone.

---

## 5. Orphan rule / coherence analysis

The recommended design is coherent, and here is the precise reasoning:

1. **Who owns what.** `H3Connector` and `H3Acceptor` are defined in `h3-util`
   (`h3-util/src/client.rs:26`, `h3-util/src/server.rs:6`). `ConnectorFn<F>` /
   `AcceptorFn<F>` are *also* defined in `h3-util`.
2. **The recommended impls are local-trait + local-type.** `impl H3Connector for
   ConnectorFn<F>` and `impl H3Acceptor for AcceptorFn<F>` are both written in
   `h3-util`, for a type `h3-util` owns. This is the most basic allowed impl —
   the orphan rule never even engages, because there is no foreign trait *and* no
   foreign self type.
3. **The third party never impls an `h3-util` trait.** `foo-quic-h3` implements
   only `h3::quic::*` traits (foreign trait `h3::quic::Connection`, local type
   `FooConnection`) — which is the standard, allowed "foreign trait for local
   type" case, and requires only an `h3` dependency, not `h3-util`. This is
   exactly what the in-tree bridge crates already do
   (`h3-util/src/quinn/client.rs:42-43`).
4. **The generic parameters `C`, `Fut`, `E` may all be foreign** — that is fine.
   The orphan rule constrains the *Self type* and the *trait*, not the generic
   arguments. Since `Self = ConnectorFn<F>` is local and the trait is local, any
   foreign `F`/`C`/`E` is permitted. (This is the same reason the blanket-impl
   Option B would *also* compile: `h3-util` owning the trait may blanket-impl it.
   Option A is preferred for surface control, §3.)
5. **Associated-type coherence** holds because all five projected types come from
   the single upstream `C: h3::quic::Connection<Bytes>` and its guaranteed
   equalities (`h3-0.0.8/src/quic.rs:121-231`; see §2.4), plus the one explicit
   `BidiStream<Bytes>` bound noted there, so the `OpenStreams = Self::OS` /
   `SendStream = Self::SS` / `BidiStream = Self::BS` requirements of
   `H3Connector`/`H3Acceptor` are satisfied structurally. This was confirmed by
   compiling the adaptor against the real traits (§4.4).

No negative reasoning, no `unsafe`, no specialization — plain coherent Rust.

---

## 6. Backward compatibility & migration

- **Additive only (G4).** New items: `ConnectorFn`, `connector_fn`, `AcceptorFn`,
  `acceptor_fn` (and optionally `AcceptorState`). No existing item changes
  signature or behavior.
- **In-tree backends (G2).** `H3QuinnConnector` / `H3QuinnAcceptor` and the
  msquic/s2n/quiche impls keep their hand-written `impl H3Connector` /
  `impl H3Acceptor` blocks verbatim (`h3-util/src/quinn/client.rs:23-57`,
  `h3-util/src/quinn/server.rs:65-97`). They could *optionally* be reimplemented
  on top of the adaptors later, but that is not required and is out of scope; the
  production quinn path is unchanged, so there is no regression risk.
- **Consumers (G3).** `tonic_h3::H3Channel`, `tonic_h3::server::H3Router`,
  `axum_h3::H3Router` remain generic over `C: H3Connector` / `AC: H3Acceptor`
  (`tonic-h3/src/client.rs:1-3`, `tonic-h3/src/server.rs:15-33`,
  `axum-h3/src/lib.rs:209-229`); the adaptor types simply satisfy those bounds.
- **Feature/packaging.** The adaptors need no new dependencies (only `h3` +
  `hyper::body::Bytes`, both already core deps of `h3-util`). They can ship
  unconditionally, or behind a default-on `adaptors` feature per the workspace
  convention of minimal features (workspace `Cargo.toml` declares deps with
  minimal features and crates opt in).
- **Docs.** Add a short "Out-of-tree backends" section to the `h3-util` crate
  docs pointing backend authors at `connector_fn` / `acceptor_fn` and stating the
  only required dependency is `h3`.

---

## 7. Open questions & risks

1. **`Clone` + `Send + 'static` on the client factory.** `H3Connector: Send +
   'static + Clone` (`h3-util/src/client.rs:26`) forces the closure to be
   `Clone + Send + 'static`. Most factories capturing an `Endpoint` (which is
   `Clone`) satisfy this, but it is a real ergonomic constraint worth documenting
   with a working example.
2. **Acceptor mutability shape.** `accept(&mut self)` returning
   `Result<Option<CONN>, _>` (`h3-util/src/server.rs:22-24`) maps naturally to
   `FnMut`, but borrow-checker friction can arise when the closure must hold a
   `&mut listener` across `await`. The struct-based `AcceptorState<S, F>` variant
   (§4.2) is the escape hatch; decide whether to ship it now or on demand.
3. **Error conversion surface.** Relying on `E: Into<crate::Error>` is maximally
   permissive (any `Error + Send + Sync + 'static` works via the blanket `From`),
   but backend authors returning a non-`Send`/non-`Sync` error would not compile;
   document the `Send + Sync` requirement (it mirrors `crate::Error`,
   `h3-util/src/lib.rs:56`).
4. **Executor plumbing.** `SharedExec` is consumer-provided
   (`h3-util/src/client.rs:108-114`; `h3-util/src/executor.rs`), so out-of-tree
   backends need no executor awareness — but if a backend spawns its own tasks
   (as `H3QuinnAcceptor` does via `JoinSet`, `h3-util/src/quinn/server.rs:53`) it
   must own that internally; the `FnMut` adaptor gives it no executor handle.
   Consider whether `AcceptorState` should optionally receive a `SharedExec`.
5. **Naming.** `connector_fn` / `acceptor_fn` vs. `H3Connector::from_fn` /
   builder-style APIs. `*_fn` mirrors `tower::service_fn` and reads well; confirm
   with maintainers.
6. **`trait-variant` / RPITIT.** The traits already use return-position `impl
   Future` (`h3-util/src/client.rs:38-40`); the adaptor impls use the same form,
   so no `async_trait`/`trait-variant` boxing is introduced and the zero-cost
   property is preserved.

---

## 8. Summary of recommendation

- Decoupling is **feasible**: the only thing tying a backend to `h3-util` today
  is implementing `H3Connector`/`H3Acceptor`; the connection/stream types are
  already upstream `h3::quic::*` types.
- Ship **`h3-util`-owned generic adaptors** (`ConnectorFn`/`connector_fn`,
  `AcceptorFn`/`acceptor_fn`) that implement `h3-util`'s own traits for
  `h3-util`'s own wrapper types — coherent by construction, no orphan-rule
  issues, no `dyn`, no boxing of connections.
- All five associated types collapse to a single `C: h3::quic::Connection<Bytes>`
  bound (plus one explicit `BidiStream<Bytes>` bound, verified by compiling), so
  the adaptor is small and the user only writes a factory closure.
- The change is **purely additive**; in-tree backends and all consumers are
  unchanged, and the production quinn path does not regress.
