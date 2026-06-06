# v0.9.0 Release Acceptance Matrix

This matrix is the pre-tag gate for v0.9.0. Do not tag or publish v0.9.0 until
each row is checked off or has an explicit defer decision with an owner.

For every manual smoke, record the date, OS, provider/model, command, redacted
config source, result, and follow-up issue or PR.

## Core Build And Packaging

| Gate | Owner | Ship/defer decision | Evidence |
| --- | --- | --- | --- |
| `cargo fmt --all -- --check` | release steward | ship |  |
| `cargo check --workspace --all-targets --locked` | release steward | ship |  |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | release steward | ship |  |
| `cargo test --workspace --all-features --locked` | release steward | ship |  |
| `./scripts/release/check-versions.sh` | release steward | ship | Passed locally during #2845 (`e22a7da53`) and remains part of the PR-local release gate for each stewardship slice. |
| `./scripts/release/check-ohos-deps.sh` | release steward | ship | Passed locally during #2845 (`e22a7da53`); OHOS dependency graph stayed compatible for `codewhale-tui` on `aarch64-unknown-linux-ohos`. |
| `./scripts/release/publish-crates.sh dry-run` | release steward | ship |  |
| `node scripts/release/npm-wrapper-smoke.js` after release build | release steward | ship |  |
| GitHub release asset verification before npm publish | release steward | ship |  |

## Provider, Model, And Auth

| Gate | Owner | Ship/defer decision | Evidence |
| --- | --- | --- | --- |
| DeepSeek V4 direct provider smoke | provider steward | ship |  |
| Xiaomi MiMo token-plan and pay-as-you-go config smoke | provider steward | ship |  |
| Arcee Trinity Thinking route smoke or explicit defer | provider steward | decide |  |
| Hugging Face provider route and MCP concept helpers ship; native Hub search/passports are deferred | model-lab steward | ship foundation / defer native search-passport runtime | `ProviderKind::Huggingface`, env aliases, picker/docs, and `/hf concepts` / `/hf mcp status` distinguish the chat provider route from Hugging Face MCP and explicit Hub tooling. `docs/PROVIDERS.md` states native Hub HTTP search/passport picker metadata are not shipped behavior in this checkout; #2705/#2707/#2712 remain open for native Model Lab work. |
| OpenRouter, Novita, Fireworks, and Volcengine env behavior smoke | provider steward | ship |  |
| Provider registry drift check covers aliases/default env keys | provider steward | ship | #2820 (`5d491bc68`) added the metadata-only provider registry and `scripts/check-provider-registry.py`; verification included `python3 scripts/check-provider-registry.py` and `cargo test -p codewhale-config provider_ -- --nocapture`. |
| Provider-scoped TLS skip-verify remains default-off and doctor-visible | security steward | ship | #2834 (`190e9f35e`, `6269cb91f`) landed provider-scoped TLS skip verify with default-off config, doctor warnings, docs, and CLI/runtime option tests. |

## Runtime Stability

| Gate | Owner | Ship/defer decision | Evidence |
| --- | --- | --- | --- |
| Windows input/render smoke or documented manual verification | runtime steward | ship |  |
| macOS and Linux TUI startup smoke | runtime steward | ship |  |
| Large-repo startup smoke | runtime steward | ship |  |
| Sub-agent timeout/completion smoke | subagent steward | ship |  |
| Long-running command live-state smoke | runtime steward | ship |  |
| Runtime API remains token-protected for GUI clients | GUI steward | ship | #2811/#2814 documented and consumed the existing runtime token flow from the official VS Code extension; #2822 (`bb8835812`) added `GET /v1/snapshots` behind the same runtime API token middleware. |
| Snapshot/restore surfaces are read-only unless mutation semantics are tested | GUI steward | ship | #2822 (`bb8835812`) and #2828 (`293643e27`) expose restore points as read-only listing/Agent View metadata only; #2808 restore/retry/patch-undo mutation endpoints remain unmerged pending atomicity tests. |

## UI And Workflow UX

| Gate | Owner | Ship/defer decision | Evidence |
| --- | --- | --- | --- |
| First-look screen included or explicitly deferred | UX steward | decide |  |
| Slash picker readability smoke | UX steward | ship |  |
| Transcript tool-collapse smoke or explicit defer | UX steward | decide |  |
| Sidebar detail popovers smoke or explicit defer | UX steward | decide |  |
| Plan review/handoff artifact smoke | Plan steward | ship |  |
| VS Code Agent View branch/workspace visibility smoke | GUI steward | ship | #2825 (`1bacaf763`) added `workspace` / `branch` metadata to `/v1/threads/summary`; #2832 (`50b773f1d`) added read-only auto-refresh so branch/workspace changes can appear without manual refresh. |

## v0.9.0 Feature Gates

| Gate | Owner | Ship/defer decision | Evidence |
| --- | --- | --- | --- |
| WhaleFlow typed IR, mock executor, replay, TeacherReview, StudentReplay, and cutline docs are tested | WhaleFlow steward | ship | #2821/#2824/#2831/#2833/#2839/#2840/#2841 plus focused local `cargo test -p codewhale-whaleflow --locked`; #2670 closed after `cargo test -p codewhale-whaleflow starlark --locked` passed 7/7 on current stewardship head. The `rlm_cache_change.star` dogfood workflow now has recorded mock-trace replay coverage, including a missing-record divergence check. |
| Live `workflow_run`, worktree application, provider calls, and TraceStore writes are deferred until cancellation/replay/atomicity semantics pass | WhaleFlow steward | defer | #2669 and #2679 remain open for live runtime execution, provider calls, TraceStore writes, Arcee/student replay, and CLI/TUI workflow mode; current v0.9 branch ships mock executor/replay foundations only. |
| Model Lab / Hugging Face MVP is included or deferred with release-note wording | model-lab steward | ship provider/MCP docs foundation / defer native Model Lab MVP | v0.9 ships the Hugging Face chat-provider route, provider docs, and `/hf` concept/MCP status helpers only. Native Hub search, model passports, Spaces/Jobs workflows, and Model Lab eval/export surfaces remain deferred to #2705/#2707/#2710/#2712/#2727. |
| HarnessProfile runtime MVP is deferred; schema/resolver foundation ships with release-note wording | harness steward | ship foundation / defer runtime | #2844 (`efbcc681a`) documents the cutline; `HarnessPosture` / `HarnessProfile` config schema and strict validation are present; a pure resolver matches provider/model routes without changing runtime behavior; seed-profile runtime selection, telemetry, and status display remain follow-up work. |
| `codebase_search` MVP is included or deferred with release-note wording | search steward | defer runtime / ship design doc | `docs/CODEBASE_SEARCH_DESIGN.md` is explicitly doc-only and says no catalog code ships in this cycle; runtime tool registration, index/eval fixtures, and search implementation remain deferred to #2680. |
| External memory remains explicit/optional per `WHALEFLOW_EXTERNAL_MEMORY.md` | memory steward | ship | #2842 (`a7052751e`) added the external-memory cutline: optional/explicit workflow node/plugin only, visible state/owner/storage/scope, and no hidden default context substrate. |

## Remote Workbench

| Gate | Owner | Ship/defer decision | Evidence |
| --- | --- | --- | --- |
| Remote workbench is marked included, experimental, or deferred | remote steward | defer runtime / ship setup docs only | `docs/REMOTE_VM_US.md`, `docs/REMOTE_SETUP_DESIGN.md`, and `docs/TENCENT_LIGHTHOUSE_HK.md` document possible VM/Telegram/Lark setup patterns, but no v0.9 remote workbench runtime is included. |
| If included: VM install smoke passes | remote steward | defer | Not applicable while remote workbench runtime is deferred; no v0.9 VM install smoke is required before tagging. |
| If included: Telegram bridge smoke passes | remote steward | defer | Not applicable while remote workbench runtime is deferred; Telegram bridge docs remain design/setup guidance only. |
| If deferred: release notes avoid implying remote workbench availability | remote steward | ship | Acceptance matrix and changelog wording must say setup/design docs only, not a shipped remote workbench feature. |

## Docs, Migration, And Rollback

| Gate | Owner | Ship/defer decision | Evidence |
| --- | --- | --- | --- |
| README, configuration docs, provider docs, and changelog agree | docs steward | ship | #2845 (`e22a7da53`) aligned README/config example/changelogs with the HarnessProfile cutline and removed stale `V0_9_0_EXECUTION_MAP` links. |
| Breaking changes, deprecations, and deferred v0.9 gates are listed in release notes | release steward | ship |  |
| Upgrade steps exist for users coming from `deepseek-tui` | docs steward | ship |  |
| Rollback steps exist for npm wrapper, Cargo install, and side-git restore | release steward | ship |  |
| Live GitHub Release body has its own contributor/credit section | release steward | ship |  |
| Contributors/reporters/helpers from harvested PRs and linked issues are credited | release steward | ship |  |

## Before Tagging

- [ ] Every `ship` row has evidence.
- [ ] Every `decide` row is changed to either `ship` with evidence or `defer`
      with an owner and linked follow-up.
- [ ] Draft integration PR CI is green on the exact commit that will be tagged.
- [ ] The release prompt points new agents to this matrix before any tag,
      publish, or GitHub Release action.
