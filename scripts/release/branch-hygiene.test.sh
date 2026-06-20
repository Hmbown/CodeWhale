#!/usr/bin/env bash
# Hermetic test for scripts/release/branch-hygiene.sh.
#
# Builds a throwaway git repo with a known branch topology and asserts that the
# hygiene script:
#   * marks branches whose tip is contained in main/the release branch as
#     "safe to delete",
#   * keeps a branch with unique commits from a non-Hunter contributor as
#     contributor work (never a safe delete),
#   * flags a branch with only unmerged maintainer commits as needs-review,
#   * detects a working checkout parked on an already-merged scratch branch,
#   * honors --remote when the canonical release refs live outside origin,
#   * actually deletes only safe branches under --prune --yes and never the
#     contributor branch.
#
# Run: bash scripts/release/branch-hygiene.test.sh
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
hygiene="${script_dir}/branch-hygiene.sh"

work="$(mktemp -d)"
cleanup() { rm -rf "${work}"; }
trap cleanup EXIT

fail=0
check() {
  # check <description> <expected-substring> <<<haystack-on-stdin>
  local desc="$1" needle="$2" hay
  hay="$(cat)"
  if grep -qF -- "${needle}" <<<"${hay}"; then
    echo "ok   - ${desc}"
  else
    echo "FAIL - ${desc}"
    echo "       expected to find: ${needle}"
    echo "------ output ------"
    echo "${hay}"
    echo "--------------------"
    fail=1
  fi
}
refute() {
  # refute <description> <forbidden-substring> <<<haystack>
  local desc="$1" needle="$2" hay
  hay="$(cat)"
  if grep -qF -- "${needle}" <<<"${hay}"; then
    echo "FAIL - ${desc}"
    echo "       did NOT expect to find: ${needle}"
    fail=1
  else
    echo "ok   - ${desc}"
  fi
}

# The script resolves its repo root as <script>/../.. and operates on *that*
# repo, not the current directory. So copy it into the throwaway repo at the
# same relative path and invoke the copy; that makes the temp repo its root.
mkdir -p "${work}/scripts/release"
cp "${hygiene}" "${work}/scripts/release/branch-hygiene.sh"
hygiene="${work}/scripts/release/branch-hygiene.sh"

cd "${work}"
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null
git init -q -b main .
git config user.name "Hunter Bown"
git config user.email "hmbown@gmail.com"
# Mirror the real repo's .mailmap canonicalization for Hunter.
cat >.mailmap <<'EOF'
Hunter Bown <hmbown@gmail.com> Claude <noreply@anthropic.com>
EOF

commit() {
  # commit <file> <content> <author-name> <author-email>
  echo "$2" >"$1"
  git add -A
  git -c user.name="$3" -c user.email="$4" \
    commit -q --author="$3 <$4>" -m "touch $1"
}

H_NAME="Hunter Bown"; H_EMAIL="hmbown@gmail.com"

# main: base commit by Hunter.
commit base "v0" "${H_NAME}" "${H_EMAIL}"

# release branch sits at main for this test.
git branch codex/v0.8.61 main

# merged-scratch: branched and merged back into main (tip contained in main).
git switch -q -c merged-scratch
commit feat-a "a" "${H_NAME}" "${H_EMAIL}"
git switch -q main
git merge -q --no-ff merged-scratch -m "merge merged-scratch"
# fast-forward the release branch to include the merge too.
git branch -f codex/v0.8.61 main

# contributor-branch: unique commit by a NON-Hunter contributor (must be kept).
git switch -q -c contributor-branch main
commit feat-contrib "c" "Jane Contributor" "jane@example.com"

# maintainer-scratch: unique commit by Hunter, not merged (needs review).
git switch -q -c maintainer-scratch main
commit feat-h "h" "${H_NAME}" "${H_EMAIL}"

# bot-folded: unique commit by Claude, which .mailmap folds into Hunter, so it
# must be treated as maintainer-only (needs review, NOT contributor work).
git switch -q -c bot-folded main
commit feat-bot "b" "Claude" "noreply@anthropic.com"

# Park the working checkout on an already-merged scratch branch to exercise the
# "parked checkout" warning. Point HEAD at the merged-scratch tip but on a
# fresh non-release branch name.
git switch -q -c renovate/parked merged-scratch

# --- Dry-run report ----------------------------------------------------------
report="$(bash "${hygiene}" --release-branch codex/v0.8.61 --main-ref main 2>&1)"

check "merged scratch branch is a safe delete" \
  "local : merged-scratch" <<<"${report}"
check "contributor branch is kept as contributor work" \
  "[local] contributor-branch:" <<<"${report}"
check "contributor branch names the contributor author" \
  "Jane Contributor" <<<"${report}"
check "contributor branch reason is KEEP" \
  "KEEP - unique contributor work" <<<"${report}"
check "maintainer-only scratch is flagged for review" \
  "[local] maintainer-scratch:" <<<"${report}"
check "maintainer-only scratch reason is REVIEW" \
  "REVIEW -" <<<"${report}"
check "mailmap-folded bot commit is treated as maintainer (review, not keep)" \
  "[local] bot-folded:" <<<"${report}"
check "parked working checkout warning fires" \
  "working checkout is parked on 'renovate/parked'" <<<"${report}"

# The contributor branch must NEVER appear in the safe-delete list.
safe_section="$(awk '/^-- Safe to delete/{f=1;next} /^-- Keep/{f=0} f' <<<"${report}")"
refute "contributor branch is not in the safe-delete list" \
  "contributor-branch" <<<"${safe_section}"
refute "maintainer-only scratch is not in the safe-delete list" \
  "maintainer-scratch" <<<"${safe_section}"

# --- Prune (local) -----------------------------------------------------------
prune_out="$(bash "${hygiene}" --release-branch codex/v0.8.61 --main-ref main --prune --yes 2>&1)"
check "prune deletes the merged scratch branch" \
  "deleted local merged-scratch" <<<"${prune_out}"

# After prune: contributor + maintainer + bot branches still exist; merged one
# is gone.
remaining="$(git for-each-ref --format='%(refname:short)' refs/heads/)"
check "contributor branch survives prune" "contributor-branch" <<<"${remaining}"
check "maintainer-only scratch survives prune" "maintainer-scratch" <<<"${remaining}"
refute "merged scratch branch is gone after prune" "merged-scratch" <<<"${remaining}"

# --- Custom remote name ------------------------------------------------------
git switch -q main
git switch -q -c remote-main-tip
commit remote-main-only "remote" "${H_NAME}" "${H_EMAIL}"
git branch "upstream/main" main^1
git update-ref "refs/remotes/upstream/main" "$(git rev-parse remote-main-tip)"
git update-ref "refs/remotes/upstream/codex/v0.8.61" "$(git rev-parse codex/v0.8.61)"
git update-ref "refs/remotes/upstream/merged-remote" "$(git rev-parse main)"
git update-ref "refs/remotes/upstream/remote-main-only" "$(git rev-parse remote-main-tip)"
upstream_report="$(bash "${hygiene}" --remote upstream --release-branch codex/v0.8.61 2>&1)"
check "custom remote release tip is reported" \
  "upstream" <<<"${upstream_report}"
check "custom remote default main ref is fully qualified" \
  "Main ref         : refs/remotes/upstream/main" <<<"${upstream_report}"
check "custom remote safe-delete command uses the selected remote" \
  "remote: upstream/merged-remote" <<<"${upstream_report}"
check "custom remote main ref is not confused with a same-named local branch" \
  "remote: upstream/remote-main-only" <<<"${upstream_report}"
refute "custom remote report does not hard-code origin in safe deletes" \
  "remote: origin/merged-remote" <<<"${upstream_report}"

# --- State inconsistency: diverged local vs remote release branch ------------
git switch -q main
# Simulate a remote release branch that has diverged from local.
git update-ref "refs/remotes/origin/codex/v0.8.61" "$(git rev-parse maintainer-scratch)"
git branch -f codex/v0.8.61 bot-folded
set +e
diverged_out="$(bash "${hygiene}" --release-branch codex/v0.8.61 --main-ref main 2>&1)"
diverged_ec=$?
set -e
check "divergence between local and remote release branch is reported" \
  "have DIVERGED" <<<"${diverged_out}"
if [[ "${diverged_ec}" -ne 1 ]]; then
  echo "FAIL - diverged state should exit 1, got ${diverged_ec}"
  fail=1
else
  echo "ok   - diverged state exits non-zero"
fi

echo
if [[ "${fail}" -eq 0 ]]; then
  echo "branch-hygiene.test.sh: all checks passed"
else
  echo "branch-hygiene.test.sh: FAILURES above"
fi
exit "${fail}"
