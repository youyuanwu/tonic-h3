#!/usr/bin/env bash
#
# teardown.sh - Delete a tonic-h3 benchmark topology (the whole resource group).
#
# One resource group holds an entire scenario -- including BOTH regions for the
# cross-region topology -- so a single `az group delete` removes everything.
#
# Usage:
#   ./teardown.sh <same-zone|cross-zone|cross-region> [options]
#
# Options:
#   -g, --resource-group NAME Resource group to delete
#                             (default: rg-tonich3-bench-<topology>)
#   -y, --yes                 Skip the interactive confirmation prompt
#       --no-wait             Return immediately; delete continues async
#   -h, --help                Show this help
#
set -euo pipefail

usage() { sed -n '2,18p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

TOPOLOGY="${1:-}"
if [[ -z "${TOPOLOGY}" || "${TOPOLOGY}" == "-h" || "${TOPOLOGY}" == "--help" ]]; then
  usage; exit 0
fi
shift || true

RG=""
ASSUME_YES="false"
NO_WAIT=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    -g|--resource-group) RG="$2"; shift 2 ;;
    -y|--yes)            ASSUME_YES="true"; shift ;;
    --no-wait)           NO_WAIT="--no-wait"; shift ;;
    -h|--help)           usage; exit 0 ;;
    *) echo "ERROR: unknown argument: $1" >&2; usage; exit 1 ;;
  esac
done

case "${TOPOLOGY}" in
  same-zone|cross-zone|cross-region) ;;
  *) echo "ERROR: topology must be same-zone|cross-zone|cross-region (got '${TOPOLOGY}')" >&2; exit 1 ;;
esac

RG="${RG:-rg-tonich3-bench-${TOPOLOGY}}"

if ! az group show --name "${RG}" --output none 2>/dev/null; then
  echo "Resource group '${RG}' does not exist. Nothing to do."
  exit 0
fi

echo "WARNING: about to DELETE resource group '${RG}' and ALL resources in it."
echo "         This is irreversible and will stop any billing for those VMs."

if [[ "${ASSUME_YES}" != "true" ]]; then
  read -r -p "Type the resource group name to confirm: " CONFIRM
  if [[ "${CONFIRM}" != "${RG}" ]]; then
    echo "Confirmation did not match. Aborting."
    exit 1
  fi
fi

echo "==> Deleting resource group '${RG}'..."
# shellcheck disable=SC2086
az group delete --name "${RG}" --yes ${NO_WAIT}
echo "==> Done."
