#!/usr/bin/env bash
# Post-merge branch hygiene for release and scratch branches.
#
# After a release/integration merge it is easy to leave the working checkout
# parked on a stale feature branch (e.g. renovate/website-and-readmes) even
# though HEAD already matches main and the release tag. That creates release
# anxiety: contributors cannot tell whether their work actually landed. This
# script makes the current state obvious and recommends *safe* cleanup.
#
# It is read-only and dry-run by default. It never deletes anything unless you
# pass --prune, and even then it refuses to delete any branch that carries
# unique commits from a contributor other than Hunter unless that work is
# already contained in main/the release branch (i.e. merged).
#
# What it reports:
#   1. State check: current checkout branch, local + remote release branch
#      tips, and the configured main ref, and whether they agree after an integration
#      merge.
#   2. Safe deletes: local and remote branches whose tip is already contained
#      in the main ref or the release branch.
#   3. Keep/review: branches with unique commits, naming the branch, the
#      unique commit count, the contributor author(s), and the keep reason.
#      Non-Hunter contributor work is always a keep/review, never a safe
#      delete, unless it is already merged.
#   4. A summary line: deleted / kept-for-contributor / needs-human-decision.
#
# Usage:
#   scripts/release/branch-hygiene.sh [--release-branch BRANCH]
#                                     [--remote REMOTE]
#                                     [--main-ref REF]
#                                     [--maintainer "Name <email>"]...
#                                     [--prune] [--prune-remote] [--yes]
#
# Examples:
#   # Dry-run report against codex/v0.8.61 (default release branch is the
#   # current branch if it looks like a release branch, else codex/<latest>):
#   scripts/release/branch-hygiene.sh --release-branch codex/v0.8.61
#
#   # Actually delete the local safe-delete branches (still skips remote and
#   # still refuses unmerged contributor work):
#   scripts/release/branch-hygiene.sh --release-branch codex/v0.8.61 --prune --yes
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"

usage() {
  cat <<'EOF'
usage: scripts/release/branch-hygiene.sh [options]

Reports post-merge branch hygiene for release and scratch branches and
recommends safe cleanup. Read-only and dry-run by default.

Options:
  --release-branch BRANCH   Release branch to verify and prune against
                            (default: current branch if it matches
                            codex/* or work/*, else the highest codex/v* ref).
  --remote REMOTE           Remote whose release/scratch branches are checked
                            and pruned (default: origin).
  --main-ref REF            The "everything merged here" ref
                            (default: REMOTE/main, falling back to main).
  --maintainer "N <e>"      Treat this author as the maintainer (Hunter).
                            May be repeated. Defaults are derived from
                            .mailmap plus a built-in list.
  --prune                   Delete the local safe-delete branches.
  --prune-remote            Also delete the remote safe-delete branches
                            (implies --prune). Requires push access.
  --yes                     Do not prompt before deleting (for CI/automation).
  -h, --help                Show this help.

Exit status:
  0  state is consistent (or pruning succeeded)
  1  state is INCONSISTENT (tips disagree) or a delete failed
EOF
}

release_branch=""
remote_name="origin"
main_ref=""
prune=0
prune_remote=0
assume_yes=0
declare -a extra_maintainers=()

while (($# > 0)); do
  case "$1" in
    --release-branch)
      [[ $# -ge 2 ]] || { usage >&2; exit 2; }
      release_branch="$2"
      shift
      ;;
    --remote)
      [[ $# -ge 2 ]] || { usage >&2; exit 2; }
      remote_name="$2"
      shift
      ;;
    --main-ref)
      [[ $# -ge 2 ]] || { usage >&2; exit 2; }
      main_ref="$2"
      shift
      ;;
    --maintainer)
      [[ $# -ge 2 ]] || { usage >&2; exit 2; }
      extra_maintainers+=("$2")
      shift
      ;;
    --prune)
      prune=1
      ;;
    --prune-remote)
      prune=1
      prune_remote=1
      ;;
    --yes)
      assume_yes=1
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

cd "${repo_root}"

# --- Maintainer (Hunter) identity -------------------------------------------
# A branch is only ever a "safe delete" on author grounds if every unique
# commit on it is authored by the maintainer. Everyone else is contributor
# work that must be reviewed/merged/credited/preserved before deletion.
#
# We build the maintainer set from a small built-in list plus the canonical
# left-hand side of .mailmap (which already folds bots/Claude/Copilot into
# Hunter), plus any --maintainer overrides. We compare on email when present,
# otherwise on the lowercased name.
declare -a maintainer_emails=("hmbown@gmail.com" "hmbown.dev@gmail.com")
declare -a maintainer_names=("hunter bown" "hunter b")

if [[ -f .mailmap ]]; then
  while IFS= read -r line; do
    [[ -z "${line}" || "${line}" == \#* ]] && continue
    # Canonical identity is the first "Name <email>" on each mailmap line.
    if [[ "${line}" =~ ^([^<]+)\<([^>]+)\> ]]; then
      cname="$(echo "${BASH_REMATCH[1]}" | sed -E 's/[[:space:]]+$//' | tr '[:upper:]' '[:lower:]')"
      cemail="$(echo "${BASH_REMATCH[2]}" | tr '[:upper:]' '[:lower:]')"
      [[ -n "${cname}" ]] && maintainer_names+=("${cname}")
      [[ -n "${cemail}" ]] && maintainer_emails+=("${cemail}")
    fi
  done <.mailmap
fi

for m in "${extra_maintainers[@]+"${extra_maintainers[@]}"}"; do
  if [[ "${m}" =~ \<([^>]+)\> ]]; then
    maintainer_emails+=("$(echo "${BASH_REMATCH[1]}" | tr '[:upper:]' '[:lower:]')")
    mname="$(echo "${m%%<*}" | sed -E 's/[[:space:]]+$//' | tr '[:upper:]' '[:lower:]')"
    [[ -n "${mname}" ]] && maintainer_names+=("${mname}")
  else
    maintainer_names+=("$(echo "${m}" | tr '[:upper:]' '[:lower:]')")
  fi
done

is_maintainer() {
  # args: <author-name> <author-email> (both already lowercased)
  local an="$1" ae="$2" e n
  for e in "${maintainer_emails[@]}"; do
    [[ -n "${e}" && "${ae}" == "${e}" ]] && return 0
  done
  for n in "${maintainer_names[@]}"; do
    [[ -n "${n}" && "${an}" == "${n}" ]] && return 0
  done
  return 1
}

# --- Resolve main + release refs --------------------------------------------
looks_like_release_branch() {
  [[ "$1" == codex/* || "$1" == work/* || "$1" == release/* ]]
}

current_branch="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo "")"

if [[ -z "${main_ref}" ]]; then
  if git rev-parse -q --verify "${remote_name}/main" >/dev/null 2>&1; then
    main_ref="${remote_name}/main"
  else
    main_ref="main"
  fi
fi

if [[ -z "${release_branch}" ]]; then
  if [[ -n "${current_branch}" && "${current_branch}" != "HEAD" ]] && looks_like_release_branch "${current_branch}"; then
    release_branch="${current_branch}"
  else
    # Highest codex/vX.Y.Z local branch by version sort.
    release_branch="$(git for-each-ref --format='%(refname:short)' 'refs/heads/codex/v*' \
      | sort -V | tail -n1 || true)"
  fi
fi

if ! git rev-parse -q --verify "${main_ref}" >/dev/null 2>&1; then
  echo "::error::main ref '${main_ref}' does not exist." >&2
  exit 1
fi

main_sha="$(git rev-parse --short "${main_ref}")"

echo "== CodeWhale branch hygiene =="
echo "Current checkout : ${current_branch:-<detached>} ($(git rev-parse --short HEAD))"
echo "Main ref         : ${main_ref} (${main_sha})"

inconsistent=0

if [[ -z "${release_branch}" ]]; then
  echo "Release branch   : <none found> (pass --release-branch to enable the state check)"
else
  local_rel="refs/heads/${release_branch}"
  remote_rel="refs/remotes/${remote_name}/${release_branch}"

  if git rev-parse -q --verify "${local_rel}" >/dev/null 2>&1; then
    local_rel_sha="$(git rev-parse --short "${local_rel}")"
  else
    local_rel_sha="<missing>"
  fi
  if git rev-parse -q --verify "${remote_rel}" >/dev/null 2>&1; then
    remote_rel_sha="$(git rev-parse --short "${remote_rel}")"
  else
    remote_rel_sha="<missing>"
  fi

  echo "Release branch   : ${release_branch}"
  echo "  local          : ${local_rel_sha}"
  echo "  ${remote_name}         : ${remote_rel_sha}"

  # State verification: after an integration merge into the release branch,
  # local and remote release tips should agree, and the working checkout
  # should be on the release branch (not parked on a scratch/renovate name).
  if [[ "${local_rel_sha}" != "<missing>" && "${remote_rel_sha}" != "<missing>" \
        && "${local_rel_sha}" != "${remote_rel_sha}" ]]; then
    if git merge-base --is-ancestor "${local_rel}" "${remote_rel}" 2>/dev/null; then
      echo "  ::warning:: local ${release_branch} is BEHIND ${remote_name} - fast-forward with:" \
           "git fetch ${remote_name} && git branch -f ${release_branch} ${remote_rel}" >&2
    elif git merge-base --is-ancestor "${remote_rel}" "${local_rel}" 2>/dev/null; then
      echo "  ::warning:: local ${release_branch} is AHEAD of ${remote_name} - push with:" \
           "git push ${remote_name} ${release_branch}" >&2
    else
      echo "  ::error:: local and remote ${release_branch} have DIVERGED." >&2
      inconsistent=1
    fi
  fi

  if [[ -n "${current_branch}" && "${current_branch}" != "HEAD" \
        && "${current_branch}" != "${release_branch}" ]] \
     && ! looks_like_release_branch "${current_branch}"; then
    head_sha="$(git rev-parse HEAD)"
    if git merge-base --is-ancestor "${head_sha}" "${main_ref}" 2>/dev/null \
       || { [[ "${remote_rel_sha}" != "<missing>" ]] \
            && git merge-base --is-ancestor "${head_sha}" "${remote_rel}" 2>/dev/null; }; then
      echo "  ::warning:: working checkout is parked on '${current_branch}', whose HEAD is" \
           "already merged. Switch to the release branch: git switch ${release_branch}" >&2
    fi
  fi
fi

# Containment ref: a branch is "merged" if its tip is contained in main OR the
# release branch (prefer the remote release tip, then local, then just main).
declare -a contain_refs=("${main_ref}")
if [[ -n "${release_branch}" ]]; then
  if git rev-parse -q --verify "refs/remotes/${remote_name}/${release_branch}" >/dev/null 2>&1; then
    contain_refs+=("refs/remotes/${remote_name}/${release_branch}")
  fi
  if git rev-parse -q --verify "refs/heads/${release_branch}" >/dev/null 2>&1; then
    contain_refs+=("refs/heads/${release_branch}")
  fi
fi

is_contained() {
  # arg: <commit-ish> - contained in any containment ref?
  local tip="$1" ref
  for ref in "${contain_refs[@]}"; do
    if git merge-base --is-ancestor "${tip}" "${ref}" 2>/dev/null; then
      return 0
    fi
  done
  return 1
}

# unique_commits <branch-tip>: commits on the branch not in any containment
# ref. Uses the symmetric "not reachable from contain_refs" set.
declare -a not_args=()
for ref in "${contain_refs[@]}"; do
  not_args+=("^${ref}")
done

# --- Classify branches -------------------------------------------------------
# Branches we never touch automatically.
protected_re='^(main|master|HEAD)$'

declare -a safe_local=()
declare -a safe_remote=()
declare -a keep_report=()
needs_human=0
kept_contributor=0

classify_branch() {
  # args: <scope: local|remote> <short-name> <full-ref>
  local scope="$1" name="$2" ref="$3"

  # Skip protected and the active release branch / current checkout.
  [[ "${name}" =~ ${protected_re} ]] && return 0
  [[ -n "${release_branch}" && "${name}" == "${release_branch}" ]] && return 0
  [[ "${scope}" == "local" && "${name}" == "${current_branch}" ]] && return 0

  local tip
  tip="$(git rev-parse "${ref}" 2>/dev/null || echo "")"
  [[ -z "${tip}" ]] && return 0

  if is_contained "${tip}"; then
    if [[ "${scope}" == "local" ]]; then
      safe_local+=("${name}")
    else
      safe_remote+=("${name}")
    fi
    return 0
  fi

  # Has unique commits; inspect authors for the contributor-preservation
  # policy. Never auto-delete; always keep/review.
  local unique authors non_maint=0
  unique="$(git rev-list --count "${ref}" "${not_args[@]}" 2>/dev/null || echo 0)"
  [[ "${unique}" -eq 0 ]] && return 0

  # Distinct author "Name <email>" set on the unique commits.
  local authors_raw
  authors_raw="$(git log --format='%an|%ae' "${ref}" "${not_args[@]}" 2>/dev/null \
    | sort -u || true)"

  local display_authors=""
  while IFS='|' read -r an ae; do
    [[ -z "${an}${ae}" ]] && continue
    local anl ael
    anl="$(echo "${an}" | tr '[:upper:]' '[:lower:]')"
    ael="$(echo "${ae}" | tr '[:upper:]' '[:lower:]')"
    if ! is_maintainer "${anl}" "${ael}"; then
      non_maint=1
    fi
    display_authors+="${display_authors:+, }${an}"
  done <<<"${authors_raw}"

  local reason
  if [[ "${non_maint}" -eq 1 ]]; then
    reason="KEEP - unique contributor work (not yet merged). Review/merge/credit before deleting."
    kept_contributor=$((kept_contributor + 1))
  else
    reason="REVIEW - ${unique} unmerged maintainer commit(s); confirm intentionally abandoned before deleting."
    needs_human=$((needs_human + 1))
  fi
  keep_report+=("[${scope}] ${name}: ${unique} unique commit(s); authors: ${display_authors:-unknown}; ${reason}")
}

while IFS= read -r name; do
  [[ -z "${name}" ]] && continue
  classify_branch local "${name}" "refs/heads/${name}"
done < <(git for-each-ref --format='%(refname:short)' refs/heads/)

while IFS= read -r name; do
  [[ -z "${name}" ]] && continue
  # name comes through as <remote>/<branch>; strip the remote prefix.
  short="${name#${remote_name}/}"
  [[ "${short}" == "HEAD" ]] && continue
  classify_branch remote "${short}" "refs/remotes/${remote_name}/${short}"
done < <(git for-each-ref --format='%(refname:short)' "refs/remotes/${remote_name}/")

# --- Report ------------------------------------------------------------------
echo
echo "-- Safe to delete (tip already in main or the release branch) --"
if ((${#safe_local[@]} == 0 && ${#safe_remote[@]} == 0)); then
  echo "  (none)"
else
  for b in "${safe_local[@]+"${safe_local[@]}"}"; do
    echo "  local : ${b}    (git branch -D ${b})"
  done
  for b in "${safe_remote[@]+"${safe_remote[@]}"}"; do
    echo "  remote: ${remote_name}/${b}    (git push ${remote_name} --delete ${b})"
  done
fi

echo
echo "-- Keep / needs review (has unique commits) --"
if ((${#keep_report[@]} == 0)); then
  echo "  (none)"
else
  for line in "${keep_report[@]}"; do
    echo "  ${line}"
  done
fi

# --- Optional pruning --------------------------------------------------------
deleted=0
if ((prune == 1)); then
  if ((${#safe_local[@]} == 0 && (prune_remote == 0 || ${#safe_remote[@]} == 0))); then
    echo
    echo "Nothing to prune."
  else
    if ((assume_yes == 0)); then
      echo
      printf "Delete the safe-delete branch(es) listed above? [y/N] "
      read -r reply
      if [[ ! "${reply}" =~ ^[Yy]$ ]]; then
        echo "Aborted; no branches deleted."
        prune=0
      fi
    fi
  fi

  if ((prune == 1)); then
    for b in "${safe_local[@]+"${safe_local[@]}"}"; do
      if git branch -D "${b}" >/dev/null 2>&1; then
        echo "deleted local ${b}"
        deleted=$((deleted + 1))
      else
        echo "::error::failed to delete local ${b}" >&2
        inconsistent=1
      fi
    done
    if ((prune_remote == 1)); then
      for b in "${safe_remote[@]+"${safe_remote[@]}"}"; do
        if git push "${remote_name}" --delete "${b}" >/dev/null 2>&1; then
          echo "deleted remote ${remote_name}/${b}"
          deleted=$((deleted + 1))
        else
          echo "::error::failed to delete remote ${remote_name}/${b}" >&2
          inconsistent=1
        fi
      done
    fi
  fi
fi

# --- Summary -----------------------------------------------------------------
echo
echo "-- Summary --"
if ((prune == 1)); then
  echo "  deleted (safe)            : ${deleted}"
else
  total_safe=$(( ${#safe_local[@]} + ${#safe_remote[@]} ))
  echo "  safe to delete (dry-run)  : ${total_safe}  (re-run with --prune to delete)"
fi
echo "  kept for contributor work : ${kept_contributor}"
echo "  needs human decision      : ${needs_human}"

if ((inconsistent == 1)); then
  echo
  echo "::error::branch state is INCONSISTENT - resolve the items above before releasing." >&2
  exit 1
fi

exit 0
