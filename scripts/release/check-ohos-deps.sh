#!/usr/bin/env bash
# Guard the OpenHarmony target dependency graph.
#
# This check intentionally does not require an OpenHarmony SDK or sysroot. It
# only asks Cargo to resolve the codewhale-tui dependency graph for the OHOS
# target and fails if crates known to break or be unsupported on OHOS re-enter
# that graph. It also proves the OHOS target activates the rquickjs-sys
# `bindgen` feature for codewhale-workflow-js, which is the only reason the
# crate compiles for a target with no pre-generated QuickJS bindings.
set -euo pipefail

cd "$(dirname "$0")/../.."

target="${1:-aarch64-unknown-linux-ohos}"
package="${CODEWHALE_OHOS_DEP_PACKAGE:-codewhale-tui}"
workflow_js_package="${CODEWHALE_OHOS_WORKFLOW_JS_PACKAGE:-codewhale-workflow-js}"

require_literal() {
  local file="$1"
  local literal="$2"
  local description="$3"

  if ! grep -qF -- "${literal}" "${file}"; then
    echo "::error::OHOS Windows linker contract lost ${description}: ${file}" >&2
    return 1
  fi
}

# Cargo invokes its configured linker directly. On Windows that entry point
# must be the repository launcher, not the SDK's bare clang.exe, so the final
# Rust link receives the OHOS target, sysroot, and musl flags as reliably as C,
# C++, and bindgen invocations do. This is a static, no-SDK contract check; the
# launcher delegates to the PowerShell compiler wrapper that validates the SDK
# and preserves Cargo's linker arguments and exit status.
require_literal \
  scripts/ohos-env.ps1 \
  '$linker = [System.IO.Path]::Combine($PSScriptRoot, "ohos", "ohos-clang.cmd")' \
  "the repository-local linker launcher"
require_literal \
  scripts/ohos-env.ps1 \
  '$env:CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_LINKER = $linker' \
  "Cargo's target-specific linker assignment"
require_literal \
  scripts/ohos-env.ps1 \
  '$env:OHOS_NATIVE_SDK = $sdk' \
  "the absolute SDK path inherited by Cargo"
require_literal \
  scripts/ohos/ohos-clang.cmd \
  'ohos-clang.ps1' \
  "the cmd-to-PowerShell delegation"
require_literal \
  scripts/ohos/ohos-clang.cmd \
  '-File "%OHOS_LINKER_SCRIPT%" %*' \
  "linker argument forwarding"
require_literal \
  scripts/ohos/ohos-clang.cmd \
  'exit /b %ERRORLEVEL%' \
  "PowerShell exit-status propagation"
require_literal \
  scripts/ohos/ohos-clang.ps1 \
  '& $clang -target aarch64-linux-ohos "--sysroot=$sysroot" -D__MUSL__ @args' \
  "the target/sysroot/musl linker flags"
require_literal \
  scripts/ohos/ohos-clang.ps1 \
  'exit $LASTEXITCODE' \
  "native linker exit-status propagation"

if grep -qF -- '$env:CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_LINKER = $clang' scripts/ohos-env.ps1; then
  echo "::error::OHOS Windows Cargo linker points directly at clang.exe instead of the target-aware wrapper." >&2
  exit 1
fi

echo "OHOS Windows linker wrapper contract OK."

cargo_tree_with_retry() {
  local package="$1"
  shift
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
        --target "${target}" \
        --prefix none \
        "$@" \
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

tree="$(cargo_tree_with_retry "${package}" --all-features --no-dedupe)"

disallowed="$(
  grep -E '^(starlark v|arboard v|keyring v)' <<<"${tree}" || true
)"

if [[ -n "${disallowed}" ]]; then
  {
    echo "::error::OHOS target graph for ${package} includes unsupported dependencies:"
    echo "${disallowed}"
    echo
    echo "The OpenHarmony port avoids the rustyline/starlark/arboard/keyring chain"
    echo "by target-gating those crates away from target_env=ohos. portable-pty (and"
    echo "its transitive nix dep) are now included via cfg(unix) for OHOS PTY support."
    echo "Keep this graph clean unless a real OHOS-compatible dependency update lands."
  } >&2
  exit 1
fi

echo "OHOS dependency graph OK for ${package} on ${target}."

# codewhale-workflow-js only compiles for OHOS because its
# `cfg(target_env = "ohos")` dependency gate activates rquickjs's `bindgen`
# feature, which forwards to rquickjs-sys so QuickJS bindings are generated at
# build time. Resolve the feature graph for the OHOS target (pure metadata, no
# SDK or target toolchain needed) and fail loudly if either feature edge
# disappears — for example because the target gate was dropped or an rquickjs
# upgrade renamed the feature.
workflow_js_features="$(cargo_tree_with_retry "${workflow_js_package}" --edges features)"

if ! grep -qF 'rquickjs feature "bindgen"' <<<"${workflow_js_features}" \
  || ! grep -qF 'rquickjs-sys feature "bindgen"' <<<"${workflow_js_features}"; then
  {
    echo "::error::OHOS target graph for ${workflow_js_package} lost the rquickjs bindgen feature edges:"
    grep -E '^(rquickjs|rquickjs-sys|rquickjs-core|bindgen)( v| feature)' <<<"${workflow_js_features}" || true
    echo
    echo "crates/workflow-js/Cargo.toml must keep activating rquickjs's \`bindgen\`"
    echo "feature under cfg(target_env = \"ohos\"); rquickjs ships no pre-generated"
    echo "QuickJS bindings for OpenHarmony, so without that edge the crate cannot"
    echo "compile for ${target}."
  } >&2
  exit 1
fi

echo "OHOS rquickjs-sys bindgen feature edge OK for ${workflow_js_package} on ${target}."
