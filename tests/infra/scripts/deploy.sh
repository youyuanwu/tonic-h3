#!/usr/bin/env bash
#
# deploy.sh - Provision one tonic-h3 benchmark topology into Azure.
#
# Creates the resource group (idempotent) then deploys bicep/main.bicep with the
# matching *.bicepparam. The SSH public key and the admin SSH source CIDR are
# injected via environment variables so no secrets live in source control.
#
# Usage:
#   ./deploy.sh <same-zone|cross-zone|cross-region> [--admin-cidr <CIDR>] [options]
#
# Required:
#   <topology>                same-zone | cross-zone | cross-region  (first arg)
#
# Options:
#   -c, --admin-cidr CIDR     Source CIDR allowed to SSH (e.g. 203.0.113.10/32).
#                             If omitted, auto-detects this machine's public egress
#                             IP as <ip>/32 (needs outbound internet).
#   -g, --resource-group NAME Resource group name
#                             (default: rg-tonich3-bench-<topology>)
#   -l, --location LOC        Region for the RG + primary node (default: eastus2)
#   -L, --secondary-location LOC
#                             Second region, cross-region topology ONLY
#                             (default: from param file, westus2)
#   -k, --ssh-key PATH        SSH public key file (default: ~/.ssh/id_ed25519.pub)
#   -m, --vm-size SIZE        VM size (default: from param file, Standard_D2s_v5;
#                             must support Accelerated Networking + Premium storage)
#   -s, --suffix SUFFIX       Optional resource-name suffix (default: none, i.e.
#                             clean names like tonich3-client)
#       --image-publisher PUB Marketplace image publisher (default: Canonical)
#       --image-offer OFFER   Marketplace image offer (default: ubuntu-26_04-lts,
#                             chosen to match the build host's glibc)
#       --image-sku SKU       Marketplace image SKU (default: server, Gen2 x64)
#   -n, --no-public-ip        Do not attach management public IPs (Bastion mode)
#       --what-if             Preview changes only (az deployment ... --what-if)
#   -h, --help                Show this help
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BICEP_DIR="${SCRIPT_DIR}/../bicep"

# Print the leading comment block (from line 2 up to the first non-comment line).
usage() { awk 'NR>=2 && /^#/ {sub(/^# ?/, ""); print; next} NR>=2 {exit}' "${BASH_SOURCE[0]}"; }

TOPOLOGY="${1:-}"
if [[ -z "${TOPOLOGY}" || "${TOPOLOGY}" == "-h" || "${TOPOLOGY}" == "--help" ]]; then
  usage; exit 0
fi
shift || true

RG=""
LOCATION="eastus2"
SECONDARY_LOCATION=""
SSH_KEY_FILE="${HOME}/.ssh/id_ed25519.pub"
VM_SIZE=""
ADMIN_CIDR=""
SUFFIX=""
IMAGE_PUBLISHER=""
IMAGE_OFFER=""
IMAGE_SKU=""
EXTRA_PARAMS=()
WHATIF=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    -g|--resource-group) RG="$2"; shift 2 ;;
    -l|--location)       LOCATION="$2"; shift 2 ;;
    -L|--secondary-location) SECONDARY_LOCATION="$2"; shift 2 ;;
    -k|--ssh-key)        SSH_KEY_FILE="$2"; shift 2 ;;
    -m|--vm-size)        VM_SIZE="$2"; shift 2 ;;
    -c|--admin-cidr)     ADMIN_CIDR="$2"; shift 2 ;;
    -s|--suffix)         SUFFIX="$2"; shift 2 ;;
    --image-publisher)   IMAGE_PUBLISHER="$2"; shift 2 ;;
    --image-offer)       IMAGE_OFFER="$2"; shift 2 ;;
    --image-sku)         IMAGE_SKU="$2"; shift 2 ;;
    -n|--no-public-ip)   EXTRA_PARAMS+=("enablePublicIpForSsh=false"); shift ;;
    --what-if)           WHATIF="--what-if"; shift ;;
    -h|--help)           usage; exit 0 ;;
    *) echo "ERROR: unknown argument: $1" >&2; usage; exit 1 ;;
  esac
done

case "${TOPOLOGY}" in
  same-zone|cross-zone|cross-region) ;;
  *) echo "ERROR: topology must be same-zone|cross-zone|cross-region (got '${TOPOLOGY}')" >&2; exit 1 ;;
esac

if [[ -n "${SECONDARY_LOCATION}" && "${TOPOLOGY}" != "cross-region" ]]; then
  echo "WARNING: --secondary-location is only used by the cross-region topology; ignoring it for '${TOPOLOGY}'." >&2
  SECONDARY_LOCATION=""
fi

PARAM_FILE="${BICEP_DIR}/params/${TOPOLOGY}.bicepparam"
[[ -f "${PARAM_FILE}" ]] || { echo "ERROR: missing param file ${PARAM_FILE}" >&2; exit 1; }

if [[ -z "${ADMIN_CIDR}" ]]; then
  echo "==> --admin-cidr not given; detecting public egress IP..."
  DETECTED_IP="$(curl -fsS --max-time 10 https://api.ipify.org 2>/dev/null \
    || curl -fsS --max-time 10 https://ifconfig.me 2>/dev/null || true)"
  # Accept only a bare IPv4 address from the echo service.
  if [[ "${DETECTED_IP}" =~ ^[0-9]{1,3}(\.[0-9]{1,3}){3}$ ]]; then
    ADMIN_CIDR="${DETECTED_IP}/32"
    echo "    detected admin CIDR: ${ADMIN_CIDR}"
  else
    echo "ERROR: could not auto-detect a public IP for --admin-cidr." >&2
    echo "       Pass it explicitly, e.g. --admin-cidr 203.0.113.10/32." >&2
    exit 1
  fi
fi
if [[ ! -f "${SSH_KEY_FILE}" ]]; then
  echo "ERROR: SSH public key file not found: ${SSH_KEY_FILE}" >&2
  echo "       Generate one with: ssh-keygen -t ed25519 -f ~/.ssh/id_ed25519" >&2
  exit 1
fi

RG="${RG:-rg-tonich3-bench-${TOPOLOGY}}"

# Inject secrets via env (read by the .bicepparam via readEnvironmentVariable).
export TONICH3_SSH_PUBKEY
TONICH3_SSH_PUBKEY="$(cat "${SSH_KEY_FILE}")"
export TONICH3_ADMIN_CIDR="${ADMIN_CIDR}"

DEPLOY_NAME="tonich3-bench-${TOPOLOGY}-$(date +%Y%m%d%H%M%S)"

echo "==> topology        : ${TOPOLOGY}"
echo "==> resource group  : ${RG}"
echo "==> location        : ${LOCATION}"
[[ -n "${SECONDARY_LOCATION}" ]] && echo "==> secondary loc   : ${SECONDARY_LOCATION}"
echo "==> ssh key file    : ${SSH_KEY_FILE}"
[[ -n "${VM_SIZE}" ]] && echo "==> vm size         : ${VM_SIZE}"
echo "==> admin ssh CIDR  : ${ADMIN_CIDR}"
[[ -n "${SUFFIX}" ]] && echo "==> suffix override : ${SUFFIX}"
[[ -n "${IMAGE_OFFER}" || -n "${IMAGE_SKU}" || -n "${IMAGE_PUBLISHER}" ]] && \
  echo "==> image override  : ${IMAGE_PUBLISHER:-Canonical}:${IMAGE_OFFER:-ubuntu-26_04-lts}:${IMAGE_SKU:-server}"
[[ ${#EXTRA_PARAMS[@]} -gt 0 ]] && echo "==> extra params    : ${EXTRA_PARAMS[*]}"
[[ -n "${WHATIF}" ]] && echo "==> mode            : what-if (preview only)"

echo "==> Ensuring resource group exists..."
az group create --name "${RG}" --location "${LOCATION}" --output none

# Assemble parameter overrides (inline values take precedence over the .bicepparam file).
OVERRIDES=("location=${LOCATION}")
[[ -n "${SECONDARY_LOCATION}" ]] && OVERRIDES+=("secondaryLocation=${SECONDARY_LOCATION}")
[[ -n "${VM_SIZE}" ]] && OVERRIDES+=("vmSize=${VM_SIZE}")
[[ -n "${SUFFIX}" ]] && OVERRIDES+=("nameSuffix=${SUFFIX}")
[[ -n "${IMAGE_PUBLISHER}" ]] && OVERRIDES+=("imagePublisher=${IMAGE_PUBLISHER}")
[[ -n "${IMAGE_OFFER}" ]] && OVERRIDES+=("imageOffer=${IMAGE_OFFER}")
[[ -n "${IMAGE_SKU}" ]] && OVERRIDES+=("imageSku=${IMAGE_SKU}")
if [[ ${#EXTRA_PARAMS[@]} -gt 0 ]]; then
  OVERRIDES+=("${EXTRA_PARAMS[@]}")
fi

echo "==> Deploying (${DEPLOY_NAME})..."
# shellcheck disable=SC2068
az deployment group create \
  --name "${DEPLOY_NAME}" \
  --resource-group "${RG}" \
  --template-file "${BICEP_DIR}/main.bicep" \
  --parameters "${PARAM_FILE}" \
  --parameters ${OVERRIDES[@]} \
  ${WHATIF} \
  --output json

if [[ -z "${WHATIF}" ]]; then
  echo "==> Deployment outputs:"
  az deployment group show \
    --name "${DEPLOY_NAME}" \
    --resource-group "${RG}" \
    --query properties.outputs \
    --output json
  echo ""
  echo "Client/server PRIVATE IPs above are what the benchmark harness targets."
  echo "Tear down with: ${SCRIPT_DIR}/teardown.sh ${TOPOLOGY} --resource-group ${RG}"
fi
