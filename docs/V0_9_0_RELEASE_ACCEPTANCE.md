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
| `./scripts/release/check-versions.sh` | release steward | ship |  |
| `./scripts/release/check-ohos-deps.sh` | release steward | ship |  |
| `./scripts/release/publish-crates.sh dry-run` | release steward | ship |  |
| `node scripts/release/npm-wrapper-smoke.js` after release build | release steward | ship |  |
| GitHub release asset verification before npm publish | release steward | ship |  |

## Provider, Model, And Auth

| Gate | Owner | Ship/defer decision | Evidence |
| --- | --- | --- | --- |
| DeepSeek V4 direct provider smoke | provider steward | ship |  |
| Xiaomi MiMo token-plan and pay-as-you-go config smoke | provider steward | ship |  |
| Arcee Trinity Thinking route smoke or explicit defer | provider steward | decide |  |
| Hugging Face route/search/passport smoke or explicit defer | model-lab steward | decide |  |
| OpenRouter, Novita, Fireworks, and Volcengine env behavior smoke | provider steward | ship |  |
| Provider registry drift check covers aliases/default env keys | provider steward | ship |  |
| Provider-scoped TLS skip-verify remains default-off and doctor-visible | security steward | ship |  |

## Runtime Stability

| Gate | Owner | Ship/defer decision | Evidence |
| --- | --- | --- | --- |
| Windows input/render smoke or documented manual verification | runtime steward | ship |  |
| macOS and Linux TUI startup smoke | runtime steward | ship |  |
| Large-repo startup smoke | runtime steward | ship |  |
| Sub-agent timeout/completion smoke | subagent steward | ship |  |
| Long-running command live-state smoke | runtime steward | ship |  |
| Runtime API remains token-protected for GUI clients | GUI steward | ship |  |
| Snapshot/restore surfaces are read-only unless mutation semantics are tested | GUI steward | ship |  |

## UI And Workflow UX

| Gate | Owner | Ship/defer decision | Evidence |
| --- | --- | --- | --- |
| First-look screen included or explicitly deferred | UX steward | decide |  |
| Slash picker readability smoke | UX steward | ship |  |
| Transcript tool-collapse smoke or explicit defer | UX steward | decide |  |
| Sidebar detail popovers smoke or explicit defer | UX steward | decide |  |
| Plan review/handoff artifact smoke | Plan steward | ship |  |
| VS Code Agent View branch/workspace visibility smoke | GUI steward | ship |  |

## v0.9.0 Feature Gates

| Gate | Owner | Ship/defer decision | Evidence |
| --- | --- | --- | --- |
| WhaleFlow typed IR, mock executor, replay, TeacherReview, StudentReplay, and cutline docs are tested | WhaleFlow steward | ship |  |
| Live `workflow_run`, worktree application, provider calls, and TraceStore writes are included only if cancellation/replay/atomicity semantics pass | WhaleFlow steward | decide |  |
| Model Lab / Hugging Face MVP is included or deferred with release-note wording | model-lab steward | decide |  |
| HarnessProfile runtime MVP is deferred; schema/config foundation ships with release-note wording | harness steward | ship foundation / defer runtime | #2844 (`efbcc681a`) documents the cutline; `HarnessPosture` / `HarnessProfile` config schema and strict validation are present; resolver, seed-profile runtime selection, telemetry, and status display remain follow-up work. |
| `codebase_search` MVP is included or deferred with release-note wording | search steward | decide |  |
| External memory remains explicit/optional per `WHALEFLOW_EXTERNAL_MEMORY.md` | memory steward | ship |  |

## Remote Workbench

| Gate | Owner | Ship/defer decision | Evidence |
| --- | --- | --- | --- |
| Remote workbench is marked included, experimental, or deferred | remote steward | decide |  |
| If included: VM install smoke passes | remote steward | decide |  |
| If included: Telegram bridge smoke passes | remote steward | decide |  |
| If deferred: release notes avoid implying remote workbench availability | remote steward | decide |  |

## Docs, Migration, And Rollback

| Gate | Owner | Ship/defer decision | Evidence |
| --- | --- | --- | --- |
| README, configuration docs, provider docs, and changelog agree | docs steward | ship |  |
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
