#!/usr/bin/env bash
# Bump every version-bearing file for a release in one shot.
#
# Usage: ./scripts/release/prepare-release.sh <new-version>
#
# Touches: Cargo.toml (workspace version), crates/*/Cargo.toml (internal
# codewhale-* dependency pins), npm/codewhale/package.json (version +
# codewhaleBinaryVersion), README*.md install-tag examples, Cargo.lock,
# crates/tui/CHANGELOG.md (via sync-changelog.sh) and
# web/lib/facts.generated.ts (via derive-facts.mjs).
#
# It does NOT write the CHANGELOG entry — add the `## [X.Y.Z] - YYYY-MM-DD`
# section first (see docs/RELEASE_CHECKLIST.md), then run this script, then
# let check-versions.sh (run at the end here) confirm everything agrees.
set -euo pipefail

new="${1:?usage: $0 <new-version>}"
if ! [[ "${new}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "error: '${new}' is not a plain X.Y.Z version" >&2
  exit 1
fi

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${repo}"

old="$(grep -E '^version = "' Cargo.toml | head -n1 | sed -E 's/^version = "([^"]+)".*/\1/')"
if [[ "${old}" == "${new}" ]]; then
  echo "workspace is already at ${new}; nothing to bump"
  exit 0
fi
echo "Bumping ${old} -> ${new}"

if ! grep -q "^## \[${new}\]" CHANGELOG.md; then
  echo "warning: CHANGELOG.md has no '## [${new}]' entry yet — add it before tagging" >&2
fi

OLD_VERSION="${old}" NEW_VERSION="${new}" python3 - <<'PY'
import os, pathlib, re, sys

old, new = os.environ["OLD_VERSION"], os.environ["NEW_VERSION"]
old_re = re.escape(old)

def bump(path, pattern, repl, minimum):
    p = pathlib.Path(path)
    text = p.read_text()
    out, n = re.subn(pattern, repl, text, flags=re.MULTILINE)
    if n < minimum:
        sys.exit(f"error: expected >= {minimum} replacement(s) in {path}, made {n}")
    p.write_text(out)
    print(f"  {path}: {n} replacement(s)")

# 1) Workspace version.
bump("Cargo.toml", rf'^version = "{old_re}"$', f'version = "{new}"', 1)

# 2) Internal codewhale-* dependency pins in every crate manifest.
total = 0
for manifest in sorted(pathlib.Path("crates").glob("*/Cargo.toml")):
    text = manifest.read_text()
    out, n = re.subn(
        rf'(codewhale-[a-z0-9-]+\s*=\s*\{{[^}}]*version = "){old_re}(")',
        rf"\g<1>{new}\g<2>",
        text,
    )
    if n:
        manifest.write_text(out)
        print(f"  {manifest}: {n} pin(s)")
        total += n
if total == 0:
    sys.exit("error: no internal dependency pins were bumped — wrong old version?")

# 3) npm wrapper.
bump(
    "npm/codewhale/package.json",
    rf'("(?:version|codewhaleBinaryVersion)": "){old_re}(")',
    rf"\g<1>{new}\g<2>",
    2,
)

# 4) README install-tag examples (all translations).
for readme in ["README.md", "README.zh-CN.md", "README.ja-JP.md", "README.vi.md"]:
    bump(readme, rf"--tag v{old_re}\b", f"--tag v{new}", 1)
PY

echo "Refreshing Cargo.lock..."
cargo update --workspace --offline >/dev/null

echo "Regenerating crates/tui/CHANGELOG.md slice..."
./scripts/sync-changelog.sh

echo "Regenerating web/lib/facts.generated.ts..."
node web/scripts/derive-facts.mjs

echo "Validating..."
./scripts/release/check-versions.sh
echo "Done. Review 'git diff', commit, and follow docs/RELEASE_CHECKLIST.md."
