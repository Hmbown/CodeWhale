#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/release/crates.sh
source "${script_dir}/crates.sh"

mode="${1:-dry-run}"
case "${mode}" in
  dry-run|publish) ;;
  *)
    echo "usage: $0 [dry-run|publish]" >&2
    exit 1
    ;;
esac

packages=("${release_crates[@]}")

workspace_version=""
workspace_codewhale_packages=()
workspace_package_dep_flags=()

while IFS=$'\t' read -r kind name value; do
  case "${kind}" in
    version)
      workspace_version="${name}"
      ;;
    crate)
      workspace_codewhale_packages+=("${name}")
      workspace_package_dep_flags+=("${value}")
      ;;
  esac
done < <(
  python3 - <<'PY'
import json
import subprocess

metadata = json.loads(
    subprocess.check_output(["cargo", "metadata", "--format-version", "1", "--no-deps"])
)
workspace_members = set(metadata["workspace_members"])
workspace_packages = [
    pkg for pkg in metadata["packages"] if pkg["id"] in workspace_members
]
workspace_by_name = {pkg["name"]: pkg for pkg in workspace_packages}

versions = sorted({pkg["version"] for pkg in workspace_packages})
if not versions:
    raise SystemExit("workspace has no packages")
if len(versions) != 1:
    raise SystemExit(f"workspace packages have mixed versions: {', '.join(versions)}")
print(f"version\t{versions[0]}\t")

for pkg in sorted(workspace_packages, key=lambda item: item["name"]):
    if not pkg["name"].startswith("codewhale-"):
        continue
    has_workspace_dep = any(
        dep.get("path") and dep["name"] in workspace_by_name
        for dep in pkg["dependencies"]
    )
    print(f"crate\t{pkg['name']}\t{1 if has_workspace_dep else 0}")
PY
)

if [[ -z "${workspace_version}" ]]; then
  echo "Could not determine workspace version." >&2
  exit 1
fi

missing_packages=()
for workspace_package in "${workspace_codewhale_packages[@]}"; do
  found=0
  for package in "${packages[@]}"; do
    if [[ "${package}" == "${workspace_package}" ]]; then
      found=1
      break
    fi
  done
  if [[ "${found}" == "0" ]]; then
    missing_packages+=("${workspace_package}")
  fi
done

extra_packages=()
for package in "${packages[@]}"; do
  found=0
  for workspace_package in "${workspace_codewhale_packages[@]}"; do
    if [[ "${package}" == "${workspace_package}" ]]; then
      found=1
      break
    fi
  done
  if [[ "${found}" == "0" ]]; then
    extra_packages+=("${package}")
  fi
done

if (( ${#missing_packages[@]} > 0 || ${#extra_packages[@]} > 0 )); then
  if (( ${#missing_packages[@]} > 0 )); then
    echo "publish package list is missing workspace crates: ${missing_packages[*]}" >&2
  fi
  if (( ${#extra_packages[@]} > 0 )); then
    echo "publish package list contains non-workspace crates: ${extra_packages[*]}" >&2
  fi
  exit 1
fi

package_has_workspace_deps() {
  local package_name="$1"
  local index
  for ((index = 0; index < ${#workspace_codewhale_packages[@]}; index += 1)); do
    if [[ "${workspace_codewhale_packages[$index]}" == "${package_name}" ]]; then
      [[ "${workspace_package_dep_flags[$index]}" == "1" ]]
      return
    fi
  done

  echo "Unknown workspace crate: ${package_name}" >&2
  return 1
}

crate_version_exists() {
  local crate_name="$1"
  local crate_version="$2"
  curl -fsSL "https://crates.io/api/v1/crates/${crate_name}/${crate_version}" >/dev/null 2>&1
}

wait_for_crate_version() {
  local crate_name="$1"
  local crate_version="$2"
  local attempts=30

  for ((attempt = 1; attempt <= attempts; attempt += 1)); do
    if crate_version_exists "${crate_name}" "${crate_version}"; then
      return 0
    fi
    echo "Waiting for ${crate_name} ${crate_version} to appear on crates.io (${attempt}/${attempts})..."
    sleep 10
  done

  echo "Timed out waiting for ${crate_name} ${crate_version} to appear on crates.io" >&2
  return 1
}

for package in "${packages[@]}"; do
  echo "::group::${mode} ${package}"
  if [[ "${mode}" == "dry-run" ]]; then
    if package_has_workspace_deps "${package}"; then
      cargo package --allow-dirty --locked --list -p "${package}" >/dev/null
      echo "Verified package contents for ${package}; full crates.io dry-run requires workspace dependencies at ${workspace_version} to be published first."
    else
      cargo publish --dry-run --locked --allow-dirty -p "${package}"
    fi
  else
    if crate_version_exists "${package}" "${workspace_version}"; then
      echo "Skipping ${package} ${workspace_version}; already published."
    else
      cargo publish --locked -p "${package}"
      wait_for_crate_version "${package}" "${workspace_version}"
    fi
  fi
  echo "::endgroup::"
done
