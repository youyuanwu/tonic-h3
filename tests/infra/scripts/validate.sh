#!/usr/bin/env bash
#
# validate.sh - Compile every Bicep template and parameter file.
#
# No Azure login or subscription is required; this only runs the Bicep compiler
# (`az bicep build` / `az bicep build-params`). Generated ARM JSON is written to
# a temp dir and discarded so nothing is committed.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BICEP_DIR="${SCRIPT_DIR}/../bicep"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT

fail=0

echo "==> Building Bicep templates..."
while IFS= read -r -d '' f; do
  rel="${f#"${BICEP_DIR}/"}"
  if az bicep build --file "$f" --outfile "${TMP_DIR}/$(basename "$f").json" 2>"${TMP_DIR}/err"; then
    echo "  OK    ${rel}"
  else
    echo "  FAIL  ${rel}"
    cat "${TMP_DIR}/err"
    fail=1
  fi
done < <(find "${BICEP_DIR}" -name '*.bicep' -print0 | sort -z)

echo "==> Building Bicep parameter files..."
while IFS= read -r -d '' f; do
  rel="${f#"${BICEP_DIR}/"}"
  if az bicep build-params --file "$f" --outfile "${TMP_DIR}/$(basename "$f").json" 2>"${TMP_DIR}/err"; then
    echo "  OK    ${rel}"
  else
    echo "  FAIL  ${rel}"
    cat "${TMP_DIR}/err"
    fail=1
  fi
done < <(find "${BICEP_DIR}" -name '*.bicepparam' -print0 | sort -z)

if [[ "${fail}" -ne 0 ]]; then
  echo "==> Validation FAILED."
  exit 1
fi
echo "==> All Bicep templates and parameter files compiled cleanly."
