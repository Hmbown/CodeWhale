# v0.8.67 Constitution Setup Current-Build Evidence

This note records current-build text/render evidence for the v0.8.67
constitution-first setup lane. It complements
`docs/evidence/v0867-constitution-setup-qa-matrix.md`; it is not a release
tag, artifact, or publish record.

Representative guided output examples, including a GLM-5.2-oriented profile,
are recorded in `docs/evidence/v0867-guided-constitution-examples.md`. As of
this snapshot the wizard also offers model-assisted drafting: `A` on the
Constitution step asks the first configured model to draft from the guided
answers, gated through `UserConstitution::from_untrusted_json`
(parse/sanitize/bound) and the same ratification preview + explicit `G` save.

- Date: 2026-07-02T03:54:02Z
- Branch: `claude/v0.8.67-constitution-setup-174rj9`
- Head: `fa7c4b055`
- Workspace version observed in `Cargo.toml`: `0.8.66`

## Covered Surfaces

These checks cover the text-snapshot side of the #3412 release-docs request:

- `/setup` constitution step at blocker terminal sizes `80x24`, `100x30`,
  `120x32`, and `160x40`.
- `/setup` provider/model readiness, runtime posture, constitution choice,
  guided preview/save, update checkpoint, skip/retry, verification report, and
  zh-Hans checkpoint copy.
- `/constitution` manager, preview, edit/repair/bundled/repo/explain/posture
  help paths, including zh-Hans manager/preview copy.
- Prompt injection for the user-global
  `<codewhale_user_constitution>` block and suppression for bundled/deferred or
  expert-override choices.
- `codewhale doctor --json` setup state derivation and persisted-state readback.
- Context report constitution/WHALE.md migration diagnostics without loading
  legacy `WHALE.md` bodies.
- Locale JSON validity for the shipped setup locale files.

## Commands Run

```sh
cargo test -p codewhale-tui --bin codewhale-tui --locked setup_wizard_is_usable_and_opaque_at_blocker_sizes -- --nocapture
```

Result: 1 passed, 0 failed.

```sh
cargo test -p codewhale-tui --bin codewhale-tui --locked constitution -- --nocapture
```

Result: 42 passed, 0 failed.

```sh
cargo test -p codewhale-tui --bin codewhale-tui --locked setup -- --nocapture
```

Result: 127 passed, 0 failed (includes the model-draft request/ingestion,
ratification, discard-on-tune, and authoring-provenance tests).

```sh
cargo test -p codewhale-tui --bin codewhale-tui --locked context_report -- --nocapture
```

Result: 10 passed, 0 failed.

```sh
cargo test -p codewhale-tui --bin codewhale-tui --locked doctor_setup -- --nocapture
```

Result: 5 passed, 0 failed.

```sh
cargo test -p codewhale-config --lib
```

Result: 342 passed, 0 failed (includes the `untrusted_draft_*` ingestion-gate
tests and the `constitution_authoring` round-trip/legacy-load tests).

```sh
cargo test -p codewhale-tui --bin codewhale-tui --locked verification_report -- --nocapture
```

Result: 2 passed, 0 failed.

```sh
jq empty crates/tui/locales/en.json crates/tui/locales/es-419.json crates/tui/locales/ja.json crates/tui/locales/pt-BR.json crates/tui/locales/vi.json crates/tui/locales/zh-Hans.json crates/tui/locales/zh-Hant.json
```

Result: passed.

## Localization Coverage and Fallback (zh-Hant)

The v0.8.67 setup/constitution surfaces are fully localized for `en` and
`zh-Hans` (545+ message keys each). `zh-Hant` ships a partial catalog
(~162 keys) that does not yet include the setup/constitution strings.

Documented fallback behavior for the strings zh-Hant does not carry: the
runtime message loader is initialized with `i18n!("locales", fallback =
["en"])` (`crates/tui/src/main.rs`), so untranslated zh-Hant keys render in
**English**. The `LocaleSpec { fallback: "zh-Hans" }` entry in
`crates/tui/src/localization.rs` is descriptive metadata only and is not
wired into message resolution; a per-locale zh-Hant → zh-Hans chain would be
a behavior change and is deliberately left out of v0.8.67. This satisfies
the #3412/#3794 acceptance alternative of "documented fallback" for
zh-Hant; full zh-Hant coverage (or a zh-Hans fallback chain) remains open
as follow-up localization work.

## Release-Orchestration Snapshot (2026-07-01)

- Branch: `claude/v0.8.67-constitution-setup-174rj9`
- Head: `19971477a`
- Workspace version observed in `Cargo.toml`: `0.8.66`

Landed since the prior snapshot: `008987464` (constitution-first setup copy
polish — welcome dual-meaning arc, ritual draft invitation, powers-and-limits
plus continuity-not-memory ratification framing, drafting-prompt steering),
`b58dcf99c` (#3884: sub-agent failure records carry the full classified error
chain), `b4ca8b539` (#3883: durable-review floor keys on action kind; routine
YOLO background work no longer prompts; destructive/publish holds preserved),
`19971477a` (QA-matrix rows for failed health check and legacy `.deepseek`
config migration).

Gate results at this head (exact observed counts):

```
cargo fmt --all -- --check                     PASS
git diff --check                               PASS
jq empty crates/tui/locales/*.json             PASS (7 files incl. zh-Hant)
cargo test -p codewhale-config --lib           342 passed; 0 failed
cargo test ... --locked setup                  127 passed; 0 failed
cargo test ... --locked constitution            42 passed; 0 failed
cargo test ... --locked context_report          10 passed; 0 failed
cargo test ... --locked doctor_setup             5 passed; 0 failed
cargo test ... --locked tui::onboarding          9 passed; 0 failed
RUSTFLAGS="-D warnings" cargo test ... --no-run  clean (1m 07s)
cargo build --release -p codewhale-cli -p codewhale-tui  clean (1m 02s)
```

Additional suites exercised for the two fix commits: yolo_mode 10, auto_review
31, approval 163, engine 272, widgets 186, command_safety 58, client 240,
retry 41, subagent 256 — all passing.

## Localization Coverage and Fallback (other locales)

Like zh-Hant, the `es-419`, `ja`, `pt-BR`, and `vi` catalogs do not yet carry
the v0.8.67 setup/constitution keys (their catalogs predate this lane); those
strings render in **English** through the same `fallback = ["en"]` loader.
This is the accepted state for v0.8.67; expanding setup/constitution coverage
to the remaining full locales is follow-up localization work (tracked under
the #3792/#3793 localization residuals).

## Enhancement-Pass Snapshot (2026-07-01, overnight)

- Branch: `claude/v0.8.67-constitution-setup-174rj9`
- Head: `d73875b7a` (19 commits ahead of origin)

Landed after the release-orchestration snapshot: `d46047a74` (keep-existing
constitution checkpoint completion, #3794), `ed6b21be1` (calm/compact
stakes-based approval prompt + `agent` tool classification), `41d26c774` /
`c57062077` / `7f82737b7` (#3757 startup and @mention performance +
startup milestone tracing), `a429f82c2` (route-identity pinning fixes four
machine-dependent test failures, verified pre-existing at the handoff head),
`d73875b7a` (model-drafted fleet profiles behind the draft→preview→ratify
gate — the constitution pipeline's second consumer), and website phase 1
(`28efab313`, `5a38e2059`, `d3d1d5e25`: live star badge/version nav, the
constitution-thesis hero in en+zh, and the animated terminal player over the
real session traces).

Final battery at this head (exact observed counts):

```
cargo test -p codewhale-tui --bin codewhale-tui --locked   5676 passed; 0 failed; 2 ignored
cargo test -p codewhale-config --lib                        342 passed; 0 failed
cargo build --release -p codewhale-cli -p codewhale-tui     clean (47.94s)
cargo fmt --all -- --check                                  PASS
cd web && npm run build                                     prerenders all locale routes
```

## Overnight Review + Hardening Snapshot (2026-07-02)

- Branch: `claude/v0.8.67-constitution-setup-174rj9`
- Head: `44eb7e935` (~35 commits ahead of origin; branch diff touches only
  `crates/`, `web/`, `docs/`, `scripts/`, `Cargo.*`)

A five-lens adversarial ultracode review of the overnight diff ran (34
agents; correctness / constitution-safety-invariants / performance / UX /
test-adequacy, each finding refuted-or-confirmed by an independent skeptic).
Confirmed findings were fixed:

- **Safety-floor destroyer gap** (`88c545f2b`): the #3883 floor narrowing had
  stopped holding `dd`-to-device / `mkfs` / `shred` / `wipefs` / forced
  recursive deletion of absolute system paths in YOLO background;
  `segment_is_device_or_filesystem_destroyer` restores those holds.
- **Repo law as mechanism** (`88c545f2b`): `.codewhale/constitution.json`
  `protected_invariants` may now carry path globs + `action` (ask|block),
  compiled into write holds in the tool gate (`crates/tui/src/repo_law.rs`),
  tighten-only, non-bypassable by mode, with a receipt naming the invariant.
- **Boot-janitor races** (`007ce2680`): backgrounded session cleanup no longer
  races session restore (skips/excludes the resumed id).
- **Event-loop-freeze class closed**: both the fleet-profile (`138dbad1b`) and
  constitution (`58aefa392`) model drafters moved off the inline await onto the
  background-cell + poll pattern; the UI stays interactive during drafting.
- **Fleet-draft finish + UX parity** (`007ce2680`): provider-readiness gating,
  en+zh localization, Enter-ratifies, duplicate-id guard.
- **UX copy** (`126633e78`, `e09fd46c2`): status classifier no longer paints
  negated-success failures green; shell exit codes are human-readable; lock
  jargon replaced with actionable copy; GitHub issue numbers removed from
  `--help` and config UI; session/model pickers have actionable empty states.
- **Test-adequacy gaps** (`432fa0fbe`) and a headless QA probe
  (`scripts/v0867-setup-qa.sh`, `93b5c1e59`) added.
- **#3830 missing-auth handoff** (`e2b32ec4c`): a route switch that fails for
  want of a key opens `/provider` at that provider's key entry.

Final gate battery at this head:

```
cargo fmt --all -- --check                              PASS
git diff --check                                        PASS
jq empty crates/tui/locales/*.json                      PASS (7 files)
cargo test -p codewhale-tui --bin codewhale-tui --locked  5686 passed; 0 failed; 2 ignored
cargo test -p codewhale-config --lib                    342 passed; 0 failed
RUSTFLAGS="-D warnings" cargo test ... --locked --no-run  clean
cargo build --release -p codewhale-cli -p codewhale-tui   clean (47s)
scripts/v0867-setup-qa.sh                               9 passed; 0 failed
```

## PR Push + Community Harvest Snapshot (2026-07-02)

- Branch pushed to origin as PR #3861 (ready for review), 118 commits ahead of
  `main`; `Cargo.toml` still `0.8.66`.
- A five-lens adversarial review of the branch found and closed real repo-law
  enforcement bypasses (interior `./`/`..` path evasion, apply_patch header
  forms, `fim_edit` ungated) and safety-floor destroyer evasions
  (env/wrapper prefixes, quotes, pipes) — all fixed with tests before push.
- Drafter event-loop-freeze class fully closed (fleet + constitution).
- Two safe runtime perf wins (idle offline-queue clone skipped; tool output
  hashed once). Structural render-loop items documented in the local roadmap
  for human-supervised TUI QA rather than changed blind.
- **Community harvest (v0.8.67):** four vetted PRs harvested with credit
  (contributor as canonical author + `Co-authored-by`): #3763 (@idling11,
  website i18n matrix), #3760 (@idling11, Homebrew rollout docs), #3872 and
  #3871 (@cyq1017, dead-code removals). Duplicates #3873/#3879 noted on-PR.
  MCP-spawn/provider/persistence/constitution-context PRs left NEEDS-REVIEW.
- CI at push: fmt / clippy (workspace, CI flags) / provider-registry /
  co-author (`--check-authors` canonical) / version-drift / GitGuardian /
  CodeQL / ubuntu+macOS tests all green; only slowest jobs finishing at
  snapshot time; zero failures.

## Remaining Manual Evidence

Before the release is called ready, keep the final manual pass from the QA
matrix: open a current TUI build and visually confirm the same flow through
`/setup`, `/constitution`, `/setup report`, `doctor --json`, and
`doctor --context-json`. This file records automated current-build coverage,
not a human visual acceptance pass.
