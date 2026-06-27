#!/usr/bin/env bash
# Guard the OpenHarmony target dependency graph.
#
# This check intentionally does not require an OpenHarmony SDK or sysroot. It
# only asks Cargo to resolve the codewhale-tui dependency graph for the OHOS
# target and fails if crates known to break or be unsupported on OHOS re-enter
# that graph.
set -euo pipefail

cd "$(dirname "$0")/../.."

target="${1:-aarch64-unknown-linux-ohos}"
package="${CODEWHALE_OHOS_DEP_PACKAGE:-codewhale-tui}"

cargo_tree_with_retry() {
  local attempt
  local max_attempts="${CODEWHALE_OHOS_DEP_RETRIES:-3}"
  local delay_seconds="${CODEWHALE_OHOS_DEP_RETRY_DELAY_SECONDS:-10}"
  local err_file
  local output
  local status

  if ! [[ "${max_attempts}" =~ ^[0-9]+$ ]] || ((max_attempts < 1)); then
    echo "CODEWHALE_OHOS_DEP_RETRIES must be an integer greater than or equal to 1." >&2
    return 1
  fi

  err_file="$(mktemp)"
  for ((attempt = 1; attempt <= max_attempts; attempt++)); do
    if output="$(
      cargo tree \
        --locked \
        --package "${package}" \
        --all-features \
        --target "${target}" \
        --prefix none \
        --no-dedupe \
        2>"${err_file}"
    )"; then
      rm -f "${err_file}"
      printf '%s\n' "${output}"
      return 0
    else
      status=$?
    fi

    cat "${err_file}" >&2
    if ((attempt >= max_attempts)); then
      rm -f "${err_file}"
      return "${status}"
    fi
    echo "cargo tree for OHOS dependency graph failed (attempt ${attempt}/${max_attempts}); retrying in ${delay_seconds}s..." >&2
    sleep "${delay_seconds}"
  done
}

tree="$(cargo_tree_with_retry)"

disallowed="$(
  grep -E '^(nix v0\.(28|29)\.|portable-pty v|starlark v|arboard v|keyring v)' <<<"${tree}" || true
)"

if [[ -n "${disallowed}" ]]; then
  {
    echo "::error::OHOS target graph for ${package} includes unsupported dependencies:"
    echo "${disallowed}"
    echo
    echo "The OpenHarmony port avoids the rustyline/starlark/portable-pty/nix chain"
    echo "by target-gating those crates away from target_env=ohos. Keep this graph"
    echo "clean unless a real OHOS-compatible dependency update lands."
  } >&2
  exit 1
fi

echo "OHOS dependency graph OK for ${package} on ${target}."
