# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.8.67] - 2026-07-04

### Added

- Added the `whaleflow` tool: the orchestrator model can author and run
  sandboxed dynamic-workflow scripts (`task()` fan-out, `tools.*`, live
  `budget`, `parallel`/`pipeline`) with agent-equivalent approval and depth
  gating, fleet-party profiles via `task({profile})`, and turn-cancel
  hard-kill of runaway scripts. Script `tools.*` calls run at full posture
  parity with the calling context: each call inherits that context's tool
  surface and goes through the same approval decision as a model-issued
  call, so approval-required tools prompt in manual modes rather than being
  silently blocked or silently run.
- Added the WhaleFlow dynamic-workflow bridge: a sandboxed in-process
  QuickJS runtime (new `codewhale-whaleflow-js` crate) where scripts call
  `await task({...})` to spawn real sub-agents (recursive, gated by the
  existing spawn-depth ceiling), `await tools.<name>(args)` against a
  read-only-by-default tool tier, and read `globalThis.budget`
  (total/spent/remaining) backed by the live budget scope with
  reservation-based overshoot protection; includes a `parallel`/`pipeline`/
  `log` prelude and start/complete/error events on the fan-out panel.
- Added Fleet party construction: `/fleet party` prints an honest roster of
  saved worker profiles with per-slot role, model class, and pinned model;
  `/fleet setup` gains typed custom roles and a `fast`/`heavy`/`omni`/
  `custom` model-class taxonomy with visible typed/pasted input; profiles
  support ranked model preferences (`models = ["best", "fallback"]`) that
  are pre-flighted against the active provider and degrade to the class
  route instead of failing on the wire; the orchestrator's `agent` tool
  advertises the party and spawns members via `profile:"<id>"` (WhaleFlow
  `task({profile})` too), with a one-time `/fleet setup` hint when no party
  exists.
- Added typed custom guidance to the Constitution setup step (`T`): free
  text (typing and paste) is sanitized, bounded, preview-gated, and saved
  into the user constitution as advisory guidance; expanded the guided
  choices with three new article-anchored options; `/constitution edit`
  now preserves previously ratified custom guidance.
- Added a grouped provider picker (Active / Ready / Available with
  truthful credential badges and a count line, full catalog by default)
  and a progressive-disclosure model picker (bounded shortlist with a
  details strip for the focused row; Ctrl+A expands to all models).
- Added recovery hints to error messages (context overflow suggests
  `/compact`/`/restore`; auth failures suggest `/provider`) and made
  `codewhale doctor` exit non-zero when checks fail.
- Added `NO_COLOR` / `CODEWHALE_NO_COLOR` / `DEEPSEEK_NO_COLOR` support
  (forces ANSI-16 with distinguishable named colors).
- Added a situationally-aware right-click menu (Copy leads during
  selection, Paste only over editable targets, Open/Copy link over links),
  wheel-scroll on modals/pickers/overlays that previously ignored it,
  bare `?` help on an empty composer, an idle-footer key-hint chip, and
  empty-state action hints.
- Added docs: CONSTITUTION, FAQ, CREDIT (how credit works), EXTENDING,
  RESTORE, DIRECTORY_STRUCTURE, ADDING_CONTEXT, SESSIONS, COST, a docs/
  index, and copy-pasteable workflow recipes; the website now renders repo
  docs as first-class pages with client-side search and `llms.txt`;
  bilingual (en/zh-CN) issue forms; README trust block, WhaleFlow section,
  and contributor wall.
- Added a weekly `cargo audit` CI job and a `[profile.release]` with thin
  LTO, one codegen unit, and symbol stripping.
- Added a website localization matrix with a locale registry and drift checks.
  Harvested from #3763 by @idling11.

### Changed

- Raised sub-agent capacity for program-driven fan-out: the concurrency
  default is now cores-scaled (16-64, absolute ceiling 256), admission
  queue 1000, with a 1000-agent per-session lifetime backstop and a
  10-minute completed-agent retention window.
- Reworked hot render paths: transcript markdown parses once per cell via
  an AST cache (streaming no longer re-parses the whole message per
  commit), the footer/header context meter memoizes token estimation
  behind a cheap fingerprint, the runtime event journal batches token
  deltas instead of double-fsyncing each one, web tools share HTTP
  connection pools, and tool-parser/hook regexes are compiled once.
- Made transcript tool/plan cards theme-aware (light themes no longer
  render dark-theme card colors), tuned all shipped presets to pass
  contrast floors, and added a guard test that keeps raw RGB literals out
  of non-palette modules.
- Onboarding: the trust screen's Esc now steps back (as documented)
  instead of quitting, and API-key copy is provider-aware instead of
  DeepSeek-hardcoded.
- Anchored the guided user-constitution creator to the shipped CodeWhale
  Constitution articles, so the configurable focus axis now maps to Ground
  Truth/Verification, Momentum/Legacy/Help, or Priority/Domain Context/Inquiry
  instead of generic preference prose.
- Documented the Homebrew rollout strategy and added a distribution-channel
  check to the release checklist. Harvested from #3760 by @idling11.

### Fixed

- Fixed sub-agent spawns on the ChatGPT Codex OAuth route failing with
  Responses-API 400s or premature timeouts (#3884 follow-up): foreign model
  pins (fleet profiles, role config, explicit `model:`) are skipped or
  rejected before the wire and degrade to the parent Codex model with a
  notice; unconfigured Codex children inherit the parent's 900s stream
  quiet-time budget instead of a 120s total cap; forked children keep their
  system-role fork context on the Responses route; completed children report
  their whale nickname; and `max_depth: 0` spawn requests are refused up
  front.
- Fixed the agent tool's fleet-party roster staying frozen at its spawn-time
  snapshot in long-lived sub-agent registries: it now refreshes when
  `.codewhale/agents` profiles are created, edited, or removed mid-session,
  fingerprint-gated so an unchanged party keeps the tool catalog byte-stable
  for prefix caching.
- Fixed `codewhale metrics` reading only the legacy `~/.deepseek` state so
  new installs silently reported nothing; it now resolves the CodeWhale
  home with a legacy fallback.
- Fixed fleet workers pinned to a model the active provider cannot serve
  spawning anyway and dying as opaque HTTP errors; unusable pins now skip
  to the next ranked preference or the class route, with the degradation
  shown in `/fleet party` and logged at spawn. Also fixed `fast`-class
  profiles without pins not routing to the faster lane.
- Fixed the wizard's advertised setup hotkeys being case-sensitive, the
  Constitution card clipping its controls at 80x24, and the keep-existing
  gate ratifying a constitution file changed on disk after preview.
- Fixed three command descriptions missing Simplified Chinese
  translations, `/cache` silently ignoring unknown arguments, `/sidebar`
  usage/error drift, stale keybinding documentation and in-app catalog
  entries, and a duplicate reqwest dependency tree pulled in via oauth2.
- Raised the streamed model-response idle timeout and matched the TUI stall
  watchdog to the configured stream budget so long reasoning pauses are not
  recovered as stalled turns (#2487).
- Fixed Codex OAuth/sub-agent release diagnostics so `auth list` reports an
  active Codex OAuth file, Responses API child requests encode inherited tool
  names safely, and rate-limited Codex child requests checkpoint as resumable
  provider interruptions instead of terminal opaque failures (#3884).
- Fixed fresh launch/setup testing with an explicit `CODEWHALE_HOME` so config,
  settings, theme prefs, and doctor legacy-state diagnostics do not inherit
  unrelated ambient `~/.deepseek` files.

### Security

- WhaleFlow scripts inherit exactly the calling context's tool surface
  (role posture plus explicit allowlist, minus delegation tools): a
  restricted sub-agent can no longer widen its posture by routing calls
  through `tools.*` inside a script.
- The WhaleFlow approval card labels its plan summary a static heuristic
  preview, always shows the "may spawn sub-agents and call any tool
  available to this context" capability warning, and quotes a script's own
  leading comment as untrusted instead of echoing it in the approval
  headline, desktop notifications, and audit log.

### Removed

- Removed unused model-registry helpers. Harvested from #3872 by @cyq1017.
- Removed unused request-tuning metadata. Harvested from #3871 by @cyq1017.

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

## [0.8.54] - 2026-06-08

### Added

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

---

Older releases: [CHANGELOG.md](https://github.com/Hmbown/CodeWhale/blob/main/CHANGELOG.md) and [docs/CHANGELOG_ARCHIVE.md](https://github.com/Hmbown/CodeWhale/blob/main/docs/CHANGELOG_ARCHIVE.md).
