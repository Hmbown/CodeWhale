# Configuration

DeepSeek TUI reads configuration from a TOML file plus environment variables.
At process startup it also loads a workspace-local `.env` file when present.
Use the tracked `.env.example` as the template; copy it to `.env`, then edit
only the provider and safety knobs you need.

## Where It Looks

Default config path:

- `~/.deepseek/config.toml`

Overrides:

- CLI: `deepseek --config /path/to/config.toml`
- Env: `DEEPSEEK_CONFIG_PATH=/path/to/config.toml`

If both are set, `--config` wins. Environment variable overrides are applied after the file is loaded.

### Per-project overlay (#485)

When the TUI starts in a workspace that contains a
`<workspace>/.deepseek/config.toml` file, the values declared in that
file are merged on top of the global config. This lets a repo lock its
own provider, model, sandbox policy, or approval policy without
touching the user's `~/.deepseek/config.toml`. Pass
`--no-project-config` to skip the overlay for one launch.

Supported keys in the project overlay (top-level fields only):

| Key | Effect |
|---|---|
| `provider` | switch backend (e.g. `"nvidia-nim"` for an enterprise repo) |
| `model` | override `default_text_model` |
| `api_key` | use a per-repo key (typically read from `.env`, **not committed**) |
| `base_url` | point at a self-hosted endpoint |
| `reasoning_effort` | force `"high"` / `"max"` for a complex repo |
| `approval_policy` | `"never"` / `"on-request"` / `"untrusted"` for opinionated repos |
| `sandbox_mode` | `"read-only"` / `"workspace-write"` / `"danger-full-access"` |
| `mcp_config_path` | per-repo MCP server set |
| `notes_path` | keep notes in-repo |
| `max_subagents` | clamp concurrency for a constrained repo (clamped to 1..=20) |
| `allow_shell` | gate shell tool access on `false` |

The overlay is intentionally narrow — it covers the fields a repo
maintainer is most likely to want to standardize across contributors.
Other settings (skills_dir, hooks, capacity, retry, etc.) stay
user-global. If your repo needs more, file an issue describing the
specific use case.

The `deepseek` facade and `deepseek-tui` binary share the same config file for
DeepSeek auth and model defaults. `deepseek auth set --provider deepseek` (and
the legacy `deepseek login --api-key ...` alias) saves the key to
`~/.deepseek/config.toml`, and `deepseek --model deepseek-v4-flash` is forwarded
to the TUI as `DEEPSEEK_MODEL`.

Credential lookup uses `config -> keyring -> env` after any explicit CLI
`--api-key`. Run `deepseek auth status` to inspect the active provider's config
file, OS keyring backend, environment variable, winning source, and last-four
label without printing the key itself. The command only probes the active
provider's keyring entry.

For hosted, generic OpenAI-compatible, or self-hosted providers, set
`provider = "nvidia-nim"`, `"openai"`, `"fireworks"`, `"sglang"`, `"vllm"`, or
`"ollama"` or pass `deepseek --provider <name>`. The facade saves provider
credentials to the shared user config and forwards the resolved key, base URL,
provider, and model to the TUI process. Use
`deepseek auth set --provider nvidia-nim --api-key "YOUR_NVIDIA_API_KEY"` or
`deepseek auth set --provider openai --api-key "YOUR_OPENAI_COMPATIBLE_API_KEY"` or
`deepseek auth set --provider fireworks --api-key "YOUR_FIREWORKS_API_KEY"` to
save provider keys through the facade. The generic `openai` provider defaults
to `https://api.openai.com/v1`, accepts `OPENAI_BASE_URL`, and passes model IDs
through unchanged for OpenAI-compatible gateways. SGLang, vLLM, and Ollama are
self-hosted and can run without an API key by default. Ollama defaults to
`http://localhost:11434/v1` and sends model tags such as `deepseek-coder:1.3b`
or `qwen2.5-coder:7b` unchanged.

Third-party OpenAI-compatible gateways that need extra request headers can set
`http_headers = { "X-Model-Provider-Id" = "your-model-provider" }` at the top
level or under a provider table such as `[providers.deepseek]`. When configured,
DeepSeek TUI sends those custom headers on model API requests. The equivalent
environment override is `DEEPSEEK_HTTP_HEADERS`, using comma-separated
`name=value` pairs such as
`X-Model-Provider-Id=your-model-provider,X-Gateway-Route=dev`. `Authorization`
and `Content-Type` are managed by the client and are not overridden by this
setting.

To bootstrap MCP and skills directories at their resolved paths, run `deepseek-tui setup`.
To only scaffold MCP, run `deepseek-tui mcp init`.

Note: setup, doctor, mcp, features, sessions, resume/fork, exec, review, and eval
are subcommands of the `deepseek-tui` binary. The `deepseek` dispatcher exposes a
distinct set of commands (`auth`, `config`, `model`, `thread`, `sandbox`,
`app-server`, `mcp-server`, `completion`) and forwards plain prompts to
`deepseek-tui`.

## Profiles

You can define multiple profiles in the same file:

```toml
api_key = "PERSONAL_KEY"
default_text_model = "deepseek-v4-pro"

[profiles.work]
api_key = "WORK_KEY"
base_url = "https://api.deepseek.com/beta"

[profiles.nvidia-nim]
provider = "nvidia-nim"
api_key = "NVIDIA_KEY"
base_url = "https://integrate.api.nvidia.com/v1"
default_text_model = "deepseek-ai/deepseek-v4-pro"

[profiles.fireworks]
provider = "fireworks"
default_text_model = "accounts/fireworks/models/deepseek-v4-pro"

[profiles.openai-compatible]
provider = "openai"

[profiles.openai-compatible.providers.openai]
base_url = "https://openai-compatible.example/v4"
model = "glm-5"

[profiles.sglang]
provider = "sglang"
base_url = "http://localhost:30000/v1"
default_text_model = "deepseek-ai/DeepSeek-V4-Pro"

[profiles.vllm]
provider = "vllm"
base_url = "http://localhost:8000/v1"
default_text_model = "deepseek-ai/DeepSeek-V4-Pro"

[profiles.ollama]
provider = "ollama"
base_url = "http://localhost:11434/v1"
default_text_model = "deepseek-coder:1.3b"
```

Select a profile with:

- CLI: `deepseek --profile work`
- Env: `DEEPSEEK_PROFILE=work`

If a profile is selected but missing, DeepSeek TUI exits with an error listing available profiles.

## Environment Variables

Most runtime environment variables override config values. API-key variables are
fallbacks after saved config and keyring credentials:

- `DEEPSEEK_API_KEY`
- `DEEPSEEK_BASE_URL`
- `DEEPSEEK_HTTP_HEADERS` (custom model request headers, comma-separated `name=value` pairs)
- `DEEPSEEK_PROVIDER` (`deepseek|nvidia-nim|openai|openrouter|novita|fireworks|sglang|vllm|ollama`)
- `DEEPSEEK_MODEL` or `DEEPSEEK_DEFAULT_TEXT_MODEL`
- `DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS` (stream idle timeout in seconds; default `300`, clamped to `1..=3600`)
- `DEEPSEEK_STREAM_OPEN_TIMEOUT_SECS` (connection setup + response-header wait in seconds; default `45`, clamped to `5..=300`; distinct from the per-chunk idle timeout)
- `NVIDIA_API_KEY` or `NVIDIA_NIM_API_KEY` (preferred when provider is `nvidia-nim`; falls back to `DEEPSEEK_API_KEY`)
- `NVIDIA_NIM_BASE_URL`, `NIM_BASE_URL`, or `NVIDIA_BASE_URL`
- `NVIDIA_NIM_MODEL`
- `OPENAI_API_KEY`
- `OPENAI_BASE_URL`
- `OPENAI_MODEL`
- `OPENROUTER_API_KEY`
- `OPENROUTER_BASE_URL`
- `NOVITA_API_KEY`
- `NOVITA_BASE_URL`
- `FIREWORKS_API_KEY`
- `FIREWORKS_BASE_URL`
- `SGLANG_BASE_URL`
- `SGLANG_MODEL`
- `SGLANG_API_KEY` (optional; many localhost SGLang servers do not require auth)
- `VLLM_BASE_URL`
- `VLLM_MODEL`
- `VLLM_API_KEY` (optional; many localhost vLLM servers do not require auth)
- `OLLAMA_BASE_URL`
- `OLLAMA_MODEL`
- `OLLAMA_API_KEY` (optional; many localhost Ollama servers do not require auth)
- `DEEPSEEK_LOG_LEVEL` or `RUST_LOG` (`info`/`debug`/`trace` enables lightweight verbose logs)
- `DEEPSEEK_SKILLS_DIR`
- `DEEPSEEK_MCP_CONFIG`
- `DEEPSEEK_NOTES_PATH`
- `DEEPSEEK_MEMORY` (`1|on|true|yes|y|enabled` turns user memory on)
- `DEEPSEEK_MEMORY_PATH`
- `DEEPSEEK_ALLOW_SHELL` (`1`/`true` enables)
- `DEEPSEEK_APPROVAL_POLICY` (`on-request|untrusted|never`)
- `DEEPSEEK_SANDBOX_MODE` (`read-only|workspace-write|danger-full-access|external-sandbox`)
- `DEEPSEEK_MANAGED_CONFIG_PATH`
- `DEEPSEEK_REQUIREMENTS_PATH`
- `DEEPSEEK_MAX_SUBAGENTS` (clamped to `1..=20`)
- `DEEPSEEK_TASKS_DIR` (runtime task queue/artifact storage, default `~/.deepseek/tasks`)
- `DEEPSEEK_ALLOW_INSECURE_HTTP` (`1`/`true` allows non-local `http://` base URLs; default is reject)
- `DEEPSEEK_FORCE_HTTP1` (`1|true|yes|on` pins the HTTP client to HTTP/1.1, disabling HTTP/2; useful on Windows or behind proxies that mishandle long-lived H2 streams)
- `DEEPSEEK_HOME` (override the base data directory; defaults to `~/.deepseek`)
- `DEEPSEEK_AUTOMATIONS_DIR` (override the automations storage directory; defaults to `~/.deepseek/automations`)
- `DEEPSEEK_CAPACITY_ENABLED`
- `DEEPSEEK_CAPACITY_LOW_RISK_MAX`
- `DEEPSEEK_CAPACITY_MEDIUM_RISK_MAX`
- `DEEPSEEK_CAPACITY_SEVERE_MIN_SLACK`
- `DEEPSEEK_CAPACITY_SEVERE_VIOLATION_RATIO`
- `DEEPSEEK_CAPACITY_REFRESH_COOLDOWN_TURNS`
- `DEEPSEEK_CAPACITY_REPLAN_COOLDOWN_TURNS`
- `DEEPSEEK_CAPACITY_MAX_REPLAY_PER_TURN`
- `DEEPSEEK_CAPACITY_MIN_TURNS_BEFORE_GUARDRAIL`
- `DEEPSEEK_CAPACITY_PROFILE_WINDOW`
- `DEEPSEEK_CAPACITY_PRIOR_CHAT`
- `DEEPSEEK_CAPACITY_PRIOR_REASONER`
- `DEEPSEEK_CAPACITY_PRIOR_V4_PRO`
- `DEEPSEEK_CAPACITY_PRIOR_V4_FLASH`
- `DEEPSEEK_CAPACITY_PRIOR_FALLBACK`
- `NO_ANIMATIONS` (`1|true|yes|on` forces `low_motion = true` and
  `fancy_animations = false` at startup, regardless of the saved
  settings; see [`docs/ACCESSIBILITY.md`](./ACCESSIBILITY.md)).
- `SSL_CERT_FILE` — corporate-proxy / TLS-inspecting MITM users
  point this at a PEM bundle (or single DER cert) and the cert(s)
  get added alongside the platform's system trust store. Failures
  log a warning and continue — the existing system roots still
  apply.

### Instruction sources (`instructions = [...]`, #454)

Add a list of additional system-prompt sources that get
concatenated, in declared order, alongside the auto-loaded
`AGENTS.md`:

```toml
instructions = [
    "./AGENTS.md",
    "~/.deepseek/global.md",
    "~/team/agents-shared.md",
]
```

Rules:

- Paths run through `expand_path` so `~` and env vars work.
- Each file is capped at 100 KiB; oversized files are
  truncated with a `[…elided]` marker rather than skipped.
- Missing files are skipped with a tracing warning so a stale
  entry doesn't fail the launch.
- Project config (`<workspace>/.deepseek/config.toml`)
  **replaces** the user array wholesale rather than merging.
  If you want both, list `~/global.md` inside the project
  array. Set `instructions = []` in the project to clear the
  user list for that repo.

### `/hooks` listing

Run `/hooks` (or `/hooks list`) inside the TUI to see every
configured lifecycle hook grouped by event, including each
hook's name, command preview, timeout, and condition. The
`[hooks].enabled` flag's state is shown at the top so it's
obvious when hooks are globally suppressed. Hooks are
configured under `[[hooks.hooks]]` entries — see the existing
hook-system documentation for the full schema.

### Composer stash (`/stash`, Ctrl+S)

Press **Ctrl+S** in the composer to park the current draft to
`~/.deepseek/composer_stash.jsonl`. `/stash list` shows parked
drafts with one-line previews and timestamps; `/stash pop`
restores the most recently parked draft (LIFO); `/stash clear`
wipes the file. Capped at 200 entries; multiline drafts
round-trip intact.

## Settings File (Persistent UI Preferences)

DeepSeek TUI also stores user preferences in:

- `~/.config/deepseek/settings.toml`

Notable settings include `auto_compact` (default `false`), which opts into
replacement-style summarization only near the active model limit. The default
V4 path preserves the stable message prefix for cache reuse; use manual
`/compact` or enable `auto_compact` only when you explicitly want automatic
replacement compaction. You can inspect or update these from the TUI with
`/settings` and `/config` (interactive editor).

Common settings keys:

- `theme` (default, dark, light, whale)
- `auto_compact` (on/off, default off)
- `paste_burst_detection` (on/off, default on): fallback rapid-key paste
  detection for terminals that do not emit bracketed-paste events. This is
  independent of terminal bracketed-paste mode.
- `show_thinking` (on/off)
- `show_tool_details` (on/off)
- `locale` (`auto`, `en`, `ja`, `zh-Hans`, `pt-BR`; default `auto`): UI chrome
  locale. `auto` checks `LC_ALL`, `LC_MESSAGES`, then `LANG`; unsupported or
  missing locales fall back to English. The runtime also exposes the resolved
  locale in the system prompt as the fallback natural language for V4 reasoning
  and replies when the latest user message is ambiguous. Clear user language
  still takes priority; Chinese turns should produce Chinese `reasoning_content`
  and Chinese final replies even when the resolved locale is English.
- `background_color` (`#RRGGBB`, `RRGGBB`, or `default`): optional main TUI
  background color applied to the root, header, transcript, and footer
  surfaces while preserving panel contrast.
- `cost_currency` (`usd`, `cny`; default `usd`): currency used by the footer,
  context panel, `/cost`, `/tokens`, and long-turn notification summaries. The
  aliases `rmb` and `yuan` normalize to `cny`.
- `default_mode` (agent, plan, yolo; legacy `normal` is accepted and normalized to `agent`)
- `max_history` (number of submitted input history entries; cleared drafts are
  also kept locally for composer history search)
- `default_model` (model name override)

Only `agent`, `plan`, and `yolo` are visible modes in the UI. Switch between
them with `/mode`. For compatibility, older settings files with
`default_mode = "normal"` still load as `agent`.

Localization scope is tracked in [LOCALIZATION.md](LOCALIZATION.md). The v0.7.6
core pack covers high-visibility TUI chrome only; provider/tool schemas,
personality prompts, and full documentation remain English unless explicitly
translated later.

Readability semantics:

- Selection uses a unified style across transcript, composer menus, and modals.
- Footer hints use a dedicated semantic role (`FOOTER_HINT`) so hint text stays readable across themes.
- The footer includes a compact `coherence` chip that describes how stable and
  focused the current session is right now. Possible states are `healthy`,
  `crowded`, `refreshing`, `verifying`, and `resetting`; these are derived from
  capacity and compaction events without exposing internal formulas in normal UI.

### Token Quantities and Drivers

DeepSeek V4 prefix caching makes token labels matter. These quantities are kept
separate:

| Quantity | Meaning | Allowed to drive |
|---|---|---|
| Active request input estimate | Conservative estimate of the next request's live system prompt and transcript payload. | Header/footer context percent, hard-cycle trigger, opt-in Flash seam trigger, and emergency overflow preflight. |
| Reserved response headroom | The internal turn budget plus safety headroom. v0.8.16 keeps normal turns at `262144` reserved output tokens and adds `1024` safety tokens for context-window checks, even though V4 capability metadata reports the official `384000` max output. | Hard-cycle and emergency overflow budget checks only. |
| Cumulative API usage | Provider-reported input plus output tokens summed across completed API calls; multi-tool turns may count the same stable prefix more than once. | Session usage and approximate cost telemetry only. |
| Prompt cache hit/miss | Provider cache telemetry for the most recent call when available. | Cache-hit display and cost estimation only; never compaction, seam, or cycle triggers. |
| Context percent | Active request input estimate divided by the model context window. | Display only; it mirrors the active-input basis used by context safeguards. |
| Cost estimate | Approximate spend from provider usage and configured DeepSeek rates. | Display only. |

For the default V4 path, hard cycles fire when active input reaches the smaller
of the configured cycle threshold (`768000`) and the model window minus reserved
response headroom. Replacement compaction remains opt-in (`auto_compact = false`
by default), the Flash seam manager remains opt-in (`[context].enabled = false`),
and the capacity controller remains disabled unless configured.

### Command Migration Notes

If you are upgrading from older releases:

- Old: `/deepseek`
  New: `/links` (aliases: `/dashboard`, `/api`)
- Old: `/set model deepseek-reasoner`
  New: `/config` and edit the `model` row to `deepseek-v4-pro` or `deepseek-v4-flash`
- Old: visible `Normal` mode or `default_mode = "normal"`
  New: use `Agent` / `default_mode = "agent"`; legacy `normal` still maps to `agent`
- Old: discover `/set` in slash UX/help
  New: use `/config` for editing and `/settings` for read-only inspection

## Key Reference

### Core keys (used by the TUI/engine)

- `provider` (string, optional): `deepseek` (default), `nvidia-nim`, `openai`, `openrouter`, `novita`, `fireworks`, `sglang`, `vllm`, or `ollama`. Legacy `deepseek-cn` configs are still accepted as an alias for `deepseek`; DeepSeek uses the same official host [`https://api.deepseek.com`](https://api-docs.deepseek.com/) worldwide. `nvidia-nim` targets NVIDIA's NIM-hosted DeepSeek endpoints through `https://integrate.api.nvidia.com/v1`; `openai` targets a generic OpenAI-compatible endpoint, defaulting to `https://api.openai.com/v1`; `fireworks` targets `https://api.fireworks.ai/inference/v1`; `sglang` targets a self-hosted OpenAI-compatible endpoint, defaulting to `http://localhost:30000/v1`; `vllm` targets a self-hosted vLLM OpenAI-compatible endpoint, defaulting to `http://localhost:8000/v1`; `ollama` targets Ollama's OpenAI-compatible endpoint, defaulting to `http://localhost:11434/v1`.
- `api_key` (string, required for hosted providers): must be non-empty for DeepSeek/hosted providers (or set the provider API key env var). Self-hosted SGLang, vLLM, and Ollama can omit it.
- `base_url` (string, optional): defaults to `https://api.deepseek.com/beta` for DeepSeek's OpenAI-compatible Chat Completions API, including legacy `provider = "deepseek-cn"` configs, `https://api.openai.com/v1` for `provider = "openai"`, or the provider-specific endpoint for hosted/self-hosted providers. Set `https://api.deepseek.com` or `https://api.deepseek.com/v1` explicitly to opt out of DeepSeek beta features.
- `default_text_model` (string, optional): defaults to `deepseek-v4-pro` for DeepSeek, `deepseek-ai/deepseek-v4-pro` for NVIDIA NIM, `gpt-4.1` for generic OpenAI-compatible endpoints, `accounts/fireworks/models/deepseek-v4-pro` for Fireworks, `deepseek-ai/DeepSeek-V4-Pro` for SGLang/vLLM, and `deepseek-coder:1.3b` for Ollama. Current public DeepSeek IDs are `deepseek-v4-pro` and `deepseek-v4-flash`, both with 1M context windows, 384K max output, and thinking mode enabled by default. Legacy `deepseek-chat` and `deepseek-reasoner` remain compatibility aliases for `deepseek-v4-flash` until July 24, 2026. Provider-specific mappings translate `deepseek-v4-pro` / `deepseek-v4-flash` to each provider's model ID where supported. Generic `openai` and Ollama model IDs are passed through unchanged. OpenRouter provider configs with a custom `base_url` also preserve explicit model values, which lets OpenAI-compatible gateways accept bare model IDs. Use `/models` or `deepseek models` to discover live IDs from your configured endpoint. `DEEPSEEK_MODEL` overrides this for a single process.
- `reasoning_effort` (string, optional): `off`, `low`, `medium`, `high`, or `max`; defaults to the configured UI tier. DeepSeek Platform receives top-level `thinking` / `reasoning_effort` fields. NVIDIA NIM receives equivalent settings through `chat_template_kwargs`.
- `allow_shell` (bool, optional): defaults to `true` (sandboxed).
- `approval_policy` (string, optional): `on-request`, `untrusted`, or `never`. Runtime `approval_mode` editing in `/config` also accepts `on-request` and `untrusted` aliases.
- `sandbox_mode` (string, optional): `read-only`, `workspace-write`, `danger-full-access`, `external-sandbox`.
  Platform support is not identical. macOS uses Seatbelt for policy
  enforcement. Linux support is helper-gated around Landlock. Windows does not
  currently advertise an OS sandbox; the planned Windows helper contract starts
  with process-tree containment only and must not be described as read-only
  filesystem isolation, workspace-write enforcement, network blocking,
  registry isolation, or AppContainer isolation until those are implemented.
- `managed_config_path` (string, optional): managed config file loaded after user/env config.
- `requirements_path` (string, optional): requirements file used to enforce allowed approval/sandbox values.
- `max_subagents` (int, optional): defaults to `10` and is clamped to `1..=20`.
- `subagents.*` (optional): per-role/type model defaults for `agent_spawn` and
  related sub-agent tools. Explicit tool `model` values win, then role/type
  overrides, then the parent runtime model. Supported convenience keys are
  `default_model`, `worker_model`, `explorer_model`, `awaiter_model`,
  `review_model`, `custom_model`, and `max_concurrent`. The
  `[subagents] max_concurrent` value overrides top-level `max_subagents` and is
  also clamped to `1..=20`. `[subagents.models]` accepts lower-case role or type
  keys such as `worker`, `explorer`, `general`, `explore`, `plan`, and
  `review`. Values must normalize to a supported DeepSeek model id before an
  agent is spawned.
- `skills_dir` (string, optional): defaults to `~/.deepseek/skills` (each skill is a directory containing `SKILL.md`). Workspace-local `.agents/skills` or `./skills` are preferred when present; the runtime also discovers global agentskills.io-compatible `~/.agents/skills` and the broader Claude-ecosystem `~/.claude/skills`.
- `mcp_config_path` (string, optional): defaults to `~/.deepseek/mcp.json`.
  It is visible in `/config` and can be changed from the TUI. The new path is
  used immediately by `/mcp`, but rebuilding the model-visible MCP tool pool
  requires restarting the TUI.
- `notes_path` (string, optional): defaults to `~/.deepseek/notes.txt` and is used by the model-visible `note` tool.
- `[memory].enabled` (bool, optional): defaults to `false`. When `true`,
  the TUI loads the user memory file into a `<user_memory>` prompt block,
  enables `# foo` quick-capture in the composer, surfaces the `/memory`
  slash command, and registers the `remember` tool. The same toggle is
  available via `DEEPSEEK_MEMORY=on`.
- `memory_path` (string, optional): defaults to `~/.deepseek/memory.md`.
  Used by the user-memory feature when enabled — see
  [`MEMORY.md`](MEMORY.md) for the full feature surface (`# foo`
  composer prefix, `/memory` slash command, `remember` tool, opt-in
  toggle).
- `snapshots.*` (optional): side-git workspace snapshots for file rollback:
  - `[snapshots].enabled` (bool, default `true`)
  - `[snapshots].max_age_days` (int, default `7`)
  - snapshots live under `~/.deepseek/snapshots/<project_hash>/<worktree_hash>/.git` and never use the workspace's own `.git` directory
- `context.*` (optional): append-only Flash seam manager, currently opt-in.
  Thresholds use the active request input estimate, not lifetime summed API
  usage:
  - `[context].enabled` (bool, default `false`)
  - `[context].verbatim_window_turns` (int, default `16`)
  - `[context].l1_threshold` (int, default `192000`)
  - `[context].l2_threshold` (int, default `384000`)
  - `[context].l3_threshold` (int, default `576000`)
  - `[context].cycle_threshold` (int, default `768000`)
  - `[context].seam_model` (string, default `deepseek-v4-flash`)
- `retry.*` (optional): retry/backoff settings for API requests:
  - `[retry].enabled` (bool, default `true`)
  - `[retry].max_retries` (int, default `3`)
  - `[retry].initial_delay` (float seconds, default `1.0`)
  - `[retry].max_delay` (float seconds, default `60.0`)
  - `[retry].exponential_base` (float, default `2.0`)
- `capacity.*` (optional): runtime context-capacity controller. This is opt-in
  because its active interventions can rewrite the live transcript.
  - `[capacity].enabled` (bool, default `false`)
  - `[capacity].low_risk_max` (float, default `0.50`)
  - `[capacity].medium_risk_max` (float, default `0.62`)
  - `[capacity].severe_min_slack` (float, default `-0.25`)
  - `[capacity].severe_violation_ratio` (float, default `0.40`)
  - `[capacity].refresh_cooldown_turns` (int, default `6`)
  - `[capacity].replan_cooldown_turns` (int, default `5`)
  - `[capacity].max_replay_per_turn` (int, default `1`)
  - `[capacity].min_turns_before_guardrail` (int, default `4`)
  - `[capacity].profile_window` (int, default `8`)
  - `[capacity].deepseek_v3_2_chat_prior` (float, default `3.9`)
  - `[capacity].deepseek_v3_2_reasoner_prior` (float, default `4.1`)
  - `[capacity].deepseek_v4_pro_prior` (float, default `3.5`)
  - `[capacity].deepseek_v4_flash_prior` (float, default `4.2`)
  - `[capacity].fallback_default_prior` (float, default `3.8`)
- `[notifications].method` (string, optional): `auto`, `osc9`, `bel`, or
  `off`. Defaults to `auto`. The TUI fires this on completed (successful)
  turns whose elapsed time meets `threshold_secs`; failed and cancelled
  turns are silent. `auto` resolves to `osc9` for `iTerm.app`, `Ghostty`,
  and `WezTerm` (detected via `$TERM_PROGRAM`). Otherwise the fallback is
  `bel` on macOS / Linux and `off` on Windows (where BEL maps to the
  system error chime — see the [Notifications](#notifications) section
  for the full rationale, #583).
- `[notifications].threshold_secs` (int, optional): defaults to `30`.
  Only completed turns whose elapsed time meets or exceeds this fire a
  notification.
- `[notifications].include_summary` (bool, optional): defaults to
  `false`. When `true`, the notification body includes the elapsed
  duration and the turn's cost in the configured display currency.
- `tui.alternate_screen` (string, optional): `auto`, `always`, or `never`. This is retained for config compatibility, but interactive sessions now always use the TUI-owned alternate screen so host terminal scrollback cannot hijack the viewport.
- `tui.mouse_capture` (bool, optional, default `true` on non-Windows terminals when the alternate screen is active; `false` on Windows and inside JetBrains JediTerm — PyCharm/IDEA/CLion/etc. — where mouse-event escapes leak into the input stream as garbled text, see #878 / #898): enable internal mouse scrolling, transcript selection, right-click context actions, and transcript scrollbar dragging. TUI-owned drag selection copies only transcript text and keeps selection scoped to the transcript pane. Set this to `false` or run with `--no-mouse-capture` for raw terminal selection; set it to `true` or run with `--mouse-capture` to opt in anywhere it's defaulted off. On Windows, raw terminal selection may cross the right sidebar because the terminal, not the TUI, owns the selection.
- `tui.terminal_probe_timeout_ms` (int, optional, default `500`): startup terminal-mode probe timeout in milliseconds. Values are clamped to `100..=5000`; timeout emits a warning and aborts startup instead of hanging indefinitely.
- `tui.osc8_links` (bool, optional, default `true`): emit OSC 8 escape sequences around URLs in transcript output so terminals that support them (iTerm2, Terminal.app 13+, Ghostty, Kitty, WezTerm, Alacritty, recent gnome-terminal/konsole) render them as Cmd+click hyperlinks. Terminals without OSC 8 support render the plain URL and ignore the escape. Set `false` for terminals that misrender the sequence; selection/clipboard output always strips the escapes.
- `hooks` (optional): lifecycle hooks configuration (see `config.example.toml`).
- `features.*` (optional): feature flag overrides (see below).

### Workspace notes

`/note` manages a simple notes file in the current workspace at
`.deepseek/notes.md`. Existing `/note <text>` usage still appends a note.
The management forms are:

| Command | Action |
|---|---|
| `/note <text>` | Append a note (legacy shorthand) |
| `/note add <text>` | Append a note explicitly |
| `/note list` | List notes with temporary 1-based numbers |
| `/note show <n>` | Show the full note at number `n` |
| `/note edit <n> <text>` | Replace note `n` with new text |
| `/note remove <n>` | Delete note `n`; `rm` and `delete` are aliases |
| `/note clear` | Empty the workspace notes file |
| `/note path` | Show the resolved workspace notes path |

The numbers shown by `/note list` are not stored in the file; they are derived
from the current order each time notes are read. This keeps the file format
compatible with the existing `---`-separated notes.

### User memory

User memory is split across one top-level path setting and one opt-in
toggle table:

```toml
memory_path = "~/.deepseek/memory.md"

[memory]
enabled = true
```

Notes:

- `memory_path` stays at the top level beside `notes_path` and
  `skills_dir`; it is not nested under `[memory]`.
- `DEEPSEEK_MEMORY_PATH` overrides the file path from the environment.
- `DEEPSEEK_MEMORY=on` (also `1`, `true`, `yes`, `y`, or `enabled`)
  flips the feature on without editing `config.toml`.
- The feature is inert when disabled: no file is injected, `# foo`
  falls through to normal message submission, and the model does not
  see the `remember` tool.
- See [`MEMORY.md`](MEMORY.md) for examples and the full `/memory`
  command surface.

### Notifications

The TUI can emit a desktop notification (OSC 9 escape or plain BEL) when a turn **completes successfully** and took longer than a threshold, so you can tab away while a long task runs. Failed or cancelled turns are intentionally silent — the notification is a "your task is ready" cue, not a generic ping. Configuration lives under `[notifications]`:

```toml
[notifications]
method          = "auto"  # auto | osc9 | bel | off
threshold_secs  = 30      # only notify when the turn took >= this many seconds
include_summary = false   # include elapsed time + cost in the notification body
```

Method semantics:

- `auto` (default) — picks `osc9` for `iTerm.app`, `Ghostty`, and `WezTerm` (detected via `$TERM_PROGRAM`). On macOS and Linux it falls back to `bel`. **On Windows the fallback is `off`** instead of `bel`, because the Windows audio stack maps `\x07` to the `SystemAsterisk` / `MB_OK` chime — the same sound application error popups use, so a successful-turn notification ends up sounding like an error (#583).
- `osc9` — emit `\x1b]9;<msg>\x07`. Inside tmux the sequence is wrapped in DCS passthrough so it reaches the outer terminal.
- `bel` — emit a single `\x07` byte. Use this on Windows only if you actively want the chime back.
- `off` — disable post-turn notifications entirely.

Windows users who run inside a known OSC-9 terminal (e.g. WezTerm on Windows) keep getting OSC-9 notifications; the `off` fallback only applies when no recognised `TERM_PROGRAM` is detected.

### Parsed but currently unused (reserved for future versions)

These keys are accepted by the config loader but not currently used by the interactive TUI or built-in tools:

- `tools_file`

## Feature Flags

Feature flags live under the `[features]` table and are merged across profiles.
Defaults are enabled for built-in tooling, so you only need to set entries you
want to force on or off.

```toml
[features]
shell_tool = true
subagents = true
web_search = true # enables canonical web.run plus the compatibility web_search alias
apply_patch = true
mcp = true
exec_policy = true
```

You can also override features for a single run:

- `deepseek-tui --enable web_search`
- `deepseek-tui --disable subagents`

Use `deepseek-tui features list` to inspect known flags and their effective state.

## Local Media Attachments

Use `@path/to/file` in the composer to add local text file or directory context
to the next message. Use `/attach <path>` for local image/video media paths, or
`Ctrl+V` to attach an image from the clipboard. DeepSeek's public Chat
Completions API currently accepts text message content, so media attachments are
sent as explicit local path references instead of native image/video payloads.
Attachment rows appear above the composer before submit; move to the start of
the composer, press `↑` to select an attachment row, then press `Backspace` or
`Delete` to remove it without editing the placeholder text by hand.

## Managed Configuration and Requirements

DeepSeek TUI supports a policy layering model:

1. user config + profile + env overrides
2. managed config (if present)
3. requirements validation (if present)

By default on Unix:
- managed config: `/etc/deepseek/managed_config.toml`
- requirements: `/etc/deepseek/requirements.toml`

Requirements file shape:

```toml
allowed_approval_policies = ["on-request", "untrusted", "never"]
allowed_sandbox_modes = ["read-only", "workspace-write"]
```

If configured values violate requirements, startup fails with a descriptive error.

See `docs/capacity_controller.md` for formulas, intervention behavior, and telemetry.

## Notes On `deepseek-tui doctor`

`deepseek-tui doctor` follows the same config resolution rules as the rest of the
TUI. That means `--config` / `DEEPSEEK_CONFIG_PATH` are respected, and MCP/skills
checks use the resolved `mcp_config_path` / `skills_dir` (including env overrides).

To bootstrap missing MCP/skills paths, run `deepseek-tui setup --all`. You can
also run `deepseek-tui setup --skills --local` to create a workspace-local
`./skills` dir.

`deepseek-tui doctor --json` prints a machine-readable report that skips the
live API connectivity probe. Top-level keys: `version`, `config_path`,
`config_present`, `workspace`, `api_key.source`, `base_url`,
`default_text_model`, `mcp`, `skills`, `tools`, `plugins`, `sandbox`,
`platform`, `api_connectivity`, `capability`. CI consumers should rely on `api_key.source`
(`env`/`config`/`missing`) rather than parsing the human-readable `doctor`
text.

The `capability` key contains per-provider capability info derived from
static knowledge (release docs, API guides) rather than live API probes.
Top-level sub-keys: `resolved_provider`, `resolved_model`, `context_window`,
`max_output`, `thinking_supported`, `cache_telemetry_supported`,
and `request_payload_mode`.

Use `capability.context_window` and `capability.max_output` for model-limit
checks in CI scripts; do not treat `capability.max_output` as the per-turn
request budget. Use `capability.thinking_supported` to decide whether to
configure reasoning effort.

## Setup status, clean, and extension dirs

`deepseek-tui setup` accepts a few flags beyond the existing `--mcp`,
`--skills`, `--local`, `--all`, and `--force`:

- `--status` — print a compact one-screen status (api key, base URL, model,
  MCP/skills/tools/plugins counts, sandbox, `.env` presence). Read-only and
  network-free; safe to run in CI. If `.env` is missing and `.env.example` is
  present in the workspace, the status output points at `cp .env.example .env`.
- `--tools` — scaffold `~/.deepseek/tools/` with a `README.md` describing the
  self-describing frontmatter convention (`# name:` / `# description:` /
  `# usage:`) and an `example.sh` that follows it. The directory is
  intentionally not auto-loaded; wire individual scripts into the agent via
  MCP, hooks, or skills.
- `--plugins` — scaffold `~/.deepseek/plugins/` with a `README.md` and an
  `example/PLUGIN.md` placeholder using the same frontmatter shape as
  `SKILL.md`. Plugins are not loaded automatically either; reference them
  from a skill or MCP wrapper when you want them active.
- `--all` now scaffolds MCP + skills + tools + plugins together.
- `--clean` — list `~/.deepseek/sessions/checkpoints/latest.json` and
  `offline_queue.json` if they exist. Pass `--force` to actually remove them.
  This never touches real session history or the task queue.

`--status` and `--clean` are mutually exclusive with the scaffold flags.

## Why the engine strips XML/`[TOOL_CALL]` text

DeepSeek TUI sends and receives tool calls only over the API tool channel
(structured `tool_use` / `tool_call` items). The streaming loop in
`crates/tui/src/core/engine.rs` recognizes a fixed set of fake-wrapper start
markers — `[TOOL_CALL]`, `<deepseek:tool_call`, `<tool_call`, `<invoke `,
`<function_calls>` — and scrubs them from visible assistant text without ever
turning them into structured tool calls. When a wrapper is stripped, the loop
emits one compact `status` notice per turn so the user can see why their
visible text shrank. Treat any change that re-enables text-based tool
execution as a regression; the protocol-recovery tests in
`crates/tui/tests/protocol_recovery.rs` lock the contract.
