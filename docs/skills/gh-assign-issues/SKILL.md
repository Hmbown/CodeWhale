---
name: gh-assign-issues
description: "Use to assign GitHub issues to a milestone and/or owners in bulk, verifying each."
---

# gh-assign-issues

Retarget or assign a set of Codewhale issues to a milestone and/or owners in
bulk, verifying every one. The milestone (or assignee) change is the signal;
do not narrate it with comments. Use `gh` for all GitHub
calls.

## When to use

- You have a concrete list of issue numbers (e.g. from triage) to move into a
  release milestone such as `v0.8.61`, or to assign to owners.
- A milestone was renamed/created and its issues need re-pointing.
- You finished grouping the inbox and want the queue to reflect it without
  posting public chatter.

This skill changes milestone/assignee only. It does not close, label, comment,
merge, or release. Those stay with the maintainer.

## Workflow

1. Confirm the exact milestone title. `gh issue edit --milestone` matches by
   name, so a typo silently fails (or worse, no-ops). Read the real titles and
   note the starting open-count:

   ```bash
   gh api repos/Hmbown/CodeWhale/milestones \
     --jq '.[] | "\(.number)\t\(.title)\topen=\(.open_issues)\tstate=\(.state)"'
   ```

   Copy the title string verbatim (e.g. `v0.8.61`). If the target milestone is
   missing or closed, stop and ask the maintainer; do not create one.

2. Pre-flight each number. `gh issue edit` will happily retarget a PR or a
   closed issue, so screen first. The `url` reveals PR-vs-issue (`/pull/` vs
   `/issues/`); skip anything not OPEN or that is a PR:

   ```bash
   for N in 3101 3102 3103; do
     gh issue view "$N" --repo Hmbown/CodeWhale \
       --json number,state,url,milestone \
       --jq '"\(.number)\t\(.state)\t\(.url)\tmilestone=\(.milestone.title // "none")"'
   done
   ```

   Flag any row whose `url` contains `/pull/` (it is a PR) or whose state is
   not `OPEN`, and exclude it from the loop below.

3. Apply the change, one issue at a time, reporting per-issue success. Use
   `--milestone`, `--add-assignee`, or both:

   ```bash
   for N in 3101 3102 3103; do
     if gh issue edit "$N" --repo Hmbown/CodeWhale \
          --milestone "v0.8.61" >/dev/null 2>&1; then
       echo "ok   #$N -> v0.8.61"
     else
       echo "FAIL #$N (PR? closed? bad milestone title?)"
     fi
   done
   # owners: add --add-assignee handle (do not invent logins)
   ```

   Edit is idempotent: re-pointing an already-correct issue is harmless.

4. Verify the milestone moved. Re-run the step-1 command and confirm the open
   count rose by the number of issues you moved in (minus any you skipped).
   Spot-check a few with the step-2 view to confirm `milestone.title` is now
   the target.

5. Report a tight ledger: issues moved, issues skipped and why (PR / closed /
   title mismatch), the before/after open-count, and any owner assignments.
   No public comment is posted.

## Red flags / don't

- Don't edit from a list of titles alone. Resolve to OPEN issue numbers and
  confirm each is an issue, not a PR (`/pull/` in the url), before editing.
- Don't guess the milestone title or assignee login. A near-miss name no-ops
  or fails silently; an invalid login errors mid-loop.
- Don't post "moved to v0.8.61" comments. The milestone change is the signal;
  extra chatter is noise to good-faith contributors.
- Don't close, merge, label, tag, or release as part of this. Those need
  explicit maintainer approval (see AGENTS.md).
- Don't skip step 4. An unmoved open-count means the title was wrong or every
  edit silently failed.
- Preserve contributor credit: this skill never alters authorship, harvest
  trailers (`Co-authored-by` / `Harvested-from`), or closclosing references.
