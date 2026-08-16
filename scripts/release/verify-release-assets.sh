#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"

usage() {
  cat <<'EOF'
usage: scripts/release/verify-release-assets.sh [--allow-npm-binary-mismatch] [VERSION]

Proves the public GitHub Release assets for VERSION were built from the same
tag commit that will be published to Cargo/npm.

Checks:
  - local tag vVERSION exists
  - remote tag vVERSION resolves to the same commit SHA
  - GitHub Release vVERSION exists
  - a successful asset-publishing job in a Release workflow run used that SHA
  - npm/codewhale release:check sees the fresh binary/archive/installer matrix
    and both required checksum manifests

Set GH_BIN=/path/to/gh to choose a GitHub CLI binary. Set
CODEWHALE_GITHUB_REPO=owner/repo or CODEWHALE_RELEASE_REMOTE=remote to override
the default Hmbown/CodeWhale origin check.
EOF
}

allow_npm_binary_mismatch=0
version=""

while (($# > 0)); do
  case "$1" in
    --allow-npm-binary-mismatch)
      allow_npm_binary_mismatch=1
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      if [[ -n "${version}" ]]; then
        usage >&2
        exit 2
      fi
      version="$1"
      ;;
  esac
  shift
done

cd "${repo_root}"

if [[ -z "${version}" ]]; then
  version="$(grep -E '^version = "' Cargo.toml | head -n1 | sed -E 's/^version = "([^"]+)".*/\1/')"
fi
version="${version#v}"
tag="v${version}"

if [[ -z "${version}" ]]; then
  echo "Could not determine release version." >&2
  exit 1
fi

repo="${CODEWHALE_GITHUB_REPO:-Hmbown/CodeWhale}"
remote="${CODEWHALE_RELEASE_REMOTE:-origin}"
gh_bin="${GH_BIN:-gh}"

if ! command -v "${gh_bin}" >/dev/null 2>&1; then
  echo "GitHub CLI not found: ${gh_bin}" >&2
  echo "Install gh or set GH_BIN=/path/to/gh." >&2
  exit 1
fi

local_sha="$(git rev-list -n 1 "${tag}" 2>/dev/null || true)"
if [[ -z "${local_sha}" ]]; then
  echo "Local tag ${tag} does not exist." >&2
  exit 1
fi

remote_sha="$(git ls-remote --tags "${remote}" "refs/tags/${tag}^{}" | awk 'NR == 1 {print $1}')"
if [[ -z "${remote_sha}" ]]; then
  remote_sha="$(git ls-remote --tags "${remote}" "refs/tags/${tag}" | awk 'NR == 1 {print $1}')"
fi
if [[ -z "${remote_sha}" ]]; then
  echo "Remote tag ${tag} does not exist on ${remote}." >&2
  exit 1
fi
if [[ "${local_sha}" != "${remote_sha}" ]]; then
  echo "Tag SHA mismatch for ${tag}:" >&2
  echo "  local : ${local_sha}" >&2
  echo "  remote: ${remote_sha}" >&2
  exit 1
fi
echo "Tag check OK: ${tag} -> ${local_sha}"

release_url="$("${gh_bin}" release view "${tag}" --repo "${repo}" --json url --jq '.url')"
if [[ -z "${release_url}" ]]; then
  echo "GitHub Release ${tag} was not found in ${repo}." >&2
  exit 1
fi
echo "GitHub Release OK: ${release_url}"

run_candidates="$(
  TAG_SHA="${local_sha}" "${gh_bin}" run list \
    --repo "${repo}" \
    --workflow "Release" \
    --limit 100 \
    --json databaseId,headSha,headBranch,event,conclusion,status,createdAt,updatedAt,url \
    --jq 'map(select(.headSha == env.TAG_SHA and (.event == "push" or .event == "workflow_dispatch"))) | sort_by(.updatedAt) | reverse | .[] | "\(.databaseId)\t\(.headBranch)\t\(.event)\t\(.url)"'
)"

run_summary=""
while IFS=$'\t' read -r run_id head_branch run_event run_url; do
  if [[ -z "${run_id}" ]]; then
    continue
  fi

  release_job_id="$(
    "${gh_bin}" run view "${run_id}" \
      --repo "${repo}" \
      --json jobs \
      --jq '.jobs[] | select(.name == "release" and .conclusion == "success") | .databaseId' \
      | head -n 1
  )"
  # Freshness vs the release job's own started_at (not the run-level
  # run_started_at, which job-level reruns bump past the uploads) is enforced
  # in npm/codewhale/scripts/verify-release-assets.js — see #5429. This check
  # only proves a successful release job exists for the tag SHA.
  if [[ -n "${release_job_id}" ]]; then
    run_summary="${run_id}\t${head_branch}\t${run_event}\t${run_url}\trelease job ${release_job_id}"
    break
  fi
done <<<"${run_candidates}"

if [[ -z "${run_summary}" ]]; then
  echo "No successful asset-publishing job found in the last 100 Release workflow runs for ${tag} at ${local_sha}." >&2
  echo "Rerun the Release workflow before publishing Cargo/npm." >&2
  exit 1
fi
printf 'Release asset job OK: %b\n' "${run_summary}"

npm_package_version="$(node -p "require('./npm/codewhale/package.json').version")"
npm_binary_version="$(
  node -p "const p=require('./npm/codewhale/package.json'); p.codewhaleBinaryVersion || p.deepseekBinaryVersion || p.version"
)"
if [[ "${npm_package_version}" != "${version}" ]]; then
  echo "npm/codewhale package version ${npm_package_version} does not match ${version}." >&2
  exit 1
fi
if [[ "${npm_binary_version}" != "${version}" && "${allow_npm_binary_mismatch}" != "1" ]]; then
  echo "npm/codewhale codewhaleBinaryVersion ${npm_binary_version} does not match ${version}." >&2
  echo "Use --allow-npm-binary-mismatch only for an intentional packaging-only npm release." >&2
  exit 1
fi

(
  cd npm/codewhale
  env \
    -u CODEWHALE_RELEASE_BASE_URL \
    -u DEEPSEEK_TUI_RELEASE_BASE_URL \
    -u DEEPSEEK_RELEASE_BASE_URL \
    -u CODEWHALE_USE_CNB_MIRROR \
    DEEPSEEK_TUI_VERSION="${version}" \
    DEEPSEEK_TUI_GITHUB_REPO="${repo}" \
    CODEWHALE_ALLOW_NPM_BINARY_MISMATCH="${allow_npm_binary_mismatch}" \
    npm run release:check
)

echo "Release asset gate OK: ${tag} assets match ${local_sha} and npm/codewhale is ready for publish."
