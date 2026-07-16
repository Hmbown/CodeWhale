#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "${tmp_dir}"' EXIT

remote="${tmp_dir}/remote.git"
checkout="${tmp_dir}/checkout"
git init --bare --quiet "${remote}"
# GitHub's versions job intentionally uses a depth-1 checkout. A local clone
# of that checkout remains shallow, so this disposable remote must accept the
# fixture's tag without requiring history that the test never inspects.
git -C "${remote}" config receive.shallowUpdate true
git clone --quiet --no-hardlinks "${repo_root}" "${checkout}"
git -C "${checkout}" remote set-url origin "${remote}"
git -C "${checkout}" config user.name "Release Test"
git -C "${checkout}" config user.email "release-test@example.invalid"

release_sha="$(git -C "${checkout}" rev-parse HEAD)"
version="$(grep -E '^version = "' "${checkout}/Cargo.toml" | head -n1 | sed -E 's/^version = "([^"]+)".*/\1/')"
tag="v${version}"
git -C "${checkout}" tag "${tag}"
git -C "${checkout}" push --quiet origin "refs/tags/${tag}"

"${checkout}/scripts/release/require-release-tag-checkout.sh" "${version}"

printf 'dirty\n' >> "${checkout}/README.md"
if "${checkout}/scripts/release/require-release-tag-checkout.sh" \
  "${version}" >/dev/null 2>&1; then
  echo "dirty checkout unexpectedly passed" >&2
  exit 1
fi
git -C "${checkout}" restore README.md

git -C "${checkout}" commit --quiet --allow-empty -m branch-ahead
if "${checkout}/scripts/release/require-release-tag-checkout.sh" \
  "${version}" >/dev/null 2>&1; then
  echo "branch-ahead checkout unexpectedly passed" >&2
  exit 1
fi

git -C "${checkout}" push --quiet --force origin "HEAD:refs/tags/${tag}"
git -C "${checkout}" checkout --quiet --detach "${release_sha}"
if "${checkout}/scripts/release/require-release-tag-checkout.sh" \
  "${version}" >/dev/null 2>&1; then
  echo "remote-moved tag unexpectedly passed" >&2
  exit 1
fi

echo "require-release-tag-checkout tests passed"
