# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.58] - 2026-06-11

### Added

- **Native Anthropic provider.** A dedicated Messages API adapter
  (`/v1/messages` with `x-api-key` auth) replaces OpenAI-dialect shims for
  Claude models: adaptive thinking with `output_config.effort` shaping,
  prompt-cache breakpoints (capped at 4, earliest dropped), signed-thinking
  replay via `signature_delta`, normalized cache-hit/miss usage telemetry,
  and SSE error envelopes. `claude-opus-4-8`, `claude-sonnet-4-6`, and
  `claude-haiku-4-5` join the model registry; configure with
  `ANTHROPIC_API_KEY` (#3014).
- **Hooks v2.** `tool_call_before` hooks can now return a JSON decision —
  `{"decision": "allow"|"deny"|"ask", "reason", "updatedInput",
  "additionalContext"}` — with deny > ask > allow precedence across multiple
  hooks, last-writer-wins input rewriting, and concatenated context. Exit
  code 2 remains a legacy hard deny. Hooks support glob matchers and
  project-local `.codewhale/hooks.toml` (#3026).
- **Clickable sidebar.** Background-job rows show/cancel on click, the
  Ctrl+K hint row runs `/jobs cancel-all`, and agent rows open `/subagents`;
  row actions are built in the same pass as the rendered lines so a click
  can never target the wrong job (#3028).
- OSC 8 out-of-band hyperlink infrastructure with per-region open/close
  sequences that survive partial redraws (#3029).
- `codewhale exec` gains `--allowed-tools`, `--disallowed-tools` (deny wins),
  `--max-turns`, and `--append-system-prompt` (#3027).
- Constitution prompt source: YAML source-of-truth plus Python renderer for
  the system prompt, with the active prompt now served from
  `constitution.md` (#3015, renderer reconciliation still tracked).
- Agent-task issue template, labels, and runner protocol (#3021); remote
  smoke-test droplet loop hardening — gh CLI, swapfile, agent sessions
  (#3022).

### Changed

- **Sub-agent routing is provider-aware.** DeepSeek ids are no longer
  hardcoded into model validation; routing works from per-provider
  big/cheap candidates, the network router is skipped when a provider has
  no cheap tier, and spawn-time model requests are validated against the
  active provider (#3018).
- Model-specific facts in the system prompt (context window, sub-agent
  pricing, thinking notes, architecture characteristics) are now templated
  per-model instead of hardcoded DeepSeek V4 claims, in both `base.md` and
  `constitution.md` (#3025).
- Provider capability lookups for Moonshot/OpenAI/Atlascloud resolve from
  per-model registry rows (bare and vendor-prefixed ids) instead of
  hardcoded 64K-era floors (#3023).
- Reasoning-effort now reaches Atlascloud (DeepSeek dialect), Moonshot
  (`thinking` enable/disable), and Ollama (`think` param) (#3024); Moonshot/
  Kimi models joined the reasoning-content provider and model gates (#3016).
- Transcript polish: compact tool-call cells without boilerplate (#3031),
  internal turn/agent ids hidden behind stable labels (#3030), and Ctrl+B
  now backgrounds the running foreground shell directly instead of opening
  a menu (#3032).
- The Tasks sidebar separates "Model reasoning" from "Background commands",
  and `auth list` reports the same active-credential source as
  `auth status` for openai-codex.

### Fixed

- **TUI freeze under sub-agent load.** Rapid `AgentProgress` events
  saturated the render loop and starved terminal input; progress-driven
  repaints are now throttled to one per 100ms (#3033).
- **Hooks on Windows.** Hook commands were passed to `cmd /C` through
  CRT-style argument quoting, which injected literal `\"` sequences that
  cmd.exe never unescapes — JSON decisions could not parse. Commands now
  reach cmd.exe verbatim via `raw_arg`.
- Codex Responses: assistant tool results are converted to
  `function_call_output` items (multi-turn tool calling previously broke),
  tool schemas are sanitized for the Responses API, and `maximum` effort
  maps to `xhigh` (#3019, #3017 — both partially; retry/backoff and
  per-tool strict mode remain open).
- Better tool-denial and provider error messages harvested from PR #2933
  (#3020).


## [0.8.57] - 2026-06-10

### Added

- **Turns now survive system sleep.** When the host suspends mid-stream, the
  connection used to die on wake with `Stream read error: error decoding
  response body` and the turn was lost (#2990). The engine now stamps stream
  progress with both monotonic and wall-clock time; a large divergence on a
  stream error identifies a sleep/wake cycle, and the request is silently
  re-issued (up to the existing 3-retry budget) instead of failing the turn.
- **One-command release prep.** `./scripts/release/prepare-release.sh X.Y.Z`
  bumps the workspace version, every internal crate dependency pin, the npm
  wrapper, and the README install-tag examples, refreshes `Cargo.lock`,
  regenerates the embedded TUI changelog slice and web facts, and runs
  `check-versions.sh` — the v0.8.56 release needed nine follow-up commits for
  exactly these sync points.
- `.github/CODEOWNERS` and `.github/dependabot.yml` (weekly cargo +
  github-actions updates, monthly npm for `web/`).

### Changed

- **The changelog went on a diet.** Root `CHANGELOG.md` now carries recent
  releases (v0.8.40+); older entries moved to `docs/CHANGELOG_ARCHIVE.md`.
  `crates/tui/CHANGELOG.md` — embedded into every binary for `/change` — is a
  generated 15-release slice (`scripts/sync-changelog.sh`), no longer a
  357 KB manual byte-for-byte copy (~300 KB smaller binaries).
- GitHub Release bodies are generated from the tagged version's changelog
  section (`scripts/release/generate-release-body.sh`) instead of a
  hardcoded workflow blob with a hand-pasted contributor list.
- `check-versions.sh` now also gates `web/lib/facts.generated.ts` and the
  README install-tag examples; the CNB mirror pipeline validates the pushed
  tag against `Cargo.toml` before generating release notes.
- Docs reorganized: internal design notes moved under `docs/rfcs/`; stale
  internal docs (old audits, handoffs, region-specific VM notes) removed.
- Agent-facing polish: the system prompt environment block reports
  `codewhale_version` (was `deepseek_version`), the legacy
  `.deepseek/instructions.md` path is no longer advertised in the prompt
  (still honored for back-compat), and oversized instruction files are
  truncated with an explicit `[…truncated: N bytes omitted]` marker instead
  of a bare ellipsis.

### Fixed

- **Docker images build again.** The release `docker` job failed for v0.8.56
  because the Dockerfile still copied the pre-rebrand `deepseek` /
  `deepseek-tui` binaries; they are now symlinks to the codewhale binaries
  inside the image, so legacy container entrypoints keep working.
- `.devcontainer/devcontainer.json` used the pre-rebrand container name,
  mount path, and `deepseek` remote user.
- Stale `--bin deepseek` examples, `DeepSeek-TUI` strings in `/change`
  output, and pre-rebrand doc comments.

### Removed

- Unused dependencies: `tracing-appender` and `zeroize` (TUI crate),
  `rustls` (release crate); the orphaned `vendor/schemaui-0.12.0` lockfile
  leftover and a machine-specific one-off `scripts/verify_task.sh`.

## [0.8.56] - 2026-06-09

### Added

- **Status picker localization.** The status picker surface (7 MessageIds) is
  now localized across all supported locales (#2896, @gordonlu).
- **Approval dialog localization.** The approval dialog surface is now
  localized across 7 locales: English, Simplified Chinese, Japanese,
  Vietnamese, Portuguese, Spanish, and French (#2891, @gordonlu).
- **Volcengine provider in TUI dispatcher.** The `codewhale` / `codewhale-tui`
  CLI dispatcher now allows the Volcengine provider, so users can launch
  directly into a Volcengine-backed session (#2923, @hongchen1993).
- **Dispatcher API-key preference.** When a provider-specific API key is
  supplied via the CLI dispatcher, it is now preferred over the saved root
  key, fixing a regression where saved keys masked explicit CLI keys (#2928,
  @hongchen1993).
- **Qwen 3.6 Plus model support.** Added complete Qwen 3.6 Plus model
  resolution with dedicated version-bump tests (#2930, @idling11).
- **Oversized paste spill.** Pastes larger than ~10 KB are now written to
  `.codewhale/pastes/` instead of being truncated or dropped, preserving the
  full content for the session (#2920, @sximelon).
- **Cross-session prompt cache.** Added a disk-backed cross-session prompt
  base-section cache so post-mode-flip and post-restart turns reuse the
  byte-stable prefix without rebuilding it from scratch.

### Fixed

- **Background shell routing.** Shell commands expected to take >5 seconds are
  now automatically guided to background tasks instead of blocking the agent
  loop, with the task panel syncing immediately on cancel (#2947, #2941,
  @cyq1017, @idling11).
- **`allow_shell` error naming.** Shell-tool refusal errors now explicitly name
  `allow_shell = false` as the reason and suggest `/config allow_shell true` as
  the escape hatch (#2905, @cyq1017).
- **Prefix-cache stability across mode flips.** `allow_shell` is now decoupled
  from the static system-prompt prefix, so mode changes (Plan ↔ Agent ↔ YOLO)
  no longer rebuild the byte-stable message[0] and invalidate the DeepSeek
  prefix cache (#2949, @LeoAlex0).
- **`visibility="internal"` explained.** The Runtime Policy Reference section
  of the system prompt now explains the `visibility="internal"` attribute so
  models stop narrating their current mode between steps (#2951, @LeoAlex0).
- **Bocha web search response handling.** Updated response parsing for the
  Bocha search backend after an upstream API change (#2946, @h3c-hexin).
- **PDF read hang.** Full-PDF reads now use `extract_text_by_pages` to avoid
  a hang on large or complex PDFs (#2898, @idling11).
- **9 critical bugs.** Fixed bugs across tools, client, and commands: stale
  `ContentBlockStop` cleanup, missing `#[test]` attribute, trailing-space
  restoration on English `ApprovalField` labels, and several
  correctness/stability issues (#2880, @HUQIANTAO).

### Changed

- **CNB shim cleanup.** Removed deprecated `deepseek` shim references from the
  CNB mirror path.
- **Style.** Applied `cargo fmt` to `crates/tools/src/file.rs`.

## [0.8.55] - 2026-06-08

### Added

- **Together AI provider.** Added Together AI as a first-class provider
  (`[providers.together]`, `TOGETHER_API_KEY`/`TOGETHER_BASE_URL`/`TOGETHER_MODEL`)
  with default models `deepseek-ai/DeepSeek-V4-Pro` and
  `deepseek-ai/DeepSeek-V4-Flash`, TUI provider-picker/auth/capability support,
  and CLI `auth list`/`auth status` coverage.
- **Model catalog updates.** Added Qwen 3.7 Max (`qwen/qwen3.7-max`), MiniMax 2.7
  (`minimax/minimax-2.7`), and NVIDIA Nemotron 3 Ultra (`nvidia/nemotron-3-ultra`)
  on OpenRouter.
- **OpenAI Codex (ChatGPT) provider — experimental.** Added an `openai-codex`
  provider that reuses an existing ChatGPT/Codex CLI OAuth login. The access
  token is read and refreshed from `~/.codex/auth.json` (no API key is stored),
  and requests use the OpenAI Responses API at `/codex/responses` with the
  `chatgpt-account-id` header and `responses=experimental` beta opt-in. Env
  overrides: `OPENAI_CODEX_ACCESS_TOKEN`/`CODEX_ACCESS_TOKEN`,
  `OPENAI_CODEX_BASE_URL`/`CODEX_BASE_URL`, `OPENAI_CODEX_MODEL`/`CODEX_MODEL`,
  `OPENAI_CODEX_ACCOUNT_ID`/`CODEX_ACCOUNT_ID`, `OPENAI_CODEX_AUTH_FILE`,
  `CODEX_HOME`. Default model `gpt-5.5`. The live Responses round-trip has not
  been exercised against the production backend in CI; treat as preview.

## [0.8.54] - 2026-06-08

### Added

- **Benchmark harness runners.** Added CodeWhale-native benchmark entry points for SWE-bench, Terminal-Bench, and PinchBench, plus a local PinchBench runner that can grade tool-use traces with an LLM judge.
- **Direct MiMo benchmark routing.** The benchmark runner now defaults to direct Xiaomi MiMo v2.5 Pro routing when configured, while keeping provider/model selection explicit.
- Added `/restore list [N]` so users can inspect more side-git rollback
  snapshots with UTC timestamps before choosing a restore point. Plain
  `/restore` now shows the 20 most recent snapshots, numeric restore targets can
  reach beyond that default listing up to a bounded index, and list requests
  above the visible cap fail explicitly instead of silently truncating.
- Added HarmonyOS/OpenHarmony support scaffolding: environment-driven
  `OHOS_NATIVE_SDK` setup scripts and compiler wrappers, platform docs,
  explicit Rustls ring-provider installation for the no-provider TLS build, and
  OHOS fallbacks for unsupported keyring, clipboard, sandbox, browser-open, TTY,
  execpolicy Starlark parsing, and self-update surfaces.
- Added `scripts/release/check-ohos-deps.sh` and wired it into CI/release
  preflight so the OpenHarmony target graph fails if unsupported `nix`,
  `portable-pty`, `starlark`, `arboard`, or `keyring` dependencies re-enter.
- Added `.github/AUTHOR_MAP` and a CI co-author credit check so harvested
  commits use GitHub-mappable numeric noreply identities instead of `.local`,
  placeholder, bot/tool, or raw third-party emails.
- Added a `turn_end` observer hook that fires after post-turn TUI state and
  token totals are updated. Hooks receive structured JSON with status, usage,
  totals, duration, tool count, and queued-message count on stdin; stdout is
  ignored and failures are warn-only (#1364, #2578).
- Added provider-scoped `insecure_skip_tls_verify` for private
  OpenAI-compatible gateways that cannot use a trusted CA bundle. The setting is
  disabled by default, applies only to the active LLM provider HTTP client, and
  is surfaced by `codewhale doctor`; `SSL_CERT_FILE` remains the preferred path
  for corporate or private CA roots. Thanks @wavezhang for the original #1893
  direction.
- Added a default-disabled hard-compaction planner that can identify the
  summarizable middle of a long conversation while preserving the recent tail,
  existing tool-call/result pair guarantees, and working-set pinning. This
  harvests the safe planning layer from #2522 without enabling hard compaction
  or adding a message-rewrite execution path yet. Thanks @HUQIANTAO for the
  proposal.
- Added rich PlanArtifact support to `update_plan`: Plan mode can now carry
  grounded objectives, context, sources, critical files, constraints,
  verification, risks, and handoff notes through the transcript card, Plan
  confirmation prompt, `/relay`, fork-state, and saved-session replay.
- Added the first `codewhale-whaleflow` foundation crate with typed workflow
  config/IR validation and deterministic phase ordering tests. This preserves
  the WhaleFlow direction from #2482/#2486 without exposing a runtime
  `workflow_run` tool until cancellation, replay, and worktree semantics are
  release-safe. The foundation now includes explicit `WorkflowSpec`,
  `WorkflowNode`, branch/leaf/policy metadata structs, plus serializable branch,
  leaf, and control-node result records toward the #2668 TraceStore contract.
  It also adds a crate-local mock executor skeleton for Sequence, BranchSet,
  Leaf, Reduce, LoopUntil, Cond, Expand, BranchTournament, and ParetoFrontier
  control flow so #2669 can progress without spawning agents, applying
  worktrees, or exposing a `workflow_run` runtime tool yet. A first Starlark
  authoring layer now compiles fail-closed model-authored workflow files into
  that typed IR, with `rlm_cache_change.star` and `issue_fix_tournament.star`
  examples plus a one-pass repair for common `ctx.*` authoring aliases (#2670).
  Leaf, branch, and workflow execution results now carry deterministic token
  and cost telemetry fields that the mock executor can aggregate without live
  provider calls or runtime sub-agent fanout (#2486). The mock executor now
  carries crate-local cancellation and budget-exhaustion status markers so the
  branch/leaf runtime contract can be tested before live workflow execution is
  exposed (#2669). A crate-only replay executor now evaluates workflows from
  recorded leaf/control records, computes
  stable SHA-256 leaf input hashes, and marks missing records as
  `replay_diverged` instead of calling models again (#2673); the runtime replay
  command and live-provider replay fallback remain deferred. The crate also now
  has a model-agnostic role/capability registry with mock provider plumbing and
  fail-closed JSON repair parsing, so WhaleFlow can choose capable models for
  roles without hardcoding provider-specific runtime paths (#2672). The
  `rlm_cache_change.star` dogfood workflow now exercises candidate branches,
  LoopUntil verification, tournament selection, teacher review, and mock
  execution in CI-oriented crate tests (#2679). Leaf, branch, and workflow
  results now also carry separate ARMH/shared-memo and provider prompt-cache
  telemetry counters, with mock aggregation tests, so #2671 can progress
  without wiring live RLM calls or billing-affecting provider behavior yet. The
  Starlark and typed-IR gates now also reject unknown leaf dependencies,
  reducer inputs, and teacher-review candidates before mock execution or replay,
  keeping generated workflows fail-closed while runtime/worktree semantics stay
  deferred. TeacherReview now has serializable GEPA-style candidate artifacts
  for notes, workflow recipes, skills, regression tests, cache policy, branch
  heuristics, and Starlark authoring prompt patches, plus an offline helper
  that proposes candidates from recorded execution traces without promoting
  them or training model weights (#2674). StudentReplay results can now be
  stored on teacher candidates, and a deterministic PromotionGate compares
  baseline-vs-candidate replay deltas, required tests, policy violations,
  staleness, and cost constraints before marking a candidate promotable (#2675).
  The external-memory cutline now documents that Aleph-style memory stays
  optional, explicit, visible, and clear/export-capable for v0.9.0 rather than
  becoming a hidden default context substrate (#2677).
  A dedicated v0.9.0 release acceptance matrix now tracks provider, runtime,
  UI, WhaleFlow, Model Lab, remote-workbench, docs, rollback, and credit gates
  that must be checked or explicitly deferred before tagging (#2729).
  HarnessProfile docs now pin the v0.9.0 order: posture/schema/resolver/seed
  profiles/status display must precede evidence stores, promotion gates, or any
  automatic Harness Creator, with DeepSeek, MiMo, Arcee, and generic/HF/local
  posture expectations called out separately (#2728).
  Hugging Face / Model Lab and `codebase_search` release gates now explicitly
  ship only the provider/MCP/docs/design foundation in v0.9; native Hub search,
  model passports, Spaces/Jobs workflows, eval/export surfaces, and runtime
  `codebase_search` registration remain deferred (#2705, #2680, #2727).
  Remote workbench acceptance is also marked docs/setup-only for v0.9 so release
  notes do not imply a shipped VM or Telegram bridge runtime (#2724).
  Release-facing HarnessProfile docs now match the current implementation:
  v0.9 ships the typed schema/config foundation and defers runtime resolver,
  telemetry, seed-profile selection, and status-display behavior until later
  verified slices. `config.example.toml` includes a commented dormant
  harness-profile example, and README links point at the real acceptance matrix
  and HarnessProfile cutline docs.
  The release acceptance matrix now records evidence for already-landed gates:
  provider-registry drift checks, provider-scoped TLS skip verify, read-only
  GUI runtime/restore-point surfaces, VS Code Agent View branch visibility,
  WhaleFlow mock/runtime foundations, explicit external-memory boundaries, and
  docs alignment. Live workflow execution, provider calls, TraceStore writes,
  and mutation-oriented GUI endpoints remain deferred until their atomicity and
  replay contracts are tested. The `rlm_cache_change.star` dogfood workflow can
  now be replayed from recorded mock leaf/control records, and missing dogfood
  records produce `ReplayDiverged` instead of falling back to live execution
  (#2679). The UI/workflow UX rows now also distinguish shipped transcript
  tool-run collapse, sidebar detail popovers, and PlanArtifact review/handoff
  evidence from the deferred first-look/home redesign, and record focused
  slash-picker readability smoke coverage for visibility, selection, skill
  insertion, Esc priority, and stable composer height (#2692, #2694, #2691,
  #2713).
  Thanks @AdityaVG13 for the WhaleFlow draft and cost-tracking direction.
- Added a state-store v2 schema migration for WhaleFlow trace tables covering
  workflow, branch, leaf, control-node, and teacher-candidate runs. The
  migration creates persistence shape only; workflow execution and replay
  remain deferred until the runtime semantics are safe (#2668).
- Added an official VS Code extension Phase 0 scaffold with terminal launch,
  local runtime attach checks, status bar state, and a read-only Agent View
  preview backed by recent runtime thread summaries, plus a read-only
  `GET /v1/snapshots` endpoint for GUI clients to inspect side-git restore
  points. The extension now renders those restore points read-only in its Agent
  View, and thread summaries include read-only workspace, branch, current Git
  head, and dirty-state metadata so the VS Code Agent View can show when a
  thread or agent lane is on another branch or has changed worktree state. Agent
  View and restore-point data now auto-refresh on a configurable
  read-only interval so branch/workspace/status changes become visible without a
  manual refresh. Agent View refreshes keep thread branch/workspace rows
  independent from restore-point loading, so a snapshot-listing failure no
  longer clears already-available thread metadata. This answers the VS Code GUI
  lane without exposing chat webviews, inline edits, or retry/undo/restore
  runtime mutation endpoints yet
  (#461, #462, #480, #1217, #2341, #1584, #2327, #2580, #2808). Thanks @AiurArtanis
  for the Agent View prompt, @lbcheng888 for the earlier scaffold, @gaord for
  the GUI runtime API direction, @douglarek, @caeserchen, and @nightt5879 for
  the branch visibility trail, and @BigBenLabs, @lzx1545642258, @yangdaowan,
  @mangdehuang, @VerrPower, @hejia-v, @nasus9527, and @ygzhang-cn for the
  GUI/VS Code demand and validation trail.
- Added inline live-output refresh for background shell Exec cards keyed by the
  exact shell task id, so long-running commands can show bounded stdout/stderr
  tails without consuming deltas or matching by command text. Thanks
  @donglovejava for the live shell-output direction in #2048.
- Added a static prompt composer override for embedders that need to replace
  the byte-stable base/personality prompt segment while leaving mode metadata,
  approval policy, tool taxonomy, Context Management, and the Compaction Relay
  under CodeWhale's runtime prompt assembly. This refines the embedder prompt
  customization path from #2786 without weakening prompt-continuity safeguards.
  Thanks @h3c-hexin.
- Added `POST /v1/sessions` for runtime clients to save a completed thread as a
  managed session. The endpoint preserves thread title/model/mode/workspace
  metadata, maps missing threads to 404, and returns 409 instead of snapshotting
  queued or active turns.
- Added cost-estimate pricing for the Xiaomi MiMo primary chat models, which
  were previously unpriced: `mimo-v2.5-pro` / `xiaomi/mimo-v2.5-pro` reuse the
  DeepSeek V4-Pro rate table and `mimo-v2.5` / `xiaomi/mimo-v2.5` reuse the
  DeepSeek V4-Flash rates. Existing DeepSeek pricing is unchanged (#2731, #2750).
- Added a metadata-only `codewhale-config` provider registry with canonical
  lookup, alias-aware resolution, provider defaults, config-table keys, and
  API-key env candidates. Runtime routing remains unchanged and fallback
  providers stay dormant; this harvests the safe provider-trait foundation from
  #2479 toward #2075. Thanks @sximelon.
- Added optional `[search].base_url` / `CODEWHALE_SEARCH_BASE_URL` support for
  DuckDuckGo-compatible private search endpoints, while keeping
  `DEEPSEEK_SEARCH_BASE_URL` as a legacy alias. Custom endpoints are gated by
  their configured host, do not fall back to public Bing, and report the custom
  host as the result source for diagnostics (#2436, #2510).
- Added `completion_sound = "file"` with `[notifications].sound_file` so
  Windows users can play a custom WAV file for turn-completion sounds without
  changing the global Windows sound scheme (#2484, #2512).
- Added `[tui].stream_chunk_timeout_secs` and `/config stream_chunk_timeout_secs`
  so slow local or OpenAI-compatible model servers can extend the SSE idle
  timeout without mutating process environment. The legacy
  `DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS` env var remains a fallback (#2365, #2507).
- Added dormant `fallback_providers = [...]` config parsing plus a provider-chain
  helper for future fallback routing. This preserves the requested contract
  without enabling silent runtime provider switches yet (#2574, #2777). Thanks
  @hsdbeebou for the request and @idling11 for the data-model draft.
- Added `/hf` with `/huggingface` alias for Hugging Face MCP status/setup
  helpers and `/hf concepts` provider/MCP/Hub guidance. The helper points users
  to Hugging Face's settings-generated MCP configuration and intentionally does
  not include Hub search, direct Hugging Face HTTP requests, or upload behavior
  (#2709, #2782). Thanks @idling11 for the original Hugging Face MCP draft.
- Added an in-process response cache for deterministic non-streaming,
  tool-free chat requests. The cache is keyed by provider, base URL, path
  suffix, API-key fingerprint, and final wire body, and zeroes usage on hits so
  local spend counters are not double-counted (#2501). Thanks @HUQIANTAO for
  the response-cache proposal and canonical-body key update.
- Added `/sidebar` so users can toggle, show, hide, and optionally persist the
  TUI sidebar from the command line instead of relying on copy-hostile sidebar
  state during long transcript work (#2766, #2788). Thanks @mo-vic for the
  detailed report and @aboimpinto for the fix.
- Added a pausable custom slash-command MVP: commands with `pausable: true`
  can pause before further tool execution, preserve the paused command while
  separate messages are handled, and resume only on explicit continue/resume
  wording. Harvested from #2732 with thanks to @aboimpinto.
- Added Sofya (`provider = "sofya"`) as a search-tool backend with
  `SOFYA_API_KEY` fallback, while keeping Sofya scoped to web search rather
  than model-provider routing (#2790). Thanks @yusufgurdogan for the
  implementation.
- Added Xiaomi MiMo `mode` / `XIAOMI_MIMO_MODE` / `MIMO_MODE` selection for
  Token Plan region endpoints and pay-as-you-go routing, plus dedicated Token
  Plan env keys for `tp-*` subscriptions (#2621, #2627). Thanks @springeye for
  the request and @xyuai for the implementation.
- Added the first TUI hotbar action registry foundation so future UI controls
  can dispatch typed app actions instead of growing another command match
  surface (#2866). Thanks @reidliu41 for the implementation.
- Added the narrow multi-tab core and persistence foundation, including tab
  manager snapshots, delegation/group restore counters, mention parsing,
  cross-tab events, and corruption-tolerant persisted state, while leaving the
  broader collaboration UI wiring to follow-up work (#2864). Thanks
  @ljm3790865 for the tab-core implementation and #2753 direction.
- The VS Code Agent View now renders the runtime thread summary's Git `head`
  and dirty-worktree flag alongside branch metadata, keeping branch switches
  visible without adding retry/undo/restore mutation endpoints yet (#2580,
  #2862). Thanks @AiurArtanis and @nasus9527 for the IDE/agent-view requests
  and @gaord for the runtime metadata direction.

### Changed

- Removed the deprecated `deepseek` and `deepseek-tui` binary shims from the
  v0.9.0 Cargo crates and GitHub release artifact matrix. The canonical
  `codewhale`, `codew`, and `codewhale-tui` entry points remain, the private
  deprecated `npm/deepseek-tui` notice package stays unpublished, and DeepSeek
  provider/model/env/config compatibility remains first-class.
- Command-adjacent config persistence and auto model routing now live in
  neutral TUI modules instead of command-owned files, reducing command-boundary
  coupling while preserving current `/config`, `/model`, UI, runtime, and
  sub-agent behavior (#2871). Thanks @aboimpinto for landing this first staged
  command-boundary layer from the broader #2851/#2791 design direction.
- `/config` now reports the canonical `~/.codewhale/settings.toml` path for TUI
  settings while still reading legacy DeepSeek-branded settings fallbacks and
  migrating them into the CodeWhale home on load.
- Provider switches now roll back transactionally when the first request to a
  newly selected provider fails authentication: CodeWhale restores the previous
  provider/model, model-ID passthrough, onboarding/API-key state, runtime
  config, persisted provider selection, and engine handle so users can return
  to DeepSeek after a failed Moonshot/Kimi switch (#2754, #2755). Thanks
  @Dr3259 for the Windows repro and @cyq1017 for the draft fix.
- `PATCH /v1/threads/{id}` can now update a thread's persisted workspace for
  GUI/runtime clients. Workspace changes reject active turns and evict idle
  cached engines so the next turn starts in the new workspace.
- Split `web_run` session/page cache state so cached page reads use shared
  page handles and do not serialize through the mutation path. The harvest also
  adds panic-safe state write-back and serializes cache-mutating unit tests so
  the global web cache remains stable under normal Cargo test parallelism.
- Appended volatile `<turn_meta>` blocks after user text in outgoing user
  message content arrays so provider prefix caches can keep matching the stable
  user-input prefix across date, route, and working-set changes.
- Projected mode, approval, and tool-taxonomy prompt metadata per request
  instead of mutating stored system prompts, keeping provider prefix-cache
  inputs byte-stable while preserving mode-specific instructions (#2687).
  Thanks @LeoAlex0 for the implementation.
- Softened contribution intake automation: external issues now receive a warm
  triage note and are never auto-closed by the contribution gate, while the PR
  gate copy makes clear that dry-run observations are about maintainer safety,
  not contributor quality.
- Added a PR gate marker guard so reopened unapproved PRs do not get duplicate
  intake comments, and clarified that PR reopening should happen after
  allowlist approval is merged.
- Ollama `/model` completions no longer show hosted DeepSeek API model IDs.
  The picker preserves the current or saved local Ollama tag, and users can
  still fetch installed model IDs through `/models` instead of relying on a
  stale static default (#2742). Thanks @reidliu41 for the focused report and
  draft fix.
- MCP runtime API tool listings and approval summaries no longer split
  underscored MCP server names at the first `_`. Tool-call routing already used
  the longest registered server name; the list endpoint now reuses that parser,
  and approval cards show the full MCP target route instead of a guessed server
  segment (#2744). Thanks @lioryx, @cyq1017, and @puneetdixit200 for the report
  and matching fixes.
- Documented the agent and sub-agent stewardship ethos so future automation
  preserves human issue intake, careful PR review, and contributor credit.
- Moved the TUI Starlark execpolicy parser and PTY support behind non-OHOS
  target dependencies so published OpenHarmony builds no longer pull `nix` 0.28
  through `rustyline` or `portable-pty`.
- Explicit `skills_dir` configuration is now unioned with workspace skill
  discovery instead of being shadowed by workspace-local skills, and configured
  skills take precedence over global defaults when prompt space is constrained.
- Tool-agent sub-agent routing now inherits the parent session model, or an
  explicit tool-agent override, instead of hard-coding `deepseek-v4-flash`;
  the fast lane still disables thinking through provider-aware request shaping.
- Dense successful read/search/list tool runs now collapse into a single
  expandable transcript row by default, while running, failed, shell, patch,
  review, diff, and other risky tool cells remain visible. The setting
  `tool_collapse = "compact" | "expanded" | "calm"` controls the behavior.
- Pending-input preview rows now label delivery mode explicitly as steer
  pending, rejected steer, or queued follow-up, with wrapped continuation rows
  aligned under the label so busy-turn input state is easier to read (#2054).
- Editing a queued follow-up is now an explicit pending-input state. Pressing
  `Esc` while editing a queued follow-up restores the original queued message
  instead of cancelling the active turn or silently dropping the queued work
  (#2054).
- Approval prompts now render prominent command, directory, file, path, or
  target rows before falling back to raw JSON params. Shell approvals preserve
  long command tails, split common shell chains for review, and show compact
  `printf > file` previews while keeping intent summaries visible (#1991,
  #2269).
- Sidebar hover details now use row-level metadata for truncated Work, Tasks,
  and Agents rows. Mouse hover opens a bordered, wrapping popover with the full
  underlying row text, long turn/agent ids, and current sub-agent progress
  instead of repeating the already-ellipsized sidebar label (#2694, #2734).
- Sub-agents now preserve checkpoint metadata around long model calls. A
  per-step API timeout marks the child as interrupted with a continuable
  checkpoint instead of ending as a null failed result, and `agent_eval` can
  explicitly continue a live checkpointed interrupted child while normal
  completed/failed/cancelled follow-up behavior stays unchanged (#2029).
- Durable task recovery no longer requeues tasks that were `running` when the
  previous CodeWhale process exited. On restart those records are marked failed
  with a recovery note, and any running tool-call summaries are marked failed
  too, so stale shell/task state cannot silently become live work again (#1786).
- Auto-generated project instructions now reuse the bounded Project Context
  Pack data instead of running an unbounded summary/tree scan when no
  `.codewhale/instructions.md` file exists. The fallback keeps later
  top-level folders visible in noisy large workspaces while the dynamic
  `<project_context_pack>` marker remains controlled by its own setting
  (#697, #1827).
- Project context loading now uses a bounded process-local content-signature
  cache for repeated hot-path loads. The cache covers workspace/parent
  instructions, global AGENTS/WHALE fallbacks, repo constitution files,
  generated-context targets, trust markers, and trust config paths, and it
  stores post-load signatures so auto-generated context deletion/regeneration
  stays correct (#2636).
- Configuration docs now show the provider-local `path_suffix` escape hatch
  for OpenAI-compatible gateways that accept `/chat/completions` but reject
  `/v1/chat/completions`, while making clear that model listing and DeepSeek
  beta routes keep their built-in paths (#1874).
- The config crate now carries the v0.9 HarnessPosture data model:
  `HarnessPosture`, `HarnessProfile`, and typed posture/compaction/tool/safety
  enums. The schema rejects misspelled posture names or unknown profile keys
  instead of silently falling back to `custom`; a pure resolver can match
  provider/model routes for tests and future status plumbing, while runtime
  provider/model posture selection remains a follow-up (#2693, #2741, #2728).

### Fixed

- **Benchmark workspace copying.** Fixed benchmark workspace file copying so local benchmark tasks can preserve their intended file layout during agent runs.
- **MiMo default tests.** Guarded Xiaomi MiMo default-model tests against ambient CI provider environment variables.
- Stream/body decode failures such as `Stream read error: error decoding
  response body` are now classified as recoverable network interruptions
  instead of generic internal errors, keeping the transcript and triage metadata
  aligned with the existing stream retry path (#2847). Thanks
  @qamranmushtaq-collab for the Windows/npx DeepSeek report.
- The TUI footer, `/status`, `/mcp` manager, and command-palette MCP entries
  now count trusted workspace-local `.codewhale/mcp.json` servers together with
  the global MCP config, matching `codewhale mcp list` for merged global +
  project setups (#2787). Thanks @yekern for the detailed reproduction.
- AltGr key chords in the composer no longer get swallowed by sidebar shortcuts
  on AZERTY and other international layouts, so characters such as `@`, `#`,
  `$`, `!`, and `%` can be entered normally (#2863, #2867). Thanks
  @ousamabenyounes for the fix and report.
- Sub-agent shell completions now refresh the workspace branch/status chip
  immediately, and `/subagents` plus the Agents sidebar show each sub-agent's
  current workspace branch when it is running in a child worktree.
- Authentication failures now include redacted request context such as provider,
  base URL authority, model, key source, key type, and key fingerprint, making
  stale provider, endpoint, or API-key state diagnosable without exposing the
  secret (#2665, #2792). Thanks @mvanhorn for the implementation.
- Browser-opening actions now compile on non-desktop targets by delegating the
  unsupported-platform error to the shared URL opener instead of hiding the TUI
  wrapper behind a narrower macOS/Linux/Windows cfg. Thanks @ci4ic4 for the
  NetBSD/pkgsrc packaging report and fix (#2789).
- MCP tool routing now preserves server names that contain underscores.
  `parse_prefixed_name` matches the qualified `mcp_<server>_<tool>` name against
  the set of registered server names and prefers the longest match, so tools on
  a server like `my_db` are reachable and an overlapping `my` / `my_db` pair
  routes correctly. Falls back to the legacy first-underscore split when no
  registered server matches (#2744).
- Schema-hydrated deferred tools no longer render as a completed run. The first
  use of a deferred tool returns a schema-hydration result instead of executing;
  the transcript and sidebar now show "tool loaded — retry required" via a
  dedicated hydrated status, so it is no longer indistinguishable from a real
  successful execution. A hydrated row also ranks with active work rather than
  completed successes (#2648).
- `codewhale sessions` now shows `codewhale resume <session-id>` in the footer
  instead of the invalid dispatcher command `codewhale --resume <session-id>`
  (#2758, #2760).
- TUI HTTP clients now install the Rustls ring crypto provider before building
  `reqwest` clients, covering engine, runtime API, tool, MCP, config, and skill
  download paths. This keeps the no-provider TLS build from panicking during
  tests or embedded startup paths that do not enter through the main binary.
- Prompt byte-stability tests now pin their temporary home and skills
  environment under the shared test-env lock so global skill directories cannot
  perturb deterministic prompt bytes during parallel test runs.

### Community

Thanks to **@sximelon** for reporting and fixing the saved-session resume
footer hint (#2758, #2760), **@cyq1017** for the custom
DuckDuckGo-compatible search endpoint, custom completion sound file support,
restore-listing implementation, and pending-input delivery-mode label work
(#2510, #2512, #2513, #2532, #2054),
**@Artenx** for the private-search endpoint report (#2436),
**@LHqweasd** for the Windows custom notification sound request (#2484),
**@wywsoor** for the broader macOS/iTerm rollback UX report (#2494),
**@HUQIANTAO** for the `web_run` lock-splitting work (#2502), turn-metadata
prefix-cache stability work (#2517), and project-context cache direction
(#2636), **@xyuai** for canonical CodeWhale
settings-path migration work (#2730), **@gaord** for the runtime thread
workspace update and completed-thread save APIs (#2640, #2639),
**@shenjackyuanjie** for the
HarmonyOS/OpenHarmony port and MatePad Edge validation trail (#2634),
**@ousamabenyounes** for the AZERTY AltGr composer shortcut fix (#2863,
#2867), **@reidliu41** for the hotbar action-registry foundation (#2866), and
**@ljm3790865** for the multi-tab core/persistence foundation and broader
collaboration direction (#2864, #2753),
**@aboimpinto** for the direct command-support boundary cleanup in #2871 and
the broader #2851/#2791 command-layer design direction,
**@idling11** for the PlanArtifact direction in Plan mode (#2733), the dense
tool-call transcript collapse/sidebar detail direction (#2738, #2734, #2692,
#2694), and the HarnessPosture config model for provider/model posture (#2741,
#2693), and
**@h3c-hexin** for the tool-agent model inheritance and configured
`skills_dir` fixes (#2736, #2737), **@AresNing** for the turn-end observer hook
work (#2578), and **@tdccccc** for the approval key-detail and shell-preview
work (#1991, #2269). Thanks also to **@qiyuanlicn** for the
checkpoint/resume report that shaped the sub-agent recovery slice (#2029),
**@bevis-wong** for the long-running shell/task liveness report (#1786),
**@shuxiangxuebiancheng** for the third-party OpenAI-compatible path report
(#1874), **@hongqitai** and **@cyq1017** for the follow-up path-suffix PR
review trail (#2508, #2506), **@NASLXTO** and **@wuxixing** for the
large-workspace startup reports (#697, #1827), and **@linzhiqin2003** and
**@merchloubna70-dot** for earlier context-cap and startup-diagnosis work that
shaped this bounded fallback. Thanks also to **@cyq1017** for the MCP
underscore-server-name fix and Xiaomi MiMo pricing (#2747, #2744, #2750, #2731)
and **@puneetdixit200** for independently diagnosing and fixing the same MCP
underscore issue (#2746, #2744), **@mvanhorn** for the hydrated deferred-tool
render fix (#2757, #2648), and **@xyuai** for the Xiaomi MiMo Token Plan region
documentation (#2756, #2735). Additional thanks to **@Implementist** for Plan
prompt scrolling, wrapping, and display-width fixes, **@jrcjrcc** for the
Windows sub-agent completion render-width fix, and **@punkcanyang** for the
original `/init` implementation harvested through #2771/#2745.

## [0.8.53] - 2026-06-03

### Added

- **Hugging Face Inference Providers.** Added `huggingface` as a native
  provider route (`/provider huggingface`). Supports `HUGGINGFACE_API_KEY`
  or `HF_TOKEN` for auth, `HUGGINGFACE_BASE_URL` and `HUGGINGFACE_MODEL`
  for overrides, and `deepseek-ai/DeepSeek-V4-Pro` / `deepseek-ai/DeepSeek-V4-Flash`
  as default models. Org-prefixed model IDs pass through.

### Fixed

- **Agent-mode shell error copy.** The missing-tool error for shell tools
  now directs users to `allow_shell = true` instead of nudging toward YOLO
  mode. `/config` surfaces `allow_shell` in the Permissions section.
- **Provider description.** `/provider` command description is now neutral
  instead of recommending specific providers.

### Community

Thanks to **@xyuai** for provider persistence, `/logout` scope clarification,
provider picker key replacement, and MiMo auth cleanup work (#2714, #2715,
#2717, #2718), and **@RefuseOdd** for configurable `path_suffix` support on
OpenAI-compatible endpoints (#2558).

## [0.8.52] - 2026-06-03

### Added

- **SiliconFlow China region provider.** Added the `siliconflow-CN` provider
  variant for the China regional endpoint, sharing the existing
  `[providers.siliconflow]` credentials and `SILICONFLOW_API_KEY` slot
  instead of creating a second credential namespace; the provider picker and
  registry docs now expose the regional route explicitly (#2588, #2615).
- **Multimodal `/attach` image forwarding.** Attached images are now sent as
  OpenAI-compatible `image_url` content blocks so multimodal providers can
  actually see image attachments (#2584, #2587, #2607).
- **Sub-agent lifecycle hooks and runtime metadata.** Sub-agent spawn/complete
  hook events, mode-change runtime messages, mode metadata on turns, localized
  context-inspector strings, and drag-to-resize sidebar width are included in
  this release slice.

### Fixed

- **Sub-agents now auto-cancel after stale heartbeats.** Running sub-agents
  track manager-visible progress and are auto-cancelled after the configurable
  `[subagents] heartbeat_timeout_secs` window (default 300s), releasing their
  concurrency slot and unblocking parent turns that would otherwise wait
  forever (#2603, #2614, #2620).
- **Work panel state survives transient lock misses.** The sidebar caches the
  last successful Work summary so checklist and strategy progress no longer
  disappear into "Work state updating..." while the engine briefly owns the
  shared todo/plan locks (#2606, #2616).
- **SiliconFlow-CN no longer breaks main.** Filled the missing CLI provider
  exhaustiveness arms and removed the duplicate/unreachable TUI config arms
  left by the #2615 landing; direct auth now stores the China-region variant in
  the shared SiliconFlow provider table (#2616, #2618, #2619).
- **v0.8.51 image-attach closure corrected.** The `/attach` multimodal fix
  landed after the v0.8.51 tag, so this release is the first version that
  actually contains it for users installing from the published release line
  (#2584, #2607).
- **Legacy SSE MCP reconnects are retryable again.** Closed or reset
  `POST /messages` requests on stale legacy SSE sessions now trigger the same
  reconnect-and-retry path as closed SSE streams, removing a release-gate flake
  and matching the intended recovery behavior (#2597).
- **Cache-hit cost accounting uses one telemetry source.** Mixed DeepSeek
  `prompt_cache_hit_tokens` and OpenAI-style `cached_tokens` usage payloads no
  longer infer cache misses from the wrong hit count, avoiding inflated TUI cost
  estimates on cached DeepSeek turns (#2567, #2609).
- **Cygwin/MSYS2 config paths honor exported `$HOME`.** CodeWhale and legacy
  DeepSeek config roots now prefer a non-empty `$HOME` before falling back to the
  platform home resolver, while `CODEWHALE_HOME` remains the strongest explicit
  override (#2369, #2610).

### Community

Thanks to **@xyuai** (#2587), **@IcedOranges** (#2584), **@BH8GCJ** (#2588),
**@shenjackyuanjie** (#2618, #2619), **@idling11** (#2606, #2616),
**@AresNing** (#2578), **@caiyilian** (#2567), **@buko** (#2369),
**@gordonlu**, **@encyc**, and **@simuusang** (#2603, #2620) for reports,
patches, retesting, and release-stabilization signals that shaped this pass.

## [0.8.51] - 2026-06-02

### Added

- **Arcee AI as a direct provider.** New `[providers.arcee]` config block and
  `ARCEE_API_KEY` / `ARCEE_BASE_URL` / `ARCEE_MODEL` environment variables,
  wired through CLI auth (`codewhale auth set --provider arcee`), the TUI
  provider picker, and the model registry. The default direct-API model is
  `trinity-large-thinking` (reasoning-capable, 262K context and 262K max
  output); `trinity-large-preview` (262K context, non-reasoning) and
  `trinity-mini` (128K context) are also selectable. OpenRouter's
  `arcee-ai/trinity-large-thinking` route remains separate.
- **Arcee Cloudflare-WAF compatibility.** The opening turn to the Arcee gateway
  uses a benign read-only tool surface (`read_file`, `list_dir`, `file_search`,
  `grep_files`, `git_status`, `git_diff`, `checklist_write`, `update_plan`) and
  splits example payloads such as `python -c …` out of the system prompt, so the
  WAF does not reject the first request; the full tool catalog stays reachable
  through tool-search. `trinity-large-thinking`'s `reasoning_content` is
  recognized and replayed on tool-call turns.
- **Expanded model catalog.** Added context-window, max-output, and
  reasoning-capability metadata for additional model IDs, including
  `qwen/qwen3.6-flash`, `qwen/qwen3.6-plus`, `qwen/qwen3.6-max-preview`, and
  Xiaomi MiMo v2.5 chat/ASR/TTS variants; `trinity-large-preview`'s context
  window was corrected to 262K.
- **Provider-aware model picker.** The picker groups models by provider, shows
  per-model hints, and remembers a saved model per provider.

### Changed

- **Auto-compaction is now percentage- and model-aware.** The per-model
  threshold helper is `compaction_threshold_for_model_at_percent(model,
  percent)` (replacing the effort-based variant), and the default
  `auto_compact_threshold_percent` is 80%. Auto-compaction defaults on for
  models with a context window of 256K or smaller and stays opt-in for 1M-token
  models (e.g. DeepSeek V4) to protect prefix-cache economics, unless the user
  has explicitly set `auto_compact`.
- **Clearer provider/gateway errors.** HTTP error bodies are sanitized before
  display — HTML interstitials and Cloudflare "Access Denied" pages collapse to
  a one-line reason (with the ray/error ID) instead of dumping raw markup into
  the transcript — and 403s are split into authentication vs. authorization
  (gateway/WAF block) categories.
- The invalid-model error now names the active provider and lists Arcee among
  the options.

### Removed

- **The session "cycle" / checkpoint-restart system.** Removed the `/cycles`,
  `/cycle <n>`, and `/recall` commands, the `recall_archive` tool, the
  cycle-handoff briefing prompt, the sidebar "cycles" lines, and the
  `cycle_manager` engine plumbing (`EngineConfig.cycle`, `Event::CycleAdvanced`,
  seam-manager cycle thresholds and flash briefings). Long sessions no longer
  auto-reset their context at a fixed token boundary — reclaim budget with
  `/compact` or model-aware auto-compaction instead. Existing on-disk cycle
  archives are left untouched but are no longer read or written.

### Fixed

- Assistant turns no longer leave an orphaned role glyph (the stray "blue dot")
  when a turn streams only whitespace between reasoning and a tool call.
- Scrolling the mouse wheel over the right-hand sidebar no longer leaks into the
  transcript scroll.
- The sidebar hover tooltip now appears only for truncated lines, sits below the
  cursor, and uses a neutral surface color instead of the warning-orange
  highlight that overlapped neighbouring rows.
- Corrected the README's description of the Constitution (Article VII is the
  hierarchy itself; Article II's truth duty overrides even a user request) to
  match `prompts/base.md`.
- Repaired release-blocking unit and integration tests left failing by the
  cycle-removal and compaction-threshold refactors (relay instruction,
  model-reject message, compaction budget, mock-LLM threshold helper).
- Fixed DEC private-mode CSI fragment leakage into composer text after
  terminal resets, restoring clean prompt editing (#2592).
- The engine now recovers from turn-level panics instead of killing the
  main event loop, keeping the session alive through transient failures
  (#2583, #1269).
- Deeply nested files are now discoverable via @-mention and Ctrl+P file
  picker; the default walk depth was relaxed to handle monorepo layouts (#2488).
- Command-palette selection stays visible when scrolling through long lists
  instead of scrolling off-screen (#2590).
- exec_shell child processes now inherit .NET/NuGet and Windows app-data
  environment variables, fixing toolchain resolution on Windows (#1857).
- A warning is emitted when shell/sandbox config keys are nested under
  unknown top-level sections instead of being silently ignored (#2589).
- Diff-render now preserves leading whitespace in patch content lines,
  fixing an extra-space regression in PR previews (#2591). Thanks @zlh124.
- Model selection from the /model command now persists per-provider across
  restarts, with a warning when persistence fails.

### Community

Thanks to **@zlh124** (#2591) and **@reidliu41** (#2601) for the fixes
harvested into this release. Thanks also to **@idling11** (#2602),
**@gordonlu** (#2585), **@cyq1017** (#2593), **@xyuai** (#2587, #2584),
and **@IcedOranges** (#2584) for reports, drafts, and investigations
that shaped this release cycle.

## [0.8.50] - 2026-06-02

### Added

- Added a Windows NSIS installer release artifact and classroom/lab deployment
  checklist, harvested from #2045 for #1987. The release workflow now builds
  `CodeWhaleSetup.exe` from the canonical Windows binaries, and the installer
  adds/removes only the exact current-user PATH entry.
- Added deterministic session timestamps in session listings, receipt-export
  boundary docs, and current-model turn metadata for routed/auto sessions.
- Added exact AtlasCloud provider-hinted model ID pass-through for explicit
  `vendor/model-id` selections, harvested from #2569 without freezing a
  brittle provider catalog.
- Added Xiaomi MiMo speech/TTS support with a `codewhale speech` CLI command,
  `tts` tool alias, and config wiring for voice-design and voice-clone models,
  harvested from #2560.
- Added a three-zone immutable prefix diagnostic layer (FrozenPrefix Phase 2)
  that logs cache-prefix drift at debug level without blocking requests,
  harvested from #2514.
- Added a Cache Guard CI integration test suite simulating prefix-cache
  behaviour across nine scenarios, gated behind `CODEWHALE_CACHE_GUARD=1`,
  harvested from #2503.
- Added a plan-mode byte-stability invariant test verifying that the tool
  catalog head remains byte-identical across mode toggles, harvested from
  #2519.
- Localized all 15 `/queue` command messages across 7 shipped locales,
  harvested from #2568.
- Added localized `FanoutCounts` MessageId for i18n of the aggregate worker
  stats line in fanout cards, harvested from #2566.
- Added contribution gate CI workflows (PR gate, issue gate, contributor
  approval) with a dry-run mode, harvested from #2565.

### Changed

- Hardened theme repainting and sidebar color use so theme switches do not
  leave stale Whale-dark panel colors behind.
- Made legacy config migration visible when CodeWhale copies old DeepSeek-era
  config into the CodeWhale config path.

### Fixed

- Fixed `/context` to use the effective routed model for context-window
  budgeting, so DeepSeek V4 routes report the 1M-token window and legacy
  DeepSeek routes keep the 128K fallback.
- Fixed npm wrapper version output so `--version` prefers the installed binary
  version instead of stale package metadata when both are available.
- Fixed multiline composer arrow navigation so holding Up/Down at the first or
  last line no longer replaces the current draft with prompt history.
- Fixed foreground `exec_shell` output collection so timeout and inherited-pipe
  cleanup cannot wedge later tool calls behind the global tool lock.
- Clarified the English DeepSeek account-balance footer chip from `bal` to
  `balance` so it is less likely to be mistaken for session spend.
- Fixed truncated subagent tool calls and repeated truncated subagent responses
  so they return model-visible errors instead of silently failing.
- Moved Paste to the first position in the right-click context menu so users
  copying text from the output area can paste with a single left-click instead
  of navigating past cell-specific actions.

### Community

Thanks to **@ZhulongNT** (#2045), **@cyq1017** (#2521, #2536, #2537, #2559,
#2562, #2563, #2564), **@HUQIANTAO** (#2527, #2519, #2503), **@lucaszhu-hue**
(#2569), **@idling11** (#2573), **@encyc** (#2514), **@xyuai** (#2560),
**@gordonlu** (#2568, #2566), and **@nightt5879** (#2565) for the work
harvested into this release pass. Thanks
also to issue reporters and verification helpers including **@New2Niu**
(#2561), **@buko** (#2533, #2369), **@wywsoor** (#2494), **@ctxyao** (#2556),
**@Dr3259** (#2380), **@caiyilian** (#2567), and **@chinaqy110** (#2571) for
reports and acceptance details that shaped these fixes, plus the WeChat/Chinese
UX reports relayed during the final triage pass.

## [0.8.49] - 2026-06-01

### Added

- Added the missing `[providers.moonshot]` example block for Moonshot/Kimi,
  documented `completion_sound`, and refreshed the tool-surface docs for the
  current registry, including `finance`, `web.run`, git history tools, memory,
  OCR, and other registered tools.

### Changed

- Hardened prefix-cache fingerprints to hash API-visible tool schema details,
  not just tool names, so schema and description drift invalidates cached
  prefixes before it can confuse model calls (#2264).
- Kept `finance` registered independently from web-search tools and prevented
  duplicate web/patch tool registration in agent and YOLO modes.

### Fixed

- Fixed the DeepSeek V4-Pro cost estimate after the 2026-05-31 pricing cutoff:
  the post-promotion official rate remains one quarter of the original price,
  so CodeWhale no longer shows roughly 4x too much after June 1 (#2489).
- Fixed Kimi/Moonshot tool schema normalization by moving parent `type` fields
  into `anyOf`/`oneOf` items, with regression coverage for nested schema shapes
  that could otherwise still fail Kimi validation (#2438).
- Fixed raw ANSI/SGR fragments leaking into footer, shell-label, and sidebar
  activity text during active tool execution (#2481).
- Fixed `[tui]` config parsing when `status_items` is omitted, restoring the
  documented default footer order for older and hand-written configs (#2483).
- Fixed a shell env-scrubbing test so it does not depend on the user's default
  shell understanding POSIX parameter expansion.
- Removed stale `qwen/qwen3.7-max` references left in `config.example.toml`
  after the v0.8.48 preset removal.

### Community

Thanks to **@idling11** (#2480, #2485), **@reidliu41** (#2493),
**@hongqitai** (#2495), and **@encyc** (#2477) for the fixes and reliability
work harvested into this release.

Thanks also to reporters and verification helpers whose issues shaped the
release: **@A-Corner** (#2438), **@taiwan988** (#2483), **@AiurArtanis**
(#2489), and **@Hmbown** (#2481).

## [0.8.48] - 2026-05-31

### Added

- **Recent large OpenRouter model presets.** Added completions, aliases,
  routing metadata, and docs for Arcee Trinity Large Thinking,
  MiniMax M3, Xiaomi MiMo v2.5, Qwen 3.6 open-weight models, Kimi K2.6,
  GLM 5.1, Tencent Hy3, Gemma 4, and Nemotron (#2461).
- **Provider and web-search expansion.** Added Xiaomi MiMo provider support,
  SiliconFlow, AtlasCloud static models, Volcengine Ark search, Baidu AI
  Search, provider-picker coverage, and richer custom-provider docs
  (#2246, #1868, #2421, #2429, #2371, #2394, #2287).
- **Workflow and tool ergonomics.** Added the external-tool abstraction,
  pluggable TUI tool registry, custom slash-command allowed-tools enforcement,
  opt-in Unix socket hook sink, message-submit transform hooks, tool-cache
  introspection, and cache warmup-key tracking (#2294, #2420, #2326, #2430,
  #2434, #2423, #2424).
- **TUI workflow features.** Added `/purge`, `/hunt`, thinking fold/unfold,
  terminal-transparent/Solarized Light/Claude themes, footer branch display,
  macOS notifications, intent summaries before approval prompts, and the
  mobile runtime smoke/QR workflow (#2387, #2306, #2385, #2276, #2270, #2267,
  #2347, #2260, #2389, #2403).
- **Platform and localization coverage.** Added RISC-V prebuilt-binary
  support, Vietnamese localization, Java/Vue language-server defaults, runtime
  event envelopes, task migration/env isolation fixes, and state-message
  parent IDs for future forks (#2383, #2358, #2367, #2252, #2272, #2308).

### Removed

- **Qwen 3.7 Max OpenRouter preset.** Removed from the model registry, docs,
  and examples. Qwen 3.7 Max is a hosted model, not open-source; the preset
  will return when an open-weight Qwen 3.7 release ships.

### Changed

- **Release hardening.** CI now runs clippy/docs checks, web frontend lint and
  type checks, provider-registry drift checks, broader crate docs, and a large
  unit-test pass across core, MCP, TUI core, app-server, and web helpers
  (#2443, #2444, #2274, #2446-#2460, #2440, #2441, #2450, #2448, #2454).
- **Prompt, context, and model routing behavior.** Stabilized project-context
  pack ordering, exposed the auto route in turn metadata, allowed embedders to
  override or inline constitutional instructions, moved volatile environment
  context below the prompt boundary, and used the effective model for
  compaction budgeting (#2418, #2410, #2356, #2311, #2314, #2437).
- **Execution policy foundation.** Added typed ask-rule groundwork and kept
  `task_shell_start` gated behind `allow_shell`, preparing the permission UI
  path without broadening default shell access (#2404, #2384).

### Fixed

- **Windows and shell reliability.** Suppressed alt-screen logging on Windows,
  added the Windows batch launcher path, kept task shell tools eagerly loaded,
  loaded exec-shell companion tools consistently, covered controlling-terminal
  behavior, and improved shell tool availability errors (#2259, #2295, #1861,
  #2271, #2331, #2414, #2412).
- **Session and transcript durability.** Fixed hidden-worktree discovery
  saturation, stalled in-progress turn recovery, session persistence
  truncation, cached-transcript user-message highlighting, large tool-output
  receipting, session-detail block serialization, and deterministic composer
  history flushing (#2273, #2329, #2283, #2395, #2386, #2297, #2265, #2375).
- **Provider and UI polish.** Accepted custom model IDs in `/model` for
  non-DeepSeek providers, fixed Feishu per-chat model switching, localized
  context-menu labels, updated terminal tab naming, kept picker selections
  visible, allowed slash-space composer messages, and improved PDF text
  cleanup (#2280, #2149, #2320, #2319, #2324, #2316, #2266).
- **Security and dependency hygiene.** Bumped `tar` and `qs`, trusted fake-IP
  placeholder ranges only when explicitly configured, decoded Bing result URL
  entities, fixed legacy MCP SSE connections, and replaced manual tool error
  display code with `thiserror` derives (#2364, #2425, #2355, #2245, #2301,
  #2442).

### Community

Thanks to contributors whose PRs landed or were harvested in this release:
**@cy2311** (#1861),
**@LING71671** (#1902, #2287, #2292),
**@axobase001** (#1968, #2296, #2297, #2298),
**@dzyuan** (#1993),
**@mvanhorn** (#2107, #2236),
**@malsony** (#2129),
**@gaord** (#2133, #2265, #2285),
**@yuanchenglu** (#2149),
**@idling11** (#2161, #2266, #2306),
**@h3c-hexin** (#2245, #2311, #2313, #2314, #2354, #2355, #2356),
**@AdityaVG13** (#2246),
**@Sskift** (#2248),
**@cyq1017** (#2252, #2332, #2375),
**@HUQIANTAO** (#2257, #2267, #2283, #2384, #2385, #2389, #2403, #2440-#2458, #2460),
**@New2Niu** (#2260),
**@AiurArtanis** (#2270),
**@Lee-take** (#2272),
**@nightt5879** (#2274, #2344, #2347, #2373),
**@AresNing** (#2278, #2318/#2434),
**@AccMoment** (#2281),
**@reidliu41** (#2291, #2316, #2324, #2357, #2366, #2386, #2431),
**@aboimpinto** (#2290, #2294, #2295, #2326, #2433),
**@zhuangbiaowei** (#2301),
**@donglovejava** (#2302, #2329, #2330, #2331),
**@hongqitai** (#2308, #2432),
**@zlh124** (#2319, #2320, #2325),
**@encyc** (#2336, #2338),
**@Implementist** (#2426/#2429, #2439),
**@lihuan215** (#2333/#2430),
**@LeoAlex0** (#2388, #2395),
**@jimmyzhuu** (#2371),
**@rockyzhang** (#2383),
**@mo-vic** (#2387),
**@hufanexplore** (#2367),
**@hoclaptrinh33** (#2358),
and **@BryonGo** (#2437).

Thanks also to reporters and verification helpers whose issues, patches,
screenshots, logs, or retest requests shaped this release: **@buko** (#2359,
#2360, #2369, #2469), **@yyyCode**, **@gaslebinh-glitch**, **@Dr3259**,
**@lpeng1711694086-lang**, **@VerrPower**, **@yan-zay**, **@jretz**,
**@Neo-millunnium**, **@caeserchen**, **@T-Phuong-Nguyen**, **@zhyuzhyu**,
**@0gl20shk0sbt36**, **@hatakes**, **@goodvecn-dev**, **@bevis-wong**,
**@PurplePulse**, and **@nbiish**.

## [0.8.47] - 2026-05-26

### Added

- **Closed-loop verification gate, runtime goal tools, DuckDuckGo default
  web search, Xiaomi MiMo, global AGENTS.md fallback, `/new`, composer
  selection, transcript copy cleanup, CNB mirror support, and Docker toolbox
  docs** shipped in the published v0.8.47 release.

### Changed

- **DeepSeek-first release framing, project-context logging, state-root
  migration, CodeWhale README paths, and reasoning-locale behavior** were
  finalized for the v0.8.47 release.

### Fixed

- **Provider picker scrolling, auto model restore, cache-inspect hashing,
  insecure LAN provider guard, large tool-output compaction, queued-message
  ordering, shell/Yolo startup handling, Windows alt-screen logging, and
  tooltip contrast** were fixed in the v0.8.47 release.

### Community

Thanks to contributors credited in the v0.8.47 GitHub Release, including
**@Fire-dtx**, **@imkingjh999**, **@harvey2011888**, **@victorcheng2333**,
**@IIzzaya**, **@PurplePulse**, **@cyq1017**, **@knqiufan**,
**@Colorful-glassblock**, **@hongqitai**, **@EmiyaKiritsugu3**,
**@aboimpinto**, **@HUQIANTAO**, **@mvanhorn**, **@LING71671**, and
**@reidliu41**.

## [0.8.46] - 2026-05-26

### Added

- **`CODEWHALE_*` env aliases.** `CODEWHALE_PROVIDER`, `CODEWHALE_MODEL`,
  and `CODEWHALE_BASE_URL` are public product-scoped aliases that take
  precedence over the legacy `DEEPSEEK_*` forms. The `DEEPSEEK_*` names
  remain accepted for back-compat.
- **Platform archive bundles.** Release artifacts now ship as per-platform
  archives (`tar.gz` for Linux/macOS, `.zip` for Windows) containing both
  `codewhale` and `codewhale-tui` binaries plus an install script. No more
  downloading two loose files and guessing which ones to pick (#2193).
- **Windows portable archive.** `codewhale-windows-x64-portable.zip` ships
  the two binaries without an install script for USB-stick distribution
  (#2193).
- **Web install download tile.** The website install page now shows a
  platform-aware download tile with arch detection, SHA256 checksum
  display, and China mirror links, instead of burying the download behind
  the Cargo instructions (#2192).
- **Whale dark palette refresh.** Better contrast and layer separation
  across the TUI color scheme (#2197).
- **Auto-collapse finished sub-agents.** Completed sub-agent sessions now
  collapse automatically in the sidebar, reducing noise during long
  sessions (#2195).
- **Shell-running status chip.** A `⏳ shell running` chip appears in the
  TUI footer while background shell tasks are active (#2194).
- **Sandbox process hardening (Linux).** `PR_SET_DUMPABLE=0`,
  `NO_NEW_PRIVS`, and `RLIMIT_CORE=0` are applied at shell startup to
  harden child processes against inspection and privilege escalation
  (#2183).
- **CONTRIBUTING.md cross-links.** Issue and PR templates are now
  cross-linked from CONTRIBUTING.md to improve contributor onboarding
  (#2203).

### Changed

- **DeepSeek-first focus.** v0.8.46 refocuses on delivering the
  highest-quality experience on DeepSeek first. Additional first-class
  provider paths are planned for v0.9.0 after the core DeepSeek workflow
  is solid.

### Fixed

- **Model name casing preserved.** `normalize_model_name_for_provider` no
  longer lowercases user-set model names such as `DeepSeek-V4-Flash`,
  preventing API lookup failures on case-sensitive backends (#2109).
- **Esc in model picker applies selection.** Dismissing the model picker
  with Esc now applies the last-highlighted choice instead of reverting
  (#2196).
- **Web install downloads both binaries.** The `install-binary.tsx`
  snippet now fetches both `codewhale` and `codewhale-tui`, fixing the
  `MISSING_COMPANION_BINARY` trap on fresh npm installs (#2191).
- **`grep_files` skips large directories.** The pure-Rust search tool
  now skips known-large directories (`.git`, `node_modules`, `target`)
  before walking, preventing hangs on deep or slow filesystems.
- **Version-update hint uses semver.** The update notification in the
  footer now compares versions semantically instead of lexicographically,
  so `0.8.10 > 0.8.9` is recognized correctly.
- **CVE-2026-8723 in feishu-bridge.** Bumped `qs` to `>=6.15.2` in the
  Feishu bridge integration (#2198).

### Community

Thanks to new contributors whose PRs landed in this release:
**@donglovejava** (#2154, #2163, #2166, #2167, #2168),
**@encyc** (#2152),
**@saieswar237** (#2178),
**@sximelon** (#2174),
**@nanookclaw** (#2135),
**@Sskift** (#2119),
**@xin1104** (#2105),
**@mrluanma** (#2059),
**@Lellansin** (#2055),
**@zhuangbiaowei** (#2145),
**@aboimpinto** (#1872),
and continuing contributors **@reidliu41**, **@cyq1017**, **@idling11**,
**@h3c-hexin**, **@wdw8276**, **@zlh124**, and **@jeoor**.

## [0.8.45] - 2026-05-25

### Added

- **RLM session objects.** `rlm_open` can now load `session://` refs,
  exposing the active prompt, history, and session data as symbolic objects
  inside RLM REPLs (#2047).
- **Command palette voice input.** The command palette can launch a configured
  speech-to-text helper and show footer status while transcription runs
  (#2047).
- **Moonshot/Kimi provider.** Moonshot/Kimi is now a first-class provider,
  including API-key auth, model completion, CLI auth, secret-store
  integration, and optional Kimi CLI credential reuse.
- **Deterministic whale-species sub-agent names.** Sub-agents now get stable,
  human-readable whale-species nicknames (e.g. "Beluga", "Orca") while
  preserving the raw agent ID in the popup (#2035, #2016).
- **`/balance` command scaffold.** Registered the `/balance` slash command
  as a placeholder for future provider billing queries (#2035, #2019).
- **Readable `/restore` snapshot labels.** Snapshot labels now include the
  originating user prompt so restore listings are easier to identify. Thanks
  @idling11 (#2111).
- **Sidebar hover tooltips.** Truncated Work and Tasks sidebar lines now expose
  their full text on hover. Thanks @idling11 (#2110).

### Changed

- **AGENTS.md is now maintainer-local.** The project instructions file no
  longer ships as a tracked repo file; it lives in maintainer-local ignored
  state (#2047).

### Fixed

- **Sub-agent completion handoff compatibility.** Completion handoffs now use a
  chat-template-safe role and emit before terminal updates, fixing strict
  OpenAI-compatible/self-hosted backends and preserving transcript ordering.
  Thanks @h3c-hexin and @cyq1017 (#2057, #2120).
- **Self-hosted context budgeting.** Sub-500K self-hosted model windows now keep
  a usable input budget instead of disabling preflight compaction after output
  reservation underflow. Thanks @h3c-hexin (#2060).
- **Goal prompts start actionable.** Goal-start prompts now open in an
  actionable state instead of requiring an extra nudge. Thanks @cyq1017
  (#2097).
- **Composer session title display.** The composer chrome shows the current
  session title again and avoids grayscale luma overflow in debug builds.
  Thanks @wdw8276 (#2108).
- **Approval prompts use a one-step confirmation flow.** Enter now commits the
  selected approval option directly, destructive warnings remain visible, and
  abort cancels the active turn instead of only denying the current tool call.
  Thanks @reidliu41 (#2143).
- **Model picker selection survives Esc.** Dismissing the model picker with Esc
  no longer loses the highlighted selection. Thanks @reidliu41 (#2056).
- **Moonshot/Kimi sessions launch from the dispatcher.** The `codewhale`
  wrapper now includes Moonshot/Kimi in the TUI provider allowlist, so
  `codewhale --provider moonshot --model kimi-k2.6` reaches the TUI instead of
  stopping after config resolution.
- **Slash recovery no longer restores command tails in the composer.**
  Resuming a session or recovering from a crash no longer leaves stale
  slash-command text (e.g. `/sessions`) in the composer input (#2047, #2032).
- **Remembered tool approvals now update the live active turn.**
  When the "remember" checkbox is set on an approval dialog, the active
  turn's auto-approve flag flips immediately instead of waiting for the
  next turn. Thanks @gaord (#2047, #2041).
- **YAML block scalars in SKILL.md frontmatter.** Multi-line descriptions
  using `>` or `|` indicators are now parsed correctly — folded block
  scalars join non-empty lines with spaces, literal scalars preserve
  newlines, and all three chomping modes (strip/clip/keep) are supported.
  Thanks @zlh124 (#1908, #1907).
- **User messages highlighted in the transcript.** User-authored messages
  now render with a full-row background in the live TUI transcript, making
  it easier to scan prior turns. Assistant and system messages are
  unaffected. Thanks @reidliu41 (#1995, #1672).
- **Cancellable `list_dir` and `file_search`.** Long directory walks and
  file searches now respond to user cancel/stop requests with a 30-second
  fallback timeout, preventing the TUI from hanging on deep or slow
  filesystems (#2035).

### Community

- **README contributor acknowledgements resynced.** The Thanks list now
  includes the latest contributor rows for @donglovejava, @encyc,
  @saieswar237, @sximelon, @nanookclaw, @Sskift, @xin1104, @mrluanma,
  @Lellansin, and @zhuangbiaowei, while preserving the existing @jeoor
  acknowledgement in the consolidated list.

---

Older releases: [CHANGELOG.md](https://github.com/Hmbown/CodeWhale/blob/main/CHANGELOG.md) and [docs/CHANGELOG_ARCHIVE.md](https://github.com/Hmbown/CodeWhale/blob/main/docs/CHANGELOG_ARCHIVE.md).
