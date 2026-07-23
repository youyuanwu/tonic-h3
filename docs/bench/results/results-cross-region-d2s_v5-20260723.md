# Results — cross-region, `Standard_D2s_v5` (2026-07-23)

Curated results from the apples-to-apples comparison matrix
([`scenarios.yml`](../../../tests/infra/ansible/scenarios.yml)): every comparable
transport runs the same workload points. Raw run artifacts live in the
git-ignored `tests/infra/ansible/results/` and are **not** committed; only these
curated tables are.

Companion runs on the same hardware/software:
[same-zone](results-same-zone-d2s_v5-20260723.md) ·
[cross-zone](results-cross-zone-d2s_v5-20260723.md).

## Setup / provenance

| Field | Value |
|-------|-------|
| Topology | `cross-region` (client eastus2 ↔ server westus2, peered private VNets) |
| Measured RTT | **~67 ms** (`ping` min/avg/max 65.9 / 67.0 / 70.4 ms, 0 % loss) |
| VM SKU | `Standard_D2s_v5` (2 vCPU / 8 GiB, Accelerated Networking) |
| OS image | Ubuntu 26.04 LTS (`ubuntu-26_04-lts:server`, glibc 2.43) |
| Client → Server | `tonich3-client` (eastus2, 10.20.1.4) → `tonich3-server` (westus2, 10.30.1.4) |
| `libmsquic` | 2.5.8 |
| Repo commit | `646ca4e` (harness recorded this HEAD; the apples-to-apples matrix, warmup, and durations were uncommitted local edits at run time — pin to the landing commit for exact repro) |
| Run id / UTC | `a9a20067b0bd5448` / `20260723T043550Z` |
| Result | 20 / 20 scenarios, **0 failures** |

**Method.** Each transport runs the identical five workload points. Throughput
points use a fixed 15 s `duration` at steady state with a 3 s warmup (discarded),
uniform across transports. The latency point uses a fixed count — **reduced to
2 000 requests for cross-region** (vs 20 000 same/cross-zone) because at ~67 ms
RTT a c=1 serialized run of 20 000 requests would take ~22 min per transport;
2 000 still gives a stable percentile sample. gRPC echo protocol over the
private cross-region link. `quiche` excluded (single-stream). Transports:
`tcp-tls` (HTTP/2 baseline), `quinn` (**production** QUIC), `msquic` / `s2n-quic`
(experimental QUIC).

> **Reading these numbers:** at ~67 ms RTT the round-trip time dominates. A c=1
> run is pinned at ~1 RTT/request (~15 req/s) for every transport, so the c=1
> table measures the link, not the transport. The interesting signal is in the
> **concurrent** points, where transport behavior under a high bandwidth-delay
> product (head-of-line blocking, flow-control windows, loss recovery) diverges.

---

## Point 1 — latency probe (64 B, concurrency 1, count 2 000)

Pure RTT floor — all transports are identical (one serialized round-trip per
request). This row validates the link, not the transport.

| Transport  | Throughput (req/s) | p50 (ms) | p90 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|----------|--------|
| `tcp-tls`  | 15                 | 65.983   | 66.111   | 66.303   | 0      |
| `quinn`    | 15                 | 65.983   | 66.175   | 66.367   | 0      |
| `msquic`   | 15                 | 65.919   | 66.047   | 66.303   | 0      |
| `s2n-quic` | 15                 | 65.983   | 66.175   | 66.367   | 0      |

All four sit at the ~66 ms RTT floor, as expected.

## Point 2 — small-payload throughput (64 B, concurrency 32, 15 s)

| Transport  | Throughput (req/s) | p50 (ms) | p90 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|----------|--------|
| `quinn`    | 485                | 66.047   | 66.303   | 66.751   | 0      |
| `s2n-quic` | 485                | 66.047   | 66.303   | 66.751   | 0      |
| `msquic`   | 480                | 66.687   | 67.135   | 67.903   | 0      |
| `tcp-tls`  | 335                | 68.607   | 131.455  | 132.095  | 0      |

**QUIC wins clearly.** All three QUIC backends hold a clean ~66 ms p99 and
~45 % higher throughput than `tcp-tls`, whose p90/p99 balloon to ~132 ms (≈ 2
RTT) — classic HTTP/2 head-of-line blocking over a fat-pipe WAN link, where a
single TCP connection serializes independent streams.

## Point 3 — high-concurrency throughput (64 B, concurrency 128, 15 s)

| Transport  | Throughput (req/s) | p50 (ms) | p90 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|----------|--------|
| `msquic`   | 1 860              | 69.055   | 69.887   | 70.783   | 0      |
| `tcp-tls`  | 1 787              | 66.559   | 85.567   | 120.639  | 0      |
| `quinn`    | 730                | 157.055  | 263.935  | 395.775  | 0      |
| `s2n-quic` | 561                | 199.167  | 329.983  | 397.311  | 0      |

**The ordering inverts at very high concurrency.** `msquic` scales cleanly to
1 860 req/s with a tight ~70 ms tail; `tcp-tls` also scales (1 787). But `quinn`
and `s2n-quic` **collapse** to 730 / 561 req/s with p99 tails near 400 ms — the
Rust userspace QUIC stacks appear flow-control / stream-window limited at 128-way
concurrency over the high bandwidth-delay-product link (they do not reach the
~1 900 req/s the RTT budget allows). This is the most important finding of the
cross-region run and a concrete `quinn` scaling limit worth investigating.

## Point 4 — medium-payload throughput (4 KiB, concurrency 32, 15 s)

| Transport  | Throughput (req/s) | p50 (ms) | p90 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|----------|--------|
| `quinn`    | 484                | 65.791   | 66.239   | 69.119   | 0      |
| `s2n-quic` | 484                | 65.855   | 66.943   | 68.159   | 0      |
| `msquic`   | 476                | 67.327   | 67.967   | 68.415   | 0      |
| `tcp-tls`  | 466                | 66.047   | 67.263   | 123.903  | 0      |

At c=32 all transports are ~1 RTT/request (RTT-bound). QUIC backends hold a
tight ~68 ms p99; `tcp-tls` matches on throughput but shows a ~124 ms p99 tail.

## Point 5 — large-payload throughput (64 KiB, concurrency 32, 15 s)

| Transport  | Throughput (req/s) | p50 (ms) | p90 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|----------|--------|
| `quinn`    | 479                | 66.495   | 66.943   | 68.543   | 0      |
| `s2n-quic` | 477                | 66.431   | 67.583   | 75.583   | 0      |
| `tcp-tls`  | 221                | 136.575  | 187.391  | 205.183  | 0      |
| `msquic`   | 161                | 198.271  | 199.039  | 204.031  | 0      |

**The biggest QUIC win.** `quinn` and `s2n-quic` sustain ~478 req/s at ~66 ms p50
(1 RTT), while `tcp-tls` (221) and `msquic` (161) are 2–3× slower with ~2–3 RTT
p50 — over a high-BDP link the 64 KiB response needs a large in-flight window,
and `quinn`/`s2n-quic` handle it far better here. (Note the c=128 winner `msquic`
is the c=32/64 KiB **loser** — its large-payload flow control lags on this link.)

---

## Narrative

- **Headline:** At ~67 ms cross-region RTT, **HTTP/3 (QUIC) decisively beats the
  TCP/TLS baseline** at moderate concurrency and at large payloads — the exact
  regimes where TCP head-of-line blocking hurts. `tcp-tls` p99 repeatedly
  inflates to ~2 RTT (~132 ms) while the QUIC backends hold ~1 RTT.
- **Where production `quinn` wins:** 64 B / c=32 (+45 % vs baseline, 2× tighter
  tail) and 64 KiB / c=32 (2× the baseline throughput). This is the strongest
  case for `tonic-h3` over `tonic`+TLS in the whole sweep.
- **Where `quinn` loses — a real scaling limit:** at 64 B / c=128 both `quinn`
  and `s2n-quic` collapse (730 / 561 req/s, ~400 ms p99) while `msquic` and
  `tcp-tls` scale to ~1 800. The userspace Rust QUIC stacks appear flow-control /
  stream-window bound at very high concurrency over a high-BDP link — a concrete
  follow-up for the production backend.
- **vs same/cross-zone:** In-zone/in-region (sub-ms RTT) the baseline led almost
  everywhere; adding real WAN RTT flips the result — QUIC's loss/HoL-independent
  streams are worth 45–100 % here. RTT, not raw CPU, is the deciding variable.
- **Caveats:** 2 vCPU; the c=1 latency point used count 2 000 (not 20 000) for
  runtime — it only measures the RTT floor anyway. `msquic` / `s2n-quic` are
  experimental. `quiche` excluded (single-stream). All 20 scenarios `failed = 0`.

## Reproducing

```bash
tests/infra/scripts/deploy.sh cross-region         # eastus2 (client) + westus2 (server)
cd tests/infra/ansible
python3 inventory.py --resource-group rg-tonich3-bench-cross-region
# latency point reduced to count 2000 for the ~67ms RTT link:
ansible-playbook run-bench.yml -e @scenarios.yml \
  -e '{"scenarios":[ ...same as scenarios.yml, but the c=1 points use count: 2000 ]}' \
  -e topology=cross-region -e vm_size=Standard_D2s_v5 -e region=eastus2-westus2 \
  -e bench_port=50051 -e bench_libmsquic_version=2.5.8
```

Each cell traces back to a `result-*.json` + `*.meta.json` pair in the
git-ignored results dir (run id `a9a20067b0bd5448`).
