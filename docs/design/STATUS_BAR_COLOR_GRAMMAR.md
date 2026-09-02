# Status-bar color grammar

The full-screen TUI already speaks a **color vocabulary**. This note names
that vocabulary so chrome cannot pick up an eighth meaning, or spend true
red on something that is not a failure.

RGB values and theme presets stay in `crates/tui/src/palette/`. Widgets do
not invent colors. Status-bar ink goes through
`crates/tui/src/palette/grammar.rs` (`SemanticFamily` + `ChromeInk`).
Each colour has one token name: the Identity blue is `WHALE_ACTION`
(`WHALE_INFO`, `WHALE_ACCENT_PRIMARY`, `STATUS_INFO` were aliases of it and
are gone), and the whale theme's `info` / `accent_primary` slots both hold it.

Not in scope: new themes, new orange semantics, or restyling existing
assignments. Community themes may paint a Cognition slot with a red-like
hue (Full Access on some presets); the **role** is still Cognition, not
Failure.

## The seven families

| Family | Spoken as | Means | Typical chrome |
| --- | --- | --- | --- |
| Outcome | GREEN | success / settled result / model output | phase `done` |
| Cognition | ORANGE | consequential action / elevated capability | permission ramp, waiting/approval, update chip |
| Active | CYAN | currently live / orchestration | phase working/verifying, live goal |
| Policy | PURPLE | user-selected mode | act / plan / operate |
| Identity | BLUE | who / which route | status mark, effort, context meter |
| Metadata | GRAY | passive location / historical state | route, repo/worktree, version, separators |
| Failure | RED | actual failure / destructive warning **only** | phase `failed` |

Red is reserved so it stays powerful: tool denied, destructive confirmation,
crashed agent, context failure. A dirty worktree is Metadata (`*`), not
Failure. The reservation is checked against *every* selectable preset, not
just the whale default: no Outcome, Active, Policy, Identity, or Metadata
ink may resolve to a theme's `error_fg`. Cognition is the one exemption
described above.

Metadata carries four weights, all the same meaning: `MetadataValue` for a
readable number, `Metadata` for its label, `MetadataHint` for the version
stamp, `MetadataDim` for separators.

## Status-bar map

The underwater header (top bar) and phase strip (footer rail) are the
status bar. Each segment picks a `ChromeInk` that already exists as a
`UiTheme` slot:

| Segment | `ChromeInk` | Family |
| --- | --- | --- |
| Status mark | `Identity` | Identity |
| Provider · model | `Metadata` | Metadata |
| Mode (act / plan / operate) | `PolicyAct` / `PolicyPlan` / `PolicyOperate` | Policy |
| Effort | `Info` | Identity |
| Permission (Ask / Auto-Review / Full Access) | `PermissionAsk` / `PermissionAutoReview` / `PermissionFullAccess` | Cognition |
| Goal (live / paused) | `Active` / `Attention` | Active / Cognition |
| Automation slot (`⏱ N scheduled · M running`) | `Info` / `Active` / `Attention` | Identity / Active / Cognition |
| Workflow chip | `Info` | Identity |
| Update chip | `Attention` | Cognition |
| Repo / worktree · branch`*` | `Metadata` | Metadata |
| Context meter / token breakdown | `Info` | Identity |
| Session metrics value | `MetadataValue` | Metadata |
| Session metrics label | `Metadata` | Metadata |
| Version | `MetadataHint` | Metadata |
| Separators | `MetadataDim` | Metadata |
| Phase idle | `Metadata` | Metadata |
| Phase typing | `Identity` | Identity |
| Phase working / verifying | `Active` | Active |
| Phase waiting / approval | `Waiting` | Cognition |
| Phase done | `Outcome` | Outcome |
| Phase failed | `Failure` | Failure |
| Footer toast info / success / warning / error | `Info` / `Outcome` / `Attention` / `Failure` | Identity / Outcome / Cognition / Failure |

YOLO / Full Access **mode** still paints as `PolicyAct` on the header. The
header must not borrow Failure red for a selected mode.

## Repo / worktree honesty

Repository chrome is derived from Git's common directory and the cached
`GitStatusSnapshot` (`crates/tui/src/tui/git_status.rs`). The render path
never probes. The label is:

- main checkout: `repo · branch*`
- linked worktree: `repo/worktree · branch*`
- unknown branch, known location: `repo` or `repo/worktree` (no invented
  ref)
- not a git repository: omit the segment

`*` is dirtiness. Ahead / behind stay on the same metadata string. Narrow
widths truncate the label by `ShellTier` and drop the segment rather than
wrap.

## Adding chrome

1. Pick one of the seven families. If none fit, the fact does not belong
   in color — use a word or glyph (`menu_style::StatusMark`).
2. Add a `ChromeInk` only when an existing `UiTheme` slot already carries
   that meaning. Do not add a theme and do not introduce a new family.
3. Name the KV-cache / density effect if the change also adds session
   context (it should not: this grammar is paint-only).
