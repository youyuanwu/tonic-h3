# Deferred items

This document tracks review findings that were **intentionally not addressed**
in the deep-review fix cycle, along with the rationale for deferral. It is the
backlog companion to the completed fixes documented elsewhere in `docs/`.

The findings originate from the Society-of-Thought deep review of the workspace
crates (`tonic-h3`, `axum-h3`, `h3-util`). Each entry records what the finding
is, why it was deferred, and what a future fix would entail.

## Status of the must-fix (MF) items

| Item | Summary | Status |
|------|---------|--------|
| MF-1 | Quinn address failover aborts on first async handshake failure | ✅ Fixed (PR #31) |
| MF-2 | Transient connect/DNS failure permanently bricks buffered `H3Channel` | ✅ Fixed (PR #31) |
| MF-3 | Interrupted body-send appears as graceful FIN instead of an HTTP/3 reset | ✅ Fixed (PR #32) |
| MF-4 | Server spawns handshake/connection/request tasks without explicit caps | ⏸️ Deferred — see below |

---

## MF-4 — Unbounded server task spawning (reclassified: not a distinctive defect)

**Where:** `h3-util/src/quinn/server.rs` (handshake `JoinSet`), `axum-h3/src/lib.rs`
(per-connection and per-request `executor.execute`), `h3-util/src/executor.rs`
(`SharedExec::execute` returns no handle).

**Original concern:** Each incoming QUIC handshake, accepted connection, and
HTTP/3 request stream is spawned without semaphores, retained handles, queue
limits, or backpressure — framed by the security specialist as a
memory-exhaustion DoS lever.

**Why deferred (parity analysis):** When benchmarked against the reference
`tonic`-over-HTTP/2 (`hyper`/`h2`) stack, this is **not a distinctive defect**:

- **Per-connection request concurrency is already bounded by the QUIC
  transport.** Quinn's default `TransportConfig` sets
  `max_concurrent_bidi_streams = 100`
  (`quinn-proto/src/config/transport.rs`), advertised to the peer as the QUIC
  `initial_max_streams_bidi` transport parameter. A peer therefore cannot hold
  more than ~100 concurrent request streams per connection, and credit is
  returned as each request completes. This is the direct analogue of HTTP/2's
  `SETTINGS_MAX_CONCURRENT_STREAMS` (hyper's default is 200). The per-request
  task loop is thus **not** truly unbounded.
- **Connection-accept is unbounded in both stacks.** `tonic`/`hyper` also spawn
  one task per accepted connection in an unbounded accept loop. Connection-level
  DoS is conventionally mitigated at deployment (load balancer, `ulimit`, OS
  accept backlog) and by Quinn's own address-validation / retry-token machinery
  and `incoming_buffer_size_total` limit — not by the application transport
  crate.

**Conclusion:** Reclassified from "must-fix DoS" to **optional future
enhancement**. Matching the ecosystem baseline is acceptable for the current
production (`quinn`) posture.

**If revisited, a proper fix would be a *feature*, not a bug fix:**
- Expose configurable limits: max concurrent connections, max in-flight
  handshakes, and (optionally) max in-flight requests below the transport cap.
- Provide shedding/backpressure semantics (reject or queue with a bound) rather
  than unbounded spawning, comparable to `tower`'s `concurrency_limit` /
  `load_shed` layers.
- Retain task handles so limits and graceful shutdown (see SF-2) can coordinate.

---

## Other open findings from the deep review (not yet scheduled)

These are lower-severity findings surfaced by the review that remain open. Full
evidence and file:line references live in the review synthesis artifact
(`.paw/reviews/crates-deep-review/REVIEW-SYNTHESIS.md`, not committed).

### Should-fix

| ID | Summary | Notes |
|----|---------|-------|
| SF-1 | `RequestSender::call` can panic on URIs missing scheme/authority/path | Validate the base URI in the channel constructor; return a typed error instead of `unwrap()` in the hot path. |
| SF-2 | `serve_with_shutdown` stops accepting but does not drain in-flight work | Decide stop-only vs graceful-drain semantics; if graceful, retain task handles + drain with a timeout. Couples with MF-4. |
| SF-3 | A peer `GOAWAY` can pin the client to a sender that rejects new requests | Retire the cached `send_request` when it reports `RemoteClosing` at call time, not only when the driver task ends. |
| SF-4 | Opaque `Box<dyn Error>` prevents gRPC status mapping and selective retry | Introduce a non-exhaustive transport error enum preserving the source; own a `tonic::Status` mapping policy. |
| SF-5 | The generic HTTP/3 server driver lives behind the axum adapter | Consider moving the framework-neutral connection/request loop into `h3-util`, keeping axum/tonic as thin adapters. |
| SF-6 | Destination identity is split across connector URI, channel URI, and TLS name | A validated endpoint/target type with derived defaults would prevent accidental mismatches. |
| SF-7 | Trace logs can expose sensitive request headers and trailers | Redact sensitive gRPC metadata (e.g. `authorization`) or log only header names. |
| SF-8 | The client `Buffer` is fixed at 1024 entries with no public sizing control | Expose a builder/constructor option and document backpressure/sizing guidance. |

### Consider

| ID | Summary |
|----|---------|
| C-1 | Client/server incoming-body and send-body types duplicate near-identical frame state machines (do after any body-semantic changes). |
| C-2 | Body send loops keep polling after trailers; a malformed `Body` could write an invalid frame sequence. Track a `trailers_sent` state. |
| C-3 | Reconnect state is encoded as coupled `Option` fields; an explicit state enum + fake-connector tests would improve testability. |
| C-4 | `H3Router` stores a `SharedExec` but `new` always uses Tokio and offers no `with_executor`. Expose it or document Tokio-only. |
| C-5 | Backend features don't separate client vs server capability, so client-only users compile the axum server stack. |
| C-6 | `H3Connector`/`H3Acceptor` require implementers to repeat stream associated types already fixed by the connection type. |
| C-7 | The unary client path allocates a cancellation channel and boxes futures before knowing whether a background body task is needed. |
| C-8 | Stale commented-out code / TODOs and typoed trace strings (`incomming`) obscure intent. |

---

## Provenance

Deferral decisions were made during an evaluate-then-fix cycle in which each
must-fix finding was verified against real dependency source
(`tower`, `quinn`/`quinn-proto`, `h3`). MF-1/MF-2/MF-3 were fixed; MF-4 was
reclassified as above. The should-fix and consider items were not in scope for
that cycle and are recorded here so they are not lost.
