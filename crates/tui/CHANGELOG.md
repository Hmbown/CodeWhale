# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- Preserve Solarized Light's canonical Base3 (`#fdf6e3`) shell background
  instead of tinting it green-grey through the default underwater Ombre
  treatment, while retaining foreground ambient life (#4457 by
  @AiurArtanis).

## [0.9.1] - 2026-07-17

Codewhale v0.9.1 ships a first-class local web client over the Runtime API,
first-class OpenCode Go and restored xAI device login on the provider surface,
calendar-correct hourly automations, and a hardening pass over the community
site's draft, feed, login, and content-watch boundaries.

### Added

- Add `codewhale web [--port 7878]`, a first-class loopback-only browser
  client over the canonical Runtime API. The dependency-free embedded shell
  supports thread lifecycle, snapshot-then-SSE transcripts, turn start/steer/
  interrupt, approvals, and user questions, including pending-request recovery
  across tab reloads, while leaving unsupported managed,
  files, PTY, model-selection, and Fleet controls absent. Browser auth uses a
  short-lived one-time loopback capability exchanged for an opaque, bounded,
  process-local HttpOnly, SameSite=Strict session cookie with a same-origin
  mutation guard; Runtime tokens never enter URLs, HTML, browser storage, logs,
  or browser-launch arguments (#4423).
- Add OpenCode Go as a first-class, subscription-backed Chat Completions
  provider with `[providers.opencode_go]`, `OPENCODE_GO_API_KEY`, and the eight
  models currently documented on its `/v1/chat/completions` endpoint. Models
  served only through OpenCode Go's Anthropic `/messages` endpoint remain out
  of this narrow route until Codewhale supports per-model wire selection
  (#1481 by @seanthefuturegorilla; implementation harvested from PR #773 by
  @zhangweiii and PR #1050 by @sternelee).
- Add OpenCode Zen as a separate model-aware API-key provider. Its curated
  model catalog selects Responses, Anthropic Messages, or Chat Completions per
  model; unsupported Gemini and unknown models fail closed, and Zen missing
  credentials never fall through to ChatGPT/Codex OAuth guidance.

### Fixed

- Restore xAI/Grok device-code OAuth login against the live xAI OIDC
  contract: discovery with issuer/endpoint validation and documented
  fallbacks, user-principal scope set, RFC 8628 `slow_down` backoff capped at
  code expiry, and bounded, sanitized error reporting for denial, expiry, and
  malformed responses (#4410).
- Anchor `FREQ=HOURLY` automations with `BYHOUR`/`BYMINUTE` to persisted
  local-calendar slots so intervals keep their wall-clock phase across DST,
  restart, resume, RRULE updates, duplicate-slot recovery, and post-run
  advancement. Nonexistent clock slots are skipped and ambiguous slots run at
  their first occurrence (#4381 by @h3c-hexin).
- Give content-watch drafts canonical identities: the link and semantic-drift
  watchers now write and dedup through one canonical draft-storage key with
  deterministic hash-suffixed IDs, validate and bound model drift output
  before any KV writes, and show truthful admin draft labels, so unchanged
  findings dedup and changed findings re-draft instead of colliding (#4453).
- Make model-policy JSON repair consider object and array payloads in source
  order, matching nested delimiters across quoted strings and escapes and
  returning the earliest balanced candidate that parses, instead of letting
  an unmatched opening delimiter or object-first preference corrupt array
  payloads (#4430).
- Convert persisted sub-agent completion and still-running control events into
  concise, non-authoritative resume checkpoints, keeping their raw runtime
  envelopes, sentinels, and retry instructions out of restored model and TUI
  conversation state (#4409).
- Deliver failed, stopped, and stale sub-agent outcomes exactly once to the
  awaiting parent, lifecycle mailbox, and TUI before closing their runtime
  state. Restart now reconciles orphaned queued/model/tool-wait worker records
  to interrupted while preserving checkpoints, and cancelled workers no
  longer read as completed in the TUI (#4408).
- Give host applications a cancellation boundary for MCP OAuth login so a
  stalled or abandoned provider login no longer hangs the calling session
  (#4380).
- Avoid blocked reader joins after Windows process kills so terminated shell
  sessions cannot hang their readers (#4383).
- Keep the Hotbar Setup action list synchronized with keyboard focus when the
  selection moves beyond the visible rows, including Down past `/export`
  (#4418).
- Reconcile the website roadmap with reality: the retired share-link
  direction is now an explicit non-goal, Workrooms is the considered
  direction, and the local web client appears as underway, in English and
  Chinese (#3418).

### Security

- Restrict cross-origin Runtime API browser preflights to the documented
  authentication and content headers, explicitly allowing `Authorization`
  instead of relying on a wildcard (#4454).

### Contributors

Thank you to the contributors whose code, reports, and reviews shaped v0.9.1:

- [@h3c-hexin](https://github.com/h3c-hexin) — calendar-anchored hourly
  automation recurrence (PR #4381) and the MCP OAuth cancellation report
  (#4380).
- [@zhangweiii](https://github.com/zhangweiii) and
  [@sternelee](https://github.com/sternelee) — the original first-class
  OpenCode Go implementations (PRs #773 and #1050), harvested into the
  current provider architecture.
- [@seanthefuturegorilla](https://github.com/seanthefuturegorilla) — the
  canonical OpenCode Go/Zen provider request and acceptance direction
  (#1481).
- [@nightt5879](https://github.com/nightt5879) — the Solarized Light
  background preservation fix (PR #4471).
- [@AiurArtanis](https://github.com/AiurArtanis) — the Solarized Light
  regression report and reproduction (#4457).
- [@shenjackyuanjie](https://github.com/shenjackyuanjie) — the HarmonyOS
  workflow-js bindgen, portable-pty gating, and SDK environment work
  (PR #4470).
- [@shenyongqing](https://github.com/shenyongqing) — the original HarmonyOS
  bindgen approach (PR #4384), carried into the landed implementation.

## [0.9.0] - 2026-07-16

Codewhale v0.9.0 replaces the default terminal shell with the underwater
interaction system, makes Operate message-first, and hardens the Fleet,
Workflow, routing, accounting, and release surfaces that support day-to-day
agent work. The release also expands localization and gives the public site a
quieter, docs-first community foundation. Its provider work replaces the old
hand-maintained picker boundary with live ProviderLake discovery and adds the
largest curated model-and-pricing expansion in the project so far.

### Fixed — final integration

- Redact configured, environment, file-backed, and bare active credentials
  from every tool result before it crosses any model-provider wire protocol;
  retrieved spillover content is sanitized again at that boundary. The
  `read_file` tool also refuses CodeWhale configuration, backup, and
  credential-store paths, preventing routine tool use from exposing those
  local files.
- Keep immediate TUI submit failures inside the shell: custom-provider route
  preflight and closed-mailbox errors now restore the exact composer draft and
  selected skill for retry, with a sticky visible error instead of exiting.
- Anchor automatic compaction thresholds to the route's spendable input
  budget after output reservation and safety headroom, so large-output and
  tight self-hosted routes compact before provider context rejection. The TUI
  pre-send gate and warning copy now use the same token threshold as the
  engine. Preserve the 262K Kimi route's usable input budget and use the
  documented 32K default generation budget instead of mirroring the context
  window as output (#4293 by @SamhandsomeLee, #4368 by @bruce6135, and #4378
  by @mvanhorn).
- Fail closed instead of reporting base-rate dollar estimates for direct OpenAI
  GPT-5.4/5.4 Pro, GPT-5.5 (including dated snapshots), and GPT-5.6
  Sol/Terra/Luna requests above 272K input tokens. Exact tiered accounting
  remains deferred to the generalized pricing schema; smaller 5.4 variants,
  GPT-5.5 Pro, Codex subscription, and foreign-provider routes are unchanged
  (#4317).
- Retire `deepseek-chat` and `deepseek-reasoner` before they reach DeepSeek's
  first-party OpenAI or Anthropic wire APIs, migrating both to the documented
  `deepseek-v4-flash` replacement while preserving legacy non-thinking /
  thinking intent when no explicit reasoning tier is set. Aggregator, Wanjie
  Ark, self-hosted, and custom endpoint model ids remain provider-owned (#4320).
- Make Operate a message-first multitask surface: ordinary prompts work without
  a Workflow, direct parent tools follow the same approval, sandbox, shell,
  ask-rule, and repository protections as Act, and follow-ups can queue while
  work is active. Bounded background workers remain preferred for independent,
  parallel, isolated, or long-running work; child handoffs cannot inherit
  standing Full Access, and each dispatch produces one durable completion
  receipt.
- Let personal Fleet profiles in `CODEWHALE_HOME/agents` travel across
  repositories while project profiles in `.codewhale/agents` override them.
  Saving refreshes the live roster, and the UI now says explicitly that profile
  availability does not expand workspace, trust, or filesystem authority.
- Move file-mention discovery onto one bounded, generation-safe background
  worker so a slow filesystem read cannot freeze composer input. Exact paths
  resolve on send; fuzzy matches stay in the completion popup instead of
  silently attaching an arbitrary same-name file (#4365 by @WavesMan, with the
  initial bounded-walk approach from #4367 by @LeoLin990405).
- Keep the opt-in `remember` tool in the model-visible first-turn catalog so
  durable preference capture works without requiring a model to discover a
  tool it cannot yet know exists (#4373 by @Angel-Hair and #4377 by
  @mvanhorn).
- Make `review` handle a staged snapshot relative to a base ref by comparing
  the branch merge-base tree with the index. This preserves committed and
  staged branch work, excludes unstaged edits, and avoids the invalid
  `git diff --cached <base>...HEAD` form.
- Honor each MCP server's advertised discovery capabilities before calling
  optional tools, resources, templates, or prompts; keep optional probes
  independently bounded and fail-soft (#4308 by @nsfoxer).
- Make offline `scorecard` pricing provider-aware: `turn_end` records carry the
  effective route and a non-secret billing surface, runtime exports and
  supported aliases ingest cleanly, legacy/unknown routes remain explicitly
  unpriced, and route-scoped cache and recorded-time pricing replace model-only
  guesses. Historical runtime aggregates use each turn's recorded time;
  costless catalog routes fail closed while exact provider-owned hand-price
  rows remain available. StepFun PAYG and Step Plan usage now stay distinct
  without persisting raw endpoint URLs, so subscription quota is never reported
  as token spend (#4335). Completion-only shell, manual-compaction, and purge
  events remain visible to `turn_end` observers as explicitly non-model
  lifecycle records. This builds on the scorecard introduced by @findshan in
  #3388.
- Preserve named custom-provider identity across TUI sessions, `exec --resume`,
  runtime threads, exports, cache and Workflow receipts. Restores resolve the
  saved provider against live configuration before creating a client, never
  infer a provider from the model ID, and fail closed when the named route was
  removed, invalid, or ambiguous (#4334).
- Bind credentials to the endpoint that owns them. Environment-selected custom
  hosts can no longer inherit saved provider keys, keyring entries, OAuth, or
  ambient provider variables; only an explicitly source-marked CLI key may
  follow an explicit CLI endpoint override. `auth_mode = "none"` also strips
  credential-shaped custom headers consistently in the TUI and app server,
  while keyless loopback routes remain usable as local runtimes.
- Make hosted runtime threads deterministic and provider-exact: serialize
  thread, turn, and event mutation; keep cancellation ownership with the host;
  preserve the selected provider through every durable turn; terminalize
  exceptional streams once; and prevent the runtime manager from silently
  dispatching unclaimed goal continuations or child turns.
- Treat required user confirmation as a real goal blocker instead of a failed
  goal, and explain how to recover when a previously cached approval is denied.
  Cached-denial recovery is also committed as a settled transcript receipt, so
  tool completion or a later status update cannot erase it from scrollback or
  accessibility output. The notice now describes matching, process-scoped
  denials truthfully across all shipped locales; approval audits honor
  `CODEWHALE_HOME`, and expired status toasts cannot remain trapped behind a
  persistent entry. Both states remain visible and actionable instead of
  looking like unexplained model or tool failure (#4374 and #4375 by
  @Angel-Hair, with the final hardening in #4385 by @nightt5879).
- Make Fleet launch and teardown deterministic: route flags are placed before
  `exec`, workers are contained in owned Unix sessions or Windows Job Objects,
  and cancellation reaps surviving descendants with bounded escalation before
  manager state settles. Fence progress, terminal status, verification
  receipts, and evidence by durable attempt generation so a stale process can
  never complete or overwrite a restarted attempt; terminal state and receipt
  now commit atomically, stale-heartbeat decisions use a full lease CAS,
  exhausted-retry alerts are exactly once, and crash-truncated ledger tails are
  quarantined before the next append. Standalone CLI and Runtime API restart
  controls now drive the replacement attempt through a real executor to its
  terminal receipt, while per-run manager ownership prevents concurrent
  controllers from launching the same attempt twice.
- Keep the stopship Workflow fixture bounded to measured 24k-per-turn role
  budgets and a 360k aggregate. Authored child step and wall-time limits now
  reach the live runtime, including launch-queue wait; promoted evidence stays
  intact between roles, tool-free handoff consumers omit tool fields on the
  provider wire, and a terminal `BLOCK` fails the Workflow instead of producing
  a successful Lane receipt. Free-form descriptions no longer fabricate write,
  shell, or network risk; unknown structured risk remains fail-closed.
- Keep repository trust affirmative and explicit: only `1`/`Y` are advertised
  as acceptance keys, while Enter remains non-affirmative and explains the
  required choice.
- Replace literal legal and doctrinal metaphors in Simplified Chinese setup and
  `/constitution` copy with direct collaboration terminology reviewed by a
  native speaker (#4369 by @hmr-BH).
- Keep the transcript reviewable while an inline approval card is active:
  Page Up/Down, modified arrows, Home/End, and the mouse wheel now move through
  the visible evidence without changing or dismissing the pending decision
  (#4371 by @amuthantamil).
- Match generated worker names to the active UI language while preserving
  explicit user names, and tighten the 89x50 shell rhythm across Fleet rows,
  choice dialogs, transcript boundaries, and the idle composer.
- Put docs content and search before the full index on small screens, reduce
  mobile dead space, and keep the public community copy focused on issues,
  pull requests, and international contributors.

### Changed — the underwater shell

- Replace the default TUI shell with the underwater interaction system: one
  renderer owns the header, top work strip, transcript ledger, composer, and
  footer, with explicit compact/normal/wide tiers and no legacy sidebar or
  dashboard in the default path. The legacy composition survives only behind
  the internal `classic` treatment.
- Add a distinct pre-session launch screen — new session, new worktree (with
  inline naming and real lane provisioning), scoped resume count, changelog,
  quit — with reliable non-colliding keys and row/keyboard parity.
- Render turns as a ledger: user message, short narration, settled tool
  receipts, and exactly one live row. Fast tool bursts land directly as
  batch receipts (no spinner churn), completed receipts stay inspectable,
  failures hold a coral receipt with stderr one `v` away, and one shared
  tool rail replaces nested card borders.
- Make completion a one-shot exhale: `working -> finishing -> done` in the
  footer only, with no transcript repaint, no lingering loop, and no stale
  cancel action in the completed state.
- Rebuild the secondary rooms on one hairline grammar — config, setup,
  sessions, help, context, theme, model/route, Fleet, file attach — each
  with a title hairline, row objects with focus/selection/mouse parity,
  one panel-owned scroll rail, and wrapped action footers.
- Make `/model` a model-first atomic route picker across configured
  providers: provider and model switch together on apply, and every row
  prints the resolved model. `/theme` gains a live preview with truthful
  Esc revert across all 12 shipped themes.
- Add a live context inspector (Alt+C) backed by the current route: exact
  system/messages/free token buckets, a proportional map, drill-down into
  the detail pager, and no frozen session while it is open.
- Project Workflow runs as an in-stream run map: a collapsed one-line card
  that unfolds into per-lane rows with role, resolved model, worktree,
  elapsed track, and per-member running/waiting/failed/cancelled/done
  states, plus gates and a debrief built only from real run data. Child
  transcripts never flood the parent shell.
- Unify Fleet into roster/setup/workers rooms: the operator is pinned first
  with the live session route, members show resolved route truth (inherit /
  fast lane / pinned), and the workers tab is a control surface with
  row-local open/stop and real lifecycle counts.
- Distinguish repository-law approvals from ordinary approvals: the
  constitution prompt names its authority, source, matched rule, and target,
  and Full Access never bypasses it. Ordinary approvals render as a still
  coral band above the visible transcript.
- Keep streaming honest and cheap: provider-unit deltas replace per-grapheme
  queueing, the transcript is top-anchored so appended lines stop shifting
  settled rows, ambient animation stops during real work, and ordinary
  completion no longer triggers full-screen clears (verified by render-diff
  logs: suffix updates of tens of cells while streaming, zero periodic
  full repaints).
- Give every underwater treatment ambient life: ombre breathes its water
  column while flat and Terminal-owned keep the idle fish and bubble
  (foreground-only for Terminal), a typed treatment setting replaces string
  comparisons, reduced motion freezes life legibly, `fancy_animations =
  false` stills the chrome, and typing scatters the fish immediately. Fish
  keep a one-cell gap from occupied text; the whale remains the single brand
  mark and returns to stillness between caustic sweeps.
- Bring the whale mark to life with a soft diagonal caustic sweep, then let it
  genuinely rest. Active markers now share a smoother 8 Hz clock after the
  existing earned-motion delay, while reduced motion, hidden/off-screen views,
  modal ownership, and compact-terminal redraw budgets remain authoritative.
  The motion is adapted from the Apache-2.0 Grok Build interaction language,
  not copied as a global pulse or high-frequency receipt cascade.
- Keep compact terminals operable: `/config` and `/resume` collapse
  secondary chrome before sacrificing their selectable rows at 40x12 and
  60x16, bodies budget for the footer's real wrapped height, and the
  selection stays visible through resizes.
- Route footer notices through the classified toast system so informational
  acknowledgements (for example "Auto-compaction enabled") expire instead of
  becoming permanent idle chrome, while warnings and errors hold as sticky
  notices until their window passes.
- Complete the `CODEWHALE_ASCII_SAFE=1` decorative tier: the whale mark,
  context meter, braille state markers (mapped by dot density so the working
  bubble still reads as a rising fill), bubbles, rails, and role/lane glyphs
  all narrow to semantic ASCII while user, model, and CJK text passes
  through untouched. Verified by whole-surface rendered-buffer sweeps.
- Repair the Help catalog to match handler truth (`Alt+G`, `Alt+Shift+G`,
  `Alt+[`, `Alt+]`, `Alt+L`, `Alt+?`), and give theme, Help, model, and
  config rows direct mouse paths with the same activation as Enter.

### Changed — integrated runtime and TUI

- Make worker delegation route-aware and identity-safe: workers receive a
  small role-scoped system prompt instead of stale parent/model boilerplate,
  faster routes resolve through the configured provider, and opening a worker
  shows its complete available transcript. Remove `token_budget` from the
  ordinary model-facing Agent schema so agents do not micromanage ad-hoc
  launches; explicit legacy calls remain readable for compatibility.
- Mature `/config` interaction for enumerated and boolean settings with
  pickers/toggles, mouse-wheel scrolling, stable focus, and configured-provider
  selection. Startup mode is now only Agent or Plan; legacy `operate`/`yolo`
  settings migrate to Agent with permission posture represented separately.
- Show where effective permission policy comes from and keep profile,
  environment, project, managed, and requirements-controlled posture read-only
  in the in-session editor. Runtime presets edit only proven user-owned root
  settings and no longer persist temporary environment overlays.
- Restore the original four-line whale mark and make ambient ocean motion
  coherent across the full scroll surface: one continuous ombre, eased fish
  that face their direction of travel, fish in otherwise blank scrollback, and
  explicit reduced-motion and animation controls.
- Keep model reasoning in the transcript rather than the Tasks strip, retain
  the live header status indicator, separate worker and success colors, and use
  the same rail grammar for both work-strip and transcript scrollbars.
- Present the default Z.AI Coding Plan route, including child routes, as
  subscription quota instead of estimated per-token dollars. No undocumented
  account endpoint is called by this change.

### Added

- Thinking Machines Lab's Inkling through Together using the exact wire model
  `thinkingmachines/inkling`, with `inkling` and `together-inkling` aliases and
  exact `none` / `minimal` / `low` / `medium` / `high` / `max` reasoning
  values. Codewhale does not invent a context window, price, or offline picker
  claim while the provider's public catalog metadata remains inconsistent.
- Expand the verified offline catalog with Claude Sonnet 5, Claude Fable 5,
  GPT-5.3 Codex, and Qwen3.7 Plus, including time-aware Sonnet 5 introductory
  pricing and explicit cache rates. Refresh stale GLM-5.1, Kimi K2.6, Trinity,
  Qwen3.6, Nemotron, Anthropic, GLM-5.2, Kimi K2.7 Code, GLM-5 Turbo, and
  GPT-5 Codex price or limit rows; keep Xiaomi MiMo explicitly unpriced where
  the provider's token plan and pay-as-you-go surfaces cannot be distinguished.
- MiniMax Messages provider support for MiniMax-M3 and MiniMax-M2.7, with
  OpenAI-compatible and Messages routes, regional endpoint guidance, request
  coverage, catalog limits, and tier-aware pricing (PR #4354 by @octo-patch).
- Dynamic MCP server infrastructure and an approval-gated tool that lets the
  model start a configured MCP server from chat context. Harvested from
  #3869 and #3866 by @bistack with authorship preserved.
- Parent `--disallowed-tools` restrictions now flow into sub-agents and Fleet
  workers by default, including deny-wins, wildcard, catalog-filtering, and
  multi-generation inheritance coverage. Harvested from #4096 by @JayBeest
  (#4042).
- Korean (ko) UI locale with full key parity and onboarding/setup wiring
  (PR #4347 by @moduvoice).
- Localize the entire underwater layer: 104 new UI strings — launch menu,
  phase words, mode/permission chips, footer hints, session picker, context
  inspector, route and theme pickers, Fleet roster, workflow status, sidebar
  work strip, repository-law approval copy, and file-attach titles — wired
  through MessageIds and translated into ja, zh-Hans, es-419, pt-BR, vi,
  and ko. Every complete pack now holds exact raw key parity with English
  (856 keys), enforced by new tests that the old English-fallback gate could
  not perform. The permission chip maps from typed state, so localization
  can never silently collapse it. Machine-authored translations follow each
  pack's existing terminology and are flagged for native review.
- Anthropic adapter: sanitize top-level `oneOf`/`anyOf`/`allOf` in tool
  input schemas so affected tools no longer fail the whole request with
  HTTP 400 (PR #4346 by @qinlinwang).
- Anthropic pricing: bill cache-write tokens at published rates
  (PR #4348 by @knqiufan, #4318).
- NetBSD: generate QuickJS bindings at build time so `codewhale-workflow-js`
  compiles (PR #4349 by @ci4ic4).
- Real-PTY release gates for six-worker fan-out liveness with Esc cancel,
  multi-terminal route isolation, queued steering via terminal-safe Ctrl+G
  (with Ctrl+S retained where the terminal forwards it), the one-shot
  completion footer, and per-theme ANSI output for every shipped palette.

### Fixed

- Make release publication complete and source-anchored: every build checks out
  the resolved tag commit, tag movement is rejected before GHCR, GitHub
  Release, Homebrew, Cargo, or npm writes, and registry helpers require a clean
  checkout exactly matching the remote tag. Manual recovery runs are
  exact-tag-only and execute the same parity gate as automatic tag pushes.
- Publish a coherent distribution set: both checksum manifests now contain
  usable public basenames and cover the full 29-asset matrix; GHCR, Homebrew,
  GitHub archives, and the Linux x64 CNB mirror carry `codewhale`, `codew`, and
  `codewhale-tui`. The CNB shortcut now fails clearly outside Linux/OpenHarmony
  x64 instead of promising assets that the mirror does not build.
- Preserve task text when a skill is invoked through dollar, unified-slash, or
  explicit skill syntax, while keeping bare skill invocations and management
  subcommands intact (PR #4372 by @nightt5879, co-authored by @CCChisato;
  #3915).
- Honor MCP server discovery capabilities: require advertised or legacy
  `tools/list`, keep optional resource/template/prompt probes independently
  bounded and fail-soft, and format descriptions Unicode-safely (#4308,
  harvested with co-authorship from @nsfoxer).
- Age-evict terminal sub-agent worker records from the state ledger so
  long-lived, high-fan-out sessions do not keep rewriting multi-megabyte
  terminal history (#4217; root-cause and fix direction from @yekern).
- Resolve the sub-agent completion/cancellation race with one terminal-state
  claim: cancellation suppresses late mailbox/parent/UI delivery, while a
  completed result remains publicly running until its notification is safely
  delivered.
- Keep Workflow panel controls from stealing ordinary composer letters. Enter,
  Delete, Up/Down, and Esc own panel actions; typed characters return focus to
  the composer and start the message normally.
- Preserve the composer prompt gutter from the first typed character through
  wrapping, scrolling, cursor placement, and mouse hit-testing so the `>` does
  not disappear or make input appear to jump.
- Emit terminal-native OSC 8 metadata for rendered URLs without placing escape
  payload bytes in the measured text, keeping long links visible, selectable,
  and clickable in supporting terminals.

- Keep headless structured output terminal-clean: `codewhale exec` engines
  no longer emit interactive terminal-title/taskbar OSC sequences, so
  `--output-format stream-json` stdout stays parseable, escape-free JSONL.
  Interactive TUI sessions keep their terminal chrome.
- Localization honesty: the parity gate was blinded by its own English
  fallback — two keybinding rows (`KbCyclePermissions`, `KbCycleThinking`)
  were missing from all five "complete" packs and now ship translated; the
  Operate-mode copy that drifted in English was retranslated in every pack
  (including zh-Hant's slice); three MessageIds absent from
  ALL_MESSAGE_IDS are visible to tests again; and the `/config`
  theme/locale hints and the invalid-locale error derive from the shipped
  registries instead of stale hand lists that advertised 4 of 12 themes
  and 4 of 8 locales.
- The setup wizard's constitution step no longer claims a "55-line core"
  in any language (the bundled core is larger today); the guided draft says
  "the bundled core stays active" instead.
- In-app selection copy is rail-clean and now regression-tested: copied
  transcript text excludes the `▎ ╎ │ ●` decorations via cache metadata
  (#4208 — thanks @eugenicum for the report and code-aware fix direction;
  terminal-native selection with mouse capture off remains a product
  decision on the proposed `rail_style` option).

### Docs

- Stamp every 0.9-era roadmap document with an explicit status (current,
  historical, superseded, principle-only, or future RFC), correct trackers
  that recorded unshipped work as done, and describe what remains after
  v0.9.0 in `docs/AGENT_RUNTIME.md`.
- Add `docs/rfcs/UNIFIED_PROVIDER_LOGIN.md`: one `codewhale auth login`
  surface for Anthropic, OpenAI Codex, and xAI, with the Anthropic adapter
  gated on verifying flow permissions before any constants are adopted.
- Refresh `docs/ACCESSIBILITY.md` for treatment-independent ambient life
  and the completed ASCII tier.

### Changed — runtime foundations

- Make the advertised Android/Termux release target buildable by generating
  QuickJS bindings against the Android NDK instead of expecting an upstream
  pre-generated `aarch64-linux-android` binding file, and give Android CLI/TUI
  HTTP clients a preconfigured rustls root store (Mozilla WebPKI roots) so
  standalone Termux processes stop panicking inside
  `rustls-platform-verifier`'s JVM expectations (#4236, #4242).
- Rebalance the bundled Constitution after the v0.8.67 prompt ablation: keep
  the procedural policy tail in mode-specific layers, while restoring concise
  behavioral guidance for momentum, causal investigation, constraint-first
  decisions, mechanism-backed guarantees, and clean continuity.
- Wire live catalog cache into provider/model pickers without dropping stale or
  prior rows after TTL expiry / refresh failure (#4139). Remove the dead
  `OFFERING_SEEDS` hand table so the bundled Models.dev catalog is the sole
  seed source; pickers show a compact `stale` / `cache failed` chrome chip when
  the Models.dev layer is past TTL or last refresh failed.
- Make `work_update` the sole model-facing To-do / Work progress tool (#4132).
  `checklist_*` and `todo_*` remain registered as hidden compat aliases for
  transcript replay; `update_plan` stays Strategy metadata/context/route, not
  a second checklist. Mode/approval prompts nudge the single surface.
- Demote the bundled Models.dev snapshot to an offline/stale fallback after
  live catalog refresh (#4188). ProviderLake precedence is live Models.dev >
  bundled seed > legacy hardcoded completion names; pickers, inventory, and
  subagent validation stay catalog-backed, and Codewhale-only providers keep
  defaults when Models.dev has no rows.

### Added
- Wire xAI device-code OAuth into `codewhale auth xai-device`, the TUI
  `/auth xai-device` command, and guided provider setup, with comment-preserving
  auth-mode persistence and loopback exchange coverage (#4257).
- Add GPT-5.6 Sol, Terra, and Luna to the OpenAI API route, including their
  1.05M context metadata, 128K output limits, pricing, and `max` reasoning
  effort. Add Meta Model API as a first-class OpenAI-compatible provider for
  Muse Spark 1.1 with 1M context, tool/reasoning metadata, provider aliases,
  and both `META_MODEL_API_KEY` and Meta's `MODEL_API_KEY` credential names.
- Catalog automation: `scripts/catalog_models_dev.py` refreshes secret-free
  Models.dev / OpenRouter listings and validates the offline seed snapshot
  (`snapshot --check`) without ever persisting API keys (#4117).
- `/model` picker cycles six catalog views with `A` (Configured → Catalog →
  Recent → Coding → Cheap → Long context) and richer row metadata from the
  live/bundled catalog (context, max output, tools, reasoning, price/M,
  freshness). Discoverability views do not auto-apply a surprising route
  (#4115).

- Workflow runs are now durable: every run appends to a
  `.codewhale/workflow-runs.jsonl` journal and hydrates on startup, so
  `workflow status` survives restarts; runs left `running` by a dead process
  are recovered as failed (#4011). The transcript renders workflow tool
  output as a run card (status, goal, children, progress, verification)
  instead of a generic one-liner (#4038), and `workflow` accepts a `verify`
  flag that runs post-completion verification gates and fails the run when
  gates fail (#4013).
- Hotbar sources for MCP tools and skills: MCP tool slots prefill the
  composer (execution stays behind the normal tool-approval flow) and skill
  slots activate through the existing `$skill` alias (#2068, #2069).
- Mode & permission surface: Tab cycles Plan → Act → Operate; Shift+Tab
  cycles the Agent permission posture (Ask / Auto-Review / Full Access) with
  a footer permission chip; Ctrl+T cycles reasoning effort and Ctrl+Shift+T
  opens the live transcript overlay. Operate is the orchestration mode
  (delegate, wait, inspect, dispatch) and raises sub-agent fan-out while
  focusing the Agents sidebar.
- Provider lake facade: the provider/model pickers, hotbar, and model
  inventory now enumerate configured providers' models from the bundled
  catalog (with an `A` toggle to browse the full catalog), replacing the
  hardcoded per-provider model table (#3830 follow-up).
- Added Cursor-integrated-terminal dogfood evidence for the published v0.8.67
  release, covering installed binary provenance, release/publication checks,
  headless runtime smoke, setup QA, and remaining manual visual TUI checks.
- README and README.zh-CN now point users to the community-maintained
  CodeWhale for VS Code GUI frontend while clarifying that this repository's
  `extensions/vscode/` scaffold remains the read-only Phase 0 viewer (#4035).

### Fixed

- Sub-agent waiting no longer peek→sleep polls: `agent(action="wait")` joins
  children, unchanged peeks are throttled (~30s) with an anti-polling nudge,
  and mode prompts teach the join primitive (#4097). Harvested from PR #4098
  by [@Mr-Moon121](https://github.com/Mr-Moon121) (Jeffrey Luna).
- `/provider` picker remembers catalog/configured view and highlighted row
  across reopen, matching `/model` picker memory.
- Mode picker roster is exactly Act / Plan / Operate (no Multitask, no
  numeric `4`/`5` gaps). Legacy `yolo`/`4` remain invisible one-way
  permission shorthand for Act + Bypass.

- Fleet setup is a role/profile roster editor, not a provider-scoped model
  picker: the Model step lists routes from every configured provider (not
  only the active one), a picked route's provider is persisted explicitly in
  the saved profile TOML (`provider = "..."`, never inferred from the model
  id), and the loader/route resolver read that field back out verbatim. The
  draft-preview save keypress no longer competes with a separate pager's
  `g`/`G` scroll bindings — the exact TOML preview now renders inline on the
  same Review step that saves it (#4093).
- `codewhale fleet run` and interactive in-process Fleet launches now honor a
  profile-pinned provider/model route instead of merely recording it on the
  receipt. Headless workers receive the non-secret `--provider` and `--model`
  pair; TUI workers resolve the same explicit route in process. Credentials
  still come from the worker's environment, provider is never inferred from a
  model id, and unpinned workers continue to inherit the run route (#4093,
  #4193).
- The Fleet setup `m` model-assisted redraft no longer drops a picked
  cross-provider route: the provider/model the operator chose are re-pinned
  onto the drafted profile (a model draft is always `provider: None`), so
  saving it keeps the explicit route instead of persisting an ambiguous,
  provider-scoped profile (#4093).
- Saving a Fleet profile now fails with a clear message when it pins a
  provider that has no configured credentials, using the same
  configured-provider check the model picker uses (#4093).
- Workflow correctness: completion polling fails closed instead of
  fabricating success when a sub-agent reports no terminal status; cancel
  interrupts the JS VM (cancel handle + abort) and blocks further spawns;
  and `budget.spent()` reports real manager-scope usage instead of always 0.
- Sub-agent spawns validate the model↔provider pair before dispatch:
  inherited/faster routes remap foreign models to the provider's catalog
  default, and explicit pins fail fast with a diagnostic instead of an
  upstream model-not-found error.
- TUI stability: engine event drains break every 8–16 events / 8 ms to keep
  input live (#1830, #2317, #1198); the terminal input pump restarts after
  stall recovery on macOS/Linux too; the startup raw-mode probe no longer
  leaks raw mode on timeout; recovery snapshots persist every 45 s during
  long turns and the offline queue persists on every push (#1830);
  queue/steer paths surface toasts while streaming (#2317, #1338); and
  modal submit errors re-open the modal instead of being swallowed (#1198).
- Core/state: paused jobs persist as paused across restarts; unarchive
  updates the in-memory cache; tool dispatch has a timeout; MCP
  notifications no longer receive responses; corrupted checkpoints surface
  errors instead of loading empty state; the session index compacts instead
  of growing unbounded; and recording thread-goal usage no longer
  self-deadlocks the state store.
- Runtime compaction summaries are now persisted into `/v1` thread records so
  engine reloads and restarts preserve compacted context. Contributed by
  MXAntian (@MXAntian) (#4091).
- The TUI leaves xterm alternate-scroll mode off when mouse capture is disabled,
  preserving native terminal text selection in light-theme/no-mouse-capture
  sessions. Contributed by Nightt (@nightt5879) (#4088, #4026).
- The public `/api/github/feed` endpoint is now forced dynamic on Cloudflare so
  it returns live GitHub activity instead of a build-time empty feed.

### Security

- Require bearer authentication for `/v1/chat/completions`, compare tokens in
  constant time, return accurate 4xx/5xx statuses, bound request bodies and SSE
  frames, redact secrets from stdio `config get`, and reliably reap the runtime
  child during shutdown.
- Keep trust precedence and secret persistence fail-closed: user ExecPolicy
  rules outrank agent-layer rules, chained commands cannot propose unsafe
  trusted-prefix amendments, and config and secret writes are atomic with
  filesystem synchronization on every supported platform.

### Changed

- Tool-hang watchdog trimmed from 15 minutes to 10 (#1862); approval modal
  footer hints use a higher-contrast tier (#3380); status/mode copy is
  disclosed once across header, footer, cards, and sidebar instead of
  repeated per layer.
- Removed the unused `tui::whale_routes` taxonomy module and its tests.
  Contributed by Darrell Thomas (@DarrellThomas) (#4041, #3852).

### Deprecated

- YOLO mode: `--yolo`, `default_mode = "yolo"`, and the hotbar YOLO action
  now map to Act + Full Access permissions via a compatibility shim and
  show a one-shot deprecation notice. Removal is deferred beyond v0.9.0 so
  this release does not break existing scripts without a dedicated cutover.

### Removed

- Remove the deprecated `deepseek` and `deepseek-tui` binary shims in this
  breaking release. `codewhale`, `codew`, and `codewhale-tui` are the supported
  entry points; existing DeepSeek provider support and legacy config/session
  migration remain intact.

### Known issues

- Android/Termux arm64 remains a preview in v0.9.0. The target, asset wiring,
  updater selection, dependency graph, and source-build path have automated or
  static coverage, but shell/PTY/config/TUI startup and runtime behavior remain
  unverified on a real device (#4236, #4242). Do not use a GNU/Linux arm64
  archive in Termux.

### Contributors

Thank you to the international community whose code, reports, reviews, and
reproductions shaped v0.9.0:

- [@amuthantamil](https://github.com/amuthantamil),
  [@bistack](https://github.com/bistack),
  [@bruce6135](https://github.com/bruce6135),
  [@CCChisato](https://github.com/CCChisato),
  [@ci4ic4](https://github.com/ci4ic4),
  [@cyq1017](https://github.com/cyq1017), and
  [@DarrellThomas](https://github.com/DarrellThomas).
- [@eugenicum](https://github.com/eugenicum),
  [@findshan](https://github.com/findshan),
  [@gaord](https://github.com/gaord),
  [@hmr-BH](https://github.com/hmr-BH),
  [@hongqitai](https://github.com/hongqitai), and
  [@idling11](https://github.com/idling11).
- [@JayBeest](https://github.com/JayBeest),
  [@knqiufan](https://github.com/knqiufan),
  [@LeoLin990405](https://github.com/LeoLin990405),
  [@moduvoice](https://github.com/moduvoice),
  [@mvanhorn](https://github.com/mvanhorn),
  [@Mr-Moon121](https://github.com/Mr-Moon121), and
  [@MXAntian](https://github.com/MXAntian).
- [@Angel-Hair](https://github.com/Angel-Hair),
  [@nightt5879](https://github.com/nightt5879),
  [@nsfoxer](https://github.com/nsfoxer),
  [@octo-patch](https://github.com/octo-patch),
  [@qinlinwang](https://github.com/qinlinwang),
  [@SamhandsomeLee](https://github.com/SamhandsomeLee), and
  [@taixinguo](https://github.com/taixinguo).
- [@WavesMan](https://github.com/WavesMan),
  [@wuisabel-gif](https://github.com/wuisabel-gif), and
  [@yekern](https://github.com/yekern).

## [0.8.67] - 2026-07-06

### Added

- The model you select in `/model` is now the operator: fleet workers whose
  task spec and roster profile pin no model inherit the active session route
  instead of a hardcoded `auto` sentinel, matching the pinned operator row in
  `/fleet roster`. Task-level and profile model overrides still win, and
  route receipts record which source applied (`task.model`,
  `agent_profile.model`, or `run.model`).
- Added the `/workflow` command (aliases `/workflows`, `/wf`) as the user
  opt-in to workflow orchestration. Bare `/workflow` orchestrates the current
  work — the model synthesizes the objective from the conversation context;
  `/workflow <objective>` narrows the run; `/workflow status [run_id]` and
  `/workflow cancel <run_id>` relay typed run receipts without starting new
  runs.
- Bare `/goal` with no active goal now declares a goal from the conversation
  context via `create_goal` instead of printing usage; with an active goal it
  remains the status readout, and explicit `/goal <objective>` is unchanged.
- Added the constitution-first setup wizard: a unified `/setup` shell with
  resume, back navigation, and skip-retry state; provider/model readiness
  cards with a custom-provider form and provider-picker detail layout; a
  runtime posture card with preset application and project-override warnings;
  a setup verification report; and transactional setup persistence with
  secret redaction and rollback (#3402, #3403, #3404, #3405, #3406, #3410,
  #3411).
- Added a structured user-global constitution with a deterministic renderer,
  prompt-block injection, guided principle authoring with preview and preset
  save, and a `/constitution` manager command as the primary constitution
  management surface, with file state shown in setup and actions surfaced in
  diagnostics (#3793, #3806, #3811).
- Added model-assisted constitution drafting behind an explicit ratify gate
  and fleet-profile drafting behind an explicit preview-before-save gate, with
  untrusted-draft provenance recorded so model-authored text is never applied
  silently. Updating users keep their existing constitution unchanged, and a
  localized constitution checkpoint is required after update (#3794).
- Added the Hotbar route editor v1 with route-switch slot actions and support
  for custom model routes, plus a configured-provider route manager for
  `/provider` and `/model` with a missing-auth handoff into provider key
  entry (#2066, #3830, #3831).
- Added auto-discovery of `.codewhale/rules/` and `.claude/rules/`
  directories as project context, with a total byte-budget cap on the
  assembled rules block. Contributed by maple (@yekern).
- Exposed `context_input_budget_for_route` from the engine so external
  integrations can reuse route budget math. Contributed by hexin
  (@h3c-hexin).
- Added GUI config persistence to the runtime API. Contributed by @gaord.
- Added a website localization matrix with a locale registry and drift
  checks. Harvested from #3763 by @idling11 (#3090).
- Added `doctor` detection of half-applied setup state, and startup milestone
  tracing for boot-performance diagnosis.
- Added a v0.8.67 computer-use dogfood prompt that covers the Cursor-terminal
  QA flow, headless gates, setup, sub-agent completion, Fleet, Workflow, model
  pricing, and release evidence collection.
- Fleet: local worker memory usage is now reported, including retained memory
  while a task is in Running status. Contributed by @cyq1017 (#3901).
- Website: community hub, constitution thesis page and constitution-centered
  homepage, models page generated from the provider registry, docs dark mode
  and full SEO metadata/sitemap coverage, terminal player for real
  constitution traces, and a live star badge and version.
- Added Meituan LongCat as a first-class OpenAI-compatible provider
  (`longcat`, with `long-cat`, `meituan-longcat`, and `meituan` aliases),
  `LONGCAT_API_KEY` discovery, the `LongCat-2.0` default model, provider
  picker wiring, model completions, provider docs, and web provider facts.
- Fleet: added per-provider setup cards (Persistence, Constitution, Hotbar,
  Tools/MCP, Remote Runtime) with a unified setup catalog and provider-specific
  credential links. Provider setup progress is persisted transactionally with
  rollback guards, Codex OAuth is kept out of provider key storage, and a
  headless QA contract verifies setup readiness across providers.
- Fleet: added Fleet starter profiles with role-aware loadouts (scout→Fast,
  manager→Inherit, etc.), `/fleet setup` profile-authoring wizard, Fleet
  effective-permission recording, and route intent-source tracking.
- Fleet: added 'operator' as a built-in Fleet roster member — the preferred
  helm Fleet slot for workflow coordination. Operator plans, routes, reviews
  outputs, and calls other Fleet slots as needed. This is a roster role, not a
  separate app mode. The full Operation/Operate-mode architecture is deferred
  to 0.9.0.
- Workflow: declarative workflows now run through the production driver, the
  workflow tool is wired to sub-agent dispatch, public Workflow surfaces are
  renamed, and typed workflow-run and status receipts are emitted for
  debugging and verification.
- Added provider-agnostic Fleet rosters and loadouts: provider-specific
  subagent limits, launch concurrency, and admission caps are derived from
  config without hardcoding any single provider.
- Added Workflow runtime foundations: the internal JS authoring/runtime crates
  compile and replay example workflows. 0.8.67 ships the `/workflow` opt-in,
  production-driver dispatch path, sub-agent task handoff, and typed run/status
  receipts; richer authoring UX and the full TUI run view remain tracked for
  v0.8.68 (#2974, #4038).

### Changed

- Clarified the Fleet coordination hierarchy and made roles carry real
  doctrine: the **operator** (the session's `/model` selection) runs the
  operation and assigns managers to workflows; a **manager** is the middle
  manager of exactly one workflow. The built-in **reviewer** is now explicitly
  adversarial (assume the change is broken, try to refute it), and the review
  sub-agent intro adopts the same framing. Built-in `manager`/`operator`/
  `reviewer` roster members now ship role `instructions` that flow into worker
  prompts on both the Fleet task-spec and agent/workflow `profile:` spawn
  paths; custom profiles override them via the same `instructions` field.
- Removed the decorative Fleet vocabulary that never routed differently:
  the `tool-heavy` slot and the `strong`/`balanced`/`deep-reasoning`/`code`/
  `review`/`tool-heavy` loadout tiers. `inherit` (the operator's route) and
  `fast` (the provider's faster class) remain; retired names in existing
  configs keep parsing (as custom labels) with identical auto routing, and
  the `/fleet setup` model-class step now offers only the real choices.
- Raised the default subagent concurrency for high-throughput fanout:
  `max_subagents` default 20 → 64 (config ceiling 128) and the queued+running
  admission cap 200 → 1024. Users on metered plans who want the old behavior
  can set `max_subagents = 20` in config.toml.
- Renamed the internal `whaleflow` subsystem to `workflow` across the
  workspace: the `codewhale-whaleflow`/`codewhale-whaleflow-js` crates become
  `codewhale-workflow`/`codewhale-workflow-js`, Rust identifiers and JS bridge
  symbols are renamed, the `CODEWHALE_WHALEFLOW_JS_*` environment variables
  become `CODEWHALE_WORKFLOW_JS_*`, and the authoring/RFC docs move to
  `WORKFLOW_AUTHORING.md` and `WORKFLOW_EXTERNAL_MEMORY.md`. Historical
  changelog and retro-ledger entries keep the old name as a record.
- Documented the Homebrew rollout strategy and added a distribution-channel
  check to the release checklist. Harvested from #3760 by @idling11 (#3489).
- Paused Linux RISC-V prebuilt release and nightly artifacts because
  `rquickjs-sys` 0.12.0 does not ship `riscv64gc-unknown-linux-gnu` bindings;
  installers, docs, and update paths now treat RISC-V as unsupported until
  upstream bindings or a bindgen-enabled build lands.
- Made the approval prompt calm, compact, and honest, and centered the
  first-run follow-up on the constitution; first-run onboarding now hands off
  into the setup wizard, and the language picker offers every shipped locale
  (#3929).
- Startup performance: boot janitors and store scans no longer block the
  first frame, `@mention` completion no longer re-walks the workspace per
  keystroke, and idle offline-queue clones and duplicate tool-output hashing
  were eliminated.
- Clarified the misleading "Ctrl+B backgrounds this command" shell wording
  (#3859) and the hotbar help shortcuts. Docs contribution by Chanhyo Jung
  (@roian6).
- Documented the enforced repo-law invariants, the constitution flow, and the
  `/fleet setup` profile-authoring wizard; aligned `permissions.toml` action
  docs. Docs contribution by @greyfreedom.
- Bumped web dependencies: wrangler 4.103.0 → 4.107.0, mermaid 11.15.0 →
  11.16.0, vitest 4.1.8 → 4.1.9 (@dependabot).
- Backfilled v0.8.67 regression coverage across sub-agent completion, budget
  exhaustion, delegate ordering, provider onboarding, setup scroll, model
  catalog pricing, Fleet routing, and Workflow gates (#4076).
- Split the large TUI debug command group and palette/theme internals into
  smaller modules without changing user-visible behavior (#4078, #4081).

### Fixed

- Fixed the goal sidebar elapsed timer so completed and blocked goals freeze
  their "completed in {elapsed}" readout instead of ticking forever. Goal state
  now records a `finished_at` instant that both sidebar render paths and the
  engine snapshot clamp elapsed against; `/goal resume` clears the freeze and
  the timer ticks again.
- Fixed paused goals silently un-freezing their sidebar timer: usage keeps
  accruing while paused, and the next goal snapshot used to clear the frozen
  instant. Paused goals now stay frozen until an explicit resume.
- Fixed durable `/goal` progress accounting so usage and continuation updates
  release the shared SQLite connection before re-reading the updated goal,
  unblocking resumed goal loops and full workspace release tests.
- Fixed a scheduled-automation race where deleting an automation while its
  run was being enqueued left the already-created task running untracked;
  the run record is now persisted unconditionally.
- Removed `panic = "abort"` from the release profile: it disabled unwinding
  and broke the panic supervision that keeps one failing tool call from
  taking down the whole session. The `lto`/`strip`/`codegen-units` size and
  speed tuning is unchanged.
- Fixed session save/load to persist and restore the active model provider
  across restarts. Previously sessions created under one provider (e.g.
  DeepSeek) would silently load under a different active provider. Provider,
  subagent limits, fallback chain, context window, and reasoning effort are now
  restored from saved session metadata, with `"deepseek"` as the default for
  legacy sessions.
- Raised the streamed model-response idle timeout and matched the TUI stall
  watchdog to the configured stream budget so long reasoning pauses are not
  recovered as stalled turns (#2487, #3998).
- Fixed Codex OAuth/sub-agent release diagnostics so `auth list` reports an
  active Codex OAuth file, Responses API child requests encode inherited tool
  names safely, rate-limited child requests checkpoint as resumable provider
  interruptions, and failure records surface the real Responses API error
  (#3884).
- Fixed fresh launch/setup testing with an explicit `CODEWHALE_HOME` so
  config, settings, theme prefs, and doctor legacy-state diagnostics do not
  inherit unrelated ambient `~/.deepseek` files (#4001, #4002).
- Sub-agent state now persists to `.codewhale/` instead of the lingering
  pre-rebrand `.deepseek/` path (#3864). Contributed by Stime (@yekern).
- `/plugin enable|disable` now persists across restarts (#3918), and the
  plugin command is hidden from the root slash menu and kept canonical after
  the scanner merge. Contributed by Nightt (@nightt5879).
- `/config ask-rules` now shows ask rule actions with improved diagnostics,
  with file-rule action precedence under test. Contributed by @greyfreedom.
- Fleet/sub-agents: enforced an absolute recursion-depth ceiling and widened
  task-id entropy, gave each atomic state write a unique temp path, kept
  sub-agent tool catalogs in parent parity (#3836), and made the Agents
  sidebar reconcile sub-agent completion and cancellation live (#3837).
- Fixed apply_patch mangling newlines, defaulted fuzz to 3, and made writes
  atomic; fixed compaction to preserve pins on emergency compaction, harden
  the summary fallback, and count image tokens; corrected backtrack boundary,
  checkpoint clear ordering, prune guard, and durable rename.
- Fixed the SSE client to flush the final frame, join multi-line data fields,
  and stop corrupting multibyte UTF-8 split across network reads.
- Kept review-only turns read-only, aliased `auto` mode to the agent policy,
  showed the mode-derived safety policy in status (contributed by @cyq1017),
  and stopped the durable-review floor from holding routine YOLO work
  (#3883).
- Fixed self-update to prefer exact binary release assets. Contributed by
  @LI-Jialu.
- UI polish: stopped constitution and fleet-profile model drafts from
  freezing the event loop, scoped the context-menu backdrop to the popup
  rect, stacked model-picker panes on narrow modals, unified display-width
  helpers on one contract (#3924), removed misleading success toasts,
  issue-number leaks, and dead-end empty states, and repaired the onboarding
  trust and api-key keys.
- Fixed the onboarding Trust step so plain Enter no longer silently grants
  workspace trust; users must choose the explicit trust or exit keys.
- Fixed same-root skill-name collisions being silently shadowed; duplicate
  normalized skill names now warn while keeping discovery deterministic
  (#3919).
- Normalized discovered skill names, removed unenforced trust copy, and
  surfaced the gated constitution override in prompts.
- Fixed a parallel `subagent::` suite flake where one test's process-wide
  `Retry-After` pause could strand unrelated budget-capped workers for the
  full stale window; requests now re-poll the global pause in bounded slices
  and the rate-limit test clears the window on drop.
- Sub-agent and Fleet reliability now fail empty, step-limited, and
  budget-exhausted children with explicit diagnostics instead of silent
  `Completed (no output)` success; budget exhaustion preserves partial output,
  `worktree: true` discovers one-level nested repos from harness directories,
  and completion-before-start delegate events recover into named rows instead
  of ellipsis-only identities (#4050, #4051, #4052, #4053).
- Goal-mode writing and research tasks can complete with
  `verification.status = "not_applicable"` without triggering continuation
  loops (#4054).
- First-run onboarding routes API keys through the selected provider, setup
  wizard bodies scroll with PageUp/PageDown, shipped locale packs are back to
  `en.json` parity with zh-Hant explicitly partial, stable feature flags stay
  out of Experimental, and model/provider rows include current LongCat and
  sourced-pricing hints (#4056, #4057, #4058, #4062, #4063).
- Running tool rows animate while a lone foreground tool is active, and
  workflow receipts render run/status/failure cards instead of one-line or
  null-success output (#4059).
- Model-facing turn metadata now includes a compact git workspace snapshot and
  escalates context pressure at the same thresholds as the TUI, helping agents
  narrow scope or compact before truncation (#4071, #4073).
- Successful child sub-agent completions inline the child's `EVIDENCE` block
  before the completion sentinel, so parents can cite child findings without
  re-running tools (#4072).
- Deferred tools hydrate and execute in the same batch when the original
  arguments are valid, and `[tools].always_load` now keeps configured MCP tools
  active instead of forcing the first-call retry. Thanks @SparkofSpike for the
  hot-path MCP report (#4074, #4027).
- New commit-range co-author checks reject bot/tool trailers on newly pushed
  commits; historical release-range cleanup remains a separate maintenance
  concern (#4075).
- Fixed fuzzy `edit_file` matching so matches that begin with multibyte UTF-8
  characters, including CJK text, advance on character boundaries instead of
  panicking. Contributed by Nightt (@nightt5879), reported by Taixin Guo
  (@taixinguo) (#3971, #4045).
- Fixed Unix dispatcher/TUI output under early-closing pipes such as
  `codewhale doctor | head` by restoring the default `SIGPIPE` handler before
  printing and propagating signal exits quietly. Contributed by @aznikline,
  reported by @BrathonBai (#4030, #4043).
- Suppressed dead_code warnings in the unused plugin registry module and
  fixed formatting across the command-group files. Contributed by Paulo Aboim
  Pinto (@aboimpinto).
- Pointed the website Community nav link at the community hub.

### Security

- MCP client hardening: closed an SSE-endpoint SSRF, bounded the HTTP
  response body via Content-Length instead of a streaming read, bounded stdio
  line reads to prevent OOM denial of service, fixed a dead timeout, and
  removed an unbounded buffer.
- Made execpolicy deny/trust rules segment-aware, closing a command-chaining
  bypass.
- Closed repo-law and safety-floor bypasses found by adversarial review:
  protected invariants are now enforced as mechanism, the destroyer gap in
  the safety floor is closed, a catalog-present tool with no execution path
  now fails closed, `web_run` open/click is classified as destructive, and
  the allow-list gained wildcard and case handling.
- Refused symlinked rules directories to prevent workspace escape via
  discovered rules. Contributed by maple (@yekern).
- Bounded Fleet sub-agent worker output so fanout cannot exhaust TUI memory
  (#3882), and preserved event headroom for progress. Contributed in part by
  @cyq1017.
- Added an untrusted constitution-draft gate with authoring provenance so
  model-drafted constitutions require explicit human ratification.

### Removed

- Removed unused model-registry helpers. Harvested from #3872 by @cyq1017.
- Removed unused request-tuning metadata. Harvested from #3871 by @cyq1017.
- Removed dead fleet task helpers (#3894 by @cyq1017), the unused
  approval-cache container (#3845) and localization QA metadata (both by
  @nightt5879), the dormant tab collaboration subsystem (#3838), the legacy
  flash auto-router (#3839), the stale project_doc loader (#3840), ignored
  mock LLM placeholders (#3841), dead model-catalog helpers (#3842), the
  unused execpolicy amend module, and dead MCP/client retry helpers.
- Retired the deprecated `WHALE.md` context fallback (#3798).

## [0.8.66] - 2026-06-29

### Added

- Added `codewhale doctor` / `codewhale doctor --json` legacy-state
  diagnostics that compare known `~/.deepseek` state paths with their
  `~/.codewhale` counterparts and flag unmigrated or dual-root data (#3727).
- Added Sakana AI Fugu as a first-class OpenAI-compatible provider with
  `sakana`/`fugu` aliases, `FUGU_API_KEY` / `SAKANA_API_KEY` discovery,
  provider-picker wiring, model completions, and provider docs. Harvested from
  #3748 by @lerugray.
- Added WhaleFlow-to-Fleet launch-shape validation: the default Fleet workflow
  contract allows up to 100 total agents and 5 recursive rings, requires
  bounded loops/expands before launch, and preserves per-slot model selection.
- Added a read-only `/config ask-rules` view for the resolved
  `permissions.toml` path, file status, rule count, and configured
  tool/command/path ask rules. Merged from #3569 by @greyfreedom.
- Added provider-level `context_window` overrides so OpenAI-compatible
  gateways and self-hosted providers can budget against their real model
  context window (#3545).
- Added the native `codew` shim to release archives, Windows installer inputs,
  local release-asset preparation, and checksum verification so manual installs
  receive the same short command that Cargo installs build.
- Added OpenModel as a first-class Anthropic Messages provider, with config,
  CLI, provider picker, docs, and registry coverage. Harvested from #3585 by
  @noaft.
- Added WeCom Bridge deployment and security documentation, with shipped
  runtime/bridge commands and approval-timeout environment guidance. Harvested
  from #3640 by @pkeging.
- Added a token/cache/cost `scorecard` command for offline release gating,
  baseline regression checks, and per-turn cost visibility (#3388). Stream-JSON
  exec metadata now also reports conservative `input_analysis` and
  `visible_final_answer_chars`, so benchmark harnesses can measure transcript
  growth and final-answer bloat without guessing (#2956, #2957).
- Added a release evidence ledger for v0.8.66 and opened the external ACP
  registry submission for CodeWhale after validating the published
  `codewhale@0.8.65` ACP auth handshake against the upstream registry checker
  (#3192).
- Added a typed `[verifier]` config table for the verifier-preview lane, with
  `enabled` and the shipped `verdict_policy = "hunt"` mapping documented and
  validated (#2093).
- Added Hotbar `Alt+1`–`Alt+8` quick-slot switching with decision-card key
  disambiguation, plus an introductory card that explains and can dismiss the
  Hotbar (#3796, #3788).
- Release/docs hygiene: guarded public install/version snippets and the npm
  `codewhaleBinaryVersion` pointer against drift, made `check-docs`/`check-facts`
  fail on stale snippets or unmapped providers, and stopped `sync-changelog`
  from dropping a release when only `[Unreleased]` exists (#3767, #3768, #3769,
  #3770, #3771, #3772).

### Changed

- Deferred Auto mode from the user-facing mode picker, cycle, hotbar, `/mode`
  command, and runtime-thread mode overrides until it has a distinct prompt and
  auto-review behavior; existing `auto` mode text now folds back to Agent
  instead of selecting a hollow mode, and approval modal copy no longer implies
  the current mode is YOLO (#3730, #3733).
- Clarified the Fleet setup surface and docs so Fleet is treated as the durable
  sub-agent configuration layer while WhaleFlow is the agent-authored
  orchestration plan that selects and monitors Fleet slots.
- Slimmed the default Constitution prompt while keeping its required structural
  anchors under regression coverage, reducing the static prompt footprint for
  cache-sensitive turns (#2953).
- Made the approval prompt inline and bottom-anchored instead of a full-screen
  takeover, so context and controls stay visible while a tool awaits a decision
  (#3799).
- The Hotbar is now hidden by default until explicit setup opt-in (#3807); the
  interactive Agent shell also defaults to approval-gated on with a shared
  baseline (#3756).
- Mode authority now resolves approval prompts through a single authority
  source instead of per-surface checks (#3795).

### Fixed

- Surfaced legacy state relocation with a user-visible migration notice whenever
  `~/.deepseek/<state>` is moved or copied into `~/.codewhale/<state>`, so
  upgraded users know their data was preserved and where the canonical state
  now lives (#3726).
- Restored legacy `.deepseek/sessions` visibility for upgraded installs where
  an empty `~/.codewhale/sessions` directory already existed, by copying
  missing legacy session entries into the primary CodeWhale session store
  without overwriting newer data (#3724).
- Calmed approval risk classification for read-only shell commands such as
  `codewhale --version`, `codewhale --help`, and `git status --porcelain` so
  the modal no longer labels proven read-only shell as destructive (#3730).
- Added provider/model route columns to `/cache` turn telemetry so DeepSeek
  cache-hit regressions can be correlated with Auto route changes (#3738).
- Fixed runtime API approval handling so workspace trust no longer auto-resolves
  ordinary tool approvals; trust now only participates in full-access retry
  decisions while YOLO/auto-approve remains the approval bypass (#3736).
- Fixed modal surfaces so the shared view stack paints an opaque backdrop before
  any overlay, while Plan/request-input popup interiors stay opaque and the Plan
  confirmation footer keeps action choices visible on narrow terminals (#3732).
- Added a turn-loop Plan-mode guard for file-writing tools and write-capable MCP
  tools so Plan's "no writes" promise is enforced before approval or execution,
  not only by the sandbox/catalog layer (#3734).
- Preserved the durable review safety floor for publish-like shell actions in
  YOLO mode, so `cargo publish`, `npm publish`, and tag/release pushes force
  approval instead of silently auto-approving (#3735).
- Fixed Ctrl+O external-editor freezes where CodeWhale's terminal input pump
  could keep reading keys while Vim/editor owned the terminal, especially in
  Windows mintty/cygwin shells. Thanks @buko for the precise repro (#3657).
- Hardened the OHOS dependency drift check against transient Cargo registry EOFs
  by retrying the dependency graph probe before failing CI.
- Updated the `/links` provider fallback to the current CodeWhale docs URL and
  added a Baidu Qianfan docs link. Harvested from #3621 by @noaft.
- Hardened `CODEWHALE_TOOL_SURFACE=shell-only` for benchmark/exec runs: the
  shell-only surface hides native tools from the model-visible catalog, and
  unknown `CODEWHALE_TOOL_SURFACE` values now warn instead of silently falling
  back to the full tool surface (#2954).
- Sub-agent fanout and lock hot paths: preserved event-channel headroom for
  progress events (#3783, thanks @cyq1017), let independent sub-agent starts
  join a single parallel dispatch batch instead of serializing (#3801), rendered
  the sub-agent sidebar/ListSubAgents from a read-only snapshot with bounded
  cleanup (#3803), used nonblocking best-effort sends for ListSubAgents refresh
  while still awaiting critical events (#3802), moved sub-agent state
  persistence disk I/O off the manager write lock (#3805), and used `try_lock`
  for shell-manager refresh in async UI paths (#3804).
- Provenance: runtime continuations and `SubAgentHandoff` now inherit standing
  YOLO authority, while `MemoryRecall`, `ImportedTranscript`, and
  `AssistantGenerated` inputs remain guarded (#3817).
- Approval honesty: labeled session-scoped approvals accurately instead of
  "always", and surfaced approval decisions in tool results (#3766).

## [0.8.65] - 2026-06-24

### Added

- **Provider/model/route resolution (EPIC #2608).** Canonical provider, model,
  offering, and route types with a single `RouteResolver` that produces a
  resolved `ReadyRouteCandidate` (endpoint, wire protocol, model id, context
  limit, price) for every switch (#3458, #3084, #3384). The executing client is
  now constructed from the resolved candidate rather than re-derived from config
  (#3384). A committed, network-free Models.dev-shaped catalog gives models real
  context windows and pricing, with a secret-free live cache (#3497, #3498,
  #3385). Offering pricing with provenance is projected onto candidates (#3501,
  #3085), and route limits feed a route-aware context-budget service (#3508,
  #3523, #3086).
- **Fleet execution substrate (EPIC #3154).** Fleet profile types and config
  (#3469), durable manager resume, workspace agent-profile loading resolved into
  the worker runtime (#3367), loadout intent carried in task specs (#3512), and
  receipts that persist the resolved route for inspection (#3154, #3166). Worker
  status is folded into the unified `/fleet` surface and exposed through the
  Runtime API.
- **Provider surfaces.** A `/provider` readiness dashboard with reasoning
  readiness, an experimental/supported maturity marker, and an "open models for
  this provider" action (#3083, #2984, #3485); cross-provider `/model` search
  with scroll and provider type-ahead (#3484, #3075); inline `<think>`
  reasoning-stream routing with per-provider overrides (#3222); usage telemetry
  normalized into canonical token classes including Responses cache-miss and
  reasoning tokens (#2961, #3509); and remote MCP OAuth login with bearer/header
  auth precedence (#3527).
- **More providers and routes.** User-defined OpenAI-compatible custom providers
  via `[providers.<name>]` (#1519); a DeepSeek Anthropic-compatible route (#2963,
  #3449); a Qianfan route (#3425); Zhipu folded into Z.ai with equal-treatment
  model normalization (#3539); DashScope/Together fixtures.
- **Localized mode picker and composer indicators.** The `/mode` picker prompt,
  mode names, and hints, plus the composer's Vim mode indicator, now render in
  all seven shipped locales (model-facing mode labels stay English). Harvested
  from #2239 by @gordonlu.
- **Website and automation.** A runtime/integrations page, provenance and
  mirror-trust copy, a fact-drift CI gate, a published install script, and a
  weekly community digest archive on codewhale.net (#3419, #3421, #3415, #3482,
  #3420); per-automation mode/shell/trust/approval settings (#3467).
- **Model reference browser.** A read-only `/modeldb` command (aliases
  `model-reference`, `modelref`) opens a pager over the bundled catalog — every
  model's factual context window, max output, modality, and price, grouped by
  provider/kind. Labels only: it never selects, routes, or tiers a model
  (#3205, #2300).
- **Transcript presets.** A `/config preset <name> [--save]` mechanism with a
  first `calm` preset — calm mode, calm tool collapse, comfortable spacing, and
  low motion — presentation-only and evidence-preserving (#3478).
- **Model capability profiles.** A typed `model_profile` module separates
  intrinsic model facts from resolved provider-route capability, so compact
  routes defer heavier nonessential tools while standard/full routes keep the
  eager tool surface (#3451, #3365).
- **Live provider catalog refresh.** A secret-free `/models` live-fetch layer
  (401/403/404/429 mapped to typed outcomes) feeds the catalog cache; the API
  key authorizes the request but is never persisted into the delta or cache
  (#3385).

### Changed

- **Config modularization (#3311).** `ProviderKind` (#3505), harness posture
  (#3507), and provider default seeds (#3503) moved into dedicated modules, and
  the `config.rs` monolith split into clean leaf modules (paths, search,
  model/base-URL constants, sub-agent limits) behind a `pub use` facade.
  `AppMode` helpers were centralized (#3510), and mode-vs-permission policy is
  now derived through a single `base_policy_for_mode` resolver instead of
  scattered mutation (#3386, advisory review-intent behavior preserved).
- **Leaner tool surface.** Dropped `task_shell_*` from the active set and folded
  `tool_search_*` (#3463); ablated the in-turn loop_guard and encoded reasoning
  dispositions (#3462); added the Orchestration disposition to the constitution.
- **Routing.** Provider/model switches and the capability-aware fallback chain
  resolve through `RouteResolver`; reasoning effort is normalized for the
  *resolved* provider; the fallback chain now skips providers that lack auth
  (#2574); and context window and memory-pressure come from the resolved route
  (#3086).
- **UX.** Approval modal gained a group divider and selected-row caret (#3515);
  picker scroll/type-ahead and selection contrast hardened (#3500); the README
  was rewritten as an architecture end-cap (#3087); and repo agent guidance was
  de-hardcoded to live truth.
- **Fleet identity and defaults.** Fleet workers now enter with an explicit
  "summoned Fleet member" operating contract, setup/profile prompts keep the
  default model behavior as same-route inheritance, and generated worker
  instructions avoid leaking recursive topology that only the orchestrator
  needs.
- **Legacy swarm cleanup.** Removed the obsolete `/swarm` core command/menu
  registration so `/fleet` is the product surface, while `/subagents` remains a
  compatibility shortcut to worker status.
- **Running-state animation.** Tool cards and background-task rows now share one
  faster braille spinner cadence, so Bash/background work reads consistently
  alive across the transcript and sidebar.
- **Restored contributor credit.** Threaded machine-readable credit
  (`docs/CONTRIBUTORS.md` + `.github/AUTHOR_MAP`) for earlier merged work that
  shipped without it, including the `/jobs cancel-all` action and the npm
  retry-timeout hint (#1538) by @jieshu666, and the community ACP adapter
  reference by @rockeverm3m.

### Fixed

- **Release hygiene.** The strict `cargo clippy --workspace --all-targets --locked
  -- -D warnings` gate passes; `npm run build` no longer dirties the generated
  web facts; the site sets `metadataBase`; the community digest page parses each
  record independently and localizes its chrome; and `cargo audit` is clean with
  the starlark-transitive unmaintained advisories documented.
- **Routing and mode correctness.** Ordinary prompt text is no longer
  interpreted as a mode switch (#3387, #3491); model candidates are scoped to the
  active provider; Together-owned DeepSeek routes are accepted (#3426); insecure
  `http://` custom endpoints raise an advisory warning (#1519); and the Fleet
  setup planner's role/model selection now drives the generated profile.
- **Runtime stability.** MCP connection drops are explicit (#3524), HTTP API
  calls reuse a shared MCP pool (#3532), and per-agent sub-agent mailbox
  telemetry is throttled to cut UI lag (#3454).
- **YOLO background-shell approvals.** A background shell command no longer pops
  an approval modal in YOLO mode. `classify_risk` marks all shell commands
  destructive, so the auto-review safety floor held every *background* shell for
  review, and the `ForcePrompt` site never checked `auto_approve` — only
  background commands surfaced it, since foreground shells take the
  `Interactive` origin and skip that branch.
- **Bash approval modal fit.** The shell approval modal now labels Bash
  commands directly, avoids repeating command/workdir in the impact summary,
  wraps long commands, and switches to compact controls on short terminals so
  the decision keys stay visible.
- **Custom-provider picker rows.** Concrete `[providers.<name>]` entries now
  appear in the provider picker (id, endpoint, auth readiness, wire protocol,
  current model) instead of only the generic placeholder; auth readiness honors
  per-entry key/env/metadata/no-auth/loopback.
- **Passive MCP tool discovery.** Runtime API-owned stdio MCP processes are no
  longer spawned from passive `/v1/apps/mcp/tools` requests; live discovery
  remains available through `?connect=true`. `doctor` now warns on relative-path
  stdio MCP commands without `cwd`.

## [0.8.64] - 2026-06-22

### Added

- **Seamless auto-compaction defaults.** Known large-context routes now keep
  automatic compaction on by default while carrying summaries forward through
  the stable prompt path, reducing surprise context loss without changing
  explicit opt-out behavior.
- **Runtime web automation readiness.** Local app automation gains a
  loopback-only dev-server readiness primitive so agents can wait for TCP and
  optional HTTP health checks before browser verification. Harvested from
  #3376 by @cyq1017.
- **Model and integration polish.** `/model pro` and `/model flash` shortcuts
  now resolve to the current DeepSeek V4 routes while preserving existing model
  IDs. Harvested from #3350 by @KUK4. The WeCom bridge landed with
  maintainer follow-up hardening for state permissions and chat-facing error
  reporting, from #3370 by @pkeging.

### Fixed

- **Security and trust-boundary hardening.** Project-local config can no longer
  loosen user-owned shell or instruction-file policy, file edits now require a
  fresh read of the target file, git history inputs reject option-shaped or
  control-character revisions, interactive execution surfaces require approval,
  and local tool paths are narrowed through workspace/root validation.
- **Runtime and diagnostics redaction.** Generated runtime/app-server tokens,
  raw session lineage identifiers, provider registry drift values, review
  receipt internals, and webhook URLs are no longer echoed into human-facing
  logs or diagnostics.
- **Network and alert safety.** Provider TLS verification bypass requests now
  fail closed, fleet alert webhooks require HTTPS, fetch URL hostnames are
  resolved before requests, and runtime mobile auth no longer relies on
  token-bearing URLs.
- **Path-state hardening.** Config sibling files, project MCP cwd values,
  runtime thread store files, sub-agent state, project-local state roots, and
  app-server sidecar config paths now resolve through checked roots before
  reads/writes.
- **Release CI repair.** Nightly cross-target builds install Rust targets
  explicitly and retry transient cargo failures; auto-tag runs are serialized
  and treat an already-created remote tag as a no-op. Safe slices harvested
  from #3374 by @donglovejava.
- **Provider wait and sidebar regressions.** Provider-wait footers suppress
  noisy countdowns until useful while keeping timeout warnings visible,
  harvested from #3375 by @idling11. The pinned sidebar can render at a
  narrower 64-column boundary, harvested from #3371 by @donglovejava.
- **Delegated server cleanup.** Delegated `serve` / `app-server` children gain
  OS-level parent-death cleanup on supported platforms, completing the #3259
  follow-up from #3378 and #3317 by @wuisabel-gif.
- **ACP and sandbox correctness.** ACP sessions preserve multi-turn
  conversation history across prompt turns, harvested from #3372 by @xulongzhe.
  Worktree Git metadata writes are allowed through sandbox policy without
  broad trust-mode escalation, from #3356 by @cyq1017 and the #3355 report by
  @linletian.

### Changed

- **Community and dependency harvests.** The release train carries focused
  community-credit slices from #3379 by @greyfreedom, #3348 by @nightt5879,
  #3346 by @hongqitai, #3345/#3333 by @cyq1017, and Dependabot updates for
  `windows`, `toml`, `tokio`, `lru`, `similar`, and web tooling security locks.
- **Public release surface cleanup.** Benchmark-specific materials were kept
  out of the public release repo; benchmark source fragments belong in the
  separate `codewhale-bench` lane.

## [0.8.63] - 2026-06-19

### Added

- **Sub-agent fanout safeguards (#3318, #3319).** High-fanout Workflow runs can
  now queue and drain more agents than the instantaneous concurrency cap by
  default, with `[subagents] max_admitted` available to tune that bounded
  admission population. Distinct `agent` calls are no longer capped by the
  per-turn loop guard before runtime launch concurrency and provider
  rate-limit backoff can apply. `[subagents] token_budget` applies a shared
  aggregate token ceiling to a root `agent` run and its descendants.
- **Per-worker sub-agent token enforcement (#3321).** A `token_budget` /
  `max_tokens` set on an individual `agent` call now bounds that single worker
  mid-run: once its accumulated model tokens exceed the cap it stops cleanly
  with a `budget_exhausted` status instead of running to `max_steps`. This
  complements the scope-level admission gate (#3319) — the per-worker cap stops
  one runaway worker, the scope cap bounds total fan-out — without
  double-counting. Harvested from #3321 by @donglovejava.
- **Provider-specific sub-agent fanout config.** `[subagents.providers.<provider>]`
  profiles now override `enabled`, `max_concurrent`, `max_admitted`,
  `launch_concurrency`, `max_depth`, token budget, API timeout, and heartbeat
  timeout for the active provider. Use broad direct-API profiles such as
  `[subagents.providers.deepseek]` and tighter subscription profiles such as
  `[subagents.providers.glm]`; `/config subagents status` shows both global
  and active-provider resolved values.
- **Sub-agent control and isolation.** The single `agent` tool now exposes
  status, peek, and cancel actions for running children, and accepts
  `worktree: true` to create an isolated git worktree/branch for parallel edit
  lanes instead of requiring callers to hand-roll a `cwd`.

### Fixed

- **Mode and tool catalog correctness.** Core action tools remain discoverable
  in the model-facing catalog/tool search, and a consistency self-check flags
  registered handlers that drift out of the advertised catalog. Review-looking
  prompts in explicit Agent/YOLO mode now keep the requested mode and tools,
  with only an advisory review hint.
- **Sub-agent orchestration recovery.** Child agents now retry transient
  provider header/SSE timeouts before failing, and parent runs synthesize missed
  child completions from terminal child state so orchestration cannot hang on a
  lost completion event.
- **DeepSeek thinking tool calls.** DeepSeek chat-completions requests now omit
  explicit `tool_choice` whenever reasoning/thinking is enabled, avoiding
  provider rejections while leaving no-thinking routes unchanged.
- **Task sidebar shortcuts and attribution.** Ctrl-K stays palette/emacs-kill,
  while Ctrl-X is scoped to Tasks-sidebar background shell cancellation. Shell
  jobs launched by sub-agents now render with their child-agent owner in the
  Tasks sidebar and transcript.
- **Long-turn recovery and context economy.** Repeated read-only search
  loop blocks now return guidance instead of fatal tool failures, Python build
  failures that are missing `setuptools` include an install/retry hint, long
  foreground shell timeouts steer models toward background execution, and noisy
  shell/test/web outputs are compacted earlier for large-context routes.
- **Config display redaction.** `codew config get/list` now recursively masks
  token-, secret-, password-, credential-, and authorization-like keys inside
  unknown `extras` tables and redacts sensitive HTTP header values before
  printing config output.
- **Queued follow-up hints and force-steer keys.** The pending-input preview now
  advertises `Ctrl+S send now` whenever queued follow-ups exist, and
  Ctrl/Cmd+Enter force-steering also accepts the common Ctrl+J terminal
  encoding while a turn is running.
- **Sidebar default visibility restored (#3328).** New and upgraded sessions
  now use a pinned composed sidebar by default when the terminal is wide
  enough, so live Agents and Tasks surface without opting back into idle
  auto-collapse. Older settings files that captured the v0.8.62 auto-collapse
  default now migrate to `pinned` unless `/sidebar auto --save` records an
  explicit opt-in. `/sidebar` now reports when width or auto-collapse
  suppresses rendering instead of saying the sidebar is visible. Reported by
  @dxfq.
- **JavaScript execution proxy env handling (#3273, #3331).** `js_execution`
  now enables Node's environment-proxy mode when proxy variables are present,
  mirrors lowercase proxy variables for the child process, and backfills
  `HTTP_PROXY` / `HTTPS_PROXY` from `ALL_PROXY`. Reported by @lordwedggie and
  harvested from #3331 by @cyq1017.
- **Legacy app-server non-loopback auth hardening (#3258).** Bare
  `codewhale app-server --host 0.0.0.0` now fails fast unless an explicit
  `--auth-token` or `CODEWHALE_APP_SERVER_TOKEN` is supplied, keeping generated
  one-time `cwapp_*` tokens loopback-only.
- **Legacy `.deepseek` state write-path migration (#3240).** State subdirectories
  (`sessions`, `slop_ledger`, `trophies`, `catalog`) are now always written under
  `~/.codewhale/`, and the first write of a subdir relocates any pre-existing
  `~/.deepseek/<sub>` contents into the primary location so the legacy tree stops
  growing while old data is preserved. The read resolver still finds legacy data
  for backfill until each subdir migrates. Reported by @Final527; onboarding
  marker slice from #3302 by @nightt5879.
- **State subdir validation on Windows (#3240).** State path hardening now
  rejects rooted/prefixed subdir strings such as `/etc` before resolving or
  migrating state directories, keeping the `.codewhale` write resolver inside
  its state root across platforms.

## [0.8.62] - 2026-06-17

### Changed

- **GLM-5.2 is now the default direct Z.AI model.** `DEFAULT_ZAI_MODEL` resolves
  to `GLM-5.2` in both `codewhale-tui` and `codewhale-config`; the `glm-5.1`
  alias still resolves to `GLM-5.1` (the defaulting was decoupled from the alias
  arm so it no longer tracks the default). Docs and `config.example.toml` no
  longer describe GLM-5.2 as an opt-in preview.
- **GLM-5-Turbo registered as a real model** and wired as the faster/explore
  sub-agent sibling for the GLM family: a `GLM-5.2` parent routes
  faster/explore children to `GLM-5-Turbo` (direct Z.ai) and `z-ai/glm-5-turbo`
  (OpenRouter), instead of down to GLM-5.1. GLM-5.1 and GLM-5-Turbo themselves
  have no cheaper tier and keep children on the parent.
- **`type: "explore"` sub-agents default to `model_strength: "faster"`.** Bounded
  read-only lookup/search/status work now uses the cheaper same-family sibling
  automatically, unless an explicit `model` or `model_strength: "same"` is
  supplied. Non-explore roles keep the conservative `same` default.
- **GPT-5.5 / OpenAI Codex faster route stays on GPT-5.5** with reasoning
  resolved to `low` (the Codex Responses API has no true `off`, so the resolved
  effort is now honest `low` rather than `off` silently rewritten). No
  DeepSeek/GLM fallback is fabricated when no cheaper same-provider sibling
  exists. DeepSeek Pro→Flash routing and its no-thinking faster lane are
  unchanged.
- **Base prompt / delegate skill guidance** updated to encourage parallel
  read-only exploration (2-4 `type: "explore"` sub-agents) for broad repo,
  version, branch, release, and API-surface investigations, while keeping
  architecture, integration, and final verification in the parent. The
  delegate skill examples now use provider-neutral `model_strength` instead of
  hardcoded DeepSeek model ids.
- **Agent synthesis guardrails.** The base constitution now frames tools around
  sufficient evidence rather than open-ended persistence: extra reads, searches,
  and delegation must target a missing fact, and agents should answer with
  limits instead of broadening searches indefinitely. The runtime loop guard
  now blocks duplicate read-only/delegated calls earlier and caps repeated
  broad lookup/delegation loops in a single turn with a synthesis-forcing tool
  error. Guard metadata distinguishes exact duplicates
  (`identical_tool_call`) from no-progress loops (`no_progress_tool_loop`).
- **Sub-agent handoff and visibility.** Direct sub-agent completions are drained
  before the next parent model request, so finished children can wake the main
  model promptly instead of waiting for an empty-tool-use branch or idle engine
  path. Nested sub-agents now report completions to their immediate parent
  inbox; the main model still receives only direct-child completions, avoiding
  grandchild floods while preserving nested evidence flow. Sub-agent output
  guidance now requires child-agent provenance when a sub-agent relies on a
  child report: cite the child `agent_id` and the child's EVIDENCE line(s), and
  do not present child findings as directly verified facts. The sidebar orders
  sub-agents as a parent/child tree and annotates nested rows with parent and
  depth information in hover text.
- **Sub-agent summary provenance (#2652).** A sub-agent's free-text result is now
  explicitly treated as an unverified self-report rather than confirmed
  evidence. The completion sentinel carries `summary_kind: complete | truncated`
  so the parent model can branch on whether it saw the full report or a clipped
  excerpt. Short summaries (≤ 12,000 chars) get a soft "re-verify material
  claims" suffix; longer ones are head+tail truncated with an honest marker
  stating the elided middle is not retrievable via `retrieve_tool_result`.
  Every summary therefore carries exactly one boundary marker, never both.
- **Provider metadata centralization.** Provider env vars, config keys, aliases,
  and auth hints are now resolved through the shared `ProviderMetadata` registry
  across `codewhale-config`, `codewhale-tui`, and `codewhale-cli`, reducing drift
  between the provider picker, `codewhale auth`, `doctor --json`, and setup
  hints.

### Added

- **Agent clarification questions (#3102).** Agents now have a first-class
  `request_user_input` tool to ask the user structured clarifying questions
  through a modal UI surface instead of only emitting a chat message and hoping
  the user notices. Mirrors the approval/secret-request flow the harness
  already used for permissions. The tool accepts 1-3 questions, each with a
  header, an id, 2-4 selectable options (label + description), and
  `allow_free_text` / `multi_select` flags (both default to `false` for
  back-compat). Input is validated up front with actionable errors. Wired
  across all layers: the `request_user_input` tool, engine handling
  (`turn_loop` → `approval`), an interactive TUI modal (`UserInputView`) with
  full keyboard navigation, and the runtime protocol
  (`EventFrame::UserInputRequest` + `AppRequest::SubmitUserInput`) so headless
  / app-server clients can answer programmatically. Parity tests cover the
  wire round-trip and the omitted-flags default.
- **Transcript hyperlinks — out-of-band OSC 8 (#3029).** Clickable file /
  file:line / URL links now reach the terminal through a column-drift-safe
  path. Link payloads are embedded in-band by the markdown renderer, then
  extracted out of the ratatui buffer cells and re-emitted out-of-band by
  `ColorCompatBackend` — so the `ESC` bytes never occupy display columns or
  corrupt selection. Supporting terminals get live hyperlinks; others see the
  label text unchanged. Clipboard/selection extraction strips residual codes as
  defense-in-depth.
- **CodeWhale-only skill discovery gate (#3296).** New
  `[skills].scan_codewhale_only = true` limits session-time skill discovery to
  CodeWhale-owned roots (`<workspace>/.codewhale/skills`, `~/.codewhale/skills`,
  and any explicit `skills_dir`) while ignoring cross-tool directories such as
  `.claude/skills`, `.opencode/skills`, `.cursor/skills`, and `~/.agents/skills`.
  The default remains the broad compatibility scan.
- **Permission/ask runtime rules (#3295).** Sibling `permissions.toml` ask-only
  rules are now loaded by the TUI engine and applied to `exec_shell` before
  Auto/session approval shortcuts. Matching ask rules force an approval prompt
  in otherwise auto-approved flows and are rejected under
  `approval_mode = "never"`.
- **Runtime API no-auth documentation.** `docs/RUNTIME_API.md` now documents
  `codewhale app-server --insecure-no-auth` for loopback-only testing and warns
  against combining it with `--mobile` on `0.0.0.0`.

### Fixed

- **TUI polish.** The empty-startup welcome block is centered by the actual
  rendered text width, fixing the off-center layout left over from the old
  sidebar-oriented welcome composition. Streaming HTTP body read errors now
  explain whether CodeWhale can retry before output, or is surfacing a warning
  after partial output to avoid replaying and duplicating streamed text.
- **Config comment preservation.** Rewriting `config.toml`, `settings.toml`, or
  `tui.toml` now merges user comments and formatting back into the serialized
  document; if comment merge fails, the write falls back to plain serialized
  output rather than failing.
- **Snapshot gate respected for per-tool snapshots (#3292).** Per-tool snapshots
  now check `[snapshots].enabled` before writing, matching the existing
  session-level gate.
- **Poppler `pdftotext` detection (#1667).** The dependency resolver now probes
  `pdftotext -v` instead of `--version`, because Poppler treats `--version` as
  an input filename. Fixes detection on systems where only Poppler is installed.
- **Plan confirmation checklist visibility.** The Plan-mode confirmation modal
  now shows the active checklist under the plan details, so users can review the
  concrete `checklist_write` work breakdown before accepting or revising a plan.

### Retroactive credits

A credit-reconciliation pass found shipped community fixes that were never
recorded in this changelog. Crediting them now, with the version they shipped in:

- Global `~/.deepseek/AGENTS.md` fallback loading — thanks @manaskarra (fix) and @xfy6238 (report) (#1157, v0.8.27)
- CRLF SSE event parsing for MCP — thanks @reidliu41 (fix) and @djairjr (report) (#1309, v0.8.29)
- Reduce-motion default on VTE/flicker terminals — thanks @Geallier (report) (#1470, v0.8.34)
- `portable-pty` 0.9 upgrade for LoongArch64 — thanks @quentin-lian (fix) and @k0tran (report) (#1531, #1992, v0.8.46)
- `DEEPSEEK_ALLOW_INSECURE_HTTP` guard for LAN vLLM — thanks @F1LT3R (report) (#1656, v0.8.47)
- Hidden `reasoning_content` kept in English regardless of locale — thanks @cmyyy (report) (#1842, v0.8.47)
- `ExternalTool` abstraction layer — thanks @aboimpinto (#1794, #2294, v0.8.48)
- Ephemeral generated project context — thanks @Final527 (report) (#3058, v0.8.59)

## [0.8.61] - 2026-06-15

This release lands the **runtime control plane** for multi-agent work: the TUI stays
responsive while sub-agents run, sub-agents converge toward fleet-style durable workers
with per-role model routing, and provider/model routes are isolated per session. It also
folds in several community contributions.

### Added

- **WhaleFlow runtime foundations** — worker runtime profiles (role / permissions / shell /
  tools / model-route, with non-escalating child derivation), a cross-provider model registry
  with offline catalog hydration, and provider-readiness / context-budget / provider-adapter /
  resource-telemetry services. (#3217, #3071, #3072, #3073)
- **Per-role, heterogeneous-model sub-agent routing** — sub-agents can be assigned a model and
  provider per role (e.g. scout vs. synthesis; verifiers route to a fast model). (#2027, #1768)
- **Durable goal mode** — cross-turn goal progress with token/time accounting and a
  verifier-as-judge gate before a goal may complete. (#3215, #891, #1976, #2058, #2029)
- Parent-visible worker interaction contract — a recommended action per worker. (#3226)
- Maintainer GitHub workflow skills; ACP registry submission prepared. (#3192)
- OpenAI-compatible `/v1/chat/completions` endpoint on the legacy app-server HTTP transport,
  provider-neutral, with model registry resolution and configured-credential forwarding.

### Changed

- **Sub-agents converge toward fleet-style durable workers** — real worker lifecycle states are
  projected to the sidebar instead of a hardcoded "running", and a sub-agent returns a structured
  needs-input checkpoint instead of parking. (#3226, #3096, #3154)
- The per-turn runtime tag exposes capability posture instead of human-facing mode labels. (#3213)
- Independent shell and verifier work defaults to background jobs with nonblocking waits and a
  completion notification; blocking now requires an explicit wait. (#3212)
- Sub-agent launches now expose explicit `model_strength` and `thinking` controls to the model
  instead of hidden child-model auto-routing; `explore` work is documented as a good fit for
  faster models and `thinking: "off"`.
- Plan mode is strictly read-only (no shell tools), consistent with its runtime posture.
- `/swarm` is gated behind the durable worker substrate. (#3218)
- Legacy `deepseek` install/update path resolves to `codewhale`. (#2960, #2924, #2917)

### Fixed

- **TUI freeze when multiple sub-agents spawn (launch blocker)** — the terminal input pump runs
  off the render thread, AgentProgress events are coalesced, and sub-agents no longer park on
  input with no orchestrator to answer; a six-worker stress test guards input/render/cancel
  liveness. (#3216, #3096)
- Idle sub-agent completion notifications now resume the parent turn instead of waiting for a
  later user message; thanks @giovanni-paolilla for the deadlock report (#3266).
- **Provider/model route isolation** — provider and model state is session-local, and a
  mismatched provider+model tuple is rejected at the route boundary. (#3227)
- Route-effective context-window metadata, over-limit preflight, and bounded recovery from
  `context_length_exceeded` instead of re-looping. (#3204)
- Synchronous tools (`file_search`, `grep_files`, `list_dir`) are cancellable and no longer hold
  a turn open against cancellation. (#1791)
- MCP stdio proxy startup prompts no longer strand YOLO / non-interactive runs. (#2475)
- Stalled / failed background-shell recovery; configurable sub-agent API timeout. (#1737, #1786, #1806)
- Composer: reliable queued steering + Ctrl+S send (#3203, #3224); footer busy/idle indicator
  (#2982); CJK word-wrap (#963); clickable sidebar stop targets (#3028); live token throughput
  (#3190); auto-expiring terminal sub-agent cards (#3078).
- Linux glibc preflight in the installer/update path with a clear error. (#3207, #1067)
- Self-update retries transient GitHub metadata/asset failures and falls back from the GitHub
  REST API to the public `releases/latest` redirect before constructing release asset URLs. (#3232)
- Provider picker lists providers in neutral alphabetical order instead of hard-coding DeepSeek first; the active provider stays pre-selected. (#3076)
- Work sidebar no longer shows stale `phase now:` / `phase next:` strategy rows once the checklist
  is 100% complete.
- Plan mode no longer shortcuts investigation for requests that name a repository, URL, version,
  release, build state, bug, PR, issue, API surface, or local code path.
- Oversized pasted text stays editable in the composer, with a file backup appended at submit
  time for model access; thanks @idling11 (#3267, closes #3263).
- Bare digit keys `1`-`8` now insert text instead of firing hotbar slots; use `Alt+digit` for
  hotbar actions. Thanks @wjq2026 for the report and @DieMoe233 for the paste-path note (#3243).
- Kimi/Moonshot tool schemas normalize empty function parameters to a root object schema; thanks
  @jghwwnq for the provider repro (#3265).
- Novita defaults to its OpenAI-compatible `/openai/v1` endpoint so chat completions no longer
  404 out of the box; thanks @buko for the report and endpoint verification (#3255).
- Dependency security: `ws` pinned to 8.21.0 across npm packages to close remote memory-exhaustion
  DoS (dependabot).

### Community contributions

- Non-DeepSeek model pricing — thanks @mvanhorn (#3201)
- Telegram polling transport — thanks @cyq1017 (#3195)
- Mobile event history — thanks @RobertEmprechtinger (#3220)
- Runtime-API session save — thanks @gaord (#3199)
- Whale-accent rename — thanks @nightt5879 (#3197)
- `DEEPSEEK_BASE_URL` / `MODEL` honored in `exec` — thanks @hongchen1993 (#3221)
- VS Code read-only API documentation — thanks @cyq1017 (#3013)
- Atomic ask-only permission rule persistence — thanks @greyfreedom (#3233)
- DeepInfra provider support and release-surface follow-through — thanks @idling11 (#3235, closes #3231) and @nightt5879 (#3236)
- Editable oversized paste composer flow — thanks @idling11 (#3267, closes #3263)
- WeChat bridge (`integrations/weixin-bridge` via Feishu + Tencent OpenClaw) — thanks @VincentCorleone (#3206)
- Config robustness: atomic permission-rule save, one-time config `.bak` backup before the first changed write, `CODEWHALE_HOME` as primary config home, and accepting the dispatcher-written config shape (camelCase aliases + `[features.enabled]` table) so legacy/dual-written configs parse cleanly
- Dependency/CI bumps: docker login/qemu actions, softprops gh-release, download-artifact, vitest, @opennextjs/cloudflare, form-data, js-yaml, dompurify, ws

## [0.8.60] - 2026-06-13

### Added

- **Agent Fleet real-run cutover (#3154/#3096).** `codewhale fleet run` now
  launches durable workers through the headless `codewhale exec --output-format
  stream-json` path instead of the local simulation interpreter, with terminal
  worker events freeing leases so queued fleet tasks continue running.
- **Read-only shell parallelism (#2983).** The engine can now run conservative
  read-only shell calls in parallel, including strict `bash`/`sh`/`zsh -c`
  wrappers for whitelisted commands, while writes, stdin, background TTY work,
  redirects, pipes, command substitution, and follow-mode tails stay serial.
- **Declarative JS/TS WhaleFlow authoring (#3097).** WhaleFlow now accepts a
  compile-only `workflow({...})` JavaScript/TypeScript authoring form that
  lowers into the existing `WorkflowSpec` validator without executing user
  JavaScript.
- **Slash-menu Ctrl+P/Ctrl+N navigation (#3196).** The slash command menu now
  supports Ctrl+P/Ctrl+N movement without letting the global file picker steal
  focus while the menu is open. Thanks @1Git2Clone for the PR.
- **New models and first-party provider routes.** This release adds
  **GLM-5.2** (selectable on the Z.ai Coding Plan and over OpenRouter as
  `z-ai/glm-5.2`, alongside the existing GLM-5.1 default), a first-party
  **Z.ai** provider route, a first-party **StepFun / StepFlash** route
  (`step-3.7-flash`), and a first-party **MiniMax** route defaulting to
  `MiniMax-M3` with the M2.7/M2.5/M2.1 family selectable (#3187/#3191).

### Changed

- **README and contributor credits.** The README now has a shorter public
  overview and moves the full contributor ledger to `docs/CONTRIBUTORS.md`,
  preserving public thanks for [DeepSeek](https://github.com/deepseek-ai),
  [DataWhale](https://github.com/datawhalechina),
  [OpenWarp](https://github.com/zerx-lab/warp), and
  [Open Design](https://github.com/nexu-io/open-design).
- **Fleet-backed sub-agent direction.** Runtime docs now state the intended
  cutover clearly: "sub-agent" is role/UX vocabulary, while durable detached
  work should converge on the fleet-backed worker lifecycle with retries,
  receipts, and ledgered inspection.

### Fixed

- **Sub-agent eval no longer blocks by default.** `agent_eval` now returns the
  current projection immediately and delivers follow-up input without waiting
  for a running child to finish its provider call. Pass `block:true` for an
  intentional terminal wait.
- **Z.ai GLM thinking traces.** Direct Z.ai requests now use the documented
  `thinking` shape, preserve and replay `reasoning_content`, classify GLM
  reasoning streams as thinking output, and accept `ultracode` as a max-effort
  alias.
- **Claude skill archive compatibility (#2743).** `/skill install` keeps
  portable Claude-style skill folders supported while rejecting multi-skill
  Claude plugin archives clearly instead of silently installing only one skill
  and dropping plugin semantics. Thanks @AiurArtanis for the ecosystem request.

## [0.8.59] - 2026-06-12

### Added

- **Moonshot Kimi K2.7 Code model.** The Moonshot/Kimi provider now defaults to
  `kimi-k2.7-code`, recognizes `kimi`/`kimi-k2` aliases for that model, keeps
  explicit `kimi-k2.6` selectable, and adds the OpenRouter
  `moonshotai/kimi-k2.7-code` registry row.
- **Concise verbosity mode (#3052).** CLI noninteractive launches now default
  to concise prompt/output discipline unless overridden by config, env, or
  `--verbosity`, while interactive TUI launches remain normal by default.
  Thanks @cyq1017 for the PR.
- **Ephemeral generated project context (#3058).** Opening CodeWhale in a
  directory with no instruction files now keeps the bounded generated project
  overview in memory instead of creating `.codewhale/instructions.md`.
- **ACP registry auth metadata (#1447).** The ACP stdio adapter now advertises
  terminal authentication setup in `initialize.authMethods`, matching the
  registry's validation requirement.
- **Sidebar context menus (#3065).** Right-clicking the sidebar no longer shows
  `Paste`; clickable sidebar rows now offer their row command as the first
  context action.
- **Sidebar hover popovers (#3088).** Streaming turns now keep sidebar hover
  popovers responsive while continuing to throttle transcript/body mouse
  motion.
- **Dark-theme selection contrast (#3074, thanks @drpars).** Session, config,
  help, context-menu, and approval selections now use the muted selection
  background instead of the bright accent color.
- **Cursor-style activity metadata rows (#3146).** Dense successful tool-run
  summaries now render as a single muted `Explored ...` / `Updated metadata`
  row, include short command-family labels for successful generic verifier
  groups, and keep keyboard/mouse expansion and detail inspection intact.
- **Provider-wait observability (#3095).** Footer stall reasons now name the
  active provider/model route, idle seconds vs stream budget, and whether a
  fanout plan is still at `0 running` or dispatch is pending. Structured
  provider-wait incidents log once per turn from the main tick loop (not on
  every footer redraw).
- **Interactive fanout launch gate (#3095).** Direct sub-agent children queue
  behind a configurable semaphore (`[subagents] interactive_max_launch`,
  default 4) with a visible `queued: waiting for an interactive fanout slot`
  reason before their first model step.
- **Goal lifecycle controls.** `/goal` is now the primary command surface for
  session goals, with `pause`, `resume`, `complete`, `blocked`, and `clear`
  controls while `/hunt` remains a compatibility alias.
- **Persistent thread-goal API.** App-server clients can now set, get, and clear
  durable thread goals through `thread/goal/set`, `thread/goal/get`, and
  `thread/goal/clear`, backed by the state store with Codex-style status and
  token/time accounting fields.
- **Command-boundary ownership layers (#2888/#3055).** Built-in slash command
  metadata now lives in `commands/registry.rs`, slash parsing in
  `commands/parse.rs`, and handlers under group-owned command areas, preserving
  the existing dispatch surface while reducing future `commands/mod.rs` churn.
- **Approval-rule source metadata (#1186/#2971).** Runtime API
  `approval.required` events now include optional `matched_rule` metadata when
  an execution-policy rule caused the prompt. Thanks @greyfreedom for the PR
  and @Ram9199 for the audit-semantics discussion.
- **Localized tool-family labels (#2901).** Tool activity labels for read,
  patch, run, find, delegate, fanout, RLM, verify, think, and generic tool
  work now route through the shipped locale tables. Thanks @gordonlu for the
  PR.
- **Localized config section labels (#2918).** The interactive config view now
  localizes section and session/saved scope labels while preserving English
  search terms. Thanks @gordonlu for the PR.
- **Localized config editor labels (#2919).** The config editor modal now
  localizes edit labels, default/unavailable placeholders, and effective
  currency hints. Thanks @gordonlu for the PR.
- **Hotbar number-key dispatch (#3056).** Bare `1`-`8` now trigger bound
  hotbar slots only when the composer is empty, while `Alt+1`-`Alt+8` trigger
  slots regardless of composer text and overlays keep key ownership. Thanks
  @reidliu41 for the PR.
- **Voice dictation commands (#3051).** `/voice`, `/voice-send`, and
  `/voice-control` now record through `sox`/`rec`/`arecord`, transcribe via the
  active provider's chat-completions API, and insert transcripts at the
  composer cursor. The `voice.toggle` hotbar action dispatches the real voice
  command, with help and status text localized across all seven shipped
  locales. Thanks @huqiantao for the PR.
- **Thread rewind and snapshot restore API (#2808).** GUI clients can now call
  `POST /v1/threads/{id}/undo`, `/patch-undo`, and `/retry` to fork, roll back,
  or rerun recent thread turns, plus `POST /v1/snapshots/{id}/restore` to
  restore a workspace snapshot by id. Thanks @bengao168 for the PR.
- **Active provider fallback chain (#2773).** Configured `fallback_providers`
  now build an ordered primary-plus-fallback route that the TUI can report,
  advance through, and reset with `/provider fallback reset`, including footer
  visibility for fallback state. Thanks @idling11 for the PR.
- **Provider metadata registry (#3005).** Built-in provider ids, display names,
  defaults, env vars, config keys, aliases, and wire formats now live in a
  shared metadata registry, with the provider drift check covering the registry
  contract. Thanks @sximelon for the PR.
- **Hugging Face provider route (#2879).** Hugging Face Inference Providers now
  have first-class config, env, docs, and registry coverage for the
  OpenAI-compatible router, including `huggingface`/`hugging-face`/
  `hugging_face`/`hf` aliases and `HUGGINGFACE_*`/`HF_*` env fallbacks. Thanks
  @mvanhorn for the PR.

### Fixed

- **SSE data lines without spaces (#3152).** Chat Completions, Responses, and
  Anthropic stream readers now accept both `data: {...}` and `data:{...}` SSE
  frames, matching the spec and preventing providers that omit the optional
  space from streaming empty output. Thanks @wgeeker for the PR.
- **Runtime thread detail N+1 reads (#3141).** `get_thread_detail` now scans
  persisted turn items once and groups them by turn instead of reading the
  items directory once per turn, preserving item order while keeping large
  thread detail loads responsive.
- **Project-local hook trust boundary (#3140).** `.codewhale/hooks.toml` is now
  loaded only after the workspace is trusted in user-owned config, matching the
  project-local MCP trust model while preserving the documented shell-command
  hook contract.
- **Skill registry sync latency (#3139).** `/skills sync` now syncs registry
  entries with bounded ordered concurrency, so network latency no longer stacks
  one skill at a time while output order stays deterministic.
- **SiliconFlow China provider config (#2893/#2895).** `siliconflow-CN`
  now reads its own `[providers.siliconflow_cn]` / `[providers.siliconflow-CN]`
  table and falls back to `[providers.siliconflow]` only for unset
  `api_key`/`base_url`/`model` fields. Thanks @Artenx for the report and
  @idling11 for the PR.
- **Self-update download timeout (#3006).** `codewhale update` now applies a
  five-minute HTTP client timeout so blocked or very slow GitHub release
  downloads fail instead of hanging indefinitely. Thanks @New2Niu for the PR.
- **Legacy `deepseek` update migration (#2960/#3013/#3053).** Running
  `deepseek update` or `deepseek-tui update` from a pre-rebrand install now
  returns copy-pasteable npm, Cargo, Homebrew, and manual-binary migration
  steps instead of trying to spawn a missing `codewhale` binary. README and
  rebrand docs now cover the same upgrade path. Thanks @jazzi and
  @tiangangQiu for the reports, @cyq1017 for the update-path PR, and
  @angus-guo for the README PR.
- **Short `codew` shim delegation.** The `codew` convenience binary now
  prefers the sibling `codewhale` dispatcher installed next to it before
  falling back to `PATH`, preventing fresh local builds or installs from
  accidentally invoking an older global dispatcher.
- **Constitution trust wording (#2950/#3008).** The base prompt now explains
  that "begins with an A" means a baseline of trust, not a literal output
  formatting rule. Thanks @cyq1017 for the PR.
- **TUI provider-source recovery (#3007/#3011).** Unsupported interactive
  providers now report whether the value came from `--provider`, environment,
  or config. Config-sourced unsupported providers fall back to DeepSeek without
  forwarding stale keyring secrets. Thanks @cyq1017 for the PR.
- **Exec auto-model handoff (#3148).** `codewhale exec --model auto` now
  survives the CLI/TUI boundary by honoring the CodeWhale model env alias and
  legacy DeepSeek model handoff before falling back to provider defaults.
  Thanks @hongchen1993 for the PR.
- **macOS shortcut modifiers (#2938/#2943).** Ctrl-like shortcuts that are
  reported as `SUPER` by macOS terminals now work for backgrounding tasks and
  sidebar-focus chords without rewriting clipboard shortcuts. Thanks @idling11
  for the PR.
- **TUI mouse-report leak (#3063/#3067).** Strip raw SGR mouse coordinate
  tails from the composer even when `use_mouse_capture` is false, covering
  orphaned terminal reporting state after crashes or focus races.
- **Interrupted sub-agent lifecycle (#3080).** API-timeout interruptions now
  emit `MailboxMessage::Interrupted`, render terminal interrupted cards, and
  reconcile stale running fanout counts from manager snapshots.
- **OpenAI Codex stream diagnostics and active tool collapse (#3146).** The
  Responses bridge now reports nested `response.failed` /
  `response.incomplete` errors instead of `unknown`, and dense successful
  in-flight tool bursts collapse into the same calm activity metadata row as
  committed history.
- **OpenAI Codex reasoning tiers.** Switching from DeepSeek to `openai-codex`
  now normalizes stale reasoning state into Responses-compatible
  `low`/`medium`/`high`/`xhigh` tiers. Startup, `/config`, and the model
  picker now display Codex labels instead of leaking DeepSeek
  `off`/`max` names, while Codex still reports as a Responses payload
  provider. The Responses request builder also clamps legacy `minimal` input
  to `low` and has regression coverage that Codex requests use
  `reasoning.effort`, not DeepSeek `thinking` fields.
- **OpenAI Codex context metadata (#3070).** The `gpt-5.5` default and
  CodeWhale aliases now use OpenAI's documented 1,050,000-token context window
  and 128,000 max-output metadata for context pressure, prompts, and doctor
  capability output.
- **OpenAI Codex effective context budgeting.** The public OpenAI API metadata
  for `gpt-5.5` remains 1,050,000 tokens, but the `openai-codex` OAuth route now
  budgets prompts against the 400K Codex-family effective window so preflight
  compaction runs before the backend returns `context_length_exceeded`.
- **OpenRouter Nemotron 3 Ultra preset.** The OpenRouter preset and model
  registry now emit `nvidia/nemotron-3-ultra-550b-a55b` while keeping the old
  Ultra aliases compatible.
- **OpenRouter auth after MiMo switches (#3064).** Switching from Xiaomi MiMo
  to OpenRouter now has regression coverage for preflight key failures and
  Bearer auth header isolation before any request can be dispatched.
- **Responses strict-tool schema compatibility (#3062/#3017/#1883).** Responses
  function tools now preserve per-tool strict-mode compatibility, keep optional
  strict-schema fields nullable, and append deterministic constraint notes when
  root composition groups must be flattened for Responses.
- **Runtime prompt autonomous loop guard (#3061).** Runtime policy reference
  now explicitly forbids initiating new work when `<runtime_prompt>` is the
  only new turn content and no tool/sub-agent handoff is pending.
- **Goal runtime status sync.** Goal token budgets and active/paused/complete
  status now sync into the engine alongside the objective, and model-visible
  `update_goal` can only mark goals complete or blocked.

### Contributors

- Devin session work on #3080/#3095 (PRs #3103, #3104, #3106) — Hunter Bown
  (maintainer integration/cherry-pick on `codex/v0.8.59-release-ready`).
- Nightt (@nightt5879) for the Responses strict-tool schema hardening in PR
  #3062.
- yekern (@yekern) for the #3061 runtime-prompt loop safety report and repro
  that shaped the dispatch guard.
- Paulo Aboim Pinto (@aboimpinto) for the staged command-boundary design and
  Layer 3 registry/parser extraction in PR #2888, plus the #2851/#2791/#2870
  architecture stream that guided the grouped command areas in #3055.

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
  (`minimax/minimax-m2.7`), and NVIDIA Nemotron 3 Ultra (`nvidia/nemotron-3-ultra`)
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

---

Older releases: [CHANGELOG.md](https://github.com/Hmbown/CodeWhale/blob/main/CHANGELOG.md) and [docs/CHANGELOG_ARCHIVE.md](https://github.com/Hmbown/CodeWhale/blob/main/docs/CHANGELOG_ARCHIVE.md).
