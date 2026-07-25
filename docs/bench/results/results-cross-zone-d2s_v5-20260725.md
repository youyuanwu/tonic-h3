# Results — cross-zone, `Standard_D2s_v5` (2026-07-25)

Curated results from the apples-to-apples comparison matrix
([`scenarios.yml`](../../../tests/infra/ansible/scenarios.yml)): every transport
runs the exact same workload points, now including **quiche** (`quiche-h3` 0.0.3
fixed the high-concurrency stall). Raw run artifacts stay git-ignored; only these
curated tables are committed. Earlier 4-transport runs are under [`archive/`](archive/).

Companion runs: [same-zone](results-same-zone-d2s_v5-20260725.md) ·
cross-region (same date).

## Setup / provenance

| Field | Value |
|-------|-------|
| Topology | `cross-zone` (client zone 1, server zone 2, same region, peered private VNet) |
| VM SKU | `Standard_D2s_v5` (2 vCPU / 8 GiB, Accelerated Networking) |
| OS image | Ubuntu 26.04 LTS (`ubuntu-26_04-lts:server`, glibc 2.43) |
| Region | `eastus2` |
| Client → Server | `tonich3-client` (10.20.1.4, zone 1) → `tonich3-server` (10.20.1.5, zone 2) |
| `libmsquic` | 2.5.8 |
| `quiche-h3` | 0.0.3 |
| Repo commit | `9fad5f3` |
| Run id / UTC | `578d8708acdb0cd5` / `20260725T061055Z` |
| Result | 25 / 25 scenarios, **0 failures** |

**Method.** Five transports × five identical workload points. Throughput points
use a fixed 15 s `duration` with a 3 s warmup (discarded); the latency point uses
a fixed count of 20 000. gRPC echo over the private inter-zone link. `tcp-tls`
(HTTP/2 baseline), `quinn` (**production** QUIC), `msquic` / `s2n-quic` / `quiche`
(experimental QUIC).

---

## Point 1 — latency probe (64 B, concurrency 1, count 20 000)

| Transport  | Throughput (req/s) | p50 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|--------|
| `tcp-tls`  | 3 326              | 0.297    | 0.362    | 0      |
| `quinn`    | 2 769              | 0.358    | 0.416    | 0      |
| `s2n-quic` | 2 342              | 0.425    | 0.481    | 0      |
| `msquic`   | 1 909              | 0.524    | 0.633    | 0      |
| `quiche`   | 1 813              | 0.552    | 0.625    | 0      |

## Point 2 — small-payload throughput (64 B, concurrency 32, 15 s)

| Transport  | Throughput (req/s) | p50 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|--------|
| `s2n-quic` | 42 613             | 0.728    | 1.395    | 0      |
| `tcp-tls`  | 41 481             | 0.761    | 1.082    | 0      |
| `quinn`    | 38 819             | 0.815    | 1.241    | 0      |
| `msquic`   | 25 547             | 1.249    | 1.596    | 0      |
| `quiche`   | 20 522             | 1.510    | 2.607    | 0      |

## Point 3 — high-concurrency throughput (64 B, concurrency 128, 15 s)

| Transport  | Throughput (req/s) | p50 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|--------|
| `tcp-tls`  | 59 177             | 2.083    | 3.443    | 0      |
| `s2n-quic` | 49 955             | 2.437    | 5.571    | 0      |
| `quinn`    | 46 886             | 2.497    | 6.211    | 0      |
| `msquic`   | 35 312             | 3.657    | 4.787    | 0      |
| `quiche`   | 19 275             | 6.731    | 9.799    | 0      |

## Point 4 — medium-payload throughput (4 KiB, concurrency 32, 15 s)

| Transport  | Throughput (req/s) | p50 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|--------|
| `s2n-quic` | 25 747             | 1.232    | 1.719    | 0      |
| `tcp-tls`  | 24 009             | 1.309    | 2.033    | 0      |
| `quinn`    | 22 658             | 1.417    | 2.137    | 0      |
| `msquic`   | 18 552             | 1.730    | 2.163    | 0      |
| `quiche`   | 12 699             | 2.469    | 4.007    | 0      |

## Point 5 — large-payload throughput (64 KiB, concurrency 32, 15 s)

| Transport  | Throughput (req/s) | p50 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|--------|
| `tcp-tls`  | 6 348              | 4.915    | 8.199    | 0      |
| `msquic`   | 3 895              | 8.083    | 11.919   | 0      |
| `s2n-quic` | 3 122              | 11.399   | 14.391   | 0      |
| `quinn`    | 3 091              | 11.103   | 15.767   | 0      |
| `quiche`   | 1 971              | 16.607   | 26.911   | 0      |

---

## Narrative

- **Headline:** Cross-zone (sub-ms inter-zone RTT) mirrors same-zone: the TCP/TLS
  baseline leads or ties, `s2n-quic` is the best QUIC backend, `quinn` close
  behind on small payloads, `msquic`/`quiche` trail. `quiche` completes every
  point with **0 failures**.
- **quiche behavior:** functional at all concurrencies but the slowest transport;
  like same-zone it does not gain throughput from c=32→c=128 (~20k both).
- **Baseline vs QUIC:** the small inter-zone RTT is still not enough for QUIC's
  loss/HoL advantages to overturn the baseline — that shift appears at
  cross-region RTT (see the cross-region run).
- **Caveats:** 2 vCPU is CPU-bound for userspace QUIC at 64 KiB. `msquic`,
  `s2n-quic`, and `quiche` are experimental.

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
git-ignored results dir (run id `578d8708acdb0cd5`).
