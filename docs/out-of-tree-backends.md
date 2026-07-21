# Out-of-Tree, Third-Party QUIC Backends

Status: **Design / exploration** (no product code changes) — **revised following a
design debate** (see "Debate outcome" below). The recommendation has changed: the
integration path we recommend *now* is a **direct trait impl** (optionally behind a
backend-side cargo feature), not the function/factory adaptors this doc originally
led with.

This document evaluates whether a **third-party QUIC backend crate can integrate
with `tonic-h3` / `axum-h3` while minimizing its dependency on `h3-util`**, and
designs the cleanest additive API to support that. It is a design artifact only;
the API sketches below are illustrative and are validated conceptually against the
real traits (with file/line citations).

## Debate outcome

The original version of this doc recommended shipping `h3-util` function/factory
adaptors (`connector_fn` / `acceptor_fn`) so a third-party backend could depend on
`h3` alone ("Option A"). A five-specialist Society-of-Thought debate
(architecture, maintainability, assumptions, release-manager, edge-cases; 2
rounds) **superseded that recommendation**. Its verdict is **B-now + A-later**:

- **Ship nothing new as the mechanism now.** Out-of-tree backends already
  integrate today by implementing `H3Connector` / `H3Acceptor` **directly** — the
  orphan rule permits `impl h3_util::client::H3Connector for FooConnector`
  (foreign trait, local type). This is the recommended path, ideally behind an
  **off-by-default cargo feature on the backend crate** so non-`tonic` users get a
  pure-`h3` dependency graph while Cargo still reconciles the `h3` version.
- **Two position-independent must-fixes outrank the whole A-vs-B choice:**
  1. **`h3`-version-alignment ownership gap.** Both the traits *and* any adaptor
     bind every associated type to `h3::quic::*` from **one exact `h3` line**
     (`h3 = "0.0.8"`, `Cargo.toml:8`). Under `0.0.x` caret rules `0.0.8`/`0.0.9`
     do **not** unify, so version skew fails as an opaque trait-projection error.
     Fix: `h3-util` must add `pub use h3;` (none exists today), document a
     mandatory same-`h3`-version contract, and publish a compat table keyed by
     exact `h3` line. See §4.2.
  2. **No server `AcceptorFn`.** `H3Acceptor::accept(&mut self)` is a stateful
     lending accept loop that a `FnMut() -> Fut` closure cannot express; drop the
     server adaptor. See §4.4.
- **Defer Position A's adaptors** to a proven-demand convenience tier and narrow
  them to the **client `ConnectorFn` only**, with a documented clone/reconnect
  contract (§4.3). No out-of-tree backend exists today (YAGNI); the adaptors are
  speculative surface plus a second, non-converging integration pattern.

The sections below preserve the original coupling and coherence analysis
(§2, §5), which the debate found accurate, and rewrite the options (§3),
recommended design (§4), migration (§6), and summary (§8) to reflect the verdict.

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

- **G1 — Reduce the backend's `h3-util` dependency surface.** A third-party
  backend crate should be able to integrate without a *forced, always-on*
  `h3-util` dependency — ideally depending only on `h3` (+ its own QUIC library)
  in its default configuration. Note this only reduces the `h3-util` surface; the
  `h3` coupling itself is irreducible (every associated type is an `h3::quic::*`
  type from one exact `h3` line — see §2.3 and §4.2), so "no `h3-util` dependency"
  never means "no version coupling."
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

**Conclusion of §2: the decoupling is feasible.** The `impl` of
`H3Connector`/`H3Acceptor` must live somewhere, but the third party can already
supply it *directly* — the orphan rule permits `impl h3_util::client::H3Connector
for FooConnector` (foreign trait, local type; §2.2, §5), which is exactly what the
in-tree backends do. That direct impl is the recommended path (§3, §4). A
generic `h3-util`-owned adaptor is an *alternative* that additionally lets the
third party avoid naming an `h3-util` type — a convenience the debate deferred
(§3, §4.3). Either way, the `h3::quic::*` version coupling remains (§4.2).

---

## 3. Design options

The debate (see "Debate outcome") ordered these by what to do **now** versus
**later**. The recommended path is the direct trait impl (**Option B**), optionally
behind a backend-side cargo feature. The function/factory adaptors (**Option A**)
are a deferred, client-only convenience. The remaining entries are adaptor-shape
variants that only matter *if* Option A is ever adopted.

### Option B — Direct `H3Connector` / `H3Acceptor` impl, optionally feature-gated — **recommended (ship now)**

The out-of-tree backend implements `H3Connector` / `H3Acceptor` **directly** for
its own connector/acceptor type. Under Rust's orphan rule this is always allowed —
it is a *foreign trait* (`h3_util::client::H3Connector`) for a *local type*
(`FooConnector`), the standard case (§2.2, §5). This is exactly how all four
in-tree backends integrate today (`h3-util/src/quinn/client.rs:23-57`,
`h3-util/src/quinn/server.rs:65-97`), so there is **one** integration pattern, no
new `h3-util` surface, and the most diagnosable type errors (they point at the
backend's own named associated types).

To reduce the dependency surface (G1), the backend crate can put its `h3-util`
dependency behind an **off-by-default cargo feature**, e.g.:

```toml
# foo-quic-h3/Cargo.toml
[dependencies]
h3 = "0.0.8"           # always required — the transport ABI
h3-util = { version = "…", optional = true }

[features]
tonic = ["dep:h3-util"]   # off by default
```

Non-`tonic` users of `foo-quic-h3` then get a pure-`h3` dependency graph, while
`tonic` users opt in and Cargo reconciles the shared `h3` version for them.

**Pros**

- Works **today**; nothing new ships in `h3-util` as the mechanism.
- One integration pattern, identical to the in-tree backends (G2) — easiest to
  explain, document, and diagnose.
- Keeps Cargo as the enforcer of `h3`-version alignment (the backend's `h3-util`
  edge forces reconciliation at resolve time; see §4.2).
- Optional-feature delivers real dependency-surface decoupling at **zero**
  `h3-util` API cost, and consumers/in-tree backends are untouched (G3, G4).

**Cons / honest limits**

- The optional-feature form still leaves a **published crate** (`foo-quic-h3`
  itself, in its `tonic`-enabled configuration) on the `h3-util` release axis: an
  `h3-util` bump can require a `foo-quic-h3` republish. Only the full-adaptor
  Option A uniquely pushes all `h3-util`-coupled glue to the **leaf binary**, so
  that *zero* intermediate crates sit on that axis. That is Option A's single
  unique benefit — and it only matters for a **multi-backend future that does not
  yet exist** (§7).
- The backend author writes the trait impl by hand (five associated types), but
  the derivation is mechanical (§2.4) and mirrors the in-tree backends.

### Option A — Function / factory adaptors owned by `h3-util` — **deferred convenience (client-only)**

`h3-util` ships a generic adaptor struct plus an ergonomic constructor so a
backend/user can supply a closure instead of a hand-written trait impl:

- `ConnectorFn<F>` + `connector_fn(f)` for the client — **the only compile-validated
  adaptor** (§4.3.2); deferred until a real backend author requests it, and then
  shipped only with a documented clone/reconnect contract (§4.3.1).
- ~~`AcceptorFn<F>` + `acceptor_fn(f)` for the server~~ — **dropped.** A
  `FnMut() -> Fut` closure cannot model the stateful lending accept loop
  (§4.4). Stateful server backends use a direct `H3Acceptor` impl (Option B).

The user supplies a closure/factory that returns a future resolving to a value
that implements `h3::quic::Connection<Bytes>`. `h3-util` implements its **own**
`H3Connector` trait for its **own** `ConnectorFn` type — a textbook coherent impl
(local trait, local type). This lets the third-party crate avoid *naming* an
`h3-util` type at all; the glue closure lives in the user's binary (or a thin
optional shim).

**Pros**

- Lets the third party depend only on `h3` even in a hand-written form (no
  `h3_util::` path named in the backend).
- Coherent by construction (see §5); no orphan-rule tricks.
- Additive (G4); in-tree backends untouched (G2); consumers untouched (G3).
- Zero-cost: monomorphized generic wrapper, no `dyn`, no boxing of the connection.

**Cons**

- **Speculative:** zero out-of-tree backends exist to consume it (§7) — YAGNI.
- Adds a **second, non-converging integration pattern** ("two ways to satisfy one
  trait") with no migration path unless the in-tree backends are reimplemented on
  it (§7).
- **Makes version skew later/more confusing** in its full form: removing the
  backend-side `h3-util` Cargo edge removes the only tool-enforced `h3`-version
  reconciliation point, so skew fails at the leaf call site instead of at
  resolve time (§4.2).
- The five `h3::quic::*` bounds must be restated on the adaptor's impl (verbose,
  but written once inside `h3-util`).
- `Clone` requirement on the client side pushes a `Clone` bound onto the user's
  closure, and the client clones the connector before every reconnect — a
  documented hazard (§4.3.1).

#### Option A variants (only relevant if/when Option A is adopted)

**A1 — Blanket impls for arbitrary `Fn`/factory types.** Instead of a named
wrapper, blanket-impl the trait directly:

```rust
impl<F, Fut, C, E> H3Connector for F
where F: Fn() -> Fut + Send + 'static + Clone, /* … */ {}
```

This is *coherent* — `h3-util` owns `H3Connector`, so it may add any impl it likes,
including a blanket impl over `F: Fn`. (The orphan rule forbids only a *downstream*
crate impl'ing `h3-util`'s trait for a foreign type — not this.) It is the most
ergonomic call site (bare closure, no constructor), but a blanket impl for *all*
`F: Fn()` permanently claims that shape (future overlap risk), produces worse type
errors, and still requires `Clone` on the closure with no wrapper to hang
docs/bounds on. If Option A is ever adopted, the named `ConnectorFn` wrapper is
safer and clearer than a blanket impl.

**A2 — Separate crate / feature-gated module.** Put the adaptors in a separate
`h3-util-adaptors` crate or behind a feature. The adaptor **must** be defined in
the crate that owns the trait to be coherent (§5), so a standalone crate would have
to define its *own* wrapper type *and* still depend on `h3-util` — a crate for no
coherence benefit. If Option A ships at all, keep the adaptor in `h3-util` core
(it needs nothing beyond `h3` + `hyper::body::Bytes`, already core deps),
optionally behind a feature only if the maintainers prefer opt-in surface.

**A3 — Object-safe / `dyn` connection adaptor.** Erase the connection behind a
boxed `dyn` and expose a non-generic `BoxConnector`. **Rejected:**
`h3::quic::Connection` is not object-safe (generic methods / associated types), so
this needs a hand-written vtable/newtype per stream type — large, intrusive, and
it would regress the zero-cost generic path.

---

## 4. Recommended design

Adopt **B-now + A-later**. The mechanism to ship now is the **direct
`H3Connector` / `H3Acceptor` impl** (§4.1), optionally behind a backend-side cargo
feature. The single highest-priority change is documenting and enforcing
**`h3`-version alignment** (§4.2) — this is a *correctness/reliability* fix that
applies regardless of which integration path is used. The client `ConnectorFn`
adaptor is a **deferred, contract-bound convenience** (§4.3); the server
`AcceptorFn` is **rejected** (§4.4).

### 4.1 Recommended now — direct `H3Connector` / `H3Acceptor` impl

An out-of-tree `foo-quic-h3` implements the two traits directly, exactly like the
in-tree quinn backend (`h3-util/src/quinn/client.rs:23-57`,
`h3-util/src/quinn/server.rs:65-97`). The `h3-util` dependency is put behind an
**off-by-default** cargo feature so non-`tonic` users get a pure-`h3` graph:

```toml
# foo-quic-h3/Cargo.toml
[dependencies]
h3 = "0.0.8"              # always required — the transport ABI (see §4.2)
foo-quic = "…"
h3-util = { version = "…", optional = true }

[features]
tonic = ["dep:h3-util"]   # off by default
```

```rust
// foo-quic-h3/src/lib.rs
use h3::quic;             // upstream traits, always required
use hyper::body::Bytes;

/// Wraps a `foo-quic` connection and implements the upstream h3 traits.
pub struct FooConnection(/* foo_quic::Connection */);

impl quic::Connection<Bytes> for FooConnection { /* … */ }
impl quic::OpenStreams<Bytes> for FooConnection { /* … */ }
// … SendStream / RecvStream / BidiStream on the stream types …

// The `h3-util` integration lives behind the `tonic` feature, so non-tonic
// users never pull `h3-util`.
#[cfg(feature = "tonic")]
mod integration {
    use super::*;
    use h3_util::client::H3Connector;
    use h3_util::server::H3Acceptor;

    #[derive(Clone)]
    pub struct FooConnector { /* endpoint handle, uri, server name … */ }

    // Foreign trait (`h3_util::client::H3Connector`) for a local type
    // (`FooConnector`) — always allowed by the orphan rule (§5), and mirrors
    // `H3QuinnConnector` (`h3-util/src/quinn/client.rs:23-57`).
    impl H3Connector for FooConnector {
        type CONN = FooConnection;
        type OS = <FooConnection as quic::Connection<Bytes>>::OpenStreams;
        type SS = <FooConnection as quic::OpenStreams<Bytes>>::SendStream;
        type RS = <FooConnection as quic::Connection<Bytes>>::RecvStream;
        type BS = <FooConnection as quic::OpenStreams<Bytes>>::BidiStream;
        async fn connect(&self) -> Result<Self::CONN, h3_util::Error> { /* … */ }
    }

    pub struct FooAcceptor { /* owns the listener + any in-flight state */ }

    // Direct impl handles the stateful `&mut self` accept loop naturally —
    // mirrors `H3QuinnAcceptor` (`h3-util/src/quinn/server.rs:65-97`).
    impl H3Acceptor for FooAcceptor {
        type CONN = FooConnection;
        type OS = <FooConnection as quic::Connection<Bytes>>::OpenStreams;
        type SS = <FooConnection as quic::OpenStreams<Bytes>>::SendStream;
        type RS = <FooConnection as quic::Connection<Bytes>>::RecvStream;
        type BS = <FooConnection as quic::OpenStreams<Bytes>>::BidiStream;
        async fn accept(&mut self) -> Result<Option<Self::CONN>, h3_util::Error> { /* … */ }
    }
}
```

User binary — the only place `h3-util`/`tonic-h3` and `foo-quic-h3` meet:

```rust
use tonic_h3::H3Channel;                 // client
use tonic_h3::server::H3Router;          // server
use foo_quic_h3::integration::{FooConnector, FooAcceptor};

// ---- client ----
let uri: http::Uri = "https://example.com:443".parse()?;
let connector = FooConnector::new(/* endpoint, uri, "example.com" */);
let channel: H3Channel<_> = H3Channel::new(connector, uri, None);
// … build a tonic client on `channel` exactly as with the quinn backend …

// ---- server ----
let acceptor = FooAcceptor::bind("0.0.0.0:443").await?;
H3Router::from(router)
    .serve_with_shutdown(acceptor, shutdown_signal())
    .await?;
```

The backend author writes five associated types by hand (mechanically derivable
per §2.4), and `foo-quic-h3` is the only crate that names an `h3-util` type — and
only when its `tonic` feature is on. `tonic_h3::H3Channel::new` and
`H3Router::serve_with_shutdown` are called with their existing signatures (G3
met). This is one integration pattern, identical to all four in-tree backends
(G2), and the trait-projection errors point at the backend's own named types.

### 4.2 `h3`-version alignment (top-priority must-fix)

This is the **highest-confidence, position-independent** finding of the debate and
outranks the whole adaptor question. Every associated type on both traits is bound
to `h3::quic::*` from **one exact `h3` line** — `h3 = "0.0.8"`
(`Cargo.toml:8`; `h3-util/src/client.rs:26-40`; `h3-util/src/server.rs:6-25`).
Under Cargo's `0.0.x` caret rules, `0.0.8` and `0.0.9` do **not** unify, so any
`h3` version skew between the backend and `h3-util` fails as an **opaque
`H3Connector` / `H3Acceptor` trait-projection mismatch**, far from its cause.

A direct `h3-util` dependency edge (Option B, §4.1) is what forces Cargo to
reconcile the `h3` version at resolve time. The **full** Option A adaptor removes
that edge and therefore makes the failure **later and more confusing** — skew
surfaces at the user's leaf call site instead of at dependency resolution. This is
a decisive reason the debate did not recommend pushing glue to the leaf binary.

**Required fixes (do these regardless of A vs B):**

1. **Add `pub use h3;` to `h3-util`.** None exists today (`h3-util/src/lib.rs:56-57`
   re-exports only `Error`/`StdError`). Re-exporting `h3` lets downstreams pin the
   exact `h3` line *through* `h3-util` (`use h3_util::h3;`), guaranteeing they
   compile against the same `h3` the traits are defined over.
2. **Document a mandatory same-`h3`-version contract.** A backend must build
   against the exact `h3` line `h3-util` was built against; a mismatch is a hard,
   compile-time break, not a soft incompatibility.
3. **Publish a compatibility table keyed by exact `h3` line**, e.g.:

   | `h3-util` | requires `h3` |
   |---|---|
   | current | `=0.0.8` |

   Extend one row per `h3` bump. Until `h3` reaches `1.0`, every `h3` bump is a
   breaking transport-ABI bump for `h3-util` and its backends.

### 4.3 Deferred: client `ConnectorFn` convenience tier (on demand)

**Status: deferred.** Do **not** ship this until a concrete out-of-tree backend
author asks for it (zero exist today — §7). When shipped, it is an
explicitly-scoped *stateless, version-matched* convenience, and it is the **only**
adaptor that was compile-validated (§4.3.2). The sketch below stands, plus a
mandatory clone/reconnect contract (§4.3.1).


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

#### 4.3.1 Mandatory clone/reconnect contract

`H3Connector: Clone` and the client **clones the connector before every
(re)connection attempt** (`h3-util/src/client_conn.rs:272-278`: it clones
`self.conn` then calls `.connect().await`). `ConnectorFn` derives `Clone` from its
captured factory `F`, so any value-copied capture (counters, queues, one-shot
tokens, per-connection buffers) is **duplicated/reset on every reconnect** rather
than shared. The real quinn connector avoids this by cloning a *shared endpoint
handle* (`h3-util/src/quinn/client.rs:6-18` — `H3QuinnConnector` holds a cloneable
`quinn::Endpoint`).

Therefore the convenience tier ships with a documented, tested contract:

- The factory closure must capture **only shared handles** (e.g. an `Arc`, or a
  cheaply-cloneable endpoint), never per-connection value state.
- `connect()` must be **safely repeatable** — calling it again after a driver ends
  must re-establish a working connection.

Without this contract the clone-on-reconnect behavior is a silent footgun; with it,
the client `ConnectorFn` is release-worthy.

#### 4.3.2 Verification

The `ConnectorFn` sketch above was compiled against the **real** `h3-util` traits
and a **real** `h3::quic::Connection<Bytes>` type (`h3_quinn::Connection`), and
wired end-to-end into the real `tonic_h3::H3Channel::new` in a throwaway crate
(no product code changed). It compiles once the explicit `BidiStream<Bytes>` bound
from §2.4 is present; without that bound the compiler rejects it with:

```text
error[E0277]: the trait bound
  `<C as OpenStreams<Bytes>>::BidiStream: BidiStream<Bytes>` is not satisfied
  note: required by a bound in `H3Connector::BS`
        (h3-util/src/client.rs:36)
```

This confirms both feasibility and coherence (§5) on the actual codebase rather
than by inspection alone. Note the **server `AcceptorFn` was never compile-verified**
(§4.4).

### 4.4 Rejected: server `AcceptorFn` / `AcceptorState`

**Status: rejected — do not ship.** The debate found the server adaptor cannot
express the real stateful accept loops it claims to support:

- `H3Acceptor::accept(&mut self)` is a **stateful lending loop**: it returns a
  future that borrows the receiver and drives owned state across `await`
  (`h3-util/src/server.rs:22-24`). The production quinn acceptor mutates a
  `JoinSet` across `await` (`h3-util/src/quinn/server.rs:51-95`); the msquic
  acceptor holds a `Mutex<Listener>` (`h3-util/src/msquic/server.rs:7`).
- The `F: FnMut() -> Fut` shape below fixes **one** `Fut` with no call-lifetime, so
  it cannot borrow per-call state; the "simplified" closure example could not hold
  `&mut listener` across `await`, and `AcceptorState<S, F>` is a two-field struct
  with **no constructor, impl, or lending contract** — storage, not a solution.
- Unlike the client `ConnectorFn`, this adaptor was **never compile-verified**
  (§4.3.2).

Stateful server backends therefore MUST use a **direct `H3Acceptor` impl**
(§4.1) until a proven lending/poll-based design exists. The sketch below is
retained only to record *why* it does not work.

```rust
// h3-util/src/server.rs — REJECTED: cannot model a stateful/lending accept loop
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

The sketched state-carrying variant does **not** rescue it — it is storage with no
lending contract, no constructor, and no impl:

```rust
/// State-carrying variant: `poll`-style accept over owned state `S`.
/// (Storage only — no lending contract; NOT a working solution.)
pub struct AcceptorState<S, F> { state: S, accept: F }
```

A real server adaptor would need a demonstrated lending/poll design validated
against a Quinn-shaped `JoinSet` acceptor before it could ship; until then, use the
direct `H3Acceptor` impl of §4.1.

---

## 5. Orphan rule / coherence analysis

Both the **recommended direct impls** (§4.1) and the **deferred client
`ConnectorFn`** (§4.3) are coherent. Here is the precise reasoning:

1. **Who owns what.** `H3Connector` and `H3Acceptor` are defined in `h3-util`
   (`h3-util/src/client.rs:26`, `h3-util/src/server.rs:6`). The deferred
   `ConnectorFn<F>` would *also* be defined in `h3-util`.
2. **The recommended path — a direct impl — is foreign-trait + local-type.** A
   backend writing `impl h3_util::client::H3Connector for FooConnector` (§4.1)
   implements a *foreign* trait for a *local* type — the standard, always-allowed
   orphan-rule case, exactly what all four in-tree backends do
   (`h3-util/src/quinn/client.rs:23-57`).
3. **The deferred adaptor impl is local-trait + local-type.** `impl H3Connector for
   ConnectorFn<F>` is written in `h3-util`, for a type `h3-util` owns — the most
   basic allowed impl; the orphan rule never engages, since neither the trait nor
   the self type is foreign.
4. **The generic parameters `C`, `Fut`, `E` may all be foreign** — that is fine.
   The orphan rule constrains the *Self type* and the *trait*, not the generic
   arguments. Since `Self = ConnectorFn<F>` is local and the trait is local, any
   foreign `F`/`C`/`E` is permitted. (This is the same reason the blanket-impl
   variant A1 would *also* compile: `h3-util` owning the trait may blanket-impl it.
   A named wrapper is preferred for surface control, §3.)
5. **Associated-type coherence** holds because all five projected types come from
   the single upstream `C: h3::quic::Connection<Bytes>` and its guaranteed
   equalities (`h3-0.0.8/src/quic.rs:121-231`; see §2.4), plus the one explicit
   `BidiStream<Bytes>` bound noted there, so the `OpenStreams = Self::OS` /
   `SendStream = Self::SS` / `BidiStream = Self::BS` requirements of
   `H3Connector`/`H3Acceptor` are satisfied structurally. This was confirmed by
   compiling the client adaptor against the real traits (§4.3.2).

No negative reasoning, no `unsafe`, no specialization — plain coherent Rust.

---

## 6. Backward compatibility & migration

The near-term change is **not** shipping adaptors. It is documentation plus one
additive re-export:

- **`pub use h3;` in `h3-util` (the only code change).** Purely additive; lets
  downstreams pin the exact `h3` line through `h3-util` (§4.2). No existing item
  changes signature or behavior (G4).
- **Docs.** Add an "Out-of-tree backends" section to the `h3-util` crate docs that
  (a) shows the direct `H3Connector` / `H3Acceptor` impl pattern (§4.1), (b) shows
  the optional backend-side `tonic`/`h3-util` feature gate, and (c) states the
  mandatory same-`h3`-version contract plus the compatibility table (§4.2).
- **In-tree backends (G2).** `H3QuinnConnector` / `H3QuinnAcceptor` and the
  msquic/s2n/quiche impls are **unchanged** — they already use the recommended
  direct-impl pattern (`h3-util/src/quinn/client.rs:23-57`,
  `h3-util/src/quinn/server.rs:65-97`). The production quinn path does not regress.
- **Consumers (G3).** `tonic_h3::H3Channel`, `tonic_h3::server::H3Router`,
  `axum_h3::H3Router` remain generic over `C: H3Connector` / `AC: H3Acceptor`
  (`tonic-h3/src/client.rs:1-3`, `tonic-h3/src/server.rs:15-33`,
  `axum-h3/src/lib.rs:209-229`) — **unchanged**; a direct backend impl simply
  satisfies those bounds.
- **Deferred (A-later).** If a concrete out-of-tree backend author requests the
  client `ConnectorFn` convenience (§4.3), it ships additively then, with the
  clone/reconnect contract and — should it ever be adopted — a migration path that
  reimplements the in-tree backends on it (so the ecosystem does not carry two
  non-converging patterns, §7). The server `AcceptorFn` is not shipped (§4.4).

---

## 7. Open questions & risks

**Debate resolutions (recorded).**

- **`h3`-version alignment is the top priority (resolved: must-fix).** It outranks
  the A-vs-B choice; the full Option A adaptor makes skew *later and more
  confusing* by removing the Cargo edge that forces reconciliation. Fix = `pub use
  h3;` + same-version contract + compat table (§4.2).
- **Server `AcceptorFn` (resolved: deferred/rejected).** A `FnMut` closure cannot
  model the stateful lending accept loop; use a direct `H3Acceptor` impl until a
  proven lending/poll design exists (§4.4).
- **Client `ConnectorFn` clone hazard (resolved: documented contract).** The client
  clones the connector before every reconnect (`h3-util/src/client_conn.rs:272-278`),
  so the convenience tier ships with a clone/reconnect contract requiring
  shared-handle captures and a repeatable `connect()` (§4.3.1). Not disqualifying.
- **Demand is unproven (resolved: defer).** Zero out-of-tree backends exist; three
  of four in-tree backends are experimental (`h3-util/src/lib.rs:28-31`). The
  direct impl already works, so the adaptor tier is deferred until a real author
  requests it (YAGNI).
- **Second, non-converging pattern (resolved: migration-gated).** If the adaptor is
  ever adopted, adopt it *with* a migration path that reimplements the in-tree
  backends on it, so there is one converging pattern rather than two.

**Remaining open questions.**

1. **`Clone` + `Send + 'static` on the client factory.** `H3Connector: Send +
   'static + Clone` (`h3-util/src/client.rs:26`) forces a direct impl's connector
   (or a deferred factory) to be `Clone + Send + 'static`. Most connectors holding
   a cloneable `Endpoint` satisfy this (`h3-util/src/quinn/client.rs:6-18`), but it
   is a real constraint worth documenting with a working example.
2. **Error conversion surface.** Relying on `E: Into<crate::Error>` is maximally
   permissive (any `Error + Send + Sync + 'static` works via the blanket `From`),
   but backend authors returning a non-`Send`/non-`Sync` error would not compile;
   document the `Send + Sync` requirement (it mirrors `crate::Error`,
   `h3-util/src/lib.rs:56`).
3. **Boxed error conversion loses attempt context.** `map_err(Into::into)` preserves
   the source chain, but a generic client adaptor cannot aggregate multi-address
   connect failures the way the real quinn connector does (`h3-util/src/quinn/client.rs:31-55`).
   A direct impl can; a deferred `ConnectorFn` cannot — another reason the direct
   impl is the primary path.
4. **Executor plumbing.** `SharedExec` is consumer-provided
   (`h3-util/src/client.rs:108-114`; `h3-util/src/executor.rs`), so out-of-tree
   backends need no executor awareness — but a backend that spawns its own tasks
   (as `H3QuinnAcceptor` does via `JoinSet`, `h3-util/src/quinn/server.rs:53`) owns
   that internally, which a direct `H3Acceptor` impl handles naturally.
5. **Naming (only if the deferred tier ships).** `connector_fn` vs.
   `H3Connector::from_fn` / builder-style APIs. `*_fn` mirrors `tower::service_fn`
   and reads well; confirm with maintainers when demand arrives.
6. **`trait-variant` / RPITIT.** The traits already use return-position `impl
   Future` (`h3-util/src/client.rs:38-40`); direct impls and any deferred adaptor
   use the same form, so no `async_trait`/`trait-variant` boxing is introduced and
   the zero-cost property is preserved.

---

## 8. Summary of recommendation

- **Verdict: B-now + A-later.** Integrate out-of-tree backends *now* with a
  **direct `H3Connector` / `H3Acceptor` impl** (§4.1) — the orphan rule already
  permits it (foreign trait, local type), it matches all four in-tree backends, and
  it is the most diagnosable single pattern. Put the backend's `h3-util` dependency
  behind an **off-by-default cargo feature** so non-`tonic` users get a pure-`h3`
  graph while Cargo still reconciles the `h3` version.
- **Two must-fixes outrank the whole adaptor question:**
  1. **`h3`-version alignment** — add `pub use h3;` to `h3-util` (none exists
     today), document a mandatory same-`h3`-version contract, and publish a compat
     table keyed by exact `h3` line. Version skew is an opaque trait-projection
     break under `0.0.x` caret rules; the full adaptor makes it *later and more
     confusing* by removing the Cargo reconciliation edge (§4.2).
  2. **No server `AcceptorFn`** — a `FnMut` closure cannot express the stateful
     lending accept loop; stateful server backends use a direct `H3Acceptor` impl
     (§4.4).
- **The client `ConnectorFn` is a deferred, contract-bound convenience** — ship it
  only when a concrete out-of-tree author requests it, and only with the
  clone/reconnect contract (shared-handle captures, repeatable `connect()`; §4.3).
  It is the only adaptor that was compile-verified.
- **The change is minimal and additive:** in the near term, just `pub use h3;` plus
  docs. In-tree backends and all consumers are unchanged, and the production quinn
  path does not regress. Decoupling remains *partial by nature* — the `h3::quic::*`
  version coupling is irreducible under either path.
