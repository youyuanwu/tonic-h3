# End-to-end run procedure

This is the operational runbook: from nothing, to numbers in
`tests/infra/ansible/results/`, to a torn-down (non-billing) Azure environment.
It ties together the [infrastructure](../../tests/infra/README.md), the
[benchmark binaries](../../tests/bench/README.md), and the
[scenario matrix](scenario-matrix.md).

> Run all `ansible-playbook` / `inventory.py` commands **from
> `tests/infra/ansible/`** so the local [`ansible.cfg`](../../tests/infra/ansible/ansible.cfg)
> (inventory path, host-key checking off, pipelining) is active.

## Prerequisites (control node)

- **Azure CLI** (`az`), logged in to the target subscription.
- **`ansible-core`** on your PATH (`pipx install ansible-core`, or
  `pip install --user ansible-core` then `export PATH="$HOME/.local/bin:$PATH"`).
  Only the built-in modules are required — no extra collections.
- **Rust toolchain** matching [`rust-toolchain.toml`](../../rust-toolchain.toml)
  (for building the release binaries locally) and **`protoc`**.
- Native **`libmsquic`** installed locally *only if you also run the binaries
  locally*; the VMs get it via `deploy-bench.yml`.
- An SSH key whose **public** half is authorized on the VMs (see
  `deploy.sh -k`); the **private** half is referenced by `inventory.py -k`
  (default `~/.ssh/id_ed25519`).

## Step 0 — build the release binaries locally

```bash
# from the repo root
cargo build --release -p tonic-h3-bench
# produces target/release/bench-server and target/release/bench-client
```

> ### libc-compatibility constraint (read this)
> `deploy-bench.yml`'s primary strategy is to **copy the locally built binaries**
> onto the VMs. Those binaries are dynamically linked against the control node's
> **glibc**, so the control node's glibc must be **≤** the VMs'. The Bicep default
> image is now **Ubuntu 26.04 LTS** (`imageOffer=ubuntu-26_04-lts`), chosen to
> match the maintainers' build host so prebuilt binaries load without a
> `GLIBC_x.yz not found` error. If **your** build host is newer than the VM image,
> either override the image to match your build host
> (`deploy.sh ... --image-offer <offer> --image-sku <sku>`, or pass the
> `imageOffer`/`imageSku` Bicep params directly), build in a container matching
> the VM's release, or use the **build-on-VM fallback** in
> [Step 3b](#step-3b--build-on-vm-fallback-glibc-mismatch) *instead of* Step 3.

## Step 1 — provision the VMs

```bash
# pick a topology: same-zone | cross-zone | cross-region
tests/infra/scripts/deploy.sh same-zone \
  --admin-cidr "$(curl -s ifconfig.me)/32" \
  --vm-size Standard_D2s_v5 \
  -k ~/.ssh/id_ed25519.pub
```

Creates resource group `rg-tonich3-bench-<topology>` with one **client** and one
**server** VM on private networking. See the
[infra README](../../tests/infra/README.md#deploy) for all flags (secondary
location for `cross-region`, custom location, etc.).

## Step 2 — generate the inventory

```bash
cd tests/infra/ansible
python3 inventory.py --topology same-zone      # or --resource-group <rg>
```

Writes `inventory.ini`: a `bench` group with hosts `client` and `server`, each
carrying `ansible_host` (public IP for SSH), `private_ip`, `peer`, and
**`vm_name`** (the real Azure VM name, recorded in run metadata). Regenerate it
after every deploy (IPs are per-deploy; the file is git-ignored).

Optionally confirm the private path first with the existing
[`ping.yml`](../../tests/infra/ansible/ping.yml):

```bash
ansible-playbook ping.yml
```

## Step 3 — deploy the binaries + runtime

```bash
ansible-playbook deploy-bench.yml
```

This ([`deploy-bench.yml`](../../tests/infra/ansible/deploy-bench.yml)):

1. asserts `target/release/bench-server|bench-client` exist on the control node;
2. installs the **packages.microsoft.com** apt repo and **`libmsquic`**
   (version-pinned to `2.5.8`, matching the build host and the Ubuntu 26.04
   pool — override with `-e bench_libmsquic_version=...`, or `""` for latest) on
   both VMs;
3. copies both binaries to `~/bench/` (`mode 0755`);
4. runs `bench-server --help` / `bench-client --help` on each VM as a smoke test
   that also proves `libmsquic.so.2` loads.

Useful overrides: `-e bench_bin_dir=/path/to/target/release`,
`-e bench_dest_dir=/usr/local/bin` (add `-e ansible_become=true` for system
dirs). If you pin a non-default `-e bench_libmsquic_version=...` here, pass the
**same** value to `run-bench.yml` in Step 4 so it is recorded in each result's
metadata sidecar.

### Step 3b — build-on-VM fallback (glibc mismatch)

Use this **instead of Step 3** only when the
[libc-compatibility constraint](#step-0--build-the-release-binaries-locally)
prevents copying control-node binaries (control-node glibc newer than the VMs').
It builds the binaries **on each VM**, so ABI compatibility is guaranteed. Run
from `tests/infra/ansible/` (inventory already generated in Step 2):

```bash
# 1. Install libmsquic runtime + build toolchain deps on both VMs
ansible bench -b -m apt \
  -a "name=libmsquic,libmsquic-dev,protobuf-compiler,build-essential,pkg-config,git update_cache=true"

# 2. Install rustup + the pinned toolchain on both VMs (rust-toolchain.toml is honored)
ansible bench -m shell \
  -a "command=command -v cargo || (curl -sSf https://sh.rustup.rs | sh -s -- -y)"

# 3. Clone the repo at the exact revision you are benchmarking on both VMs
ansible bench -m git \
  -a "repo=https://github.com/youyuanwu/tonic-h3 dest=~/tonic-h3 version=$(git rev-parse HEAD)"

# 4. Build the release binaries on each VM
ansible bench -m shell \
  -a "chdir=~/tonic-h3 cmd=~/.cargo/bin/cargo build --release -p tonic-h3-bench"
```

Then in Step 4 point `bench_dest_dir` at the VM-built binaries so no copy is
attempted: `-e bench_dest_dir=~/tonic-h3/target/release`. (The `libmsquic`
runtime was installed in sub-step 1, so the copy-based `deploy-bench.yml` is not
needed on this path.)

## Step 4 — run the benchmark matrix

```bash
ansible-playbook run-bench.yml \
  -e topology=same-zone \
  -e vm_size=Standard_D2s_v5 \
  -e region=eastus2 \
  -e bench_libmsquic_version=2.5.8
# override the scenario list from a file:
ansible-playbook run-bench.yml -e @scenarios.yml -e topology=same-zone -e vm_size=Standard_D2s_v5 -e region=eastus2
# build-on-VM fallback path (Step 3b): also pass the VM-built binary dir
ansible-playbook run-bench.yml -e topology=same-zone -e vm_size=Standard_D2s_v5 -e region=eastus2 -e bench_dest_dir=~/tonic-h3/target/release
```

Pass `bench_libmsquic_version` matching what Step 3 installed so it is captured
in each metadata sidecar. Malformed scenarios (missing transport, or not exactly
one of `count`/`duration`) are rejected by a preflight `assert` before any
server starts. If any scenario fails, the matrix still completes and cleans up,
then the play exits non-zero with a summary (opt out with
`-e bench_fail_on_scenario_error=false`).

For each scenario ([`run-bench.yml`](../../tests/infra/ansible/run-bench.yml) →
[`tasks/run-scenario.yml`](../../tests/infra/ansible/tasks/run-scenario.yml)),
with a `block/rescue/always` lifecycle:

1. **start** `bench-server` on the server host, bound to its private IP,
   backgrounded (`nohup` + pidfile), logging to a per-scenario file;
2. **wait** until the server logs `listening on` — a transport-agnostic
   readiness signal that works for both the TCP baseline and the UDP/QUIC
   backends (a TCP port probe would not);
3. **run** `bench-client --format json` on the client host against the server's
   private IP, capturing the JSON result and a metadata sidecar;
4. **always fetch** the result + metadata back to
   `tests/infra/ansible/results/`, and **always stop** the server with `SIGTERM`
   (graceful QUIC teardown frees the port), verifying the process exited before
   the next scenario.

The `topology`, `vm_size`, and `region` you pass are recorded in every filename
and metadata sidecar (defaulting to `unknown` if omitted), so cross-SKU runs stay
distinguishable. See the [scenario matrix](scenario-matrix.md) for the axes.

### Where results land

Each scenario produces four files, all suffixed with a per-run `<utc>-<runid>`
and a zero-padded scenario index `-sNNN` so nothing collides within or across
runs:

```
tests/infra/ansible/results/
├── result-same-zone-Standard_D2s_v5-quinn-p64-c16-n20000-<utc>-<runid>-s000.json        # client result (JSON)
├── result-same-zone-Standard_D2s_v5-quinn-p64-c16-n20000-<utc>-<runid>-s000.meta.json   # provenance sidecar
├── result-same-zone-Standard_D2s_v5-quinn-p64-c16-n20000-<utc>-<runid>-s000.client.log  # client stderr
├── result-same-zone-Standard_D2s_v5-quinn-p64-c16-n20000-<utc>-<runid>-s000.server.log  # server stdout/stderr
└── ...
```

The `.json` is the machine-readable client result (schema in the
[bench README](../../tests/bench/README.md#machine-readable-output---format-json));
the `.meta.json` records provenance (scenario index, topology, VM size, region,
transport, payload, concurrency, budget, libmsquic version, git commit,
timestamps, both VM names/private IPs). The `.client.log` / `.server.log` are the
raw stderr/stdout — always fetched, so even a **failed** scenario leaves a
durable, self-explanatory record. This directory is **git-ignored** — curate the
numbers you care about into [`results-template.md`](results-template.md).

## Step 5 — tear down (stop paying)

Steps 2–4 leave you in `tests/infra/ansible/`. Return to the repo root first (or
use the relative path shown), so the script resolves correctly:

```bash
cd -                                              # back to the repo root
./tests/infra/scripts/teardown.sh same-zone       # prompts for confirmation
./tests/infra/scripts/teardown.sh same-zone --yes # non-interactive
# …or, without leaving tests/infra/ansible/:
../scripts/teardown.sh same-zone
```

> **Cost warning:** the VMs bill while running. Tear the resource group down as
> soon as your runs complete.

## Validating the orchestration without Azure

You can exercise the full **start → wait → client → capture → fetch → stop** flow
on your control node's loopback, using the already-built local binaries and the
loopback inventory
([`inventory.local.ini`](../../tests/infra/ansible/inventory.local.ini)) — no
Azure, no cost:

```bash
cd tests/infra/ansible

# syntax-check every playbook
ansible-playbook --syntax-check deploy-bench.yml run-bench.yml ping.yml

# loopback dry-run of run-bench.yml (both roles -> 127.0.0.1)
REL="$(cd ../../../target/release && pwd)"
ansible-playbook -i inventory.local.ini run-bench.yml \
  -e bench_dest_dir="$REL" \
  -e bench_results_dir_remote=/tmp/bench-dryrun \
  -e topology=loopback -e vm_size=local -e region=local \
  -e '{"scenarios":[{"transport":"quinn","payload_size":64,"concurrency":1,"count":20,"port":5051},{"transport":"tcp-tls","payload_size":64,"concurrency":1,"count":20,"port":5052}]}'

ls results/     # fetched result + metadata JSON should appear here
```

This proves the lifecycle end-to-end (server starts, client connects over
loopback, result/metadata files are written and fetched, server is stopped and
the port freed) before you spend anything on Azure. `ansible-lint` / `yamllint`
are optional; if installed, run them against the playbooks as an extra check.
