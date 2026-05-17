# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.8.39] - 2026-05-17

### Fixed

- **Feishu/Lark bridge startup order is guarded.** The bridge now keeps
  `ThreadStore` initialized before startup opens persisted thread state, with a
  regression test to prevent moving it below its first use.
- **`/model` picker opens instantly with the curated list again.** Reverted
  the v0.8.38 live-catalog rework: the picker no longer makes a blocking
  network call on open and once again shows the curated `auto` /
  `deepseek-v4-pro` / `deepseek-v4-flash` rows. The `/models` command still
  lists the live provider catalog.
- **"Approve for session" groups by command family again.** Session approvals
  are keyed by a lossy, arity-aware fingerprint once more, so approving
  `cargo build` also covers `cargo build --release`. Denials keep the exact
  per-call fingerprint from #1617, so denying one call no longer over-blocks
  later, different calls to the same tool.
- **Docker first-run state directories are writable.** The image now
  pre-creates `/home/deepseek/.deepseek` with `deepseek` ownership so the
  documented named-volume launch can create runtime thread state on first use
  (#1684).
- **Runtime API system prompt overrides survive the first turn.** Threads
  created with a `system_prompt` override now keep that prompt through
  mode/context refreshes before the model request is built (#1688).
- **Compaction keeps a user text query in tool-heavy histories.** Automatic
  compaction now pins the latest user text message when the retained tail only
  contains tool calls/results, avoiding OpenAI-compatible Jinja template
  failures on the next request (#1704).
- **Pager jumps land at the visible bottom.** Pressing `G` or End in the pager
  no longer overshoots the render clamp, so `k`/Up scrolls upward immediately
  afterward, and mouse wheels now scroll pager overlays directly (#1706,
  #1716).
- **Mouse-wheel-as-arrow scrolling preserves composer drafts.** When
  `composer_arrows_scroll` is enabled, Up/Down now scroll the transcript even
  with text in the composer instead of replacing the draft with input history
  (#1677).
- **Multiline composer arrows move between input lines.** Plain Up/Down now
  move the cursor within multiline drafts before falling back to input history,
  while single-line mouse-wheel-as-arrow scrolling remains unchanged (#1721).
- **Third-party `reasoning_content` streams no longer corrupt text output.**
  Generic OpenAI-compatible providers that stream answer text in
  `reasoning_content` now render it as normal text unless the selected provider
  is one whose reasoning-content semantics are supported (#1673).
- **macOS system theme detection recognizes Light mode.** When `COLORFGBG` is
  missing or unusable, `theme = "system"` now falls back to macOS appearance
  detection and treats a missing `AppleInterfaceStyle` key as Light mode
  (#1670).
- **`rlm_open` accepts schema-filled blank source fields.** Empty `file_path`,
  `content`, and `url` strings now count as absent, so calls that provide one
  real source no longer fail the exactly-one-source validator (#1712).
- **Resize keeps transcript paging usable immediately.** After a terminal
  resize, PageUp/PageDown now use the resized viewport height instead of
  falling back to one-line jumps before the next render (#1724).
- **ACP responses stringify JSON-RPC ids.** `serve --acp` now returns string
  ids even when clients send numeric ids, matching Zed's stricter ACP client
  expectations (#1696).

### Thanks

Thanks to **Matt Van Horn ([@mvanhorn](https://github.com/mvanhorn))** for the
Docker first-run permission fix in #1699 and the runtime system-prompt
regression tests harvested from #1702. Thanks to **Kristopher Clark
([@krisclarkdev](https://github.com/krisclarkdev))** for the compaction
user-query preservation fix in #1704. Thanks to **Stephen Xu
([@wlon](https://github.com/wlon))** for the pager jump-bottom fix in #1706.
Thanks to **tdccccc ([@tdccccc](https://github.com/tdccccc))** for the
composer scroll fix in #1715 and pager mouse-wheel support in #1716.
Thanks to **Paulo Aboim Pinto
([@aboimpinto](https://github.com/aboimpinto))** for the multiline composer
arrow navigation tests harvested from #1719. Thanks to **LittleBlacky
([@LittleBlacky](https://github.com/LittleBlacky))** for the provider-gated
`reasoning_content` stream fix in #1680.
Thanks to **Eosin Ai ([@Aitensa](https://github.com/Aitensa))** for the macOS
system appearance fallback in #1674.
Thanks to **Anaheim ([@AnaheimEX](https://github.com/AnaheimEX))** for the
`rlm_open` schema validation report in #1712.
Thanks to **THatch26 ([@THatch26](https://github.com/THatch26))** for the
terminal resize paging fix in #1724.
Thanks to **Alvin ([@alvin1](https://github.com/alvin1))** for the Zed ACP id
compatibility report in #1696.

## [0.8.38] - 2026-05-15

### Changed

- **Update guidance is clearer on the website.** The homepage and install page
  now surface `deepseek update` while keeping package-manager update paths
  visible for Homebrew, npm, and Cargo installs.
- **README setup docs are current again.** The English, Simplified Chinese,
  and Japanese READMEs now use the current Docker volume/workspace invocation,
  document update paths, list the current provider/model switching surface, and
  send release-specific feature notes back to the changelog.

### Fixed

- **OpenAI-compatible providers receive stricter request bodies.** Fireworks
  requests now use `reasoning_effort` without the DeepSeek/Anthropic-style
  top-level `thinking` field, and chat tool schemas no longer include
  Anthropic-only metadata such as `allowed_callers`, `defer_loading`, or
  `input_examples` (#1592).
- **pnpm global installs no longer hang in optional postinstall.** pnpm
  postinstall now skips install-time binary downloads and leaves the existing
  runtime downloader to verify or fetch binaries on first run (#1637).
- **Terminal modes are restored on early TUI exits.** A cleanup guard now
  restores raw mode, alternate screen, focus events, mouse capture, bracketed
  paste, keyboard flags, and cursor visibility if startup returns early after
  terminal initialization (#1593, #1582).
- **Wrapped OSC 8 links keep their full target.** Long clickable URLs now
  reopen the original full link target on each wrapped visual chunk instead of
  exposing truncated hyperlink targets (#1577).
- **Provider-selected models survive startup and picker reselects.** The
  `/model` picker now uses live provider model catalogs when available, saved
  default providers sync into the runtime config before the first request, and
  reselecting the active provider from the picker keeps the current model
  instead of falling back to the provider default (#1632).
- **OpenAI-compatible batch tool calls keep all start events.** Streaming
  responses with multiple `tool_calls` in one assistant message now preserve
  every tool-use block instead of pairing many tool results with only the last
  tool start event (#1686).
- **Diagnostics tool schemas include an empty `required` list.** The built-in
  `diagnostics` tool now sends `required: []` with its empty object schema so
  DeepSeek no longer rejects it as a null required array (#1685).
- **Windows wheel-as-arrow scrolling works with mouse capture enabled.**
  `composer_arrows_scroll` now defaults on for Windows terminals even when
  mouse capture is enabled, so wheel events that arrive as arrow keys scroll the
  transcript instead of cycling composer history (#1578).
- **Plain Windows PowerShell / ConHost uses calmer rendering.** Unmarked
  legacy Windows console hosts now automatically enable low-motion rendering,
  disable fancy animations, and resolve `synchronized_output = "auto"` to off
  so streaming redraws do not overlap or visibly flicker (#1590).
- **LoopGuard blocks now count as failed tool calls.** Identical tool-call
  blocks now return a failed tool result instead of a success, so repeated
  blocked checklist/tool retries can trip the existing failure warning and halt
  path instead of spinning indefinitely (#1574).
- **Denied tool approvals are scoped to the exact call.** Denying one
  write/shell approval now caches the canonical argument fingerprint instead of
  a lossy tool/prefix key, so later calls to the same tool with different
  arguments can still be reviewed and approved (#1617).

### Thanks

Thanks to **DC ([@duanchao-lab](https://github.com/duanchao-lab))** for the
terminal cleanup-guard idea harvested from #1630, and **imkingjh999
([@imkingjh999](https://github.com/imkingjh999))** for the provider/model
switching fixes harvested from #1642. Thanks to **Photo
([@eng2007](https://github.com/eng2007))** for the provider-aware `/model`
picker catalog work harvested from #1201. Thanks to **hexin
([@h3c-hexin](https://github.com/h3c-hexin))** for the OpenAI batch tool-call
streaming fix in #1686. Thanks to **chennest
([@chennest](https://github.com/chennest))** for the diagnostics schema report
in #1685. Thanks to
**[@kunpeng-ai-lab](https://github.com/kunpeng-ai-lab)** for the Windows
composer scroll fix harvested from #1578, and **WuMing
([@asdfg314284230](https://github.com/asdfg314284230))** for the Windows
PowerShell flicker fix harvested from #1591. Thanks to
**[@maker316](https://github.com/maker316)** for the LoopGuard/checklist loop
report in #1574. Thanks to **lalala
([@lalala-233](https://github.com/lalala-233))** for the approval denial
regression report in #1617, and **Nightt
([@nightt5879](https://github.com/nightt5879))** for the exact-call approval
key work harvested from #1624.

## [0.8.37] - 2026-05-14

### Added

- **Tencent Lighthouse + Feishu/Lark bridge setup.** Added a `/opt/whalebro`
  Lighthouse runbook, systemd deploy assets, a long-connection Feishu/Lark
  bridge, a bridge config validator, and a VPS doctor for runtime, Node,
  binaries, env, systemd, and localhost health checks.
- **Tencent Cloud remote-first onboarding.** Documented the CNB + Lighthouse +
  Feishu/Lark + optional EdgeOne teaching path and added non-active CNB deploy
  templates for a future Lighthouse deploy button. Feishu/Lighthouse branches
  are now mirrored to CNB for Tencent-first bootstrap.
- **Homebrew tap automation is release-gated.** The release workflow can update
  `Hmbown/homebrew-deepseek-tui` from the checksum manifest when a tap token is
  configured, and skips cleanly before downloading release assets when no tap
  token exists.

### Changed

- **Bing is the default `web_search` backend.** DuckDuckGo remains selectable
  with `[search] provider = "duckduckgo"` and keeps its Bing fallback path.

### Fixed

- **First-run onboarding stays usable without an API key.** Missing-key startup
  no longer aborts the TUI before onboarding can collect provider settings.
- **Streamable HTTP MCP sessions keep their server-issued session ID.** Custom
  headers also apply to GET preflight requests, fixing authenticated MCP
  servers that require both.
- **DeepSeek model completions use canonical IDs.** Alias completions now
  resolve to stable DeepSeek model names before being written to config.
- **Terminal and child-process reliability is tighter.** Signal shutdown now
  restores the terminal, child tasks preserve proxy environment variables, and
  Windows Enter / CSI-u input handling avoids the prior event mismatch.
- **Long terminal text wraps instead of overflowing.** Streaming output, diff
  rendering, and the pager now hard-wrap overlong no-whitespace and CJK runs.
- **Release and platform edges are safer.** The TUI no longer trips the Windows
  Instant-underflow test path, unsupported desktop targets compile the external
  URL opener, and legacy DeepSeek CN provider aliases deserialize to the
  canonical DeepSeek provider.
- **Footer diagnostics are less cryptic.** Prefix-cache stability is no longer
  shown in the default footer, and the opt-in `/statusline` chip now says
  `cache prefix 100%` instead of the ambiguous `P 100%`.
- **Feishu/Lark bridge dependency installs are locked and audited.** The
  bridge now ships a package lock, installs with `npm ci` on Lighthouse when
  available, and overrides the Lark SDK's transitive `axios` dependency to a
  patched line.
- **China-friendly update fallback.** `deepseek update` now supports mirrored
  release assets through `DEEPSEEK_TUI_RELEASE_BASE_URL` plus
  `DEEPSEEK_TUI_VERSION`, and its network-failure hints point users behind
  GitHub-blocking networks to the CNB `cargo install --git` path for both
  shipped binaries.
- **CNB is the default Tencent release-candidate mirror.** The CNB sync
  workflow now mirrors Feishu/Lighthouse release branches, so Tencent
  Lighthouse bootstrap can use CNB before the release branch merges.

### Thanks

Thanks to **ZzzPL ([@Oliver-ZPLiu](https://github.com/Oliver-ZPLiu))** for
the MCP Streamable HTTP and Homebrew automation fixes (#1643, #1631),
**Reid ([@reidliu41](https://github.com/reidliu41))** for CI, streaming wrap,
and model-completion fixes (#1603, #1628, #1601), **MidoriKurage
([@mdrkrg](https://github.com/mdrkrg))** for the onboarding crash fix (#1598),
**Gordon ([@gordonlu](https://github.com/gordonlu))** for the Windows Enter /
CSI-u fix (#1612), **Aitensa ([@Aitensa](https://github.com/Aitensa))** for
the CJK diff/pager wrap fix (#1622), **qiyan233
([@qiyan233](https://github.com/qiyan233))** for legacy DeepSeek CN provider
aliases (#1645), **jieshu666 ([@jieshu666](https://github.com/jieshu666))**
for the repaint-flicker reduction (#1563), **Vishnu
([@Vishnu1837](https://github.com/Vishnu1837))** for terminal restoration on
signals (#1586), and **axobase001
([@axobase001](https://github.com/axobase001))** for proxy environment
preservation in child tasks (#1608).

## [0.8.36] - 2026-05-14

### Added

- **The right sidebar can be hidden for copy-friendly terminals.**
  `sidebar_focus = "hidden"` (or `Ctrl+Alt+0` for the current session) removes
  the Work/Tasks/Agents/Context rail so raw terminal selection cannot copy
  sidebar borders alongside transcript text.

### Changed

- **Sub-agent completion handoffs are leaner and more cache-friendly.**
  Internal `<deepseek:subagent.done>` sentinels now point to the preceding
  human summary line instead of duplicating the summary, elapsed time, and
  step count inside JSON sent to the parent model.
- **Prefix stability is visible beside cache telemetry by default.** The
  footer now includes the prefix-stability chip in the default status layout,
  and low last-request cache hit rates are no longer colored as hard errors
  when the system/tool prefix itself is stable.
- **RLM batch helpers now require an explicit independence assertion.**
  `sub_query_batch`, `sub_query_map`, and low-level `*_batched` helpers refuse
  dependency-unsafe parallel fanout unless callers pass
  `dependency_mode="independent"`, and RLM now exposes `sub_query_sequence`
  for A-to-B dependent work.

## [0.8.35] - 2026-05-13

A post-0.8.34 cleanup release focused on prompt hygiene, context-pressure
guidance, and keeping the next release branch clearly separated from the
already-published v0.8.34 tag.

> **Note on v0.8.34 contributor credits:** Horace Liu ([@liuhq](https://github.com/liuhq))
> contributed Nix package support and install documentation in the v0.8.34
> cycle but was inadvertently omitted from that release's changelog. The
> README contributor list and this note correct the record.

### Changed

- **First-turn prompt context is leaner and easier to audit.** The
  generated project context pack now ignores hidden tool/cache state,
  balances top-level directories before descending, and `/context`
  shows named prompt layers instead of a single opaque system blob.
- **Model-visible prompt policy de-conflicted.** The base and mode
  prompts no longer forbid useful `deepseek` CLI diagnostics, no
  longer require checklists for simple one-step work, and align
  long-session compaction guidance around the 60% suggestion threshold.
- **Context-pressure guidance now has one split rule.** Manual
  `/compact` suggestions start around 60% during sustained work, while
  automatic replacement compaction remains an opt-in hard guardrail near
  80% so DeepSeek V4 prefix-cache economics stay intact.
- **The Tasks sidebar now ages out stale live-tool noise.** Completed
  active tool rows linger briefly and then leave the right rail; very old
  running shell rows collapse to a single row instead of occupying the
  whole Tasks panel.

### Fixed

- **`auto_compact` settings help now reports the real default**, which
  has been off since v0.8.11 to avoid unnecessary cache-prefix rewrites.

## [0.8.34] - 2026-05-13

A polish, terminal-protocol, and internal-cleanup release. The model-facing
surface is stable; this cycle focused on prefix-cache stability metrics,
broader terminal protocol coverage, bundled skills, and shrinking the
mega-files that had grown around the agent loop and TUI.

### Added

- **Prefix-cache stability tracking.** A footer chip surfaces how stable
  the cached prefix has been across recent turns (inspired by Reasonix),
  so users can spot cache-busting edits before cost climbs.
- **Bundled DeepSeek-native workflow skills.** A starter set of skills
  ships in-binary so a fresh install has a usable `/skills` catalog
  without external assets.
- **Native Kitty + Ghostty notification protocols.** `OSC 99` (Kitty)
  and `OSC 777` (Ghostty) are now first-class alongside the existing
  desktop notification fallback.
- **Theme picker with more presets.** Catppuccin, Tokyo Night, Dracula,
  and Gruvbox join the built-in palette set; `/theme` now shows a
  live picker.
- **Chunked parallel-safe tool execution.** The engine batches
  side-effect-free tool calls into a chunked dispatch so independent
  reads/searches finish in one turn instead of serialising round-trip
  by round-trip.
- **Cancel-all shell jobs.** A single action stops every running
  background shell command instead of cancelling them one-by-one.
- **`edit_file` tolerates typographic punctuation drift.** When the
  exact-match and leading-whitespace-fuzzy passes both fail and
  `fuzz: true` is set, the tool retries with smart quotes (`"`/`"` →
  `"`, `'`/`'` → `'`), en/em-dashes (`–`/`—` → `-`), and non-breaking
  spaces (U+00A0 → space) normalized to ASCII. Catches the copy-paste
  failure mode where a browser or chat client substituted Unicode
  punctuation for the ASCII the file actually contains.

### Changed

- **`crates/tui/src/tui/ui.rs` split into focused modules.** The
  former 10k-line single-file TUI dispatcher is decomposed into smaller
  modules with clearer responsibilities so reviewing a UI change does
  not require holding the entire surface in head.
- **`crates/tui/src/core/engine.rs` reduced.** Helper clusters moved
  into the existing `core/engine/` submodule directory next to the
  turn loop and tool execution code, making the agent-loop core
  easier to read end-to-end.
- **Structured tracing on tool dispatch.** Tool entry, exit, duration,
  and result/error are emitted through `tracing` events so
  `RUST_LOG=engine.tool_execution=debug` produces a coherent timeline
  instead of scattered ad-hoc prints.
- **`/init` updates `AGENTS.md` in place** instead of refusing when
  the file already exists, so adding new project guidance does not
  require manual stitching.
- **Reasoning tokens included in cost calculations**, and the cost
  display auto-switches to CNY when the session locale is `zh-Hans`.
- **Stale repo-root development docs removed.** `TAKEOVER_PROMPT.md`
  (v0.8.6 era), `PROMPT_ANALYSIS.md`, and the redundant
  `DEPENDENCY_GRAPH.md` no longer ship in releases; `docs/ARCHITECTURE.md`
  remains the canonical crate-layout reference.

### Fixed

- **Auth keys checked against the saved provider on startup**, so a
  stored DeepSeek key is no longer rejected after switching providers
  mid-session.
- **Auto router skipped for decisive local routes**, removing an
  extra model round-trip on prompts the dispatcher can route directly.
- **Reasoning content stripped for generic providers** that do not
  understand the `reasoning_content` field, preventing HTTP 400s when
  pointing at an OpenAI-compatible gateway that lacks DeepSeek
  thinking semantics.
- **`FocusGained` debounced** so terminals (Tabby) that emit rapid
  focus events no longer trigger a repaint flicker loop.
- **MCP HTTP transport defaults `Accept: application/json,
  text/event-stream`** and persists `Mcp-Session-Id` across requests,
  matching the spec for resumable streams.
- **Shell output tail preserved when truncating**, so the last lines
  of a long command output (usually the error trailer) survive the
  in-transcript summary.
- **Prefix cache preserved while pruning tool results.** Old
  side-effect tool payloads no longer invalidate the prefix that
  the next turn would otherwise reuse.
- **Review sub-agents prevented from spawning further sub-agents**
  (#1489), keeping recursive depth bounded.
- **Help overlay closes cleanly** and repaints without a stale frame.
- **Pinyin `/skills` alias dispatched correctly** so Chinese-locale
  users reach the same surface.
- **VTE flicker terminals get reduced motion** by default to avoid
  thrashing on terminals that mishandle frequent partial redraws.
- **Composer border no longer shows the derived session title**, keeping
  the composer chrome reserved for editor and mode state.

## [0.8.33] - 2026-05-12

A sub-agent and RLM renovation release. The model-facing delegation
surface is now session-oriented instead of one-shot: RLM work happens
through `rlm_open` / `rlm_eval` / `rlm_configure` / `rlm_close`,
sub-agent work happens through `agent_open` / `agent_eval` /
`agent_close`, and large outputs can be parked behind typed handles
that the model reads back explicitly with `handle_read`.

### Added

- **Persistent RLM sessions with bounded REPL helpers.** RLM prompts now
  use `peek`, `search`, `chunk`, `context_meta`, `sub_query`,
  `sub_query_batch`, `sub_query_map`, `sub_rlm`, and
  `finalize(value, confidence)` instead of exposing the full parent
  context as an ambient variable.
- **Fork-aware sub-agent sessions.** `agent_open` supports named
  sessions, `fork_context`, and bounded recursive depth so the parent can
  ask for multiple perspectives while preserving prompt-cache-friendly
  prefix context where available.
- **Shared `handle_read` storage.** RLM finals, sub-agent transcripts,
  and other large structured results can return `var_handle` references
  with slice, range, count, and JSONPath projections.
- **Slash-command routing for the new surface.** `/rlm [N] ...` and
  `/agent [N] ...` now prompt the assistant to use the persistent tools
  instead of the removed foreground RLM operation.
- **Harness-friendly non-interactive exec sessions.** `deepseek exec`
  now supports `--resume`, `--session-id`, `--continue`, and
  `--output-format stream-json` so backend wrappers such as ClawBench can
  keep conversation state and parse one JSON event per line without running
  a long-lived server.
- **`/relay` slash command with CJK aliases** (`/接力`). Hands the
  assistant a structured handoff prompt for coordinated multi-turn
  continuation across sessions.
- **`checklist_write` sidebar rename.** The sidebar focus tab formerly
  known as "Plan" / "Todos" is now "Work" — one panel for the active
  checklist and optional plan, consistent across all three modes.
- **Grayscale theme.** `/theme grayscale` and
  `/set theme grayscale --save` provide a low-opinion black/white palette
  for users who want less brand color in the terminal.

### Changed

- **Prompts and docs now teach only the new tool names.** Legacy
  RLM/sub-agent helpers remain internally where needed for durable
  transcript compatibility, but the registry exposes the session tools.
- **Large or noisy tool results are easier to keep out of context.**
  Tool output summaries, sub-agent results, and transcript snapshots now
  point the model toward `handle_read` when it needs raw detail.
- **Tool-surface smoke guidance is explicit.** Release checks now document
  the exact version commands and registry-name searches for `handle_read`,
  persistent RLM tools, and persistent sub-agent tools.
- **README acknowledgements expanded.** The project thanks OpenWarp and
  Open Design for support and collaboration around terminal-agent and
  design-forward workflows.
- **Light theme tuned for calmer contrast.** The canvas, panel, elevated,
  border, and selection tokens now separate surfaces without the washed-out
  white-on-white feel.
- **Session picker is history-first.** `/sessions` and `Ctrl+R` now show
  the full selected session history on the left with the session list on
  the right; number keys `1`-`9` open visible session histories, `PgUp` /
  `PgDn` scroll that history, and `Enter` still resumes.
- **Foreground RLM operation removed.** The old `Op::Rlm` path and its
  `handle_rlm` engine method are gone; all RLM work now flows through
  the persistent-session tools.
- **Stale competitive-analysis doc removed.** The old cross-agent matrix
  had become an unreliable inventory of tool names rather than useful
  release guidance.

### Fixed

- **Local/custom endpoints stay prompt-free when auth is optional.**
  The dispatcher no longer reads the secret store for SGLang, vLLM,
  Ollama, or loopback custom URLs unless API-key auth is explicitly
  requested, and the direct TUI treats loopback model endpoints as
  no-key by default. This avoids macOS Keychain prompts and stale
  DeepSeek keys when users point the app at local OpenAI-compatible
  servers.
- **Transcript browsing stays put across resizes.** If the user is reading
  older chat history, terminal resize events preserve the current transcript
  position instead of jumping back to the live tail; the scrollbar and
  jump-to-latest affordance now follow the active theme.
- **Backtrack preview opens near the selected turn.** Pressing Esc twice no
  longer opens the live transcript preview at the oldest conversation line;
  the highlighted recent user turn is pinned into view, and changing the
  backtrack target re-pins only that selection.
- **Completed thinking no longer masquerades as prompt text.** Collapsed
  completed reasoning now shows only explicit `Summary:` content inline; raw
  reasoning remains available through Ctrl+O/transcript instead of appearing
  as assistant self-talk in the main flow. When Ctrl+O starts from a reasoning
  block, it opens a full-session reasoning timeline instead of a single
  isolated chunk.
- **Transcript selection keeps working while the agent is streaming.**
  The loading-state mouse filter now drops inert move events but allows
  active transcript and scrollbar drags to continue (reported as a known
  issue in v0.8.32).
- **Empty-composer arrow scrolling feels less twitchy.** When configured to
  scroll the transcript, plain Up/Down now move by a small wheel-like step
  instead of a single-line flick.
- **Mouse and trackpad scrolling feel less sticky in long logs.** Rapid
  same-direction transcript scrolls now get bounded acceleration while
  direction changes reset to precise single-line movement.
- **RLM smoke-test papercuts fixed.** `rlm_eval` now binds `content` as a
  convenience alias for `_context`, tolerates common `timeout_secs` keyword
  guesses on child-query helpers while preserving session-level timeout
  policy, and stores JSON-serializable `finalize(...)` values as JSON handles
  so `handle_read` can project them directly.
- **RLM REPL uses the shared Python resolver.** RLM startup now tries
  `python3`, `python`, and `py -3`, matching the dependency resolver used by
  code execution and avoiding Windows failures where `python3` is absent
  (harvested from PR #1540).
- **Session titles and history previews hide metadata noise.** Saved
  session titles and the picker history strip leading `<turn_meta>` envelopes
  and thinking-tag blocks so historical conversations read like user-visible
  chat rather than prompt plumbing (harvested from PR #1510).
- **Companion binary version smoke is unambiguous.** `deepseek-tui --version`
  now reports the `deepseek-tui` binary name instead of the dispatcher label.
- **Vision path boundary test is platform-native.** The absolute-path
  rejection smoke uses a Windows absolute path on Windows and `/etc/hosts`
  elsewhere (harvested from PR #1526).
- **Tool papercuts:** `file_search` has safer default excludes and an
  explicit `exclude` option; `grep_files` returns single-line context as
  strings; `fetch_url` can project JSON fields and returns headers;
  `edit_file` can opt into leading-indentation fuzz; `exec_shell` can
  merge stdout/stderr in chronological order; `revert_turn` rejects
  no-op snapshot boundaries.
- **CLI reasoning-effort honoured on non-auto exec routes** (PR #1511
  from **@h3c-hexin**). `deepseek -p "..." --reasoning-effort high` now
  applies the flag correctly instead of falling back to the config-file
  default.
- **Edit-file replacement boundaries clarified** (PR #1516). The tool
  description and error messages now make it unambiguous that
  `edit_file` is for one clear replacement in one file.
- **Pandoc output validated before probing** (PR #1523). Binary-format
  conversions that produce empty or invalid output now surface a clear
  error instead of a confusing pandoc stack trace.
- **Running turns can be steered and repainted** (PR #1533, #1537).
  Composer input during an active turn no longer stalls; the TUI
  redraws the transcript as the agent streams.
- **Tasks and Activity Detail are calmer under load.** The Tasks panel now
  keeps live/background/recent activity from double-counting the same shell
  or RLM work, groups repeated read/search/checklist noise, and keeps
  failures, status, command summaries, and durations visible. Ctrl+O now
  opens Activity Detail for selected/live/recent tool work and the reasoning
  timeline for thinking blocks, while Alt+V remains the direct tool-detail
  pager; the idle footer now advertises that split for the visible activity.
- **npm retry shows timeout hint on first failure** (PR #1538).
  Installations behind slow proxies now see a clear "retrying" message
  instead of a silent hang.
- **Issue templates improved** (PR #1525 from **@reidliu41**). Bug and
  feature-request templates are clearer and easier for new contributors.

### Credits

Thanks to **@reidliu41** (#1525/#1526), **@h3c-hexin** (#1511),
**@xulongzhe** (#1530/#1544), **@tyouter** (#1510), and
**@Duducoco** (#1540) for community contributions in this release.

## [0.8.32] - 2026-05-12

A "more useful tools" release. v0.8.31 made the tool surface
reliable on every host; v0.8.32 expands it. Anchor is the question
every new contributor asks: "what does the model actually have to
work with?" — and the answer is now closer to "everything you'd
reach for from a shell, including the document formats the real
world uses." Five new tools (`pdf-extract` swap, `js_execution`,
`pandoc_convert`, `image_ocr`, `image_analyze`), ten community PR
harvests targeting model-protocol bugs (vLLM thinking) and UX
papercuts (Shift+Enter on Windows VSCode, mention truncation
splitting CJK codepoints, approval modal hiding the transcript),
and a snapshot-self-disable on workspaces over 2 GB of
non-excluded content so first-turn `git add -A` no longer hangs
the TUI on multi-hundred-GB project directories.

### Performance

- **Move `instructions = [...]`, user memory, and session goal
  below the prompt's volatile-content boundary so DeepSeek's KV
  prefix cache survives mid-session edits** (harvested from PR
  #1345 by **@Duducoco**). Before this change, the per-workspace
  `instructions` block, the user memory file (`/memory`), and the
  current session goal (`/goal`) were rendered at position 2.5
  in the system prompt — inside the static prefix layer that the
  cache hits. Any edit to those files (or any `# foo`
  quick-add to memory) busted the cached prefix from that byte
  onwards, forcing the next turn to re-tokenize the rest of the
  static layer. Relocating them to position 6 (immediately above
  the previous-session handoff block) means the cache hit covers
  the entire static prefix — mode, project context, env, skills,
  context management, compact template — regardless of how often
  the user edits their memory file. Skills, context management,
  and the compact template stay always-cacheable in the static
  layer where they belong.

### Removed

- **Shift-to-bypass-mouse-capture is gone.** The #376 escape-hatch
  feature (hold Shift while moving the mouse → temporarily disable
  alt-screen mouse capture so terminal-native text selection works,
  then re-enable on release) was causing visible scroll/redraw
  thrash: every Shift transition flipped the mouse-capture mode AND
  pushed a status toast ("Native selection — release Shift to
  return" / "Mouse capture restored"). On modern terminals that
  honor `xterm-modifyOtherKeys` the toast cycle fired on stray
  Shift events and produced what users described as a "scroll
  demon." Removing the bypass path entirely: text selection in
  alt-screen sessions now goes through the same path as any other
  TUI (your terminal's modifier-bypass — typically Option/Alt on
  macOS, Shift in some Linux terminals — still works at the
  terminal level, this just stops us from second-guessing it).

### Fixed

- **Tool-result spillover and wire-dedup now share a retrieval
  namespace, so `retrieve_tool_result` finds what the model was
  pointed at.** Two systems used to mint reference blocks
  independently — disk spillover keyed by tool-call id
  (`~/.deepseek/tool_outputs/<id>.txt`, only above 100 KB) and the
  Chat-Completions wire compactor that replaced repeated tool
  results with `<TOOL_RESULT_REF sha="…"/>` (any size, keyed by
  SHA256 of the content). The SHA refs were impossible to
  retrieve: `retrieve_tool_result ref=<sha>` looked in the
  tool-call-id directory and 404'd. Worse, the wire dedup fired
  on tiny outputs (a 65-byte `gh run view --json` could turn into
  a ref the model then chased through three guesses), and the
  `[artifact: …]` block emitted by `apply_spillover_with_artifact`
  showed `id:` (with `art_` prefix), `path:` (with a slash), and
  `tool_call_id:` separately with no indication of which one
  `retrieve_tool_result` accepted. Reported by users hitting
  "spilled tool result was not found" 4–5 times per session while
  polling CI runs.
  - The wire compactor now persists deduped content to
    `~/.deepseek/tool_outputs/sha_<sha>.txt` on first sighting,
    and only dedupes outputs ≥ 1 KiB — tiny results stay inline
    on both occurrences instead of becoming a ref the model has
    to chase. The `<TOOL_RESULT_REF>` block grew a
    literal `retrieve: retrieve_tool_result ref=sha:<sha>` line.
  - `retrieve_tool_result` learns five new ref shapes:
    `sha:<64-hex>`, bare 64-hex, `art_<tool_call_id>`,
    `artifacts/art_<id>.txt`, and absolute paths anywhere under
    the session-artifact root. The lookup tries the legacy
    spillover dir, the current session's artifact dir, and the
    `art_` → legacy fallback in one call. When everything misses,
    the error enumerates every candidate path tried and lists
    every accepted ref form so the model can correct on the next
    attempt instead of guessing blind.
  - The `[artifact: …]` block emitted alongside spilled outputs
    now includes a literal `retrieve:     retrieve_tool_result
    ref=art_<id>` line so the model never has to guess between
    `id:`, `path:`, and `tool_call_id:`.
- **`<TURN_META_REF sha="…" original_chars="…" />` is gone.** The
  legacy wire-level dedup for identical per-turn metadata blocks
  emitted an opaque SHA-tagged reference that periodically leaked
  into model reasoning and user-visible debug dumps with no
  retrieval mechanism — the SHA was an artifact of the cache
  optimization, not a content address the model could resolve.
  The compactor now emits a self-explanatory
  `<turn_meta_unchanged />` marker instead. Same KV-cache
  friendliness (bytes in the same prompt position when nothing
  changed), zero ambiguity if the marker ever surfaces.
- **"Request cancelled while awaiting approval" now says
  *why*.** The approval and user-input handlers used to emit the
  same opaque string regardless of source — user Esc, runtime-API
  DELETE, parent-agent cancel, or a torn-down channel all
  produced an identical error. A new `CancelReason` enum is
  latched alongside the `CancellationToken` and surfaced as a
  `(reason: …)` suffix: `user cancelled the request`, `request
  cancelled by external caller`, `request was preempted by a new
  turn`, `engine torn down before approval resolved`. A closed
  approval channel reports the teardown race explicitly instead
  of just "Approval channel closed." Remaining non-user
  cancellation call sites are tracked in #1541.
- **`pandoc_convert` validates binary-output requests before
  resolving the `pandoc` binary** (harvested from PR #1523 by
  **@muyuliyan**). Hosts without pandoc installed now still get
  the intended validation error for `target_format = "docx"`
  without `output_path`, which keeps the CI test independent of
  runner packages.
- **Mouse movement no longer leaks xterm tracking bytes into the
  composer while the model is streaming** (harvested from PR #1533
  by **@Oliver-ZPLiu**, fixes #1529). When mouse capture is
  enabled, move/drag events can arrive faster than the TUI needs
  them during a streaming turn; those stale movement events are now
  discarded while loading, while click/scroll handling remains
  active when the TUI is idle.
- **Resize and successful turn completion avoid blank-frame
  flicker** (harvested from PR #1537 by **@czf0718**, addresses
  #1515 and #1539). Resize now performs the viewport reset, clear,
  and redraw inside a single synchronized-output batch. Successful
  `TurnComplete` events use the incremental renderer instead of
  forcing a clear+redraw every time; full repaint remains reserved
  for interrupted/failed turns and periodic resyncs.
- **The npm wrapper shows binary-download remediation on the first
  retryable timeout** (harvested from PR #1538 by **@jieshu666**,
  addresses #1532). Users behind networks that time out when
  fetching GitHub Release assets now see the mirror/proxy/Cargo
  alternatives immediately, and the installer points at the fixed
  `docs/INSTALL.md#npm-binary-download-times-out` anchor.
- **`edit_file` states its exact-replacement boundary and warns on
  multi-match replacements** (from #1516). The tool description and
  base prompt now steer structural or intertwined edits toward
  `apply_patch` / `write_file`, while keeping legitimate repeated
  replacements compatible with an advisory "verify with read_file"
  hint.
- **Shift+Enter explicitly steers a running turn.** Plain Enter
  while busy continues to queue (the agreed default since
  #1331). Pressing **Shift+Enter** during an in-flight turn
  routes the draft directly through `engine.steer()` — the same
  path Ctrl+Enter already drove — without going through the
  queue at all. When the agent is idle, Shift+Enter still inserts
  a composer newline as before. Ctrl+Enter remains bound for
  terminals that swallow the Shift modifier on Enter at the
  protocol level. (Pattern cross-checked against pi-agent's
  `streamingBehavior: "steer"` API: see `agent-session.ts:156` —
  same `Shift+Enter → steer / Plain Enter → queue` split, just
  surfaced as a keypress instead of a programmatic flag.)
- **`exec_shell_wait` default timeout raised from 5 s → 30 s.**
  The 5 s default was fine for "did the command exit yet?" polls
  but forced models into hand-rolled `while true; do
  exec_shell_wait …; sleep 30; done` loops for `gh run watch`,
  long `cargo build`s, and `cargo login`-style interactives. 30 s
  covers the common case in a single tool call without burning a
  whole turn on the timeout boundary; the model can still pass
  any value up to 600 s.
- **Snapshots no longer try to index a multi-hundred-GB workspace
  on first turn.** Reported by users running `deepseek-tui` inside
  project directories with hundreds of GB of content — datasets,
  model weights (`.safetensors`, `.gguf`, `.pt`), Docker image
  dumps, parquet / arrow caches — where the side-git snapshot
  initialization would hang the TUI for minutes or hours while
  `git add -A` walked the workspace. v0.8.32 adds a default
  2 GB ceiling on non-excluded workspace content (measured before
  any git work, walking the same excludes the snapshot path
  already honors). When the cap is exceeded the side repo isn't
  initialized; subsequent snapshots are skipped with a clear
  WARN-level log line referencing the new
  `[snapshots] max_workspace_gb` config knob users can raise (or
  set to `0` to disable the cap entirely and restore v0.8.31
  behaviour). The bounded estimator also early-exits past 200k
  file entries, so a workspace full of tiny files trips the cap
  before paying for a full walk. Pre-existing v0.8.27 fixes for
  the growth-over-time angle (#1112: retention cap, mid-session
  prune, expanded built-in excludes) continue to apply; this
  closes the orthogonal "snapshots-too-big-to-start" path.
- **Toast stack overlay no longer renders on top of the composer
  input** (harvested from PR #1485 by **@MeAiRobot**). When a
  deferred tool's schema auto-loaded after the model requested
  it, the resulting status toast ("Auto-loaded deferred tool
  'edit_file' after model request.") could render at
  `footer_area.y - 1` — which on tight layouts is the bottom row
  of the composer area, visibly overwriting the start of the
  user's typed text. `render_toast_stack_overlay` now clamps
  `max_above` to the gap between `composer_area.y +
  composer_area.height` and `footer_area.y`, so when the composer
  and footer are adjacent the overlay collapses to zero rows and
  the toast is suppressed rather than drawn on top.
- **`/sessions` picker highlights the selected row more strongly
  in dark terminals** (harvested from PR #1493 by **@reidliu41**).
  Previously the selection background was subtle enough to lose
  in low-contrast dark themes; keyboard navigation up/down didn't
  obviously change which row was active. The selected row now
  uses a bolded label on a stronger background so the focused row
  reads cleanly across the dark palettes the TUI ships with.
- **TUI input no longer freezes while long-running shell jobs
  flood stdout** (#1299, harvested from PR #1494 by
  **@CrepuscularIRIS / autoghclaw**). The job-panel refresh path
  was calling `full_output()` from inside the `ShellManager`
  mutex, which cloned the entire accumulated stdout/stderr buffer
  every 2.5 seconds. For browser-automation or large-build jobs
  the buffer grew unboundedly; cloning held the mutex for
  O(total_bytes) time, starving the `crossterm::event::poll` loop
  and dropping keystrokes. The refresh now reads only the last
  `max_tail_chars * 4` bytes under the lock (lock hold time is
  O(1) regardless of total output volume) and decodes those into
  a tail string for display. `stdout_len` / `stderr_len` still
  report the true total byte counts so no caller invariant
  breaks. Also tightens `take_delta_from_buffer` to slice
  `[cursor..total]` inside the lock guard instead of cloning the
  whole buffer first, and skips UTF-8 continuation bytes at
  `tail_start` so `from_utf8_lossy` never emits a leading U+FFFD
  in the job panel.
- **`@`-mention truncation no longer splits multi-byte UTF-8
  sequences** (#1441, harvested from PR #1495 by
  **@CrepuscularIRIS / autoghclaw**). When `@`-mentioning a file
  larger than 128 KB the composer truncated the buffer at exactly
  `MAX_MENTION_FILE_BYTES`, which on CJK / emoji content landed
  mid-codepoint and produced a stray U+FFFD at the cut point. The
  truncator now uses `str::from_utf8(...).error_len()` to detect
  the incomplete-tail case and rounds down to the last valid
  codepoint boundary before decoding. Genuinely invalid UTF-8
  files still surface the "file is not UTF-8" error (the rounding
  is only applied when the error is an incomplete tail, not a
  real decoding failure mid-buffer).
- **vLLM provider: `reasoning_effort = "off"` now actually
  disables thinking on Qwen3 / DeepSeek-R1 servers, cutting
  TTFT from ~13s to ~270ms** (harvested from PR #1480 by
  **@h3c-hexin**). The vLLM branch of `apply_reasoning_effort`
  was injecting `thinking: {type: "disabled"}` at the top of
  the request body — but vLLM speaks OpenAI's chat-completions
  protocol, not Anthropic-native fields, and silently ignored
  the directive. The model then emitted a full hidden reasoning
  trace into the non-standard `reasoning` field (which this
  client doesn't surface), so users saw a multi-second freeze
  before any content token arrived. The vLLM branch now emits
  the OpenAI extension `chat_template_kwargs.enable_thinking`
  (which vLLM forwards into the model's chat template — the
  canonical way to toggle Qwen3's `<think>...</think>` mode).
  Measurement against vLLM + Qwen3.6-35B-A3B-FP8: TTFT
  13039ms → 274ms, total LLM call 13s → 5.7s. The `high` /
  `max` effort levels likewise switch to the OpenAI extension.
  No change for non-vLLM providers.
- **`/sessions` picker no longer shows `<turn_meta>` as the
  session title** (harvested from PR #1498 by **@wdw8276**).
  `session_manager::create_saved_session_with_id_and_mode`
  picked the first text content block off the user message via
  `find_map`; the engine prepends an internal `<turn_meta>` block
  ahead of the real user text, so the picker rendered that
  metadata blob as the session name. Guard added so titles fall
  through to the actual user input. Existing sessions without
  the prefix block are unaffected.
- **Kitty keyboard protocol now activates on Windows (VSCode +
  Windows Terminal), so `Shift+Enter` inserts a newline instead
  of submitting** (#1359, harvested from PR #1483 by
  **@CrepuscularIRIS / autoghclaw**). Root cause: crossterm's
  `PushKeyboardEnhancementFlags` gates the escape sequence on
  `is_ansi_code_supported()`, which on Windows queries the
  console mode rather than the VT capability and unconditionally
  returns false — so the Kitty push (`\x1b[>1u`) was never
  written, leaving xterm.js in legacy mode where `Shift+Enter`
  and `Enter` both produce `\r` and are indistinguishable.
  `Alt+Enter` / `Ctrl+J` were affected the same way. The fix
  writes the push and pop escapes directly under `#[cfg(windows)]`,
  bypassing the capability gate; terminals that don't speak the
  protocol silently discard the sequences. Also extends the
  pop-on-exit path to two missed call sites (the `main.rs` panic
  hook and `external_editor.rs::spawn_editor_for_input`) so a
  crash or `$EDITOR` invocation can no longer leave the parent
  shell's keyboard state corrupted.
- **Approval modal can be collapsed to a one-line banner with
  Tab** (harvested from PR #1455 by **@tiger-dog**). Previously the
  approval prompt rendered as a full-screen takeover that hid the
  transcript behind it, so users had to dismiss the modal just to
  remember which tool call they were being asked to approve. Tab
  now toggles between the takeover card and a single-line bottom
  banner — the rest of the conversation stays visible while the
  decision is pending. Tab again restores the full card; the
  selection state is preserved across the toggle.
- **Markdown renderer no longer eats underscores inside
  identifiers** (harvested from PR #1455 by **@tiger-dog**). The
  inline parser was matching `_italic_` against the underscore in
  `deepseek_tui` / `foo_bar_baz` and rendering the second half of
  the identifier in italic, which made transcript snippets that
  named code symbols read as garbled prose. Both `_italic_` and
  `*italic*` now apply a CommonMark-style boundary check on the
  closing delimiter — when the next character is a letter, digit,
  or underscore, the delimiter is treated as literal text instead
  of markup. Regression-pinned with cases like `crate deepseek_tui
  handles approvals` and `look at *not_emphasised*tail`.

### Added

- **npm wrapper installs cleanly on OpenHarmony / HarmonyPC**
  (#1072, harvested from PR #1499 by **@CrepuscularIRIS /
  autoghclaw**). `os.platform()` returns `openharmony` on
  HarmonyPC and on OpenHarmony's Linux ABI-compatible userspace,
  but the npm wrapper's platform-asset matrix only covered
  `linux` / `darwin` / `win32`, so `npm i -g deepseek-tui` would
  abort with `Unsupported platform: openharmony` even though the
  Linux x64 / arm64 binaries run unchanged on that environment.
  Added a `PLATFORM_ALIASES` mapping that resolves `openharmony`
  to the `linux` asset family before lookup so install succeeds
  on those hosts. The error message for genuinely unsupported
  platforms still reports the raw platform name (`freebsd`,
  etc.) so OS-mismatch reports stay diagnostic.
- **Startup empty-state shows useful context instead of
  repeating the header** (harvested from PR #1444 by
  **@reidliu41**). The center of the welcome view used to repeat
  information already displayed in the header and footer. It now
  shows the build version, the active model with a `/model`
  hint, and the current working directory so first-time users
  have somewhere to look while they decide what to type.
- **Opt-in `v4-best-practices` bundled skill** (harvested from
  PR #1448 by **@SamhandsomeLee**). A single 50-line `SKILL.md`
  encoding three V4-specific workflow rules for multi-step
  thinking-mode tasks. Each rule maps to a concrete observable
  failure class. Discovered through the existing
  `crates/tui/assets/skills/...` mechanism alongside the
  `skill-creator` skill — not enabled by default; users opt in
  via the standard `/skills` UI.
- **`image_analyze` tool — vision-model image understanding**
  (harvested from PR #1467 by **@MMMarcinho**). Sends an image
  file to an OpenAI-compatible vision endpoint and returns the
  model's natural-language description. Complements `image_ocr`:
  use `image_ocr` for "what text is on this image", `image_analyze`
  for "what is this image about". **Opt-in only** — gated by both
  the `[features] vision_model = true` flag and a `[vision_model]`
  config block specifying `model` (and optionally `api_key` /
  `base_url`). Default configuration ships the feature flag at
  `false`, so no install sees vision API calls fire without an
  explicit two-step opt-in. **Billing**: each call hits the
  configured vision endpoint (OpenAI by default), so usage is
  billed by the third-party provider; calls are stateless (no
  conversation context attached). Workspace-boundary check: the
  tool rejects absolute paths and any `..` parent-dir traversal
  before any base64 encoding or API call. To disable later: set
  `[features] vision_model = false` (or omit `[vision_model]`).
  Supports PNG, JPEG, GIF, WebP, and BMP inputs.
- **`image_ocr` tool — extract text from images via local
  tesseract.** Lets the model OCR a screenshot, scanned receipt,
  whiteboard photo, or image-only PDF the user drops into the
  workspace, without bouncing through `exec_shell`. Spawns
  `tesseract <image> -` and returns the recognised text inline;
  no file is written. PNG / JPEG / TIFF inputs supported.
  Registration is gated on `dependencies::resolve_tesseract()`;
  when tesseract is missing the tool isn't advertised, so the
  model never tries to call an OCR engine the host can't run.
  `deepseek doctor` reports tesseract status alongside the other
  external-binary dependencies with platform-aware install hints
  (`brew install tesseract` / `apt install tesseract-ocr` /
  `winget install UB-Mannheim.TesseractOCR`). For non-default
  language packs or PSM modes, users can still drop into
  `exec_shell` with the full tesseract CLI surface.
- **`pandoc_convert` tool — convert documents between formats via
  the local pandoc binary.** Pandoc is the Swiss Army knife the
  real world uses for moving prose around — Markdown to HTML,
  HTML to Markdown, reST to anything, anything to DOCX / EPUB /
  LaTeX — and surfacing it as a model-callable tool unblocks
  "rewrite this report as ..." / "publish this changelog as ..."
  workflows that previously needed the user to drop into a
  terminal between turns. Curated target whitelist of 11 formats
  (markdown, gfm, commonmark, html, rst, latex, docx, odt, epub,
  plain, asciidoc) so the model can't ask for `pdf` (would need
  LaTeX) or typos like `markown`. Binary targets (docx, odt,
  epub) require an `output_path`; text targets can return the
  converted text inline. Approval routes through the WritesFiles
  / Suggest tier on every call. Registration is gated on
  `dependencies::resolve_pandoc()`; `deepseek doctor` surfaces
  the binary's status with platform-aware install hints.
- **`js_execution` tool — execute model-provided JavaScript via a
  local Node.js runtime.** Mirrors `code_execution` (Python) so
  the model has a single consistent surface for "run this snippet
  locally and tell me what it printed" across both interpreters.
  Same tempfile-spawn pattern, same 120-second timeout, same
  stdout/stderr/return_code result shape — so prompt-cache
  layouts that cover one tool also cover the other. Registration
  is gated on `crate::dependencies::resolve_node()`: when Node is
  missing the tool is simply not advertised, so the model never
  sees a runtime it can't actually use. `deepseek doctor` reports
  Node availability under "Tool Dependencies" with platform-aware
  install hints (`brew install node` / `apt install nodejs` /
  `winget install OpenJS.NodeJS`). Approval routes through the
  same Suggest tier as `code_execution`.
- **`/translate` opt-in: respond in the user's UI locale, with a
  post-hoc fallback for English that leaks through** (harvested
  from PR #1462 by **@YaYII**). Two-layer design: when the user
  enables translation via the `/translate` slash command, a
  `## Language Output Requirement` block is appended to the
  system prompt instructing the model to reply in the resolved
  session locale (Simplified Chinese, Traditional Chinese,
  Japanese, or Brazilian Portuguese — code identifiers and
  user-requested English code blocks are exempt). For replies
  that still surface English despite the directive, a heuristic
  in `tui::translation` (Latin-vs-CJK character ratio with
  weighting for CJK information density) detects the leak and
  invokes a focused per-message translation API call to render
  the localised version before display. Both layers are off by
  default and have no effect on installs that don't enable them.
  Trust-boundary scope: opt-in only, system prompt addition is
  conditional on the runtime flag, no model behaviour change for
  English-locale users.
- **AtlasCloud is now a first-class provider** (harvested from
  PR #1436 by **@lucaszhu-hue**). AtlasCloud hosts the V4 family
  (and other DeepSeek-compatible models) on its own endpoint at
  `https://api.atlascloud.ai/v1`, and several contributors had
  been running it through the OpenAI-compatible passthrough with
  manual `base_url` / model overrides. Selecting
  `provider = "atlascloud"` in `~/.deepseek/config.toml` (or via
  `DEEPSEEK_PROVIDER=atlascloud`) now wires up the documented
  defaults, a `[providers.atlascloud]` config block for per-user
  api_key / base_url / model / http_headers overrides, the
  `ATLASCLOUD_API_KEY` env var path, and the
  provider-picker / `/provider` slash command entries — same
  shape as the existing NVIDIA NIM / Fireworks / OpenAI provider
  rows. Default remains DeepSeek; nothing changes for installs
  that don't opt in.
- **`web_search` supports Tavily and Bocha as configurable
  backends** (harvested from PR #1294 by **@sandofree**). DuckDuckGo
  with Bing fallback remains the default — no API key required —
  but users in regions where those scrapers are unreliable can now
  set `[search] provider = "tavily" | "bocha"` plus
  `api_key = "..."` in `config.toml` (or via the
  `DEEPSEEK_SEARCH_PROVIDER` / `DEEPSEEK_SEARCH_API_KEY` env vars)
  to route every `web_search` call through the chosen API. Tavily
  is an AI-search API targeted at general use; Bocha is the
  mainland-China-friendly equivalent. Trust-boundary pins: an
  unset `api_key` on an opted-in provider surfaces a clear
  `ToolError` naming the missing key rather than silently falling
  through to a different provider, the network policy gate
  (`[network]`) is consulted for the provider host on every call,
  and the default path is unchanged so no install sees provider
  behaviour change unless they explicitly opt in.
- **`/change` slash command** displays the most recent
  CHANGELOG.md version section from inside the TUI, so users can
  see what they just upgraded into without leaving the chat
  (harvested from PR #1416 by **@zhuangbiaowei**). The command
  works against the bundled release-notes copy when no workspace
  CHANGELOG is available, and on non-English locales it requests
  a model-side translation of the section so localised users see
  the changelog in their UI language. Pure offline fallback when
  no API key is configured.

### Fixed

- **`deepseek update` now refreshes the companion TUI binary
  alongside the dispatcher** (harvested from PR #1492 by
  **@NorethSea**). Closes the documented two-binary footgun:
  `~/.cargo/bin/deepseek` would update to the latest dispatcher,
  but `~/.cargo/bin/deepseek-tui` would stay at the previously
  installed version, so users saw the dispatcher report a new
  release while the TUI runtime they actually interacted with
  reported the old version. Most painful for Volta-managed npm
  installs and any maintainer flow that calls `update` instead of
  re-running both `cargo install --path crates/{cli,tui}`. The
  updater now enumerates colocated binaries up front, downloads
  and verifies every release asset before replacing anything,
  then swaps the sibling first and the running dispatcher last so
  a partial network failure cannot leave the launcher updated
  while the TUI remains stale.

### Changed

- **`read_file` now extracts PDFs in pure Rust by default — no
  Poppler install required.** Before v0.8.32 the PDF path shelled
  out to `pdftotext` (Poppler), so first-time users on hosts without
  it saw `read_file` return a `binary_unavailable` sentinel and had
  to `brew install poppler` / `apt install poppler-utils` before
  the model could open a PDF. The bundled `pdf-extract` crate
  (which already powered URL-fetched PDFs in `web_run`) now drives
  the local `read_file` path too. The `pages` parameter still
  filters by 1-indexed inclusive page range; both the whole-file
  and per-page variants run with no system dependency. Users with
  column-heavy or complex-table PDFs (academic papers, financial
  filings) where `pdftotext -layout` still wins can opt into the
  external path with `prefer_external_pdftotext = true` in
  `~/.config/deepseek/settings.toml` — when set, the previous
  Poppler dispatch (and the `binary_unavailable` install hint when
  the binary is missing) returns. `deepseek doctor` now reports
  `pdftotext` as optional and explains how to opt in instead of
  framing it as a missing dependency.

### Known issues

- **Terminal-native text selection can still be blocked while the
  agent is thinking or streaming a response.** v0.8.32 removed the
  noisy Shift-to-bypass-mouse-capture path that caused visible
  scroll/redraw thrash, but the replacement selection path is not
  complete yet. v0.8.33 is planned to ship the text-selection fix
  alongside the sub-agent and RLM renovation.

## [0.8.31] - 2026-05-12

A "tools that actually work" release. `code_execution` no longer
fails on Windows hosts where `python3` isn't on `PATH` — we probe
for the interpreter at catalog-build time and only advertise the
tool when one resolves, so the model never sees a runtime it can't
actually use. The new `deepseek doctor` "Tool Dependencies" and
"Terminal Quirks" sections surface external-binary status and
active env-driven overrides so flicker / motion / missing-tool
puzzles answer themselves before a bug report gets filed. Ptyxis
50.x users on Ubuntu 26.04 get a manual `synchronized_output = off`
knob plus auto-detection that opts them out of the DEC 2026
synchronized-output wrap their VTE 0.84 mishandles. The CNB Cool
mirror workflow is rewritten with concurrency and scoped pushes so
release tags reliably reach `cnb.cool/deepseek-tui.com/DeepSeek-TUI`
for users behind GitHub-blocking networks. Plus a new auto-close
workflow that closes contributor PRs whose code has been harvested
into `main`, so credit lands at the same moment the fix does.

### Fixed

- **Windows `exec_shell` preserves MSVC toolchain env** (harvested
  from PR #1487 by **@Jianfengwu2024**). When the parent shell has
  already loaded `VsDevCmd` / `vcvars` (Developer Command Prompt,
  the standard way to run Rust + MSVC on Windows), `exec_shell` was
  stripping `LIB` / `LIBPATH` / `INCLUDE` and the related VS / SDK /
  CRT root variables on its way to the child. That made
  model-driven `cargo build` calls fail to resolve `kernel32.lib`
  even though `link.exe` was reachable via `PATH`. The allowlist
  in `child_env.rs` now preserves the 13 MSVC env vars so the
  toolchain context survives the sanitisation pass.
- **`code_execution` no longer fails with "program not found" on
  Windows** (and any other host without `python3` on `PATH`). Before
  v0.8.31 the tool hardcoded `python3` and was unconditionally
  advertised in Agent / YOLO modes — so the model would call it,
  spawn would fail, and the error surfaced as a generic tool failure
  with no upstream hint. The fix probes for a Python interpreter
  (`python3` → `python` → `py -3`) at catalog-build time, caches the
  resolved interpreter, and only advertises `code_execution` when one
  resolves. On hosts with no Python the tool is not registered at all
  — the model never sees a tool it can't actually run. Reported by a
  Windows contributor; resolver lives at
  `crates/tui/src/dependencies.rs` and is also surfaced by
  `deepseek doctor`. Folds in the contributor's "write code to a
  tempfile and run the file" suggestion at the same time, so multiline
  code with quote nesting no longer round-trips through `python3 -c`.
- **Termius and every SSH session auto-enable low-motion**
  (#1433, harvested from PR #1479 by **@CrepuscularIRIS / autoghclaw**).
  Termius desktop sets `TERM_PROGRAM=Termius`; sshd exports
  `SSH_CLIENT` for every TCP session and `SSH_TTY` for interactive
  PTY logins. Any of those signals now flips `low_motion` and
  `fancy_animations` like the existing VS Code / Ghostty path, so
  the 120 FPS cursor-repositioning that races the SSH round-trip
  no longer flickers a remote TUI. Disk-loaded `fancy_animations =
  true` is unconditionally overridden under these signals,
  matching the existing env-precedence contract.
- **DEC 2026 synchronized output is auto-disabled on Ptyxis** (the new
  default terminal on Ubuntu 26.04 and an increasingly common Linux
  TUI host). Ptyxis 50.x ships on VTE 0.84.x, which parses the
  `\x1b[?2026h` / `\x1b[?2026l` begin/end pair but still flashes the
  entire viewport on every wrapped frame instead of deferring
  rendering — so a TUI that uses DEC 2026 to avoid tearing
  experiences visible flicker on every redraw. gnome-terminal 3.58
  on the same VTE renders cleanly, so the heuristic must stay narrow:
  we trigger only on `TERM_PROGRAM` matching `ptyxis`
  case-insensitively, or `PTYXIS_VERSION` set to any non-empty value.
  Either signal flips the new `synchronized_output` setting from
  `auto` to `off`; the renderer then skips the begin/end pair on
  every draw, in `reset_terminal_viewport`, and in `resume_terminal`.
  Users on Ptyxis who upgrade past the upstream fix (or who want to
  confirm a fix landed) can override with
  `/set synchronized_output on` or by adding
  `synchronized_output = "on"` to `~/.config/deepseek/settings.toml`.

### Added

- **`deepseek doctor` now reports tool-dependency status.** A new
  "Tool Dependencies" section lists which external binaries the
  registered tools rely on, with ✓ when present and ✗ + an
  install hint when missing. Today this covers the Python
  interpreter (`code_execution`) and `pdftotext` (`read_file` PDF
  path). A separate "Terminal Quirks" section shows which env-driven
  auto-overrides (VS Code / Ghostty / Termius / SSH / Ptyxis) are
  currently active so users can see at a glance why a particular
  rendering compromise is in effect. Foundation for surfacing future
  tool dependencies as the toolset grows.
- **New `synchronized_output` setting** controls whether the renderer
  wraps each frame in DEC mode 2026 synchronized output. Accepts
  `auto` (default; respect the Ptyxis env opt-out), `on` (always emit
  DEC 2026, override the heuristic), or `off` (never emit DEC 2026).
  The cost of `off` is brief tearing on terminals that handle DEC
  2026 cleanly; it is purely a rendering-quality knob, not a
  correctness one. Set via `/set synchronized_output <auto|on|off>`
  or in `~/.config/deepseek/settings.toml`.
- **`read_file` accepts `start_line` and `max_lines`** for chunked,
  bounded reads of large files (#1450, harvested from PR #1451 by
  **@Oliver-ZPLiu**). Default window is 200 lines / ~16 KB; the hard
  cap is 500 lines. Small files (≤ 200 lines AND ≤ 16 KB) still
  return their contents unchanged, so existing prompts that read
  config files / single source files see no behavior change. Large
  files now return a `<file …>`-wrapped, line-numbered window with
  `shown_lines`, `truncated`, and `next_start_line` attributes plus
  a `[TRUNCATED]` continuation hint — so the model can page through
  a 50 KB file in 16 KB slices instead of dragging the whole thing
  into the conversation context on every turn. PDFs continue to use
  `pages`; `start_line` / `max_lines` apply to text files only.
- **`web/` dependency security updates.** Bumps:
  - `next` 15.5.16 → 15.5.18 (GHSA-26hh-7cqf-hhc6 — App Router
    middleware/proxy bypass via segment-prefetch routes; high
    severity).
  - `mermaid` 11.14.0 → 11.15.0 (GHSA family: Gantt-chart infinite-
    loop DoS, `classDef` HTML injection, `classDefs` /
    configuration CSS injection; all medium severity).
  - `eslint-config-next` 15.5.16 → 15.5.18 (matches Next.js).
  `npm run build` confirmed clean on the bumped lockfile. None of
  these affect the Rust TUI binary; the bumps are for the
  separately-deployed `deepseek-tui.com` site.
- **MCP HTTP servers accept custom headers** for authentication
  (#1454, harvested from PR #1456 by **@Oliver-ZPLiu**). Mirrors the
  `headers` field that Claude Code, Codex, and OpenCode already
  accept in their MCP config — add e.g.
  `"headers": { "Authorization": "Bearer ${HF_TOKEN}" }` under any
  HTTP server entry in `~/.deepseek/mcp.json` and the headers are
  sent on every Streamable HTTP request. Headers are sent
  literally — env-var interpolation is a follow-up, so tokens
  pasted directly into mcp.json live there as plain text. The
  Streamable HTTP transport filters out empty keys, framing
  overrides (`Accept`, `Content-Type`), and CR/LF in values
  (response-splitting defense) so a single bad entry can't break
  protocol negotiation or smuggle a header through a misbehaving
  proxy. Stdio servers (`command`-based) and the legacy SSE
  transport ignore the field; SSE coverage is a follow-up.

## [0.8.30] - 2026-05-11

A "tighten what we shipped" release. Bare single-letter keystrokes
(`g`, `G`, `[`, `]`, `?`, `l`, `v`) no longer get eaten as transcript-
nav shortcuts when the composer is empty — every one of them is now
freely usable as the first character of a message. The water-spout
animation in the footer is decoupled from `low_motion` so typewriter
mode no longer hides the wave, and the v0.3.5-era 🐳→🐋 cycling
indicator is back next to the effort chip after a long detour through
geometric dots. Plus a handful of provider, shell, and config fixes
that surfaced during v0.8.29 testing.

### Added

- **The whale is back.** Restored the `🐳 → 🐳. → 🐳.. → 🐳... → 🐋 → 🐋.
  → 🐋.. → 🐋... → 🐋.. → 🐋. → 🐳..` cycling status indicator that
  originally shipped in v0.3.5 and silently disappeared in commit
  `1a04659a9` (the "smoother TUI streaming" pass, which swapped the
  12-frame whale sequence for a 6-frame geometric `◍ ◉ ◌` ring) and then
  was deleted outright in `f4dbf828c` (footer-polish commit). The chip
  renders in the header status cluster, immediately before the
  reasoning-effort chip — exactly where long-time users remember it.
  Idle frame is a steady 🐳; the cycle advances every 420 ms keyed off
  `App::turn_started_at`, so the breaching whale shows up halfway
  through any active turn.

  Configurable via the new `status_indicator` setting:
  - `whale` (default) — the historical cycling whale.
  - `dots` — the geometric `◍ ◉ ◌` frames from the dots era.
  - `off` — hide the chip entirely.

  Set via `/config status_indicator <whale|dots|off>` or in
  `settings.toml`.

### Changed

- **Transcript-nav single-letter shortcuts now require `Alt`.** Before
  v0.8.30, pressing a bare `g`, `G`, `[`, `]`, `?`, `l`, or `v` with an
  empty composer hijacked the keystroke for transcript navigation — so
  typing "good morning" produced "ood morning" with no warning, and the
  v0.8.29 spot-fix at `c13ddb04d` (gg double-tap) only suppressed the
  scroll, not the lost character. The bindings are now uniformly
  `Alt+<key>`, mirroring `Alt+R` (history search) and `Alt+V` (tool
  details) which already followed this pattern:

  | Old (bare) | New (`Alt+…`) | What it does |
  |---|---|---|
  | `gg` (double-tap) | `Alt+G` | scroll transcript to top |
  | `G` or `Shift+G` | `Alt+Shift+G` | scroll transcript to bottom |
  | `[` | `Alt+[` | jump to previous tool output |
  | `]` | `Alt+]` | jump to next tool output |
  | `?` | `Alt+?` | open the searchable help overlay (F1 / `Ctrl+/` also bound) |
  | `l` | `Alt+L` | open pager for last message |
  | `v` / `V` | `Alt+V` | open tool-details pager |

  Plain letters are now always inserted into the composer as text. The
  `App::transcript_pending_g` field from the v0.8.29 half-fix is removed;
  the unified `alt_nav_modifiers` predicate replaces the per-key
  `is_empty()` checks.

### Fixed

- **`low_motion = true` no longer hides the footer water-spout** when
  `fancy_animations = true`. The spout-strip animation in the footer was
  hard-gated on `!low_motion`, collapsing two unrelated concerns —
  streaming pacing and footer animation — onto one flag. The two are now
  orthogonal: `low_motion` governs only streaming pacing (typewriter vs.
  upstream cadence), and `fancy_animations` alone decides whether the
  water-spout strip renders. The wave itself is unchanged from prior
  releases (wall-clock-driven sine, same cadence as v0.8.29).
- **Custom-base-URL providers preserve the user's model name** (#857
  class). Only OpenRouter was previously whitelisted; Sglang, Novita,
  Fireworks, Vllm, Ollama, and NvidiaNim users hitting custom gateways
  with a bare model name were getting HTTP 400s because the dispatcher
  rewrote the model identifier. Now any provider with a user-set
  `base_url` is treated as a custom endpoint and passes the model name
  through unchanged.
- **`exec_shell` no longer freezes the TUI when a background subprocess
  outlives its parent shell** (#828, cherry-picked from PR #1475 by
  **@CrepuscularIRIS / autoghclaw**). Orphaned children that kept the
  pipe write-end open made `handle.join()` in `collect_output` block
  indefinitely; every transcript-rendering tick that called
  `list_jobs()` then hung the UI. The collector now kills the process
  group before joining the reader threads, and the previously dead
  `cleanup()` is now wired to drop completed jobs older than an hour.

## [0.8.29] - 2026-05-11

A maintenance release anchored by a regression fix for the
"scroll demon" (#1085 class, re-introduced by v0.8.27's flicker
patch) and a wrong-project session-restore bug (#1395). Plus 25
community PRs covering MCP transport, prompt steering, auto-routing
language coverage, web-search SERP filtering, and broad test
coverage additions.

### Fixed

- **Scroll demon — alt-screen no longer drifts under parallel
  sub-agent load** (#1085 regression). The v0.8.27 flicker fix
  dropped the `\x1b[2J\x1b[3J` deep-clear from the viewport-reset
  path, which had been silently masking three `eprintln!` sites
  inside the sub-agent and network-policy modules. Each leak
  scrolled the alt-screen up by one row while ratatui's diff
  renderer remained convinced its model matched reality. Three
  layers of defence now ship together: a `tracing-subscriber`
  writing to `~/.deepseek/logs/tui-YYYY-MM-DD.log`, an fd-level
  `dup2` stderr redirect for the alt-screen lifetime (Unix only;
  Windows follow-up tracked), and module-level
  `#![deny(clippy::print_stdout, clippy::print_stderr)]` on
  `tools/`, `core/`, `tui/`, `runtime_threads.rs`, and
  `network_policy.rs`. The three known leak sites
  (`subagent::persist_state_best_effort`,
  `subagent::new_shared_subagent_manager`, `network_policy::record`)
  now route through `tracing::warn!` with structured fields.
- **`Ctrl+R` session-restore picker is workspace-scoped** (#1395,
  PR #1397 from **@linzhiqin2003**). `SessionPickerView::new`
  previously listed every saved session on disk sorted globally —
  so opening DeepSeek-TUI in Project B and pressing `Ctrl+R` could
  hand back Project A's last conversation. The picker now filters
  by current workspace, with a fallback hint when no in-workspace
  sessions exist.
- **MCP discovery survives malformed items** (PR #1410 from
  **@Liu-Vince**). The `tools/list`, `resources/list`,
  `resources/templates/list`, and `prompts/list` walks previously
  did `serde_json::from_value::<Vec<…>>(…).unwrap_or_default()`,
  which silently discarded the entire page when any single entry
  was misshapen. Each list now iterates per-item, skipping
  malformed entries with a `tracing::debug!` instead of dropping
  the rest of the catalogue. Composes with the v0.8.x pagination
  loop landed for #1256.
- **MCP SSE transport accepts CRLF-framed endpoint events** (#1309,
  PR #1358 from **@reidliu41**). FastMCP / uvicorn-style SSE
  streams using `\r\n\r\n` separators now discover the endpoint and
  send initialization requests instead of timing out while waiting
  for an LF-only event boundary.
- **Composer ignores leaked SGR mouse-report bursts** (#1418,
  PR #1421 from **@reidliu41**). Some SSH / IDE terminal chains
  leak fragments like `[<35;44;18M` into stdin while mouse capture
  is enabled; the composer now filters those bursts at the insertion
  boundary without stripping ordinary coordinate-like typed text.
- **Footer right-cluster chips can no longer crowd the left status
  line** (#1357, PR #1417 from **@Wenjunyun123**). The footer now
  reserves visible space for the left status before selecting cache /
  aux chips, dropping oversized right-side chips instead of pushing
  the row over the available terminal width.
- **Web search drops spam-stuffed SERPs** (#964, PR #1396 from
  **@linzhiqin2003**). The Bing / DDG fallback paths now filter
  the SEO-farm domains that were poisoning quick lookups.
- **Language directive: `reasoning_content` follows the user's
  message language** (#1118, PR #1398 from **@linzhiqin2003**) —
  previously the project context's inferred `lang` could override
  the latest user message, leading to English thinking for a
  Chinese turn.
- **Deferred tools hydrate their schema before first execution**
  (#1419, PR #1429 from **@SamhandsomeLee**). When the model asks
  for a deferred tool such as `edit_file` before seeing its schema,
  the engine now loads the tool, returns a non-executed hydration
  result with the expected fields, and requires a retry instead of
  executing guessed argument names. Common `edit_file` aliases such
  as `old_string -> search` and `new_string -> replace` are called
  out in the retry hint.
- **DeepSeek public aliases replay thinking-mode tool turns**
  (PR #1428 from **@Beltran12138**). `deepseek-chat` and
  `deepseek-reasoner` now classify as V4 reasoning models for
  `reasoning_content` replay, preventing second-turn HTTP 400s
  after tool calls when users keep the onboarding default model
  alias.
- **`Ctrl+O` expands thinking blocks still in flight.**
  Two compounding bugs were making the "thinking collapsed; press
  Ctrl+O for full text" affordance a lie. (1) `open_thinking_pager`
  only searched `app.history`, but after `ThinkingComplete` the
  finalized thinking entry sits in `app.active_cell` with
  `streaming = false` until the active cell flushes at end-of-turn;
  during that window the handler surfaced "No thinking blocks to
  expand" while the affordance pointed at the live entry. Routed
  through the existing `cell_at_virtual_index` / `virtual_cell_count`
  resolver that `open_tool_details_pager` already uses, so
  selection-based and most-recent lookups both reach in-flight
  entries. (2) The keybinding guard required `key.modifiers ==
  KeyModifiers::CONTROL` (exact match), so any extra modifier bit
  set by the terminal — Shift while a native-selection bypass was
  active, Caps Lock indicator on some keyboard layouts — silently
  fell through to the `$EDITOR` arm and did nothing visible on an
  empty composer. Relaxed to `contains(KeyModifiers::CONTROL)` to
  match the existing Ctrl+P / Ctrl+B pattern. Regression-guarded by
  `open_thinking_pager_finds_thinking_in_active_cell`.
- **Skill completions no longer flood the top-level slash menu**
  (#1437, PR #1442 from **@reidliu41**). Installed skills now
  complete under `/skill <name>` while the root `/` menu stays
  focused on built-in commands.
- **`edit_file` rejects no-op replacements** (PR #1460 from
  **@xiluoduyu**). Identical `search` / `replace` arguments now
  fail fast with a clear validation error instead of producing an
  empty diff that can trap the model in retry loops.
- **Windows-terminal glyph widths are stable** (#1314, PR #1465
  from **@CrepuscularIRIS**). SMP emoji in the header and file tree
  were replaced with BMP-width-safe symbols / text so cmd,
  PowerShell, WezTerm, and Alacritty do not mismeasure rows.
- **Ghostty defaults to low-motion rendering** (#1445, PR #1468
  from **@CrepuscularIRIS**). `TERM_PROGRAM=ghostty` now receives
  the same animation cap as VS Code terminals to avoid redraw
  flicker on affected setups.
- **Docker buildx provenance permission failures get an actionable
  hint** (#1449, PR #1469 from **@CrepuscularIRIS**). macOS shell
  outputs matching the restricted provenance metadata failure now
  include guidance to disable provenance for that build.
- **Windows CMD mouse-wheel fallback scrolls the transcript**
  (#1443, PR #1471 from **@CrepuscularIRIS**). When mouse capture is
  off, composer arrow-scroll defaults on so terminal wheel events
  mapped to Up / Down do not cycle composer history.

### Added

- **MCP HTTP transport honors `HTTP(S)_PROXY` / `NO_PROXY`** (#1408
  from **@hlx98007**). Reqwest 0.13 does not auto-detect proxy env
  vars by default, so MCP HTTP connections were bypassing the
  proxy that every other tool on the box (curl, npm, git, …) was
  using. Connections behind corporate egress proxies and
  China-mainland Clash / Shadowsocks tunnels now work transparently.
  Malformed `HTTPS_PROXY` values log a `tracing::warn!` and the
  connection proceeds without a proxy rather than failing the MCP
  attach.
- **Note management slash commands** (PR #1407 from
  **@reidliu41**). `/note add`, `/note list`, and friends for
  persistent maintainer-style notes inside the TUI, backed by
  `~/.deepseek/notes/`.
- **Header surfaces the runtime version chip.** A `v0.8.29` tag
  sits in the header's right cluster after the provider / effort /
  Live / context chips. Styled with `palette::TEXT_HINT` so it
  reads behind the streaming indicators. Drops first under tight
  terminal width.
- **Global `~/.deepseek/AGENTS.md` now merges with project
  AGENTS.md** (#1157, PR #1399 from **@linzhiqin2003**) instead of
  being shadowed when a workspace ships its own.
- **Auto-routing recognises CJK debug / search keywords** (PRs
  #1401 and #1402 from **@linzhiqin2003**) — `--model auto` and
  the reasoning-effort picker correctly route Chinese / Japanese
  technical queries that previously fell through to the generic
  baseline.

### Security

- **`sync-cnb.yml` workflow hardened** (CodeQL finding from
  v0.8.28). Adds explicit `permissions: contents: read`
  (least-privilege), bumps `actions/checkout` v3 → v4, and
  narrows the trigger from `on: [push]` to `on: push.branches:
  [main]` + `tags: ['v*']`. Feature branches no longer mirror to
  CNB; only `main` and tagged releases do.
- **Post-exit resume hint avoids session-id taint.** The TUI now
  checks whether a session exists separately from the constant
  resume-hint text it prints after leaving the alt-screen, resolving
  the `rust/cleartext-logging` CodeQL alert without reintroducing
  scroll-demon stdout writes.

### Internal

- **+438 LOC of new test coverage** across four PRs from
  **@linzhiqin2003**: `error_taxonomy::classify_error_message`
  and Display impls (#1403), `parse_pages_arg` edge cases (#1404),
  `optional_search_max_results` precedence (#1405), and
  `sanitize_stream_chunk` control-byte filtering (#1406).
- **`runtime_log` module** ships with a regression test pinning
  the `HOME` / `USERPROFILE` / `dirs::home_dir()` resolution
  order, holding the process-wide `test_support::lock_test_env()`
  lock for env-mutation safety.
- **Header rendering** gains two regression tests
  (`header_renders_version_chip_when_width_allows` and
  `narrow_header_drops_version_chip_before_dropping_mode`)
  pinning the version chip's cascade priority.
- **Workspace/session test isolation** tightened (PR #1431 from
  **@reidliu41**). Git-root detection ignores invalid parent `.git`
  markers, env-mutating tests share the crate-wide test lock, and
  the streamable HTTP MCP mock server stays alive for the full test.
- **Config-mutating smoke tests now isolate `DEEPSEEK_CONFIG_PATH`.**
  The command registry and web-config commit tests no longer rewrite
  the developer's real `~/.deepseek/config.toml` while validating
  release candidates locally.

## [0.8.28] - 2026-05-10

A maintenance release bundling four streaming / approvals / cache
bug-fix cherry-picks, six smaller community fixes, a Cmux
notification probe, GPU-terminal flicker hardening via DEC 2026
synchronized output, VS Code low-motion auto-detection, a CNB
mirror workflow, V4-steered tool descriptions, and test-suite
stabilization for parallel-test environment races.

### Added

- **CNB mirror workflow** (PR #1373 from **@Anyexyz**) — a
  GitHub Actions workflow (`sync-cnb.yml`) mirrors every push to
  the `cnb.cool/deepseek-tui.com/DeepSeek-TUI` repository,
  closing out the long-standing China-mirror request. Requires the
  `CNB_GIT_TOKEN` repo secret.
- **Cmux desktop notification support via `LC_TERMINAL`** (#1281,
  PR #1340 from **@CrepuscularIRIS**) — Cmux sets
  `LC_TERMINAL=Cmux` rather than `TERM_PROGRAM`, so the previous
  notification probe fell back to `BEL` instead of using OSC 9.
  `resolve_method()` now checks `LC_TERMINAL` as a secondary probe
  and adds Cmux to the OSC 9 allowlist. Terminals that set
  neither env var can still force OSC 9 via
  `[notifications].method = "osc9"`. Two regression tests pin the
  Cmux and WezTerm `LC_TERMINAL` paths; the existing
  unknown-terminal-on-Unix test now clears `LC_TERMINAL` before
  asserting fallback so it doesn't flake on CI hosts that set it.
- **DEC 2026 synchronized output around terminal repaints** (PR
  #1361 from **@xuezhaoyu**) — the viewport-reset path now wraps
  `terminal.clear()` in `\x1b[?2026h` / `\x1b[?2026l` so
  GPU-accelerated terminals (Ghostty, VSCode Terminal, Kitty,
  WezTerm) defer rendering until the whole frame is staged,
  eliminating mid-frame flicker on resize / focus / TurnComplete.
  The earlier "drop destructive 2J/3J" fix from v0.8.27 stays;
  this PR is complementary, batching the same lighter reset
  sequence into a single synchronized frame. Terminals without
  DEC 2026 support silently ignore the sequence.
- **`low_motion` auto-enables under VS Code integrated terminal**
  (PR #1365 from **@CrepuscularIRIS**) — `apply_env_overrides()`
  now treats `TERM_PROGRAM=vscode` the same way it treats
  `NO_ANIMATIONS=1`: force `low_motion = true` and
  `fancy_animations = false`. The VS Code terminal compositor
  cannot keep up with 120 fps redraws and produces rapid flicker
  (#1356); the 30 fps low-motion cap is the right default there.
  Env overlays always win over the disk-loaded value, matching
  the existing precedence for `NO_ANIMATIONS`.

### Fixed

- **Cache usage shows 0 when API omits cache data** (#1391, PR #1392
  from **@Oliver-ZPLiu**) — `SessionUsage.cache_creation_input_tokens` /
  `cache_read_input_tokens` are now `Option<u64>` instead of `u64`
  defaulting to 0. When the upstream API doesn't report cache
  hit/miss, the model sees `null` instead of misleading zeros, and
  reasoning about cache utilization is accurate.
- **Deny of one tool call no longer blocks all future calls of the
  same tool** (#1377, PR #1388 from **@Oliver-ZPLiu**) — denying a
  tool call now only caches the per-call `approval_key`, not the
  tool type. Subsequent invocations of the same tool prompt for
  approval again instead of being silently auto-denied.
- **Streaming thinking blocks no longer drop their tail on
  MessageComplete** (#861 RC3, PR #1389 from **@linzhiqin2003**) —
  the active streaming entry is now drained into the finalized
  cell on `MessageComplete`, eliminating a data-loss path where
  the last chunk(s) of a streaming "thinking" reply could be
  discarded when `MessageComplete` arrived ahead of
  `ThinkingComplete` in a bursty event stream. Also closes a
  related HTTP 400 on the next turn (DeepSeek V4 requires
  `reasoning_content` replay for assistant messages that carry
  tool calls).
- **Streaming thinking renders live in collapsed view** (#861 RC4,
  #1324, PR #1390 from **@linzhiqin2003**) — collapsed thinking
  cells now stream their content as it arrives instead of staying
  at a static "thinking..." placeholder until streaming ends. When
  the live body exceeds the collapsed budget, the truncation
  affordance ("thinking continues; press Ctrl+O for full text")
  now fires during streaming with head lines dropped so the
  visible window tracks the live cursor at the bottom.
- **First-turn latency bounded on large workspaces** (#697, PR #1386
  from **@linzhiqin2003**) — the working-set file walker now caps
  the number of entries it visits during initial indexing, so
  starting a session in a workspace with a deep `node_modules`,
  `target`, or `.venv` no longer stalls the first response on
  filesystem traversal.
- **Duplicate error toast on transcript-rendered turn errors** (PR
  #1368 from **@douglarek**) — when a turn error is already in the
  transcript as a system/error cell, the status-line toast is
  suppressed so the user doesn't see the same failure twice.
- **Clearer continue tip on idle prompts** (PR #1370 from
  **@nightfallsad**) — the "press Tab to continue" affordance now
  uses concrete language instead of a vague hint.
- **Ctrl+Enter content lost when engine is idle** (#1331, PR #1347
  from **@Oliver-ZPLiu**) — when no turn was active, `Ctrl+Enter`
  routed the message to `rx_steer` (only monitored inside
  `handle_deepseek_turn`), so the user saw their message in the
  transcript via the local mirror but the LLM never received it —
  the next regular Enter would drain it as a "stale steer". The
  idle path now sends through the standard `handle_send_message`
  flow so the submission reaches the engine.
- **Explicit hidden / ignored `@`-mention completions work**
  (#1270 follow-up) — PR #1270 from **@SamhandsomeLee** landed
  the `add_local_reference_completions` helper and tests in
  v0.8.27 but never wired it into `Workspace::completions()` or
  `build_file_index`. The two regression tests were ignored with
  a "v0.8.28 follow-up" marker. This release wires the helper
  into both entry points so `@.deepseek/commands/start-task.md`
  and `@.generated/specs/device-layout.md` (and the basename
  fuzzy-resolve equivalent) now surface from gitignored
  user-folders while `.deepseekignore` entries stay blocked.
  Both tests un-ignored.

### Changed

- **Prompt-side reliability guidance** (PR #1393 from
  **@Oliver-ZPLiu**) — `prompts/base.md` gains three Verification
  Principle bullets steering the model to verify before reporting
  complete, preserve only key facts from tool results, and
  inspect errors before retrying. Combined with the truthful-
  reporting addition from #1392, the model is less likely to claim
  unverified successes or repeat the identical failing tool call.
- **V4-steered tool descriptions** (#711, PR #1379 from
  **@linzhiqin2003**) — every model-visible tool description
  (`read_file`, `write_file`, `edit_file`, `list_dir`,
  `grep_files`, `file_search`, `web_search`, `apply_patch`,
  `fetch_url`) now opens with a short *"use this instead of X
  in exec_shell"* steering line, the return shape, and the
  limits. Routes V4 toward our typed tools and away from
  shell footguns. All description strings stay under 1024
  chars (max: 350) with no embedded newlines so the cached
  tool catalogue stays prefix-stable for V4's KV cache.
  Removes the unused legacy `normal.txt` / `plan.txt` /
  `yolo.txt` prompt templates (referenced only by their own
  self-tests).

### Internal

- Test-suite parallelism stabilization (commit
  `test: stabilize parallel test execution`). Folds three local
  test-mutex implementations into the process-wide
  `test_support::lock_test_env`, eliminating a class of
  intermittent failures (`refresh_system_prompt_is_noop_when_unchanged`,
  `save_api_key_for_openrouter_writes_provider_table`,
  `list_archives_sorts_by_cycle_number`) observed during the
  v0.8.27 release cycle.
- Windows `task_manager` timeout bumped 3s → 10s on four tests
  exercising durable task recovery, addressing an intermittent
  CI timeout on Windows under file-I/O load.
- `provider_switch_clears_turn_cache_history` now isolates
  `HOME` / `USERPROFILE` to a tempdir for its lifetime. The test
  was silently writing `default_provider = "ollama"` to the
  developer's real `~/Library/Application Support/deepseek/settings.toml`
  on every run, which then contaminated parallel-running picker
  tests because Ollama is a pass-through provider that hides the
  DeepSeek model rows.
- `settings::tests::no_animations_test_guard` and
  `term_program_test_guard` both now return
  `crate::test_support::lock_test_env()` instead of their own
  module-local mutexes — folding them into the same
  process-wide test env lock the v0.8.27 EnvGuard family was
  migrated to. Without this, a `NO_ANIMATIONS=1` write from one
  test family could race a `TERM_PROGRAM=iTerm.app` write from
  the other through the shared `apply_env_overrides` path and
  flip `low_motion` to `true` on the assertion side.

## [0.8.27] - 2026-05-10

A polish release bundling 17 community PRs plus a focused user-issue
sweep over the 24–48 hours after v0.8.26 shipped. Headline fixes:
cross-terminal flicker on Ghostty / VSCode / Win10 conhost (most-
reported v0.8.26 regression), long-text right-edge overflow, an
in-app pager copy-out, context-sensitive Ctrl+C, an MCP pool that
auto-reloads on config changes, and a model-callable `notify` tool.
Big thanks to every contributor below.

### Added

- **Unified `/mode` command** (#1247) — `/mode [agent|plan|yolo|1|2|3]`
  replaces the separate `/agent`, `/plan`, and `/yolo` commands. Running
  `/mode` without arguments opens a picker modal. The legacy aliases
  (`/yolo`, `/agent`, `/plan`) are kept as compatibility shorthands.
  Thanks **@reidliu41**.
- **`/status` runtime diagnostics** (#1223) — shows version, provider,
  model, workspace, mode, permissions, context-window usage, cache
  hit/miss, and session cost. Previously `/status` was an alias for
  `/statusline` (footer config); that alias is now `/statusline` only.
  Thanks **@reidliu41**.
- **`/feedback` command** (#1185) — opens the matching GitHub issue
  template (bug report, feature request) in the browser. Security
  vulnerability reports route through the project's security policy
  page first. Thanks **@reidliu41**.
- **Session artifact metadata** (#1220) — large tool outputs spilled to
  the session artifacts directory are now tracked in a durable metadata
  index, so saved sessions retain references across save/restore cycles.
  Thanks **@THINKER-ONLY**.
- **Subagent results are self-reports** (#1140) — the compacted result
  summary now notes that child-agent outputs are unverified self-reports.
  The parent model should verify side effects with tools like `read_file`
  or `list_dir` before claiming success. Thanks **@THINKER-ONLY**.
- **Global AGENTS.md fallback** (#1197) — when the workspace and its
  parents don't provide project instructions, the TUI now loads
  `~/.deepseek/AGENTS.md` before falling back to auto-generated
  instructions. Repo-local context still takes priority.
  Thanks **@manaskarra**.
- **`--yolo` forwarded from CLI to TUI** (#1233) — the `deepseek --yolo`
  flag now propagates through the dispatcher to the TUI binary via
  `DEEPSEEK_YOLO=true`. Previously the flag set `yolo` in the CLI
  process but the TUI session started in its default mode.
  Thanks **@fuleinist**.
- **`composer_arrows_scroll` config** (#1211) — a new
  `tui.composer_arrows_scroll` option (default `false`) makes plain
  Up/Down arrow keys scroll the transcript when the composer is empty,
  instead of navigating input history. Helpful for terminals that map
  trackpad gestures to arrow keys. Thanks **@lbcheng888**.
- **Session cost persistence** (#1192) — accumulated costs (session +
  sub-agents, both USD and CNY) and the displayed-cost high-water mark
  now survive session save/restore, so the monotonic cost guarantee
  (#244) holds across restarts. Thanks **@lbcheng888**.
- **Provider-aware model picker and provider persistence** (#1320) —
  switching providers now persists the choice to
  `~/.deepseek/settings.toml` so it survives restarts. The model
  picker hides DeepSeek-specific models when a non-DeepSeek provider
  is active. `OPENAI_MODEL` env var now overrides the per-provider
  model rather than the global `default_text_model`. Bailian / ZhiPu
  Coding Plan endpoints are now supported.
  Thanks **@imkingjh999**.
- **HTTP User-Agent header** (#1320) — all outbound API requests now
  carry `deepseek-tui/{version}` in the User-Agent, matching the format
  `fetch_url` already uses. Thanks **@imkingjh999**.

### Fixed

- **Cross-terminal flicker on TurnComplete / focus / resize** (#1119,
  #1260, #1295, #1352, #1356, #1363, #1366) — the viewport-reset
  sequence emitted before each forced repaint no longer includes
  `\x1b[2J\x1b[3J`. Combined with the immediately-following ratatui
  `terminal.clear()`, the destructive pair produced a double-clear that
  Ghostty, the VSCode integrated terminal, and Win10 conhost rendered
  as a visible blank-then-repaint flicker. The lighter sequence
  (`\x1b[r\x1b[?6l\x1b[H`) plus the alt-screen buffer's double-buffering
  handles viewport correctness without flicker. macOS Terminal.app /
  iTerm2 / alacritty users were already unaffected and remain so.
- **`/skills --remote` and `/skills sync` diagnostics** (#1329) — the
  underlying anyhow chain has always been formatted with `{err:#}`, but
  the chain alone is often opaque (e.g. "error sending request"). The
  error message now appends a one-line hint when the chain matches a
  common failure pattern: DNS / connection refused / TLS / 4xx / 429 /
  timeout. Each hint points at the most likely cause and a concrete
  next step.

### Added

- **Pager copy-out** (#1354) — full-screen pagers (`Alt+V` tool details,
  `Ctrl+O` thinking content, shell-job / task / MCP-manager pagers, and
  the selection pager) now accept `c` or `y` to copy the entire body to
  the system clipboard. The pager intercepts mouse capture so terminal-
  native selection isn't available inside it; this restores the
  copy-out path that users on macOS / Windows / WSL expect. The footer
  hint now reads `…  / search  c copy  q/Esc close`. A status toast
  confirms success ("Pager content copied"), empty-body, or failure.
- **`notify` tool** (#1322) — model-callable desktop notification.
  Always-loaded (no ToolSearch round-trip). Routes through the existing
  `tui::notifications` infrastructure: OSC 9 on iTerm2 / Ghostty /
  WezTerm, BEL fallback on macOS / Linux, `MessageBeep` on Windows when
  explicitly opted in. Honours the user's `[notifications].method`
  config — when set to `off`, the tool is a silent no-op. Title and
  body are length-capped (80 / 200 chars) on character (not byte)
  boundaries to keep the OSC 9 escape clean and avoid mid-grapheme
  truncation. The tool description steers the model away from chatter:
  use only when a long-running task completes or genuinely needs the
  user's attention.

### Fixed (cont.)

- **Long output text overflowed the right edge** (#1344, #1351) —
  paragraph rendering (`render_line_with_links`) and code-block
  wrapping (`wrap_text` for `Block::Code`) were word-based: a single
  word wider than the available column was placed alone on a line and
  silently overflowed. Long URLs, paths, hashes, and no-whitespace CJK
  runs all hit this. Both paths now hard-break overlong words at the
  character level, matching the v0.8.25 fix for table cells. The
  rendered width is capped at the budget for every line; full content
  is preserved across wrapped segments. Snapshot-style tests pin the
  invariant at widths 40, 60, 80, and 120.

### Changed

- **`Ctrl+C` now copies an active transcript selection** (#1337) — on
  Windows, plain `Ctrl+C` is the OS-wide copy chord, and treating it
  as "exit" stole work whenever a user copy-pasted from the
  transcript. `Ctrl+C` is now a four-stage decision: 1) selection
  active → copy + clear (matches the OS convention); 2) turn in
  flight → cancel (unchanged); 3) quit-armed within 2s → exit cleanly
  (unchanged); 4) idle, no selection → arm the 2-second
  "press Ctrl+C again to quit" prompt (unchanged). The decision is
  factored into a `CtrlCDisposition` helper with a unit-tested
  priority table. `Cmd+C` (macOS) and `Ctrl+Shift+C` continue to copy
  unchanged.
- **Cancel-key discoverability hint on turn start** (#1367) — when a
  turn begins, the status-message slot now surfaces "Press Esc or
  Ctrl+C to cancel" if the slot is otherwise empty. Real transient
  status messages still take precedence; the hint clears as soon as
  any other update fires. Closes the loop on users who didn't know
  how to interrupt a long-running turn.
- **Lazy auto-reload of MCP pool on config-file change** (#1267 part 2) —
  v0.8.26 surfaced the underlying spawn errors; v0.8.27 closes the
  loop on the second half of the report (manual `/mcp reload` after
  `~/.deepseek/mcp.json` edits). `McpPool::get_or_connect` now does a
  cheap `stat` + content-hash check before each connection lookup. If
  the on-disk file's mtime moved AND its content hash changed since
  the pool was loaded, all live connections are dropped so the next
  `get_or_connect` reattaches under the new config. Pool-construction
  via `McpPool::new` (tests, ad-hoc snapshots) is unaffected — only
  pools built with `from_config_path` watch the source file. No file
  watcher; no long-lived task. mtime-only churn (touched but
  byte-unchanged content) does not trigger a reload, so networked
  filesystems with coarse mtime granularity won't churn the pool.
- **Paste consolidation now happens at paste time, not submit time** —
  large bracketed pastes that exceed the 16 000-char safety cap are
  now folded into a workspace `.deepseek/pastes/paste-…md` file and
  swapped for an `@`-mention immediately on paste, instead of waiting
  until the user presses Enter. The user sees the `@`-mention in the
  composer (and the "consolidated → @mention" toast) before deciding
  whether to send, eliminating the "I pressed Enter and an `@`-mention
  appeared in the chat I didn't authorise" surprise. The submit-time
  consolidation remains as a safety net for any other code path that
  fills the buffer above the cap, so the cap is still enforced exactly
  once.
- **Auto-disable paste-burst once bracketed paste verified** — the
  rapid-keystroke paste-burst heuristic (default-on for terminals
  without bracketed paste) used to keep running on every session.
  Once a real `Event::Paste` arrives in a session, paste-burst now
  short-circuits — bracketed paste is verified working, and running
  the heuristic alongside it just creates false positives on fast
  typing / IME commits / autocomplete bursts. Terminals that never
  deliver bracketed paste (the original target audience) are
  unaffected; the heuristic still fires there.
- **Short CJK multi-line paste no longer auto-submits first line**
  (#1302) — pasting `请联网搜索：\nSTM32 …` (short non-ASCII first line
  followed by a newline) used to fail the paste-burst detection
  heuristic because the first line had no whitespace and was under
  the 16-char threshold; the trailing pasted newline then fell
  through as a real Enter and submitted the first line on its own.
  The heuristic now treats any non-ASCII run as paste-like, so the
  Enter is absorbed into the burst buffer. Thanks **@reidliu41**
  (PR #1342).
- **Onboarding screens render in the selected language** — when a
  user picked 简体中文 / 日本語 / Português (Brasil) at the language
  step, every subsequent screen (API key entry, workspace trust
  prompt, final tips) used to remain in English. The
  `set_locale_from_onboarding` path now drives the title, body
  copy, hints, and footer of each onboarding screen through the
  localization table, so once you pick your language the rest of
  the flow is in that language. Particularly nice for users on
  CJK input methods who want to avoid IME juggling during setup.
- **`/skills <prefix>` filters the local skills list** (#1318) — on
  top of the v0.8.26 inter-row spacing (#1328 from @reidliu41), the
  list now narrows to skills whose names start with the typed
  prefix. Case-insensitive. The header reflects matched count vs
  registry total; an empty match set says so explicitly and points
  back at unfiltered `/skills`. `--remote` and `sync` stay
  reserved as subcommands; any `--`-prefixed argument is rejected
  rather than being silently treated as a no-match prefix.
- **HTTP 400 quota errors retried** (#1203) — some OpenAI-compatible
  gateways return quota/rate-limit errors as HTTP 400 instead of 429.
  These are now classified as retryable `RateLimited` errors.
  Thanks **@dst1213**.
- **Explicit hidden/ignored file completions** (#1270) — when the user
  types an explicit path starting with `.` (e.g., `.deepseek/commands/`),
  the file-completion system now surfaces hidden and gitignored entries
  while still respecting `.deepseekignore`. Thanks **@SamhandsomeLee**.

### Changed

- **Windows mouse capture docs** (#1181) — the `--mouse-capture` help
  text and the configuration docs now mention scrollbar dragging and
  note that raw terminal selection on Windows may cross the sidebar.
  Thanks **@Oliver-ZPLiu**.
- **README zh-CN sync** (#1235) — the Chinese README's quickstart section
  now shows `deepseek run pr <N>` instead of the outdated
  `deepseek pr <N>`. Thanks **@whtis**.
- **Tool output render perf** (#1098) — tool output summaries and the
  "is this a diff?" check are now pre-computed once at cell creation
  instead of re-parsed every frame. Tool output cells also got a visual
  card-rail (`╭ │ ╰`) for clearer grouping. Thanks **@lbcheng888**.

### Internal

- Test coverage for approval decision branches (@tuohai666, #1316)
- Test coverage for hook event dispatch paths (@tuohai666, #1317)

## [0.8.26] - 2026-05-09

A security + polish release. Two responsibly-disclosed issues were
patched, plus a small batch of internal release-pipeline fixes. Big
thanks to **@JafarAkhondali** and **@47Cid** for the disclosures.

### Security

- Hardened the `fetch_url` tool's network-target validation
  (GHSA-88gh-2526-gfrr). Thanks to **@JafarAkhondali**.
- Tightened the default privileges of sub-agents created through
  `task_create` (GHSA-72w5-pf8h-xfp4). Thanks to **@47Cid**.

Both items will have full advisory text once the GHSA entries are
published.

### Fixed

- **Hint when root `base_url` is set with a non-DeepSeek provider
  (#1308)** — config load now logs a warning telling the user to
  move the URL under the matching `[providers.<name>]` table or use
  the `*_BASE_URL` env var. Closes the silent-ignore footgun for
  Ollama / vLLM / OpenAI-compatible setups.
- **Insecure base-URL error message is more discoverable (#1303)** —
  the rejection now spells out which env var to set (with underscores
  visible), notes that loopback hosts are auto-allowed, and shows a
  one-line `DEEPSEEK_ALLOW_INSECURE_HTTP=1 deepseek` example.
- **Workspace skills survive prompt truncation** — when the skill
  catalog needs trimming to fit the prompt budget, workspace-local
  skills now keep precedence over global ones rather than being
  truncated indiscriminately. Thanks **@hhhaiai**.
- **`/skills` listing has visual spacing** between entries so long
  skill descriptions don't run together. Thanks **@reidliu41**.
- **Provider base-URL overrides reach the active provider** — the
  per-provider `*_BASE_URL` env vars (e.g. `OPENAI_BASE_URL`,
  `OPENROUTER_BASE_URL`) now propagate into the active provider's
  config entry consistently. Closes a gap where the override was
  parsed but never applied. Thanks **@reidliu41**.
- **WSL2 turn-start timeout** — `TurnStarted` is now emitted before the
  snapshot step so a slow snapshot on WSL2's `/mnt/*` volumes doesn't
  push past the runtime watchdog and surface a spurious "engine may
  have stopped" error. Thanks **@michaeltse321**.
- **`/init` auto-adds `.deepseek/` to `.gitignore` (#1326)** when the
  workspace is a git repo, so workspace-local snapshots, instructions,
  and pastes don't get accidentally committed. Idempotent on repeated
  runs. Thanks **@Giggitycountless**.
- **MCP tool ordering is deterministic** — discovered tools and the
  resulting API tool block are now sorted by name so the prompt
  prefix the model sees is stable across runs, regardless of
  server-side pagination order. Improves prompt-cache hit rates with
  multi-server MCP setups. Thanks **@hxy91819**.
- **Error cells render as plain text** so env-var names (`API_KEY_FOO`)
  in error messages keep their underscores instead of being parsed as
  markdown emphasis. Thanks **@douglarek**.
- **`/clear` resets the Todos sidebar (#1258)** — previously `/clear`
  only reset the Plan panel; the Todos checklist persisted across
  clears. Thanks **@Giggitycountless**.
- **Drag-select past the viewport edge auto-scrolls (#1163, #1255,
  #1292, #1298)** — when the mouse drag reaches the top or bottom of
  the transcript area the viewport now scrolls to follow the
  selection, the way text editors do. **Copy strips every visual-only
  decoration glyph** — tool-card rails (`╭│╰`), transcript rails
  (`▏`), reasoning rails (`╎`), tool-status symbols (`·•◦`), and
  tool-family glyphs no longer leak into clipboard output. Thanks
  **@Oliver-ZPLiu**.
- MCP stdio servers no longer discard stderr. The spawn site now pipes
  stderr through a bounded ring buffer; when a server crashes
  mid-session, the transport-closed error includes the captured stderr
  tail instead of disappearing into `Stdio::null`. Useful for debugging
  Node/Python MCP servers that fail well after `initialize`.
- Mouse capture now defaults on inside Windows Terminal (#1169, #1298,
  #1331). When `WT_SESSION` is set, in-app text selection is enabled
  by default and the wheel scrolls the transcript again (rather than
  the terminal interpreting wheel events as input-history keys).
  Legacy conhost stays opt-in via `--mouse-capture` or `[tui]
  mouse_capture = true` to preserve the protections from #878 / #898.
  Selection now clamps to the transcript region instead of the
  terminal painting native selection across the sidebar.
- The build script now invalidates its cache on `.git/HEAD` changes, so
  the embedded short-SHA in `deepseek --version` stays current after
  commits and branch switches without needing `cargo clean`. Both
  regular checkouts and `git worktree` layouts are handled.
- The release-time `changelog_entry_exists_for_current_package_version`
  gate walks up from the crate manifest to find `CHANGELOG.md` instead
  of assuming a fixed `../../CHANGELOG.md` layout. The workspace path
  still resolves; running the suite from a packaged crate skips the
  gate quietly instead of panicking.

## [0.8.25] - 2026-05-09

A stabilization + drift-fixes release. Headline work hardens the
self-update path (no more `curl` shellout, real SHA-256 verification),
fixes long-cell truncation in markdown tables, centralizes the MCP
JSON-RPC framing, and unifies terminal-mode recovery on focus events.
Big thanks to **Reid Liu (@reidliu41)** (Streamable HTTP MCP transport,
`/config` column alignment), **Duducoco (@Duducoco)** (cache-stable
`reasoning_content` replay), **jinpengxuan (@jinpengxuan)** (provider
credentials during onboarding), **heloanc (@heloanc)** (Home/End cursor
keys), **Wenjunyun123 (@Wenjunyun123)** (docs anchor scroll), and
**Liu-Vince (@Liu-Vince)** (zh-Hans approval-dialog wording) for the
contributions below.

### Added

- **Streamable HTTP MCP endpoints with SSE fallback (#1300)** — adds
  the third MCP transport alongside stdio and SSE. The new transport
  posts JSON-RPC over plain HTTP with optional SSE upgrade for servers
  that prefer streaming responses. Thanks **Reid Liu (@reidliu41)**.
- **`recall_archive` exposed in the parent agent registry** — the
  read-only BM25 archive search tool was previously only available to
  sub-agents; it is now callable from Plan, Agent, and YOLO parent
  registries. Plan mode's read-only contract is preserved (the existing
  registry test was updated to assert membership while still rejecting
  write/exec tools).

### Changed

- **Markdown tables wrap long cells instead of truncating (#1163-adjacent)**
  — long cell content is word-wrapped within the column instead of
  collapsing to `…`. Column separators are preserved on every wrapped
  line so the table grid stays readable.
- **MCP JSON-RPC framing centralized** — request/response correlation,
  timeout handling, and message framing now live above the byte-level
  transports. Stdio, SSE, and the new Streamable HTTP transport share a
  single protocol layer instead of each maintaining its own copy of the
  framing code.
- **Self-update is curl-free and verifies SHA-256** — `deepseek update`
  no longer shells out to system `curl` (and no longer needs the
  Schannel `--ssl-no-revoke` Windows hack from v0.8.23). Downloads now
  use `reqwest::blocking` with rustls, and the aggregated
  `deepseek-artifacts-sha256.txt` manifest is parsed and checked
  against each downloaded asset before it is installed. Verification
  status is surfaced in the update output.
- **Terminal-mode recovery unified in `recover_terminal_modes()`** —
  startup, `FocusGained`, and `resume_terminal` all route through one
  idempotent helper that re-establishes keyboard enhancement flags,
  mouse capture, bracketed paste, and focus events. Adding a new mode
  flag now only has to happen in one place.

### Fixed

- **`reasoning_content` replay stable for prompt cache (#1297)** —
  reasoning text replayed from saved sessions now hashes consistently
  across turns so the cache-aware prompt builder's static-prefix
  stability isn't broken by replays. Thanks **Duducoco (@Duducoco)**.
- **Active provider credentials respected during onboarding (#1265)**
  — the onboarding flow now reads credentials from the active provider
  instead of falling back to the default DeepSeek path when another
  provider is selected. Thanks **jinpengxuan (@jinpengxuan)**.
- **Home/End keys move the input cursor (#1246)** — Home and End now
  jump the composer cursor to line start/end instead of being
  swallowed. Thanks **heloanc (@heloanc)**.
- **Docs anchor scroll-margin overrideable (#1282)** — the
  scroll-margin offset on docs anchors is now overrideable so embedded
  contexts can adjust it without forking the stylesheet. Thanks
  **Wenjunyun123 (@Wenjunyun123)**.
- **`/config` view columns aligned (#1290)** — the `/config` table now
  sizes the key column from the actual data instead of a fixed width,
  so long keys no longer overflow into the value column. Thanks
  **Reid Liu (@reidliu41)**.
- **zh-Hans approval dialog wording (#1274)** — uses 终止 (terminate)
  instead of 中止 (abort) in the Chinese approval dialog, matching the
  English semantics. Thanks **Liu-Vince (@Liu-Vince)**.

### Removed

- **Unwired `[context.per_model]` config field** — the field had no
  runtime consumer and was only present in the config schema. Removed
  to keep the schema honest. Existing configs that still contain a
  `[context.per_model.*]` table continue to load (serde ignores
  unknown keys; covered by a regression test).
- **Stale aspirational `[cycle.per_model]` comments** — reference to a
  config table that was never wired. No behavior change.

### Documentation

- **`.claude/CODEMAP_v0.8.25_dead_code.md`** — committed the
  cycle/seam/coherence/capacity codemap with a softened
  `cycle_manager` classification: live by code trace, design
  load-bearing, practical load-bearing unproven. Use this to decide
  the v0.8.26+ product direction for the cycle/seam/capacity
  subsystems.

### Known issues

- **Windows 10 conhost flicker regression (#1260, #1251)** —
  v0.8.22-and-later content flickering on Windows 10 is still present.
  The viewport-reset escape sequence added in v0.8.22 needs a Windows
  guard. Deferred to v0.8.26.
- **Snapshot system still snapshots every turn** — the v0.8.24 500 MB
  hard cap protects against blowups, but the underlying design still
  snapshots on every turn regardless of whether the workspace changed.
  A write-aware skip is planned for v0.8.26.
- **`▏` glyph leak in code blocks (#1212)**, **mouse selection
  crossing the sidebar (#1169)**, **drag-select edge auto-scroll
  (#1163)**, **mid-run MCP server stderr capture** — all deferred to
  v0.8.26.

## [0.8.24] - 2026-05-09

A bugfix + refactor release picking up the backlog after the v0.8.23 security
release. Big thanks to **wplll** (cache-aware prompt + `/cache inspect`),
**Liu-Vince** (MCP pagination diagnosis), **@Giggitycountless** (snapshot cap
proposal), and to issue reporters **@SamhandsomeLee**,
**@barjatiyasaurabh**, **@tyculw**, **@hongyuatcufe**, and **@ljlbit** for
the bugs fixed below.

### Fixed

- **Mouse-wheel scroll survives focus toggles** — on macOS, switching away
  (Cmd+Tab, opening the screenshot tool, etc.) and back can drop the
  terminal's mouse-tracking mode, leaving wheel scroll dead until restart.
  The TUI now re-arms `EnableMouseCapture` on `FocusGained` alongside the
  existing keyboard-mode recapture, so wheel events keep flowing after a
  focus round-trip.
- **Workspace-local slash commands are now loaded (#1259)** — user command
  files placed in `<workspace>/.deepseek/commands/`,
  `<workspace>/.claude/commands/`, and `<workspace>/.cursor/commands/` are
  now discovered alongside the existing global `~/.deepseek/commands/`.
  Workspace-local commands shadow global by name, matching the precedence
  model already used for skills. Reported by **@SamhandsomeLee**.
- **`@`-mention completion finds AI-tool dot-directories** — files inside
  `.deepseek/`, `.cursor/`, `.claude/`, and `.agents/` are now discoverable
  in `@`-mention Tab-completion even when those directories are excluded by
  `.gitignore`. The fix also applies to the Ctrl+P file picker and fuzzy
  file resolution.
- **MCP paginated discovery (#1250, #1256)** — tools, resources, resource
  templates, and prompts from MCP servers that paginate their responses
  (e.g., gbrain at 5 items per page) are now fully discovered by following
  the MCP spec's `nextCursor` across all pages. Reported by
  **@hongyuatcufe**; thanks to **Liu-Vince** for the diagnosis and PR
  #1256 with the same fix shape.
- **Snapshot storage has a disk-space cap (#1112)** — the snapshot side repo
  now enforces a 500 MB hard limit. When the limit is exceeded at snapshot
  time, the oldest snapshots are pruned aggressively to stay under a 400 MB
  target. Guards against the reported 1.2 TB snapshot blowup during
  high-churn sessions. Reported by **@tyculw**; thanks to
  **@Giggitycountless** for the PR #1131 proposal that informed the
  hard-cap approach.
- **`/clear` now resets the Todos sidebar (#1258)** — previously `/clear`
  only reset the Plan panel; the Todos checklist persisted across clears
  until app restart. The fix ensures `clear_todos()` clears the
  `SharedTodoList` inner state. Reported by **@barjatiyasaurabh**.

### Added

- **Cache-aware prompt diagnostics + payload optimization (#1196)** — adds
  a `PromptBuilder` that classifies the system prompt into `static` /
  `history` / `dynamic` layers for cache-prefix stability, plus:
  - `/cache inspect` — shows SHA-256 hashes per layer, base static prefix
    hash vs full request prefix hash, static-prefix stability across
    turns, and first-divergence tracking. Does not print prompt text.
  - `/cache warmup` — prefetches the stable prefix to seed the DeepSeek
    context cache.
  - **Project Context Pack injected into the stable prefix by default**
    — a structured workspace summary (directory listing up to 4 levels /
    400 entries, README excerpt up to 4 KB, config + key source file
    lists). Adds **~1–10 KB to every prompt depending on repo size**, in
    exchange for a much more cacheable prefix. **Default ON**; disable
    with `[context] project_pack = false` in `~/.deepseek/config.toml`
    if you'd rather keep prompts minimal.
  - Wire-payload optimization: large tool outputs are budgeted, repeated
    identical tool outputs and `<turn_meta>` blocks are deduplicated
    with stable refs (wire-only — local session messages stay intact).
  - Footer cache-hit % chip from `prompt_cache_hit_tokens` /
    `prompt_cache_miss_tokens` in the API response.
  
  Thanks **wplll** for the design and implementation.

### Changed

- **Language directive strengthened against project-context bias (#1118)**
  — the system prompt now explicitly instructs the model that project
  context (AGENTS.md, auto-generated instructions, file trees) is NOT a
  language signal. Chinese filenames in a repo no longer bias the model
  toward Chinese replies when the user writes in English. Reported by
  **@ljlbit**.

### Known issues

- **Windows flicker/shake regression (#1260, #1251)** — v0.8.22 and v0.8.23
  exhibit content flickering on Windows 10 (v0.8.20 works correctly). The
  issue is likely caused by the viewport-reset escape sequence
  (`\x1b[r\x1b[?6l\x1b[H\x1b[2J\x1b[3J`) added in v0.8.22 to fix viewport
  drift. On Windows conhost, this sequence may trigger a full screen clear
  on every repaint. A platform guard or less aggressive sequence is needed.

## [0.8.23] - 2026-05-08

A security-focused follow-up to v0.8.22. The bulk of the diff is hardening of
the child-process surface — shells, MCP stdio servers, and other spawned
subprocesses — plus a related set of MCP, secret-store, and tool-policy
fixes uncovered during follow-up review.

### Security

- **Sanitized child-process environments** - shells, MCP stdio servers, hooks,
  and other child processes spawned from the TUI now start from an explicit
  allowlist of parent environment variables rather than inheriting every
  parent var. The base allowlist covers `PATH`, `HOME`, `USER`, `LANG`/`LC_*`,
  `TERM`/`COLORTERM`, `SHELL`, `TMPDIR`/`TMP`/`TEMP`, and the corresponding
  Windows variables. Stops casual exfiltration of `*_API_KEY`, `AWS_*`,
  `GITHUB_TOKEN`, and similar through a spawned subprocess.
- **Tighter shell safety classification** - the `exec_shell` deny-list was
  reviewed and broadened to cover additional dangerous command patterns.
- **Plan mode tool surface narrowed** - planning sub-agents see a smaller,
  read-only tool surface so a plan-mode call can no longer mutate workspace
  state.
- **Sub-agent approval boundaries preserved** - sub-agents inherit the
  parent's approval policy and cannot escalate beyond it.
- **Symlinked workspace walks no longer followed** - workspace-relative
  walkers (file-search, project context) now refuse to traverse symlinks
  pointing outside the workspace root.
- **Path and output handling tightened** - several tools that build paths
  from model output now reject `..` segments and absolute paths outside the
  workspace.
- **Runtime API requires authentication by default** - `deepseek serve --http`
  no longer accepts unauthenticated requests in its default configuration.
- **Security-sensitive dependencies bumped** - routine bump pass for crates
  with recent advisories.
- **MCP config paths reject traversal** - `load_config`/`save_config` now
  refuse paths containing `..` components.
- **Hardened `run_tests` approval policy.** Thanks to **@47Cid** for the
  responsible disclosure.

### Fixed

- **macOS Keychain prompt at startup** - the file-backed secret store is now
  the default. The OS keyring is opt-in via
  `DEEPSEEK_SECRET_BACKEND=system|keyring`, and the auth status surface
  refers to "secret store" rather than "keyring" where appropriate.
- **MCP stdio spawn errors are now visible (#1244)** - when spawning a stdio
  MCP server fails (e.g., `npx` not on `PATH`), the underlying OS error is
  now shown ("No such file or directory (os error 2)") instead of the opaque
  wrapper "MCP stdio spawn failed (...)". The fix applies to the snapshot,
  the `mcp connect` / `mcp validate` CLI commands, and the in-TUI status
  events.
- **MCP servers no longer break under env scrub (#1244)** - MCP stdio launches
  now inherit a wider env allowlist than arbitrary shell tools, so common
  `npx ...`, `uvx ...`, `python -m mcp_server_*`, and proxy-bound corporate
  setups keep working under the new child-env scrub. Pass-through includes
  `NVM_DIR`, `NODE_OPTIONS`, `NODE_PATH`, `NODE_EXTRA_CA_CERTS`,
  `NPM_CONFIG_*`, `VOLTA_HOME`, `COREPACK_HOME`, `PYTHONPATH`, `PYTHONHOME`,
  `VIRTUAL_ENV`, `PIPX_*`, `POETRY_HOME`, `UV_*`, `GEM_*`, `BUNDLE_*`,
  `JAVA_HOME`, `HTTP_PROXY` / `HTTPS_PROXY` / `NO_PROXY` / `ALL_PROXY` /
  `FTP_PROXY` (case-insensitive), `SSL_CERT_FILE`, `SSL_CERT_DIR`,
  `REQUESTS_CA_BUNDLE`, `CURL_CA_BUNDLE`. Secret-bearing parent env stays
  scrubbed.

### Changed

- **Live thinking is compact by default** - the streaming "thinking" panel
  collapses by default; expand via the existing details toggle.

### Added

- **`docs/RELEASE_CHECKLIST.md`** - explicit pre-tag checklist (CHANGELOG,
  versions, preflight, npm wrapper smoke) so the v0.8.21/v0.8.22 CHANGELOG
  gap does not recur.

### Known issues

- **Mid-run MCP server stderr is still suppressed** - if a stdio MCP server
  spawns successfully but exits later (e.g., crashes during `initialize`),
  its stderr is not yet captured. Spawn-time OS errors (the most common
  case from #1244) are visible. Full mid-run stderr capture is planned for
  v0.8.24.

## [0.8.22] - 2026-05-08

A focused security release.

### Security

- **Hardened `fetch_url` redirect handling.** Thanks to **@47Cid** for the
  responsible disclosure.

## [0.8.21] - 2026-05-08

A community-heavy release rolling up two weeks of contributor PRs across the
TUI, runtime, and docs. Big thanks to **Reid (@reidliu41)**,
**jiaren wang (@JiarenWang)**, **Friende (@pengyou200902)**,
**ZzzPL (@Oliver-ZPLiu)**, **Sun**, **Liu-Vince**, **kitty**, and
**Aqil Aziz** for the contributions below.

### Added

- **Distinct user-message body color** (#1168) - user turns now render in a
  green body color so the conversation flow is easier to scan at a glance.

### Fixed

- **Plan mode enforces read-only tool boundaries** (#1114) - planning calls
  can no longer reach into write-side tools. Thanks **jiaren wang**.
- **Composer arrow keys navigate input history** (#1117) - up/down in the
  composer cycles through prior prompts when the cursor is on the first/last
  line. Thanks **Reid**.
- **RLM preserves prompt cache usage** (#1127) - the RLM batch path no longer
  resets prompt-cache hits between calls. Thanks **Sun**.
- **`fetch_url` proxy DNS opt-in** (#1103) - the proxy DNS path is now opt-in
  rather than always forced, fixing breakage in environments where the proxy
  cannot resolve the target host. Thanks **Sun**.
- **Undo syncs session context after snapshot restore** (#1150, fixes #1139) -
  rolling back a turn now correctly resyncs the in-memory session so a
  follow-up turn doesn't see stale context. Thanks **jiaren wang**.
- **Stale busy-state watchdog** (#1170) - the TUI now recovers if the busy
  indicator gets stuck after an aborted turn. Thanks **ZzzPL**.
- **`gh` discovered across common install paths** - the `gh` tool is found
  whether installed via Homebrew, apt, the Windows MSI, or the GitHub CLI
  installer. Thanks **kitty**.
- **Code block indentation preserved in transcript** - leading whitespace
  inside fenced code blocks is no longer collapsed during rendering.
  Thanks **Liu-Vince**.
- **Stream pacing preserves upstream cadence** - long streaming responses
  no longer chunk together when the upstream is bursty.
  Thanks **Sun**.
- **Task list output gets headers** - the long-form `/tasks` output now has
  group headers so it scans cleanly. Thanks **Reid**.
- **macOS option-V details shortcut** - the details toggle now works correctly
  on US Mac keyboards where Option+V produces `√`.
- **Uppercase approval shortcuts accepted** - `[A]/[D]/[V]` work in either
  case in the approval dialog.
- **Transcript scrollbar inert** - the transcript scrollbar no longer captures
  clicks intended for content below it.
- **Hide transcript rail before code blocks** - the rail glyph no longer
  bleeds onto the line just above a fenced code block.
- **Pager exit hint prominent** - the "press q to exit" hint is now visible
  on the pager footer.
- **Empty tool call names fall back to a placeholder** - a model that returns
  an empty `function.name` in a tool call no longer hangs the turn.
- **MCP SSE waits for endpoint before connect returns** (#1225) - the SSE
  transport no longer reports "connected" before the endpoint event has been
  received, fixing a race where the first request was lost.
- **Git branch status item renders** (#1226, fixes #1217) - the
  `StatusItem::GitBranch` toggle now produces a footer entry instead of a
  blank slot.
- **Beta endpoint routes non-beta paths to v1** (#1174) - paths that aren't
  available on the DeepSeek beta host are transparently redirected to the v1
  host instead of failing.
- **Skill packs accept workflow-pack archive layouts** (#1164) - skill
  archives produced by the workflow pack tool now install correctly.
- **Interactive sessions stay in alternate screen** (#1158) - returning from
  a sub-process no longer kicks the TUI back to the primary screen mid-turn.
- **Slash-menu arrow navigation wraps** (#1152) - up at the top / down at the
  bottom of the slash menu wraps to the other end.
- **CLI preserves split prompt words from Windows shims** (#1160) - prompt
  arguments forwarded by the npm wrapper on Windows are no longer joined into
  one giant token.
- **`libc` extended to all Unix targets** (#1173) - improves FreeBSD build
  compatibility.
- **Memory truncation marker reports omitted bytes** - the `[…N bytes
  omitted]` marker now shows an accurate count. Thanks **Friende**.

### Docs

- **Memory skill link** (#1096) - corrected. Thanks **Aqil Aziz**.
- **Help keybinding reference** (#1095) - corrected. Thanks **Friende**.
- **Additional environment variables** documented in the config reference.
  Thanks **Liu-Vince**.
- **Docker volume guidance** - the install snippet now uses a writable named
  data volume rather than a bind mount that may be read-only on some hosts.
- **Competitive analysis reflects LSP diagnostics** (#1171) - the doc now
  matches the shipping LSP diagnostics implementation.
- **Dispatcher path for `/run-pr`** (#1227) - the README now points at the
  dispatcher binary.

## [0.8.20] - 2026-05-08

### Added
- **Global AGENTS.md fallback** - when a workspace and its parents do not
  provide project instructions, DeepSeek TUI now loads `~/.deepseek/AGENTS.md`
  before falling back to auto-generated `.deepseek/instructions.md`, keeping
  repo-local instructions higher priority while supporting shared defaults.

### Fixed
- **Chinese reasoning stays Chinese** - restore the #588 language contract after
  the deterministic environment prompt regressed it. The latest user message now
  chooses the natural language for both `reasoning_content` and the final reply;
  the resolved `lang` field is only a fallback when the user turn is ambiguous.

## [0.8.19] - 2026-05-08

### Fixed
- **DeepSeek beta endpoint stays default for Chinese locales** - the legacy
  `deepseek-cn` runtime path no longer routes users to the non-beta
  `https://api.deepseek.com` base URL. It is now a backwards-compatible alias
  for the normal `deepseek` provider default, `https://api.deepseek.com/beta`,
  so strict tool mode and other beta-gated features stay available worldwide.
- **Provider docs stop advertising `deepseek-cn` as a separate provider** -
  runtime docs now describe it only as a legacy config alias. DeepSeek uses the
  same official host worldwide; users with private mirrors should set
  `base_url` explicitly.

## [0.8.18] - 2026-05-07

This is the v0.8.17 follow-up release: a tighter TUI/runtime/install pass with
safer session startup semantics, Docker images promoted to a supported install
path, and several community PRs harvested into the release branch. VS Code and
Feishu/Lark/mobile companion work remain out of scope for this release.

### Added
- **Prebuilt Docker images on GHCR** - release builds now publish
  `ghcr.io/hmbown/deepseek-tui` with `latest`, semver, and `vX.Y.Z` tags, and
  the GitHub release notes include a Docker install snippet. Docker publishing
  is now a release gate rather than a best-effort check.
- **Draggable transcript scrollbar** (#1075, #1076) - when mouse capture is
  enabled, drag the transcript scrollbar thumb to move through long sessions.
  The implementation also clears stale drag state on resize and new left-clicks.
  Thanks @Oliver-ZPLiu.
- **PTY regression for viewport drift** (#1085) - the QA harness now covers the
  blank-top-rows failure after a failed/long turn so future layout changes catch
  terminal viewport drift.

### Changed
- **Plain `deepseek` starts a fresh session** - opening a second `deepseek` in
  the same folder no longer silently attaches to the same in-flight checkpoint.
  Crash/interrupted checkpoints are preserved as saved sessions and recovered
  explicitly through `deepseek --continue`.
- **npm postinstall is recoverable for transient download failures** (#1059) -
  install-time GitHub download/extract failures are non-blocking and documented,
  while unsupported platforms, checksum mismatches, glibc preflight failures,
  and runtime wrapper failures remain fatal. Thanks @Fire-dtx.
- **Docker Buildx cargo caches are platform-isolated and locked** - registry,
  git, and target caches now use platform-specific cache IDs plus locked
  sharing to avoid the `.cargo-ok File exists` unpack race in release checks.
- **Long-session palette is easier to read** (#1070, #936 partial) - default
  body text is slightly softer, reasoning/thinking text uses a warmer accent,
  and `/theme` now updates the terminal color adapter so light mode keeps those
  contrasts coherent after an in-session toggle. Thanks @bevis-wong and
  @oooyuy92 for the readability reports.
- **Install docs add a second rustup mirror fallback** (#1011) - `rsproxy.cn`
  is documented as an alternate rustup mirror, and old Debian/Ubuntu Cargo
  `edition2024` failures now point users to rustup stable. Thanks @wuwuzhijing.

### Fixed
- **Chinese destructive approval dialogs keep explicit risk wording** (#1087,
  #1091) - zh-Hans destructive approval copy now localizes the operation label,
  title, prompt, and destructive-risk warning without changing English default
  behavior. Thanks @qinxianyuzou and @axobase001.
- **Terminal viewport is reset before repaint** (#1085) - the TUI now clears
  scroll margins/origin mode before key repaints after resume, resize, and turn
  completion, preventing alt-screen content from drifting downward and leaving
  blank rows at the top.
- **Interactive subprocesses wait for terminal release** (#1085) - shell/editor
  handoff now waits until the UI has actually left alt-screen/raw mode before
  launching the child process, preventing the TUI from repainting into host
  scrollback after interactive tool use.
- **Light theme reasoning blocks stay light** (#1070, #936 partial) -
  thinking/reasoning background tints now map to the light reasoning surface
  instead of keeping the dark-mode tint after `/theme light`.
- **FreeBSD can compile the secrets crate** (#1089) - platforms without a native
  `keyring` dependency now fail the OS-keyring probe cleanly and fall back to
  the file-backed secret store instead of referencing a missing crate. Thanks
  @avysk for the FreeBSD report.
- **Windows sandbox docs no longer overstate guarantees** (#1015, #1058) - the
  docs and code comments now describe the future Windows helper as
  process-tree containment only until filesystem, network, registry, or
  AppContainer isolation is actually implemented. Thanks @axobase001.

## [0.8.17] - 2026-05-07

A focused reliability release built almost entirely from community contributions.
Fixes Plan-mode safety, paste-Enter auto-submit, slash-menu skills coverage, the
`deepseek-cn` endpoint preset, and a handful of platform / streaming /
gateway-compatibility issues. Also lands a small PTY-driven QA harness so the
next round of TUI fixes can be verified against real terminal behaviour.

### Added
- **`/theme` command** (#1057) — toggle between dark and light themes inline,
  without round-tripping through `/config`. Thanks @MengZ-super.
- **PTY/frame-capture TUI QA harness** — new
  `crates/tui/tests/support/qa_harness/` lets integration tests spawn
  `deepseek-tui` in a real pseudo-terminal, send scripted keys / paste /
  resize, and assert on the parsed terminal frame plus the workspace
  filesystem. Initial scenarios cover boot smoke and the #1073 paste regression.
  Adding-a-scenario walkthrough lives in `crates/tui/tests/support/qa_harness/README.md`.
- **Whalescale desktop runtime bridge** — the local runtime API now exposes
  `POST /v1/approvals/{id}`, `GET /v1/runtime/info`, `enabled` flags on
  `GET /v1/skills`, and `POST /v1/skills/{name}` toggles. Runtime thread
  events also carry `agent_reasoning` items so desktop clients can render
  thinking separately from assistant text.

### Changed
- **`deepseek-cn` provider preset now defaults to the official
  `https://api.deepseek.com` host** (#1079, #1084) — matches
  [api-docs.deepseek.com](https://api-docs.deepseek.com/). The legacy typo
  host `api.deepseeki.com` is still recognized in URL heuristics and chat-client
  normalization so existing user configs keep working. Thanks @Jefsky.
- **Plan mode runs shell commands in a read-only sandbox** (#1077) — was
  `WorkspaceWrite` with the workspace as a writable root, which let
  `python -c "open('f','w').write('x')"` mutate files inside the workspace.
  Now `SandboxPolicy::ReadOnly`: no writes anywhere on the filesystem, no
  network. Read-only inspection commands (`ls`, `git log`, `grep`,
  `cargo metadata`, …) keep working through the per-platform sandbox; for
  anything that creates or modifies files, switch to Agent mode (`/agent`).
  Thanks @DI-HUO-MING-YI.

### Fixed
- **Pasting multi-line text with a trailing newline no longer auto-submits**
  (#1073) — the composer's Enter handler now consults the paste-burst
  suppression state and either appends `\n` to the in-flight burst buffer or
  inserts it into the composer text directly, instead of falling through to
  `submit_input()`. Reproduced from the original Windows / PowerShell
  symptom; fix covers both the bracketed-paste and rapid-keystroke detection
  paths. Thanks @bevis-wong for the precise reproducer.
- **Slash menu, `/skills`, and `/skill <name>` show project-local AND global
  skills** (#1068, #1083) — switched the cache to `discover_in_workspace`, so
  the UI surfaces stay in sync with the system-prompt skills block. Bonus
  fix: `SKILL.md` frontmatter values are now stripped of surrounding YAML
  quotes, so `name: "hud"` registers as `hud` and matches prefix lookup.
  Thanks @AlphaGogoo / @Duducoco.
- **Windows shell output is decoded as UTF-8 even on non-UTF-8 system code
  pages** (#982, #1018) — Windows shell commands are now wrapped with
  `chcp 65001 >NUL & ` so subprocesses output UTF-8 instead of GBK / other
  ANSI code pages. `display_command` strips the prefix so transcripts and
  approval prompts stay clean. Thanks @chnjames.
- **Stale snapshot `tmp_pack_*` files are cleaned up on startup** (#975,
  #1055) — interrupted side-repo git pack operations no longer leak orphaned
  temp files; `prune_unreachable_objects` runs during the regular prune
  cycle to drop loose objects from rolled-back snapshots. Closes the
  ~30 GB+ disk-usage report. Thanks @axobase001.
- **Window-resize artifacts on macOS Terminal.app and Windows ConHost are
  gone** (#993) — forces the resize-event size during the post-resize draw
  so ratatui's internal `autoresize()` cannot shrink the viewport back to a
  stale dimension and leave the newly-expanded area filled with stale
  content. Same class as #582 for additional emulator families. Thanks
  @ArronAI007.
- **Streaming thinking blocks finalize cleanly on stream errors and
  restarts** (#861 partial, #1078) — the engine-error handler now drains
  the in-flight thinking block into the transcript instead of leaving the
  partial reasoning orphaned in `StreamingState`. Refactor extracts the
  thinking lifecycle into named helpers (`start_streaming_thinking_block`,
  `finalize_current_streaming_thinking`, `stash_reasoning_buffer_into_last_reasoning`).
  Thanks @reidliu41.
- **OpenRouter and other custom-endpoint providers preserve explicit model
  IDs** (#1066) — when a provider has an explicit model AND a custom
  `base_url` (different from the provider default), the model name is no
  longer rewritten by provider-specific normalization. Lets OpenAI-compatible
  gateways accept bare IDs like `deepseek/deepseek-v4-pro`,
  `accounts/fireworks/models/...`, or `glm-5`. Thanks @THINKER-ONLY.
- **Auto-generated `.deepseek/instructions.md` stabilizes the KV prefix
  cache** (#1080) — replaces the per-turn filesystem-scan fallback in
  `prompts.rs` with a real on-disk artifact when no context file exists, so
  the system prompt's prefix stays byte-stable across turns and prefix-cache
  hit-rate improves. The auto-generated file is plainly labelled and the
  user can edit or delete it freely. Thanks @lloydzhou.
- **SSE responses behind compressing gateways decode correctly** (#1061) —
  enables reqwest's `gzip` and `brotli` features so streams through proxies
  that compress the response come through clean instead of as protocol
  corruption. Quiets one of the failure modes behind some "stuck working"
  reports. Thanks @MengZ-super.
- **NVIDIA NIM provider configs use their own API key even when a legacy
  root DeepSeek key is present** (#1081) — `[providers.nvidia_nim] api_key`
  now wins for NIM requests, avoiding 401s caused by accidentally sending the
  top-level DeepSeek credential to NVIDIA. Thanks @wlon for the focused
  diagnosis.
- **npm installs explain the release-mirror escape hatch when GitHub Releases
  are blocked** (#1051, #1056) — network/DNS failures now point at the
  existing `DEEPSEEK_TUI_RELEASE_BASE_URL` override and the required checksum
  manifest / binary layout instead of stopping at a raw `ENOTFOUND github.com`.
  Thanks @axobase001.

### Notes for contributors

This release shifts the project's PR-handling philosophy: every contribution
has value somewhere; the maintainer's job is to find it, use it, and credit
the contributor — never to close a PR with nothing taken. If a PR is too
large or scope-mixed to merge whole, useful commits / files / ideas are
harvested directly rather than asking the contributor to split it. Trust
boundary on credentials, sandbox, providers, publishing, telemetry,
sponsorship, branding, and global prompts still requires explicit
maintainer sign-off, but the burden of getting there is on us. See
`AGENTS.md` for the full text.

## [0.8.16] - 2026-05-07

A focused hotfix for v0.8.15 regressions in RLM, sub-agent visibility, and
terminal ownership. This release keeps the v0.8.15 feature set intact while
making long-running delegated work easier to inspect and safer to run.

### Changed
- **RLM has no fixed 180s wall-clock timeout** (#955) — RLM turns can continue
  past the old hard limit when the long-input REPL is still making progress.
- **RLM output is easier to audit** (#955) — final reports now include compact
  execution metadata: input size, iteration count, elapsed time, sub-LLM RPC
  count, and termination state.
- **RLM chunking guidance is stricter for exact work** (#955) — prompts now
  tell the sub-agent to use deterministic Python over the full `context` for
  counts/aggregation and to report chunk coverage when splitting a whole input.
- **Tool guidance is less defensive** (#955) — the system prompt now explains
  when to use tools instead of discouraging the model from using capabilities
  that are actually available.

### Fixed
- **Active RLM work stays visible** (#955) — foreground RLM calls surface in the
  active task/right-rail state instead of leaving the Tasks panel saying
  `No active tasks`.
- **`/subagents` no longer reports false emptiness** (#955) — the sub-agent
  overlay now includes live progress-only agents and transcript fanout workers
  when the manager cache has not refreshed yet.
- **Sub-agent cards are quieter and more useful** (#955) — low-signal scheduler
  lines such as `step 1/100: requesting model response` are hidden, while
  compact tool activity remains visible.
- **Sub-agent completion protocol stays internal** (#955) — completion
  sentinels are routed as internal runtime events instead of user messages, so
  the parent agent does not explain raw protocol XML back to the user.
- **Sub-agents cannot take over the parent terminal** (#955) — background
  agents reject `exec_shell` with `interactive=true`; they can still use
  non-interactive shell, background shell, `tty=true`, and task-shell tools.
- **Terminal scrollback ownership is restored** (#955) — the TUI re-enters
  alternate-screen mode after foreground/sub-agent work drains, preventing the
  host terminal scrollbar from taking over the live interface.

## [0.8.15] - 2026-05-06

An auth, Windows, editor-integration, and setup stabilization release. This
release keeps the existing DeepSeek V4 architecture intact while landing small
community fixes that make first-run setup, terminal behavior, skills, cost
display, and recovery paths easier to trust.

### Added
- **ACP stdio adapter for Zed/custom agents** (#782) — `deepseek serve --acp`
  starts a local Agent Client Protocol server over stdio. The first slice
  supports new sessions and prompt responses through the user's existing
  DeepSeek config/API key; tool-backed editing and checkpoint replay remain
  outside the ACP surface for now.
- **Yuan/CNY cost display** (#806) — `cost_currency = "cny"` (also accepts
  `yuan` / `rmb`) switches footer, context panel, `/cost`, `/tokens`, and
  long-turn notification summaries from USD to CNY.
- **Slash autocomplete for skills** (#808) — installed skills are visible in
  the slash-command autocomplete menu.
- **`/rename` session titles** (#836) — sessions can be renamed without
  editing save files manually.

### Changed
- **Current local date in turn metadata** (#893, closes #865) — real user turns
  now include the current local date in `<turn_meta>`, without changing the
  stable system prompt/cache prefix.
- **Doctor endpoint diagnostics** (#823) — `deepseek doctor` shows the resolved
  provider/API endpoint to make proxy, China endpoint, and inherited-env
  debugging more concrete.
- **More conservative request sizing** (#826) — API requests cap `max_tokens`
  against the active model/context budget before dispatch.
- **Safer config and secret file writes** (#833, #837) — generated config files
  use restrictive permissions and improved secret redaction.

### Fixed
- **Env-only API key failure recovery** (#892) — runtime auth failures now say
  when the rejected key came from inherited `DEEPSEEK_API_KEY` and no saved
  config key is present, matching the clearer `deepseek doctor` guidance.
- **Windows Unicode output** (#887, closes #872) — TUI startup now best-effort
  switches the Windows console input/output codepages to UTF-8, improving
  Chinese and other non-ASCII rendering.
- **Windows resume picker** (#886, closes #866) — the dispatcher keeps the
  resume picker path on Windows instead of bypassing it.
- **Windows clipboard fallback** (#850) — copy operations have a fallback path
  when the primary clipboard backend is unavailable.
- **Workspace trust persistence** (#870) — approval/trust choices persist in
  global config instead of surprising users on the next launch.
- **Ctrl+E composer behavior** (#883, closes #876) — plain Ctrl+E moves to the
  end of the composer again; file-tree toggling moved to the shifted shortcut.
- **Plain Markdown skills** (#869) — `SKILL.md` files without frontmatter now
  fall back to the first `# Heading` instead of being ignored.
- **Workspace-scoped latest resume** (#830, closes #779) — `resume --last`,
  `--continue`, and fork/resume helpers choose the latest session for the
  current workspace/repo rather than the newest saved session globally.
- **Npm wrapper version fallback** (#885) — `deepseek --version` / `-v` can
  report the package version when the native binary has not been downloaded
  yet.
- **TUI exit resume hint** (#863, closes #682) — exiting the TUI now points
  users toward the relevant resume command.
- **Startup and terminal reliability** — includes bounded stream-open waits
  (#847), cursor-lag reduction for `@` mentions (#849), OSC52 clipboard fallback
  for SSH (#845), legacy Ctrl+V paste recognition (#786), Windows mouse capture
  defaulting off (#785), and UTF-8-preserving ANSI stripping (#784).
- **Install and policy reliability** — avoids unstable Rust file-locking APIs
  (#821), enforces network policy in `web_run` (#800), fixes repeated setup
  language prompts after API-key setup (#844), and explains dispatcher TUI spawn
  failures (#853).
- **Workspace safety** — refuses dangerous snapshots for `$HOME` or unsafe
  workspaces (#798, #804), fixes path-escape false positives for double-dots in
  names (#824), scopes snapshot built-in excludes (#854), and replaces provider
  `unreachable!()` paths with proper errors (#835).
- **Skills discovery** — recursively reads the skills directory (#811), ignores
  symlinks outside the selected install root (#814), discovers global Agents
  skills (#848), and includes `.cursor/skills` (#817).
- **Provider/model compatibility** — restores auto model routing (#772),
  completes vLLM provider integration (#737), accepts provider-prefixed DeepSeek
  model IDs (#794), preserves requested model ID casing (#733), and pins RLM
  child calls to Flash (#832).

### Thanks
- Thanks to [@reidliu41](https://github.com/reidliu41) for the resume hint and
  workspace trust fixes (#863, #870).
- Thanks to [@Oliver-ZPLiu](https://github.com/Oliver-ZPLiu) for the Windows
  clipboard fallback (#850).
- Thanks to [@xieshutao](https://github.com/xieshutao) for the plain Markdown
  skill fallback (#869).
- Thanks to [@GK012](https://github.com/GK012) for the npm wrapper version
  fallback (#885).
- Thanks to everyone filing Windows, Chinese-language setup, auth, and
  first-run reports. Those concrete reproductions shaped the release.

## [0.8.13] - 2026-05-05

A stabilization release for DeepSeek V4 runtime and TUI reliability. The
v0.8.13 milestone was narrowed to direct runtime/TUI fixes; prompt hygiene,
trajectory logging, Anthropic-wire support, and larger UI cleanup were moved
out of this release.

### Added
- **No-LLM tool-result prune before compaction** (#710) — old verbose tool
  results are mechanically summarized before the paid summary pass. Duplicate
  reads keep the freshest full body and replace older copies with one-line
  summaries; if that gets the session back under the compaction threshold, the
  LLM summary call is skipped entirely.
- **Repeated-tool anti-loop guard** (#714) — the engine now tracks
  `(tool_name, args)` pairs per user turn. On the third identical call it
  inserts a synthetic corrective tool result instead of running the same tool
  again unchanged; per-tool failures warn at three and halt at eight.
- **V4 cache-hit telemetry fallback** (#721) — usage parsing now recognizes
  `usage.prompt_tokens_details.cached_tokens`, so the existing footer cache-hit
  chip works with DeepSeek V4's automatic prefix-cache telemetry as well as the
  older explicit hit/miss fields.

### Fixed
- **Invalid tool-call JSON repair** (#712) — malformed streamed tool arguments
  now pass through a deterministic repair ladder before dispatch.
- **Hallucinated tool-name recovery** (#713) — common non-canonical tool names
  are resolved through the registry before the engine reports a missing tool.
- **Tool-schema sanitation** (#715) — schemas are normalized before API
  emission so provider-strict JSON Schema handling does not reject valid tools.
- **Case-sensitive model IDs** (#717, #729) — valid configured model IDs keep
  caller-provided case while compact DeepSeek aliases still canonicalize.
- **Stale `working...` state after failed dispatch** (#738) — if the UI fails
  to send a message to the engine before a turn starts, the composer loading
  state is cleared instead of trapping later input in pending state.
- **Prompt-free doctor key checks** — `deepseek doctor` no longer reads the OS
  keyring, avoiding macOS Keychain prompts during diagnostics.
- **macOS Terminal color compatibility** — `xterm-256color` sessions now
  receive 256-color palette indexes instead of truecolor SGR, preventing
  Apple Terminal from misrendering whale blues as green/cyan blocks.
- **Chat client repair after Responses cleanup** — restored the chat client
  body and regression coverage after removing the dead experimental Responses
  fallback path.
- **Up/Down arrow transcript scroll when composer is empty** — bare Up/Down
  arrows now scroll the transcript when the composer input is empty (or
  whitespace-only); with text present they still navigate composer history.
  Previously the gate was hardcoded to false, leaving users in virtual
  terminals (Ghostty, Codex, Kitty-protocol) unable to scroll without
  modifier shortcuts.

## [0.8.11] - 2026-05-04

### Changed
- **Cache-maxing prompt path for DeepSeek V4** — the engine now skips
  system-prompt reassignment when the assembled stable prompt is unchanged,
  keeps the volatile repo working-set summary out of the system prompt, and
  injects it as per-turn metadata on the latest user message instead.
- **Tool catalog cache anchor** — the model-visible tool array now marks
  the final native tool with `cache_control: ephemeral` so DeepSeek can
  anchor the stable tool prefix explicitly.
- **V4-scale automatic compaction defaults** — automatic compaction keeps a
  500K-token hard floor and the fallback compaction threshold now reflects
  the V4-scale late-trigger policy instead of the old 50K-era default.
- **Token-only compaction trigger** — the message-count compaction trigger
  was a 128K-era heuristic that fired on long sessions of small messages
  — exactly the case where rewriting V4's prefix cache is most wasteful.
  Removed `CompactionConfig::message_threshold` and the message-count
  branch in `should_compact`; token budget is now the sole automatic
  trigger (gated by the 500K floor). Manual `/compact` is unchanged.

### Fixed
- **Legacy 128K context naming** — the 128K fallback is now named and
  documented as legacy DeepSeek-only behavior, reducing ambiguity with the
  1M-token DeepSeek V4 defaults.
- **`npm install` resilience for slow / firewalled networks** — the
  postinstall binary fetch from GitHub Releases now retries on transient
  errors (5 attempts, 1-16 s exponential backoff with jitter), enforces a
  per-attempt timeout (default 5 min, configurable via
  `DEEPSEEK_TUI_DOWNLOAD_TIMEOUT_MS`) plus a 30 s stall detector, honors
  `HTTPS_PROXY` / `HTTP_PROXY` / `NO_PROXY` env vars (pure-Node CONNECT
  tunneling, no new dependencies), and prints a download-progress line
  to stderr so users know it isn't hung. Suppressible with
  `DEEPSEEK_TUI_QUIET_INSTALL=1`. Reported by a community user from China
  whose install through a CN npm mirror took 18 minutes — the bottleneck
  was the GitHub fetch, which CN npm mirrors do not proxy.
- **YOLO sandbox dropped to DangerFullAccess** — YOLO mode was still
  routing shell commands through the WorkspaceWrite sandbox, which
  intercepted legitimate outside-workspace writes (package installs,
  sub-agent workspaces, `~/.cache`, brew, `npm install -g`, pipx) and
  forced approval round-trips — contradicting the "no guardrails"
  contract. YOLO already auto-approves all tools and enables trust mode;
  the sandbox was the last residual restriction. Now uses
  DangerFullAccess (no sandbox), consistent with the full YOLO posture.
- **Scroll position lock preserved across render resolve** — user
  scroll-up during live streaming was being yanked back to the live tail
  on the next chunk. The `user_scrolled_during_stream` lock was cleared
  prematurely when content briefly fit in one screen, or when the
  transcript shrank between renders (e.g. sub-agent card collapsed).
  Fixed by snapshotting the prior tail state before `resolve_top` and
  only clearing the lock when the user was deliberately at the bottom.
- **Capacity controller disabled by default** — the capacity controller
  was silently clearing the transcript (`messages.clear()`) based on
  slack-based `p_fail` calculations, independent of token utilization or
  the `auto_compact` setting. This contradicted the v0.8.11 default of
  `auto_compact = false` — the user opted into trusting the model with
  the full 1M-token V4 window, and the controller was auto-managing the
  prefix on their behalf. The controller now defaults to `enabled = false`;
  power users can opt in via `capacity.enabled = true`.

### Docs
- **README clarity pass** (#685) — title-cased section headings, an explicit
  Node + npm prerequisites block before the `npm install -g` snippet, a
  China-friendly `--registry=https://registry.npmmirror.com` install
  variant, a DeepWiki badge for AI-assisted repo browsing, and a 🐳 mark
  on the title. *Thanks to [@Agent-Skill-007](https://github.com/Agent-Skill-007)
  for this PR.*

## [0.8.12] - 2026-05-05

A feature release built on the v0.8.11 cache-maxing foundation: 20 community
PRs merged, covering reasoning-effort automation, V4 FIM edits, bash-arity
execpolicy, skill-registry sync, vim composer mode, large-tool-output routing,
pluggable sandbox backends, layered permission rulesets, and cache-aware
resident sub-agents. No breaking changes.

### Added
- **Reasoning-effort auto mode** (#669) — `reasoning_effort = "auto"` inspects
  the last user message for keywords (debug/error → Max, search/lookup → Low,
  default → High) and resolves the tier before each API request. Sub-agents
  always get Low.
- **FIM edit tool for V4 /beta** (#668) — `fim_edit` tool sends
  fill-in-the-middle requests to DeepSeek's `/beta` endpoint for surgical code
  edits.
- **Bash arity dictionary** (#655) — `auto_allow = ["git status"]` now matches
  `git status -s` but NOT `git push`. The arity dictionary knows command
  structure for git, cargo, npm, yarn, pnpm, docker, kubectl, aws, make, and
  others. Legacy flat prefix matching still works for unlisted commands.
- **Unified slash-command namespace** (#661) — user-defined commands in
  `~/.deepseek/commands/` support `$1`, `$2`, `$ARGUMENTS` template
  substitution. User commands override built-in commands.
- **Skill registry sync** (#654) — `/skills sync` fetches the community skill
  registry and installs/updates all listed skills. Network-gated by the
  existing `[network]` policy.
- **Vim modal editing in composer** (#659) — `vim.insert_mode` / `vim.normal_mode`
  settings enable modal editing in the message composer with standard Vim
  keybindings.
- **Separate tui.toml** (#657) — theme colors and keybind overrides can live in
  `~/.deepseek/tui.toml` alongside the main `config.toml`. *Note: file format
  is defined but not yet loaded at startup — wiring deferred to v0.8.13.*
- **Large-tool-output routing** (#658) — tool results exceeding a configurable
  token threshold are routed through a workshop with truncated previews,
  protecting the parent context window. Synthesis is currently truncation-only;
  V4-Flash sub-agent synthesis deferred to follow-up.
- **Pluggable sandbox backends** (#645) — a `SandboxBackend` trait and
  Alibaba OpenSandbox HTTP adapter let `exec_shell` route commands to a remote
  sandbox instead of spawning locally. Config keys: `sandbox_backend`,
  `sandbox_url`, `sandbox_api_key`.
- **Layered permission rulesets** (#653) — `ExecPolicyEngine` supports
  builtin, agent, and user-priority layers for allow/deny prefix rules.
  Deny-always-wins semantics.
- **Cache-aware resident sub-agents** (#660) — sub-agents spawned with
  `resident_file` prepend the file contents to their system prefix for V4
  prefix-cache locality. A global lease table prevents two agents from holding
  a resident lease on the same file simultaneously. Leases are released on
  agent completion.
- **Context-limit handoff** (#667) — engine-level support for replacing
  routine compaction with a `.deepseek/handoff.md` file write when context
  pressure triggers. *Note: config knob removed pending implementation.*
- **LSP auto-attach diagnostics** (#656) — edit results now include post-edit
  diagnostics via the engine-level LSP hooks path.

### Docs
- **README install section rewritten** (#672) — the previous lede claimed
  "no Node.js or Python runtime" but the very next paragraph told readers to
  install Node before continuing. Replaced with a three-path Install block
  (npm / cargo / direct download) that makes the npm wrapper's role explicit:
  it downloads the prebuilt binary, but `deepseek` itself does not depend on
  Node at runtime. zh-CN README mirrored.
- **Windows Scoop install instructions** (#696) — README and zh-CN README now
  document `scoop install deepseek-tui` for Windows users. *Thanks to
  [@woyxiang](https://github.com/woyxiang) for this PR.*
- **DeepSeek Pro discount window extended** (#692) — pricing footnote updated
  from 5 May 2026 to 31 May 2026 to match the platform-side promotion. *Thanks
  to [@wangfeng](mailto:wangfengcsu@qq.com) for this PR.*
- **`deepseek resume <SESSION_ID>` surfaced in Usage** — the command exists
  since v0.7 but was undocumented. Reported via #682.
- **SECURITY.md** (#648) — vulnerability reporting policy and supported
  versions.
- **CODE_OF_CONDUCT.md** (#686) — Contributor Covenant v2.1. *Thanks to
  [@zichen0116](https://github.com/zichen0116) for this PR.*
- **zh-Hans locale activation docs** (#652) — README.zh-CN.md and
  config.example.toml now document `locale = "zh-Hans"`.

### Fixed
- **Cross-workspace session bleed (security)** — launching `deepseek` from
  any directory silently auto-recovered the most recent interrupted session,
  even if that session originated in a completely different workspace. Tools
  then operated on the prior workspace's file paths while the status bar
  displayed the *current* workspace name — a confusing trust-boundary
  violation that could leak `api_messages`, `working_set` entries, and any
  secrets the prior session had accumulated into a new terminal that was
  never meant to see them. `try_recover_checkpoint()` now compares the saved
  session's workspace to `std::env::current_dir()` (canonicalised, with a
  strict-equality fallback when canonicalisation fails) and only auto-recovers
  on a match. On a mismatch the checkpoint is persisted as a regular session
  (so the user can find it via `deepseek sessions` / `deepseek resume <id>`)
  and cleared, and the new launch starts fresh — no data is lost. Hotfixed
  to `main` ahead of the v0.8.12 tag.
- **`cargo install` on stable Rust** — the language-picker match guard at
  `crates/tui/src/tui/ui.rs:1603` used `&& let Some(...) = ...` inside an
  `if`-guard, which requires the nightly-only `if_let_guard` feature on Rust
  before 1.94. Reported by an external user whose `cargo install
  deepseek-tui` failed with E0658. Rewrote as a plain match guard with a
  nested `if let` inside the arm body. The workspace also now declares
  `rust-version = "1.88"` (the actual minimum for `let_chains` in
  `if`/`while`) so users on too-old toolchains see a clear cargo error
  instead of a confusing rustc one. AGENTS.md gains a "stable Rust only"
  section so this doesn't regress.
- **Resident-file lease never released after spawn** (#660) — the lease was
  stamped as `"pending"` at spawn time because the agent id is only assigned
  by the manager after the spawn call returns. The release-on-terminal-state
  path (added in the original #660 commit) matched leases by agent id, so
  it could never find these placeholder entries. Now the placeholder is
  replaced with the real agent id immediately after spawn so existing
  release wiring fires. Resolves the v0.8.12 caveat documented at RC time.
- **Color::Reset across all UI widgets** (#651, #671) — replaced hardcoded
  `Color::Black` and `Color::Rgb(18, 29, 39)` backgrounds with `Color::Reset`
  so the TUI respects the terminal's actual background color on light-themed
  and non-standard terminals.
- **Windows MessageBeep** (#646) — `notify_done_to` now calls `MessageBeep` on
  Windows when BEL method is selected.
- **truncate_id optimization** (#649) — replaced manual string slicing with a
  shared `truncate_id` helper across session, picker, and UI call sites.

### Maintenance
- Workspace `cargo fmt` sweep across community PRs that landed unformatted.
- Issue-triage GitHub Actions added (#688): keyword-driven auto-labeller,
  stale-bot for `needs-info` issues (14 d → stale → 7 d → close), and a
  spam lockdown that auto-closes promotional issues from accounts <30 d
  old. All pure GitHub Actions — no third-party services.
- Annotated `TuiPrefs` (#657) and `handoff::THRESHOLDS` (#667) with
  `#[allow(dead_code)]` so the deferred APIs don't trip CI's `-D warnings`
  flag while their call sites are staged for v0.8.13.
- Removed dead `prefer_handoff` field from `CompactionConfig` — config knob
  existed but zero code paths consulted it (#667).
- Removed dead `use_terminal_colors` field from `TuiConfig` — no rendering
  code read the value (#671).
- Fixed `expect()` panic risk in `OpenSandboxBackend::new()` — now returns
  `Result` (#645).
- Fixed broken `section_bg` test assertion after Color::Reset migration (#651).
- Fixed `resolve_prefixes` docstring to accurately describe deny-always-wins
  behavior (#653).
- Wired `create_backend()` into `Engine::build_tool_context` — sandbox backend
  was defined but never activated (#645).
- Wired resident lease release on agent completion/cancellation/failure (#660).

### Contributors

First-time contributor to this release: **@zichen0116** (#686). Welcome — and
thank you.

Bulk community contributions by [@merchloubna70-dot](https://github.com/merchloubna70-dot)
(#645–#681, 28 PRs spanning features, fixes, and VS Code extension scaffolding).
*Thank you for the remarkable volume and quality of work.*

## [0.8.10] - 2026-05-04

A patch release: hotfixes, small UX polish, and four whalescale-unblocking
runtime API additions. No breaking changes.

### Added
- **OPENCODE shell.env hook** (#456) — lifecycle hooks can now inject
  shell environment into spawned commands without hard-coding env in
  prompts or wrapper scripts.
- **Stacked toast overlay** (#439) — status toasts can queue and render
  together instead of overwriting each other.
- **File @-mention frecency** (#441) — file mention suggestions learn
  from recent selections via `~/.deepseek/file-frecency.jsonl`.
- **Durable keybinding catalog** (#559) — `docs/KEYBINDINGS.md` is now
  the source-of-truth audit for current shortcuts and the future
  configurable-keymap registry.
- **Runtime API quartet for whalescale-desktop integration** (#561, #562, #563,
  #564, #567) — addresses whalescale#255/256/260/261:
  - `[runtime_api] cors_origins` config / `--cors-origin URL` flag (repeatable) /
    `DEEPSEEK_CORS_ORIGINS` env var, all stacking on top of the built-in
    dev-origin defaults (#561 / whalescale#255).
  - `PATCH /v1/threads/{id}` extended from `archived`-only to the full
    editable field set: `allow_shell`, `trust_mode`, `auto_approve`, `model`,
    `mode`, `title`, `system_prompt`. Empty string clears `title` /
    `system_prompt`. New `title` field on `ThreadRecord` is additive — no
    schema_version bump (#562 / whalescale#256).
  - `archived_only=true` query param on `GET /v1/threads` and
    `/v1/threads/summary`, backed by a new `ThreadListFilter` enum
    (#563 / whalescale#260).
  - `GET /v1/usage?since=&until=&group_by=<day|model|provider|thread>`
    aggregates token totals + cost (via `pricing.rs`) across all
    threads/turns. Empty time ranges yield empty `buckets` (never 404)
    (#564 / whalescale#261).
- **Language picker in first-run onboarding** (#566) — new step between
  Welcome and ApiKey lists every shipped locale (`auto` / `en` / `ja` /
  `zh-Hans` / `pt-BR`) with the native name (日本語, 简体中文, …) plus an
  English label so the target language is reachable without already
  speaking it. Hotkeys 1-5 select; persists immediately to
  `~/.deepseek/settings.toml`.
- **Windows + China install documentation** (#578) — expanded
  `docs/INSTALL.md` with Windows source-build setup, Visual Studio Build
  Tools / MSVC environment notes, rustup and Cargo mirror guidance, and
  antivirus troubleshooting. *Thanks to
  [@loongmiaow-pixel](https://github.com/loongmiaow-pixel) for this PR.*

### Changed
- **Agent prompt now explicitly describes DeepSeek cache-aware behavior**
  — long-session guidance explains why stable prompt prefixes, sub-agents,
  RLM, and late compaction matter for V4 cache economics.
- **Whale sub-agent nicknames now interleave Simplified Chinese with
  English** (`Blue` / `蓝鲸` / `Humpback` / `座头鲸` / …). Pure cosmetic;
  doubles the labeling pool size and gives a roughly even mix on each
  new spawn.
- **User memory docs + help polish** (#497, #569) — `/memory` is now
  listed in slash-command help, supports `/memory help`, and the README
  / configuration docs now point at the full `docs/MEMORY.md` guide and
  document both `[memory].enabled` and `DEEPSEEK_MEMORY`. *Thanks to
  [@20bytes](https://github.com/20bytes) for this PR.*

### Fixed
- **Compaction summaries are cache-aligned for DeepSeek V4** (#575, #580)
  — when the summarized message prefix fits the large V4 context budget,
  the summary request now reuses the original messages and appends the
  summary instruction as a normal user message instead of rebuilding a
  fresh `SUMMARY_PROMPT + dropped messages` input. This lets the summary
  call benefit from DeepSeek prefix caching. *Thanks to
  [@lloydzhou](https://github.com/lloydzhou) and
  [@jeoor](https://github.com/jeoor) for the cost reports and concrete
  strategy.*
- **Windows Terminal API-key paste during onboarding** (#577) — the
  setup wizard now handles Ctrl/Cmd+V before generic character input and
  filters control/meta-modified keys out of the API-key text path.
  *Thanks to [@toi500](https://github.com/toi500) for the report and
  workaround details.*
- **Terminal startup repaint** (#581) — the TUI clears the terminal
  immediately after initialization so normal-screen startup no longer
  leaves stale default-background rows above the first frame. *Thanks to
  [@xsstomy](https://github.com/xsstomy) for the screenshot.*
- **Markdown rendering for tables, bold/italic, and horizontal rules**
  (#579) — transcript markdown now handles table rows, strips separator
  rows, renders horizontal rules, applies inline bold/italic styles, and
  avoids an infinite-loop edge case on unclosed markers. *Thanks to
  [@WyxBUPT-22](https://github.com/WyxBUPT-22) for the PR, screenshots,
  and tests.*
- **Slash-prefix Enter activation** (#573) — typing a short prefix such
  as `/mo` and pressing Enter now activates the first slash-command
  match. *Thanks to [@melody0709](https://github.com/melody0709) for
  the report.*
- **macOS seatbelt blocked `~/.cargo/registry`** (#558) — `cargo publish`
  / `cargo build` from inside the TUI's shell tool was getting
  sandbox-denied. The seatbelt now allows read on `(param "CARGO_HOME")`
  and write on the `registry/` and `git/` subpaths whenever the policy
  isn't read-only. Honors `CARGO_HOME` env with a `$HOME/.cargo`
  fallback.
- **Stdio MCP servers now receive SIGTERM on shutdown** (#420) — instead
  of SIGKILL via `kill_on_drop`. New `async fn shutdown` on
  `McpTransport` overrides on `StdioTransport` to send SIGTERM and wait
  up to 2s for graceful exit before drop fires SIGKILL as the backstop.
  Wired into the engine's `Op::Shutdown` path so graceful exit is the
  default. A Drop fallback still SIGTERMs on abnormal exit paths.
- **Shell-spawned children get `PR_SET_PDEATHSIG(SIGTERM)` on Linux**
  (#421) — the kernel sends SIGTERM the moment the parent (TUI) exits,
  even on SIGKILL of the parent. Closes the leak window the cooperative
  cancellation path can't cover. macOS / Windows watchdog tracked as a
  follow-up; the existing `kill_on_drop` + process_group SIGKILL on
  cancellation still cover normal shutdown there.
- **npm install on older glibc now fails fast** (#555, #560, #556, #565)
  — the prebuilt Linux x64 / arm64 binaries are now built via
  `cargo zigbuild` targeting `x86_64-unknown-linux-gnu.2.28` /
  `aarch64-unknown-linux-gnu.2.28`, lowering the requirement from glibc
  ≥ 2.39 to ≥ 2.28. The npm postinstall also runs a Linux-only glibc
  preflight that fails fast with a clear "build from source" message
  when the host is incompatible (or musl). *Thanks to
  [@staryxchen](https://github.com/staryxchen) (#556) and
  [@Vishnu1837](https://github.com/Vishnu1837) (#565) for these PRs.*
- **Shell tool `cwd` parameter now validated against the workspace
  boundary** (#524) — the model could previously pass `cwd` paths
  outside the workspace; now `exec_shell` runs `ToolContext::resolve_path`
  on `cwd` like every other path-taking file tool, returning
  `PathEscape` on violations. `trust_mode = true` still bypasses,
  consistent with the file-tool pattern. *Thanks to
  [@shentoumengxin](https://github.com/shentoumengxin) for this PR.*

### Contributors

First-time contributors to this release: **@staryxchen** (#556),
**@shentoumengxin** (#524), **@Vishnu1837** (#565), **@20bytes**
(#569), **@loongmiaow-pixel** (#578), and **@WyxBUPT-22** (#579).
Welcome — and thank you.

## [0.8.8] - 2026-05-03

### Added
- **User memory MVP** (#489–#493) — opt-in persistent note file
  injected into the system prompt as a `<user_memory>` block.
  - `# foo` typed in the composer appends a timestamped bullet
    without firing a turn (#492).
  - `/memory [show|path|clear|edit]` slash command for inline
    inspection / editing hints (#491).
  - `remember` model-callable tool so the agent can capture
    durable preferences itself; auto-approved because writes are
    scoped to the user's own file (#489).
  - Hierarchy loader pulls `~/.deepseek/memory.md` (path
    configurable via `memory_path` / `DEEPSEEK_MEMORY_PATH`) and
    injects above the volatile-content boundary in the prompt
    (#490).
  - Default off; enable with `[memory] enabled = true` or
    `DEEPSEEK_MEMORY=on` (#493).
  - Full feature documentation in `docs/MEMORY.md`.
- **Inline diff rendering for `edit_file` / `write_file`** (#505) —
  tool results now emit a unified diff at the head of the body,
  picked up by the existing diff-aware renderer with line numbers
  and coloured `+`/`-` gutters. New `similar` crate dep.
- **OSC 8 hyperlinks** (#498) — URLs in the transcript become
  Cmd+click-openable in supporting terminals (iTerm2, Terminal.app
  13+, Ghostty, Kitty, WezTerm, Alacritty). Clipboard path strips
  the escapes so yanked text stays clean. Off-switch:
  `[tui] osc8_links = false`.
- **Retry/backoff visual countdown** (#499) — `⟳ retry N in Ms — reason`
  banner ticks down during HTTP backoff. On exhaustion the row turns
  red `× failed: <reason>` until the next turn starts.
- **MCP server health chip** (#502) — colour-coded `MCP M/N` in the
  footer's right-cluster: success / warning / error / muted by
  reachability. Hidden when zero MCP servers are configured.
- **Per-project config overlay** (#485) — `<workspace>/.deepseek/config.toml`
  overlays a curated set of fields on top of the user-global config:
  `model`, `reasoning_effort`, `approval_policy`, `sandbox_mode`,
  `notes_path`, `max_subagents`, `allow_shell`, plus the
  `instructions = [...]` array (#454). Pass `--no-project-config`
  to bypass for one launch.
- **Project-scope deny-list for credentials/redirects** (#417) —
  `api_key`, `base_url`, `provider`, and `mcp_config_path` are
  refused at project scope. A malicious
  `<workspace>/.deepseek/config.toml` would otherwise be able to
  exfiltrate prompts to an attacker-controlled endpoint by
  swapping the user's credentials and target host with
  project-controlled values, or redirect the MCP loader at a
  config that spawns arbitrary stdio servers under the user's
  identity. The denied key emits a stderr warning so a user who
  expected the override sees the deny instead of a silent drop.
- **Project-scope value-deny for the loosest postures** (#417
  follow-up) — `approval_policy = "auto"` and
  `sandbox_mode = "danger-full-access"` are pure escalation
  values, denied unconditionally at project scope regardless
  of the user's prior value. Sub-tightening comparisons
  (e.g. user `"never"` → project `"on-request"` is allowed
  even though it loosens) stay v0.8.9 follow-up because they
  need a richer ordering check.
- **`SSL_CERT_FILE` honored in the HTTPS client** (#418) — corporate
  proxy / TLS-inspecting MITM users can now point at their custom
  CA bundle and have it added alongside the platform's system
  trust store. Tries PEM-bundle parsing first (covers single-cert
  files too), falls back to DER. Failures log a warning and
  continue — the existing system roots still apply, so a
  malformed env var won't bring down the launch. Documented in
  `docs/CONFIGURATION.md`.
- **Execpolicy heredoc handling** (#419) — `normalize_command` now
  strips heredoc bodies before shlex tokenization so a user's
  `auto_allow = ["cat > file.txt"]` pattern matches the heredoc
  form `cat <<EOF > file.txt\nbody\nEOF` cleanly. Recognises the
  common forms (`<<DELIM`, `<<-DELIM`, `<<'DELIM'`, `<<"DELIM"`)
  while leaving the here-string operator (`<<<`) untouched.
  Without this fix, heredoc-form file writes would skip the
  user's auto-approve list and route through the approval modal
  even for explicitly-blessed commands.
- **Sub-agent role taxonomy expansion** (#404) — adds `Implementer`
  ("land this change with the minimum surrounding edit") and
  `Verifier` ("run the test suite, report pass/fail with evidence")
  to the existing `general` / `explore` / `plan` / `review` /
  `custom` set. Each role has a distinct system prompt posture.
  Documented in `docs/SUBAGENTS.md`.
- **`docs/SUBAGENTS.md`** — full sub-agent reference: role taxonomy,
  alias map, concurrency cap, lifecycle, session-boundary
  classification, output contract.
- **`docs/MEMORY.md`** — user-facing memory feature documentation.
- **Competitive analysis doc** — `docs/COMPETITIVE_ANALYSIS.md`
  catalogues capability matrix vs OpenCode and Codex CLI.
- **Session prune helper + `/sessions prune <days>`** (#406 phase-1) —
  drops persisted sessions older than N days from
  `~/.deepseek/sessions/`. Skips the checkpoint subdirectory and
  compares against metadata `updated_at` (not fs mtime, which can
  lie after an rsync). 10 total tests cover the helper's contract
  and the slash-command dispatch surface. Phase 2 (boot-prune +
  retention policy) stays v0.8.9 work.
- **`deepseek doctor --json`** now surfaces a `memory` block
  (`enabled` / `path` / `file_present`) so operators can verify
  memory configuration without booting the TUI.
- **Tool-output spillover** (#422 + #423 + #500) — tool outputs over
  100 KiB now spill to `~/.deepseek/tool_outputs/<id>.txt` from the
  engine's tool-execution path. The model receives a 32 KiB head plus
  a footer pointing at the spillover file (`Use read_file path=…`),
  the tool cell renders an inline `full output: <path>` annotation in
  live mode, and a 7-day boot prune keeps the directory bounded.
  Spillover is skipped on error results so the model still sees the
  failure message verbatim. The existing tool-details pager surfaces
  the truncated head so the user can verify what the model saw.

### Changed
- **Sub-agent concurrency cap raised to 10 by default** (#509) —
  was 5; configurable via `[subagents].max_concurrent` (hard
  ceiling 20). Running-count now ignores non-running, no-handle,
  and finished handles so completed agents stop occupying slots.
- **`SharedSubAgentManager` is `Arc<RwLock<...>>`** (#510) — read
  paths take read locks, eliminating the multi-agent fan-out UI
  freeze.
- **Sub-agent output summarized before parent context** (#511) —
  `compact_tool_result_for_context` now compresses
  `agent_result` / `agent_wait` payloads instead of dumping the
  full snapshot back into the parent's context window.
- **`agent_list` defaults to current-session view** (#405) — each
  manager mints a `session_boot_id` and stamps every spawn; agents
  loaded from prior sessions are filtered unless
  `include_archived=true` is passed. Each result carries a
  `from_prior_session` flag.
- **Concise todo / checklist update rendering** (#403) — repeat
  `todo_update` / `checklist_update` calls render a one-line
  `Todo #N: <title> → STATUS` card with full list still
  reachable via Alt+V instead of dumping the entire item array on
  every call.
- **Compact `agent_spawn` rendering** (#409) — the generic tool
  block for `agent_spawn` collapses to one header line in live
  mode (`◐ delegate · agent-abc12 [running]`) since the
  `DelegateCard` already owns live action progress. Transcript
  replay keeps the full block.
- **Plan panel role clarified** (#408) — drops the "No active
  plan" placeholder when the panel is otherwise empty; documents
  the panel's narrow role (`update_plan` tool output + `/goal` +
  cycle counter, distinct from todos).
- **Sub-agent description copy** — `agent_spawn` tool description
  and `prompts/base.md` updated to reflect the new default cap of
  10 (was stale "Max 5 in flight").
- **`agent_spawn` / `agent_assign` schema descriptions** (#404
  follow-up) — type/agent_name property descriptions now list
  `implementer` and `verifier` so the model surfaces those roles
  without having to discover them from `docs/SUBAGENTS.md`. Adds
  the long-form aliases (`builder` / `validator` / `tester`) on
  `agent_assign` for parity with the alias map.
- **Multi-day duration formatting** (#447) — `humanize_duration`
  now caps at two units and promotes through h/d/w boundaries.
  Long-running sessions render as `2d 3h` instead of `188415s`,
  and the previous "192m 30s" cycle output becomes `3h 12m`. The
  `/goal` status line picks up the same formatter so multi-day
  goal-elapsed times stay readable.
- **Accessibility flag** (#450) — `NO_ANIMATIONS=1` env var now
  forces `low_motion = true` and `fancy_animations = false` at
  startup, regardless of the saved `settings.toml`. Recognises
  the standard truthy spellings (`1`, `true`, `yes`, `on`).
  Documented end-to-end in the new `docs/ACCESSIBILITY.md`,
  including the existing `low_motion` / `calm_mode` /
  `show_thinking` / `show_tool_details` toggles for
  screen-reader users.
- **Cumulative session-elapsed footer chip** (#448) — a
  low-priority `worked 3h 12m` chip in the footer's right
  cluster shows session age once it crosses 60s. Hidden during
  the first minute of a launch so a fresh start doesn't flash a
  ticker. Drops first under narrow widths so the existing chips
  (coherence / agents / replay / cache / mcp) keep their slots.
  Sampled at props-build time (matches the `retry` capture
  pattern) so render stays pure for tests.
- **`instructions = [...]` config array** (#454) — declare
  additional instruction files (`./AGENTS.md`,
  `~/.deepseek/global.md`, …) and they're concatenated into the
  system prompt in declared order, above the skills block. Each
  file is capped at 100 KiB; missing files log a warning and are
  skipped instead of failing the launch. Project config replaces
  the user-level array wholesale (the typical "merge" pattern is
  for users who want both — they list `~/global.md` inside the
  project array). Documented in `config.example.toml`.
- **Keyboard-enhancement flags pop on suspend paths too** (#443
  follow-up) — `pause_terminal` (Ctrl+Z / shell-suspend) and
  `external_editor::spawn_editor_for_input` (composer `$EDITOR`
  launch) now pop the flags before handing the terminal to the
  child process, matching the existing shutdown and panic-hook
  paths. Defense-in-depth: if a future code path enables the
  flags explicitly, the suspend handlers won't leak them to a
  Vim / less / shell child that hasn't asked for them.
- **`load_skill` tool** (#434) — model-callable tool that takes a
  skill id and returns the SKILL.md body plus the sibling
  companion-file list in one call. Faster than the existing
  `read_file` + `list_dir` dance; surfaces the skill's
  description as a quote block at the head so a single tool
  result is self-contained. Resolves the skills directory with
  the same hierarchy `App::new` uses (`.agents/skills` →
  `skills` → `~/.deepseek/skills`). Available in Plan and
  Agent/Yolo modes.
- **Kitty keyboard protocol opt-in** (#442) — pushes
  `DISAMBIGUATE_ESCAPE_CODES` at startup so terminals that
  support the protocol (Kitty, Ghostty, Alacritty 0.13+,
  WezTerm, recent Konsole / xterm) report unambiguous events
  for Option/Alt-modified keys, plain Esc, and multi-byte
  sequences. Legacy terminals silently discard the escape and
  see no change. Only the disambiguation tier is pushed —
  release-event reporting was deliberately skipped because the
  existing handlers would mis-route releases as duplicate
  presses. The flags are popped on shutdown / panic / suspend
  paths (#443).
- **Multi-directory skill discovery** (#432) — the system
  prompt's `## Skills` listing and the `load_skill` tool now
  walk every candidate directory in the workspace plus the
  global default: `<workspace>/.agents/skills` →
  `<workspace>/skills` → `<workspace>/.opencode/skills` →
  `<workspace>/.claude/skills` → `~/.deepseek/skills`. Skills
  installed for any AI-tool convention show up in the same
  catalogue. Name conflicts resolve first-match-wins per the
  precedence order so workspace-local skills shadow user/global
  ones. New `skills_directories()` and
  `discover_in_workspace()` helpers in
  `crates/tui/src/skills/mod.rs`.
- **`tool.spillover` audit event** (#500 polish) — emit a
  discrete audit-log entry whenever `apply_spillover` writes a
  spillover file, so operators tailing
  `~/.deepseek/audit.log` can correlate large-output episodes
  with disk-usage growth in `~/.deepseek/tool_outputs/`. Fires
  in both the sequential and parallel tool paths.
- **Prompt stash** (#440) — Ctrl+S in the composer parks the
  current draft to a JSONL-backed stash at
  `~/.deepseek/composer_stash.jsonl` (no-op on empty composer).
  `/stash list` shows parked drafts (oldest first, with one-line
  previews and timestamps); `/stash pop` restores the most
  recently parked draft into the composer (LIFO). Self-healing
  parser drops malformed lines instead of poisoning the stash.
  Capped at 200 entries; multiline drafts round-trip intact via
  JSON's newline escaping.
- **`deepseek pr <N>` subcommand** (#451) — fetches PR
  title/body/diff via `gh` and launches the interactive TUI
  with a review prompt pre-populated in the composer. The
  diff is capped at 200 KiB (codepoint-safe truncation) so a
  massive PR doesn't blow the context window before the user
  hits Enter. Optional `--repo <owner/name>` and `--checkout`
  flags; falls back gracefully with an actionable error
  message if `gh` isn't on PATH. Adds a new
  `TuiOptions::initial_input` plumb that any future caller can
  reuse to drop the model into a session with text already
  typed.
- **`/stash clear` subcommand** (#440 polish) — wipes the
  entire stash file and reports how many parked drafts were
  dropped. Pairs with `/stash list` and `/stash pop` so the
  user can fully manage the stash from inside the TUI without
  reaching for `rm`.
- **`/hooks` read-only listing** (#460 MVP) — slash command
  enumerates configured lifecycle hooks grouped by event,
  showing each hook's name, command preview, timeout, and
  condition. Notes the global `[hooks].enabled` flag's state.
  No more `cat ~/.deepseek/config.toml` to debug "did my hook
  actually load". The picker / persisted enable-disable
  surface from #460 stays as v0.8.9 follow-up. Available via
  `/hooks` or `/hooks list`; aliased to `/hook`. Localized in
  en/ja/zh-Hans/pt-BR.
- **`deepseek doctor` reports cross-tool skill dirs** (#432
  follow-up) — both the human-readable and JSON outputs now
  surface `.opencode/skills/` and `.claude/skills/` presence /
  count, so operators can confirm at a glance whether any
  cross-tool skill folder is contributing to the merged
  catalogue. Empty dirs are omitted from the human-readable
  output to keep the report scannable; JSON always emits all
  five slots (`global`, `agents`, `local`, `opencode`,
  `claude`) for stable machine consumption.
- **`deepseek doctor` reports storage surfaces** (#422 / #440 /
  #500 follow-up) — new `Storage:` section surfaces the
  tool-output spillover dir
  (`~/.deepseek/tool_outputs/`) with file count and the
  composer stash file
  (`~/.deepseek/composer_stash.jsonl`) with parked-draft
  count. Mirrored under `storage.{spillover,stash}` in the
  JSON output so `deepseek doctor --json` keeps a stable
  schema.
- **`/hooks events` subcommand** (#460 polish) — lists every
  supported `HookEvent` value with a short blurb so users can
  discover which events to target in `[[hooks.hooks]]` entries
  without reading source. Ordered lifecycle → per-tool →
  situational, stable across releases.
- **Structured-Markdown compaction template** (#429) —
  `prompts/compact.md` switches from the legacy
  Active-task/Files-touched/Key-decisions/Open-blockers
  framing to the spec'd structure: Goal / Constraints /
  Progress (Done / In Progress / Blocked) / Key Decisions /
  Next step. The richer Progress sub-bullets help long
  resumed sessions distinguish "what's verified done" from
  "what's mid-flight" — useful when the model writes
  `.deepseek/handoff.md` before a long break. Backwards-
  compat: existing handoff.md files continue to render fine
  because the loader injects them as plain markdown (the
  template only guides what NEW handoffs look like). The
  pinned-tool-output configurability part of #429's spec
  stays a v0.8.9 follow-up — that requires changes to
  `cycle_manager.rs` compaction logic itself.
- **`tool_call_before` / `tool_call_after` / `message_submit` /
  `on_error` hooks all fire now** (#455 observer-only slice) —
  these events were defined in the `HookEvent` enum but never
  fired from production code. Wired through:
  `tool_call_before` and `tool_call_after` fire from
  `tool_routing.rs`; `message_submit` fires from
  `dispatch_user_message` before engine dispatch; `on_error`
  fires from `apply_engine_error_to_app` before the error cell
  reaches the transcript. Hook contexts populate the relevant
  fields (`tool_name` + `tool_args` / `tool_result`,
  `message`, `error`). Hooks remain read-only in this slice;
  argument / result / message mutation is a v0.8.9 follow-up
  because it needs a synchronous-gate contract that doesn't
  exist today. Combined with the existing `session_start` /
  `session_end` / `mode_change` events, every variant in the
  `HookEvent` enum now has a live producer. Each fire is
  fast-path-gated by
  `HookExecutor::has_hooks_for_event(event)` so per-tool
  dispatch never pays for `HookContext` allocation when the
  user has no hooks configured (the common case).
- **RLM tool family** (#512) — `rlm` tool cards map to
  `ToolFamily::Rlm` and render `rlm`, not `swarm`. Stale "swarm"
  wording cleaned out of docs / comments / tests.
- **Foreground RLM visible in Agents sidebar** (#513 — stopgap)
  — projection now shows foreground RLM work; full async
  lifecycle remains v0.8.9.

### Fixed
- **`Don't auto-approve git -C ...`** (#416, shipped 2026-05-03) —
  v0.8.8 release runtime fix; foundation for the rest of the
  stabilization batch.
- **Self-update arch mapping** (#503) — `update.rs` uses release
  asset naming (`arm64`/`x64`) instead of raw Rust constants
  (`aarch64`/`x86_64`); rejects `.sha256` siblings as primary
  binaries.
- **Composer Option+Backspace deletes by word** (#488) — was
  deleting by character.
- **Offline composer queue is session-scoped** (#487) — legacy
  unscoped queues fail closed instead of leaking content into
  unrelated chats.
- **`display_path` test race + Windows separator** (#506) —
  tests no longer mutate `$HOME`; `display_path_with_home` walks
  components and joins with `MAIN_SEPARATOR_STR` so Windows shows
  `~\projects\foo` not `~\projects/foo`.
- **Footer reads statusline colours from `app.ui_theme`** (#449) —
  was using a bespoke palette.
- **Keyboard-enhancement flags pop on panic exit too** (#443/#444) —
  raw-mode startup probe is now bounded by a configurable
  timeout.
- **CI workflow cleanup** (#507) — pruned three duplicated/dead
  workflows (`crates-publish.yml`, `parity.yml`, `publish-npm.yml`);
  `release.yml` `build` job now allows `parity` to be skipped on
  manual `workflow_dispatch`; release-runbook reconciled.
- **Slash-menu layout jitter on Windows** — typing through a
  `/foo` autocomplete used to shrink the matched-entry count,
  which shrank the composer height every keystroke, which forced
  the chat area above to repaint. On Windows 10 PowerShell + WSL
  the per-cell write cost made the jitter visible. Composer now
  reserves its panel-max envelope for the whole slash/mention
  session so the chat-area Rect stays stable; the menu still
  renders only the entries that actually match.

- **Linux ARM64 prebuilt binaries** — the release workflow now publishes
  `deepseek-linux-arm64` and `deepseek-tui-linux-arm64` (built natively on
  GitHub's `ubuntu-24.04-arm` runner). The npm wrapper picks them up
  automatically on `arm64` Linux hosts, so HarmonyOS thin-and-light,
  openEuler/Kylin, Asahi Linux, Raspberry Pi, AWS Graviton, etc. now work
  with a plain `npm i -g deepseek-tui`.
- **Interactive TUI hangs on `working.` at 100% CPU (#549)** — the event
  loop's blocking terminal poll starved the tokio runtime, preventing the
  engine task from dispatching the API request. Fixed by yielding to the
  scheduler before each poll cycle and clamping the event-poll timeout to
  a minimum of 1ms so a zero-timeout hot-loop can't monopolize the thread.
- **Backspace key inserts "h" instead of deleting (#550)** — terminals
  that send `^H` (Ctrl+H) for Backspace were not recognized. Added
  `is_ctrl_h_backspace()` guard in both the composer and API-key input
  handlers so Ctrl+H is treated as a delete, matching the existing
  `KeyCode::Backspace` behavior.

### Changed
- **npm `postinstall` failure messages** — when no prebuilt is available for
  the host's `os.platform() / os.arch()` combo, the wrapper now prints the
  full `cargo install` fallback recipe and a link to
  [`docs/INSTALL.md`](docs/INSTALL.md) instead of just the bare error.
- **`DEEPSEEK_TUI_OPTIONAL_INSTALL=1`** — new env knob that downgrades a
  postinstall failure to a warning + `exit 0`, so CI matrices that include
  unsupported platforms don't fail the whole `npm install`.

### Docs
- New [`docs/INSTALL.md`](docs/INSTALL.md) — every supported platform,
  prebuilt vs. `cargo install` vs. manual download, cross-compiling x64 → ARM64
  Linux with `cross` or `gcc-aarch64-linux-gnu`, and a troubleshooting section
  covering the common `Unsupported architecture`, `MISSING_COMPANION_BINARY`,
  and self-update mismatch errors.
- README and `README.zh-CN.md` now have an explicit **Linux ARM64** quickstart
  pointing ARM64 users at `cargo install deepseek-tui-cli deepseek-tui --locked`
  for v0.8.7 and at `npm i -g deepseek-tui` for v0.8.8+.

### Releases
- npm wrapper publish remains manual (npm 2FA OTP requirement).
- GitHub release automation depends on `RELEASE_TAG_PAT` secret —
  without it `auto-tag.yml` creates the tag but `release.yml`
  doesn't fire.

## [0.8.7] - 2026-05-03

### Fixed
- **Selection across transcript cell types** — the selection-tightening from
  v0.8.6 (#383) restricted copy/select to user and assistant message bodies
  only, so text in system notes, thinking blocks, and tool output could not be
  copied. v0.8.7 removes the body-start gate; the rendered transcript block is
  fully selectable again.

## [0.8.6] - 2026-05-03

### Added
- **Long-session survivability by default** (#402) — capacity control and
  compaction defaults are enabled, transcript history is bounded, persisted
  sessions are capped, and oversized history folds into archived context
  placeholders instead of freezing the TUI.
- **v0.8.6 feature batch** (#373-#402) — adds Goal mode, cache-hit chips,
  cycle-boundary visualization, file-tree pane, `/share`, `/model auto`,
  user-defined slash commands, `/profile`, LSP diagnostic wiring,
  crash-recovery, self-update, `/init`, `/diff`, patch-aware `/undo`,
  `/edit`, inline diff highlighting, smart clipboard, native-copy escape,
  right-click context menus, clickable file:line styling, and MCP Phase A.

### Fixed
- **Lag and rendering regressions** (#399, #400) — moves git/file-tree work
  off the UI thread where possible, bounds render history, and tightens redraw
  behavior to avoid sidebar/chat text bleed-through.
- **Release-hardening follow-ups** — `/share` now writes via secure temp files,
  self-update uses secure same-directory temps with Windows-safe replacement,
  and docs/rustfmt release gates are clean.

## [0.8.4] - 2026-05-02

### Added
- **Localization expansion (Phase 1, #285)** — every slash command's help
  description, the full `/tokens` / `/cost` / `/cache` debug output, the
  footer state and chip text, and the help-overlay section headings are
  now translated for all four shipped locales (`en`, `ja`, `zh-Hans`,
  `pt-BR`). Set the language with `/config locale zh-Hans` (or
  `LANG=zh_CN.UTF-8` / `LC_ALL=zh_CN.UTF-8` from the shell). Non-Latin
  scripts render via the same `unicode_width` plumbing the existing 27
  chrome strings already use; the `shipped_first_pack_has_no_missing_core_messages`
  test enforces full coverage across all four locales for every new
  `MessageId`. Tool descriptions sent to the model and the base system
  prompt intentionally remain English (training-data alignment, prefix
  cache stability).
  - Phase 1a (#294): 44 new IDs covering slash commands.
  - Phase 1b (#295): 13 new IDs covering `/tokens` / `/cost` / `/cache`
    debug output. Templates use `{placeholder}` substitution so a
    translator can re-order args freely.
  - Phase 1c (#296): 11 new IDs covering footer state, sub-agent chip,
    quit-confirmation toast, and help-overlay section labels.
- **Stable cache prefix** (#263) — five companion fixes to keep the
  DeepSeek prefix cache stable across turns: drop volatile fields from
  the working-set summary block (#280, #287), place handoff and
  working-set after the static prompt blocks (#288 → #292), memoise the
  tool catalog so descriptions stay byte-stable (#289), sort
  `project_tree` and `summarize_project` output (#290), and use a unique
  fallback id for parallel streaming tool calls so downstream tool-result
  routing doesn't match the first call twice (#291). The combined effect
  is a meaningful jump in cache hit rate after the third turn.

### Fixed
- **Agent-mode shell exec could not reach the network** (#272) — the seatbelt
  default policy denies all outbound network including DNS, so any
  `exec_shell` command needing the network (`curl`, `yt-dlp`, package
  managers, …) failed in Agent mode unless the user dropped to Yolo. The
  engine now elevates the sandbox policy to `WorkspaceWrite { network_access:
  true, … }` for both Agent and Yolo. Plan mode is unchanged (read-only
  investigation never registers the shell tool). The application-level
  `NetworkPolicy` (`crates/tui/src/network_policy.rs`) remains the only
  outbound-traffic boundary.
- **`/skill install <github-repo-url>` failed with `invalid gzip header`** (#269)
  — `https://github.com/<owner>/<repo>` parsed as a raw direct URL, so the
  installer downloaded the HTML repo page and tried to gzip-decode HTML.
  Bare GitHub repo URLs (with or without `.git`, with or without `www.`,
  with or without a trailing slash) now route to the `GitHubRepo` source the
  same as `github:<owner>/<repo>`. URLs that already point at a specific
  archive / blob / tree path still go through `DirectUrl`.
- **V4 Pro discount expiry extended** (#267) — DeepSeek extended the V4 Pro 75%
  promotional discount from 2026-05-05 15:59 UTC to 2026-05-31 15:59 UTC. Without
  this update the TUI would have started showing 4× the actual billed cost on
  May 6 onwards. Verified at https://api-docs.deepseek.com/quick_start/pricing.

## [0.8.3] - 2026-05-01

### Fixed
- **Skills prompt referenced fabricated paths** — `render_available_skills_context`
  rendered each skill's file as `<skills_dir>/<frontmatter-name>/SKILL.md`,
  which did not exist when the directory name differed from the frontmatter
  `name` (community installs, manually-placed skills). `Skill` now carries the
  real path captured at discovery and renders that.
- **Missing-companion error was hostile to direct GitHub Release downloaders**
  (#258) — replaced "Build workspace default members to install it" wall of
  text with a concrete three-path checklist: `npm install -g deepseek-tui`,
  `cargo install deepseek-tui-cli deepseek-tui --locked`, or downloading both
  `deepseek-<platform>` AND `deepseek-tui-<platform>` from the same Release
  page. `DEEPSEEK_TUI_BIN` stays as a power-user fallback.

### Added
- **Privacy: `$HOME` contracts to `~` in viewer-visible paths** — the TUI,
  `deepseek doctor`, `deepseek setup`, and onboarding now contract the home
  directory to `~` in every path shown on screen, so screenshots, screencasts,
  and pasted help output do not leak the OS account name. Persisted state,
  audit log, session checkpoints, and LLM-bound system prompts intentionally
  keep absolute paths for full fidelity.
- **`crates.io` badge** alongside the CI and npm badges in both English and
  Simplified Chinese READMEs.
- **Engine decomposition** (#227) — `core/engine.rs` is split into focused
  submodules (`engine/{streaming,turn_loop,dispatch,tool_setup,tool_execution,tool_catalog,context,approval,capacity_flow,lsp_hooks,tests}.rs`).
  No behavior change; preparation for the future agent-loop work.

### Tests
- RLM bridge: `batch_guard` extracted and tested for the empty-batch and
  oversize-batch invariants; depth-guard fallback covered (partial #231).
- Persistence: schema-version rejection covered for `load_session`,
  `load_offline_queue_state`, `runtime_threads::load_turn`,
  `runtime_threads::load_item` (partial #233).
- Command palette: `[disabled]` server description tag (closes the
  remaining #197 acceptance gap).
- Protocol-recovery contract tests now scan the engine submodules in
  addition to `engine.rs` so the decomposition refactor doesn't silently
  hide the fake-wrapper marker assertions.

### Issue triage
- 10 issues closed with verification commits cited (#247, #235, #197,
  #250, #234, #243, #238, #236, #239, #195).

## [0.8.2] - 2026-05-01

### Fixed
- **Windows release build (LNK1104)** — drop the `deepseek` shim binary in
  `crates/tui` that 0.8.1 introduced for the bundled `cargo install`. It
  produced a second `target/release/deepseek.exe` that collided with the
  `deepseek-tui-cli` artifact during workspace builds; the second linker
  invocation hit `LNK1104: cannot open file deepseek.exe` on Windows. The
  cli crate is now the single source of `deepseek`; workspace default
  members still produce both binaries (one per crate).
- **npm wrapper offline robustness** — `bin/deepseek(-tui).js` no longer
  re-fetches the GitHub-hosted SHA-256 checksum manifest on every invocation.
  When the binary is already installed and its `.version` marker matches the
  package version, the wrapper trusts the local file. The manifest is fetched
  lazily on actual download (first install or `DEEPSEEK_TUI_FORCE_DOWNLOAD=1`),
  so GitHub flakes, captive portals, corporate proxies, and offline state no
  longer break every command.

### Added
- **Model-visible skills block** — installed skills (name, description, file
  path) are now exposed in the agent's system prompt under a `## Skills`
  section, with progressive disclosure: bodies stay on disk, the model opens a
  specific `SKILL.md` only when it decides to use that skill. Capped at a 12k
  prompt budget with 512-char per-description truncation. Threaded through
  `EngineConfig.skills_dir` so the TUI app, exec agent, and runtime thread
  manager all populate it from `Config::skills_dir()`.
- **Simplified Chinese README** (`README.zh-CN.md`) with cross-link from the
  English README.

### Changed
- **`cargo install` UX** — to install the canonical `deepseek` command,
  `cargo install deepseek-tui-cli` (the historical path). The 0.8.1
  one-command flow (`cargo install deepseek-tui` providing both binaries) is
  reverted because it broke Windows release builds; install both packages
  separately if you want the TUI binary too.

## [0.8.1] - 2026-05-01

### Fixed
- **One-command Cargo install** — `cargo install deepseek-tui --locked` now
  provides both the canonical `deepseek` dispatcher and the `deepseek-tui`
  companion binary from the main `deepseek-tui` package, so dispatcher
  subcommands such as `deepseek doctor --json` work without installing
  `deepseek-tui-cli` separately.

## [0.8.0] - 2026-05-01

### Fixed
- **Shell FD leak / post-send lag** — completed background shell jobs now release
  their process, stdin, stdout, and stderr handles as soon as completion is
  observed, while keeping the job record inspectable. This prevents long-running
  TUI sessions from hitting `Too many open files (os error 24)`, which could
  make checkpoint saves fail and cause shell spawning, message send, close, and
  Esc/cancel paths to lag or fail.
- **Windows REPL runtime CI startup** — Windows gets a longer Python bootstrap
  readiness timeout for the REPL runtime tests, matching GitHub runner startup
  contention without weakening bootstrap failures on other platforms.

### Added
- **China / mirror-friendly Cargo install docs** — README now documents
  installing through the TUNA Cargo mirror and direct release assets for users
  with slow GitHub/npm access.

### Tests
- Added a regression test proving completed background shell jobs drop their
  live process handles after `exec_shell_wait`.
- Re-ran the focused shell cancellation and Python REPL runtime slices.

## [0.7.9] - 2026-05-02

### Fixed
- **Post-turn freeze** — the checkpoint-restart cycle boundary (`maybe_advance_cycle`) now runs *before* `TurnComplete` emission instead of after, so the terminal is immediately responsive when the UI receives the completion event. The status chip ("↻ context refreshing…") remains visible during the cycle wait. (#234)
- **Enter during streaming no longer corrupts the turn** — a new `QueueFollowUp` submit disposition parks the draft on `queued_messages` when the model is actively streaming text. Previously, pressing Enter during streaming would forward the message as a mid-turn steer, which could interfere with the in-flight response. The message now dispatches as a normal user message after `TurnComplete`. (#234)
- **Idempotent Esc during fanout** — `finalize_active_cell_as_interrupted` and `finalize_streaming_assistant_as_interrupted` are now guarded by `Option::take()`. When Esc cancels a turn and the engine later delivers `TurnComplete(Interrupted)`, the second call is a no-op — no double `[interrupted]` prefix, no corrupted cell state. Regression test locks in the contract. (#243)

### Tests
- 2 new tests: `submit_disposition_queue_follow_up_when_streaming` (Enter/steering fix), `turn_complete_after_esc_is_idempotent` (Esc fanout double-call hardening)
- 1 expanded test: `submit_disposition_queue_when_offline_and_busy` now covers streaming state

## [0.7.8] - 2026-05-01

### Added
- **`exec_shell_cancel` tool** — cancel a running background shell task by id, or cancel all running tasks with `all: true`. Requires approval. (#248)
- **Foreground-to-background shell detach** — press `Ctrl+B` while a foreground command is running to open shell controls and either detach the command to the background (where it can be polled via `exec_shell_wait`) or cancel the current turn. (#248)
- **`exec_shell_wait` turn-cancellation awareness** — canceling a turn while `exec_shell_wait` is blocking now stops the wait but leaves the background task running, with `wait_canceled: true` in metadata. (#248)
- **`ShellControlView` modal** (Ctrl+B) — two-option dialog (Background / Cancel) rendered as a popup over the transcript. (#248)

### Changed
- **`exec_shell` foreground path** now spawns all foreground commands through the background job table, enabling the detach-to-background flow. Metadata now includes `backgrounded: true/false`. (#248)
- **`exec_shell_interact`** poll loop now observes the turn cancel token so stalled interactive sessions don't block turn cancellation. (#248)
- **Transcript running-tool hint** — executing shell cells now show "Ctrl+B opens shell controls" while running. (#248)
- **Keybinding registry** now includes `Ctrl+B` (opens shell controls) next to `Ctrl+C` (cancel/exits). (#248)
- **Deferred swarm card creation** — `agent_swarm` no longer pre-seeds an all-pending FanoutCard from `ToolCallStarted`; the card is created only when the first `SwarmProgress` event carries real worker state. Until then the sidebar uses the declared task count as a pending dispatch placeholder. (#236, #238)
- **Swarm wording normalized** — fanout-family fallback labels now render as `swarm`, matching the canonical `agent_swarm` / `rlm` model and avoiding mixed `fanout` / `swarm` terminology in the transcript. (#236, #238)
- **OPERATIONS_RUNBOOK** and **TOOL_SURFACE** updated with new shell control paths and `exec_shell_cancel` documentation.

### Fixed
- **Nonblocking swarm state drift** — the sidebar no longer falls back to `0` or a contradictory seeded placeholder before the first progress event arrives, which removes the visible `pending` vs `running/done` mismatch during early `agent_swarm` dispatch. (#236, #238)
- **Unicode-safe search globbing** — search wildcard matching now iterates on UTF-8 char boundaries instead of raw byte offsets, preventing panics on filenames like `dialogue_line__冰糖.mp3`. (#249)

### Tests
- 7 new integration tests: foreground-to-background detach, wait-cancel-leaves-process, single-task cancel, bulk cancel (kill-all), foreground-cancel-kills, ShellControlView default/select states
- Expanded swarm/sidebar regression coverage for deferred card creation and pending-count fallback before first `SwarmProgress`. (#236, #238)
- Added a Unicode filename regression test for wildcard search matching. (#249)

## [0.7.7] - 2026-04-30

### Added
- **Checklist card rendering** — `checklist_write` / `todo_*` results now render as a purpose-built card with completed/total + percent header, per-item status markers (✅ / `●` / `○`), and a collapsing affordance for long lists. Plumbed through `GenericToolCell` so no new variant threading is needed. (#241)
- **Context menu for transcript operations** — right-click or `Ctrl+M` opens a context-sensitive menu with Copy, Copy All, and selection-aware actions. (`crates/tui/src/tui/context_menu.rs`)
- **Windows .exe sibling lookup** — `locate_sibling_tui_binary` in the CLI dispatcher finds `deepseek-tui.exe` on Windows, honours `DEEPSEEK_TUI_BIN` override, and falls back to suffix-less lookup. Tests lock in platform-correct name resolution and env override. (#247)

### Changed
- **Swarm/sub-agent canonical data model** — `SwarmTaskOutcome` and `SwarmOutcome` are now the single source of truth. Every UI surface (sidebar, transcript FanoutCard, footer) reads from `swarm_jobs` rather than maintaining parallel projections. (#236, #238)
- **`swarm_card_index`** binds each swarm to its own FanoutCard by `swarm_id`, so overlapping fanouts no longer have one swarm's late progress clobber another's card. (#236, #238)
- **Fanout-class tools suppressed from footer** — `agent_swarm`, `spawn_agents_on_csv`, `rlm`, and `agent_spawn` no longer appear as active tools in the status strip; sidebar and FanoutCard show the actual worker counts. (#236, #238)
- **Esc clears active tool entries optimistically** — the active cell is finalized immediately on cancel rather than waiting for the engine's `TurnComplete` echo. Background `block:false` swarms remain durable and tracked through `swarm_jobs`. (#243)
- **Post-turn workspace snapshot detached** — the snapshot still runs on `spawn_blocking` but the engine no longer awaits its `JoinHandle`, so the UI accepts input immediately after `TurnComplete`. (#234)
- **Shell output preserves Cargo/test summaries under truncation** — high-signal tail lines (`test result:`, `failures:`, `error[E…]`, `Finished`, `Compiling`, panic markers) survive truncation so the agent doesn't re-run gates. (#242)
- **Monotonic spend display** — `displayed_session_cost` + `displayed_cost_high_water` ensure the visible session+sub-agent total never decreases across reconciliation events (cache discounts, provisional → final). (#244)
- Clipboard module expanded with additional platform-aware copy/paste paths. (`crates/tui/src/tui/clipboard.rs`)
- Context inspector enriched with additional metadata columns and session-scoped agent state. (`crates/tui/src/tui/context_inspector.rs`)
- Configuration documentation updated for v0.7.7 settings. (`docs/CONFIGURATION.md`, `docs/MODES.md`)

### Fixed
- **Windows npm install path** — the npm-distributed `deepseek` dispatcher now locates the platform-correct `deepseek-tui` binary (`.exe` suffix on Windows), fixing runtime failures for Windows users. (#247)
- **Sidebar/transcript/footer agreement** — all three surfaces now agree on agent counts and status because they share the canonical `swarm_jobs` store. (#236, #238)
- **Fanout card clobbering** — overlapping swarms no longer overwrite each other's progress cards. (#238)
- **Cost display regression** — negative reconciliation events (cache-hit discount applied after provisional count) no longer briefly drop the displayed cost. (#244)

### Tests
- 65+ new/expanded tests: checklist card rendering, swarm card index binding, fanout tool suppression, Esc cancel contract, monotonic spend under reconciliation, shell summary preservation, Windows sibling binary lookup, clipboard platform paths, context menu state transitions

### Added
- **UI Localization registry** — `locale` setting in `settings.toml` (`auto`, `en`, `ja`, `zh-Hans`, `pt-BR`) with `LC_ALL`/`LC_MESSAGES`/`LANG` auto-detection. Core packs shipped for English, Japanese, Chinese Simplified, and Brazilian Portuguese covering composer placeholder, history search, `/config` chrome, and help overlay. Missing/unsupported locales fall back to English. (`crates/tui/src/localization.rs`, `docs/CONFIGURATION.md`)
- **Grouped, searchable `/config` editor** — settings organized by section (Model, Permissions, Display, Composer, Sidebar, History, MCP) with live substring filter. Typing `j`/`k` navigates when the filter is empty; otherwise they enter the filter. (`crates/tui/src/tui/views/mod.rs`)
- **Pending input preview widget** — while a turn is running, queued messages, pending steers, rejected steers, and context chips render above the composer. Three-row-per-message truncation with ellipsis overflow. (`crates/tui/src/tui/widgets/pending_input_preview.rs`)
- **Alt+↑ edit-last-queued** — pops the most recently queued message back into the composer for editing. No-op when the composer is dirty. (`crates/tui/src/tui/app.rs`)
- **Composer history search and draft recovery** — `Alt+R` opens a live substring search across `input_history` and `draft_history` (max 50 entries). `Enter` accepts, `Esc` restores the pre-search draft. Unicode case-insensitive matching. (`crates/tui/src/tui/app.rs`)
- **Paste-burst detection** — fallback rapid-key paste detection independent of terminal bracketed-paste mode. Configurable via `paste_burst_detection` setting (default on). CRLF normalization (`\r\n` → `\n`, `\r` → `\n`). (`crates/tui/src/tui/paste_burst.rs`)
- **Composer attachment management** — `↑` at the composer start selects the attachment row; `Backspace`/`Delete` removes it without editing placeholder text. (`crates/tui/src/tui/app.rs`)
- **Searchable help overlay** — live substring filter across slash commands and keybindings, multi-term AND matching, localized chrome. (`crates/tui/src/tui/views/help.rs`)
- **Keyboard-binding documentation catalog** — single source of truth for help overlay rendering. Documents 38+ keyboard chords across Navigation, Editing, Submission, Modes, Sessions, Clipboard, and Help sections. (`crates/tui/src/tui/keybindings.rs`)
- **Legacy Rust deprecation audit** — non-destructive compatibility audit covering legacy MCP sync API, prompt constants, `/compact`, `todo_*` aliases, sub-agent aliases, provider `api_key` compatibility, model alias canonicalization, and palette aliases. Tracked by #218–#221. (`docs/LEGACY_RUST_AUDIT_0_7_6.md`)

### Changed
- **Shift+Tab cycles reasoning-effort** through Off → High → Max (three behaviorally distinct tiers). Previously Tab cycled modes; Shift+Tab is now the reasoning-effort shortcut. (`crates/tui/src/tui/app.rs:1119`)
- **Reasoning-effort `Off` now sends `"off"`** to the API (was `None`). Allows explicit thinking disable. (`crates/tui/src/tui/app.rs`)
- **Media `@`-mentions now emit `<media-file>` hints** directing users to `/attach` instead of inlining binary bytes. Tests lock in the contract. (`crates/tui/src/tui/file_mention.rs`)
- **`/attach` rejects non-media files** with a descriptive error pointing to `@path` for text. (`crates/tui/src/commands/attachment.rs`)
- **Configuration reference updated** to cover all v0.7.6 settings: `locale`, `paste_burst_detection`, `reasoning_effort`, `composer_density`, `sidebar_focus`, and more. (`docs/CONFIGURATION.md`)

### Fixed
- **Unicode-safe truncation** in pending-input preview and view text — no more mid-character breaks on multi-byte UTF-8. (`crates/tui/src/tui/widgets/pending_input_preview.rs`, `crates/tui/src/tui/views/mod.rs`)
- **CJK/emoji display-width handling** in locale tests and config view rendering. (`crates/tui/src/localization.rs`)
- **Context preview distinguishes `@media`, `/attach`, missing, and included files** with separate kind labels and inclusion status. (`crates/tui/src/tui/file_mention.rs`)
- **Config view filter accept `j`/`k` only when filter is empty** — typing `j` or `k` into the filter field no longer navigates away. (`crates/tui/src/tui/views/mod.rs`)

### Tests
- 7 localization tests (tag normalization, env resolution, shipped pack completeness, missing-key fallback, Unicode width truncation)
- 11 pending-input preview tests (context buckets, truncation, URL overflow, narrow-width)
- 13 paste tests (burst detection, CRLF normalization, clipboard images, Unicode)
- 9 draft/history search tests (match filter, unicode, accept/cancel, recovery)
- 93 config tests (grouping, filter, edit, j/k, localization, escape/cancel)
- 24 workspace tests (context refresh, scroll, mention completion)
- 7 file-mention tests (context references, media/attach distinction, removability)

## [0.7.1] - 2026-04-28

### Added
- Grouped active tool-call cards with compact rails and a live working-status row while tools run. (#142, #149)
- Selected-card-aware Alt+V details so the visible or selected tool card opens the matching detail payload. (#143)
- Compact terminal-native session context inspector with persisted `@path` and `/attach` reference metadata for resumed transcripts. (#146, #150)

### Changed
- Polished tool cards, diff summaries, and pending context previews for denser terminal-native scanning. (#141, #144, #145, #148)
- Ranked Ctrl+P file-picker results with working-set relevance from modified files, recent `@file` mentions, and recent tool paths while keeping fuzzy filtering in memory. (#147)

## [0.7.0] - 2026-04-28

### Added
- OS keyring-backed auth storage with `deepseek auth` subcommands, migration from plaintext config, provider-aware key resolution, and doctor visibility. (#134)
- Egress network policy with allow/deny/prompt decisions, deny-wins matching, audit logging, and enforcement hooks for network-capable tools. (#135)
- LSP diagnostics auto-injection after edits so compile feedback can be reinjected into the next agent turn. (#136)
- Side-git workspace snapshots, `/restore`, and `revert_turn` so agent edits can be rolled back without moving the user's repository HEAD. (#137)
- Esc-Esc backtrack over prior user turns, desktop turn-complete notifications, Alt+V tool-details access, safer command-prefix auto-allow matching, bundled `skill-creator`, and `/skill install` management for community skills. (#131, #132, #133, #138, #139, #140)

### Changed
- Split more engine/tool primitives into focused modules and workspace crates, including shared tool result primitives and extracted turn/capacity flow. (#67, #74)

### Tests
- Added mock LLM and skill-install integration coverage for streaming turns, reasoning replay, tool-call loops, network policy, and skill validation. (#69, #140)

## [0.6.5] - 2026-04-27

### Added
- **`rlm_process` tool — recursive language model as a tool call.** The previous `/rlm` slash command had a UI rendering gap (the answer never made it back to the model's view) and required the user to remember to invoke it manually. `rlm_process` exposes the full RLM loop as a structured tool the model itself can choose, the same way it reaches for `agent_spawn` or `rlm_query`. Inputs: `task` (small instruction, shown to the root LLM each iteration) plus exactly one of `file_path` (workspace-relative, preferred — keeps the long input out of the model's context entirely) or `content` (inline, capped at 200k chars). Optional `child_model` (default `deepseek-v4-flash`) and `max_depth` (default 1, paper experiments). Returns the synthesized answer with metadata (iterations, duration, tokens, termination reason). Loaded across Plan / Agent / YOLO; never deferred via ToolSearch. (`crates/tui/src/tools/rlm_process.rs`)
- **Reference-aligned REPL surface.** Aligned the in-REPL Python helpers with the canonical reference RLM (alexzhang13/rlm). The sub-agent now sees `context` (the full input, not `PROMPT`), `llm_query`, `llm_query_batched`, `rlm_query` (was `sub_rlm`), `rlm_query_batched`, `SHOW_VARS()`, `FINAL(...)`, `FINAL_VAR(...)`, plus `repl_get`/`repl_set`. Same prompt patterns and decomposition strategies from the paper now apply verbatim. (`crates/tui/src/repl/runtime.rs`)
- **Concurrent fanout from inside the REPL.** `llm_query_batched(prompts, model=None)` runs up to 16 child completions in parallel via a new `POST /llm_batch` sidecar endpoint — much faster than serial `[llm_query(p) for p in prompts]`. `rlm_query_batched(prompts)` does the same for recursive RLM sub-calls via `POST /rlm_batch`. (`crates/tui/src/rlm/sidecar.rs`)
- **`SHOW_VARS()`** — returns `{name: type-name}` for every user variable in the REPL. Lets the model inspect what it has accumulated across rounds before deciding whether to call `FINAL_VAR(name)`.
- **Auto-persistence of REPL variables across rounds.** Any top-level JSON-serializable variable the sub-agent creates in a `repl` block now persists to the next round automatically — no `repl_set` ceremony needed unless you want explicit control. Matches the in-process reference REPL semantics.

### Changed
- **Code fence is `repl`, not `python`.** Matches the reference RLM language identifier so the same prompts and few-shot examples work here. Backward-compat fallback to `python` / `py` retained for older model behaviors.
- **`FINAL` / `FINAL_VAR` parseable from raw response text.** The reference RLM lets the model write `FINAL(value)` on its own line outside any code block to terminate the loop. Added `parse_text_final()` so that path works alongside the existing in-REPL Python sentinel mechanism. Code-fenced occurrences of `FINAL(...)` are correctly ignored to avoid false positives.
- **Strict termination loop.** The sub-agent must emit a ```repl block (or text-level FINAL) to make progress. One fence-less round triggers a reminder; two consecutive trigger a `RlmTermination::DirectAnswer` exit so we don't loop forever.
- **`rlm_process` separates `task` (root_prompt) from `file_path`/`content` (context).** The `task` rides along as `root_prompt` and is shown to the root LLM each iteration; the big input lives only in the REPL as `context`. Mirrors the reference's `completion(prompt, root_prompt=...)` API.
- **System prompt rewritten** with the reference's strategy patterns (PREVIEW → CHUNK + map-reduce via `llm_query_batched` → RECURSIVE decomposition via `rlm_query` → programmatic computation + LLM interpretation).
- The `/rlm` slash command stays for manual experimentation but is no longer the recommended path; the description in `commands/mod.rs` now points the model toward `rlm_process` for the in-agent flow.

### Reference
- Zhang, Kraska, Khattab. "Recursive Language Models." arXiv:2512.24601.
- alexzhang13/rlm — reference implementation by the paper authors. Variable names, helper surface, and code-fence convention align with that repo so prompts and patterns transfer.


### Fixed
- **`/rlm` actually recurses now (Algorithm 1 substrate, paper-faithful).** The v0.6.3 RLM loop had the right *shape* but its recursive substrate was non-functional: `llm_query()` was a Python stub that returned a hardcoded string, and `child_model` was bound with an underscore prefix and silently dropped. The loop ran but the sub-LLM never fired. v0.6.4 fixes this end-to-end:
  - **HTTP sidecar.** Each RLM turn spins up a localhost-only axum server on a kernel-assigned port for the duration of the turn. Python's `llm_query()` and `sub_rlm()` are real `urllib.request.urlopen` POSTs; Rust services them via the existing DeepSeek client and returns the completion text. No long-lived python process, no FIFOs, no two-pass replay — Python blocks on HTTP, Rust answers it. (`crates/tui/src/rlm/sidecar.rs`)
  - **`child_model` is plumbed through.** `Op::RlmQuery` and `AppAction::RlmQuery` carry the configured child model (default `deepseek-v4-flash`) all the way to the sidecar, where every `llm_query()` call uses it. Token usage is folded into `RlmTurnResult.usage` so cost tracking works.
  - **`sub_rlm()` is exposed as a paper-faithful recursive RLM call.** The Python REPL gets a real `sub_rlm(prompt)` function that runs another full Algorithm-1 turn at depth-1 inside the same process (different sidecar route, decremented recursion budget). Default `max_depth = 2` from the `/rlm` command — the model can recurse twice before the budget hits zero. The recursive opaque-future cycle (`run_rlm_turn_inner` → `start_sidecar` → `sub_rlm_handler` → `run_rlm_turn_inner`) is broken by returning a concrete `Pin<Box<dyn Future + Send>>` from `run_rlm_turn_inner`.
  - **Strict termination.** The loop only ends via `FINAL(value)` (or the iteration cap). The previous "no fence = direct answer, end loop" early-exit deviated from the paper and could short-circuit on iteration 1 with a chatty model that never saw `PROMPT`. The new behavior tolerates one fence-less round (with a reminder appended), then falls back to a `RlmTermination::DirectAnswer` exit. `RlmTurnResult` now carries a `termination: RlmTermination` enum (`Final | DirectAnswer | Exhausted | Error`) so callers can tell what happened.
  - **Richer `Metadata(state)`.** The metadata message the root LLM sees now includes paper-required *access patterns* (`repl_get`, slicing, `splitlines`, `repl_set`, `llm_query`, `sub_rlm`, `FINAL`) and a live list of variable keys currently in the REPL state file — so the model can see what it's accumulated across rounds without us shipping the values themselves.
  - **Unicode-safe truncation.** `truncate_text` now counts Unicode codepoints (was mixing `text.len()` bytes with `chars().take(n)`), so multi-byte previews can no longer mis-count. Per-turn temp state files are cleaned up on completion. `ROOM_TEMPERATURE` typo → `ROOT_TEMPERATURE`.
  - **End-to-end smoke test.** `rlm::turn::tests::sidecar_url_is_exported_to_python_env` stands up a stand-in axum server that always replies `{"text":"pong-from-sidecar"}`, runs `print(llm_query('hello'))` in the real `PythonRuntime`, and asserts the reply round-trips. This catches future regressions in the sidecar URL passthrough.

### Reference
- Zhang, Kraska, Khattab. "Recursive Language Models." arXiv:2512.24601 (Algorithm 1).


### Added
- **Sub-agents surface in the footer status strip.** When N > 0 sub-agents are in flight, the footer grows a "1 agent" / "N agents" chip in DeepSeek-sky color matching the model badge. Hides entirely at zero. (`footer_agents_chip` in `widgets/footer.rs`)
- **`@`-mention popup is fully wired in the composer.** Previously only the App state fields existed (`mention_menu_selected`, `mention_menu_hidden`). The popup now renders below the input mirror-style with the slash menu, with `@`-prefixed entries; Up/Down navigates, Enter / Tab apply the selection, Esc hides until the next input edit. Mention takes precedence over slash because the positional check is stricter. (`visible_mention_menu_entries` + `apply_mention_menu_selection` in `file_mention.rs`)

### Fixed
- **Tool-call cells no longer flash `<command>` / `<file>` placeholders.** The engine used to emit `ToolCallStarted` from `ContentBlockStart` with `input: {}` — before any `InputJsonDelta` had streamed in — which baked the placeholder into the cell at creation time. The emission is now deferred to `ContentBlockStop` and routed through `final_tool_input`, so the cell is created with the parsed args already in hand. (engine.rs `final_tool_input`; engine/tests.rs `final_tool_input_*`)
- **`parse_invocation_count` flake.** Two `markdown_render` tests both read the global PARSE_INVOCATIONS atomic and raced when other tests called `parse()` in parallel. Switched the counter to `thread_local!<Cell<u64>>`, so each test thread sees only its own invocations. Tested 8 sequential full-suite runs: 8/8 green (was ~40% green).

### Changed
- **System prompts redesigned with decomposition-first philosophy.** All four prompt tiers (base, agent, plan, yolo) now teach the model to decompose tasks before acting — `todo_write` first for granular task tracking, `update_plan` for high-level strategy, and sub-agents for parallelizable work. Inspired by the "mismanaged geniuses hypothesis" (Zhang et al., 2026): frontier LMs are already capable enough; the bottleneck is how we scaffold their self-management. The prompts now make work visible through the sidebar (Plan / Todos / Tasks / Agents) instead of letting the model work invisibly.
- **Tool labels use progressive verbs.** "Read foo.rs" → "Reading foo.rs", "List X" → "Listing X", "Search pattern" → "Searching for `pattern`", "List files" → "Listing files". Past-tense labels read wrong while a tool is still in flight; the new forms match what the user actually sees.
- **Long-running tools grow an elapsed badge.** From 3 s onward the `running` status segment becomes `running (3s)`, `running (4s)`, … so the user can tell a tool isn't stuck. The status-animation tick (360 ms) drives the redraw; below 3 s the badge stays hidden so quick reads/greps don't churn. (history.rs `running_status_label_with_elapsed`)
- **Spinner pulse is twice as fast** — `TOOL_STATUS_SYMBOL_MS` 1800 ms → 720 ms per glyph (full 4-glyph heartbeat in ~2.88 s instead of ~7.2 s).
- **`tools/subagent.rs` is now a folder module.** Tests live in `tools/subagent/tests.rs`; runtime + manager + tool implementations stay in `tools/subagent/mod.rs`. Public API unchanged. The runtime / tool-impl split was deferred — `SubAgentTask`, `run_subagent_task`, `build_allowed_tools`, the agent prompt constants, and `normalize_role_alias` are referenced from both layers and need a small API design pass before they cleanly separate.

### Test hygiene
- **5 regression tests pin auto-scroll churn contract.** `mark_history_updated` does not scroll; tool-cell handlers only `mark_history_updated`; `add_message` and `flush_active_cell` gate on `user_scrolled_during_stream`; the per-stream lock clears at TurnComplete and when the user returns to the live tail. (P2.4)

## [0.6.1] - 2026-04-26

### Changed
- **V4 cache-hit input prices cut to 1/10th per DeepSeek's pricing update.** Pro promo 0.03625→0.003625, Pro base 0.145→0.0145, Flash 0.028→0.0028 per 1M tokens. Cache-miss and output rates unchanged.
- **Removed the "light" theme option.** It was never tested, looked bad, and the dark/whale palettes are the supported targets. Theme validation now accepts only `default`, `dark`, and `whale`.
- **System prompts redesigned with decomposition-first philosophy.** All five prompt tiers teach the model to `todo_write` before acting, `update_plan` for strategy, and sub-agents for parallel work. Inspired by the mismanaged-geniuses hypothesis (Zhang et al., 2026).

## [0.6.0] - 2026-04-25

### Added
- **`rlm_query` tool — recursive language models as a first-class structured tool.** Inspired by [Alex Zhang's RLM work](https://github.com/alexzhang13/rlm) and Sakana AI's published novelty-search research, but trimmed to what an agent loop actually needs. The model calls `rlm_query` with one prompt or up to 16 concurrent prompts; children run on `deepseek-v4-flash` by default and can be promoted to Pro per-call. Children dispatch concurrently via `tokio::join_all` against the existing DeepSeek client — no external runtime, no fenced-block DSL, no Python sandbox. Returns plain text for one prompt, indexed `[0] ...\n\n---\n\n[1] ...` blocks for many. Available in Plan / Agent / YOLO. Cost is folded into the session's running total automatically.

### Changed
- **Scroll position survives content rewrites (#56).** `TranscriptScroll::resolve_top` and `scrolled_by` no longer teleport to bottom when the anchor cell vanishes. Three-level fallback chain: same line → same cell, line 0 → nearest surviving cell at-or-before. Previously, any rewrite of the assistant message (e.g. tool-result replacement) silently dropped the user back to the live tail mid-scroll.
- **Looser command-safety chains (#57).** `cargo build && cargo test`, `git fetch && git rebase`, and similar chains of known-safe commands now escalate to `RequiresApproval` instead of being hard-blocked as `Dangerous`. Chains containing unknown commands still block.
- **`GettingCrowded` no longer surfaces a footer chip.** The context-percent header already covers conversation pressure; the chip now only fires for active engine interventions (`refreshing context`, `verifying`, `resetting plan`).

## [0.5.2] - 2026-04-25

### Added
- **`/model` opens a Pro/Flash + thinking-effort picker (#39).** Typing `/model` with no argument now pops a two-pane modal: model on the left (`deepseek-v4-pro` flagship, `deepseek-v4-flash` fast/cheap, plus a "current (custom)" row when the active id isn't one of the listed defaults), and thinking effort on the right. Tab/←/→ swaps panes, ↑/↓ moves within the focused pane, Enter applies both selections, Esc cancels. The effort pane intentionally exposes only **Off / High / Max** because [DeepSeek's Thinking Mode docs](https://api-docs.deepseek.com/guides/reasoning_model) state `low`/`medium` are mapped to `high` server-side and `xhigh` is mapped to `max` — the legacy variants stay valid in `~/.deepseek/settings.toml` for back-compat, the picker just doesn't surface them. Apply path persists `default_model` and `reasoning_effort` to settings, forwards `Op::SetModel` + `Op::SetCompaction` to the running engine so the next turn picks up the change without a restart, and resets the per-turn token gauges (cache, replay) so the footer numbers reflect the new model. `/model <id>` keeps working unchanged for power users.

## [0.5.1] - 2026-04-25

### Added
- **`fetch_url` tool** for direct HTTP GET on a known URL — complements `web_search` for cases where the link is already known. Supports `format` (`markdown` / `text` / `raw`), `max_bytes` (default 1 MB, hard cap 10 MB), `timeout_ms` (default 15 s, max 60 s), redirect following, and structured `{url, status, content_type, content, truncated}` responses. 4xx/5xx bodies are returned (with `success: false`) so the caller can read JSON error envelopes. (#33)
- **PDF support in `read_file`.** PDFs are auto-detected by extension or `%PDF-` magic bytes and extracted via `pdftotext -layout` (poppler) when available. New optional `pages` arg (`"5"` or `"1-10"`) reads page slices. Without `pdftotext`, returns a structured `{type: "binary_unavailable", kind: "pdf", reason, hint}` with install commands for macOS/Debian. (#34)
- **Reasoning-content replay telemetry, end-to-end (#30).** The chat-completions sanitizer now estimates replayed `reasoning_content` tokens (~4 chars/token), threads the value through the streaming `Usage` payload, stores it on the App, and renders an `rsn N.Nk` chip in the footer next to the cache hit-rate. The chip turns warning-coloured when replay tokens exceed 50% of the input budget, so users on long thinking-mode loops can see at a glance how much of their context window is going to V4's "Interleaved Thinking" replay (paper §5.1.1). Logged at `RUST_LOG=deepseek_tui=info` for tail-friendly diagnosis.
- **`@file` Tab-completion (#28).** Typing `@<partial>` and pressing Tab now resolves the mention against the workspace using the existing `ignore::WalkBuilder`. A unique match is spliced into the input; multiple matches with a longer common prefix extend the partial; remaining ambiguity is surfaced via the status line. The mention-expansion path that ships file contents to the model is unchanged — this is purely a discovery aid for typing the path. Inline-contents and a fuzzy popup picker are queued for v0.5.2.
- **Per-workspace external trust list (#29).** `~/.deepseek/workspace-trust.json` now records, for each workspace, the absolute paths the user has opted into reading/writing from outside that workspace. The new `/trust` slash command supports `add <path>`, `remove <path>`, `list`, `on`, `off`, and a status read with no args; the engine consults the list when constructing every `ToolContext` so changes apply on the next tool call without restart. `/diagnostics` surfaces the list. The interactive "Allow once / Always allow / Deny" approval prompt is deferred — for now grant access ahead of the turn with `/trust add <path>`.

### Fixed
- **TUI sidebar gutter bleed regression test (#36).** Snapshot tests now lock in that long single-line tool results — including a `todo_write` echo of a multi-kilobyte JSON payload — never write any cells outside `chat_area` at the widths reported in the bug (80, 120, 165, 200 cols). A second test verifies the scrollbar coexists with content along the right edge instead of overdrawing the penultimate column.
- **Version drift caught in CI.** New `versions` job in `.github/workflows/ci.yml` runs `scripts/release/check-versions.sh` on every push/PR, verifying every per-crate `Cargo.toml` inherits the workspace version, the npm wrapper matches the workspace version, and `Cargo.lock` is in sync. The release runbook now lists `check-versions.sh` as the first preflight step. (#31)
- **Per-mode soft context budget for V4 compaction trigger** (#27).
- **Phantom `web.run` references stripped** from prompts and the `web_search` tool surface (#25).
- **Unused import + `cargo fmt` drift** that landed with `feat(#27)` and broke Build / Test / npm wrapper smoke under `-Dwarnings`.

## [0.5.0] - 2026-04-25

### Fixed
- Multi-turn tool calls on thinking-mode models no longer return HTTP 400. Every assistant message in the conversation now carries `reasoning_content` when thinking is enabled — not just tool-call rounds — matching DeepSeek's actual API validation, which rejects any assistant message missing the field even though the docs describe non-tool-call reasoning as "ignored".
- Added a final-pass wire-payload sanitizer in the chat-completions client that forces a non-empty `reasoning_content` placeholder onto any assistant message still missing one at request time. This is the last line of defense after engine-side and build-side substitution, so sessions restored from older checkpoints, sub-agents that append messages directly, and cached prefix mismatches all produce a valid request.
- On a `reasoning_content`-related 400, the client now logs the offending message indices to make future regressions diagnosable.
- Stripped phantom `web.run` references from prompts and the `web_search` tool surface ([#25](https://github.com/Hmbown/DeepSeek-TUI/issues/25)).

### Changed
- Header/UI widget refactor in the TUI (`crates/tui/src/tui/ui.rs`, `widgets/header.rs`) — internal cleanup, no user-visible behavior change.

## [0.4.9] - 2026-04-27

### Fixed
- DeepSeek thinking-mode tool-call rounds now always replay `reasoning_content` in all subsequent requests (including across new user turns), matching DeepSeek's documented API contract that assistant messages with tool calls must retain their reasoning content forever.
- Missing `reasoning_content` on a tool-call assistant message now substitutes a safe placeholder (`"(reasoning omitted)"`) instead of dropping the tool calls and their matching tool results, preventing orphaned conversation chains and API 400 errors.
- Session checkpoint now persists a Thinking-block placeholder for tool-call turns that produced no streamed reasoning text, keeping on-disk sessions structurally correct so subsequent requests avoid HTTP 400 rejections.
- Token estimation for compaction now counts thinking tokens across all tool-call rounds (not just the current user turn), aligning with the updated reasoning_content replay rule.

## [0.4.8] - 2026-04-25

### Fixed
- DeepSeek V4 Pro cost estimates now use DeepSeek's current limited-time 75% discount until 2026-05-05 15:59 UTC, then automatically fall back to the base Pro rates.

## [0.4.5] - 2026-04-24

### Fixed
- Alternate-screen TUI sessions now capture mouse input by default so wheel scrolling moves the transcript instead of exposing terminal scrollback from before the TUI started. Use `--no-mouse-capture` or `tui.mouse_capture = false` when terminal-native drag selection is preferred.

## [0.4.2] - 2026-04-24

### Fixed
- DeepSeek V4 thinking-mode tool turns now checkpoint the engine's authoritative API transcript, including assistant `reasoning_content` on reasoning-to-tool-call turns with no visible assistant text.
- Chat Completions request building now drops stale V4 tool-call rounds that are missing required `reasoning_content`, preventing old corrupted checkpoints from triggering DeepSeek HTTP 400 replay errors.
- Web search now falls back to Bing HTML results when DuckDuckGo returns a bot challenge or otherwise yields no parseable results.

## [0.4.1] - 2026-04-24

### Fixed
- DeepSeek V4 tool-result context now preserves large file reads and command outputs instead of compacting noisy tools to a 900-character snippet after 2k characters.
- Capacity guardrail refresh no longer performs destructive summary compaction unless the normal model-aware compaction thresholds are actually crossed.
- V4 compaction summaries retain larger tool-result excerpts and summary input when compaction is genuinely needed.
- The transcript now follows the bottom again when sending a new message, shows an in-app scrollbar when internally scrolled, and leaves mouse capture off in `--no-alt-screen` mode so terminal-native scrolling can work.

## [0.4.0] - 2026-04-23

### Added
- **DeepSeek V4 support**: `deepseek-v4-pro` (flagship) and `deepseek-v4-flash` (fast/cheap) are now first-class model IDs with 1M context windows.
- **Reasoning-effort tier**: new `reasoning_effort` config field (`off | low | medium | high | max`) mapped to DeepSeek's `reasoning_effort` + `thinking` request fields. Defaults to `max`.
- **Shift+Tab cycles reasoning-effort** through the three behaviorally distinct tiers (`off → high → max`). The current tier is shown as a ⚡ chip in the header.
- Per-model pricing table: `deepseek-v4-pro` priced at $0.145/$1.74/$3.48 per 1M tokens (cache-hit/miss/output); `deepseek-v4-flash` and legacy aliases at $0.028/$0.14/$0.28.

### Changed
- **Default model flipped to `deepseek-v4-pro`** (from `deepseek-reasoner`).
- `deepseek-chat` / `deepseek-reasoner` remain as silent aliases of `deepseek-v4-flash` for API compatibility; priced identically.
- **Context compaction**: 1M-context V4 models now compact at 800k input tokens or 2,000 messages, so short/tool-heavy sessions do not compact as if they were 128k-context runs.
- Cycling modes is now Tab-only; Shift+Tab is repurposed for reasoning-effort (reverse-mode cycle was low-value with only three modes).
- Updated help/hint strings, validator error messages, and the model picker to reference V4 IDs.

### Fixed
- `requires_reasoning_content` now recognizes `deepseek-v4*` so thinking streams render correctly on V4 models.
- DeepSeek V4 thinking-mode tool calls now preserve prior assistant `reasoning_content` whenever a tool call is replayed, matching DeepSeek's multi-turn contract and avoiding HTTP 400 rejections on later turns.
- Raw Chat Completions requests now send DeepSeek's top-level `thinking` parameter instead of the OpenAI SDK-only `extra_body` wrapper.
- Config, env, and UI model selection now normalize legacy DeepSeek aliases to `deepseek-v4-flash` instead of preserving old model labels.
- npm wrapper first-run downloads now use process-unique temp files so concurrent `deepseek` / `deepseek-tui` invocations do not race on `*.download` files.

## [0.3.33] - 2026-04-11

### Changed
- Footer polish: simplified footer rendering, removed footer clock label, updated status line layout
- Palette cleanup: removed `FOOTER_HINT` color constant

### Removed
- `FOOTER_HINT` color constant from palette (use `TEXT_MUTED` or `TEXT_HINT` instead)

### Fixed
- Test updates to align with simplified footer logic
- Empty state placeholder text removed for cleaner UI

## [0.3.32] - 2026-04-11

### Added
- Finance tool: Yahoo Finance v8 quote endpoint with chart fallback, supporting stocks, ETFs, indices, forex, and crypto lookups.
- Header widget redesign: proportional truncation, context-usage bar with gradient fill, streaming indicator, and graceful narrow-terminal degradation.
- Expanded test coverage: 680+ tests including footer state, context spans, plan prompt lifecycle, workspace context refresh, header rendering, and finance tool integration tests with wiremock.
- Workspace context refresh with configurable TTL and deferred initial fetch.
- Config command additions for runtime settings management.

### Changed
- Redesigned footer status strip with mode/model/status layout, context bar, and narrow-terminal fallback.
- Plan prompt now uses numeric selection (1-4) instead of keyword input; old aliases are sent as regular messages.
- Archived outdated docs (`workspace_migration_status.md` -> `docs/archive/`).
- Trimmed AGENTS.md boilerplate and updated task counts.
- Clarified release-surface documentation: crates.io publication may lag the workspace/npm wrapper.

### Fixed
- Header `metadata_spans` now uses `saturating_sub` to prevent underflow on narrow terminals.
- Finance tool reuses a single HTTP client instead of rebuilding per request.
- Finance tool tests no longer leak temp directories.

## [0.3.31] - 2026-03-08

### Added
- Replaced the finance tool backend with Yahoo Finance v8 + CoinGecko fallback for reliable real-time market data (stocks, ETFs, indices, forex, crypto).
- Added compaction UX: status strip shows animated COMPACTING indicator during context summarization, footer reflects compaction state, and CompactionCompleted events now include message count statistics.
- Added send flash: brief tinted background highlight on the last user message after sending.
- Added braille typing indicator with smooth 10-frame animation cycle.

### Changed
- Redesigned the footer status strip with mode/model/token/cost layout, quadrant separators, and a context-usage bar.
- Added Unicode prefix indicators (▸ You, ◆ Answer, ● System) to chat history cells for visual distinction.
- Improved thinking token delineation with labeled delimiters in transcript rendering.
- Refactored source code into workspace crates for better modularity and dependency management.

### Fixed
- Fixed Plan mode ESC key dismissing the prompt without clearing `plan_prompt_pending`, which prevented the prompt from reappearing on subsequent plan completions.
- Fixed clippy lint (collapsible_if) in web browsing session management.

## [0.3.30] - 2026-03-06

### Added
- Added a release-ready local npm smoke path that builds binaries, serves release assets locally, packs the wrapper, installs the tarball, and checks both entrypoints before publish.
- Added an opt-in full-matrix local release-asset fixture so `npm run release:check` can be exercised before GitHub release assets exist.

### Changed
- Bumped the Rust workspace crates and npm wrapper to `0.3.30`.
- Pointed the npm wrapper's default `deepseekBinaryVersion` at `0.3.30` for the next coordinated Rust + npm release.
- Updated the crates dry-run helper to work from a dirty workspace and to preflight dependent workspace crates without requiring unpublished versions to already exist on crates.io.

## [0.3.29] - 2026-03-03

### Added
- Added npm publish-time release asset verification for the `deepseek-tui` package to fail fast when expected GitHub binaries are missing.
- Added checksum manifests to GitHub release assets and checksum verification in the npm installer.
- Added `npm pack` install-and-smoke CI coverage for the `deepseek-tui` wrapper package.
- Added an end-to-end release runbook covering crates.io, GitHub Releases, and npm publication.

### Changed
- Updated npm package documentation for clearer install modes, environment overrides, and release integrity behavior.
- Improved installer support-matrix error messaging for unsupported platform/architecture combinations.
- Decoupled npm package version from default binary artifact version via `deepseekBinaryVersion`, enabling packaging-only npm releases.
- Moved the `deepseek-tui` binary target inside `crates/tui` so `cargo publish --dry-run -p deepseek-tui` works from the workspace package layout.
- Replaced the root-level crates publish workflow with an ordered workspace publish flow.
- Reworked first-run onboarding and README copy around primary workflows instead of shortcut memorization.
- Relaxed onboarding API-key format heuristics so unusual keys warn instead of blocking setup.

## [0.3.28] - 2026-03-02

### Added
- Converted the project to a modular Cargo workspace using a `crates/` layout.
- Added new crate boundaries mirroring a deepseek architecture (`agent`, `config`, `core`, `execpolicy`, `hooks`, `mcp`, `protocol`, `state`, `tools`, `tui-core`, `tui`, and `app-server`).

### Changed
- Added parity CI coverage with protocol/state/snapshot checks.
- Updated release workflow to build both `deepseek` and `deepseek-tui` binaries.

## [0.3.26] - 2026-03-02

### Fixed
- Resolved SSE stream corruption caused by byte/string position mismatch in streaming parse flow.
- Hardened base URL validation to reject non-HTTP/HTTPS schemes.
- Prevented multi-byte UTF-8 truncation panics in common-prefix and runtime thread summary paths.
- Corrected context usage alert thresholds by separating warning and critical trigger levels.

### Changed
- Removed non-code utility tools from the runtime tool registry (`calculator`, `weather`, `sports`, `finance`, `time`) and related wiring.
- Consolidated duplicate URL encoding helpers by delegating to shared `crate::utils::url_encode`.
- Replaced broad crate-level lint suppressions with targeted `#[allow(...)]` annotations where justified.
- Cleaned up dead APIs, unused struct fields, unused builder helpers, and non-integrated modules.
- Addressed clippy findings across the codebase (collapsible conditionals, defaults, indexing helpers, and API signature cleanup).

## [0.3.24] - 2026-02-25

### Fixed
- Preserve reasoning-only assistant turns for DeepSeek reasoning models (`deepseek-reasoner`, R-series markers) when rebuilding chat history.
- Align SSE tool streaming indices so each tool block start/delta/stop uses the same block index.
- Prevent transcript auto-scroll-to-bottom when a non-empty transcript selection is active.
- Allow session picker search mode to accept the current selection with a single `Enter` press.
- Preserve tool output whitespace/indentation while still wrapping long unbroken tokens.
- Make transcript selection copy/highlighting display-width aware (wide chars and tabs).
- Gate execpolicy behavior on the `exec_policy` feature flag across CLI/tool execution paths.
- Run doctor API connectivity checks using the effective loaded config/profile (instead of reloading defaults).
- Parse DeepSeek model context-window suffix hints such as `-32k` and `-256k`.
- Update README config docs with key environment overrides and a direct link to full configuration docs.

## [0.3.23] - 2026-02-24

### Changed
- Updated project copy to describe the app as a terminal-native TUI/CLI for DeepSeek models (not pinned to a specific model generation).

### Fixed
- Model selection and config validation now accept any valid `deepseek-*` model ID (including future releases), while still normalizing common aliases like `deepseek-v3.2` and `deepseek-r1`.
- Tool-call recovery now auto-loads deferred tools when the model requests them directly, instead of failing with manual `tool_search_*` instructions.
- YOLO mode now preloads tools by default (including deferred MCP tools), so model tool calls can run immediately without discovery indirection.
- Unknown tool-call failures now include discovery guidance and nearest tool-name suggestions instead of generic availability errors.
- Slash-command errors now suggest the closest known command (for example `/modle` -> `/model`) instead of only returning a generic unknown-command message.

## [0.3.22] - 2026-02-19

### Added
- Interactive `/config` editing modal for runtime settings updates.

### Changed
- Retired user-facing `/set` command path (no longer reachable/discoverable).
- Replaced `/deepseek` command behavior with `/links` (aliases: `dashboard`, `api`).

### Fixed
- Legacy `/set` and `/deepseek` inputs now return migration guidance instead of generic unknown-command errors.

## [0.3.21] - 2026-02-19

### Added
- Parallel tool execution in `multi_tool_use.parallel` for independent task workflows.
- Session resume-thread coverage in tests.

### Changed
- Desktop and web parity polish across the TUI and runtime surfaces.
- Onboarding and approval UX refinement from prior phase 3 iteration.

### Fixed
- Runtime pre-release startup issues and config-path edge cases.
- Clippy lint regressions introduced by the last parity pass.

### Security/Hardening
- General pre-release hardening for runtime app behavior.

## [0.3.17] - 2026-02-16

### Fixed
- Config loading now expands `~` in `DEEPSEEK_CONFIG_PATH` and `--config` paths.
- When `DEEPSEEK_CONFIG_PATH` points to a missing file, config loading now falls back to `~/.deepseek/config.toml` if it exists.

### Changed
- Removed committed transient runtime artifacts (`session_*.json`, `.deepseek/trusted`) and added ignore rules to prevent re-commit.

## [0.3.16] - 2026-02-15

### Added
- `deepseek models` CLI command to fetch and list models from the configured `/v1/models` endpoint (with `--json` output mode).
- `/models` slash command to fetch and display live model IDs in the TUI.
- Slash-command autocomplete hints in the composer plus `Tab` completion for `/` commands.
- Command palette modal (`Ctrl+K`) for quick insertion of slash commands and skills.
- Persistent right sidebar in wide terminals showing live plan/todo/sub-agent state.
- Expandable tool payload views (`v` in transcript, `v` in approval modal) for full params/output inspection.
- Runtime HTTP/SSE API (`deepseek serve --http`) with durable thread/turn/item lifecycle, interrupt/steer, and replayable event timeline.
- Background task queue (`/task add|list|show|cancel` and `POST /v1/tasks`) with persistent storage, bounded worker pool, and timeline/artifact tracking.

### Changed
- Centralized the default text model (`DEFAULT_TEXT_MODEL`) and shared common model list to reduce drift across runtime/config paths.
- `/model` now clarifies that any valid DeepSeek model ID is accepted (including future releases), while still showing common model IDs.

### Fixed
- Expanded reasoning-model detection for chat history reconstruction (supports R-series and reasoner-style naming without hardcoding single versions).
- Aligned docs/config examples with the then-current runtime default model.

## [0.3.14] - 2026-02-05

### Added
- `web.run` now supports `image_query` (DuckDuckGo image search)
- `multi_tool_use.parallel` now supports safe MCP meta tools (`list_mcp_resources`, `mcp_read_resource`, etc.)

### Fixed
- Encode tool-call function names when rebuilding Chat Completions history (keeps dotted tool names API-safe)

### Changed
- Prompts: stronger `web.run` citation placement and quote-limit guidance

## [0.3.13] - 2026-02-04

### Fixed
- Restore an in-app scrollbar for the transcript view

## [0.3.12] - 2026-02-04

### Fixed
- Map dotted tool names to API-safe identifiers for DeepSeek tool calls
- Encode any invalid tool names for API tool lists while preserving internal names

## [0.3.11] - 2026-02-04

### Fixed
- Fix tool name mapping for DeepSeek API

## [0.3.10] - 2026-02-04

### Fixed
- Always enable mouse wheel scrolling in the TUI (even without alt screen)

## [0.3.9] - 2026-02-04

### Removed
- RLM mode, tools, and documentation pending a faithful implementation of the MIT RLM design
- Duo mode tools and prompts pending a citable research spec

### Fixed
- Footer context usage bar remains visible while status toasts are shown

### Changed
- Updated prompts and docs to reflect the simplified mode/tool surface

## [0.3.8] - 2026-02-03

### Fixed
- Resolve clippy warnings (CI `-D warnings`) in new tool implementations

## [0.3.7] - 2026-02-03

### Added
- Tooling parity updates: `weather`, `finance`, `sports`, `time`, `calculator`, `request_user_input`, `multi_tool_use.parallel`, `web.run`
- Shell streaming helpers: `exec_shell_wait` and `exec_shell_interact`
- Sub-agent controls: `send_input` and `wait` (with aliases)
- MCP resource helpers: `list_mcp_resources`, `list_mcp_resource_templates`, and `read_mcp_resource` alias

### Changed
- Skills directory selection now prefers workspace `.agents/skills`, then `./skills`, then global
- Docs and prompts updated to reflect new tool surface and parity notes

## [0.3.6] - 2026-02-02

### Added
- New welcome banner on startup showing "Welcome to DeepSeek TUI!" with directory, session ID, and model info
- Visual context progress bar in footer showing usage with block characters [████░░░░░░] and percentage

### Changed
- Removed custom block-character scrollbar from chat area - now uses terminal's native scroll
- Simplified header bar: removed context percentage indicator (moved to footer as progress bar)

## [0.3.5] - 2026-01-30

### Added
- Intelligent context offloading: large tool results (>15k chars) are automatically moved to RLM memory to preserve the context window
- Persistent history context: compacted messages are offloaded to RLM `history` variable for recall
- Full MCP protocol support: SSE transport, Resources (`resources/list`, `resources/read`), and Prompts (`prompts/list`, `prompts/get`)
- `mcp_read_resource` and `mcp_get_prompt` virtual tools exposed to the model
- Dialectical Duo mode with specialized TUI rendering (`Player` / `Coach` history cells)
- Dynamic system prompt refreshing at each turn for up-to-date RLM/Duo/working-set context
- `project_map` tool for automatic codebase structure discovery
- `delegate_to_agent` alias for streamlined sub-agent delegation

### Changed
- Default theme changed to 'Whale' with updated color palette
- `with_agent_tools` now includes `project_map`, `test_runner`, and conditionally RLM tools for all agent modes
- MCP `McpServerConfig.command` is now `Option<String>` to support URL-only (SSE) servers

### Fixed
- MCP test compilation errors for updated `McpServerConfig` struct shape

## [0.3.4] - 2026-01-29

### Changed
- Updated Cargo.lock dependencies

### Fixed
- Compaction tool-call pairing: enforce bidirectional tool-call/tool-result integrity with fixpoint convergence
- Safety net scanning to drop orphan tool results in the request builder
- Double-dispatch race in parallel tool execution

## [0.3.3] - 2026-01-28

### Added
- TUI polish: Kimi-style footer with mode/model/token display
- Streaming thinking blocks with dedicated rendering
- Loading animation improvements

## [0.3.2] - 2026-01-28

### Fixed
- Preserve tool-call + tool-result pairing during compaction to avoid invalid tool message sequences
- Drop orphan tool results in request builder as a safety net to prevent API 400s

## [0.3.1] - 2026-01-27

### Added
- `deepseek setup` to bootstrap MCP config and skills directories
- `deepseek mcp init` to generate a template `mcp.json` at the configured path

### Changed
- `deepseek doctor` now follows the resolved config path and config-derived MCP/skills locations

### Fixed
- Doctor no longer reports missing MCP/skills when paths are overridden via config or env

## [0.3.0] - 2026-01-27

### Added
- Repo-aware working set tracking with prompt injection for active paths
- Working set signals now pin relevant messages during auto-compaction
- Offline eval harness (`deepseek eval`) with CI coverage in the test job
- Shell tool now emits stdout/stderr summaries and truncation metadata
- Dependency-aware `agent_swarm` tool for orchestrating multiple sub-agents
- Expanded sub-agent tool access (apply_patch, web_search, file_search)

### Changed
- Auto-compaction now accounts for pinned budget and preserves working-set context
- Apply patch tool validates patch shape, reports per-file summaries, and improves hunk mismatch diagnostics
- Eval harness shell step now uses a Windows-safe default command
- Increased `max_subagents` clamp to `1..=20`

## [0.2.2] - 2026-01-22

### Fixed
- Session save no longer panics on serialization errors
- Web search regex patterns are now cached for better performance
- Improved panic messages for regex compilation failures

## [0.2.1] - 2026-01-22

### Fixed
- Resolve clippy warnings for Rust 1.92

## [0.2.0] - 2026-01-20

### Changed
- Removed npm package distribution; now Cargo-only
- Clean up for public release

### Fixed
- Disabled automatic RLM mode switching; use /rlm or /aleph to enter RLM mode
- Fixed cargo fmt formatting issues

## [0.0.2] - 2026-01-20

### Fixed
- Disabled automatic RLM mode switching; use /rlm or /aleph to enter RLM mode.

## [0.0.1] - 2026-01-19

### Added
- DeepSeek Responses API client with chat-completions fallback
- CLI parity commands: login/logout, exec, review, apply, mcp, sandbox
- Resume/fork session workflows with picker fallback
- DeepSeek blue branding refresh + whale indicator
- Responses API proxy subcommand for key-isolated forwarding
- Execpolicy check tooling and feature flag CLI
- Agentic exec mode (`deepseek exec --auto`) with auto-approvals

### Changed
- Removed multimedia tooling and aligned prompts/docs for text-only DeepSeek API

## [0.1.9] - 2026-01-17

### Added
- API connectivity test in `deepseek doctor` command
- Helpful error diagnostics for common API failures (invalid key, timeout, network issues)

## [0.1.8] - 2026-01-16

### Added
- Renderable widget abstraction and modal view stack for TUI composition
- Parallel tool execution with lock-aware scheduling
- Interactive shell mode with terminal pause/resume handling

### Changed
- Tool approval requirements moved into tool specs
- Tool results are recorded in original request order

## [0.1.7] - 2026-01-15

### Added
- Duo mode (player-coach autocoding workflow)
- Character-level transcript selection

### Fixed
- Approval flow tool use ID routing
- Cursor position sync for transcript selection

## [0.1.6] - 2026-01-14

### Added
- Auto-RLM for large pasted blocks with context auto-load
- `chunk_auto` and `rlm_query` `auto_chunks` for quick document sweeps
- RLM usage badge with budget warnings in the footer

### Changed
- Auto-RLM now honors explicit RLM file requests even for smaller files

## [0.1.5] - 2026-01-14

### Added
- RLM prompt with external-context guidance and REPL tooling
- RLM tools for context loading, execution, status, and sub-queries (rlm_load, rlm_exec, rlm_status, rlm_query)
- RLM query usage tracking and variable buffers
- Workspace-relative `@path` support for RLM loads
- Auto-switch to RLM when users request large file analysis (or the largest file)

### Changed
- Removed Edit mode; RLM chat is default with /repl toggle

## [0.1.0] - 2026-01-12

### Added
- Initial alpha release of DeepSeek TUI
- Interactive TUI chat interface
- DeepSeek API integration (OpenAI-compatible Responses API)
- Tool execution (shell, file ops)
- MCP (Model Context Protocol) support
- Session management with history
- Skills/plugin system
- Cost tracking and estimation
- Hooks system and config profiles
- Example skills and launch assets

[Unreleased]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.39...HEAD
[0.8.39]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.38...v0.8.39
[0.8.38]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.37...v0.8.38
[0.8.37]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.36...v0.8.37
[0.8.36]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.35...v0.8.36
[0.8.35]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.34...v0.8.35
[0.8.34]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.33...v0.8.34
[0.8.33]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.32...v0.8.33
[0.8.32]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.31...v0.8.32
[0.8.31]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.30...v0.8.31
[0.8.30]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.29...v0.8.30
[0.8.29]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.28...v0.8.29
[0.8.28]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.27...v0.8.28
[0.8.27]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.26...v0.8.27
[0.8.26]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.25...v0.8.26
[0.8.25]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.24...v0.8.25
[0.8.24]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.23...v0.8.24
[0.8.23]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.22...v0.8.23
[0.8.22]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.21...v0.8.22
[0.8.21]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.20...v0.8.21
[0.8.20]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.19...v0.8.20
[0.8.19]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.18...v0.8.19
[0.8.18]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.17...v0.8.18
[0.8.17]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.16...v0.8.17
[0.8.16]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.15...v0.8.16
[0.8.15]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.13...v0.8.15
[0.8.13]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.12...v0.8.13
[0.8.12]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.11...v0.8.12
[0.8.11]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.10...v0.8.11
[0.8.10]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.8...v0.8.10
[0.8.8]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.7...v0.8.8
[0.8.7]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.6...v0.8.7
[0.8.6]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.5...v0.8.6
[0.8.5]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.4...v0.8.5
[0.8.4]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.3...v0.8.4
[0.8.3]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.2...v0.8.3
[0.8.2]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.1...v0.8.2
[0.8.1]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.0...v0.8.1
[0.8.0]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.7.9...v0.8.0
[0.7.9]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.7.8...v0.7.9
[0.7.8]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.7.7...v0.7.8
[0.7.7]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.7.6...v0.7.7
[0.7.6]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.7.5...v0.7.6
[0.6.1]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.4.9...v0.6.0
[0.4.9]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.4.8...v0.4.9
[0.4.8]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.3.33...v0.4.8
[0.3.33]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.3.32...v0.3.33
[0.3.32]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.3.31...v0.3.32
[0.3.31]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.3.28...v0.3.31
[0.3.28]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.3.27...v0.3.28
[0.3.23]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.3.22...v0.3.23
[0.3.22]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.3.21...v0.3.22
[0.3.21]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.3.17...v0.3.21
[0.3.17]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.3.16...v0.3.17
[0.3.16]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.3.14...v0.3.16
[0.3.14]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.3.13...v0.3.14
[0.3.13]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.3.12...v0.3.13
[0.3.12]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.3.11...v0.3.12
[0.3.11]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.3.10...v0.3.11
[0.3.10]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.3.6...v0.3.10
[0.3.6]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.3.5...v0.3.6
[0.3.5]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.3.4...v0.3.5
[0.3.4]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.3.3...v0.3.4
[0.3.3]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.3.2...v0.3.3
[0.3.2]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.2.2...v0.3.0
[0.2.2]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.2.0...v0.2.2
[0.2.0]: https://github.com/Hmbown/DeepSeek-TUI/releases/tag/v0.2.0
[0.0.2]: https://github.com/Hmbown/DeepSeek-TUI/releases/tag/v0.0.2
[0.0.1]: https://github.com/Hmbown/DeepSeek-TUI/releases/tag/v0.0.1
[0.1.9]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.1.8...v0.1.9
[0.1.8]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.1.7...v0.1.8
[0.1.7]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.1.6...v0.1.7
[0.1.6]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.1.5...v0.1.6
[0.1.5]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.1.0...v0.1.5
[0.1.0]: https://github.com/Hmbown/DeepSeek-TUI/releases/tag/v0.1.0
