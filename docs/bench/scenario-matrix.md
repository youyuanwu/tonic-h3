# Benchmark scenario matrix

This document defines **what** we measure and **why**. A "scenario" is one
`bench-client` invocation against one `bench-server`, pinned to a specific
transport, payload size, concurrency, and request budget, run on a specific
network topology and VM SKU. The orchestration playbook
([`run-bench.yml`](../../tests/infra/ansible/run-bench.yml)) iterates a list of
scenarios; the topology and VM SKU come from the infrastructure you deployed.

## The four axes

A full data point is the cross product of:

1. **Transport** — the protocol under test.
2. **Payload size** — bytes echoed per request.
3. **Concurrency** — in-flight client workers.
4. **Request budget** — fixed request `count`, or a wall-clock `duration`.

…captured **per network topology** and **per VM SKU** (the two outer axes that
come from the deployed infrastructure, not from the scenario list).

### Transports

| Transport  | Stack                 | Status       | Role in the matrix |
|------------|-----------------------|--------------|--------------------|
| `tcp-tls`  | gRPC over HTTP/2 + TLS | baseline     | The reference every QUIC backend is compared against. |
| `quinn`    | HTTP/3 over QUIC      | production   | The primary HTTP/3 implementation. |
| `msquic`   | HTTP/3 over QUIC      | experimental | Microsoft's C QUIC library (native `libmsquic`). |
| `s2n-quic` | HTTP/3 over QUIC      | experimental | AWS's QUIC implementation. |
| `quiche`   | HTTP/3 over QUIC      | experimental | Cloudflare's QUIC — **unstable at high concurrency**. |

> **quiche caveat.** `quiche` stalls above a single in-flight request, so the
> orchestration **forces `--concurrency 1`** for it regardless of what the
> scenario asks. Treat `quiche` numbers as single-stream only and never compare
> its throughput head-to-head with a concurrent `quinn`/`tcp-tls` run.

### Payload sizes

| Size    | What it isolates |
|---------|------------------|
| `64` B  | Per-request overhead — framing, handshake amortization, syscall/scheduling cost. Latency-dominated. |
| `4096` B (4 KiB) | A realistic small-message RPC; balances overhead and data movement. |
| `65536` B (64 KiB) | Bulk transfer — exercises flow control, congestion control, and copy costs. Throughput-dominated. |

### Concurrency levels

| Concurrency | What it isolates |
|-------------|------------------|
| `1`   | Pure round-trip latency (p50/p90/p99 with no queuing). Also the only valid setting for `quiche`. |
| `16`  | Moderate parallelism — typical service load. |
| `32`+ | Saturation — where multiplexing, congestion control, and CPU start to bound throughput. |

### Request budgets

- **`count: N`** — run exactly N requests. Deterministic, best for
  apples-to-apples latency comparisons and quick smoke runs.
- **`duration: S`** — run for S seconds. Best for steady-state throughput where
  you want the transport to reach equilibrium (warm caches, congestion window
  open). Use `--warmup` on the client for duration runs.

## Network topologies

Provisioned by [`tests/infra`](../../tests/infra/README.md); pick one per deploy.

| Topology       | Latency profile        | What it isolates |
|----------------|------------------------|------------------|
| `same-zone`    | Lowest (co-located)    | CPU / protocol overhead with network latency minimized. |
| `cross-zone`   | Intra-region, inter-AZ | Sub-millisecond-to-low-ms RTT; QUIC vs TCP under realistic datacenter latency. |
| `cross-region` | Inter-region backbone  | Tens of ms RTT; where QUIC's 0/1-RTT handshakes and loss recovery should shine. |

## VM SKU sweep

The infrastructure defaults to **`Standard_D2s_v5`** (2 vCPU / 8 GiB, the
smallest SKU that supports Accelerated Networking + Premium storage). Start
there, then repeat the matrix on larger SKUs to see where the small NIC / CPU
stops being the bottleneck:

| Stage | SKU               | vCPU / RAM  | Why |
|-------|-------------------|-------------|-----|
| 1     | `Standard_D2s_v5` | 2 / 8 GiB   | Baseline; cheapest AccelNet-capable SKU. |
| 2     | `Standard_D4s_v5` | 4 / 16 GiB  | More CPU + higher NIC bandwidth; isolates client/server CPU limits. |
| 3+    | `Standard_D8s_v5` (or larger) | 8 / 32 GiB | High-throughput ceiling; confirms scaling and NIC saturation. |

Pass `--vm-size <sku>` to `deploy.sh` and record it in the run via
`-e vm_size=<sku>` so it lands in every filename and metadata sidecar.

## Default matrix (built into `run-bench.yml`)

The playbook ships a deliberately small default so a first run is quick and
cheap. It covers the baseline vs production QUIC plus a sample of each
experimental backend:

| # | transport  | payload | concurrency | budget       |
|---|------------|---------|-------------|--------------|
| 1 | `tcp-tls`  | 64 B    | 16          | count 20 000 |
| 2 | `quinn`    | 64 B    | 16          | count 20 000 |
| 3 | `quinn`    | 4 KiB   | 32          | count 20 000 |
| 4 | `msquic`   | 64 B    | 16          | count 10 000 |
| 5 | `s2n-quic` | 64 B    | 16          | count 10 000 |
| 6 | `quiche`   | 64 B    | 1 (forced)  | count 5 000  |

## Extended matrix (`scenarios.yml`)

For a fuller sweep, override with the example
[`scenarios.yml`](../../tests/infra/ansible/scenarios.yml):

```bash
ansible-playbook run-bench.yml -e @scenarios.yml \
  -e topology=cross-zone -e vm_size=Standard_D4s_v5 -e region=eastus2
```

It adds a concurrency sweep (1 → 32), a payload sweep (64 B → 64 KiB) on the
production backends, and a `duration`-based steady-state sample. Copy it and
trim/extend to taste — each scenario is just a dict of
`transport / payload_size / concurrency / (count|duration) [/ port]`.

## What a good sweep looks like

To characterize a transport on a given SKU you generally want, per topology:

- a **latency** point: `concurrency 1`, small payload, `count`-based;
- a **throughput** point: high `concurrency`, small payload, `duration`-based;
- a **bulk** point: high `concurrency`, large payload;

…repeated for `tcp-tls` (baseline) and each QUIC backend, then repeated across
SKUs. The [results template](results-template.md) is laid out to receive exactly
this shape of data.
