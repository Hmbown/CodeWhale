#!/usr/bin/env bash
# Shared release-version grammar for the publish chain. Source this file from
# bash; it defines patterns and predicates and never runs anything itself.
#
# Accepted shapes:
#   X.Y.Z        stable release      -> GitHub Release, npm `latest`, GHCR
#                                       `latest`, Homebrew tap, crates.io
#   X.Y.Z-rc.N   release candidate   -> GitHub *prerelease*, npm dist-tag
#                (N >= 1)               `next`, GHCR without `latest`, no
#                                       Homebrew tap update, crates.io
#
# A release candidate rides the same tag/asset/registry chain as a stable
# release so the chain itself is exercised, but never moves a stable channel
# pointer. Anything other than these two shapes is refused everywhere.
#
# JavaScript consumers (scripts/release/ensure-release-assets-absent.js,
# web/scripts/check-docs.mjs) carry the same grammar inline; keep them in step
# with RELEASE_VERSION_GREP below.

# Anchored ERE forms for `[[ =~ ]]`.
RELEASE_VERSION_PATTERN='^[0-9]+\.[0-9]+\.[0-9]+(-rc\.[1-9][0-9]*)?$'
RELEASE_TAG_PATTERN='^v[0-9]+\.[0-9]+\.[0-9]+(-rc\.[1-9][0-9]*)?$'
# Unanchored ERE form for `grep -oE` / `grep -E` extraction (used by sourcing
# scripts, hence the unused-variable waiver).
# shellcheck disable=SC2034
RELEASE_VERSION_GREP='[0-9]+\.[0-9]+\.[0-9]+(-rc\.[1-9][0-9]*)?'

# release_version_is_valid X.Y.Z[-rc.N]
release_version_is_valid() {
  [[ "${1:-}" =~ ${RELEASE_VERSION_PATTERN} ]]
}

# release_tag_is_valid vX.Y.Z[-rc.N]
release_tag_is_valid() {
  [[ "${1:-}" =~ ${RELEASE_TAG_PATTERN} ]]
}

# release_version_is_prerelease VERSION  -> 0 only for a valid X.Y.Z-rc.N
release_version_is_prerelease() {
  release_version_is_valid "${1:-}" && [[ "${1}" == *-rc.* ]]
}

# release_version_npm_dist_tag VERSION -> `next` for a candidate, else `latest`
release_version_npm_dist_tag() {
  if release_version_is_prerelease "${1:-}"; then
    echo next
  else
    echo latest
  fi
}
