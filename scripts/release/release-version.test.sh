#!/usr/bin/env bash
# Contract tests for the shared release-version grammar.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=scripts/release/release-version.sh
source "${repo_root}/scripts/release/release-version.sh"

fail=0
expect_valid() {
  if ! release_version_is_valid "$1"; then
    echo "expected valid version: $1" >&2
    fail=1
  fi
}
expect_invalid() {
  if release_version_is_valid "$1"; then
    echo "expected invalid version: $1" >&2
    fail=1
  fi
}
expect_tag_valid() {
  if ! release_tag_is_valid "$1"; then
    echo "expected valid tag: $1" >&2
    fail=1
  fi
}
expect_tag_invalid() {
  if release_tag_is_valid "$1"; then
    echo "expected invalid tag: $1" >&2
    fail=1
  fi
}
expect_prerelease() {
  if ! release_version_is_prerelease "$1"; then
    echo "expected prerelease: $1" >&2
    fail=1
  fi
}
expect_stable() {
  if release_version_is_prerelease "$1"; then
    echo "expected stable (not prerelease): $1" >&2
    fail=1
  fi
}
expect_dist_tag() {
  local actual
  actual="$(release_version_npm_dist_tag "$1")"
  if [[ "${actual}" != "$2" ]]; then
    echo "expected npm dist-tag $2 for $1, got ${actual}" >&2
    fail=1
  fi
}

# Stable releases.
expect_valid 0.9.12
expect_valid 1.0.0
expect_valid 10.20.30
expect_stable 0.9.12
expect_dist_tag 0.9.12 latest

# Release candidates.
expect_valid 0.9.12-rc.1
expect_valid 0.9.12-rc.12
expect_prerelease 0.9.12-rc.1
expect_dist_tag 0.9.12-rc.1 next

# Refused shapes: anything that is not exactly X.Y.Z or X.Y.Z-rc.N.
expect_invalid ""
expect_invalid v0.9.12
expect_invalid 0.9
expect_invalid 0.9.12.1
expect_invalid 0.9.12-rc
expect_invalid 0.9.12-rc.
expect_invalid 0.9.12-rc.0
expect_invalid 0.9.12-rc.01
expect_invalid 0.9.12-rc1
expect_invalid 0.9.12-beta.1
expect_invalid 0.9.12-alpha
expect_invalid 0.9.12+build.5
expect_invalid 0.9.12-rc.1+build.5
expect_invalid ' 0.9.12'
expect_invalid '0.9.12 '
expect_invalid main
expect_stable 0.9.12-beta.1   # invalid shapes are never reported as prerelease

# Tags carry a mandatory leading v.
expect_tag_valid v0.9.12
expect_tag_valid v0.9.12-rc.3
expect_tag_invalid 0.9.12
expect_tag_invalid v0.9.12-rc
expect_tag_invalid v0.9.12-rc.0
expect_tag_invalid v0.9.12-beta.1
expect_tag_invalid main

# The grep form extracts the whole version, suffix included, and nothing more.
# shellcheck disable=SC2016  # the literal ${RELEASE_TAG:-...} text is the fixture
extracted="$(printf 'RELEASE_TAG="${RELEASE_TAG:-v0.9.12-rc.4}"\n' | grep -oE "v${RELEASE_VERSION_GREP}" | head -n1)"
if [[ "${extracted}" != "v0.9.12-rc.4" ]]; then
  echo "grep form extracted '${extracted}', want v0.9.12-rc.4" >&2
  fail=1
fi
extracted="$(printf '"version": "0.9.12"\n' | grep -oE "\"version\": \"${RELEASE_VERSION_GREP}\"" | head -n1)"
if [[ "${extracted}" != '"version": "0.9.12"' ]]; then
  echo "grep form extracted '${extracted}', want \"version\": \"0.9.12\"" >&2
  fail=1
fi

if [[ "${fail}" -ne 0 ]]; then
  exit 1
fi
echo "release-version grammar tests passed"
