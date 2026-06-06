# Status: Multi-tab system harvest to upstream `Hmbown/CodeWhale`

_Last updated: 2026-06-06 (post-Phase 2 rebase)_

This file is a working-state snapshot, not a strategy doc. For the strategic
plan and Phase 0/1/2 ordering, see `.claude/plans/github-deepseek-tui-skill-proxy-woolly-crescent.md`.
For the per-thread triage flow + GraphQL tooling, see `phase2-playbook.md`.

---

## 1. Pull requests

### PR #2753 — `feat(tui): multi-tab system with cross-tab collaboration`

| field | value |
| --- | --- |
| head | `7038ab36` (on `feat/multi-agent-v0850`) |
| base | `main` |
| state | OPEN |
| size | 201 files changed, 7,832 insertions(+), 32,009 deletions(-) |
| checks | GitGuardian pending · Greptile ✓ · CodeWhale CI matrix = `action_required` (fork-PR gate, can't be approved from contributor side) |
| Hmbown verdict | "too large for the current v0.9 stabilization harvest… narrow tab-core/persistence slice after UTF-8 truncation and stub collab paths are resolved" |

Last comment chain is closed: Hmbown confirmed the narrow-harvest path; my
reply on `4638292100` committed to flipping to the narrow slice first.

After the narrow slice (#2864) opened, I rebased #2753's v0850 onto the new
stewardship head (`5bd2f6a9`, 0 conflicts) and cherry-picked the 6 Phase 2
bot-review fixes (commit `7038ab36`) on top. Reply on `4638863128` flagged
the one remaining Greptile P1 thread as stale (already addressed in the
prior `2269d656` round).

### PR #2864 — `feat(tui): add multi-tab system core (manager + persistence)`

| field | value |
| --- | --- |
| head | `7fcd7d74` (on `feat/tab-core-narrow`) |
| base | `codex/v0.9.0-stewardship` (rebased to `5bd2f6a9`) |
| state | OPEN |
| size | 12 files changed, 3,644 insertions(+), 1 deletion(-) |
| checks | gate ✓ · GitGuardian ✓ · Greptile ✓ |
| scope | `tab/{mod,manager,persistence}.rs` + their `#[cfg(test)]` modules + `tui/mod.rs` module decl + `tools/shell.rs` redundant-cast cleanup |

This is the harvest Hmbown asked for. 9 new bot review comments landed on
the original head `649d3990`; the 6 fixable ones are addressed in `7fcd7d74`,
the 3 deferred ones (close_tab cleanup, cross_tab_links snapshot, group
ID collision) are explicitly out of scope for the narrow slice. See § 4.

---

## 2. Branches

```
feat/tab-core-narrow         7fcd7d74  ← current, PR #2864 head
feat/multi-agent-v0850       7038ab36  ← current, PR #2753 head (rebased
                                          onto new stewardship head + the
                                          6 Phase 2 fixes cherry-picked)
rebase/stewardship-measured  88dc3843  ← stale; pre-#2862 backup. Superseded
                                          by the current v0850 head which is
                                          itself the latest 0-conflict
                                          measurement.
upstream/codex/v0.9.0-stewardship 5bd2f6a9  ← target base for both PRs
```

Stewardship moved +3 commits since the first measurement
(`cc3cbc82`, `137d65c3`, `5bd2f6a9`); only `5bd2f6a9` (git status metadata
in `runtime_api.rs` + docs) is non-doc and it doesn't overlap the
multi-tab diff, so the rebase stayed 0-conflict.

---

## 3. Local CI matrix

Run on `rebase/stewardship-measured` (Windows runner), flags matching
`.github/workflows/ci.yml`:

| step | result |
| --- | --- |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --workspace --all-features --locked -- -D warnings` | exit 0, 0 errors, 0 warnings |
| `cargo test --workspace --all-features --locked` | 4023 passed, 6 failed |
| `git diff --exit-code -- Cargo.lock` | exit 0 |

The 6 failures are **pre-existing** on the baseline `2269d656` (the v0850
state before this round of cleanups) and reproduce after `git stash` of all
the cleanups, so they are not caused by this PR. They cluster in
`commands::skills::*` (filesystem), `settings::tests::settings_path_defaults_…`
(path), and three `tools::shell::*` Windows-runtime tests. None of them touch
the tab system or any file path the PR changes.

Tab-scoped test subsets on the narrow branch:

| subset | result |
| --- | --- |
| `cargo test -p codewhale-tui tab::` | 72/72 pass |
| `cargo test -p codewhale-tui tui::views::` | 59/59 pass |
| `cargo test -p codewhale-tui delegator::` | 7/7 pass |

---

## 4. Pending bot review threads on PR #2864

9 unresolved review threads on `649d3990`. Triage pending — see
`phase2-playbook.md` for the decision tree.

| # | author | path:line | severity | summary |
| --- | --- | --- | --- | --- |
| 1 | gemini | `tab/manager.rs:316` | high | `close_tab` leaves orphaned delegations + active meetings |
| 2 | gemini | `tab/persistence.rs:132` | medium | oversized file silently returns `default()` → data-loss risk on next save |
| 3 | gemini | `tab/mention.rs:164` | medium | `resolve_tab_mention` sorts input → semantic bug (mention ≠ visual order) |
| 4 | gemini | `tab/persistence.rs:64` | medium | `PersistedDelegation` has no `status` field → in-flight `InProgress` reverts to `Pending` |
| 5 | gemini | `tab/manager.rs:184` | medium | `cross_tab_links` not snapshotted → collab topology lost across restart |
| 6 | gemini | `tab/manager.rs:477` | medium | `delegate_task` accepts non-existent tab IDs |
| 7 | gemini | `tab/manager.rs:512` | medium | `start_meeting` accepts non-existent participant IDs |
| 8 | greptile | `tab/group.rs:79` | P2 | `TabGroup::new()` ID from `timestamp_millis()` — same-ms collision |
| 9 | greptile | `tab/manager.rs:435` | P2 | `pending_tasks` misnamed — returns completed `DelegationResult`s |

The narrow-harvest promise to Hmbown was that #2864 is "tab-core +
persistence" only, with collab/UI deferred to a follow-up PR. That means:

- **In-scope for #2864** (defensible as bugfixes of the shipped surface):
  #2, #3, #4, #6, #7, #9.
- **Belongs to the follow-up collab/UI PR** (file paths or behaviours that
  Hmbown was told would not be in this slice): #1 (`close_tab` cleanup is
  *correct* to add, but it materialises a behaviour the collab surface was
  supposed to provide; could go either way — see playbook).
- **Out of scope / cosmetic** (does not block a merge of a WIP-stub module):
  #5, #8 — `cross_tab_links` and `group.rs` are part of the stub collab
  surface; fixing the ID scheme or the snapshot shape there is correctly
  the follow-up PR's problem.

---

## 5. Stewardship-related fixes already applied to #2864

Carried over from PR #2753 review and re-verified on the narrow branch:

- UTF-8-safe `chars().count() + chars().take(N).collect()` in
  `views::tab_picker` and `views::tab_switcher` (byte slicing would panic
  on a multi-byte char at the cut point).
- `sort_by` → `sort_by_key(Reverse)` in delegator/meeting sort sites.
- `#![allow(dead_code, unused_imports)]` scoped to the WIP collaboration
  surface (`tab/delegator`, `tab/meeting`, `tab/cross_tab`, `tab/group`,
  `tab/mention`, `tab/persistence`, `tab/manager`, `tab/mod`,
  `views/meeting_view`) with rationale captured next to each allow.
- Dropped a pre-existing redundant cast in
  `crates/tui/src/tools/shell.rs` (`child.as_raw_handle()` already returns
  `*mut c_void`).
- `pub use manager::TabManager;` re-export retained as the public entry
  point for the follow-up wiring (allow on `unused_imports` is in scope).

---

## 6. Open questions for the user

These are decisions that need a human call, not a mechanical action. The
playbook flags them at the relevant step.

1. **#1 (close_tab cleanup) — include in #2864 or defer?** Fixing the
   orphan leak requires the new code to know which delegations and
   meetings to keep, which implicitly defines a public behaviour for the
   collab surface. If Hmbown wanted that surface unchanged, this should
   go to the follow-up PR; if it can be considered a defensive
   correctness fix on `close_tab`, it belongs here.
2. **#5 (cross_tab_links snapshot) — include in #2864 or defer?** The
   `cross_tab_links` field is on `TabManager` and the snapshot already
   takes a `&TabManager` reference, so adding it to the snapshot is a
   1-liner — but the *behaviour* of which links get restored, and the
   shape of the persisted cross-link, is part of the collab design.
3. **#6, #7 (delegate_task / start_meeting validation) — return `None`
   vs add `Result`?** Both methods currently return a `String` task ID /
   meeting ID. Changing to `Option<String>` is a public-API change that
   callers in the (deferred) UI pass will see. It is also a one-line
   change each. The narrow-harvest doesn't *have* any in-tree callers
   of those methods, so the rename is safe locally, but worth a moment
   of thought before committing it.
