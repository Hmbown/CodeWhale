# CodeWhale v0.8.67 Cursor Dogfood Notes

Date: July 7, 2026
Tester: Codex via Computer Use in Cursor integrated terminal
Workspace: `/Volumes/VIXinSSD/CW/codewhale`
Evidence log: `target/dogfood/cursor-v0867/cursor-dogfood-20260707-041000.log`

## Summary

The Cursor dogfood pass exercised the installed `0.8.67` binaries from Cursor's own integrated terminal and found the local runtime healthy. The release is now visible across the checked publication surfaces: GitHub Release, npm `codewhale@0.8.67`, and the crates checked by the release script.

This pass did not claim full visual TUI coverage. It used the dogfood doc's headless equivalents for many release issues, and records the remaining manual visual checks below.

## Works

- Cursor integrated terminal sees the final release SHA:
  - `737ac9872808deb96d6dc1dea0c2d79aa84e5f6a`
  - `737ac9872 (HEAD -> main, tag: v0.8.67, origin/main, origin/HEAD)`
- PATH-visible binaries report the final build:
  - `codewhale 0.8.67 (737ac9872808)`
  - `codew 0.8.67 (737ac9872808)`
  - `codewhale-tui 0.8.67 (737ac9872808)`
- GitHub release truth:
  - `v0.8.67` exists, non-draft, non-prerelease, published `2026-07-07T08:28:12Z`.
  - No open GitHub issues remain in milestone `v0.8.67`.
- Published release verification passed after npm publication:
  - `./scripts/release/check-published.sh 0.8.67`
  - `npm codewhale@0.8.67 is published`.
  - `npm codewhaleBinaryVersion=0.8.67`.
  - 17 checked crates.io packages are visible.
- Local gates passed:
  - `./scripts/release/check-versions.sh`
  - `git diff --check`
  - `cargo fmt --all --check`
  - `cargo build -p codewhale-tui --locked`
- Doctor/setup surfaces passed:
  - `codewhale-tui doctor --json` emitted valid JSON with `.setup`.
  - Hermetic `CODEWHALE_HOME` doctor emitted valid JSON with `.setup`.
  - `codewhale doctor | head -n 1` produced no stderr and exited through quiet SIGPIPE handling.
- Feature list worked:
  - `shell_tool`, `subagents`, `web_search`, `apply_patch`, `mcp`, and `exec_policy` are stable/enabled.
  - `vision_model` is beta/disabled.
- Setup-lane QA passed:
  - `CODEWHALE_BIN=target/release/codewhale-tui ./scripts/v0867-setup-qa.sh`
  - Result: `33 passed, 0 failed`.
- Regression tests passed for the core dogfood areas:
  - `subagent`: 298 passed.
  - Targeted subagent/delegate/worktree/budget/goal/config/setup/localization/pricing/model catalog/status/fleet/workflow filters all exited successfully where tests matched.
  - `cargo test -p codewhale-workflow -p codewhale-workflow-js --locked` passed all reported unit, VM, and doctest suites.
- Headless runtime smoke passed:
  - `codewhale app-server --stdio` health/capabilities/prompt surface OK.
  - `auth list` showed configured routes for `deepseek`, `openrouter`, `xiaomi-mimo`, and `zai`.
  - DeepSeek live exec smoke passed with `deepseek-v4-flash` and returned the sentinel.

## Does Not Work / Blockers

- No current release-publication blocker found in the checked surfaces after npm publication.
- Remaining blockers, if any, should come from manual TUI dogfood or downstream install smoke rather than registry visibility.

## Gaps / Manual Checks Still Needed

- The old visible Cursor chat summary still references `dc320ebf8`; the fresh terminal evidence corrects this to `737ac9872808`, but the stale transcript is visually confusing.
- The dogfood regression list included two filters that matched zero tests:
  - `child_hit_max_steps`
  - `missing_message`
  These did not fail the command, but they should be renamed in the dogfood prompt or replaced with current test names.
- Only DeepSeek was live-smoked to keep model spend bounded. Configured `zai`/GLM, `xiaomi-mimo`, and `openrouter` were discovered but not live-called in this pass.
- Current-home `doctor --json` reports `first_run_ready=true` and `update_ready=true`, but `operate_ready=false` while provider auth and fleet readiness look configured. This may be expected because live validation is false, but the user-facing readiness meaning should be clarified.
- Visual TUI checks still need a live human/agent pass:
  - `/setup` welcome copy and choose/draft/ratify arc.
  - `/setup` Constitution step options: guided preview, keep existing, model draft, bundled.
  - `/constitution` manager layer rendering.
  - Approval prompt tone and destructive styling.
  - `/fleet setup` model-draft and TOML preview/ratify flow.
  - Spinner animation and live sidebar details during a running worker/fleet turn.

## Release Readiness Call

Local runtime readiness and publication visibility look good. The only product-quality items I would hold for a follow-up issue, not necessarily for the published `0.8.67` artifacts, are the stale dogfood filters and the ambiguous `operate_ready=false` doctor signal.
