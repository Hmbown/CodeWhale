# Phase 2 playbook: review triage for the multi-tab harvest

## Purpose

After the Phase 0 stability fixes and Phase 1 narrow tab-core/persistence
harvest land, Phase 2 is mostly **review feedback processing**: bot reviews
on the harvest PR, Hmbown's structural comments, and CI follow-ups.

The harvest is #2864 (narrow) and the source branch is #2753 (full). The
playbook is written for #2864, but the same flow applies to #2753 once
#2864 is merged.

This doc is the **decision tree and the tooling** — not the plan. For the
plan, see `.claude/plans/github-deepseek-tui-skill-proxy-woolly-crescent.md`.
For current state, see `STATUS.md`.

---

## 1. The triage decision tree

Every review thread (bot or human) flows through this:

```
review thread lands
    │
    ▼
[Q1] Is the comment author a known bot (gemini-code-assist, greptile-apps,
     github-actions, copilot-pull-request-reviewer, codewhale-ci-bot)?
    │
    ├─ no  → human reviewer → § 2 (manual triage)
    │
    └─ yes → [Q2] Does the comment identify a true correctness/correctness-
             adjacent issue, OR is it stylistic / speculative / wrong?
              │
              ├─ true issue     → § 3 (fix-and-resolve)
              ├─ false positive → § 4 (reply-and-resolve with rationale)
              └─ stylistic       → § 5 (defer or absorb)
```

A "true issue" check is **does this actually change behaviour or risk
data loss if left as-is**. Anything that doesn't trip that bar is not
worth a code change in this PR — it goes to the follow-up collab/UI PR,
or it doesn't get fixed at all.

---

## 2. Manual triage (human reviewer)

When Hmbown comments, treat the comment as binding unless the user
explicitly says otherwise. Hmbown's two previous reviews on #2753 are
the template:

- He flagged scope ("too large for v0.9") and asked for a narrow slice.
- He flagged design (visible collab paths are WIP) and asked them to be
  stubbed.
- He flagged CI ("has not run the normal CodeWhale CI matrix") and
  required matrix-clean before merge.

For each Hmbown comment, draft a reply in two parts:

1. **What I will change in this PR** (concrete commit list)
2. **What I will defer to the follow-up** (named files / behaviours)

If the reply needs to push back, do it with a one-sentence reason and
an explicit ask: "If you would prefer X, I can switch the order; this
is the path I picked because Y."

---

## 3. Fix-and-resolve (true issue from a bot)

The flow:

```
1. Open the file:line in the editor.
2. Read enough surrounding code to make sure the suggested fix matches
   the actual behaviour, not just the comment's mental model.
3. Write a one-line commit per fix (or group 2-3 mechanical ones into
   a single "chore(tui): address bot review on <topic>" commit).
4. Push the commit; the thread goes stale and is auto-outdated.
5. Reply to the thread with the fix SHA and a one-line description.
6. Resolve the thread via the GitHub UI or the GraphQL mutation in § 6.
```

Do not resolve threads without either:

- a fix commit on the PR head, or
- a reply explaining why the fix is deferred (with the follow-up PR link
  or a TODO), or
- a reply explaining why the suggestion is incorrect.

A bot thread that is closed with no reply trains the bot (and Hmbown)
to ignore the contributor's PRs.

---

## 4. Reply-and-resolve (false positive from a bot)

The flow:

```
1. Reply to the thread on the PR with a one-paragraph rationale that
   cites file:line of the relevant code (or test) that demonstrates
   the suggestion is incorrect.
2. Resolve the thread via the GraphQL mutation in § 6.
```

Templates:

- "This is intentional — see `tab/manager.rs:NNN` where the cleanup
  happens on tab close. The `delegator.pending_tasks` slice is drained
  for the closed tab by `take_pending_for_tab`."
- "Not applicable to this PR — the file is in the WIP collab surface
  (`tab/delegator.rs`) which the follow-up PR will rewire. Marking as
  out-of-scope here."

Never resolve a thread without a reply.

---

## 5. Defer or absorb (stylistic / speculative)

Stylistic bot suggestions (e.g. "consider using a UUID here" on a
collision-resistant path that already has a uniqueness check) are
**not worth a code change**. Two options:

- **Defer**: leave the thread open, add a one-line reply saying it's
  deferred to the follow-up PR. Don't resolve.
- **Absorb silently**: if the change is one line and unambiguous, take
  it in a `chore` commit and resolve the thread.

The deciding factor: would a human reviewer (Hmbown) flag this on a
re-read? If yes, absorb. If no, defer.

---

## 6. Tooling: batch-resolving review threads

The 9 bot threads on #2864 land on a single commit
(`649d3990d61503e3c13cb38c6b251150e35b925a`). Listing them:

```bash
gh api graphql -f query='
query {
  repository(owner: "Hmbown", name: "CodeWhale") {
    pullRequest(number: 2864) {
      reviewThreads(first: 50) {
        nodes {
          id
          isResolved
          isOutdated
          path
          line
          comments(first: 1) {
            nodes { author { login } body }
          }
        }
      }
    }
  }
}'
```

Resolving a single thread by ID:

```bash
gh api graphql -f query='
mutation($id: ID!) {
  resolveReviewThread(input: {threadId: $id}) {
    thread { isResolved }
  }
}' -f id=PRRT_kwDOQ9AYz86HjtoH
```

Batch-resolving every unresolved, non-outdated thread on a PR (use with
care — this resolves *everything*):

```bash
gh api graphql -f query='
query {
  repository(owner: "Hmbown", name: "CodeWhale") {
    pullRequest(number: 2864) {
      reviewThreads(first: 50) {
        nodes { id isResolved isOutdated }
      }
    }
  }
}' \
| jq -r '.data.repository.pullRequest.reviewThreads.nodes[]
         | select(.isResolved == false and .isOutdated == false)
         | .id' \
| while read id; do
    gh api graphql -f query='
    mutation($id: ID!) {
      resolveReviewThread(input: {threadId: $id}) { thread { isResolved } }
    }' -f id="$id"
  done
```

> **Caution**: only run the batch version after every thread has had a
> reply. The script will not check that.

---

## 7. The 9 threads on #2864, pre-tagged

Decision tree output for the current state of #2864. See STATUS.md § 4
for the full table.

| # | path:line | decision | commit prefix |
| --- | --- | --- | --- |
| 1 | `tab/manager.rs:316` | defer to follow-up collab PR — `close_tab` cleanup is a collab-surface design decision | n/a |
| 2 | `tab/persistence.rs:132` | **fix-and-resolve** (data loss) | `fix(tab): error on oversized persistence file` |
| 3 | `tab/mention.rs:164` | **fix-and-resolve** (semantic bug; also update test) | `fix(tab): preserve caller order in resolve_tab_mention` |
| 4 | `tab/persistence.rs:64` | **fix-and-resolve** (in-flight status lost) | `feat(tab): persist delegation status` |
| 5 | `tab/manager.rs:184` | defer to follow-up collab PR | n/a |
| 6 | `tab/manager.rs:477` | **fix-and-resolve** (validation; `Option<String>` return) | `feat(tab): validate tab IDs in delegate_task` |
| 7 | `tab/manager.rs:512` | **fix-and-resolve** (validation; `Option<String>` return) | `feat(tab): validate participants in start_meeting` |
| 8 | `tab/group.rs:79` | defer — `tab/group.rs` is in the WIP surface, narrow harvest doesn't ship it | n/a |
| 9 | `tab/manager.rs:435` | **fix-and-resolve** (rename; `pending_tasks` → `completed_delegations`) | `refactor(tab): rename misleading pending_tasks getter` |

Expected PR delta: 6 commits, all mechanical, all on the narrow branch.
None of them touch the host TUI wiring, so the Stewardship review path
stays the same.

---

## 8. Order of operations

1. Read each thread's surrounding code, confirm the suggested fix is
   correct in context (some bot suggestions don't survive a real read).
2. Land the 6 fix commits as a single `chore(tui): address Phase 2
   bot-review on #2864` series — one commit per row in the table above.
3. Re-run the local CI matrix (fmt / clippy -D warnings / test / lockfile).
4. Push the branch. The bot threads will go stale (outdated) but stay
   open until the GraphQL resolve runs.
5. For each of the 6 fix commits, post a one-line PR comment on the
   thread pointing to the commit.
6. For each of the 3 defer threads (#1, #5, #8), post a reply explaining
   the deferral with a link to the follow-up PR or a TODO.
7. Run the batch-resolve script from § 6.

After that, the PR is clean for Hmbown to re-review. The follow-up
collab/UI PR can then close threads #1 and #5 with their own fixes.

---

## 9. What this playbook does NOT do

- It does not push back on Hmbown's structural design (Phase 1/2 split).
  That was negotiated in § 1 of the strategy and is fixed.
- It does not auto-merge. Hmbown merges; the playbook only gets the PR
  to a state where Hmbown can.
- It does not touch the deferred collab/UI surface. Adding a tab
  switcher, a meeting modal, a `TabBar` widget, etc. is the follow-up
  PR's job, not this one.
