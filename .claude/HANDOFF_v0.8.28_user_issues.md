# v0.8.28 — User-Issue Strategy Handoff

**Audience:** the AI agent picking up post-v0.8.27 user-bug work.
**Scope:** items that didn't make v0.8.27 (deferred P3 / comment-pinged
P2) plus anything new that lands during the v0.8.27 → v0.8.28 inflow
window.

---

## Where you are

- **Working tree:** `/Volumes/VIXinSSD/whalebro/deepseek-tui`
- **Last shipped:** v0.8.27 (commit `aaccaee6`, tag `v0.8.27`) —
  17 community PRs + a focused user-issue sweep (flicker, wrap,
  pager copy-out, context-sensitive Ctrl+C, MCP auto-reload, notify
  tool, onboarding localization, paste UX rebuild, /skills filter).
  ~25 issues closed in the cycle. See PR #1375 for the full list.
- **Reference docs:** `.claude/HANDOFF_v0.8.26_security.md` for the
  release flow shape (same matrix → crates → npm → Homebrew → GHCR).

---

## Hard rules (same as v0.8.27 cycle)

1. **STOP and ask Hunter** before merging the release PR, tagging,
   or publishing to crates.io / npm / Homebrew.
2. No `--no-verify`, no `--no-gpg-sign`, no force push.
3. Don't leak `.private/` content into PRs / CHANGELOG / release notes.
4. v0.8.28 is **NOT** a security release by default. If a new GHSA
   arrives mid-cycle, branch a hotfix — don't bundle.
5. Time-box thorny items at 30-45 min. Defer rather than sink the
   cycle on one item.

---

## P1 — should ship; clear shape

### 1. CNB.cool mirror automated push

**Diagnosis.** v0.8.27 set up the CNB.cool destination repo at
`https://cnb.cool/deepseek-tui.com/DeepSeek-TUI` but the initial
bare-mirror push got 403 ("You do not have permission to push this
repository") with the issued PAT under user `whalebro`. The repo
shows as fresh / not-yet-initialized on the CNB side.

**Strategy.**

1. Confirm with CNB which scope the token needs (likely a "Push"
   permission separate from default repo read). Re-issue the
   PAT with that scope.
2. Verify the manual bare-clone-then-mirror push works once with
   the new PAT:
   ```bash
   git clone --bare https://github.com/Hmbown/DeepSeek-TUI.git /tmp/cnb-init
   cd /tmp/cnb-init
   git push --mirror https://whalebro:<NEW_TOKEN>@cnb.cool/deepseek-tui.com/DeepSeek-TUI
   ```
3. Wire up a `mirror-cnb` job in `.github/workflows/release.yml`
   that runs on tag push and pushes to CNB automatically. Store the
   token as a `CNB_PUSH_URL` GitHub Actions secret so future
   releases mirror without manual steps.
4. After CNB has content, add the mirror banner to `README.md` and
   `README.zh-CN.md` near the top:

   ```
   > 🇨🇳 国内镜像 / Mainland China mirror:
   >   https://cnb.cool/deepseek-tui.com/DeepSeek-TUI
   > Issues and PRs: please use GitHub.
   ```

**Cost:** ~30 min once the token has the right scope; ~1-2 hr if
the release.yml automation also needs the multi-platform Action
sorted out.

### 2. Ctrl+Enter as newline (#1372, follow-up to #1331)

**Diagnosis.** On Windows + nushell (and possibly some PowerShell
setups), users expect `Ctrl+Enter` to insert a newline. v0.8.27's
keybinding contract is:

- `Alt+Enter` / `Shift+Enter` / `Ctrl+J` → newline
- `Ctrl+Enter` → force-steer (submit into current turn)

The reporter for #1372 was on Windows + nushell. Comment-ping in
the issue asks them to try `Alt+Enter` / `Ctrl+J` and confirm
what `KeyEvent` the TUI actually sees via `RUST_LOG=debug`.

**Strategy.** Add `[tui] ctrl_enter_as_newline` config flag,
default false. When `true`, swap the priority so `Ctrl+Enter`
inserts a newline and `Ctrl+Enter+Ctrl` (or similar) force-steers.
Document the trade-off in `docs/CONFIGURATION.md`.

**Cost:** ~1 hour.

### 3. Windows `task_manager` test flake

**Diagnosis.** `task_manager::tests::persists_and_recovers_task_records`
uses `wait_for_terminal_state(&manager, &task.id, Duration::from_secs(3))`
— that 3s timeout for durable-task recovery is tight under Windows
CI file-I/O load. We saw one intermittent failure during the v0.8.27
PR run (passed on retry).

**Strategy.** Bump the timeout to `Duration::from_secs(10)` (or
`Duration::from_secs(if cfg!(windows) { 10 } else { 3 })`). No
functional change; just buys headroom for Windows CI.

**Cost:** ~10 min.

### 4. Test flakiness under load (general)

**Diagnosis.** The full tui test suite (now ~2640 tests) shows
intermittent failures on macOS under load:
- `mcp_connection_supports_streamable_http_event_stream_responses`
  (documented flaky; in handoff)
- `refresh_system_prompt_is_noop_when_unchanged` (new flake)
- `save_api_key_for_openrouter_writes_provider_table` (new flake)
- `tools::recall_archive::tests::list_archives_sorts_by_cycle_number`
  (new flake)

All four pass in isolation. Suggests env-var or filesystem state
sharing between tests that contend under parallel pressure.

**Strategy.** Audit the four tests for shared global state
(env vars, `~/.deepseek/`, tempdir patterns). The most likely
culprit is `unsafe { std::env::set_var(...) }` blocks without
holding a process-wide test mutex.

**Cost:** 1-2 hours.

---

## P2 — nice-to-have; awaiting reporter feedback

### 5. #1112 snapshot growth

v0.8.24 added a 500 MB snapshot cap + retention count + mid-session
pruning (vs startup-only). v0.8.27 cycle comment-pinged the reporter
asking for `du -sh ~/.deepseek/snapshots` on the latest version.
If they confirm growth is bounded, close. If still unbounded, the
likely culprit is iCloud Drive / network-FS interactions; investigate.

### 6. #1357 input/runtime-hint overlap

Windows reporter screenshots show input box overlapping the runtime
hint line. Comment-pinged in v0.8.27 cycle asking for terminal
width (`tput cols`), terminal type (Windows Terminal / VSCode / etc),
and the input that triggered the overlap. Likely a reserved-rows
calc bug in `crates/tui/src/tui/widgets/mod.rs` composer layout.

**Cost:** 1 hr repro + 1 hr fix once reporter responds.

### 7. #1281 Cmux notifications

Cmux is a tmux-derivative; `notify_done` doesn't fire inside Cmux.
Comment-pinged asking for `echo "TERM_PROGRAM=$TERM_PROGRAM TMUX=$TMUX"`
output and `[notifications]` config. Likely a one-line addition to
`resolve_method()` in `crates/tui/src/tui/notifications.rs` once we
know what env vars Cmux sets.

**Cost:** 30 min once reporter responds.

---

## P3 — investigate or defer

### #1338 Windows panic on Enter mid-run

The TUI's panic hook writes crash dumps to `~/.deepseek/crashes/`
already. Comment-pinged the reporter asking for the most recent
crash log. Without it the panic location is opaque. Defer until
reporter shares the log; then targeted fix.

### #1062 Capacity-memory checkpoint cross-session recovery

Old, complex. Needs scope conversation with Hunter before any work.

### #1067 glibc version (older Linux distros)

Static-link via musl build target. Purely a release.yml addition
(add a `x86_64-unknown-linux-musl` target alongside the gnu one).
**Good candidate for v0.8.28** if release.yml work happens anyway
(see Item #1 CNB mirror — both touch the release workflow).

**Cost:** ~1 hour to add the musl target + update the install script.

### #1364 Hooks v2 mutation rights + turn-end event

Real ask. Worth doing as part of a hooks-v2 rework. **Defer to
v0.9.0** — out of scope for a polish release.

### #1343 Desktop GUI

Recurring request. v0.9.x territory at the earliest. Comment with
roadmap status if not already.

### #1394 Prompt-level reliability guidance
Feature/enhancement for prompting (PR #1393). Defer to future release.

---

## Workflow

### Step 1 — Branch state confirmation

```bash
cd /Volumes/VIXinSSD/whalebro/deepseek-tui
git checkout main && git pull
git checkout -b work/v0.8.28
```

### Step 2 — Tackle items in priority order (P1 → P2 → P3)

For each item:
1. Read the issue thread on GitHub for any new reporter info.
2. Implement per the strategy.
3. Add tests (TDD where strategy specifies; verification snapshot otherwise).
4. After each commit:
   ```bash
   cargo fmt --all
   cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
   cargo test -p deepseek-tui --bin deepseek-tui --all-features --locked --no-fail-fast \
     2>&1 | grep "test result:" | tail -3
   ```
5. Add a CHANGELOG entry under `## [0.8.28]` `### Fixed` / `### Added` /
   `### Changed`, crediting the issue + original reporter where applicable.

### Step 3 — Bump version when ready

```bash
sed -i '' 's|^version = "0.8.27"|version = "0.8.28"|' Cargo.toml
find crates -maxdepth 2 -name Cargo.toml -exec sed -i '' \
  's|version = "0.8.27"|version = "0.8.28"|g' {} +
sed -i '' 's|"version": "0.8.27"|"version": "0.8.28"|' \
  npm/deepseek-tui/package.json
sed -i '' 's|"deepseekBinaryVersion": "0.8.27"|"deepseekBinaryVersion": "0.8.28"|' \
  npm/deepseek-tui/package.json
cargo update --workspace --offline
./scripts/release/check-versions.sh
```

Add `## [0.8.28] - YYYY-MM-DD` heading at the top of CHANGELOG.md.

### Step 4 — Preflight + release flow

Same as v0.8.27 — see PR #1375 and the v0.8.27 ship report for the
exact channel-ship sequence (merge → tag → matrix → crates → npm →
Homebrew → GHCR → CNB mirror → README on main → handoff transition).

If Item #1 (CNB mirror automation) lands, the CNB mirror push step
becomes automatic on tag push.

---

## Open items that may still arrive

The v0.8.27 release shipped on 2026-05-10. The 24-48 hour inflow
window after that release may surface fresh issues that should be
prioritized for v0.8.28. Default: any issue with ≥3 unique reporters
or any new flicker / panic / data-loss class bug gets P0 treatment.

---

## Quality bar (unchanged from v0.8.27)

Apply to every change:

- CI green (modulo documented flaky)
- No new `unwrap()` / `expect()` outside test code
- No new external network surfaces without `validate_network_policy`
- New env vars or config keys → `config.example.toml` entry + CHANGELOG note
- Behavior changes user-visible → CHANGELOG entry calling out the change

When in doubt, defer to v0.8.29. A clean release of 4 P1 items beats
a cluttered release of 12 with one regression.
