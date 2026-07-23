# Results — same-zone, `Standard_D2s_v5` (2026-07-23)

Curated results from the apples-to-apples comparison matrix
([`scenarios.yml`](../../../tests/infra/ansible/scenarios.yml)): every comparable
transport runs the exact same workload points. Raw run artifacts live in the
git-ignored `tests/infra/ansible/results/` and are **not** committed; only these
curated tables are.

## Setup / provenance

| Field | Value |
|-------|-------|
| Topology | `same-zone` (both VMs in eastus2, zone 1, peered private VNet) |
| VM SKU | `Standard_D2s_v5` (2 vCPU / 8 GiB, Accelerated Networking) |
| OS image | Ubuntu 26.04 LTS (`ubuntu-26_04-lts:server`, glibc 2.43) |
| Region | `eastus2` |
| Client → Server | `tonich3-client` (10.20.1.4) → `tonich3-server` (10.20.1.5) |
| `libmsquic` | 2.5.8 |
| Repo commit | `646ca4e` (harness recorded this HEAD; the apples-to-apples matrix, 15 s durations, and 3 s warmup were uncommitted local edits at run time — pin to the commit that lands them for an exact repro) |
| Run id / UTC | `2a4b0609f0fb1c40` / `20260723T034733Z` |
| Result | 20 / 20 scenarios, **0 failures** |

**Method.** Each transport runs the identical five workload points. Throughput
points use a fixed 15 s `duration` at steady state with a 3 s warmup (discarded)
applied uniformly to every transport; the latency point uses a fixed count of
20 000 requests (no warmup) so per-request percentiles sample every request.
gRPC echo protocol over the private network. `quiche` is excluded (single-stream
only — cannot run the shared `concurrency ≥ 32` points).

Transports: `tcp-tls` (tonic + tonic-tls over TCP/TLS, HTTP/2 — the non-QUIC
baseline), `quinn` (tonic-h3 over QUIC, **production**), `msquic` and `s2n-quic`
(tonic-h3 over QUIC, experimental). Throughput is `throughput_rps` (higher is
better); latencies are per-request round-trip milliseconds.

---

## Point 1 — latency probe (64 B, concurrency 1, count 20 000)

Serialized round-trips; isolates per-request latency (no queuing). Best signal
for p50/tail.

| Transport  | Throughput (req/s) | p50 (ms) | p90 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|----------|--------|
| `tcp-tls`  | 5 529              | 0.176    | 0.198    | 0.241    | 0      |
| `quinn`    | 4 842              | 0.203    | 0.224    | 0.266    | 0      |
| `s2n-quic` | 4 520              | 0.217    | 0.238    | 0.271    | 0      |
| `msquic`   | 2 905              | 0.330    | 0.425    | 0.507    | 0      |

Lowest latency is `tcp-tls`; `quinn` and `s2n-quic` follow within ~15 %. `msquic`
has the highest single-request latency (~1.6× the baseline p50).

## Point 2 — small-payload throughput (64 B, concurrency 32, 15 s)

The primary small-RPC throughput comparison.

| Transport  | Throughput (req/s) | p50 (ms) | p90 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|----------|--------|
| `s2n-quic` | 44 575             | 0.711    | 0.859    | 1.026    | 0      |
| `tcp-tls`  | 43 279             | 0.721    | 0.921    | 1.180    | 0      |
| `quinn`    | 38 237             | 0.821    | 1.076    | 1.533    | 0      |
| `msquic`   | 23 963             | 1.313    | 1.658    | 2.031    | 0      |

`s2n-quic` narrowly edges the `tcp-tls` baseline and even posts a lower p99;
`quinn` trails by ~12 %; `msquic` is ~45 % below the leaders.

## Point 3 — high-concurrency throughput (64 B, concurrency 128, 15 s)

Stresses stream multiplexing / scheduling under heavier in-flight load.

| Transport  | Throughput (req/s) | p50 (ms) | p90 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|----------|--------|
| `tcp-tls`  | 61 694             | 1.991    | 2.727    | 3.381    | 0      |
| `s2n-quic` | 49 477             | 2.463    | 3.751    | 5.343    | 0      |
| `quinn`    | 44 959             | 2.633    | 4.419    | 6.327    | 0      |
| `msquic`   | 33 786             | 3.783    | 4.523    | 5.555    | 0      |

At 128-way concurrency the `tcp-tls` baseline pulls clearly ahead (~25 % over the
best QUIC backend). The QUIC backends also show wider p99 tails here, with
`quinn` exhibiting the largest p90→p99 spread.

## Point 4 — medium-payload throughput (4 KiB, concurrency 32, 15 s)

Representative message size; balances framing overhead vs. bandwidth.

| Transport  | Throughput (req/s) | p50 (ms) | p90 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|----------|--------|
| `s2n-quic` | 26 407             | 1.197    | 1.445    | 1.783    | 0      |
| `tcp-tls`  | 25 414             | 1.237    | 1.613    | 1.951    | 0      |
| `quinn`    | 23 523             | 1.355    | 1.739    | 2.171    | 0      |
| `msquic`   | 18 144             | 1.747    | 2.033    | 2.347    | 0      |

Ordering matches the small-payload point: `s2n-quic` ≈ `tcp-tls` > `quinn` >
`msquic`, all bunched within ~30 %.

## Point 5 — large-payload throughput (64 KiB, concurrency 32, 15 s)

Bandwidth-bound regime; surfaces per-transport copy / flow-control overhead.

| Transport  | Throughput (req/s) | p50 (ms) | p90 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|----------|--------|
| `tcp-tls`  | 6 860              | 4.555    | 6.131    | 7.751    | 0      |
| `msquic`   | 3 888              | 8.187    | 10.199   | 11.815   | 0      |
| `s2n-quic` | 3 337              | 10.743   | 12.455   | 13.559   | 0      |
| `quinn`    | 2 786              | 12.431   | 14.863   | 17.055   | 0      |

The `tcp-tls` baseline dominates at 64 KiB (~1.8× the best QUIC backend). Among
the QUIC backends the order inverts versus small payloads: `msquic` leads,
`quinn` (production) is last — the userspace QUIC flow-control / copy path is the
bottleneck at large messages on this 2-vCPU SKU.

---

## Narrative

- **Headline:** On a 2-vCPU same-zone pair the TCP/TLS (HTTP/2) baseline is the
  strongest or tied-strongest at every point except moderate small-payload
  concurrency, where `s2n-quic` narrowly leads. The **production `quinn`** backend
  tracks the leaders within ~12–15 % on small payloads but falls well behind on
  64 KiB messages.
- **Baseline vs QUIC:** `tcp-tls` wins outright at c=1 latency, c=128 throughput,
  and 64 KiB payloads; it is essentially tied with `s2n-quic` at c=32 (64 B / 4 KiB).
  No QUIC backend beats the baseline on tail latency at high concurrency.
- **Among QUIC backends:** `s2n-quic` is the small-payload throughput leader;
  `quinn` is a close second and the lowest-latency QUIC backend at c=1; `msquic`
  trails on small-RPC but is the fastest QUIC backend at 64 KiB.
- **Where QUIC does not yet win:** every regime here is a low-RTT, lossless
  same-zone link — precisely where HTTP/3's loss-recovery / head-of-line-blocking
  advantages do **not** manifest. Expect the QUIC backends to close (or reverse)
  the gap under cross-zone / cross-region RTT and packet loss; those topologies
  are the motivating follow-up.
- **Caveats:** 2 vCPU is easily CPU-bound for userspace QUIC at 64 KiB — a larger
  SKU (`D4s_v5` / `D8s_v5`) may change the large-payload ordering. `msquic` and
  `s2n-quic` are experimental backends. `quiche` was excluded (single-stream).
  All 20 scenarios completed with `failed = 0` and `ok` equal to the requested
  count (or full 15 s duration), so every row is a valid data point.

## Reproducing

Deploy the same topology and re-run the recorded matrix (see the
[run procedure](../run-procedure.md)):

```bash
tests/infra/scripts/deploy.sh same-zone            # Ubuntu 26.04, D2s_v5, eastus2
cd tests/infra/ansible
python3 inventory.py --resource-group rg-tonich3-bench-same-zone
ansible-playbook run-bench.yml -e @scenarios.yml \
  -e topology=same-zone -e vm_size=Standard_D2s_v5 -e region=eastus2 \
  -e bench_port=50051 -e bench_libmsquic_version=2.5.8
```

Each cell traces back to a `result-*.json` + `*.meta.json` pair in the
git-ignored results dir (run id `2a4b0609f0fb1c40`).
