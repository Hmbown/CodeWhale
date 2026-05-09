# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

[Unreleased]: https://github.com/Hmbown/DeepSeek-TUI/compare/v0.8.0...HEAD
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
