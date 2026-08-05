# v0.9.4 rebuild — release evidence (2026-08-04)

Candidate branch: `codex/v094-fleet-rebuild` @ `15439a0e3877f7d19b5c16cef59de1cb852170a2`
(13 commits on top of `v094-integration`; pushed to origin).

Owner instruction honored: **no release, no tag, no merge to main** — the
candidate is ready for review; release PR to be opened when the owner returns.

## What the rebuild delivers

1. **Named Fleets (one saved configuration per Fleet)** — `fleet/store.rs`.
   v2 TOML (`schema = "fleet"`): operator route (provider + exact model +
   reasoning, or explicit inherit), members (pin-or-inherit, provider on pins
   only, reasoning, instructions, capability requirements with a closed
   vocabulary), save scope (user-global `$CODEWHALE_HOME/fleets/` vs
   folder-scoped `.codewhale/fleets/`). Selection = scope-explicit
   `fleets/selected` files; a workspace selection may point at a personal
   Fleet without copying it; ambiguity is surfaced, never shadowed. Legacy
   per-role profile files migrate into Fleet "Default" with a receipt naming
   every pin, winner, and ignored conflict.
2. **Fleet manager UI** — `/fleet` opens the saved-Fleet list (one row per
   Fleet: display name, [user]/[folder] scope badge, source path, selection
   star); Enter opens the detail editor (operator/member route picker,
   reasoning tiers per provider, vision requirement toggle, inline rename,
   copy-to-other-scope); `u`/`w` select user/folder; `d` delete with
   confirm; `m` migrates legacy profiles. The shadow-badge pile is gone.
3. **Session route changes are temporary** — `/model` and `/provider`
   (and picker applies) change only the live session; nothing is written
   until an explicit command: `/fleet save` (update the selected Fleet's
   operator), `/fleet save-as` (new user-global Fleet + select),
   `/model save-default` (settings.toml). Every receipt names the exact
   file. A blocking-modal and a key-band design were both tried and
   rejected because they interrupted scripted terminals — a real PTY
   regression (release multi-terminal isolation test) proved it; the
   command surface is the final mechanism.
4. **Scout replaces faster** — `fleet/scout.rs`: pinned Scout always wins;
   unpinned Scout gets the provider's documented fast sibling VERIFIED
   against the merged catalog (never name-guessed); no verified companion
   = deliberate inheritance or a precise unavailable reason. The agent and
   workflow tool schemas no longer advertise `model_strength` (legacy
   parsing survives); the Fleet detail view shows `scout → provider/model
   (pinned | catalog suggestion | inherits session route)`.
5. **Truthful picker rows** — provider → family → model grouping (dim
   family headers from the catalog), chips that state only what the
   catalog knows (vision/text-only, tools, max output, reasoning stance
   first so it survives narrow widths), rendering clipped to the viewport
   (ratatui-core 0.1.0 panics instead of clipping), hitboxes aligned to
   rows.
6. **Config/credential scope fix** — hermetic cross-directory tests
   (7/7) reproduced the "authorized here, locked there" failure: an
   explicit workspace config path made a user-global key look missing.
   `has_api_key_for` now probes the user-global config's raw provider
   table (bounded, read-only, non-migrating) before concluding a key is
   missing. Nested repos and symlinked worktrees verified identical.
7. **Workflow decoupling** — CLI `workflow run --fleet` is now optional
   (kimicode/grokbuild shape: roles + session route + always-present
   built-in roster); `--fleet` still validates when given.

## Verification ledger

- cargo fmt --all -- --check: **exit 0**
- cargo check --workspace --all-targets --locked: **exit 0**
- cargo clippy --workspace --all-targets --all-features --locked -- -D warnings:
  **exit 0** (includes fixing pre-existing lint debt: snapshot/repo.rs
  nested-if, transcript dump println, dead startup-default builders)
- cargo test -p codewhale-tui --bin codewhale-tui: **9800 passed, 0 failed**
- cargo test -p codewhale-tui --test qa_pty: **41 passed**
- cargo test -p codewhale-tui --test release_runtime_qa: **21 passed, 1
  ignored** — includes the new `release_fleet_route_save_journey` dogfood
  leg: migration banner → migrate → session-only /model receipt →
  /fleet save (receipt + on-disk v2 file) → restart (operator applied) →
  /fleet list ([user] scope + selection) → /fleet save-as (second Fleet +
  legacy file untouched)
- cargo test -p codewhale-telemetry --lib: **45 passed** (payload schema,
  bounded fields, tombstone/off-switch — the GitGuardian JWT flag was a
  test fixture; signature literally decodes to "signature")
- cargo build --release --locked -p codewhale-cli -p codewhale-tui: **exit 0**
- cargo test --workspace --all-features --locked: **2 pre-existing failures,
  both reproduced identically on the base without this rebuild**:
  - `paste_matrix_lands_in_the_composer_without_autosubmitting` — boot-window
    input starvation, already documented in-repo (the `#[ignore]` comment on
    the related recovery-boot test);
  - `work_bar_still_shows_subagents_when_todos_are_present` — the Ocean
    Tasks/Workers top rail prioritizes the to-do panel and drops the
    subagent summary when both compete. The test (committed with the P5a
    lane) is the acceptance criterion for that remaining fix.

## Remaining gates (none are regressions from this rebuild)

- The two pre-existing failures above (both base-reproduced).
- The work_bar fix (subagent summary must stay visible when the to-do
  panel fills the rail) — in-repo test is the spec.

## GitHub state

- #5135 (old release train): flagged as superseded by the candidate;
  left open for reference. No tag/release created.
- #5192 (ratatui pin) and #5095 (ohos) closed as landed via the train.
- #5236 (Model Studio #5203 live evidence) harvested with full credit.
- #5234 (alternate scroll): verified already covered on the candidate.
- #5229 (zh-CN docs) and #5242 (checkpoint resume, draft): commented with
  the candidate's position.
- GitGuardian flag (918aa8c): verified NOT a real secret (synthetic test
  JWT; can be marked false-positive).
