# Result showcase template

A reusable scaffold for presenting benchmark numbers as they are collected
**repeatedly, across VM SKUs and over time**. Copy a block, fill the cells from
the JSON files in `tests/infra/ansible/results/`, and commit only the curated
tables here — never the raw run artifacts (they are git-ignored).

> **How to fill a cell.** Each `result-*.json` is one scenario. Read
> `throughput_rps`, `p50_ms`, `p90_ms`, `p99_ms`, and check `failed` / `ok`. The
> companion `*.meta.json` tells you the topology, VM size, transport, payload,
> concurrency, budget, git commit, and timestamp. Note the **git commit** and
> **date** for each table so numbers stay reproducible.

## How to read the numbers

- **`throughput_rps`** — sustained requests/second. Higher is better. Most
  meaningful from a `duration`-based, high-concurrency run at steady state.
- **`p50_ms` (median)** — typical round-trip latency. Best measured at
  `concurrency 1` (no queuing).
- **`p90_ms` / `p99_ms` (tail)** — the slow requests. The gap between p50 and
  p99 reveals jitter / queuing / loss-recovery behavior; QUIC vs TCP tail
  behavior under loss is often the whole point.
- **Always check `failed` and `ok`.** A high throughput number with non-zero
  `failed` (or `ok` far below the requested `count`) is not a valid data point —
  investigate before recording it.
- **quiche is single-stream only** (`concurrency` forced to 1). Do **not** place
  its throughput next to a concurrent `quinn`/`tcp-tls` number as if comparable;
  compare it only against other `concurrency 1` runs, and annotate it.

---

## Table A — by topology × transport (one VM SKU)

Fill one copy of this table **per VM SKU** and **per payload/concurrency point**
you care about. State the fixed axes in the caption.

> **SKU:** `Standard_D2s_v5` · **payload:** 64 B · **concurrency:** 16 ·
> **budget:** count 20 000 · **git commit:** `__________` · **date:** `________`

| Transport  | Topology     | Throughput (req/s) | p50 (ms) | p90 (ms) | p99 (ms) | failed |
|------------|--------------|--------------------|----------|----------|----------|--------|
| `tcp-tls`  | same-zone    |                    |          |          |          |        |
| `tcp-tls`  | cross-zone   |                    |          |          |          |        |
| `tcp-tls`  | cross-region |                    |          |          |          |        |
| `quinn`    | same-zone    |                    |          |          |          |        |
| `quinn`    | cross-zone   |                    |          |          |          |        |
| `quinn`    | cross-region |                    |          |          |          |        |
| `msquic`   | same-zone    |                    |          |          |          |        |
| `msquic`   | cross-zone   |                    |          |          |          |        |
| `msquic`   | cross-region |                    |          |          |          |        |
| `s2n-quic` | same-zone    |                    |          |          |          |        |
| `s2n-quic` | cross-zone   |                    |          |          |          |        |
| `s2n-quic` | cross-region |                    |          |          |          |        |
| `quiche`¹  | same-zone    |                    |          |          |          |        |
| `quiche`¹  | cross-zone   |                    |          |          |          |        |
| `quiche`¹  | cross-region |                    |          |          |          |        |

¹ `quiche` is single-stream (`concurrency 1`); not directly comparable to the
concurrent rows above.

---

## Table B — VM-SKU sweep (one topology × transport)

Fill one copy **per (topology, transport)** pair to see how a transport scales
with VM size. This is the table the framework is really built for — extend it
downward as you add SKUs.

> **Topology:** `cross-zone` · **transport:** `quinn` · **payload:** 64 B ·
> **concurrency:** 32 · **budget:** duration 30 s · **git commit:** `________`

| VM SKU            | vCPU / RAM  | Throughput (req/s) | p50 (ms) | p90 (ms) | p99 (ms) | failed | date |
|-------------------|-------------|--------------------|----------|----------|----------|--------|------|
| `Standard_D2s_v5` | 2 / 8 GiB   |                    |          |          |          |        |      |
| `Standard_D4s_v5` | 4 / 16 GiB  |                    |          |          |          |        |      |
| `Standard_D8s_v5` | 8 / 32 GiB  |                    |          |          |          |        |      |

---

## Table C — payload sweep (one SKU × topology × transport)

Shows how each transport behaves as messages grow (overhead-bound → bandwidth-bound).

> **SKU:** `Standard_D4s_v5` · **topology:** `same-zone` · **transport:** `quinn`
> · **concurrency:** 32 · **git commit:** `________` · **date:** `________`

| Payload  | Throughput (req/s) | p50 (ms) | p90 (ms) | p99 (ms) | failed |
|----------|--------------------|----------|----------|----------|--------|
| 64 B     |                    |          |          |          |        |
| 4 KiB    |                    |          |          |          |        |
| 64 KiB   |                    |          |          |          |        |

---

## Narrative template

For each published sweep, add a short write-up alongside the tables:

- **Setup:** git commit, date, SKU(s), topology(ies), scenario axes,
  `libmsquic` version. (Everything except the write-up is in the `*.meta.json`.)
- **Headline:** the one-sentence takeaway (e.g. "at cross-region RTT, `quinn`
  holds a lower p99 than `tcp-tls` at 32-way concurrency").
- **Baseline vs QUIC:** how each QUIC backend compares to the `tcp-tls`
  baseline on throughput and on tail latency.
- **Where QUIC wins / loses:** call out the topology (RTT) and payload regimes
  where HTTP/3 helps vs where the TCP baseline is still ahead.
- **Caveats:** experimental-backend quirks, the `quiche` single-stream
  restriction, any non-zero `failed` counts, and NIC/CPU saturation if the SKU
  looked like the bottleneck.

## Reproducing a published number

Every table cell traces back to a `result-*.json` + `*.meta.json` pair. To
re-run one, read the metadata (topology, VM size, transport, payload,
concurrency, budget) and re-issue the matching deploy + `run-bench.yml` scenario
per the [run procedure](run-procedure.md). Pin to the recorded `git_commit` for
an exact repro.
