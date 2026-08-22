#!/bin/sh
set -eu

repo="Hmbown/CodeWhale"
version="${CODEWHALE_VERSION:-latest}"
release_base="${CODEWHALE_RELEASE_BASE_URL:-${DEEPSEEK_TUI_RELEASE_BASE_URL:-}}"

usage() {
  cat <<'USAGE'
Codewhale installer for macOS and Linux.

Usage:
  curl -fsSL https://codewhale.net/install.sh | sh

Environment:
  CODEWHALE_INSTALL_DIR    Install directory. Default: $HOME/.local/bin
  CODEWHALE_VERSION        Release tag to install, for example v0.9.0. Default: latest
  CODEWHALE_RELEASE_BASE_URL
                           Custom release asset base URL ending in /download
  CODEWHALE_SKIP_GLIBC_CHECK=1
                           Skip Linux arm64 glibc compatibility preflight

Examples:
  curl -fsSL https://codewhale.net/install.sh | CODEWHALE_INSTALL_DIR=/usr/local/bin sh
  curl -fsSL https://codewhale.net/install.sh | CODEWHALE_VERSION=v0.9.0 sh
USAGE
}

case "${1:-}" in
  -h|--help)
    usage
    exit 0
    ;;
esac

say() {
  printf '%s\n' "$*"
}

fail() {
  printf 'codewhale install: %s\n' "$*" >&2
  exit 1
}

if [ -n "${CODEWHALE_INSTALL_DIR:-}" ]; then
  install_dir="$CODEWHALE_INSTALL_DIR"
else
  [ -n "${HOME:-}" ] || fail "HOME is not set; set CODEWHALE_INSTALL_DIR"
  install_dir="$HOME/.local/bin"
fi

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}

download() {
  url="$1"
  out="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$out"
  elif command -v wget >/dev/null 2>&1; then
    wget -q "$url" -O "$out"
  else
    fail "curl or wget is required"
  fi
}

sha256_file() {
  file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$file" | awk '{print $1}'
  else
    fail "sha256sum or shasum is required to verify downloads"
  fi
}

verify_asset() {
  asset="$1"
  file="$2"
  manifest="$3"
  expected="$(
    awk -v name="$asset" '
      {
        digest = tolower($1)
        file = $2
        sub(/^\*/, "", file)
        if (file == name && digest ~ /^[0-9a-f]{64}$/) {
          print digest
          exit
        }
      }
    ' "$manifest"
  )"
  [ -n "$expected" ] || fail "checksum not found for $asset"
  actual="$(sha256_file "$file" | tr '[:upper:]' '[:lower:]')"
  [ "$actual" = "$expected" ] || fail "checksum mismatch for $asset"
}

glibc_version() {
  if command -v getconf >/dev/null 2>&1; then
    getconf GNU_LIBC_VERSION 2>/dev/null | awk '{ print $NF; exit }'
    return
  fi
  if command -v ldd >/dev/null 2>&1; then
    ldd --version 2>/dev/null | awk 'NR == 1 {
      for (i = 1; i <= NF; i++) {
        if ($i ~ /^[0-9]+\.[0-9]+/) {
          print $i
          exit
        }
      }
    }'
  fi
}

version_at_least() {
  have="$1"
  need="$2"
  awk -v have="$have" -v need="$need" '
    BEGIN {
      split(have, h, ".")
      split(need, n, ".")
      for (i = 1; i <= 3; i++) {
        hv = h[i] + 0
        nv = n[i] + 0
        if (hv > nv) exit 0
        if (hv < nv) exit 1
      }
      exit 0
    }
  '
}

check_glibc() {
  case "$target" in
    linux-arm64) ;;
    *) return ;;
  esac

  # Linux arm64 assets became static musl builds in v0.9.6. `latest` and
  # explicit v0.9.6+ installs therefore have no glibc floor. Keep the
  # preflight only for explicitly requested older releases, whose arm64
  # assets were linked against GNU libc on Ubuntu 24.04.
  if [ "$version" = "latest" ]; then
    return
  fi
  numeric_version="${version#v}"
  if awk -v have="$numeric_version" '
    BEGIN {
      if (have !~ /^[0-9]+\.[0-9]+\.[0-9]+$/) exit 1
      split(have, h, ".")
      if (h[1] > 0) exit 0
      if (h[1] < 0) exit 1
      if (h[2] > 9) exit 0
      if (h[2] < 9) exit 1
      exit !(h[3] >= 6)
    }
  '; then
    return
  fi

  [ "${CODEWHALE_SKIP_GLIBC_CHECK:-}" = "1" ] && return
  [ "${DEEPSEEK_TUI_SKIP_GLIBC_CHECK:-}" = "1" ] && return
  [ "${DEEPSEEK_SKIP_GLIBC_CHECK:-}" = "1" ] && return

  required="2.39"
  host="$(glibc_version || true)"
  if [ -z "$host" ] || ! version_at_least "$host" "$required"; then
    cat >&2 <<EOF
codewhale install: Codewhale $version $target assets require glibc $required or newer.
This system reports glibc ${host:-unavailable}.

Linux arm64 assets before v0.9.6 were GNU libc builds from Ubuntu 24.04.
Current v0.9.6+ assets are static musl builds. Build this older release from
source with Cargo or set
CODEWHALE_SKIP_GLIBC_CHECK=1 to bypass this check at your own risk.
EOF
    exit 1
  fi
}

detect_platform() {
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os" in
    Darwin) platform="macos" ;;
    Linux) platform="linux" ;;
    *) fail "unsupported OS: $os. Use npm, Cargo, or the GitHub Releases page." ;;
  esac

  case "$arch" in
    x86_64|amd64) cpu="x64" ;;
    arm64|aarch64) cpu="arm64" ;;
    riscv64) fail "Linux riscv64 prebuilt assets are temporarily unavailable because the locked rquickjs-sys dependency does not ship riscv64gc bindings." ;;
    *) fail "unsupported CPU architecture: $arch. Use Cargo or build from source." ;;
  esac

  printf '%s-%s' "$platform" "$cpu"
}

if [ -z "$release_base" ]; then
  if [ "$version" = "latest" ]; then
    release_base="https://github.com/$repo/releases/latest/download"
  else
    release_base="https://github.com/$repo/releases/download/$version"
  fi
fi

target="$(detect_platform)"
check_glibc
cli_asset="codewhale-$target"
shim_asset="codew-$target"
manifest_asset="codewhale-artifacts-sha256.txt"

tmpdir="$(mktemp -d 2>/dev/null || mktemp -d -t codewhale-install)"
trap 'rm -rf "$tmpdir"' EXIT INT TERM

say "Installing Codewhale for $target"
say "Release assets: $release_base"
say "Install dir: $install_dir"

download "$release_base/$manifest_asset" "$tmpdir/$manifest_asset"
download "$release_base/$cli_asset" "$tmpdir/codewhale"
download "$release_base/$shim_asset" "$tmpdir/codew"

verify_asset "$cli_asset" "$tmpdir/codewhale" "$tmpdir/$manifest_asset"
verify_asset "$shim_asset" "$tmpdir/codew" "$tmpdir/$manifest_asset"
say "Checksums verified"

chmod 755 "$tmpdir/codewhale" "$tmpdir/codew"
if command -v xattr >/dev/null 2>&1; then
  xattr -d com.apple.quarantine "$tmpdir/codewhale" "$tmpdir/codew" 2>/dev/null || true
fi

sudo_cmd=""
if [ -d "$install_dir" ]; then
  if [ ! -w "$install_dir" ] ||
    { [ -e "$install_dir/codewhale" ] && [ ! -w "$install_dir/codewhale" ]; } ||
    { [ -e "$install_dir/codew" ] && [ ! -w "$install_dir/codew" ]; }; then
    need_cmd sudo
    sudo_cmd="sudo"
  fi
else
  if ! mkdir -p "$install_dir" 2>/dev/null; then
    need_cmd sudo
    sudo mkdir -p "$install_dir"
    sudo_cmd="sudo"
  fi
fi

legacy_tui="$install_dir/codewhale-tui"
refresh_legacy_tui=0
# v0.9.4's website installer placed a regular codewhale-tui binary beside
# codewhale. A clean v0.9.5 install exposes only codewhale + codew, but an
# upgrade must not leave that installer-owned path running stale v0.9.4 code.
# Refresh the existing compatibility command from the already verified
# consolidated bytes. Do not create it for new installs or replace a symlink.
if [ -f "$legacy_tui" ] && [ ! -L "$legacy_tui" ]; then
  refresh_legacy_tui=1
fi

stage_cli="$install_dir/.codewhale.$$"
stage_shim="$install_dir/.codew.$$"
stage_legacy_tui="$install_dir/.codewhale-tui.$$"
trap 'rm -rf "$tmpdir"; rm -f "$stage_cli" "$stage_shim" "$stage_legacy_tui" 2>/dev/null || true' EXIT INT TERM

$sudo_cmd cp "$tmpdir/codewhale" "$stage_cli"
$sudo_cmd cp "$tmpdir/codew" "$stage_shim"
$sudo_cmd chmod 755 "$stage_cli" "$stage_shim"
if [ "$refresh_legacy_tui" -eq 1 ]; then
  $sudo_cmd cp "$tmpdir/codewhale" "$stage_legacy_tui"
  $sudo_cmd chmod 755 "$stage_legacy_tui"
fi
$sudo_cmd mv "$stage_cli" "$install_dir/codewhale"
$sudo_cmd mv "$stage_shim" "$install_dir/codew"
if [ "$refresh_legacy_tui" -eq 1 ]; then
  $sudo_cmd mv "$stage_legacy_tui" "$legacy_tui"
  say "Refreshed legacy compatibility command: $legacy_tui"
fi

say "Installed:"
"$install_dir/codewhale" --version || true
"$install_dir/codew" --version || true
if [ "$refresh_legacy_tui" -eq 1 ]; then
  "$legacy_tui" --version || true
fi

case ":$PATH:" in
  *":$install_dir:"*) ;;
  *)
    say ""
    say "Add $install_dir to PATH to run codewhale from any terminal."
    ;;
esac

say ""
say "Run: codew (or codewhale)"
