# Results — cross-region, `Standard_D2s_v5` (2026-07-25)

Curated results from the apples-to-apples comparison matrix
([`scenarios.yml`](../../../tests/infra/ansible/scenarios.yml)): every transport
runs the same workload points, now including **quiche** (`quiche-h3` 0.0.3 fixed
the high-concurrency stall). Raw artifacts stay git-ignored; only these curated
tables are committed. Earlier 4-transport runs are under [`archive/`](archive/).

Companion runs: [same-zone](results-same-zone-d2s_v5-20260725.md) ·
[cross-zone](results-cross-zone-d2s_v5-20260725.md).

## Setup / provenance

| Field | Value |
|-------|-------|
| Topology | `cross-region` (client eastus2 ↔ server westus2, peered private VNets) |
| Measured RTT | **~67 ms** (`ping` min/avg/max 66.3 / 66.9 / 68.8 ms, 0 % loss) |
| VM SKU | `Standard_D2s_v5` (2 vCPU / 8 GiB, Accelerated Networking) |
| OS image | Ubuntu 26.04 LTS (`ubuntu-26_04-lts:server`, glibc 2.43) |
| Client → Server | `tonich3-client` (eastus2, 10.20.1.4) → `tonich3-server` (westus2, 10.30.1.4) |
| `libmsquic` | 2.5.8 |
| `quiche-h3` | 0.0.3 |
| Repo commit | `9fad5f3` |
| Run id / UTC | `cd32e078a0c4c349` / `20260725T062815Z` |
| Result | 25 / 25 scenarios, **0 failures** |

**Method.** Five transports × five identical workload points. Throughput points
use a fixed 15 s `duration` with a 3 s warmup (discarded). The latency point uses
a fixed count — **reduced to 2 000** (vs 20 000 in-region) because at ~67 ms RTT a
c=1 serialized run of 20 000 would take ~22 min per transport. gRPC echo over the
private cross-region link.

> At ~67 ms RTT the round-trip dominates: a c=1 run is pinned at ~1 RTT/request
> (~15 req/s) for every transport, so Point 1 measures the link, not the
> transport. The signal is in the concurrent points.

---

## Point 1 — latency probe (64 B, concurrency 1, count 2 000)

All transports sit at the ~66 ms RTT floor (validates the link).

| Transport  | Throughput (req/s) | p50 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|--------|
| `tcp-tls`  | 15                 | 65.919   | 66.175   | 0      |
| `quinn`    | 15                 | 66.047   | 66.303   | 0      |
| `msquic`   | 15                 | 65.919   | 66.175   | 0      |
| `s2n-quic` | 15                 | 65.983   | 66.239   | 0      |
| `quiche`   | 15                 | 65.919   | 66.175   | 0      |

## Point 2 — small-payload throughput (64 B, concurrency 32, 15 s)

| Transport  | Throughput (req/s) | p50 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|--------|
| `quinn`    | 485                | 65.983   | 66.495   | 0      |
| `s2n-quic` | 485                | 65.983   | 66.623   | 0      |
| `quiche`   | 484                | 66.111   | 67.327   | 0      |
| `msquic`   | 480                | 66.687   | 67.135   | 0      |
| `tcp-tls`  | 336                | 67.839   | 131.839  | 0      |

**All four QUIC backends (including quiche) beat the TCP baseline** by ~45 %, each
holding a clean ~66 ms p99 while `tcp-tls` p99 doubles to ~132 ms (HTTP/2 HoL
blocking over the WAN link).

## Point 3 — high-concurrency throughput (64 B, concurrency 128, 15 s)

| Transport  | Throughput (req/s) | p50 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|--------|
| `msquic`   | 1 864              | 68.863   | 70.655   | 0      |
| `tcp-tls`  | 1 787              | 66.431   | 126.143  | 0      |
| `quiche`   | 756                | 190.335  | 197.503  | 0      |
| `quinn`    | 732                | 133.119  | 397.567  | 0      |
| `s2n-quic` | 544                | 199.295  | 397.823  | 0      |

`msquic` and `tcp-tls` scale cleanly to ~1 800. The other three lag, but
**`quiche` holds the tightest tail of them (197 ms p99 vs ~398 ms for `quinn`/
`s2n-quic`)** — a notable inversion: the previously-excluded backend now behaves
better under this stress point than the two it used to be grouped against.

## Point 4 — medium-payload throughput (4 KiB, concurrency 32, 15 s)

| Transport  | Throughput (req/s) | p50 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|--------|
| `quinn`    | 484                | 65.727   | 68.671   | 0      |
| `s2n-quic` | 484                | 65.791   | 68.159   | 0      |
| `quiche`   | 483                | 66.111   | 67.391   | 0      |
| `msquic`   | 473                | 67.775   | 68.415   | 0      |
| `tcp-tls`  | 465                | 66.047   | 126.399  | 0      |

All QUIC backends cluster at ~1 RTT/request with tight ~68 ms tails; `tcp-tls`
matches on throughput but shows a ~126 ms p99.

## Point 5 — large-payload throughput (64 KiB, concurrency 32, 15 s)

| Transport  | Throughput (req/s) | p50 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|--------|
| `quinn`    | 479                | 66.495   | 67.647   | 0      |
| `s2n-quic` | 479                | 66.431   | 73.983   | 0      |
| `quiche`   | 477                | 66.751   | 72.703   | 0      |
| `tcp-tls`  | 224                | 135.807  | 202.495  | 0      |
| `msquic`   | 161                | 198.143  | 202.239  | 0      |

**Biggest QUIC win:** `quinn`, `s2n-quic`, and `quiche` all sustain ~478 req/s at
~66 ms p50 (1 RTT), while `tcp-tls` (224) and `msquic` (161) are 2–3× slower at
2–3 RTT. quiche is fully competitive with the other userspace QUIC stacks here.

---

## Narrative

- **Headline:** At ~67 ms cross-region RTT, HTTP/3 (QUIC) beats the TCP/TLS
  baseline at moderate concurrency and large payloads — and **quiche is now a
  full participant**, competitive with `quinn`/`s2n-quic` at c=32 and holding the
  tightest tail of the three at c=128.
- **quiche cross-region:** ties the QUIC pack at 64 B/c=32 (484 rps), 4 KiB (483),
  and 64 KiB (477); at 64 B/c=128 it delivers 756 rps with a 197 ms p99 vs
  ~398 ms for `quinn`/`s2n-quic`. A very different picture from same/cross-zone,
  where it was the slowest transport.
- **`quinn`/`s2n-quic` scaling limit persists:** both still collapse at 64 B/
  c=128 (732 / 544 rps, ~398 ms p99) where `msquic`/`tcp-tls` scale to ~1 800 —
  the userspace-QUIC high-concurrency / high-BDP limit noted in the archived run.
- **Caveats:** 2 vCPU; the c=1 point used count 2 000 (RTT floor only). `msquic`,
  `s2n-quic`, `quiche` are experimental.

## Reproducing

```bash
tests/infra/scripts/deploy.sh cross-region         # eastus2 (client) + westus2 (server)
cd tests/infra/ansible
python3 inventory.py --resource-group rg-tonich3-bench-cross-region
# latency point reduced to count 2000 for the ~67ms RTT link (else ~22 min/transport):
ansible-playbook run-bench.yml -e @scenarios.yml \
  -e '{"scenarios":[ ...scenarios.yml, but the c=1 points use count: 2000 ]}' \
  -e topology=cross-region -e vm_size=Standard_D2s_v5 -e region=eastus2-westus2 \
  -e bench_port=50051 -e bench_libmsquic_version=2.5.8
```

Each cell traces back to a `result-*.json` + `*.meta.json` pair in the
git-ignored results dir (run id `cd32e078a0c4c349`).
