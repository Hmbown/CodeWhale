#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "${tmp_dir}"' EXIT

manifest="${tmp_dir}/codewhale-artifacts-sha256.txt"
formula="${tmp_dir}/codewhale.rb"
legacy="${tmp_dir}/deepseek-tui.rb"

assets=(
  codewhale-macos-arm64
  codew-macos-arm64
  codewhale-macos-x64
  codew-macos-x64
  codewhale-linux-arm64
  codew-linux-arm64
  codewhale-linux-x64
  codew-linux-x64
)

for asset in "${assets[@]}"; do
  printf '%064d  %s\n' 0 "${asset}" >> "${manifest}"
done

TAG=v1.2.3 \
MANIFEST="${manifest}" \
TAP_REPO=Hmbown/homebrew-deepseek-tui \
FORMULA_OUTPUT="${formula}" \
FORMULA_LEGACY_OUTPUT="${legacy}" \
  bash "${repo_root}/.github/scripts/update-homebrew-tap.sh"

ruby -c "${formula}" >/dev/null
ruby -c "${legacy}" >/dev/null
grep -Fq 'class Codewhale < Formula' "${formula}"
grep -Fq 'class DeepseekTui < Formula' "${legacy}"
grep -Fq 'deprecate! date: "2026-08-14", because: "renamed to codewhale"' "${legacy}"
grep -Fq 'desc "Agentic terminal for open-source and open-weight coding models"' "${formula}"
test "$(grep -Fc 'resource "codew" do' "${formula}")" -eq 4
grep -Fq 'bin.install Dir["*"].first => "codew"' "${formula}"
grep -Fq 'system "#{bin}/codew", "--version"' "${formula}"
if grep -Fq 'codewhale-tui' "${formula}"; then
  echo "Homebrew formula must not install the legacy TUI compatibility asset" >&2
  exit 1
fi
if grep -Fq 'class DeepseekTui' "${formula}"; then
  echo "Primary Homebrew formula must be Codewhale, not DeepseekTui" >&2
  exit 1
fi

echo "update-homebrew-tap tests passed"
