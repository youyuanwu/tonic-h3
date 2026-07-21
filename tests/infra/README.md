# tonic-h3 benchmark infrastructure (Azure Bicep)

Infrastructure-as-code for the **tonic-h3** networking benchmark harness. It
provisions the Azure VMs and networking on which a **client** node drives gRPC
load against a **server** node, so the maintainers can compare:

- **gRPC over HTTP/3 (QUIC / UDP)** on each backend — `quinn` (production),
  and the experimental `msquic`, `s2n-quic`, `quiche`, and
- **gRPC over HTTP/2 (TCP + TLS)** as the `tonic` baseline.

> This folder is **only the infrastructure**. It does not build or run the
> benchmark application — it provisions VMs + private networking and emits the
> nodes' **private IPs** for the harness to target. The repo's Rust integration
> tests live in [`tonic-h3-tests/`](../../tonic-h3-tests/); this Azure infra is
> deliberately kept separate under `tests/infra/`.

## What it deploys

For each run you pick one of three **network topologies**. Every topology
provisions exactly one **client** VM and one **server** VM (Ubuntu 22.04 LTS,
Accelerated Networking on), plus the VNet(s), subnet(s) and NSG(s) to connect
them **privately**:

| Topology       | Placement                                                             | Latency profile        | Private-path mechanism                                   |
|----------------|----------------------------------------------------------------------|------------------------|----------------------------------------------------------|
| `same-zone`    | Both VMs in the **same** zone, same VNet/subnet                       | Lowest                 | Same subnet + **Proximity Placement Group** co-location  |
| `cross-zone`   | VMs in **different** zones (e.g. 1 and 2), same region, same VNet     | Intra-region, inter-AZ | Same VNet, private IPs                                    |
| `cross-region` | VMs in **two regions**, one VNet each, joined by **global peering**   | Inter-region backbone  | Global VNet peering (Azure backbone, private IPs)        |

### Traffic stays private — by design

All benchmark data-plane traffic traverses **Azure internal networking (private
IPs / the Azure backbone) and never the public internet**:

- **Private IPs only.** The templates output each node's **private** IP; the
  harness targets those. Nodes talk to each other over the VNet, never via a
  public address.
- **NSG scoping.** Benchmark ports are opened for **both TCP and UDP** (for the
  HTTP/2 vs HTTP/3 comparison) but **only** from the `VirtualNetwork` service
  tag. That tag resolves to the local VNet address space **plus any peered
  VNets**, so the same rules keep cross-region traffic private without
  hard-coding remote prefixes. An explicit rule additionally **denies** the
  benchmark ports from the `Internet` tag, on top of Azure's default
  `DenyAllInBound`.
- **Global VNet peering (cross-region).** For `cross-region`, the two VNets are
  connected with **global peering** (`allowVirtualNetworkAccess: true`), which
  keeps inter-region traffic on the Azure backbone and, via the peering, folds
  the remote address space into each side's `VirtualNetwork` tag.
- **Accelerated Networking** is enabled on every NIC — essential for meaningful
  network benchmarks. The default `Standard_D2s_v5` SKU (2 vCPU / 8 GiB) supports
  Accelerated Networking + Premium storage; it is the smallest size compatible
  with this template. For higher-throughput runs where the small NIC could become
  the bottleneck, bump to `Standard_D4s_v5` (or larger) via `--vm-size`.
- **SSH is management-only.** Port 22 is allowed **only** from a
  parameterised admin CIDR. The benchmark ports are never exposed publicly.

## Design choices (and why)

- **Resource-group-scoped `main.bicep` + a wrapper script that creates the RG
  first.** `scripts/deploy.sh` runs `az group create` and then
  `az deployment group create`. RG-scoped keeps module wiring and outputs
  simple (single scope, no cross-scope `resourceId` juggling), and one RG holds
  an entire scenario — **including both regions for `cross-region`** — so
  `az group delete` tears everything down in one command.
- **Single `main.bicep`, `topology` parameter.** One template expresses all
  three topologies (`same-zone` | `cross-zone` | `cross-region`) via a
  `@allowed` parameter, rather than three near-duplicate entrypoints. Modules
  are conditionally deployed (secondary VNet + peering only for `cross-region`;
  PPG only for `same-zone`). Conditional wiring uses deterministic
  `resourceId()` strings plus `dependsOn` so there are **no possible-null
  (BCP318) warnings** and ordering is still guaranteed.
- **Secrets never in source.** The SSH **public** key and the admin CIDR are
  supplied at deploy time. The `.bicepparam` files read them from environment
  variables (`readEnvironmentVariable`) with inert placeholder defaults
  (`0.0.0.0/32` grants no access), so nothing sensitive is committed and the
  files still validate offline. Password auth is disabled (SSH key only).

## Layout

```
tests/infra/
├── README.md
├── bicep/
│   ├── main.bicep                 # topology dispatcher (RG-scoped)
│   ├── modules/
│   │   ├── network.bicep           # VNet + subnet + NSG (TCP+UDP bench rules)
│   │   ├── vm.bicep                # NIC (accel-net) + optional PIP + VM
│   │   └── peering.bicep           # bidirectional global VNet peering
│   └── params/
│       ├── same-zone.bicepparam
│       ├── cross-zone.bicepparam
│       └── cross-region.bicepparam
└── scripts/
    ├── deploy.sh                    # az group create + az deployment ... create
    ├── teardown.sh                  # az group delete (confirmation guarded)
    └── validate.sh                  # az bicep build for every template
└── ansible/
    ├── inventory.py                 # query az for VM private/public IPs -> inventory.ini
    ├── ping.yml                     # peer connectivity test (ICMP over private IPs)
    ├── ansible.cfg                  # local defaults (inventory, host-key checking off)
    └── inventory.ini               # GENERATED per deploy (git-ignored)
```

## Prerequisites

- **Azure CLI** `>= 2.54` (this repo was validated with 2.86) with the **Bicep**
  CLI (`az bicep version`; validated with 0.43.8).
- An **authenticated** subscription: `az login` and `az account set
  --subscription <id>`.
- An **SSH key pair**. Create one if needed:
  `ssh-keygen -t ed25519 -f ~/.ssh/id_ed25519`.
- Chosen regions must support **Availability Zones** and, for `cross-region`,
  **global VNet peering**. Defaults are `eastus2` (primary) and `westus2`
  (secondary), both zone-enabled backbone regions.

## Validate (no Azure needed)

Compile every template and parameter file — no login required:

```bash
tests/infra/scripts/validate.sh
```

This runs `az bicep build` / `az bicep build-params` into a temp dir and
discards the generated ARM JSON. Compiled `*.json` is also git-ignored.

## Deploy

```bash
# same-zone (PPG co-located, lowest latency).
# --admin-cidr is optional: omit it to auto-detect this machine's public IP as /32.
tests/infra/scripts/deploy.sh same-zone

# Pin the SSH source explicitly (e.g. a corporate range):
tests/infra/scripts/deploy.sh same-zone   --admin-cidr 203.0.113.10/32

# cross-zone (zones 1 and 2, same region)
tests/infra/scripts/deploy.sh cross-zone  --admin-cidr 203.0.113.10/32

# cross-region (eastus2 + westus2, global peering)
tests/infra/scripts/deploy.sh cross-region --admin-cidr 203.0.113.10/32

# cross-region with BOTH regions chosen explicitly
tests/infra/scripts/deploy.sh cross-region --admin-cidr 203.0.113.10/32 \
  --location eastus2 --secondary-location westus3
```

Common options (see `deploy.sh --help`):

| Option | Meaning | Default |
|--------|---------|---------|
| `-c, --admin-cidr CIDR` | Source CIDR allowed to SSH | auto-detected `<egress-ip>/32` |
| `-g, --resource-group`  | RG name | `rg-tonich3-bench-<topology>` |
| `-l, --location`        | Primary region | `eastus2` |
| `-L, --secondary-location` | Second region (**`cross-region` only**) | `westus2` (from param file) |
| `-k, --ssh-key PATH`    | SSH **public** key file | `~/.ssh/id_ed25519.pub` |
| `-m, --vm-size SIZE`    | VM size (must support Accel-Net + Premium storage) | `Standard_D2s_v5` |
| `-s, --suffix`          | Optional resource-name suffix | none (clean names) |
| `-n, --no-public-ip`    | No management PIPs — use **Azure Bastion** | PIPs on |
| `--what-if`             | Preview only | — |

The script injects the key/CIDR via `TONICH3_SSH_PUBKEY` / `TONICH3_ADMIN_CIDR`
so no secrets are passed on the command line or stored in files.

### Azure Bastion alternative (fully private management)

For a posture with **no public IPs at all**, deploy with `--no-public-ip` and
reach the nodes through **Azure Bastion**: add a Bastion host + `AzureBastion
Subnet` to the VNet (out of scope for these templates) and connect via the
portal or `az network bastion ssh`. The NSG SSH rule still applies; Bastion
originates from within the VNet so it is covered by the `VirtualNetwork` tag.

## Reading the outputs

On success `deploy.sh` prints the deployment outputs, or fetch them later:

```bash
az deployment group show \
  --resource-group rg-tonich3-bench-same-zone \
  --name <deployment-name> \
  --query properties.outputs
```

Key outputs consumed by the harness:

| Output | Meaning |
|--------|---------|
| `clientPrivateIp` / `serverPrivateIp` | **Private** IPs the harness targets |
| `clientLocation` / `serverLocation`   | Node regions |
| `clientZone` / `serverZone`           | Node availability zones |
| `clientSshPublicIp` / `serverSshPublicIp` | Management IPs (empty in Bastion mode) |
| `clientSshFqdn` / `serverSshFqdn`     | Management FQDNs (empty in Bastion mode) |
| `benchPorts`                          | Port range(s) opened for TCP + UDP |

SSH in for management (public-IP mode):
`ssh azureuser@<serverSshPublicIp>`.

## Peer connectivity test (Ansible)

After a deployment, verify the two nodes can reach each other over their
**private** IPs (proving the same-subnet / cross-zone / global-peering path works
without touching the public internet).

Prerequisites: `ansible-core` on your control machine
(`pipx install ansible-core` or `pip install --user ansible-core`).

```bash
cd tests/infra/ansible

# 1. Query Azure for the VM IPs and write inventory.ini
python3 inventory.py --topology cross-region        # or --resource-group <rg>

# 2. ICMP-ping each node's peer over the private link
ansible-playbook ping.yml                            # ansible.cfg -> ./inventory.ini
```

`inventory.py` builds a `bench` group with two hosts (`client`, `server`); each
carries `ansible_host` (public IP for SSH), `private_ip`, and `peer`. `ping.yml`
resolves `hostvars[peer].private_ip` and runs `ping` from each node to the other,
failing the play if the private path is unreachable. Azure's default
`AllowVnetInBound` rule permits intra-VNet/peered ICMP, so success confirms
end-to-end private connectivity.

Options: `inventory.py -u <user>` (SSH user, default `azureuser`), `-k <path>`
(private key, default `~/.ssh/id_ed25519`), `--connect-via private` (when the
control node runs inside the VNet, e.g. via Bastion — required with
`--no-public-ip` deployments). The generated `inventory.ini` is git-ignored
(per-deploy IPs); regenerate it after every deploy.

## Teardown & cost warning

> **Cost warning:** these VMs (default `Standard_D2s_v5`) bill while running.
> Tear the scenario down as soon as a benchmark run completes.

```bash
tests/infra/scripts/teardown.sh same-zone            # prompts for confirmation
tests/infra/scripts/teardown.sh cross-region --yes   # non-interactive
```

Teardown deletes the **entire resource group** (both regions for
`cross-region`), so every resource is removed in one step.
