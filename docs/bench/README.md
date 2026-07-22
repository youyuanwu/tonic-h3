# tonic-h3 Azure benchmark framework

This directory documents **how the `tonic-h3` gRPC-over-HTTP/3 benchmark is run
on Azure VMs and how the results are organized over time**. It is the glue
between three pieces that already live in the repo:

| Piece | Where | Role |
|-------|-------|------|
| Benchmark binaries (`bench-server` / `bench-client`) | [`tests/bench/`](../../tests/bench/README.md) | The actual echo load test across 5 transports. |
| Azure infrastructure (Bicep + scripts) | [`tests/infra/`](../../tests/infra/README.md) | Provisions the client + server VMs and private networking. |
| Ansible orchestration (`deploy-bench.yml`, `run-bench.yml`) | [`tests/infra/ansible/`](../../tests/infra/ansible/) | Deploys the binaries and runs the scenario matrix over the private link. |

## Audience & purpose

You want to measure gRPC throughput/latency for HTTP/3 (QUIC) vs the HTTP/2
(TCP+TLS) baseline on **real Azure networking**, across several transports,
network topologies, and VM sizes — and to **collect those numbers repeatedly**
as the code, the SKUs, or the transports evolve. This framework standardizes:

- **What** to measure (the scenario matrix) — [`scenario-matrix.md`](scenario-matrix.md)
- **How** to run it end to end — [`run-procedure.md`](run-procedure.md)
- **Where** results go and **how** to present them — [`results-template.md`](results-template.md)

## How the pieces fit

```
                 control node (your machine, az + ansible + cargo)
                          │
   1. deploy.sh ──────────┼──────────► Azure: 1 client VM + 1 server VM (private net)
                          │
   2. inventory.py ───────┤            writes inventory.ini (client/server, private IPs, vm_name)
                          │
   3. deploy-bench.yml ───┼──────────► copies release binaries + installs libmsquic on both VMs
                          │
   4. run-bench.yml ──────┼──────────► server: bench-server (private IP)
                          │            client: bench-client --format json  ──► result files on VM
                          │            fetch ◄── results + metadata pulled back to
                          │                       tests/infra/ansible/results/  (git-ignored)
                          │
   5. teardown.sh ────────┴──────────► az group delete  (stop paying)
```

Results accumulate under `tests/infra/ansible/results/` on your control node.
That directory is **git-ignored** (see the repo root `.gitignore`) — raw run
artifacts are never committed. You curate the interesting numbers by hand into
the tables in [`results-template.md`](results-template.md).

## Design principle: built for repeated, cross-SKU runs

Every result file and its metadata sidecar are named with the full scenario
identity — topology, VM size, transport, payload, concurrency, request budget,
a UTC timestamp, and a random run id:

```
result-<topology>-<vmsize>-<transport>-p<payload>-c<concurrency>-<budget>-<utc>-<runid>-s<NNN>.json
result-...-s<NNN>.meta.json      # provenance (incl. libmsquic version, git commit)
result-...-s<NNN>.client.log     # client stderr   (always fetched)
result-...-s<NNN>.server.log     # server stdout   (always fetched)
```

so runs from different days, machines, and SKUs **never collide** (the
zero-padded scenario index `-s<NNN>` also keeps duplicate scenarios within one
invocation distinct) and can be aggregated later. The client emits **JSON** (`--format json`) precisely so this
aggregation stays robust as the dataset grows. See
[`scenario-matrix.md`](scenario-matrix.md#vm-sku-sweep) for the intended SKU
progression.

## Quick links

- [Scenario matrix & rationale](scenario-matrix.md)
- [End-to-end run procedure](run-procedure.md)
- [Result showcase template](results-template.md)
- [Benchmark binaries reference](../../tests/bench/README.md)
- [Azure infrastructure reference](../../tests/infra/README.md)
