# Results — same-zone, `Standard_D2s_v5` (2026-07-25)

Curated results from the apples-to-apples comparison matrix
([`scenarios.yml`](../../../tests/infra/ansible/scenarios.yml)): every transport
runs the exact same workload points. This is the first run to include **quiche**
as a full transport (the high-concurrency stall was fixed in `quiche-h3` 0.0.3).
Raw run artifacts live in the git-ignored `tests/infra/ansible/results/` and are
**not** committed; only these curated tables are.

Earlier 4-transport runs are archived under [`archive/`](archive/).

## Setup / provenance

| Field | Value |
|-------|-------|
| Topology | `same-zone` (both VMs in eastus2, zone 1, peered private VNet) |
| VM SKU | `Standard_D2s_v5` (2 vCPU / 8 GiB, Accelerated Networking) |
| OS image | Ubuntu 26.04 LTS (`ubuntu-26_04-lts:server`, glibc 2.43) |
| Region | `eastus2` |
| Client → Server | `tonich3-client` (10.20.1.4) → `tonich3-server` (10.20.1.5) |
| `libmsquic` | 2.5.8 |
| `quiche-h3` | 0.0.3 |
| Repo commit | `9fad5f3` |
| Run id / UTC | `33dc12016ac0b0aa` / `20260725T055440Z` |
| Result | 25 / 25 scenarios, **0 failures** |

**Method.** Each of the five transports runs the identical five workload points.
Throughput points use a fixed 15 s `duration` at steady state with a 3 s warmup
(discarded), uniform across transports; the latency point uses a fixed count of
20 000 requests (no warmup). gRPC echo over the private network. Transports:
`tcp-tls` (tonic + tonic-tls, HTTP/2 baseline), `quinn` (**production** QUIC),
`msquic` / `s2n-quic` / `quiche` (experimental QUIC).

---

## Point 1 — latency probe (64 B, concurrency 1, count 20 000)

| Transport  | Throughput (req/s) | p50 (ms) | p90 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|----------|--------|
| `tcp-tls`  | 5 527              | 0.178    | —        | 0.228    | 0      |
| `quinn`    | 5 219              | 0.187    | —        | 0.256    | 0      |
| `s2n-quic` | 4 827              | 0.204    | —        | 0.254    | 0      |
| `quiche`   | 3 292              | 0.296    | —        | 0.445    | 0      |
| `msquic`   | 3 055              | 0.317    | —        | 0.481    | 0      |

`tcp-tls`/`quinn` lead; `quiche` and `msquic` share the slowest single-request path.

## Point 2 — small-payload throughput (64 B, concurrency 32, 15 s)

| Transport  | Throughput (req/s) | p50 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|--------|
| `tcp-tls`  | 47 876             | 0.651    | 1.089    | 0      |
| `s2n-quic` | 45 672             | 0.681    | 1.257    | 0      |
| `quinn`    | 42 211             | 0.740    | 1.423    | 0      |
| `msquic`   | 27 563             | 1.149    | 1.646    | 0      |
| `quiche`   | 20 525             | 1.492    | 2.773    | 0      |

## Point 3 — high-concurrency throughput (64 B, concurrency 128, 15 s)

| Transport  | Throughput (req/s) | p50 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|--------|
| `tcp-tls`  | 65 109             | 1.881    | 3.257    | 0      |
| `s2n-quic` | 50 733             | 2.413    | 5.171    | 0      |
| `quinn`    | 49 827             | 2.373    | 5.671    | 0      |
| `msquic`   | 35 702             | 3.619    | 4.839    | 0      |
| `quiche`   | 20 448             | 6.175    | 9.639    | 0      |

`quiche` does **not** scale with concurrency here (~20.5k at both c=32 and c=128)
— it is the slowest QUIC backend but completes cleanly with no failures.

## Point 4 — medium-payload throughput (4 KiB, concurrency 32, 15 s)

| Transport  | Throughput (req/s) | p50 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|--------|
| `s2n-quic` | 27 158             | 1.152    | 1.821    | 0      |
| `tcp-tls`  | 26 781             | 1.174    | 1.849    | 0      |
| `quinn`    | 25 557             | 1.249    | 1.940    | 0      |
| `msquic`   | 18 914             | 1.696    | 2.109    | 0      |
| `quiche`   | 14 104             | 2.209    | 3.707    | 0      |

## Point 5 — large-payload throughput (64 KiB, concurrency 32, 15 s)

| Transport  | Throughput (req/s) | p50 (ms) | p99 (ms) | failed |
|------------|--------------------|----------|----------|--------|
| `tcp-tls`  | 6 785              | 4.595    | 7.875    | 0      |
| `msquic`   | 3 683              | 8.623    | 12.551   | 0      |
| `s2n-quic` | 3 236              | 10.983   | 13.687   | 0      |
| `quinn`    | 2 844              | 12.095   | 16.991   | 0      |
| `quiche`   | 2 225              | 14.319   | 22.287   | 0      |

---

## Narrative

- **Headline:** On a 2-vCPU same-zone pair the TCP/TLS (HTTP/2) baseline leads or
  ties everywhere; `s2n-quic` is the strongest QUIC backend, production `quinn`
  is close behind on small payloads, and `msquic`/`quiche` trail.
- **quiche now works at concurrency.** Every quiche scenario completed with 0
  failures (previously it was excluded because it stalled above c=1). It is the
  slowest QUIC backend here and does not gain throughput from c=32→c=128, but it
  is functionally correct.
- **Baseline vs QUIC:** as before, sub-ms same-zone RTT favors the TCP baseline —
  QUIC's loss/HoL advantages don't manifest on a lossless in-zone link. See the
  cross-zone and cross-region runs where the ordering shifts.
- **Caveats:** 2 vCPU is CPU-bound for userspace QUIC at 64 KiB. `msquic`,
  `s2n-quic`, and `quiche` are experimental backends.

## Reproducing

```bash
tests/infra/scripts/deploy.sh same-zone            # Ubuntu 26.04, D2s_v5, eastus2
cd tests/infra/ansible
python3 inventory.py --resource-group rg-tonich3-bench-same-zone
ansible-playbook run-bench.yml -e @scenarios.yml \
  -e topology=same-zone -e vm_size=Standard_D2s_v5 -e region=eastus2 \
  -e bench_port=50051 -e bench_libmsquic_version=2.5.8
```

Each cell traces back to a `result-*.json` + `*.meta.json` pair in the
git-ignored results dir (run id `33dc12016ac0b0aa`).
