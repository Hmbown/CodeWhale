#!/usr/bin/env bash
set -euo pipefail

# Atomically install the exact binaries built by this checkout and leave a
# durable identity receipt. Replacing the directory entry (rather than copying
# over a running vnode) keeps live sessions on their old image while new shells
# get the new build safely.

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
src_dir="${1:-${repo_root}/target/release}"

if [[ ! -x "${src_dir}/codewhale" ]]; then
  echo "ERROR: expected executable codewhale in ${src_dir}" >&2
  # Since #5245 local builds are unstamped ("(dev)"); a dogfood build must be
  # stamped explicitly or the identity check below will (correctly) refuse it.
  echo "Build first: CODEWHALE_BUILD_SHA=\$(git rev-parse HEAD) cargo build --release -p codewhale-cli --locked" >&2
  exit 1
fi

source_sha="$(git -C "${repo_root}" rev-parse HEAD)"
source_dirty="$(git -C "${repo_root}" status --porcelain --untracked-files=no)"
if [[ -n "${source_dirty}" ]]; then
  if [[ "${CODEWHALE_ALLOW_DIRTY_DOGFOOD:-0}" != "1" ]]; then
    echo "ERROR: refusing to install from a dirty source tree" >&2
    echo "Commit/stash the source, or set CODEWHALE_ALLOW_DIRTY_DOGFOOD=1 explicitly." >&2
    exit 1
  fi
  source_identity="${source_sha}-dirty"
else
  source_identity="${source_sha}"
fi

cli_version="$("${src_dir}/codewhale" --version)"
shim_version="${cli_version}"
short_sha="${source_sha:0:12}"
if [[ "${cli_version}" != *"${short_sha}"* ]]; then
  echo "ERROR: release binaries do not embed current HEAD ${short_sha}" >&2
  echo "  codewhale: ${cli_version}" >&2
  echo "Rebuild this checkout before installing:" >&2
  echo "  CODEWHALE_BUILD_SHA=\$(git rev-parse HEAD) cargo build --release -p codewhale-cli --locked" >&2
  exit 1
fi
cli_sha="$(shasum -a 256 "${src_dir}/codewhale" | awk '{print $1}')"
shim_sha="${cli_sha}"

default_install_dirs="${HOME}/.cargo/bin:${HOME}/.local/bin"
for command_name in codewhale codew; do
  if command_path="$(command -v "${command_name}" 2>/dev/null)" \
    && [[ "${command_path}" == "${HOME}/"* ]]; then
    command_dir="$(dirname "${command_path}")"
    if [[ ":${default_install_dirs}:" != *":${command_dir}:"* ]]; then
      default_install_dirs="${default_install_dirs}:${command_dir}"
    fi
  fi
done
IFS=':' read -r -a dest_dirs <<< "${CODEWHALE_INSTALL_DIRS:-${default_install_dirs}}"

install_binary() {
  local src="$1"
  local dst="$2"
  local tmp="${dst}.tmp.$$"
  trap 'rm -f -- "${tmp}"' RETURN
  cp "${src}" "${tmp}"
  chmod 0755 "${tmp}"
  # macOS AMFI kills ad-hoc linker-signed binaries after `cp` into a new
  # path (SIGKILL on exec, no output). Re-sign in place with a proper
  # ad-hoc signature so self-built dogfood installs run after install.
  if [[ "$(uname -s)" == "Darwin" ]] && command -v codesign >/dev/null 2>&1; then
    codesign --force --sign - "${tmp}" >/dev/null 2>&1 || {
      echo "WARN: codesign failed for ${tmp}; binary may be killed by AMFI after install" >&2
    }
  fi
  mv -f "${tmp}" "${dst}"
  # Re-sign the final path as well — some macOS versions re-evaluate on rename.
  if [[ "$(uname -s)" == "Darwin" ]] && command -v codesign >/dev/null 2>&1; then
    codesign --force --sign - "${dst}" >/dev/null 2>&1 || true
  fi
  cmp -s "${src}" "${dst}" || {
    # cmp can fail after codesign rewrote the code signature; verify exec instead.
    if [[ ! -x "${dst}" ]]; then
      echo "ERROR: installed binary not executable: ${dst}" >&2
      return 1
    fi
  }
  trap - RETURN
}

installed=()
for dest in "${dest_dirs[@]}"; do
  mkdir -p "${dest}"
  install_binary "${src_dir}/codewhale" "${dest}/codewhale"
  install_binary "${src_dir}/codewhale" "${dest}/codew"
  installed+=("${dest}/codewhale" "${dest}/codew")
done

verify_fresh_shell_binary() {
  local command_name="$1"
  local command_path
  local command_version
  local dest
  local is_installed=0

  command_path="$(zsh -lc "command -v ${command_name}" 2>/dev/null || true)"
  if [[ -z "${command_path}" || ! -x "${command_path}" ]]; then
    echo "ERROR: fresh login shell cannot resolve ${command_name}" >&2
    return 1
  fi
  for dest in "${dest_dirs[@]}"; do
    if [[ "${command_path}" == "${dest}/${command_name}" ]]; then
      is_installed=1
      break
    fi
  done
  if [[ "${is_installed}" != "1" ]]; then
    echo "ERROR: fresh-shell ${command_name} resolves outside the installed destinations: ${command_path}" >&2
    return 1
  fi
  command_version="$(zsh -lc "${command_name} --version" 2>/dev/null || true)"
  if [[ "${command_version}" != *"${short_sha}"* ]]; then
    echo "ERROR: fresh-shell ${command_name} does not report current HEAD ${short_sha}" >&2
    return 1
  fi
  printf '%s\n' "${command_path}"
}

path_cli="$(verify_fresh_shell_binary codewhale)"
path_shim="$(verify_fresh_shell_binary codew)"
installed_cli_sha="$(shasum -a 256 "${path_cli}" | awk '{print $1}')"
installed_shim_sha="$(shasum -a 256 "${path_shim}" | awk '{print $1}')"

default_receipt_root="${HOME}/.codewhale/dogfood-receipts"
if [[ -d "/Volumes/VIXinSSD/CW/backups" ]]; then
  default_receipt_root="/Volumes/VIXinSSD/CW/backups/dogfood-installs"
fi
receipt_root="${CODEWHALE_DOGFOOD_RECEIPT_DIR:-${default_receipt_root}}"
mkdir -p "${receipt_root}"
timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
receipt="${receipt_root}/${timestamp}-${source_sha:0:12}.txt"
{
  echo "installed_at_utc=${timestamp}"
  echo "source_repo=${repo_root}"
  echo "source_commit=${source_identity}"
  echo "source_dir=${src_dir}"
  echo "codewhale_version=${cli_version}"
  echo "codewhale_sha256=${cli_sha}"
  echo "installed_codewhale_sha256=${installed_cli_sha}"
  echo "codew_version=${shim_version}"
  echo "codew_sha256=${shim_sha}"
  echo "installed_codew_sha256=${installed_shim_sha}"
  echo "fresh_shell_codewhale=${path_cli}"
  echo "fresh_shell_codew=${path_shim}"
  printf 'installed_path=%s\n' "${installed[@]}"
} >"${receipt}"

echo "Installed ${source_identity}:"
printf '  %s\n' "${installed[@]}"
echo "Receipt: ${receipt}"
echo "Fresh-shell check: zsh -lc 'type -a codew codewhale; codew --version; codewhale --version'"
