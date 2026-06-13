# v0.9.0 Release Acceptance Matrix

This matrix is the pre-tag gate for v0.9.0. Do not tag or publish v0.9.0 until
each row is checked off or has an explicit defer decision with an owner.

For every manual smoke, record the date, OS, provider/model, command, redacted
config source, result, and follow-up issue or PR.

## Core Build And Packaging

| Gate | Owner | Ship/defer decision | Evidence |
| --- | --- | --- | --- |
| `cargo fmt --all -- --check` | release steward | ship | Passed locally on 2026-06-06 at `2561a54df`. |
| `cargo check --workspace --all-targets --locked` | release steward | ship | Passed locally on 2026-06-06 at `2561a54df`. |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | release steward | ship | Passed locally on 2026-06-06 at `2561a54df`. |
| `cargo test --workspace --all-features --locked` | release steward | ship | Passed locally on 2026-06-06 at `2561a54df` (`4254 passed, 0 failed, 4 ignored` in `codewhale-tui`; package integration and doctest suites also passed). An earlier full run hit one transient localhost SSE reset in `mcp::tests::legacy_sse_closed_stream_reconnects_and_retries_tool_call`; the exact test passed serially before the full rerun. |
| `./scripts/release/check-versions.sh` | release steward | ship | Passed locally during #2845 (`e22a7da53`) and remains part of the PR-local release gate for each stewardship slice. |
| `./scripts/release/check-ohos-deps.sh` | release steward | ship | Passed locally during #2845 (`e22a7da53`); OHOS dependency graph stayed compatible for `codewhale-tui` on `aarch64-unknown-linux-ohos`. |
| `./scripts/release/publish-crates.sh dry-run` | release steward | ship | Passed locally on 2026-06-06 at `2561a54df`. The script performed full `cargo publish --dry-run` for crates without unpublished workspace dependencies and package-content verification for dependent workspace crates; expected 0.8.53 already-published warnings were observed. |
| `node scripts/release/npm-wrapper-smoke.js` after release build | release steward | ship | Passed locally on 2026-06-06 at `2561a54df` after `cargo build --release --locked -p codewhale-cli -p codewhale-tui`. The harness packed `codewhale-0.8.53.tgz`, served local release assets, and verified `npx --no-install codewhale doctor --help` plus `npx --no-install codewhale-tui --help`. |
| GitHub release asset verification before npm publish | release steward | post-tag/pre-npm gate | The live v0.9.0 GitHub Release does not exist yet. After tagging and before `npm publish`, verify the Release contains the expected platform archives, individual binaries, Windows installer/portable assets, `codewhale-artifacts-sha256.txt`, and `codewhale-bundles-sha256.txt`; `npm/codewhale/scripts/verify-release-assets.js` remains the npm prepublish asset guard. |

## Provider, Model, And Auth

| Gate | Owner | Ship/defer decision | Evidence |
| --- | --- | --- | --- |
| DeepSeek V4 direct provider smoke | provider steward | ship | Passed locally on 2026-06-06 at `7bd68279e` using macOS 26.1 arm64 release binary: `./target/release/codewhale --provider deepseek --model deepseek-v4-flash exec "Reply exactly CODEWHALE_V09_SMOKE_OK and nothing else."` returned `CODEWHALE_V09_SMOKE_OK`. Redacted auth source: `codewhale auth status --provider deepseek` reported config-backed DeepSeek API key present, env unset, with no secret value printed. |
| Xiaomi MiMo token-plan and pay-as-you-go config smoke | provider steward | ship config evidence / require live smoke before tag if claiming provider availability | Config coverage exercises token-plan and pay-as-you-go env behavior in `crates/config/src/lib.rs` (`xiaomi_mimo_env_token_plan_mode_uses_token_plan_key_and_endpoint`, `xiaomi_mimo_env_pay_as_you_go_mode_prefers_standard_key`) and mirrors the TUI config path in `crates/tui/src/config.rs`; `docs/PROVIDERS.md` documents Token Plan regions and pay-as-you-go mode. This is config evidence only, not a live Xiaomi call. |
| Arcee Trinity Thinking route smoke or explicit defer | provider steward | defer live smoke / ship static route metadata | Static provider/model metadata exists in `docs/PROVIDERS.md`, `crates/agent/src/lib.rs`, and `crates/tui/src/config.rs`, but no live Arcee credential smoke has been recorded. Do not claim live Arcee route readiness in v0.9 release notes unless a dated manual smoke is added. |
| Hugging Face provider route and MCP concept helpers ship; native Hub search/passports are deferred | model-lab steward | ship foundation / defer native search-passport runtime | `ProviderKind::Huggingface`, env aliases, picker/docs, and `/hf concepts` / `/hf mcp status` distinguish the chat provider route from Hugging Face MCP and explicit Hub tooling. `docs/PROVIDERS.md` states native Hub HTTP search/passport picker metadata are not shipped behavior in this checkout; #2705/#2707/#2712 remain open for native Model Lab work. |
| OpenRouter, Novita, Fireworks, and Volcengine env behavior smoke | provider steward | ship config evidence / require live smoke before claiming live route coverage | Env/config tests cover OpenRouter, Novita, Fireworks, and Volcengine key/base-url/model override behavior in `crates/config/src/lib.rs`; TUI provider defaults and Volcengine env override are covered in `crates/tui/src/config.rs`, and `docs/PROVIDERS.md` documents the env/default behavior. This is env behavior evidence only, not live provider traffic. |
| Provider registry drift check covers aliases/default env keys | provider steward | ship | #2820 (`5d491bc68`) added the metadata-only provider registry and `scripts/check-provider-registry.py`; verification included `python3 scripts/check-provider-registry.py` and `cargo test -p codewhale-config provider_ -- --nocapture`. |
| Provider-scoped TLS skip-verify remains default-off and doctor-visible | security steward | ship | #2834 (`190e9f35e`, `6269cb91f`) landed provider-scoped TLS skip verify with default-off config, doctor warnings, docs, and CLI/runtime option tests. |

## Runtime Stability

| Gate | Owner | Ship/defer decision | Evidence |
| --- | --- | --- | --- |
| Windows input/render smoke or documented manual verification | runtime steward | manual smoke required before tag | No dated Windows input/render smoke has been recorded on this matrix yet. Unit/shell-dispatcher tests are not a substitute for Windows ConPTY/manual input verification. |
| macOS and Linux TUI startup smoke | runtime steward | ship | macOS 26.1 arm64 evidence from 2026-06-06: release binaries built from the stewardship line reported `codewhale-tui 0.8.53 (2561a54df0ed)` and `codewhale 0.8.53 (2561a54df0ed)`, and `cargo test -p codewhale-tui --test qa_pty --locked` passed 6/6 startup/composer/keystroke PTY scenarios. Linux evidence from 2026-06-06: a streamed source archive built inside a Debian Bookworm arm64 `rust:1.88-bookworm` container with `libdbus-1-dev` / `pkg-config`; `cargo build --release --locked -p codewhale-cli -p codewhale-tui` passed and `./target/release/codewhale --version` / `./target/release/codewhale-tui --version` both ran successfully. |
| Large-repo startup smoke | runtime steward | defer full smoke / ship bounded-context mitigation evidence | Bounded project-context tests and changelog evidence cover the mitigation slice, but live large-workspace reports #697 and #1827 remain open. Do not close those issues or claim a full large-repo startup smoke without a dated manual run. |
| Sub-agent timeout/completion smoke | subagent steward | ship timeout/completion slice | `docs/SUBAGENTS.md` documents per-step timeout and heartbeat behavior; `crates/tui/src/tools/subagent/tests.rs` covers `api_timeout_preserves_checkpoint_and_agent_eval_continues_from_it`, parent completion ordering, and timeout propagation. Broader hung-agent issues #1806/#2614 remain open. |
| Long-running command live-state smoke | runtime steward | defer root-cause live-state smoke / ship shell-routing tests | Shell tests cover timeout/background/wait/cancel behavior and `shell_job_routing.rs` distinguishes live from stale process state, but #1786 remains open for shell PID/task-flow hangs and premature LIVE-state exit. |
| Runtime API remains token-protected for GUI clients | GUI steward | ship | #2811/#2814 documented and consumed the existing runtime token flow from the official VS Code extension; #2822 (`bb8835812`) added `GET /v1/snapshots` behind the same runtime API token middleware. |
| Snapshot/restore surfaces are read-only unless mutation semantics are tested | GUI steward | ship | #2822 (`bb8835812`) and #2828 (`293643e27`) expose restore points as read-only listing/Agent View metadata only; #2808 restore/retry/patch-undo mutation endpoints remain unmerged pending atomicity tests. |

## UI And Workflow UX

| Gate | Owner | Ship/defer decision | Evidence |
| --- | --- | --- | --- |
| First-look screen included or explicitly deferred | UX steward | defer v0.9 redesign / keep existing onboarding | The existing onboarding welcome remains covered by `first_run_user_always_starts_at_welcome`; the opinionated v0.9 first-look/home redesign remains deferred to #2713 so release notes should not imply a new home screen. |
| Slash picker readability smoke | UX steward | ship | Focused slash-menu coverage exercises visibility/hide state, removed-command filtering, Up/Down wrap behavior, argument spacing, skill command insertion, inline skill mentions, Esc priority, and locked composer height while match counts change. Verification: `cargo test -p codewhale-tui slash_menu --locked`, `cargo test -p codewhale-tui try_autocomplete_slash_command_completes_skill_argument --locked`, and `cargo test -p codewhale-tui next_escape_action_slash_menu_takes_priority --locked`. |
| Transcript tool-collapse smoke or explicit defer | UX steward | ship | #2776 (`c76ec4752`) landed dense successful tool-run collapse with guardrails for failed/running/shell/patch/review/diff cells; focused widget coverage includes `chat_widget_collapses_dense_tool_runs_by_default`, `chat_widget_expands_dense_tool_runs_on_demand`, and `chat_widget_expanded_mode_leaves_dense_tool_runs_visible`. |
| Sidebar detail popovers smoke or explicit defer | UX steward | ship | #2778 (`3cb49233e`) added row-level hover metadata and wrapping detail popovers for truncated Work/Tasks/Agents rows; #2806 (`19f5c7aa6`) preserved current sub-agent progress in the sidebar hover text. Focused coverage includes `sidebar_hover_rows_mark_source_text_diff_as_truncated` and `subagent_hover_text_preserves_full_agent_id_and_progress`. |
| Plan review/handoff artifact smoke | Plan steward | ship | #2770 (`7ac8063b6`) added rich PlanArtifact sections through the transcript/Plan prompt path; focused coverage includes `plan_update_cell_renders_rich_artifact_metadata` and `plan_prompt_renders_rich_plan_artifact_sections`. |
| VS Code Agent View branch/workspace visibility smoke | GUI steward | ship | #2825 (`1bacaf763`) added `workspace` / `branch` metadata to `/v1/threads/summary`; #2832 (`50b773f1d`) added read-only auto-refresh so branch/workspace changes can appear without manual refresh. The current stewardship slice extends the same read-only metadata with current Git `head` and `dirty` worktree state for editor/agent-lane visibility. |

## v0.9.0 Feature Gates

| Gate | Owner | Ship/defer decision | Evidence |
| --- | --- | --- | --- |
| WhaleFlow typed IR, mock executor, replay, TeacherReview, StudentReplay, and cutline docs are tested | WhaleFlow steward | ship | #2821/#2824/#2831/#2833/#2839/#2840/#2841 plus focused local `cargo test -p codewhale-whaleflow --locked`; #2670 closed after `cargo test -p codewhale-whaleflow starlark --locked` passed 7/7 on current stewardship head. The `rlm_cache_change.star` dogfood workflow now has recorded mock-trace replay coverage, including a missing-record divergence check. |
| Live `workflow_run`, worktree application, provider calls, and TraceStore writes are deferred until cancellation/replay/atomicity semantics pass | WhaleFlow steward | defer | #2669 and #2679 remain open for live runtime execution, provider calls, TraceStore writes, Arcee/student replay, and CLI/TUI workflow mode; current v0.9 branch ships mock executor/replay foundations only. |
| Model Lab / Hugging Face MVP is included or deferred with release-note wording | model-lab steward | ship provider/MCP docs foundation / defer native Model Lab MVP | v0.9 ships the Hugging Face chat-provider route, provider docs, and `/hf` concept/MCP status helpers only. Native Hub search, model passports, Spaces/Jobs workflows, and Model Lab eval/export surfaces remain deferred to #2705/#2707/#2710/#2712/#2727. |
| HarnessProfile runtime MVP is deferred; schema/resolver foundation ships with release-note wording | harness steward | ship foundation / defer runtime | #2844 (`efbcc681a`) documents the cutline; `HarnessPosture` / `HarnessProfile` config schema and strict validation are present; a pure resolver matches provider/model routes without changing runtime behavior; seed-profile runtime selection, telemetry, and status display remain follow-up work. |
| `codebase_search` MVP is included or deferred with release-note wording | search steward | defer runtime / ship design doc | `docs/rfcs/CODEBASE_SEARCH_DESIGN.md` is explicitly doc-only and says no catalog code ships in this cycle; runtime tool registration, index/eval fixtures, and search implementation remain deferred to #2680. |
| External memory remains explicit/optional per `rfcs/WHALEFLOW_EXTERNAL_MEMORY.md` | memory steward | ship | #2842 (`a7052751e`) added the external-memory cutline: optional/explicit workflow node/plugin only, visible state/owner/storage/scope, and no hidden default context substrate. |

## Remote Workbench

| Gate | Owner | Ship/defer decision | Evidence |
| --- | --- | --- | --- |
| Remote workbench is marked included, experimental, or deferred | remote steward | defer runtime / ship setup docs only | `docs/rfcs/REMOTE_SETUP_DESIGN.md` and `docs/TENCENT_LIGHTHOUSE_HK.md` document possible VM/Telegram/Lark setup patterns, but no v0.9 remote workbench runtime is included. |
| If included: VM install smoke passes | remote steward | defer | Not applicable while remote workbench runtime is deferred; no v0.9 VM install smoke is required before tagging. |
| If included: Telegram bridge smoke passes | remote steward | defer | Not applicable while remote workbench runtime is deferred; Telegram bridge docs remain design/setup guidance only. |
| If deferred: release notes avoid implying remote workbench availability | remote steward | ship | Acceptance matrix and changelog wording must say setup/design docs only, not a shipped remote workbench feature. |

## Docs, Migration, And Rollback

| Gate | Owner | Ship/defer decision | Evidence |
| --- | --- | --- | --- |
| README, configuration docs, provider docs, and changelog agree | docs steward | ship | #2845 (`e22a7da53`) aligned README/config example/changelogs with the HarnessProfile cutline and removed stale `V0_9_0_EXECUTION_MAP` links. |
| Breaking changes, deprecations, and deferred v0.9 gates are listed in release notes | release steward | ship | Changelog and this matrix list deferred Model Lab/Hugging Face native Hub work, `codebase_search`, remote workbench runtime, WhaleFlow live runtime execution, HarnessProfile runtime selection, large-repo startup smoke, long-running command live-state smoke, and Arcee live smoke. `.github/workflows/release.yml` release-body text avoids stale v0.8.x-only shim wording and keeps CodeWhale as the canonical package/asset name. |
| Upgrade steps exist for users coming from `deepseek-tui` | docs steward | ship | `docs/REBRAND.md` documents npm/Cargo migration commands, legacy state fallback, binary/package/asset naming, and the v0.9.0 compatibility cutline. |
| Rollback steps exist for npm wrapper, Cargo install, and side-git restore | release steward | ship | `docs/INSTALL.md#roll-back-to-a-previous-release` and `docs/RELEASE_RUNBOOK.md#recovery-and-rollback` document pinned npm rollback, pinned Cargo rollback for both crates, exact-tag manual asset restore with checksums, and side-git `/restore list [N]` / `/restore <N>` workspace rollback. |
| Live GitHub Release body has its own contributor/credit section | release steward | post-tag/pre-npm gate | `.github/workflows/release.yml` now creates a dedicated `## Contributors` release-body section with v0.9 contributor, reporter, helper, and harvested-PR credits. The live v0.9.0 Release does not exist yet, so this remains a release-time verification gate before npm publish or completion. |
| Contributors/reporters/helpers from harvested PRs and linked issues are credited | release steward | ship local changelog / verify live body at release time | Changelog credits include harvested PR authors, issue reporters/helpers, and external/co-authored work including @Implementist, @jrcjrcc, and @punkcanyang. `python3 scripts/check-coauthor-trailers.py --author-map .github/AUTHOR_MAP --range origin/main..HEAD --check-authors` remains the local co-author-map gate; live release-body credits are covered by the row above. |

## Before Tagging

- [ ] Every `ship` row has evidence.
- [ ] Every `decide` row is changed to either `ship` with evidence or `defer`
      with an owner and linked follow-up.
- [ ] Every `manual smoke required` row has dated smoke evidence, or is changed
      to an explicit defer decision with an owner and linked follow-up.
- [ ] Draft integration PR CI is green on the exact commit that will be tagged.
- [ ] The release prompt points new agents to this matrix before any tag,
      publish, or GitHub Release action.
