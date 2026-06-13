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

tree="$(
  cargo tree \
    --locked \
    --package "${package}" \
    --all-features \
    --target "${target}" \
    --prefix none \
    --no-dedupe
)"

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
