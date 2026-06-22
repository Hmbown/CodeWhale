# Runtime API & Integration Contract

`codewhale app-server` is the canonical local runtime API and control plane.
Local SDKs, benchmark supervisors, mobile/remote-control clients, and editor
integrations talk to it instead of screen-scraping terminal output. It serves
the full HTTP/SSE runtime API (`/v1/*`), a JSON-RPC control transport over
stdio, and the phone-friendly mobile page. `codewhale doctor --json` provides
machine-readable health, and `codewhale serve --acp` speaks the Agent Client
Protocol over stdio for editors such as Zed.

`codewhale serve --http` / `serve --mobile` remain as **compatibility aliases**
for `codewhale app-server --http` / `--mobile`; both launch the identical
server. New integrations should target `app-server`.

`codewhale exec` is the separate one-shot headless worker path (stream-json,
fleet worker subprocess, CI/benchmark primitive). It is not part of this API,
but it shares the same runtime, provider/model resolution, permission profiles,
and event vocabulary.

This document is the stable integration contract for native workbench
applications (and other local supervisors) that embed the DeepSeek engine.

## Architecture

```
local supervisor / SDK / benchmark harness
        │
        ├─ codewhale app-server --http     → HTTP/SSE runtime API (/v1/*)        [canonical]
        ├─ codewhale app-server --mobile   → runtime API + mobile control page
        ├─ codewhale app-server --stdio    → JSON-RPC control transport over stdio
        ├─ codewhale doctor --json         → machine-readable health & capability
        ├─ codewhale serve --acp           → ACP stdio agent for editors such as Zed
        ├─ codewhale serve --mcp           → MCP stdio server
        ├─ codewhale serve --http/--mobile → legacy aliases for `app-server --http/--mobile`
        └─ codewhale exec [args]           → one-shot headless worker (stream-json)
```

The engine runs as a local-only process. All APIs bind to `localhost` by
default. No hosted relay, no provider-token custody, no secret leakage.

For a proposed read-only audit export over completed turns, see
[`docs/RECEIPTS.md`](RECEIPTS.md). That document is a protocol note; the receipt
CLI/API surfaces are not implemented yet.

## Runtime API entrypoints

| Entry | Transport | Use |
|---|---|---|
| `codewhale app-server --http` | HTTP/SSE on `127.0.0.1:7878` | Full `/v1/*` runtime API (canonical) |
| `codewhale app-server --mobile` | HTTP/SSE on `0.0.0.0:7878` + `/mobile` | Runtime API + phone control page |
| `codewhale app-server --stdio` | JSON-RPC 2.0 over stdio | Local SDK / benchmark control probe (no listener) |
| `codewhale app-server` | HTTP on `127.0.0.1:8787` | Legacy in-process app-server (`/healthz`, `/thread`, `/app`, `/prompt`, `/tool`, `/jobs`) |
| `codewhale serve --http` / `--mobile` | same server as `app-server --http`/`--mobile` | Compatibility aliases |

`app-server --http` and `--mobile` launch the same mature runtime API server
historically reached through `serve --http` — no routes or behavior changed, so
every endpoint documented below is identical across both entrypoints. The
runtime API token is read from `--auth-token`, then `CODEWHALE_RUNTIME_TOKEN`,
then `DEEPSEEK_RUNTIME_TOKEN`; use `--insecure-no-auth` only with a loopback
bind. The `serve` compatibility aliases keep their `--insecure` flag.
The legacy in-process `codewhale app-server` also requires an explicit
`--auth-token` or `CODEWHALE_APP_SERVER_TOKEN` before binding a non-loopback
host; its generated one-time `cwapp_*` token is loopback-only.

The `--stdio` control transport is newline-delimited JSON-RPC 2.0. Probe it
without spending model tokens:

```bash
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"healthz"}' \
  '{"jsonrpc":"2.0","id":2,"method":"capabilities"}' \
  '{"jsonrpc":"2.0","id":3,"method":"shutdown"}' \
  | codewhale app-server --stdio
```

`capabilities` returns the advertised method families (`thread/*`, `app/*`,
`prompt/*`) and the full method list; `thread/capabilities`,
`app/capabilities`, and `prompt/capabilities` scope it per family. The method
set is pinned by a drift test in `crates/app-server/src/lib.rs`, so SDK and
benchmark clients can rely on it not changing silently.

## Benchmarking & SDK contract

The app-server exists so an external benchmark or SDK can answer — without
scraping TUI output — *what route ran, which provider/model/reasoning/permission
profile was effective, what events happened, how many tokens were used, and how
the run finished.* The durable Thread/Turn/Item data model already carries most
of this; the table maps each benchmark need to where a harness reads it.

| Benchmark need | Where it comes from | Status |
|---|---|---|
| Route / effective model | `TurnRecord` + thread `model`; per-run `--provider`/`--model` overrides | available |
| Permission / sandbox / approval profile | thread `auto_approve`, sandbox + approval policy | available |
| Run / thread / turn IDs | `thread_id`, `turn_id`, SSE event envelope | available |
| Event stream | `GET /v1/threads/{id}/events` (replay + live SSE) | available |
| Turn status / terminal classification | `TurnRecord.status` + error summary | available |
| Token usage | `TurnRecord.usage`; aggregate via `GET /v1/usage` | available |
| Single-read run receipt (route + usage + cost) | `GET /v1/threads/{id}/turns/{turn_id}/receipt` | proposed ([RECEIPTS.md](RECEIPTS.md)) |

For one-shot/headless benchmark runs, prefer `codewhale exec` with explicit
`--provider <id> --model <id>` so a failure identifies the exact provider/model
pair. Use `app-server` when the harness needs to start/resume/steer/interrupt
turns, list models/capabilities, follow the event stream, or read usage. Both
paths share the same runtime, so route-effective model resolution and the event
vocabulary match.

### Release smoke

`scripts/release/app-server-smoke.sh` is the committed pre-release check:

```bash
scripts/release/app-server-smoke.sh                 # stdio health/capabilities probe (no tokens)
scripts/release/app-server-smoke.sh --matrix        # + print the configured provider/model matrix
scripts/release/app-server-smoke.sh --matrix --real # + exec a cheap sentinel per provider
```

The stdio probe runs against a throwaway config, so it never reads real keys.
The matrix discovers configured providers from `codewhale auth list`, maps each
to a cheap model (override per provider with `SMOKE_MODEL_<SLUG>`), skips
unconfigured providers, and fails loudly on unmapped ones. `auth list` reports
presence flags only and exec output is passed through a redactor, so secrets are
never printed. The parser is covered by
`scripts/release/app-server-smoke.test.sh` against a fake `codewhale` binary.

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

## HTTP/SSE runtime API: `codewhale app-server --http`

```bash
codewhale app-server --http [--host 127.0.0.1] [--port 7878] [--workers 2] [--auth-token TOKEN] [--insecure-no-auth]
codewhale app-server --mobile [--host 0.0.0.0] [--port 7878] [--auth-token TOKEN]
codewhale app-server --mobile --host 127.0.0.1 [--port 7878] [--insecure-no-auth]

# Compatibility aliases — identical server, serve flag names:
codewhale serve --http   [...] [--insecure]
codewhale serve --mobile [...] [--insecure]
```

Defaults: host `127.0.0.1`, port `7878`, 2 workers (clamped 1–8).

The server binds to `localhost` by default. Configuration is via CLI flags —
there is no `[app_server]` config section.

`/v1/*` routes require a bearer token unless `codewhale app-server` is started
with `--insecure-no-auth` on a loopback bind such as `127.0.0.1`. Do not combine
no-auth mode with the `--mobile` default host `0.0.0.0`; use a token for LAN
mobile access, or add `--host 127.0.0.1` for local-only no-auth testing. The
`codewhale serve` compatibility aliases use `--insecure` for the same loopback
escape hatch.
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

`GET /v1/threads/summary` is the read-only summary surface used by the VS Code
Agent View. Each item includes `id`, `title`, `preview`, `model`, `mode`,
`archived`, `updated_at`, `latest_turn_id`, `latest_turn_status`, plus
workspace metadata:

```json
{
  "id": "thread_...",
  "title": "Implement MCP status count",
  "preview": "The TUI footer should count project MCP servers...",
  "model": "deepseek-v4-pro",
  "mode": "agent",
  "branch": "feature/runtime-api",
  "head": "abc1234",
  "dirty": false,
  "workspace": "/Users/you/projects/codewhale",
  "archived": false,
  "updated_at": "2026-06-06T05:43:00Z",
  "latest_turn_id": "turn_...",
  "latest_turn_status": "completed"
}
```

`branch` is resolved from the thread workspace at request time and may be
`null` when the workspace is not a Git repository or the branch cannot be read.
`head` is the current short Git commit for that workspace when available.
`dirty` is true when the workspace has staged, unstaged, or untracked changes.
`workspace` is included so editor clients can show when an agent lane is working
outside the current VS Code folder.

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

**Snapshots** (read-only side-git restore point listing)
- `GET /v1/snapshots?limit=20`

`/v1/snapshots` lists recent side-git restore points for the runtime workspace.
It is read-only and does not restore files. `limit` defaults to `20` and must be
between `1` and `100`.

```json
[
  {
    "id": "snap_...",
    "label": "post-turn:1",
    "timestamp": 1780730580
  }
]
```

Runtime API restore/retry/undo/editor-apply mutation endpoints are intentionally
deferred. GUI clients should treat thread summaries and snapshots as inspection
surfaces until atomic filesystem + conversation-state mutation semantics are
specified and tested.

**Receipts** (future read-only audit export)
- Proposed only: `GET /v1/threads/{thread_id}/turns/{turn_id}/receipt`

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
  `mode`, `task_id`, `system_prompt`, `latest_turn_id`,
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
`approval.timeout`, `sandbox.denied`.

`approval.required` events may include a `matched_rule` string when an
execution-policy rule caused the prompt. This field is explanatory metadata for
clients and does not grant or persist permissions.

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

## Runtime SDK Fleet Helpers

The v0.8.60 Runtime SDK fixture lives in `npm/runtime-sdk` and is exposed as
the `@codewhale/runtime-sdk` workspace package. It is deliberately thin: every
helper calls the local Rust Runtime API and therefore cannot bypass CodeWhale's
sandbox, approval prompts, provider configuration, or fleet ledger authority.

```js
import { createRuntimeClient } from "@codewhale/runtime-sdk";

const client = createRuntimeClient({
  baseUrl: "http://127.0.0.1:7878",
  token: process.env.CODEWHALE_RUNTIME_TOKEN,
});

const { runs } = await client.listFleetRuns();
const workers = await client.listFleetWorkers(runs[0].id);
await client.restartWorker(workers.workers[0].worker_id);
```

Fleet helpers cover the v0.8.60 HTTP surface:

| Helper | Runtime API route |
|---|---|
| `listFleetRuns()` | `GET /v1/fleet/runs` |
| `getFleetRun(runId)` | `GET /v1/fleet/runs/{run_id}` |
| `listFleetWorkers(runId)` | `GET /v1/fleet/runs/{run_id}/workers` |
| `getFleetWorker(workerId)` | `GET /v1/fleet/workers/{worker_id}` |
| `interruptWorker(workerId)` | `POST /v1/fleet/workers/{worker_id}/interrupt` |
| `restartWorker(workerId)` | `POST /v1/fleet/workers/{worker_id}/restart` |
| `stopFleetRun(runId)` | `POST /v1/fleet/runs/{run_id}/stop` |

`createFleetRun(spec)` and `fleetEvents(runId)` are typed ahead of the current
Rust routes so editor/web clients can code against the intended SDK contract.
Until the Runtime API exposes `POST /v1/fleet/runs` and a fleet event stream,
the SDK raises `RuntimeCapabilityError` with stable capability strings
(`fleet_run_create`, `fleet_event_stream`) instead of surfacing those gaps as
generic fetch failures.

Verification:

```bash
npm test --workspace @codewhale/runtime-sdk
```

## Agent Run Receipts

Sub-agent lanes persist compact run receipts in
`.codewhale/state/subagents.v1.json`. The Runtime API exposes those receipts as
a read-only inspection surface:

| Operation | Endpoint |
|---|---|
| List persisted agent runs | `GET /v1/agent-runs` |
| Inspect one run | `GET /v1/agent-runs/{run_id}` |

The response is the same worker-record shape surfaced by `agent` receipts:
`spec.run_id`, `actor_kind`, lifecycle `status`, bounded `events`,
`follow_up`, `takeover`, `artifacts`, `usage`, and `verification`. `run_id`
falls back to the worker id for older records, and `{run_id}` may be either the
run id or the worker id.

These endpoints do not start, cancel, or steer sub-agents. The API surface
exists so app/editor/headless clients can inspect the same handoff receipts that
the TUI and parent model see.

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

The app-server stdio control surface has its own drift guard — the advertised
`capabilities` method set is pinned in `crates/app-server/src/lib.rs`:

```bash
cargo test -p codewhale-app-server capabilities
```

Before a release, run the headless smoke (stdio probe + optional provider
matrix, no secrets leaked):

```bash
scripts/release/app-server-smoke.sh --matrix        # dry-run plan
bash scripts/release/app-server-smoke.test.sh       # parser self-test (fake binary)
```
