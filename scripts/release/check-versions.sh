#!/usr/bin/env bash
# Fails CI if version state is inconsistent across the workspace, npm
# wrapper, and Cargo.lock. Run on every push/PR so silent drift can't ship.
#
# Checks performed:
#   1. No `crates/*/Cargo.toml` carries a literal `version = "x.y.z"`; every
#      crate must inherit `version.workspace = true`.
#   2. `npm/codewhale/package.json` `version` matches the workspace
#      `version` in the root `Cargo.toml`. (`npm/deepseek-tui/` still
#      exists only as an unpublished compatibility notice and must stay
#      private.)
#   3. Internal `codewhale-*` path dependency pins match the workspace version.
#   4. The TUI crate's packaged changelog copy matches root `CHANGELOG.md`.
#   5. The current release has a dated Keep a Changelog entry and compare link.
#   6. README contributor additions are mentioned in the current release entry.
#   7. `SECURITY.md` keeps the dedicated security contact.
#   8. `codewhale-app-server` stays library-only; the shipped app-server
#      entrypoint belongs to `codewhale-cli`.
#   9. `Cargo.lock` is in sync with the manifests (`cargo metadata --locked`
#      fails if not).
set -euo pipefail

cd "$(dirname "$0")/../.."

fail=0

# 1) Literal versions in crate manifests.
literals="$(grep -nE '^version = "' crates/*/Cargo.toml || true)"
if [[ -n "${literals}" ]]; then
  echo "::error::Crate manifests must use 'version.workspace = true', not literal versions:" >&2
  echo "${literals}" >&2
  fail=1
fi

# 2) Workspace ↔ npm package.json.
workspace_version="$(grep -E '^version = "' Cargo.toml | head -n1 | sed -E 's/^version = "([^"]+)".*/\1/')"
npm_version="$(node -p "require('./npm/codewhale/package.json').version")"
if [[ "${workspace_version}" != "${npm_version}" ]]; then
  echo "::error::npm/codewhale/package.json version (${npm_version}) does not match workspace Cargo.toml (${workspace_version})." >&2
  fail=1
fi
if [[ -f npm/deepseek-tui/package.json ]]; then
  legacy_private="$(node -p "Boolean(require('./npm/deepseek-tui/package.json').private)")"
  legacy_publish_config="$(node -p "Boolean(require('./npm/deepseek-tui/package.json').publishConfig)")"
  if [[ "${legacy_private}" != "true" ]]; then
    echo "::error::npm/deepseek-tui/package.json must stay private so the legacy package is not republished." >&2
    fail=1
  fi
  if [[ "${legacy_publish_config}" == "true" ]]; then
    echo "::error::npm/deepseek-tui/package.json must not define publishConfig; the legacy package is deprecated." >&2
    fail=1
  fi
fi

# 3) Internal path dependency pins.
internal_dep_drift="$(
  grep -nE 'codewhale-[a-z-]+[[:space:]]*=[[:space:]]*\{[^}]*version[[:space:]]*=[[:space:]]*"' crates/*/Cargo.toml \
    | grep -v "version[[:space:]]*=[[:space:]]*\"${workspace_version}\"" || true
)"
if [[ -n "${internal_dep_drift}" ]]; then
  echo "::error::Internal codewhale-* path dependency versions must match workspace version ${workspace_version}:" >&2
  echo "${internal_dep_drift}" >&2
  fail=1
fi

# 4) Packaged TUI changelog slice (recent releases embedded in the binary).
if ! ./scripts/sync-changelog.sh --check >/dev/null 2>&1; then
  echo "::error::crates/tui/CHANGELOG.md is out of date with the root CHANGELOG.md slice." >&2
  echo "Run: ./scripts/sync-changelog.sh" >&2
  fail=1
fi

# 5) Current release-note shape.
current_section="$(
  awk -v version="${workspace_version}" '
    index($0, "## [" version "] - ") == 1 { in_section = 1; print; next }
    in_section && /^## \[/ { exit }
    in_section { print }
  ' CHANGELOG.md
)"
if [[ -z "${current_section}" ]]; then
  echo "::error::CHANGELOG.md must contain a section for ${workspace_version}." >&2
  fail=1
else
  if ! grep -qE "^## \\[${workspace_version}\\] - [0-9]{4}-[0-9]{2}-[0-9]{2}$" <<<"${current_section}"; then
    echo "::error::CHANGELOG.md section ${workspace_version} must use '## [${workspace_version}] - YYYY-MM-DD'." >&2
    fail=1
  fi
  if ! grep -qE "^### (Added|Changed|Deprecated|Removed|Fixed|Security)$" <<<"${current_section}"; then
    echo "::error::CHANGELOG.md section ${workspace_version} must contain at least one Keep a Changelog subsection." >&2
    fail=1
  fi
fi

compare_line="$(grep -E "^\\[${workspace_version}\\]: " CHANGELOG.md || true)"
if [[ -z "${compare_line}" ]]; then
  echo "::error::CHANGELOG.md must include a compare link for ${workspace_version}." >&2
  fail=1
fi

unreleased_section="$(
  awk '
    index($0, "## [Unreleased]") == 1 { in_section = 1; print; next }
    in_section && /^## \[/ { exit }
    in_section { print }
  ' CHANGELOG.md
)"
credit_sections="${current_section}
${unreleased_section}"

# 6) Contributor-credit cross-check for README additions on the release branch.
# This cannot prove every external PR author has been credited, but it does
# catch the common release-polish failure mode: adding a README contributor row
# without mentioning that credit/correction in the current release entry. While
# a release branch is still unbumped, `[Unreleased]` is also a valid credit
# surface.
previous_tag=""
current_tag="v${workspace_version}"
if [[ "${compare_line}" =~ compare/(v[0-9]+\.[0-9]+\.[0-9]+)\.\.\.${current_tag} ]]; then
  previous_tag="${BASH_REMATCH[1]}"
fi
if [[ -n "${previous_tag}" ]]; then
  if ! git rev-parse -q --verify "refs/tags/${previous_tag}" >/dev/null; then
    git fetch --quiet --depth=1 origin "refs/tags/${previous_tag}:refs/tags/${previous_tag}" || true
  fi
  if git rev-parse -q --verify "refs/tags/${previous_tag}" >/dev/null; then
    while IFS= read -r line; do
      [[ -z "${line}" ]] && continue
      handle="$(sed -E 's#.*github.com/([^)/]+).*#\1#' <<<"${line}")"
      if [[ -n "${handle}" && "${handle}" != "${line}" ]]; then
        if ! grep -Fq "github.com/${handle}" <<<"${credit_sections}" && ! grep -Fq "@${handle}" <<<"${credit_sections}"; then
          echo "::error::README.md adds contributor @${handle}, but CHANGELOG.md ${workspace_version} or [Unreleased] does not mention that credit." >&2
          fail=1
        fi
      fi
    done < <(
      git diff "${previous_tag}..HEAD" -- README.md \
        | grep -E '^\+[-*] \*\*\[[^]]+\]\(https://github.com/[^)]+\)\*\*' || true
    )
  fi
fi

# 7) Security contact guard.
security_email="hmbown@gmail.com"
if ! grep -qF "${security_email}" SECURITY.md; then
  echo "::error::SECURITY.md must list ${security_email} as the security contact." >&2
  fail=1
fi
if grep -qF "hmbown.dev@gmail.com" SECURITY.md; then
  echo "::error::SECURITY.md must not use the alternate personal fallback email; use ${security_email}." >&2
  fail=1
fi

# 8) Generated web facts carry the workspace version. The file is ignored and
# generated during web builds, so a clean CI checkout must derive it before this
# release guard can inspect it.
if [[ ! -f web/lib/facts.generated.ts ]]; then
  node web/scripts/derive-facts.mjs
fi
facts_version="$(grep -oE '"version": "[0-9]+\.[0-9]+\.[0-9]+"' web/lib/facts.generated.ts | head -n1 | sed -E 's/.*"([0-9.]+)".*/\1/')"
if [[ "${facts_version}" != "${workspace_version}" ]]; then
  node web/scripts/derive-facts.mjs
  facts_version="$(grep -oE '"version": "[0-9]+\.[0-9]+\.[0-9]+"' web/lib/facts.generated.ts | head -n1 | sed -E 's/.*"([0-9.]+)".*/\1/')"
  if [[ "${facts_version}" != "${workspace_version}" ]]; then
    echo "::error::web/lib/facts.generated.ts version (${facts_version}) does not match workspace (${workspace_version}). Run: node web/scripts/derive-facts.mjs" >&2
    fail=1
  fi
fi

# 9) README install-tag examples point at the current release.
for readme in README.md README.zh-CN.md README.ja-JP.md README.vi.md README.ko-KR.md; do
  stale_tags="$(grep -nE -- "--tag v[0-9]+\.[0-9]+\.[0-9]+" "${readme}" | grep -v -- "--tag v${workspace_version}" || true)"
  if [[ -n "${stale_tags}" ]]; then
    echo "::error::${readme} has install examples pinned to an old tag (want v${workspace_version}):" >&2
    echo "${stale_tags}" >&2
    fail=1
  fi
done

# 9b) Public install/version snippets stay on the current release (#3767).
# `codewhale --version   # X.Y.Z` verify-your-install lines across README
# locales and docs/INSTALL.md, plus the docs/INSTALL.md npm-wrapper publish
# pointer ("published at vX.Y.Z"). These drifted while this gate still passed
# on a prior lane, so guard them explicitly. Narrowly scoped to those two
# snippet shapes to avoid flagging unrelated prose.
for doc in README.md README.zh-CN.md README.ja-JP.md README.vi.md README.ko-KR.md docs/INSTALL.md; do
  [[ -f "${doc}" ]] || continue
  stale_version_comments="$(grep -nE -- "codewhale --version[[:space:]]+#[[:space:]]*[0-9]+\.[0-9]+\.[0-9]+" "${doc}" | grep -vE -- "#[[:space:]]*${workspace_version}([^0-9]|$)" || true)"
  if [[ -n "${stale_version_comments}" ]]; then
    echo "::error::${doc} has 'codewhale --version # X' snippet(s) not on ${workspace_version}:" >&2
    echo "${stale_version_comments}" >&2
    fail=1
  fi
done

stale_wrapper_pointer="$(grep -nE -- "wrapper is published at" docs/INSTALL.md | grep -E -- "v[0-9]+\.[0-9]+\.[0-9]+" | grep -v -- "v${workspace_version}" || true)"
# The publish pointer can wrap onto the next line; also scan the line after the lead-in.
wrapper_pointer_version="$(grep -A1 -E -- "wrapper is published at" docs/INSTALL.md | grep -oE -- "v[0-9]+\.[0-9]+\.[0-9]+" | head -n1 || true)"
if [[ -n "${wrapper_pointer_version}" && "${wrapper_pointer_version}" != "v${workspace_version}" ]]; then
  echo "::error::docs/INSTALL.md npm-wrapper publish pointer is ${wrapper_pointer_version}, want v${workspace_version}." >&2
  fail=1
fi

# 10) App-server is not a standalone binary.
app_server_bins="$(
  cargo metadata --locked --format-version 1 --no-deps \
    | node -e '
const fs = require("fs");
const metadata = JSON.parse(fs.readFileSync(0, "utf8"));
const pkg = metadata.packages.find((p) => p.name === "codewhale-app-server");
if (!pkg) {
  process.exit(2);
}
const bins = pkg.targets
  .filter((target) => target.kind.includes("bin"))
  .map((target) => target.name);
process.stdout.write(bins.join("\n"));
'
)"
if [[ -n "${app_server_bins}" ]]; then
  echo "::error::codewhale-app-server must stay library-only; use the codewhale-cli-owned 'codewhale app-server' entrypoint instead. Unexpected binary target(s):" >&2
  echo "${app_server_bins}" >&2
  fail=1
fi

# 11) Cargo.lock in sync.
if ! cargo metadata --locked --format-version 1 --no-deps >/dev/null 2>&1; then
  echo "::error::Cargo.lock is out of sync with the manifests. Run 'cargo update -p codewhale-tui' or 'cargo build' and commit the result." >&2
  fail=1
fi

if [[ "${fail}" -eq 0 ]]; then
  echo "Version state OK: workspace=${workspace_version}, npm=${npm_version}, lockfile in sync."
fi

exit "${fail}"
