#!/usr/bin/env bash
# Update the Homebrew tap at Hmbown/homebrew-deepseek-tui after a release.
#
# Expected environment:
#   TAG       – git tag, e.g. "v0.8.31"
#   MANIFEST  – path to deepseek-artifacts-sha256.txt
#   TAP_REPO  – owner/repo of the Homebrew tap
#   TOKEN     – PAT with contents:write on TAP_REPO (optional; skips if unset)

set -euo pipefail

: "${TAG:?}"
: "${MANIFEST:?}"
: "${TAP_REPO:?}"

if [ -z "${TOKEN:-}" ]; then
  echo "No Homebrew tap token configured; skipping."
  exit 0
fi

VERSION="${TAG#v}"

die() { echo "::error::${1}" >&2; exit 1; }

sha() {
  local file="${1:?}"
  local val
  val="$(awk -v f="${file}" '$2 == f {print $1; exit}' "${MANIFEST}")"
  if [ -z "${val}" ]; then
    die "Missing binary in checksum manifest: ${file}"
  fi
  echo "${val}"
}

# --- read checksums ---------------------------------------------------

readonly SHA_DISP_MACOS_ARM="$(sha deepseek-macos-arm64)"
readonly SHA_TUI_MACOS_ARM="$(sha deepseek-tui-macos-arm64)"
readonly SHA_DISP_MACOS_X64="$(sha deepseek-macos-x64)"
readonly SHA_TUI_MACOS_X64="$(sha deepseek-tui-macos-x64)"
readonly SHA_DISP_LINUX_ARM="$(sha deepseek-linux-arm64)"
readonly SHA_TUI_LINUX_ARM="$(sha deepseek-tui-linux-arm64)"
readonly SHA_DISP_LINUX_X64="$(sha deepseek-linux-x64)"
readonly SHA_TUI_LINUX_X64="$(sha deepseek-tui-linux-x64)"

# --- temp dirs --------------------------------------------------------

FORMULA_FILE="$(mktemp)"
TAP_DIR="$(mktemp -d)"
trap 'rm -rf "${TAP_DIR}" "${FORMULA_FILE}"' EXIT

# --- generate formula --------------------------------------------------

readonly BASE_URL="https://github.com/Hmbown/DeepSeek-TUI/releases/download/${TAG}"

cat > "${FORMULA_FILE}" << EOF
class DeepseekTui < Formula
  desc "Terminal-native coding agent for DeepSeek V4"
  homepage "https://github.com/Hmbown/DeepSeek-TUI"
  version "${VERSION}"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "${BASE_URL}/deepseek-macos-arm64", using: :nounzip
      sha256 "${SHA_DISP_MACOS_ARM}"
      resource "tui" do
        url "${BASE_URL}/deepseek-tui-macos-arm64", using: :nounzip
        sha256 "${SHA_TUI_MACOS_ARM}"
      end
    else
      url "${BASE_URL}/deepseek-macos-x64", using: :nounzip
      sha256 "${SHA_DISP_MACOS_X64}"
      resource "tui" do
        url "${BASE_URL}/deepseek-tui-macos-x64", using: :nounzip
        sha256 "${SHA_TUI_MACOS_X64}"
      end
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "${BASE_URL}/deepseek-linux-arm64", using: :nounzip
      sha256 "${SHA_DISP_LINUX_ARM}"
      resource "tui" do
        url "${BASE_URL}/deepseek-tui-linux-arm64", using: :nounzip
        sha256 "${SHA_TUI_LINUX_ARM}"
      end
    else
      url "${BASE_URL}/deepseek-linux-x64", using: :nounzip
      sha256 "${SHA_DISP_LINUX_X64}"
      resource "tui" do
        url "${BASE_URL}/deepseek-tui-linux-x64", using: :nounzip
        sha256 "${SHA_TUI_LINUX_X64}"
      end
    end
  end

  def install
    bin.install Dir["*"].first => "deepseek"
    resource("tui").stage { bin.install Dir["*"].first => "deepseek-tui" }
  end

  test do
    system "#{bin}/deepseek", "--version"
  end
end
EOF

# --- push to tap repo --------------------------------------------------

ENCODED_TOKEN="$(printf '%s' "${TOKEN}" | python3 -c 'import sys,urllib.parse;print(urllib.parse.quote(sys.stdin.read(),safe=""))')"
TAP_URL="https://x-access-token:${ENCODED_TOKEN}@github.com/${TAP_REPO}.git"

git clone --depth 1 "${TAP_URL}" "${TAP_DIR}"

mkdir -p "${TAP_DIR}/Formula"
cp "${FORMULA_FILE}" "${TAP_DIR}/Formula/deepseek-tui.rb"

cd "${TAP_DIR}"
git config user.name  "github-actions[bot]"
git config user.email "github-actions[bot]@users.noreply.github.com"

git add Formula/deepseek-tui.rb

if git diff --cached --quiet; then
  echo "Formula unchanged (already at ${VERSION}); nothing to push."
  exit 0
fi

git commit -m "chore: bump formula to ${VERSION}

Automated update from the release workflow."

git push origin HEAD:main
echo "Pushed formula update to ${TAP_REPO} (v${VERSION})"
