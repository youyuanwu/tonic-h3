#!/usr/bin/env bash
#
# deploy.sh - Provision one tonic-h3 benchmark topology into Azure.
#
# Creates the resource group (idempotent) then deploys bicep/main.bicep with the
# matching *.bicepparam. The SSH public key and the admin SSH source CIDR are
# injected via environment variables so no secrets live in source control.
#
# Usage:
#   ./deploy.sh <same-zone|cross-zone|cross-region> --admin-cidr <CIDR> [options]
#
# Required:
#   <topology>                same-zone | cross-zone | cross-region  (first arg)
#   -c, --admin-cidr CIDR     Source CIDR allowed to SSH (e.g. 203.0.113.10/32)
#
# Options:
#   -g, --resource-group NAME Resource group name
#                             (default: rg-tonich3-bench-<topology>)
#   -l, --location LOC        Region for the RG + primary node (default: eastus2)
#   -k, --ssh-key PATH        SSH public key file (default: ~/.ssh/id_rsa.pub)
#   -s, --suffix SUFFIX       Override the generated resource-name suffix
#   -n, --no-public-ip        Do not attach management public IPs (Bastion mode)
#       --cloud-init          Enable the optional cloud-init provisioning stub
#       --what-if             Preview changes only (az deployment ... --what-if)
#   -h, --help                Show this help
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BICEP_DIR="${SCRIPT_DIR}/../bicep"

usage() { sed -n '2,32p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

TOPOLOGY="${1:-}"
if [[ -z "${TOPOLOGY}" || "${TOPOLOGY}" == "-h" || "${TOPOLOGY}" == "--help" ]]; then
  usage; exit 0
fi
shift || true

RG=""
LOCATION="eastus2"
SSH_KEY_FILE="${HOME}/.ssh/id_rsa.pub"
ADMIN_CIDR=""
SUFFIX=""
EXTRA_PARAMS=()
WHATIF=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    -g|--resource-group) RG="$2"; shift 2 ;;
    -l|--location)       LOCATION="$2"; shift 2 ;;
    -k|--ssh-key)        SSH_KEY_FILE="$2"; shift 2 ;;
    -c|--admin-cidr)     ADMIN_CIDR="$2"; shift 2 ;;
    -s|--suffix)         SUFFIX="$2"; shift 2 ;;
    -n|--no-public-ip)   EXTRA_PARAMS+=("enablePublicIpForSsh=false"); shift ;;
    --cloud-init)        EXTRA_PARAMS+=("enableCloudInit=true"); shift ;;
    --what-if)           WHATIF="--what-if"; shift ;;
    -h|--help)           usage; exit 0 ;;
    *) echo "ERROR: unknown argument: $1" >&2; usage; exit 1 ;;
  esac
done

case "${TOPOLOGY}" in
  same-zone|cross-zone|cross-region) ;;
  *) echo "ERROR: topology must be same-zone|cross-zone|cross-region (got '${TOPOLOGY}')" >&2; exit 1 ;;
esac

PARAM_FILE="${BICEP_DIR}/params/${TOPOLOGY}.bicepparam"
[[ -f "${PARAM_FILE}" ]] || { echo "ERROR: missing param file ${PARAM_FILE}" >&2; exit 1; }

if [[ -z "${ADMIN_CIDR}" ]]; then
  echo "ERROR: --admin-cidr is required (source CIDR allowed to SSH)." >&2
  exit 1
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
echo "==> ssh key file    : ${SSH_KEY_FILE}"
echo "==> admin ssh CIDR  : ${ADMIN_CIDR}"
[[ -n "${SUFFIX}" ]] && echo "==> suffix override : ${SUFFIX}"
[[ ${#EXTRA_PARAMS[@]} -gt 0 ]] && echo "==> extra params    : ${EXTRA_PARAMS[*]}"
[[ -n "${WHATIF}" ]] && echo "==> mode            : what-if (preview only)"

echo "==> Ensuring resource group exists..."
az group create --name "${RG}" --location "${LOCATION}" --output none

# Assemble parameter overrides (inline values take precedence over the .bicepparam file).
OVERRIDES=("location=${LOCATION}")
[[ -n "${SUFFIX}" ]] && OVERRIDES+=("deploySuffix=${SUFFIX}")
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
