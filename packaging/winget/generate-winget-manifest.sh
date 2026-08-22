#!/usr/bin/env bash
set -euo pipefail
# Regenerate Hmbown.CodeWhale winget manifests for a given release version.
# Usage: ./packaging/winget/generate-winget-manifest.sh X.Y.Z [release-assets-dir]
#   X.Y.Z               — version without leading v (e.g. 0.9.5)
#   release-assets-dir  — directory containing codewhale-artifacts-sha256.txt and the four
#                         Windows assets (defaults to ./release-assets if present).
# The script rewrites both packaging/winget/Hmbown.CodeWhale.yaml and .winget/Hmbown.CodeWhale.yaml.

if [[ $# -lt 1 || $# -gt 2 ]]; then
  echo "usage: $0 X.Y.Z [release-assets-dir]" >&2
  exit 2
fi

version="$1"
assets_dir="${2:-release-assets}"

if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "version must be X.Y.Z without leading v, got: $version" >&2
  exit 2
fi

if [[ ! -f "$assets_dir/codewhale-artifacts-sha256.txt" ]]; then
  echo "release assets dir must contain codewhale-artifacts-sha256.txt: $assets_dir" >&2
  exit 1
fi

require_sha() {
  local name="$1"
  local sha
  sha="$(grep -E "  ${name}\$" "$assets_dir/codewhale-artifacts-sha256.txt" | awk '{print $1}' || true)"
  if [[ -z "$sha" ]]; then
    echo "SHA not found for $name in $assets_dir/codewhale-artifacts-sha256.txt" >&2
    exit 1
  fi
  if [[ ! "$sha" =~ ^[0-9a-fA-F]{64}$ ]]; then
    echo "invalid SHA for $name: $sha" >&2
    exit 1
  fi
  printf '%s' "$sha" | tr 'A-F' 'a-f'
}

installer_sha="$(require_sha "CodeWhaleSetup.exe")"
x64_sha="$(require_sha "codewhale-windows-x64.zip")"
x64_portable_sha="$(require_sha "codewhale-windows-x64-portable.zip")"
arm64_sha="$(require_sha "codewhale-windows-arm64.zip")"
arm64_portable_sha="$(require_sha "codewhale-windows-arm64-portable.zip")"
today="$(date -u +%Y-%m-%d)"

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
winget_primary="$repo_root/packaging/winget/Hmbown.CodeWhale.yaml"
winget_mirror="$repo_root/.winget/Hmbown.CodeWhale.yaml"

bump_manifest() {
  local file="$1"
  if [[ ! -f "$file" ]]; then
    echo "manifest not found: $file" >&2
    exit 1
  fi
  # PackageVersion + ReleaseNotes
  sed -i.bak -E "s/^PackageVersion:.*/PackageVersion: ${version}/" "$file"
  sed -i.bak -E "s|ReleaseNotes:.*|ReleaseNotes: https://github.com/Hmbown/CodeWhale/releases/tag/v${version}|" "$file"
  sed -i.bak -E "s|ReleaseNotesUrl:.*|ReleaseNotesUrl: https://github.com/Hmbown/CodeWhale/releases/tag/v${version}|" "$file"
  # Installer URLs (all five)
  sed -i.bak -E "s|https://github.com/Hmbown/CodeWhale/releases/download/v[0-9.]+/CodeWhaleSetup.exe|https://github.com/Hmbown/CodeWhale/releases/download/v${version}/CodeWhaleSetup.exe|g" "$file"
  sed -i.bak -E "s|https://github.com/Hmbown/CodeWhale/releases/download/v[0-9.]+/codewhale-windows-x64\.zip|https://github.com/Hmbown/CodeWhale/releases/download/v${version}/codewhale-windows-x64.zip|g" "$file"
  sed -i.bak -E "s|https://github.com/Hmbown/CodeWhale/releases/download/v[0-9.]+/codewhale-windows-x64-portable\.zip|https://github.com/Hmbown/CodeWhale/releases/download/v${version}/codewhale-windows-x64-portable.zip|g" "$file"
  sed -i.bak -E "s|https://github.com/Hmbown/CodeWhale/releases/download/v[0-9.]+/codewhale-windows-arm64\.zip|https://github.com/Hmbown/CodeWhale/releases/download/v${version}/codewhale-windows-arm64.zip|g" "$file"
  sed -i.bak -E "s|https://github.com/Hmbown/CodeWhale/releases/download/v[0-9.]+/codewhale-windows-arm64-portable\.zip|https://github.com/Hmbown/CodeWhale/releases/download/v${version}/codewhale-windows-arm64-portable.zip|g" "$file"
  sed -i.bak -E "s/^    ReleaseDate:.*/    ReleaseDate: ${today}/g" "$file"
  rm -f "$file.bak"

  # Patch SHAs in order of appearance: CodeWhaleSetup.exe, x64 zip, x64 portable, arm64 zip, arm64 portable.
  # Use awk for ordered replacement to avoid sed ambiguity with identical sha lengths.
  local tmp
  tmp="$(mktemp)"
  awk -v a="$installer_sha" -v b="$x64_sha" -v c="$x64_portable_sha" -v d="$arm64_sha" -v e="$arm64_portable_sha" '
    BEGIN { n=0 }
    /InstallerSha256:/ {
      n++
      if (n==1) sub(/InstallerSha256:.*/, "InstallerSha256: " a)
      else if (n==2) sub(/InstallerSha256:.*/, "InstallerSha256: " b)
      else if (n==3) sub(/InstallerSha256:.*/, "InstallerSha256: " c)
      else if (n==4) sub(/InstallerSha256:.*/, "InstallerSha256: " d)
      else if (n==5) sub(/InstallerSha256:.*/, "InstallerSha256: " e)
    }
    { print }
  ' "$file" > "$tmp"
  mv "$tmp" "$file"
}

bump_manifest "$winget_primary"
# The README promises a verbatim structural mirror. Generate the canonical
# manifest once, then copy those exact bytes so comments and optional fields
# cannot drift while versions and hashes still look current.
cp "$winget_primary" "$winget_mirror"

echo "winget manifests bumped to v$version ($today)"
echo "  CodeWhaleSetup.exe: $installer_sha"
echo "  codewhale-windows-x64.zip: $x64_sha"
echo "  codewhale-windows-x64-portable.zip: $x64_portable_sha"
echo "  codewhale-windows-arm64.zip: $arm64_sha"
echo "  codewhale-windows-arm64-portable.zip: $arm64_portable_sha"
echo "Validate: winget validate --manifest $winget_primary"
