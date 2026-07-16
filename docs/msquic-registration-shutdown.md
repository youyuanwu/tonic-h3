# MsQuic Registration Shutdown and the Client Waiter

Why `H3MsQuicClientWaiter` exists, what msquic constraint forces it, and how it
could move upstream.

## The teardown contract

An msquic `Registration` owns the worker pool and a *rundown* that every
connection registers with. Correct teardown for a client is a strict order:

1. `Registration::shutdown()` — asynchronously queues shutdown to all
   connections. Non-blocking.
2. Each connection eventually reports `SHUTDOWN_COMPLETE`, and the owning
   `Connection` object is dropped (which closes the connection handle).
3. `drop(Registration)` — calls `RegistrationClose`, which **synchronously
   blocks the calling thread** until every connection has released the rundown
   ref.

If step 3 runs on a Tokio worker thread while connections are still alive, the
blocking FFI call stalls the runtime and can deadlock. This is documented in
`msquic_h3::Connection::connect`:

> Registration must be kept outside of connection and must wait for all
> connections to finish before closing, else registration close will wait on
> system lock, and block rust runtime.

## This is a msquic limitation, not a binding gap

The registration API surface in `msquic.h` is only three functions —
`RegistrationOpen`, `RegistrationShutdown`, `RegistrationClose`. There is **no
registration-level event, callback, or `QUIC_PARAM`** for "idle" / "all
connections closed". Connections have `CONNECTION_EVENT_SHUTDOWN_COMPLETE`; the
registration has no equivalent async signal.

`RegistrationClose` is a synchronous, un-cancellable, un-timed block:

- `MsQuicRegistrationClose` → `CxPlatRundownReleaseAndWait` (`registration.c`).
- On posix that is `CxPlatEventWaitForever` — a bare blocking wait on a pthread
  event, no timeout (`platform_posix.c`).

What it waits on matters: the registration rundown ref is acquired per
connection in `QuicConnRegister` and released in `QuicConnUnregister`, which
runs from `QuicConnCloseHandle` (`connection.c`). That is, the ref is released
when the connection **handle is closed** — i.e. when the Rust
`msquic::Connection` is *dropped* — not merely when QUIC shutdown completes. So
`RegistrationClose` returns only after every `Connection` object has been
dropped.

Even msquic's own C++ reference wrapper concedes the model:
`~MsQuicRegistration` (`msquic.hpp`) does nothing more than an optional
`RegistrationShutdown(SILENT)` followed by the blocking `RegistrationClose`.
There is no async idle idiom upstream.

## The client-side workaround: `H3MsQuicClientWaiter`

Because msquic gives no registration-idle signal, the application reconstructs
one from the per-connection `SHUTDOWN_COMPLETE` events that `msquic-h3` exposes
via `ConnectionShutdownWaiter`.

See [`h3-util/src/msquic/client.rs`](../h3-util/src/msquic/client.rs).

The waiter tracks an active-connection count:

- `track()` (called from `H3Connector::connect`) takes the new connection's
  `ConnectionShutdownWaiter`, increments a `tokio::sync::watch<usize>`, and
  spawns a task that awaits the waiter and decrements on completion.
- `wait_shutdown()` uses `watch::Receiver::wait_for(|&n| n == 0)` — returning
  immediately when idle, otherwise blocking until every tracked connection
  (including reconnects) has reported shutdown.

Usage is: `reg.shutdown()` → `waiter.wait_shutdown().await` → `drop(reg)`.

### Why a counter and not a single slot

The original implementation stored a single `Option<ConnectionShutdownWaiter>`
and overwrote it on each connect ("the url is unique, so there is at most one
connection"). That holds only at steady state. On **reconnect**
(`RequestSender::poll_ready` rebuilds the connection when the h3 driver dies) a
new connection's waiter overwrote the old one, so `wait_shutdown()` awaited only
the last connection and `drop(reg)` could still block on a straggler — the
source of the intermittent "sometimes stuck here" hangs in the tests. The
counter tracks all connections, closing that race.

## Could this move upstream into msquic-h3?

Yes, but with a dependency constraint. `msquic-h3` 0.0.5 is deliberately
**executor-agnostic**: its runtime dependencies are `bytes`, `futures`
(`features = ["std"]`), `h3`, `msquic`, and optional `tracing`. **`tokio` is a
dev-dependency only.** All its async plumbing uses `futures::channel::{oneshot,
mpsc}`.

Implications for an upstream `Registration::wait_idle()`:

- It must be built on `futures`, **not** tokio. It cannot use
  `tokio::sync::watch` / `Notify` / `tokio::spawn` without adding a runtime
  dependency and breaking executor neutrality. The right primitive is
  `futures::task::AtomicWaker` + an `AtomicUsize` (or a `futures` channel),
  giving a lock-free "wait until count == 0" future.
- Since `msquic-h3` owns both `Connection::connect`/`attach` **and**
  `Drop for Connection`, it can increment on create and decrement in `Drop`
  (after `ConnectionClose`). That is exactly where msquic releases the rundown
  ref, making a `wait_idle` keyed on connection **Drop** both correct and
  race-free — strictly better than the app-side waiter, which signals on
  `SHUTDOWN_COMPLETE` (slightly *before* the handle close) and can therefore
  race the final `ConnectionClose`.

A "spawn_blocking around `RegistrationClose`" approach is **not** a good
upstream fit: it requires tokio and does not remove the ordering requirement
(close still blocks forever if any `Connection` is alive). It only moves the
unavoidable blocking wait off the runtime thread.

### Recommendation

The most impactful upstream change is a `futures`-based
`Registration::wait_idle()` in `msquic-h3` that tracks live connections and
signals on connection `Drop`. That would let client code delete the
`H3MsQuicClientWaiter` shim entirely, while the current tokio-based waiter in
`h3-util` remains valid because `h3-util` already depends on tokio.
