# tonic-h3 gRPC benchmark harness

A self-contained load-testing suite that measures the throughput and latency of
a gRPC **echo** service across every transport that `tonic-h3` supports, plus a
vanilla `tonic` TCP+TLS baseline. It ships two binaries:

- **`bench-server`** — serves the echo service over one selected transport.
- **`bench-client`** — drives configurable load (concurrency, payload size,
  request count or duration) and reports **throughput** (req/s) and **latency
  percentiles** (p50 / p90 / p99).

> This crate (`tonic-h3-bench`, `publish = false`) is a workspace-internal
> benchmark tool. It uses self-signed certificates and a **dangerous
> "accept any certificate" client verifier** so it can run on loopback without
> a PKI — it is **not** an example of production-grade TLS configuration.

> **Running on Azure VMs?** To deploy these binaries to real Azure client/server
> VMs and run the scenario matrix over private networking (collecting JSON
> results across topologies and VM SKUs), see the orchestration framework under
> [`docs/bench/`](../../docs/bench/README.md) and the Ansible playbooks in
> [`tests/infra/ansible/`](../infra/ansible/).

## Transports compared

The same echo RPC runs over five transports, selected with `--transport`:

| `--transport` | Stack                              | Protocol         | Maturity        |
|---------------|------------------------------------|------------------|-----------------|
| `tcp-tls`     | `tonic` + `tonic-tls` (rustls)     | HTTP/2 over TCP  | baseline        |
| `quinn`       | `tonic-h3` + quinn                 | HTTP/3 over QUIC | **production**  |
| `msquic`      | `tonic-h3` + msquic                | HTTP/3 over QUIC | experimental    |
| `s2n-quic`    | `tonic-h3` + s2n-quic              | HTTP/3 over QUIC | experimental    |
| `quiche`      | `tonic-h3` + quiche                | HTTP/3 over QUIC | experimental    |

> **Only the `quinn` backend is supported for production use.** `msquic`,
> `s2n-quic`, and `quiche` are **experimental** and provided for evaluation
> only, matching the support matrix in the [root README](../../README.md).
>
> **Note:** the `quiche` client previously stalled under high concurrency; this
> was fixed in `quiche-h3` 0.0.3, so `quiche` now runs at any concurrency like
> the other transports.

The `tcp-tls` transport negotiates **HTTP/2** (ALPN `h2`); the four QUIC
transports negotiate **HTTP/3** (ALPN `h3`). This lets you compare gRPC over
HTTP/3/QUIC against the established gRPC-over-HTTP/2/TCP baseline.

## The echo protocol

A dedicated, minimal proto (`proto/echo.proto`) with a `bytes` payload so the
client can vary the message size:

```proto
syntax = "proto3";
package echo;
service Echo { rpc Echo (EchoRequest) returns (EchoReply) {} }
message EchoRequest { bytes payload = 1; }
message EchoReply   { bytes payload = 1; }
```

The server echoes the request payload back unchanged. The client fills each
request with a deterministic, non-uniform `--payload-size`-byte pattern. The
**preflight self-check** compares the reply to the request **byte-for-byte**
(validating transport correctness, FR-009); the measured hot path then checks
only the reply **length**, keeping per-request overhead O(1) so the correctness
check does not distort throughput numbers.

## Building

The crate links **all** QUIC backends, so it needs the native `msquic` library
available at build/link time (the workspace already provisions it in CI). Build
just this crate:

```bash
cargo build -p tonic-h3-bench
```

This produces `target/debug/bench-server` and `target/debug/bench-client`.

## Running a benchmark

Start a server in one terminal, then point a client at it in another. The
server prints the **actual** bound address (useful when binding to an ephemeral
`:0` port).

**HTTP/3 over quinn (production backend):**

```bash
# terminal 1 — server
cargo run -p tonic-h3-bench --bin bench-server -- \
    --transport quinn --addr 127.0.0.1:5000

# terminal 2 — client: 50k requests, 32 concurrent workers, 4 KiB payload
cargo run -p tonic-h3-bench --bin bench-client -- \
    --transport quinn --addr 127.0.0.1:5000 \
    --count 50000 --concurrency 32 --payload-size 4096
```

**HTTP/2 over TCP+TLS (baseline):**

```bash
# server
cargo run -p tonic-h3-bench --bin bench-server -- --transport tcp-tls --addr 127.0.0.1:5000
# client
cargo run -p tonic-h3-bench --bin bench-client -- --transport tcp-tls --addr 127.0.0.1:5000 --count 50000
```

**HTTP/3 over msquic (experimental):**

```bash
# server
cargo run -p tonic-h3-bench --bin bench-server -- --transport msquic --addr 127.0.0.1:5000
# client
cargo run -p tonic-h3-bench --bin bench-client -- --transport msquic --addr 127.0.0.1:5000 \
    --count 20000 --concurrency 16
```

**HTTP/3 over s2n-quic (experimental):**

```bash
# server
cargo run -p tonic-h3-bench --bin bench-server -- --transport s2n-quic --addr 127.0.0.1:5000
# client
cargo run -p tonic-h3-bench --bin bench-client -- --transport s2n-quic --addr 127.0.0.1:5000 \
    --count 20000 --concurrency 16
```

**HTTP/3 over quiche (experimental)** — runs at any concurrency (the old
high-concurrency stall was fixed in `quiche-h3` 0.0.3):

```bash
cargo run -p tonic-h3-bench --bin bench-server -- --transport quiche --addr 127.0.0.1:5000
cargo run -p tonic-h3-bench --bin bench-client -- --transport quiche --addr 127.0.0.1:5000 \
    --count 20000 --concurrency 16
```

The server runs until it receives **Ctrl-C** (SIGINT) or **SIGTERM**, then runs
the transport-specific graceful-shutdown recipe so the port is released
promptly (no ~30 s QUIC idle-timeout hang). The client exits automatically when
the run completes and prints the result; it returns a **non-zero exit code** if
zero requests succeeded (e.g. the server was unreachable).

Example client output:

```
=== Benchmark result ===
requests:    50000 total (50000 ok, 0 failed)
elapsed:     12.104 s
throughput:  4131.7 req/s
latency p50: 6.912 ms
latency p90: 9.472 ms
latency p99: 15.231 ms
```

### Machine-readable output (`--format json`)

For collecting many runs across hosts and VM SKUs, pass `--format json` to emit a
single-line JSON object instead of the text block (the default remains `text`, so
existing usage is unchanged):

```bash
cargo run -p tonic-h3-bench --bin bench-client -- \
    --transport quinn --addr 127.0.0.1:5000 --count 50000 --format json
```

```json
{"transport":"quinn","payload_size":64,"concurrency":16,"count":50000,"ok":50000,"failed":0,"elapsed_s":12.104,"throughput_rps":4131.7,"p50_ms":6.912,"p90_ms":9.472,"p99_ms":15.231}
```

Latency percentiles are in **milliseconds** and become JSON `null` when no
requests succeeded; `elapsed_s` is in seconds; `count` is the total number of
requests attempted (`ok + failed`). The Azure run orchestration (see
[`../infra`](../infra/README.md) and [`docs/bench/`](../../docs/bench/README.md))
uses this JSON form so results aggregate cleanly across repeated cross-SKU runs.

Both binaries write **diagnostic logs to stderr** and their result output (the
text block or the JSON line) to **stdout**, so a consumer can capture a clean
result with a simple stdout redirect (`> result.json`) while logs stay on
stderr. Set `RUST_LOG` to adjust log verbosity (default `info`).

## CLI reference

### `bench-server`

| Flag          | Default            | Description                                    |
|---------------|--------------------|------------------------------------------------|
| `--transport` | *(required)*       | One of `tcp-tls`, `quinn`, `msquic`, `s2n-quic`, `quiche`. |
| `--addr`      | `127.0.0.1:5000`   | Address to bind (`host:port`). Use `:0` for an ephemeral port. |

### `bench-client`

| Flag                | Default            | Description                                          |
|---------------------|--------------------|------------------------------------------------------|
| `--transport`       | *(required)*       | Transport to connect with (must match the server).   |
| `--addr`            | `127.0.0.1:5000`   | Server address to target (`host:port`).              |
| `--payload-size`    | `64`               | Echo payload size in bytes.                          |
| `--concurrency`     | `16`               | Number of concurrent in-flight workers (must be >= 1; `0` is rejected). |
| `--count`           | `10000`\*          | Total number of requests to send. Mutually exclusive with `--duration`. |
| `--duration`        | *(unset)*          | Run for this many **seconds** instead of a fixed count. Mutually exclusive with `--count`. |
| `--warmup`          | *(unset)*          | Optional warmup duration in **seconds**; warmup results are discarded. |
| `--connect-timeout` | `5`                | Connect/preflight timeout in **seconds** (fail fast when the server is unreachable). Also bounds the TCP+TLS connect/handshake. |
| `--request-timeout` | `30`               | Per-request timeout in **seconds**. A request exceeding it is counted as a failure so runs always terminate even if a backend stalls. |
| `--format`          | `text`             | Result output format: `text` (human-readable block) or `json` (single-line machine-readable object). |

\* `--count` has no clap-level default value; when **neither** `--count` nor
`--duration` is supplied, the client falls back to a built-in default of
`10000` requests.

## Methodology

Each client run has three stages:

1. **Preflight probe** — a single bounded echo RPC (`--connect-timeout`). If it
   times out or errors, the client fails fast instead of hanging, so a
   misconfigured or down server is reported immediately.
2. **Optional warmup** — if `--warmup N` is given, the client applies load for
   `N` seconds and **discards** those samples, letting connections, TLS
   sessions, and the allocator settle before measurement.
3. **Measured run** — `--concurrency` worker tasks issue echo RPCs as fast as
   they can, either until `--count` requests have been sent (workers pull from a
   shared atomic counter) or until `--duration` seconds elapse. Each request is
   bounded by `--request-timeout`; a request that exceeds it (or otherwise
   errors) is counted as a **failure** rather than hanging the run. Each worker
   records per-request latency into a local histogram; the histograms are then
   merged.

**Metrics:**

- **Throughput** = **successful** request count ÷ wall-clock elapsed of the
  measured run (failed requests are excluded from the rate).
- **Latency percentiles** (p50/p90/p99) are computed from an
  [`hdrhistogram`](https://docs.rs/hdrhistogram) recording per-request wall time
  in microseconds. Only **successful** requests contribute latency samples;
  failed requests are counted separately.

**Environment notes:**

- Certificates are self-signed for `localhost`/`127.0.0.1` and the client uses a
  no-verification verifier — do not read anything into TLS-handshake cost beyond
  "a real handshake happened".
- Loopback runs measure protocol/stack overhead, **not** network behavior. For
  latency- or bandwidth-sensitive comparisons, run client and server on separate
  hosts (see the Azure infra in [`../infra`](../infra/README.md)).

## Interpreting results

- **Compare like with like.** Hold `--payload-size`, `--concurrency`, and the
  request budget (`--count`/`--duration`) constant when comparing transports;
  change only `--transport`.
- **Use `--warmup`** for steady-state numbers. The first requests on a fresh
  connection pay handshake and cold-cache costs that skew short runs.
- **Throughput scales with `--concurrency`** up to the point where the server,
  client, or transport saturates a core or the loopback path. A single worker
  measures round-trip latency; many workers measure pipeline throughput.
- **Watch the `failed` count and `elapsed`.** A non-zero `failed` count or an
  `elapsed` far larger than the latency percentiles imply indicates instability
  rather than throughput. If a run shows this, re-run it and check for a stalled
  or saturated backend.
- **Percentiles over averages.** p99 exposes tail latency (queueing, retransmit,
  GC pauses) that a mean would hide; it is usually the most decision-relevant
  number for RPC systems.

## Layout

```
tests/bench/
├── Cargo.toml            # package `tonic-h3-bench`, publish = false
├── build.rs              # compiles proto/echo.proto with tonic-prost-build
├── proto/echo.proto      # the echo service definition
├── README.md             # this file
└── src/
    ├── lib.rs            # proto include, EchoService, Transport enum, clap args
    ├── tls.rs            # self-signed cert + ALPN-parameterized rustls configs
    ├── server.rs         # per-transport serve + graceful-teardown recipes
    ├── client.rs         # per-transport channel construction
    ├── load.rs           # generic concurrent load generator
    ├── metrics.rs        # hdrhistogram recorder + result summary
    └── bin/
        ├── bench-server.rs
        └── bench-client.rs
```
