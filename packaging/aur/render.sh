#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 RELEASE_ASSETS_DIR OUTPUT_DIR [PKGREL]" >&2
}

if [[ $# -lt 2 || $# -gt 3 ]]; then
  usage
  exit 2
fi

assets_dir="$1"
output_dir="$2"
pkgrel="${3:-1}"
repo_root="$(cd "$(dirname "$0")/../.." && pwd)"

if [[ ! -d "${assets_dir}" ]]; then
  echo "release assets directory does not exist: ${assets_dir}" >&2
  exit 1
fi
assets_dir="$(cd "${assets_dir}" && pwd)"

if [[ -e "${output_dir}" && ! -d "${output_dir}" ]]; then
  echo "output path is not a directory: ${output_dir}" >&2
  exit 1
fi
mkdir -p "${output_dir}"
if [[ -n "$(find "${output_dir}" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
  echo "output directory must be empty: ${output_dir}" >&2
  exit 1
fi
output_dir="$(cd "${output_dir}" && pwd)"

workspace_version="$(
  grep -E '^version = "' "${repo_root}/Cargo.toml" \
    | head -n 1 \
    | sed -E 's/^version = "([^"]+)".*/\1/'
)"
if [[ ! "${workspace_version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "workspace version must be X.Y.Z, got: ${workspace_version:-<missing>}" >&2
  exit 1
fi
if [[ ! "${pkgrel}" =~ ^[1-9][0-9]*(\.[1-9][0-9]*)?$ ]]; then
  echo "PKGREL must be a positive integer or positive x.y value, got: ${pkgrel}" >&2
  exit 2
fi

artifact_manifest="${assets_dir}/codewhale-artifacts-sha256.txt"
bundle_manifest="${assets_dir}/codewhale-bundles-sha256.txt"
for manifest in "${artifact_manifest}" "${bundle_manifest}"; do
  if [[ ! -f "${manifest}" ]]; then
    echo "release assets are missing checksum manifest: ${manifest}" >&2
    exit 1
  fi
done

sha256_file() {
  local path="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "${path}" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "${path}" | awk '{print $1}'
  else
    echo "sha256sum or shasum is required" >&2
    return 1
  fi
}

manifest_sha() {
  local manifest="$1"
  local asset="$2"
  local matches match_count checksum
  matches="$(awk -v asset="${asset}" '$2 == asset { print $1 }' "${manifest}")"
  match_count="$(printf '%s\n' "${matches}" | awk 'NF { count++ } END { print count + 0 }')"
  if [[ "${match_count}" -ne 1 ]]; then
    echo "$(basename "${manifest}") must contain exactly one checksum for ${asset}" >&2
    return 1
  fi
  checksum="$(printf '%s\n' "${matches}" | awk 'NF { print; exit }')"
  if [[ ! "${checksum}" =~ ^[0-9a-fA-F]{64}$ ]]; then
    echo "invalid checksum for ${asset} in $(basename "${manifest}"): ${checksum}" >&2
    return 1
  fi
  printf '%s' "${checksum}" | tr 'A-F' 'a-f'
}

verified_archive_sha() {
  local asset="$1"
  local archive="${assets_dir}/${asset}"
  if [[ ! -f "${archive}" ]]; then
    echo "release assets are missing ${asset}" >&2
    return 1
  fi

  local artifact_sha bundle_sha actual_sha
  artifact_sha="$(manifest_sha "${artifact_manifest}" "${asset}")"
  bundle_sha="$(manifest_sha "${bundle_manifest}" "${asset}")"
  if [[ "${artifact_sha}" != "${bundle_sha}" ]]; then
    echo "checksum manifests disagree for ${asset}" >&2
    return 1
  fi
  actual_sha="$(sha256_file "${archive}")"
  if [[ "${artifact_sha}" != "${actual_sha}" ]]; then
    echo "release asset checksum mismatch for ${asset}" >&2
    return 1
  fi

  local release_arch="${asset#codewhale-linux-}"
  release_arch="${release_arch%.tar.gz}"
  local listing
  listing="$(tar -tzf "${archive}")"
  for entry in \
    "codewhale-linux-${release_arch}/codewhale" \
    "codewhale-linux-${release_arch}/codew"; do
    if ! grep -Fqx "${entry}" <<<"${listing}"; then
      echo "${asset} is missing required archive entry: ${entry}" >&2
      return 1
    fi
  done

  printf '%s' "${actual_sha}"
}

x86_64_sha="$(verified_archive_sha 'codewhale-linux-x64.tar.gz')"
aarch64_sha="$(verified_archive_sha 'codewhale-linux-arm64.tar.gz')"
license_sha="$(sha256_file "${repo_root}/LICENSE")"

render_template() {
  local source="$1"
  local destination="$2"
  local content
  content="$(<"${source}")"
  content="${content//@PKGVER@/${workspace_version}}"
  content="${content//@PKGREL@/${pkgrel}}"
  content="${content//@LICENSE_SHA256@/${license_sha}}"
  content="${content//@X86_64_SHA256@/${x86_64_sha}}"
  content="${content//@AARCH64_SHA256@/${aarch64_sha}}"
  printf '%s\n' "${content}" > "${destination}"
}

render_template "${repo_root}/packaging/aur/PKGBUILD.template" "${output_dir}/PKGBUILD"
render_template "${repo_root}/packaging/aur/SRCINFO.template" "${output_dir}/.SRCINFO"

for rendered in "${output_dir}/PKGBUILD" "${output_dir}/.SRCINFO"; do
  if grep -Eq '@(PKGVER|PKGREL|LICENSE_SHA256|X86_64_SHA256|AARCH64_SHA256)@|SKIP' "${rendered}"; then
    echo "rendered AUR metadata contains an unresolved or insecure checksum token: ${rendered}" >&2
    exit 1
  fi
done
bash -n "${output_dir}/PKGBUILD"

if command -v makepkg >/dev/null 2>&1; then
  generated_srcinfo="$(mktemp)"
  trap 'rm -f "${generated_srcinfo}"' EXIT
  (cd "${output_dir}" && makepkg --printsrcinfo) > "${generated_srcinfo}"
  if ! cmp -s "${generated_srcinfo}" "${output_dir}/.SRCINFO"; then
    echo "rendered .SRCINFO does not match makepkg --printsrcinfo" >&2
    diff -u "${output_dir}/.SRCINFO" "${generated_srcinfo}" >&2 || true
    exit 1
  fi
fi

echo "Rendered codewhale-bin ${workspace_version}-${pkgrel} from verified release archives:"
echo "  ${output_dir}/PKGBUILD"
echo "  ${output_dir}/.SRCINFO"
