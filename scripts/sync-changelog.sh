#!/usr/bin/env sh
# Regenerate crates/tui/CHANGELOG.md from the workspace root CHANGELOG.md.
# The /change command embeds this file into the binary via include_str!, so
# it deliberately keeps only the most recent release sections.
#
# The `## [Unreleased]` section is always kept but does NOT count toward the
# keep window: it is not a release, and counting it silently dropped the
# oldest retained release whenever Unreleased had content (#3768). The window
# therefore tracks a stable number of *released* versions regardless of
# in-progress notes.
#
# Usage: scripts/sync-changelog.sh [--check] [sections-to-keep]
#   --check  verify crates/tui/CHANGELOG.md is up to date without writing
#            (exit 1 if regeneration would change it)
#   sections-to-keep defaults to 15 (released versions, excluding Unreleased)
set -eu
CHECK=0
if [ "${1:-}" = "--check" ]; then
  CHECK=1
  shift
fi
KEEP="${1:-15}"
root="$(cd "$(dirname "$0")/.." && pwd)"
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
awk -v keep="$KEEP" '
  /^\[/ && /\]: http/ { exit }
  # Count only released versions toward the keep window; the Unreleased
  # section is always printed but never consumes a slot (#3768).
  /^## \[/ && $0 !~ /\[Unreleased\]/ { count++ }
  count > keep { exit }
  { print }
' "$root/CHANGELOG.md" > "$tmp"
printf '%s\n' \
  '---' \
  '' \
  'Older releases: [CHANGELOG.md](https://github.com/Hmbown/CodeWhale/blob/main/CHANGELOG.md) and [docs/CHANGELOG_ARCHIVE.md](https://github.com/Hmbown/CodeWhale/blob/main/docs/CHANGELOG_ARCHIVE.md).' \
  >> "$tmp"
if [ "$CHECK" = 1 ]; then
  if cmp -s "$tmp" "$root/crates/tui/CHANGELOG.md"; then
    echo "crates/tui/CHANGELOG.md is up to date"
  else
    echo "crates/tui/CHANGELOG.md is out of date; run scripts/sync-changelog.sh" >&2
    exit 1
  fi
else
  cp "$tmp" "$root/crates/tui/CHANGELOG.md"
  echo "wrote crates/tui/CHANGELOG.md ($(wc -l < "$root/crates/tui/CHANGELOG.md") lines, $KEEP sections kept)"
fi
