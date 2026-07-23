# Results — cross-zone, `Standard_D2s_v5` (2026-07-23)

Curated results from the apples-to-apples comparison matrix
([`scenarios.yml`](../../../tests/infra/ansible/scenarios.yml)): every comparable
transport runs the exact same workload points. Raw run artifacts live in the
git-ignored `tests/infra/ansible/results/` and are **not** committed; only these
curated tables are.

For the same hardware/software on a **same-zone** link, see
[results-same-zone-d2s_v5-20260723.md](results-same-zone-d2s_v5-20260723.md).

## Setup / provenance

| Field | Value |
|-------|-------|
| Topology | `cross-zone` (client zone 1, server zone 2, same region, peered private VNet) |
| VM SKU | `Standard_D2s_v5` (2 vCPU / 8 GiB, Accelerated Networking) |
| OS image | Ubuntu 26.04 LTS (`ubuntu-26_04-lts:server`, glibc 2.43) |
| Region | `eastus2` |
| Client → Server | `tonich3-client` (10.20.1.5, zone 1) → `tonich3-server` (10.20.1.4, zone 2) |
| `libmsquic` | 2.5.8 |
| Repo commit | `646ca4e` (harness recorded this HEAD; the apples-to-apples matrix, 15 s durations, and 3 s warmup were uncommitted local edits at run time — pin to the commit that lands them for an exact repro) |
| Run id / UTC | `97342d50ad0a978c` / `20260723T041232Z` |
| Result | 20 / 20 scenarios, **0 failures** |

**Method.** Each transport runs the identical five workload points. Throughput
points use a fixed 15 s `duration` at steady state with a 3 s warmup (discarded)
applied uniformly to every transport; the latency point uses a fixed count of
20 000 requests (no warmup) so per-request percentiles sample every request.
gRPC echo protocol over the private inter-zone link. `quiche` is excluded
(single-stream only). Transports: `tcp-tls` (tonic + tonic-tls over TCP/TLS,
HTTP/2 baseline), `quinn` (tonic-h3 over QUIC, **production**), `msquic` and
`s2n-quic` (tonic-h3 over QUIC, experimental).

---

## Point 1 — latency probe (64 B, concurrency 1, count 20 000)

Serialized round-trips; isolates per-request latency (no queuing). The
inter-zone hop adds ~0.1 ms to every transport's p50 vs same-zone.

| Transport  | Throughput (req/s) | p50 (ms) | p90 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|----------|--------|
| `tcp-tls`  | 3 371              | 0.295    | 0.326    | 0.354    | 0      |
| `quinn`    | 3 146              | 0.315    | 0.345    | 0.376    | 0      |
| `s2n-quic` | 2 750              | 0.360    | 0.392    | 0.425    | 0      |
| `msquic`   | 1 994              | 0.499    | 0.569    | 0.626    | 0      |

`tcp-tls` and `quinn` are effectively tied (within ~7 %); `msquic` is the
slowest single-request path again.

## Point 2 — small-payload throughput (64 B, concurrency 32, 15 s)

| Transport  | Throughput (req/s) | p50 (ms) | p90 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|----------|--------|
| `s2n-quic` | 47 159             | 0.663    | 0.815    | 1.074    | 0      |
| `tcp-tls`  | 44 938             | 0.704    | 0.840    | 0.992    | 0      |
| `quinn`    | 43 407             | 0.732    | 0.891    | 1.039    | 0      |
| `msquic`   | 26 894             | 1.172    | 1.382    | 1.951    | 0      |

`s2n-quic` leads; `quinn` (production) is now within ~3 % of the `tcp-tls`
baseline — noticeably closer than same-zone (~12 % gap there).

## Point 3 — high-concurrency throughput (64 B, concurrency 128, 15 s)

| Transport  | Throughput (req/s) | p50 (ms) | p90 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|----------|--------|
| `tcp-tls`  | 66 644             | 1.842    | 2.497    | 3.193    | 0      |
| `quinn`    | 52 141             | 2.249    | 3.845    | 5.627    | 0      |
| `s2n-quic` | 50 969             | 2.407    | 3.651    | 5.287    | 0      |
| `msquic`   | 36 737             | 3.515    | 4.071    | 4.583    | 0      |

`tcp-tls` again leads clearly at 128-way concurrency; `quinn` edges past
`s2n-quic` here. `msquic` keeps the tightest p90→p99 tail of the QUIC backends.

## Point 4 — medium-payload throughput (4 KiB, concurrency 32, 15 s)

| Transport  | Throughput (req/s) | p50 (ms) | p90 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|----------|--------|
| `s2n-quic` | 27 115             | 1.147    | 1.489    | 1.858    | 0      |
| `quinn`    | 26 954             | 1.187    | 1.452    | 1.699    | 0      |
| `tcp-tls`  | 25 933             | 1.209    | 1.606    | 1.927    | 0      |
| `msquic`   | 18 623             | 1.725    | 1.914    | 2.123    | 0      |

At 4 KiB the QUIC backends `s2n-quic` and **`quinn` both beat the `tcp-tls`
baseline** (quinn also posts the lowest p99) — the first regime where production
QUIC is ahead.

## Point 5 — large-payload throughput (64 KiB, concurrency 32, 15 s)

| Transport  | Throughput (req/s) | p50 (ms) | p90 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|----------|--------|
| `tcp-tls`  | 6 504              | 4.811    | 6.379    | 7.967    | 0      |
| `msquic`   | 4 006              | 7.943    | 9.935    | 11.471   | 0      |
| `s2n-quic` | 3 150              | 11.263   | 12.903   | 14.087   | 0      |
| `quinn`    | 2 839              | 12.239   | 14.711   | 16.591   | 0      |

Same as same-zone: `tcp-tls` dominates 64 KiB (~1.6× the best QUIC backend),
`msquic` is the fastest QUIC backend, `quinn` last — userspace QUIC is
CPU/flow-control bound at large messages on this 2-vCPU SKU.

---

## Narrative

- **Headline:** Moving from same-zone to cross-zone (adding inter-zone RTT but no
  loss) **narrows the gap between production `quinn` and the `tcp-tls` baseline**
  on small/medium payloads, and at 4 KiB the QUIC backends (`s2n-quic`, `quinn`)
  overtake the baseline outright.
- **vs same-zone:** Latency rose ~0.1 ms p50 across the board (the zone hop).
  Small-payload throughput barely moved for `tcp-tls`/`quinn`/`s2n-quic` (this
  link is still not the bottleneck), while `quinn` closed from ~12 % behind the
  baseline (same-zone) to ~3 % (cross-zone).
- **Baseline vs QUIC:** `tcp-tls` still wins c=1 latency, c=128 throughput, and
  64 KiB payloads. QUIC wins at 4 KiB / c=32 and ties at 64 B / c=32.
- **Among QUIC backends:** `s2n-quic` leads small/medium throughput, `quinn` is
  the lowest-latency QUIC backend and best at 4 KiB p99, `msquic` trails on
  small-RPC but leads QUIC at 64 KiB.
- **Where QUIC still does not win:** high concurrency (c=128) and large payloads
  remain baseline territory on 2 vCPU — both are CPU-bound regimes for userspace
  QUIC. A cross-**region** run (higher RTT, possible loss) and a larger SKU are
  the natural next experiments to see HTTP/3's loss-recovery advantage emerge.
- **Caveats:** 2 vCPU is CPU-bound for userspace QUIC at 64 KiB. `msquic` and
  `s2n-quic` are experimental. `quiche` excluded (single-stream). All 20
  scenarios completed with `failed = 0`.

## Reproducing

```bash
tests/infra/scripts/deploy.sh cross-zone           # Ubuntu 26.04, D2s_v5, eastus2 z1/z2
cd tests/infra/ansible
python3 inventory.py --resource-group rg-tonich3-bench-cross-zone
ansible-playbook run-bench.yml -e @scenarios.yml \
  -e topology=cross-zone -e vm_size=Standard_D2s_v5 -e region=eastus2 \
  -e bench_port=50051 -e bench_libmsquic_version=2.5.8
```

Each cell traces back to a `result-*.json` + `*.meta.json` pair in the
git-ignored results dir (run id `97342d50ad0a978c`).
