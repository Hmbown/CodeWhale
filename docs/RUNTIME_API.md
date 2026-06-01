# Runtime API & Integration Contract

codewhale exposes a local runtime API through `codewhale serve --http` and
machine-readable health via `codewhale doctor --json`. It also exposes
`codewhale serve --acp` for editor clients that speak the Agent Client Protocol
over stdio. This document is the stable integration contract for native macOS
workbench applications (and other local supervisors) that embed the DeepSeek
engine without screen-scraping terminal output.

## Architecture

```
macOS workbench (or any local supervisor)
        │
        ├─ codewhale doctor --json   → machine-readable health & capability
        ├─ codewhale serve --http    → HTTP/SSE runtime API
        ├─ codewhale serve --acp     → ACP stdio agent for editors such as Zed
        ├─ codewhale serve --mcp     → MCP stdio server
        └─ codewhale [args]          → interactive TUI session
```

The engine runs as a local-only process. All APIs bind to `localhost` by
default. No hosted relay, no provider-token custody, no secret leakage.

## Choosing an integration mode

Use the mode that matches the surface your wrapper needs:

| Mode | Best fit | Tool access | Streaming | Message shape |
|---|---|---|---|---|
| `codewhale serve --http` | Local apps, web/chat backends, mobile or desktop supervisors | Tools run through normal thread/turn execution under the configured policy, with approval APIs for gated calls | SSE events via `/v1/threads/{id}/events` and compatibility streaming via `/v1/stream` | Structured thread, turn, task, approval, and event records |
| `codewhale exec --auto --output-format stream-json` | Non-interactive CLI wrappers and harnesses that need tool-backed output without running a daemon | Built-in tools plus configured MCP tools available to the non-interactive agent | Newline-delimited JSON on stdout | A single prompt string for the turn; role/history separation must be encoded by the caller |
| `codewhale serve --acp` | Editors and ACP-compatible clients such as Zed | Not yet exposed through ACP | ACP `session/update` chunks | ACP session prompt/update messages |
| `codewhale serve --mcp` | Other MCP clients that want to call CodeWhale as a tool server | Exposes CodeWhale tools to the external MCP client | No streaming; single synchronous request/response over stdio | MCP tool-call payloads |

For Feishu/Lark bots, custom web chats, or other long-running chat backends,
prefer `serve --http` when the client needs structured conversations,
approvals, interrupt/steer controls, event replay, or separate system prompt
state. Use `exec --auto --output-format stream-json` for simpler wrappers that
can encode their own context into one prompt and only need a tool-backed NDJSON
stream. Use ACP when the host already speaks Agent Client Protocol; the current
ACP adapter is deliberately chat-only and does not load MCP tools into the ACP
session.

## ACP stdio adapter: `codewhale serve --acp`

`codewhale serve --acp` speaks JSON-RPC 2.0 over newline-delimited stdio for
ACP-compatible editor clients. The initial adapter implements the ACP baseline:

- `initialize`
- `session/new`
- `session/prompt`
- `session/cancel`

Prompt requests are routed through the configured DeepSeek client and current
default model. Responses are emitted as `session/update` agent message chunks
followed by a `session/prompt` response with `stopReason: "end_turn"`.

The adapter is intentionally conservative: it does not yet expose shell tools,
file-write tools, checkpoint replay, or session loading through ACP. Use
`codewhale serve --http` for the full local runtime API and `codewhale serve --mcp`
when another client needs DeepSeek's tools as MCP tools.

## Capability endpoint: `codewhale doctor --json`

Returns a JSON object describing the current installation's readiness state.
Suitable for health-check polling from a macOS workbench.

```bash
codewhale doctor --json
```

### Response schema (key fields)

| Field | Type | Description |
|---|---|---|
| `version` | string | Installed version (e.g. `"0.8.9"`) |
| `config_path` | string | Resolved config file path |
| `config_present` | bool | Whether the config file exists |
| `workspace` | string | Default workspace directory |
| `api_key.source` | string | `env`, `config`, or `missing` |
| `base_url` | string | API base URL |
| `default_text_model` | string | Default model |
| `memory.enabled` | bool | Whether the memory feature is on |
| `memory.path` | string | Path to memory file |
| `memory.file_present` | bool | Whether memory file exists |
| `mcp.config_path` | string | MCP config file path |
| `mcp.present` | bool | Whether MCP config exists |
| `mcp.servers` | array | Per-server health: `{name, enabled, status, detail}` |
| `skills.selected` | string | Resolved skills directory |
| `skills.global.path` / `.present` / `.count` | — | CodeWhale global skills dir (`~/.codewhale/skills`, with legacy `~/.deepseek/skills` support) |
| `skills.agents.path` / `.present` / `.count` | — | Workspace `.agents/skills/` dir |
| `skills.agents_global.path` / `.present` / `.count` | — | agentskills.io global skills dir (`~/.agents/skills`) |
| `skills.local.path` / `.present` / `.count` | — | `skills/` dir |
| `skills.opencode.path` / `.present` / `.count` | — | `.opencode/skills/` dir |
| `skills.claude.path` / `.present` / `.count` | — | `.claude/skills/` dir |
| `tools.path` / `.present` / `.count` | — | Global tools directory |
| `plugins.path` / `.present` / `.count` | — | Global plugins directory |
| `sandbox.available` | bool | Whether sandbox is supported on this OS |
| `sandbox.kind` | string or null | Sandbox kind (e.g. `"macos_seatbelt"`) |
| `storage.spillover.path` / `.present` / `.count` | — | Tool output spillover dir |
| `storage.stash.path` / `.present` / `.count` | — | Composer stash |

### Example

```json
{
  "version": "0.8.9",
  "config_path": "/Users/you/.codewhale/config.toml",
  "config_present": true,
  "workspace": "/Users/you/projects/codewhale-tui",
  "api_key": {
    "source": "env"
  },
  "base_url": "https://api.deepseek.com/beta",
  "default_text_model": "deepseek-v4-pro",
  "memory": {
    "enabled": false,
    "path": "/Users/you/.codewhale/memory.md",
    "file_present": true
  },
  "mcp": {
    "config_path": "/Users/you/.codewhale/mcp.json",
    "present": true,
    "servers": [
      {"name": "filesystem", "enabled": true, "status": "ok", "detail": "ready"}
    ]
  },
  "sandbox": {
    "available": true,
    "kind": "macos_seatbelt"
  }
}
```

## HTTP/SSE runtime API: `codewhale serve --http`

```bash
codewhale serve --http [--host 127.0.0.1] [--port 7878] [--workers 2] [--auth-token TOKEN]
codewhale serve --mobile [--host 0.0.0.0] [--port 7878] [--auth-token TOKEN]
```

Defaults: host `127.0.0.1`, port `7878`, 2 workers (clamped 1–8).

The server binds to `localhost` by default. Configuration is via CLI flags —
there is no `[app_server]` config section.

`/v1/*` routes require a bearer token unless `--insecure` is explicitly set.
Pass `--auth-token TOKEN` or set `DEEPSEEK_RUNTIME_TOKEN=TOKEN` before starting
the server. If neither is set, the process generates a one-time token and prints
it at startup. `/health` and `/v1/runtime/info` remain public for local
supervision and bootstrap. `/mobile` returns 404 when mobile mode is disabled;
when mobile mode is enabled and auth is enabled, `/mobile` returns 401 unless
the request supplies the runtime token.

Authenticated clients can provide the token as `Authorization: Bearer TOKEN`,
`X-DeepSeek-Runtime-Token: TOKEN`, or `?token=TOKEN` for EventSource-style
clients that cannot set custom headers.

### Mobile control page

`codewhale serve --mobile` starts the same HTTP/SSE runtime API and serves a
phone-friendly control page at `/mobile`. When the bind host is left at the
default, mobile mode binds to `0.0.0.0`, prints a warning, and prints local/LAN
URLs. Pass `--host 127.0.0.1` to keep the mobile page loopback-only. If a
runtime token is generated or supplied, the printed mobile URL includes it as a
query parameter; the page stores it locally and removes it from the address bar.
The static HTML page contains no secrets, but it is still token-gated when auth
is enabled so unauthenticated LAN clients cannot fingerprint the mobile surface.

The mobile page can list/create threads, send prompts, follow live SSE events,
steer or interrupt an active turn, and resolve normal tool approvals through
`POST /v1/approvals/{approval_id}`. It is still a local/LAN convenience surface:
do not expose it directly to the public internet without TLS and a trusted
fronting layer.

### Endpoints

**Health**
- `GET /health`

**Sessions** (legacy session manager)
- `GET /v1/sessions?limit=50&search=<substring>`
- `GET /v1/sessions/{id}`
- `DELETE /v1/sessions/{id}`
- `POST /v1/sessions/{id}/resume-thread`

**Threads** (durable runtime data model)
- `GET /v1/threads?limit=50&include_archived=false&archived_only=false`
- `GET /v1/threads/summary?limit=50&search=<optional>&include_archived=false&archived_only=false`
- `POST /v1/threads`
- `GET /v1/threads/{id}`
- `PATCH /v1/threads/{id}` (see body shape below)
- `POST /v1/threads/{id}/resume`
- `POST /v1/threads/{id}/fork`

Thread forks are sibling runtime threads, not an in-place tree projection.
`thread.forked` events include `source_thread_id`; internal backtrack-aware
forks may also include `backtrack_depth_from_tail` and `dropped_turn_id`.
Thread list and summary responses remain flat in v0.8.40, so clients that need
a graph should reconstruct it from events instead of assuming list order is a
complete tree.

`archived_only=true` returns archived threads only (mutually overrides
`include_archived`). Default behavior is unchanged: `include_archived=false`
and `archived_only=false` returns active threads. Added in v0.8.10 (#563).

`PATCH /v1/threads/{id}` body — every field is optional, missing means
"no change". At least one field must be present. `title` and `system_prompt`
accept an empty string to clear a previously-set value. Added in v0.8.10 (#562):

```json
{
  "archived": true,
  "allow_shell": false,
  "trust_mode": false,
  "auto_approve": false,
  "model": "deepseek-v4-pro",
  "mode": "agent",
  "title": "User-set thread title",
  "system_prompt": "You are a useful assistant."
}
```

**Turns** (within a thread)
- `POST /v1/threads/{id}/turns`
- `POST /v1/threads/{id}/turns/{turn_id}/steer`
- `POST /v1/threads/{id}/turns/{turn_id}/interrupt`
- `POST /v1/threads/{id}/compact` (manual compaction)

**Approvals**
- `POST /v1/approvals/{approval_id}` with body
  `{ "decision": "allow" | "deny", "remember": false }`

**Events** (SSE replay + live stream)
- `GET /v1/threads/{id}/events?since_seq=<u64>`

**Compatibility stream** (one-shot, backwards-compatible)
- `POST /v1/stream`

**Tasks** (durable background work)
- `GET /v1/tasks`
- `POST /v1/tasks`
- `GET /v1/tasks/{id}`
- `POST /v1/tasks/{id}/cancel`

**Automations** (scheduled recurring work)
- `GET /v1/automations`
- `POST /v1/automations`
- `GET /v1/automations/{id}`
- `PATCH /v1/automations/{id}`
- `DELETE /v1/automations/{id}`
- `POST /v1/automations/{id}/run`
- `POST /v1/automations/{id}/pause`
- `POST /v1/automations/{id}/resume`
- `GET /v1/automations/{id}/runs?limit=20`

**Introspection**
- `GET /v1/workspace/status`
- `GET /v1/skills`
- `GET /v1/apps/mcp/servers`
- `GET /v1/apps/mcp/tools?server=<optional>`

**Usage** (token/cost aggregation across threads)
- `GET /v1/usage?since=<rfc3339>&until=<rfc3339>&group_by=<day|model|provider|thread>`

`since` / `until` are inclusive RFC 3339 timestamps and may be omitted (no
bound). `group_by` defaults to `day`. Buckets are sorted by ascending key.
Empty time ranges produce empty `buckets` (never a 404). Cost is computed via
the model→pricing map; turns whose model has no pricing entry contribute
tokens but `0.0` cost. Added in v0.8.10 (#564).

```json
{
  "since": "2026-04-01T00:00:00Z",
  "until": "2026-04-30T23:59:59Z",
  "group_by": "day",
  "totals": {
    "input_tokens": 12345,
    "output_tokens": 6789,
    "cached_tokens": 0,
    "reasoning_tokens": 0,
    "cost_usd": 0.012,
    "turns": 42
  },
  "buckets": [
    {
      "key": "2026-04-30",
      "input_tokens": 1234,
      "output_tokens": 678,
      "cached_tokens": 0,
      "reasoning_tokens": 0,
      "cost_usd": 0.001,
      "turns": 3
    }
  ]
}
```

## Runtime data model

The runtime uses a durable Thread/Turn/Item lifecycle.

- **ThreadRecord** — `id`, `created_at`, `updated_at`, `model`, `workspace`,
  `mode`, `task_id`, `coherence_state`, `system_prompt`, `latest_turn_id`,
  `latest_response_bookmark`, `archived`
- **TurnRecord** — `id`, `thread_id`, `status` (`queued|in_progress|completed|
  failed|interrupted|canceled`), timestamps, duration, usage, error summary
- **TurnItemRecord** — `id`, `turn_id`, `kind` (`user_message|agent_message|
  tool_call|file_change|command_execution|context_compaction|status|error`),
  lifecycle `status`, `metadata`

Events are append-only with a global monotonic `seq` for replay/resume.

### Restart semantics

- If the process restarts while a turn or item is `queued` or `in_progress`,
  the recovered record is marked `interrupted` with an `"Interrupted by
  process restart"` error.
- Task execution performs its own recovery on top of the same persisted
  thread/turn store.

### Approval model

- The `auto_approve` flag applies to the runtime approval bridge and engine
  tool context. When enabled for a thread/turn/task, approval-required tools
  are auto-approved in the non-interactive runtime path, shell safety checks
  run in auto-approved mode, and spawned sub-agents inherit that setting.
- When omitted, `auto_approve` defaults to `false`.

### SSE event stream

The SSE event payload shape for `/v1/threads/{id}/events`:

```json
{
  "schema_version": 1,
  "seq": 42,
  "event": "item.delta",
  "kind": "item.delta",
  "thread_id": "thr_1234abcd",
  "turn_id": "turn_5678efgh",
  "item_id": "item_90ab12cd",
  "timestamp": "2026-02-11T20:18:49.123Z",
  "created_at": "2026-02-11T20:18:49.123Z",
  "payload": {
    "delta": "partial output",
    "kind": "agent_message"
  }
}
```

Compatibility notes:

- `schema_version` is the HTTP/SSE envelope schema version. It is independent of
  the runtime store schema used for persisted thread/turn/event records.
- `event` remains the SSE event name in existing clients; it is preserved as-is.
- `kind` mirrors `event` in the stable envelope for typed clients.
- `thread.started`, `turn.started`, and `turn.completed` are emitted as SSE event
  names exactly as before.
- `timestamp` remains the canonical event time for schema version 1. `created_at`
  is an equivalent alias for clients that use `created_at` naming elsewhere; do
  not require both fields to be present.

Common event names: `thread.started`, `thread.forked`, `turn.started`,
`turn.lifecycle`, `turn.steered`, `turn.interrupt_requested`,
`turn.completed`, `item.started`, `item.delta`, `item.completed`,
`item.failed`, `item.interrupted`, `approval.required`, `approval.decided`,
`approval.timeout`, `sandbox.denied`, `coherence.state`.

## Security boundary

- **Localhost by default**. The server binds to `127.0.0.1` by default.
  `--mobile` binds to `0.0.0.0` when no host is supplied so phones on the same
  LAN can reach it, and the CLI prints a warning for that rebind. Pass
  `--host 127.0.0.1` for a loopback-only mobile page. Set a non-loopback host
  only when you trust the network path or have a reverse-proxy / VPN that
  authenticates. The runtime does not provide user isolation or TLS.
- **Optional token guard**. `--auth-token` or `DEEPSEEK_RUNTIME_TOKEN`
  requires a matching bearer token for `/v1/*` routes. This is a local
  convenience guard, not a replacement for TLS, VPN, or a trusted reverse
  proxy on public networks.
- **No provider-token custody**. The server never returns the API key. The
  `api_key.source` capability field reports `env`, `config`, or `missing` —
  never the key itself.
- **No hosted relay**. The app-server is a local process under the user's
  control. There is no cloud component.
- **Capability responses** never leak secrets, file contents, or session
  message bodies. They report *metadata*: presence, counts, status flags.

### CORS allow-list

The runtime API ships with a built-in dev-origin allow-list:
`http://localhost:3000`, `http://127.0.0.1:3000`, `http://localhost:1420`,
`http://127.0.0.1:1420`, `tauri://localhost`. To add additional origins (e.g.
when developing a UI on Vite's default `:5173`), use any of:

- CLI flag (repeatable): `codewhale serve --http --cors-origin http://localhost:5173`
- Env var (comma-separated): `DEEPSEEK_CORS_ORIGINS="http://localhost:5173,http://localhost:8080"`
- Config (`~/.codewhale/config.toml`):
  ```toml
  [runtime_api]
  cors_origins = ["http://localhost:5173"]
  ```

User-supplied origins **stack on top of** the built-in defaults; they do not
replace them. Wildcard origins are not supported — the explicit allow-list
model is preserved. Added in v0.8.10 (#561).

## Session lifecycle (native UI supervision)

| Operation | Endpoint |
|---|---|
| List sessions | `GET /v1/sessions` |
| Get session | `GET /v1/sessions/{id}` |
| Delete session | `DELETE /v1/sessions/{id}` |
| Resume into thread | `POST /v1/sessions/{id}/resume-thread` |
| Create thread | `POST /v1/threads` |
| List threads | `GET /v1/threads` |
| Attach to events | `GET /v1/threads/{id}/events?since_seq=0` |
| Send message | `POST /v1/threads/{id}/turns` |
| Steer | `POST /v1/threads/{id}/turns/{turn_id}/steer` |
| Interrupt | `POST /v1/threads/{id}/turns/{turn_id}/interrupt` |
| Compact | `POST /v1/threads/{id}/compact` |

## Compatibility tests

Contract snapshots live in `crates/protocol/tests/`. Run:

```bash
cargo test -p codewhale-protocol --test parity_protocol --locked
```

This validates that the app-server's event schema hasn't drifted from the
documented contract. CI runs this on every push to `main` and on release tags.
