#!/usr/bin/env bash
# shellcheck disable=SC1091,SC2034,SC2329
# The test sources a generated PKGBUILD whose makepkg variables are dynamic.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
temp_root="$(mktemp -d)"
trap 'rm -rf "${temp_root}"' EXIT

assets_dir="${temp_root}/release-assets"
stage_dir="${temp_root}/stage"
mkdir -p "${assets_dir}" "${stage_dir}"

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

for release_arch in x64 arm64; do
  archive_root="codewhale-linux-${release_arch}"
  mkdir -p "${stage_dir}/${archive_root}"
  printf '#!/usr/bin/env sh\necho codewhale-%s\n' "${release_arch}" \
    > "${stage_dir}/${archive_root}/codewhale"
  printf '#!/usr/bin/env sh\necho codew-%s\n' "${release_arch}" \
    > "${stage_dir}/${archive_root}/codew"
  chmod 0755 \
    "${stage_dir}/${archive_root}/codewhale" \
    "${stage_dir}/${archive_root}/codew"
  COPYFILE_DISABLE=1 tar -czf "${assets_dir}/${archive_root}.tar.gz" \
    -C "${stage_dir}" "${archive_root}"
done

for manifest in codewhale-artifacts-sha256.txt codewhale-bundles-sha256.txt; do
  {
    printf '%s  %s\n' \
      "$(sha256_file "${assets_dir}/codewhale-linux-x64.tar.gz")" \
      'codewhale-linux-x64.tar.gz'
    printf '%s  %s\n' \
      "$(sha256_file "${assets_dir}/codewhale-linux-arm64.tar.gz")" \
      'codewhale-linux-arm64.tar.gz'
  } > "${assets_dir}/${manifest}"
done

first_output="${temp_root}/first"
second_output="${temp_root}/second"
revision_output="${temp_root}/revision"
bash "${repo_root}/packaging/aur/render.sh" "${assets_dir}" "${first_output}"
bash "${repo_root}/packaging/aur/render.sh" "${assets_dir}" "${second_output}"
bash "${repo_root}/packaging/aur/render.sh" "${assets_dir}" "${revision_output}" 2
cmp "${first_output}/PKGBUILD" "${second_output}/PKGBUILD"
cmp "${first_output}/.SRCINFO" "${second_output}/.SRCINFO"

workspace_version="$(
  grep -E '^version = "' "${repo_root}/Cargo.toml" \
    | head -n 1 \
    | sed -E 's/^version = "([^"]+)".*/\1/'
)"
grep -Fqx "pkgver=${workspace_version}" "${first_output}/PKGBUILD"
grep -Fqx "depends=('glibc' 'gcc-libs' 'dbus')" "${first_output}/PKGBUILD"
for dependency in glibc gcc-libs dbus; do
  grep -Fqx $'\tdepends = '"${dependency}" "${first_output}/.SRCINFO"
done
grep -Fqx 'pkgrel=2' "${revision_output}/PKGBUILD"
grep -Fqx $'\tpkgrel = 2' "${revision_output}/.SRCINFO"
grep -Fq "/releases/download/v${workspace_version}/codewhale-linux-x64.tar.gz" \
  "${first_output}/.SRCINFO"
grep -Fq "/releases/download/v${workspace_version}/codewhale-linux-arm64.tar.gz" \
  "${first_output}/.SRCINFO"
if grep -R -Eq 'SKIP|@[A-Z0-9_]+@' "${first_output}"; then
  echo "rendered metadata retained a placeholder or SKIP checksum" >&2
  exit 1
fi

for arch_case in 'x86_64:x64' 'aarch64:arm64'; do
  carch="${arch_case%%:*}"
  release_arch="${arch_case#*:}"
  package_src="${temp_root}/package-src-${carch}"
  package_root="${temp_root}/package-root-${carch}"
  mkdir -p "${package_src}" "${package_root}"
  tar -xzf "${assets_dir}/codewhale-linux-${release_arch}.tar.gz" -C "${package_src}"
  cp "${repo_root}/LICENSE" "${package_src}/LICENSE-${workspace_version}"
  (
    source "${first_output}/PKGBUILD"
    install() {
      local mode="${1#-Dm}"
      command mkdir -p "$(dirname "$3")"
      command cp "$2" "$3"
      command chmod "${mode}" "$3"
    }
    CARCH="${carch}"
    srcdir="${package_src}"
    pkgdir="${package_root}"
    package
  )
  cmp \
    "${stage_dir}/codewhale-linux-${release_arch}/codewhale" \
    "${package_root}/usr/bin/codewhale"
  cmp \
    "${stage_dir}/codewhale-linux-${release_arch}/codew" \
    "${package_root}/usr/bin/codew"
  test -L "${package_root}/usr/bin/codewhale-tui"
  test "$(readlink "${package_root}/usr/bin/codewhale-tui")" = codewhale
  cmp "${repo_root}/LICENSE" \
    "${package_root}/usr/share/licenses/codewhale-bin/LICENSE"
done

printf 'tampered\n' >> "${assets_dir}/codewhale-linux-x64.tar.gz"
if bash "${repo_root}/packaging/aur/render.sh" \
  "${assets_dir}" "${temp_root}/tampered-output" >/dev/null 2>&1; then
  echo "renderer accepted a release archive that no longer matched its manifests" >&2
  exit 1
fi

echo "AUR render contract passed"
