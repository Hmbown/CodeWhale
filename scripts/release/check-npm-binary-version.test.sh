#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
check="${repo_root}/scripts/release/check-npm-binary-version.sh"

"${check}" 0.9.11 0.9.11

if "${check}" 0.9.11 0.9.10 >/dev/null 2>&1; then
  echo "expected an unapproved npm binary mismatch to fail" >&2
  exit 1
fi

CODEWHALE_ALLOW_NPM_BINARY_MISMATCH=1 \
  "${check}" 0.9.11 0.9.10 >/dev/null

echo "npm binary-version mismatch gate tests passed"
