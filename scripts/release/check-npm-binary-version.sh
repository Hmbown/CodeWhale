#!/usr/bin/env bash
set -euo pipefail

workspace_version="${1:-}"
npm_binary_version="${2:-}"

if [[ -z "${workspace_version}" ]]; then
  echo "::error::workspace version is required for the npm binary-pin check." >&2
  exit 2
fi

if [[ "${workspace_version}" == "${npm_binary_version}" ]]; then
  exit 0
fi

if [[ "${CODEWHALE_ALLOW_NPM_BINARY_MISMATCH:-0}" == "1" ]]; then
  echo "Packaging-only npm release: workspace=${workspace_version}, binary=${npm_binary_version:-<missing>} (explicit mismatch override)."
  exit 0
fi

echo "::error::npm/codewhale/package.json codewhaleBinaryVersion (${npm_binary_version:-<missing>}) does not match workspace Cargo.toml (${workspace_version})." >&2
echo "Set CODEWHALE_ALLOW_NPM_BINARY_MISMATCH=1 only for an intentional packaging-only npm release." >&2
exit 1
