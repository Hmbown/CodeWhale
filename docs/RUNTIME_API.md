# Runtime API & Integration Contract

`codewhale app-server` is the canonical local runtime API and control plane.
Local SDKs, mobile/remote-control clients, and editor integrations talk to it
instead of screen-scraping terminal output. It serves the full HTTP/SSE runtime
API (`/v1/*`), a JSON-RPC control transport over stdio, and the phone-friendly
mobile page. `codewhale doctor --json` provides machine-readable health, and
`codewhale serve --acp` speaks the Agent Client Protocol over stdio for editors
such as Zed.

`codewhale serve --http` / `serve --mobile` remain as **compatibility aliases**
for `codewhale app-server --http` / `--mobile`; both launch the identical
server. New integrations should target `app-server`.

`codewhale exec` is the separate one-shot headless worker path (stream-json,
fleet worker subprocess, CI primitive). It is not part of this API, but it
shares the same runtime, provider/model resolution, permission profiles, and
event vocabulary.

This document is the stable integration contract for native workbench
applications (and other local supervisors) that embed the Codewhale engine.

## Architecture

```
local supervisor / SDK / automation harness
        │
        ├─ codewhale app-server --http     → HTTP/SSE runtime API (/v1/*)        [canonical]
        ├─ codewhale app-server --mobile   → runtime API + mobile control page
        ├─ codewhale app-server --stdio    → JSON-RPC control transport over stdio
        ├─ codewhale app-server --socket   → same JSON-RPC over a unix domain socket (desktop daemon)
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
| `codewhale web [--port 7878]` | HTTP/SSE on `127.0.0.1:7878` + embedded client | First-class loopback-only browser client; opens the default browser |
| `codewhale app-server --http` | HTTP/SSE on `127.0.0.1:7878` | Full `/v1/*` runtime API (canonical) |
| `codewhale app-server --mobile` | HTTP/SSE on loopback + `/mobile` | Runtime API + local mobile control page |
| `codewhale app-server --stdio` | JSON-RPC 2.0 over stdio | Local SDK / control probe (no listener) |
| `codewhale app-server --socket [--socket-path P]` | JSON-RPC 2.0 over a `0600` unix domain socket | Desktop daemon: multi-client, peer-uid checked, `daemon/attach` claim handshake (macOS/Linux; Windows named pipe reserved, not implemented) |
| `codewhale app-server` | HTTP on `127.0.0.1:8787` | Legacy in-process app-server (`/healthz`, `/thread`, `/app`, `/prompt`, `/tool`, `/jobs`); `/prompt` and `/thread` messages execute real turns via the runtime bridge |
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

### Runtime and account identity

`GET /v1/runtime/info` reports `codewhale_version` plus the full 40-character
`codewhale_commit` embedded by the shared CLI/TUI build. A source archive that
cannot provide an exact commit reports `unknown`, allowing compatibility
clients to fail closed rather than accepting an ambiguous binary pair.

The same response advertises `capabilities.account_session: true` and
`capabilities.turn_operation_idempotency: true`. A client must require the
latter before relying on `operation_key`; do not infer support from a 2xx turn
response because an older tolerant reader may ignore an unknown request field.
The response also includes a token-free account receipt:

```json
{
  "account": {
    "schema_version": 1,
    "state": "authenticated",
    "api_base": "https://api.codewhale.net",
    "account_id": "acct_...",
    "session_id": "session_...",
    "scopes": [],
    "expires_at": "2026-08-01T20:00:00Z"
  }
}
```

The Runtime reads this receipt from the exact profile- and API-origin-scoped
secure record written by `codewhale account login`; it does not run a second
login flow. States are `signed_out`, `authenticated`, `offline_cached`,
`expired`, or `revoked`. Scopes are copied only from explicit stored session
grants and are never inferred from account identity. Access/refresh tokens,
email, provider profile, and provider credentials are never returned.
`account_id` and `session_id` are included only for a request authorized with
the Runtime token (or an explicitly insecure loopback server); the public
bootstrap response remains usable but reports `signed_out`. Signed-out local
Work remains supported and never allocates cloud compute implicitly.

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
local integration clients can rely on it not changing silently.

### Daemon socket: `codewhale app-server --socket`

The desktop shell (DESKTOP-APP-BRIEF §2) attaches to a long-lived daemon over
a unix domain socket. The wire is the `--stdio` transport verbatim — the same
newline-delimited JSON-RPC 2.0 methods, dispatched by the same code — with one
handshake in front of it.

**Endpoint.** `--socket-path` if given; else `$CODEWHALE_HOME/run/daemon.sock`
when `CODEWHALE_HOME` is set (an explicit home is an isolation boundary); else
`$XDG_RUNTIME_DIR/codewhale/daemon.sock`; else
`~/Library/Application Support/codewhale/daemon.sock` on macOS or
`~/.codewhale/run/daemon.sock` elsewhere. The directory is created `0700`, the
socket is `0600`, and every accepted peer must present the daemon's own uid.
On start, a socket file nobody answers on is removed; a live one makes the new
daemon exit with `a live listener already answers on <path>; refusing to
replace it`; a non-socket file at the path is never touched. On Windows `--socket` fails with a typed
`UnsupportedPlatform` error naming the reserved pipe `\\.\pipe\codewhale-daemon`
— there is no silent TCP fallback. The daemon prints
`codewhale daemon: listening on <path>` to stderr once it is accepting.

**Handshake.** The first request on a connection must be `daemon/attach`
(`healthz` is also allowed beforehand, so a shell can probe liveness). Every
other method is refused with `-32010 attach_required` until then.

```json
{"jsonrpc":"2.0","id":1,"method":"daemon/attach","params":{
  "client":{"name":"codewhale-desktop","version":"1.2.3","pid":4242},
  "mode":"claim",
  "expect_daemon_version":"0.9.11"}}
```

`mode` is `"claim"` (this client spawned the daemon and manages its lifetime)
or `"attach"` (default: a guest that found a healthy daemon). A claim while
another connection owns the daemon fails with `-32011 daemon_already_claimed`
(`data.owner` names the holder) and the client should retry with `attach`.
`expect_daemon_version`, when present, must equal the daemon's crate version
or the attach fails with `-32013 daemon_version_skew` (the bundle-skew guard).
The reply reports the granted `role` (`owner` / `attached`), the daemon's
`pid`, `version`, `socket_path`, and `uptime_ms`, the current `owner`, and the
live `connections` count. A second `daemon/attach` on an attached connection
is `-32014 already_attached`.

**Capabilities.** On this transport `capabilities.methods` is the pinned stdio
set plus `daemon/attach` (second entry, after `healthz`); `transport` reads
`unix-socket`. `shutdown` is advertised to every connection because the method
exists, but only the owner may call it (below).

**Ownership.** Only the owner may `shutdown`; a guest's `shutdown` is refused
with `-32012 not_daemon_owner` and does not interrupt anyone's turn. When the
owner disconnects the slot frees, so a relaunched shell re-claims the daemon it
left running. The owner's `shutdown` stops the listener, closes every
connection, and removes the socket file. Journal replay from a client's
last-seen `seq` is not part of this transport yet.

### Interrupting a turn

`thread/message` streams until the turn reaches a terminal state, which can
take minutes. The read loop keeps polling stdin while a turn streams, so a
client can send:

```json
{"jsonrpc":"2.0","id":9,"method":"thread/interrupt","params":{"thread_id":"thr_..."}}
```

and the runtime is asked to interrupt that turn
(`POST /v1/threads/{id}/turns/{turn_id}/interrupt`). The reply carries
`interrupted: false` when no turn is streaming for that thread — this is not
an error, just nothing to stop. The interrupted `thread/message` then fails
with a `turn interrupted` error, and its reply is written before the
interrupt's own reply, since the turn owns the writer until it unwinds.

`shutdown` sent during a live turn also interrupts first: it needs the same
bridge that the turn holds, so without that it would wait for the very turn
it was meant to stop. Other requests that arrive mid-turn are queued and run
in order once the turn finishes.

### Running a prompt

`prompt/request` and `prompt/run` (byte-identical aliases) and the legacy
HTTP `POST /prompt` all execute a **real turn** on the runtime, through the
same bridge `thread/message` uses. There is no local fallback: nothing else
in the app-server can produce model output, so a prompt either runs or fails.

- `params.prompt` is required and must be non-empty (`-32602` otherwise).
- `params.thread_id` is optional. With one, the prompt runs on that thread and
  its history. Without one, the runtime gets a fresh thread for that single
  turn; the mapping is dropped when the turn ends, so a one-shot prompt is not
  addressable by `thread/interrupt`. Use `thread/message` when you need to be
  able to interrupt.
- `params.model` selects the model only when the call is the one that creates
  the runtime thread; an existing thread keeps the model it was created with.
- The response carries what the model actually said: `output` is the
  concatenated `agent_message` text, `model` is the model the runtime reports
  for the thread that ran it, and `events` are the real
  `response_start`/`response_delta`/`response_end` frames. Over stdio the same
  frames are also streamed to stdout while the turn runs, exactly as for
  `thread/message`.
- If the runtime cannot be reached, the call fails with `-32005`
  (`runtime_unavailable`) on stdio, or HTTP `503` with
  `{"error":{"code":"runtime_unavailable", ...}}` on `POST /prompt`. Failures
  are never shaped like a successful `PromptResponse`.

`POST /thread` with a `Message` body behaves the same way — it runs the turn
and replies `status: "completed"` with the streamed frames in `events` — where
it previously replied `accepted` without doing anything.

### Answering a clarification question

When a headless turn calls `request_user_input`, the runtime emits a
`user_input.required` event carrying a `request_id`. Reply on the runtime API:

```
POST /v1/user-input/{thread_id}/{request_id}
```

The app-server control transport cannot accept that reply.
`app/request` with `SubmitUserInput` returns `ok: false` and
`error: "user_input_reply_unsupported"`. This is a property of the transport,
not an omission: while a turn is streaming, the stdio loop executes only
`thread/interrupt` and queues everything else, so an answer sent there would
wait on the very turn that is waiting for it.

## SDK contract

The app-server exists so an external SDK can answer — without scraping TUI
output — *what route ran, which provider/model/reasoning/permission profile was
effective, what events happened, how many tokens were used, and how the run
finished.* The durable Thread/Turn/Item data model already carries most of
this; the table maps each integration need to where a local client reads it.

| Integration need | Where it comes from | Status |
|---|---|---|
| Route / effective model / billing surface | `TurnRecord` + thread `model`; per-run `--provider`/`--model` overrides | available |
| Permission / sandbox / approval profile | thread `auto_approve`, sandbox + approval policy | available |
| Run / thread / turn IDs | `thread_id`, `turn_id`, SSE event envelope | available |
| Event stream | `GET /v1/threads/{id}/events` (replay + live SSE) | available |
| Turn status / terminal classification | `TurnRecord.status` + error summary | available |
| Token usage | `TurnRecord.usage`; aggregate via `GET /v1/usage` | available |
| Single-read run receipt (route + usage + cost) | `GET /v1/threads/{id}/turns/{turn_id}/receipt` | proposed ([RECEIPTS.md](RECEIPTS.md)) |

For one-shot/headless automation, prefer `codewhale exec` with explicit
`--provider <id> --model <id>` so a failure identifies the exact provider/model
pair. Use `app-server` when a local integration needs to start, resume, steer,
or interrupt turns, list models/capabilities, follow the event stream, or read
usage. Both paths share the same runtime, so route-effective model resolution
and the event vocabulary match.

### Release smoke

`scripts/release/app-server-smoke.sh` is the committed pre-release check:

```bash
scripts/release/app-server-smoke.sh                 # stdio health/capabilities probe (no tokens)
scripts/release/app-server-smoke.sh --matrix        # + print the configured provider/model matrix
scripts/release/app-server-smoke.sh --matrix --real # + exec a cheap sentinel per provider
```

The stdio probe runs against a throwaway config, so it never reads real keys.
The matrix discovers configured providers from `codewhale auth list`, skips
unconfigured providers, and maps a provider to a cheap sentinel model only when
it has a built-in cheap default. That built-in set is deliberately conservative
(currently `deepseek`, `zai`, `moonshot`, and `openai`); every other provider —
including `arcee`, `openrouter`, `xiaomi-mimo`, and `openai-codex` — is left
unmapped on purpose and must be given a model per run via `SMOKE_MODEL_<SLUG>`
rather than a guessed default (#3205). Any configured-but-unmapped provider
fails loudly in `--real` mode. `auth list` reports presence flags only and exec
output is passed through a redactor, so secrets are never printed. The parser is
covered by `scripts/release/app-server-smoke.test.sh` against a fake `codewhale`
binary.

## ACP stdio adapter: `codewhale serve --acp`

`codewhale serve --acp` speaks JSON-RPC 2.0 over newline-delimited stdio for
ACP-compatible editor clients. The initial adapter implements the ACP baseline:

- `initialize`
- `session/new`
- `session/prompt`
- `session/cancel`

Prompt requests are routed through the configured Codewhale client and current
default model. Responses are emitted as `session/update` agent message chunks
followed by a `session/prompt` response with `stopReason: "end_turn"`.

The adapter is intentionally conservative: it does not yet expose shell tools,
file-write tools, checkpoint replay, or session loading through ACP. Use
`codewhale serve --http` for the full local runtime API and `codewhale serve --mcp`
when another client needs Codewhale's tools as MCP tools.

## Capability endpoint: `codewhale doctor --json`

Returns a JSON object describing the current installation's readiness state.
Suitable for health-check polling from a macOS workbench. This command is
strictly structural and offline: it does not load workspace credential
`.env` files, inspect credential environment values, open secret/OAuth files,
probe an OS keyring, contact providers, or start MCP processes.

```bash
codewhale doctor --json
```

### Response schema (key fields)

| Field | Type | Description |
|---|---|---|
| `version` | string | Installed version (e.g. `"0.8.9"`) |
| `config_path` | string | Resolved config file path |
| `config_present` | bool | Whether the config file exists |
| `paths` | object | Canonical config, settings, state, sessions, logs, automations, and secrets paths |
| `secret_backend` | object | Metadata-only file-store shape, or literal `unknown` / `not_probed` for system and unsupported backends |
| `workspace` | string | Default workspace directory |
| `legacy_state.primary_root` | string | Primary Codewhale state root inspected for known state paths |
| `legacy_state.legacy_root` | string | Legacy `.deepseek` state root inspected for known state paths |
| `legacy_state.needs_attention` | bool | Whether known `~/.deepseek` state paths need review or the read-only session recovery diagnostic found missing destination filenames / could not complete |
| `legacy_state.legacy_only_count` | number | Count of known state paths present only under the legacy root |
| `legacy_state.dual_present_count` | number | Count of known state paths present under both primary and legacy roots |
| `legacy_state.entries` | array | Per-path migration status: `{name, primary_present, legacy_present, status}` |
| `legacy_state.session_recovery.status` | string | `isolated`, `no_legacy_sessions`, `migration_pending`, `migration_incomplete`, `migration_complete`, or `scan_failed` |
| `legacy_state.session_recovery.read_only` | bool | Always true; doctor never invokes session migration or modifies either session directory |
| `legacy_state.session_recovery.chat_contents_read` | bool | Always false; comparison is based only on top-level `.json` filenames and filesystem metadata |
| `legacy_state.session_recovery.checkpoint_internals_scanned` | bool | Always false; `sessions/checkpoints/` and all other directories are skipped |
| `legacy_state.session_recovery.recoverable_files` | array | Bounded sample of up to 100 missing destination filenames with source and destination paths; no chat payloads |
| `legacy_state.session_recovery.recoverable_file_count` | number | Total missing destination filename count, including entries beyond the bounded sample |
| `legacy_state.session_recovery.recoverable_files_truncated` | bool | Whether more than 100 recoverable filenames were found |
| `legacy_state.session_recovery.recovery_command` | string or null | `codewhale sessions` when additive automatic recovery is available; null for isolated, complete, empty, or failed scans |
| `api_key.source` | string | Structural source state: `config_declared`, `env_declared`, `external_auth_declared`, `secret_store_unprobed`, `secret_store_unavailable`, `oauth_unprobed`, `external_consent`, `none`, `local_runtime`, or `unknown`; declarations are not availability proof |
| `api_key.availability` | string | Literal `present`, `not_required`, `not_probed`, `unavailable`, or `unknown`; only `present` and `not_required` certify structural Setup/Fleet credential readiness |
| `base_url` | string | Provider URL authority only (`scheme://host[:explicit-port]`); userinfo, path, query, and fragment are omitted |
| `default_text_model` | string | Default model |
| `memory.enabled` | bool | Whether the memory feature is on |
| `memory.path` | string | Path to memory file |
| `memory.file_present` | bool | Whether memory file exists |
| `mcp.config_path` | string | MCP config file path |
| `mcp.present` | bool | Whether MCP config exists |
| `mcp.probe_scope` | string | `configuration`; doctor does not start MCP servers |
| `mcp.live_health_checked` | bool | Always false for doctor JSON |
| `mcp.servers` | array | Per-server structural result and counts plus separate `checks`; URL userinfo/path/query/fragment and command argv, environment, header, and token values are never emitted, and all live stages are `not_checked` |
| `skills.selected` | string | Resolved skills directory |
| `skills.global.path` / `.present` / `.count` | — | Codewhale global skills dir (`~/.codewhale/skills`, with legacy `~/.deepseek/skills` support) |
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
    "source": "secret_store_unprobed",
    "availability": "not_probed"
  },
  "base_url": "https://api.deepseek.com",
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
      {"name": "filesystem", "enabled": true, "transport": "stdio", "args_count": 2, "env_count": 0, "status": "ok"}
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
codewhale app-server --mobile [--host 127.0.0.1] [--port 7878] [--auth-token TOKEN]
codewhale app-server --mobile --host ::1 [--port 7878] [--insecure-no-auth]
codewhale web [--port 7878]

# Compatibility aliases — identical server, serve flag names:
codewhale serve --http   [...] [--insecure]
codewhale serve --mobile [...] [--insecure]
```

Defaults: host `127.0.0.1`, port `7878`, 2 workers (clamped 1–8).

The server binds to `localhost` by default. Configuration is via CLI flags —
there is no `[app_server]` config section.

`/v1/*` routes require a bearer token unless `codewhale app-server` is started
with `--insecure-no-auth` on a loopback bind such as `127.0.0.1`. Mobile mode
is loopback-only: non-loopback hosts are rejected until Runtime has a TLS or
verified-overlay transport boundary. The `codewhale serve` compatibility aliases
use `--insecure` for the same loopback escape hatch.
Pass `--auth-token TOKEN` or set `CODEWHALE_RUNTIME_TOKEN=TOKEN` before starting
the server; `DEEPSEEK_RUNTIME_TOKEN` remains a compatibility alias. If neither
is set, the process generates a Runtime token for that process and does **not**
print it. `/health`, `/v1/runtime/info`, and an enabled static client shell
remain public; Runtime mutations and thread data stay behind `/v1/*`
authentication. `/mobile` returns 404 when mobile mode is disabled and serves
the unchanged static shell when it is enabled.

Authenticated clients can provide the token as `Authorization: Bearer TOKEN`,
`X-Codewhale-Runtime-Token: TOKEN`, the legacy
`X-DeepSeek-Runtime-Token: TOKEN`. Query-string and raw Runtime-token cookie
authentication are not supported.

### Local browser client

`codewhale web` starts the canonical Runtime API on `127.0.0.1`, serves
dependency-free assets embedded in the binary, prints a single-use launch URL,
and asks the operating system to open that URL in the default browser. If the
browser does not open, the printed URL remains usable for ten minutes. The
command cannot bind to a non-loopback host and cannot run with Runtime auth
disabled.

The browser-launch URL contains a random, short-lived, one-time bootstrap
capability, never the Runtime token. A loopback request exchanges that
capability for a
`codewhale_web_session=…; HttpOnly; SameSite=Strict; Path=/` cookie backed by a
single process-local server session that expires 12 hours after the server
process starts, consumes the capability immediately, and redirects to `/`.
Reused, expired, malformed, or
non-loopback bootstrap attempts fail closed. The Runtime bearer token is not
placed in rendered HTML, browser storage, logs, URL queries/fragments, or
browser-launch arguments. The one-time bootstrap capability is printed in the
local terminal and transits the OS browser launcher's argument list. A same-user
process could race the browser to the exchange, which is why the capability is
single-use, loopback-only, and expires after ten minutes — and why a same-user
attacker has strictly easier local avenues than this race.
Existing bearer/header/cookie authorization for `/v1/*` is unchanged outside
web mode. In web mode, cookie-authenticated unsafe requests must also carry the
exact local web origin, and Fetch Metadata identifying a cross-origin cookie
request is rejected. Explicit bearer and Runtime-token header clients keep
their existing behavior.

The embedded client provides a responsive thread/search rail, Runtime-owned
session facts, transcript and tool receipts, and a bottom composer. It can
create, select, rename, and archive threads; choose a provider and model for a
new thread without changing Runtime defaults; start or steer turns; interrupt
work; resolve approvals; and answer Runtime user-input requests. Selection
loads `GET /v1/threads/{id}` first, then opens the replayable event stream with
`since_seq=latest_seq`; reconnection advances from the newest accepted sequence
and drops duplicates or events from a stale selection. The thread detail
snapshot includes `pending_approvals`, `pending_user_inputs`, and
`pending_dynamic_tool_calls`; clients must hydrate those fields before
subscribing so a reload cannot strand work whose request event is at or before
`latest_seq`. Resolution is also published as `approval.decided`,
`user_input.answered`, `user_input.canceled`, `tool_call.resolved`,
`tool_call.canceled`, or `tool_call.timeout` for already-connected clients.

An existing thread's model, mode, permission posture, workspace, and branch are
display-only in this client. Files/Changes, PTY/terminal, preview, artifacts,
provider login or global-default switching, Fleet creation, and
undo/retry/restore controls are intentionally absent until the Runtime publishes
explicit contracts for them.

### Mobile control page

`codewhale serve --mobile` starts the same HTTP/SSE runtime API and serves a
phone-friendly control page at `/mobile`. It binds only to loopback
(`127.0.0.1` or `::1`); a non-loopback host is rejected because this Runtime
surface does not yet provide TLS or a verified overlay transport. The static
HTML page contains no Runtime bearer and is not itself token-gated. When
Runtime auth is enabled, the CLI prints a short-lived, single-use loopback
bootstrap URL. That capability creates a 30-minute, process-local
`Max-Age=1800; HttpOnly; SameSite=Strict` mobile session cookie plus origin-scoped browser
proofs. A sibling port that receives the host-scoped cookie cannot use it by
itself. The page can also exchange an explicitly entered bearer once, then
clears it rather than storing it in browser storage or a cookie. EventSource
connections use separate short-lived, single-use stream tickets.

The mobile page can list/create threads, send prompts, follow live SSE events,
steer or interrupt an active turn, and resolve normal tool approvals through
`POST /v1/approvals/{approval_id}`. It is a local-only convenience surface;
do not expose it directly to another device or public network until Runtime has
a TLS or verified transport boundary.

### Endpoints

**Health**
- `GET /health`

**Sessions** (durable session manager)
- `GET /v1/sessions?limit=50&search=<fuzzy>&include_archived=false&archived_only=false&workspace=<path>&sort=recent|name|size`
- `GET /v1/sessions/summary?…` (same query params; projected row shape)
- `GET /v1/sessions/{id}` (add `?peek=true&entries=12` for a bounded, redacted
  read-only peek instead of the full transcript)
- `PATCH /v1/sessions/{id}` (`{ "title"?: string, "archived"?: bool }`)
- `DELETE /v1/sessions/{id}`
- `POST /v1/sessions/{id}/resume-thread`

Sessions and threads answer the same `include_archived` / `archived_only` pair
with the same meaning, and `search` is the same fuzzy match (title, id,
workspace — substring, then subsequence) the TUI session picker and the workbar
Sessions list use. All three surfaces run one projection
(`crates/tui/src/session_projection.rs`), so a listing cannot differ between
the terminal and the dashboard.

`GET /v1/sessions/summary` returns rows that are field-compatible with
`GET /v1/threads/summary` — `id`, `title`, `preview`, `model`, `mode`,
`workspace`, `archived`, `updated_at` — plus `message_count`, `total_tokens`,
`created_at`, `parent_session_id`, and `is_current`. One caveat stated plainly:
`preview` is the session's recorded **title**, not its last message. Session
metadata does not store a last message, and reading every transcript to
synthesise one would make a list view an unbounded read. Full transcript
preview lives in the TUI session picker, which reads one selected session.

`PATCH /v1/sessions/{id}` renames and/or archives a saved session and returns a
lifecycle receipt shaped like the thread patch receipt:

```json
{
  "session": { "id": "…", "title": "Renamed", "archived": true, "…": "…" },
  "changes": { "title": "Renamed", "archived": true }
}
```

`changes` lists only what actually moved, so a no-op patch is distinguishable
from an applied one. Archiving is durable and reversible: an archived session
stays on disk and stays loadable, disappears from default listings, and is
never chosen by `--continue` or by auto-resume. The route is the same writer
the TUI picker (`e`) and `/sessions archive <id>` use — there is no second
archive notion.

While a session is open in an interactive Codewhale process, that process holds
the authoritative copy in memory and rewrites the whole document on its next
autosave. `PATCH` therefore fails closed on it with `409 Conflict` rather than
writing something that would be silently reverted. Change it in the terminal
instead. A standalone `codewhale web` holds nothing open and is never blocked.

`GET /v1/sessions/{id}?peek=true` returns a bounded, redacted, read-only view
instead of the transcript: at most 12 entries of at most 400 characters each
(`&entries=N` lowers the budget, never raises it past the cap), tool calls and
results summarised to a name and a size rather than inlined, and
credential-shaped substrings masked. `omitted_before` reports how many earlier
messages were dropped. The payload carries `"live": false` and deliberately has
no turn status, `running`, or `active` field — a saved session is a recording,
and live state comes only from a resumed thread's SSE stream.

**Threads** (durable runtime data model)
- `GET /v1/threads?limit=50&include_archived=false&archived_only=false`
- `GET /v1/threads/summary?limit=50&search=<optional>&include_archived=false&archived_only=false`
- `POST /v1/threads`
- `GET /v1/threads/{id}`
- `PATCH /v1/threads/{id}` (see body shape below)
- `POST /v1/threads/{id}/resume`
- `POST /v1/threads/{id}/fork`

`POST /v1/threads` accepts optional execution defaults in addition to the
provider, model, workspace, and permission fields:

```json
{
  "model_provider": "openai-codex",
  "model": "gpt-5.6",
  "reasoning_effort": "high",
  "allowed_tools": ["read_file", "search"]
}
```

`reasoning_effort` uses the canonical Runtime vocabulary (`auto`, `off`,
`low`, `medium`, `high`, `xhigh`, `ultra`, or `max`; documented compatibility
aliases are accepted and persisted canonically). `allowed_tools` is a
model-visible allowlist. Omitting it keeps the normal configured catalog; an
explicit empty array (`"allowed_tools": []`) exposes no tools to the model.
Both fields are additive: older thread records and clients that omit them
retain their previous behavior.

`GET /v1/threads/summary` is the read-only summary surface used by the VS Code
Agent View. `search` matches thread `id`, `title`, and `model` (and, when the
title is unset, the latest turn's input summary — the displayed title). It
does not scan turn or item bodies: `preview` is filled only after a match, so
a dashboard keystroke is not a whole-store read per thread. Each item includes
`id`, `title`, `preview`, `model`, `mode`, `archived`, `updated_at`,
`latest_turn_id`, `latest_turn_status`, plus workspace metadata:

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
- `POST /v1/threads/{id}/undo` - fork the thread with the last N turns removed (`{"depth": N}`, default 0 = last turn only); returns the forked thread plus `original_user_text` so a GUI can pre-populate the input box
- `POST /v1/threads/{id}/patch-undo` - snapshot-based file rollback followed by the same fork (`{"depth": N}`); returns `patch_result` (`files_restored`, `summary`, `snapshot_label`) alongside the forked thread
- `POST /v1/threads/{id}/retry` - fork with the last N turns removed and immediately start a new turn (`{"depth": N, "prompt": "..."}`; `prompt` overrides the original user text, which is re-used when omitted)

`POST /v1/threads/{id}/turns` accepts the same optional
`reasoning_effort` and `allowed_tools` fields as per-turn overrides:

```json
{
  "prompt": "Review this change without running tools.",
  "operation_key": "cwc-request-01J7Y6Q9W4",
  "reasoning_effort": "max",
  "allowed_tools": []
}
```

Resolution is deterministic: a turn override wins over the thread default,
which wins over the Runtime's normal configuration. For tools, reaching normal
configuration means the ordinary configured catalog; `[]` is never treated as
missing. Reasoning is normalized only after the exact provider/model route is
resolved, and `auto` remains a per-prompt reasoning decision even when the
thread uses a fixed model. The request still enters the existing
`Op::SendMessage` path and the single `Engine::run_turn` loop.

`operation_key` is an optional idempotency key for clients that may lose an
HTTP response after the Runtime accepted a turn. It is scoped to the current
Runtime store and thread, may contain at most 128 UTF-8 bytes, and may not be
empty or contain surrounding whitespace or control characters. Omitting it
preserves the legacy create-a-new-turn behavior.

The first accepted request durably binds a SHA-256 fingerprint of the key to
the Runtime turn id and a canonical request fingerprint before sending the
existing `Op::SendMessage`. An exact retry returns that original turn in the
normal `{ "thread": ..., "turn": ... }` response and emits no second engine
operation, item, or lifecycle sequence. Reusing the key on the same thread
with a different provider/model, prompt, reasoning policy, tool allowlist or
dynamic-tool schema, environment, or permission policy fails closed with
`409 Conflict`. The same caller key may be used independently on another
thread.

Only the scoped key fingerprint, request fingerprint, thread id, and turn id
are stored in the Runtime's private turn-operation index. The raw key is never
persisted or logged, and request bodies, credentials, and attachments are not
copied into that index. Existing thread/turn persistence remains the source of
the returned turn after a process restart.

**Approvals**
- `POST /v1/approvals/{approval_id}` with body
  `{ "decision": "allow" | "deny", "remember": false }`

**User input**
- `POST /v1/user-input/{thread_id}/{input_id}` with body
  `{ "answers": [{ "id": "question-id", "label": "Choice", "value": "Choice" }] }`

Submitted values are delivered to the active model turn but are deliberately
excluded from durable Runtime items and events. The settled tool item contains
only a neutral receipt and a machine-readable `response_redacted` marker. The
Runtime accepts only an exact pending `(thread_id, input_id)` request; an
unknown, concurrently settling, or already settled id returns 404 and is never
placed in the engine mailbox. It commits the secret-free
`user_input.answered` receipt before removing the snapshot-authoritative prompt
or delivering the answer to the engine. That settlement runs independently of
the HTTP connection, so disconnecting after submission cannot leave a prompt
half accepted. Terminal-turn cancellation follows the same receipt-before-
removal ordering through `user_input.canceled`.

**Client-executed dynamic tools**
- `POST /v1/threads/{thread_id}/turns/{turn_id}/tool-calls/{call_id}/result`

The thread and turn in the result route must match the pending call. A call is
settled at most once; wrong-route and duplicate results return 404. Terminal
lifecycle events carry identifiers and status only, never tool result content.
The Runtime commits the terminal lifecycle event before making a submitted
result available to the model. Result delivery, timeout, and terminal-turn
cancellation race through one settlement owner, so exactly one of these events
is durable for a call:

- `tool_call.requested` — the typed client-executed call became pending;
- `tool_call.resolved` — a result was durably accepted by the Runtime
  (`result_accepted: true`; `success` is result metadata, but result content is
  excluded);
- `tool_call.timeout` — no result won before the bounded wait expired;
- `tool_call.canceled` — the turn terminated before a submitted result won.

HTTP `202 Accepted` and `tool_call.resolved` share that durable-acceptance
meaning. Neither claims that the model consumed the result: a concurrent turn
shutdown may close the model receiver after acceptance. Once the Runtime has
accepted the result, that call is terminal and a duplicate result returns 404.

**Events** (SSE replay + live stream)
- `GET /v1/threads/{id}/events?since_seq=<u64>`

Durable history parsing runs off the async server workers and reaches SSE in
bounded batches of at most 256 events through a backpressured channel. Broadcast
delivery is only a wake-up optimization: a lagged receiver opens the same
bounded durable replay from its last accepted cursor. Optional `replay_limit`
returns the newest requested tail and may not exceed 4096; `previous_seq` on
the first returned event advances past exactly the omitted history.

**Snapshots** (side-git restore point listing + restore)
- `GET /v1/snapshots?limit=20`
- `POST /v1/snapshots/{id}/restore`

`/v1/snapshots` lists recent side-git restore points for the runtime workspace.
`limit` defaults to `20` and must be between `1` and `100`. `POST
/v1/snapshots/{id}/restore` restores workspace files from the snapshot and
returns `{"restored": "<snapshot-id>"}`.

```json
[
  {
    "id": "snap_...",
    "label": "post-turn:1",
    "timestamp": 1780730580
  }
]
```

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

Create and update requests accept an optional `model`. When present, each
scheduled or manually triggered run uses that model; omitting it keeps the
runtime's default task model.

**Operate** (always-on named operation; same `OperateRecord` as CWC
`20de981` / PR #284)

- `GET /v1/operate` — current operation + plan board
- `POST /v1/operate` — create (`direction`, optional `burnRate`)
- `PATCH /v1/operate` — steer direction, `burnRate`, or `leadPlan`
- `PUT /v1/operate/plan` — set `leadPlan` (`{ slices: [...] }`)
- `POST /v1/operate/keepalive` — observe spend / burn; never stops
- `POST /v1/operate/cancel` — explicit cancel (`/v1/operate/stop` aliases);
  also pauses the `cw-operate` keepalive so nothing keeps spending after cancel
- `POST /v1/operate/auto-merge/check` — call landed
  `scripts/check-auto-merge.py --repo --pr --agent` (does not merge)

The operation record (`current.json`) is persisted under a cross-process
file lock with atomic temp+rename writes; every PATCH / keepalive / plan
save reloads the latest state inside the lock, so concurrent saves merge
instead of losing writes. A `PATCH` that changes `direction` invalidates
the recorded `leadPlan` (workers stop executing superseded slices) and
pulls the keepalive lead run forward to re-plan. `POST /v1/operate`
installs the hourly `cw-operate` keepalive and kicks its first lead-plan
run immediately instead of waiting out the first recurrence; credentials
resolve through the normal Z.ai provider resolution (config, `api_key_env`,
secret store, or provider env vars — blank values count as missing).

`burnRate` is `{ "kind": "usd_per_hour", "amountUsdPerHour": number }`,
a positive number, or `null` (unbounded). Status is
`planning | running | idle_blocked | cancelled`. Pace
(`unbounded | hold | throttle | widen`) is not a status: over-target
throttles, under-target widens, no wallet-cap stop. Idle-blocked only
for empty direction, awaiting lead plan, missing credentials, or a
human gate. Auto-merge is `scripts/check-auto-merge.py --repo … --pr …
--agent …` from codewhale-ops `origin/main` (exit 0), then
`scripts/auto-merge-pr.py`. Do not invent a second checker.

**Introspection**
- `GET /v1/workspace/status`
- `GET /v1/skills`
- `GET /v1/apps/mcp/servers`
- `GET /v1/apps/mcp/tools?server=<optional>`

Skill activation toggles are persisted under a cross-process transaction lock.
Each mutation reloads and merges the latest exact-name state before an atomic
write, and `GET /v1/skills` refreshes that shared state so another Codewhale
process's successful toggle is visible without restarting the Runtime API.

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

## Provider and model selection

These three routes are how a GUI renders a model picker whose contents are true
for *this* runtime instead of guessed from a version snapshot. They were
undocumented until 2026-08-04, which cost a desktop integration a day: the
client probed `/v1/models`, `/v1/runtime/models`, and `/v1/runtime/providers`
(all correctly 404) and concluded the capability did not exist.

### `GET /v1/providers`

```json
{
  "current": "modelstudio-token-plan",
  "providers": [
    {
      "id": "modelstudio-token-plan",
      "model_provider_id": "modelstudio-token-plan",
      "display_name": "Alibaba Cloud Model Studio",
      "default_model": "qwen3.8-max",
      "has_model_catalog": true,
      "credentialState": "configured"
    }
  ]
}
```

`current` is the active generic provider id. Only the active entry carries an
exact identity: an active built-in normally repeats its canonical id in
`model_provider_id`, while an active named custom route has `current` set to
`custom` and the exact configured key there (for example `lm-studio`). Other
entries have a null exact id; a null id on the active `custom` entry identifies
the released legacy root-level custom route. Preserve both fields from the
selected entry and send a non-null exact id back as `POST /v1/threads`'s
`model_provider_id`; dropping a named custom id would collapse the selection to
the legacy root custom route. `credentialState` is a stable, non-secret
projection of the Runtime's existing structural credential classification:

- `configured`: credential material is structurally available;
- `login_required`: the route needs login or a usable login capability;
- `missing`: an API-style credential is unavailable;
- `no_auth`: the route explicitly disables credential use;
- `local`: the exact route is local and keyless;
- `legacy`: the compatibility route cannot be classified more precisely.

For an active named custom provider, this state is calculated from the exact
route named by `model_provider_id`, not from a generic custom-provider default.
It deliberately collapses saved-key and imported-token details into
`configured`, and login/consent-source details into `login_required`.

The response never includes endpoint URLs, credential environment-variable
names, filesystem paths, credential values, consent-source details, or token
metadata. `credentialState` is not a provider canary: `configured` does not
prove endpoint reachability, credential validity, model entitlement, or a
successful request. The models route below is also only a selection catalog; a
non-empty list does not prove that the route can currently serve a request.

### `GET /v1/providers/{id}/models`

```json
{
  "provider": "deepseek",
  "models": [
    {
      "id": "deepseek-v4-flash-vision-exp",
      "image_input": "supported"
    }
  ]
}
```

The catalog for one provider. Returns `400` for an unknown id, and for the
legacy `deepseek-cn` alias, which has no provider metadata — use `deepseek`.
An empty `models` array means the Runtime has no discoverable or configured
model ids for that provider; it does not report credential presence.

The ids returned here are exactly the values accepted by `POST /v1/threads`'s
`model` field and by the switch route below. `image_input` is the exact resolved
provider/model route's capability state: `supported`, `unsupported`, or
`unknown`. Keep `unknown` unknown rather than inferring from the model name or
wire protocol. `supported` describes the model route; it does not mean a given
client implements an image-upload control.

For a thread-scoped choice, send the provider fields from the selected entry
alongside the selected model. Omit `model_provider_id` when it is null:

```json
{
  "model_provider": "custom",
  "model_provider_id": "lm-studio",
  "model": "local-vision-model"
}
```

This creates one thread on the exact named custom route without changing the
Runtime's provider or model defaults.

### `POST /v1/providers/{id}/switch`

```json
// request  (model is optional; omit to take the provider default)
{ "model": "qwen3.8-max" }

// response
{ "provider": "modelstudio-token-plan", "model": "qwen3.8-max",
  "message": "…", "persisted": true }
```

**Use this rather than simulating a switch with repeated `POST /v1/config`
writes plus a reload.** Provider and model move together here, the change is
validated against the provider's catalog before it is applied, and `persisted`
reports whether it was written to config or applied to the live session only.
Rejects an unknown provider id and the `deepseek-cn` alias with `400`.

## Runtime data model

The runtime uses a durable Thread/Turn/Item lifecycle.

- **ThreadRecord** — `id`, `created_at`, `updated_at`, `model`,
  `model_provider` (generic kind), `model_provider_id` (optional exact configured
  route), `workspace`, `mode`, `task_id`, `system_prompt`, `latest_turn_id`,
  `latest_response_bookmark`, `archived`
- **TurnRecord** — `id`, `thread_id`, `status` (`queued|in_progress|completed|
  failed|interrupted|canceled`), `effective_provider`, `effective_model`,
  `effective_billing_surface`, timestamps, duration, usage, error summary
- **TurnItemRecord** — `id`, `turn_id`, `kind` (`user_message|agent_message|
  tool_call|file_change|command_execution|context_compaction|status|error`),
  lifecycle `status`, `metadata`

Events are append-only with a global monotonic `seq` for replay/resume.

`effective_billing_surface` is a non-secret classification derived from the
endpoint that served the turn. Recognized StepFun routes use `stepfun-payg` or
`stepfun-plan`; unknown and custom endpoints leave it unset. The raw base URL is
not persisted in `TurnRecord`.

### Restart semantics

- If the process restarts while a turn or item is `queued` or `in_progress`,
  the recovered record is marked `interrupted` with an `"Interrupted by
  process restart"` error.
- The trailing newline is an event append's commit marker. On startup, a final
  JSONL fragment without that delimiter is truncated and fsynced even when its
  bytes form valid JSON; it is an uncommitted append, and its already-reserved
  sequence number is not reused. Newline-terminated malformed records are not
  identifiable crash debris and continue to fail closed during replay.
- If a terminal turn record reached disk but its terminal event sequence did
  not, the first async read reconciles any unresolved dynamic calls as
  `tool_call.canceled` and then emits one `turn.completed`. Existing terminal
  call and turn receipts are detected and never duplicated.
- Turn-operation bindings survive restart. Retrying the same `operation_key`
  and request returns the original recovered turn (including an `interrupted`
  turn recovered from an in-progress process exit); mismatched reuse remains a
  conflict. A crash-created binding that never acquired a turn is discarded at
  startup because engine submission happens only after both records are
  durable.
- Task execution performs its own recovery on top of the same persisted
  thread/turn store.

### Approval model

- The `auto_approve` flag applies to the runtime approval bridge and engine
  tool context. When enabled for a thread/turn/task, approval-required tools
  are auto-approved in the non-interactive runtime path, shell safety checks
  run in auto-approved mode, and spawned sub-agents inherit that setting.
- When omitted, `auto_approve` defaults to `false`.
- [Authorization order](AUTHORIZATION_ORDER.md) describes where typed rules,
  registered tool requirements, safety floors, repository law, approval
  transport, and sandbox enforcement sit relative to one another.

### SSE event stream

The SSE event payload shape for `/v1/threads/{id}/events`:

```json
{
  "schema_version": 1,
  "seq": 42,
  "previous_seq": 38,
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
- `seq` is allocated globally across all Runtime threads. Consequently, gaps
  between a thread's events are normal when other threads interleave. On this
  per-thread SSE stream, `previous_seq` is the sequence of the last event
  delivered for this thread (or the requested replay cursor for the first
  event); clients detect loss by comparing it with their accepted per-thread
  cursor, not by requiring `seq == previous_seq + 1`. Sequence allocation is
  also not rewound after an append is transactionally rolled back, so a retry
  can intentionally skip an unused value without implying a missing event.
- `thread.started`, `turn.started`, and `turn.completed` are emitted as SSE event
  names exactly as before.
- `timestamp` remains the canonical event time for schema version 1. `created_at`
  is an equivalent alias for clients that use `created_at` naming elsewhere; do
  not require both fields to be present.

Common event names: `thread.started`, `thread.forked`, `turn.started`,
`turn.lifecycle`, `turn.steered`, `turn.interrupt_requested`,
`turn.completed`, `item.started`, `item.delta`, `item.completed`,
`item.failed`, `item.interrupted`, `approval.required`, `approval.decided`,
`approval.timeout`, `user_input.required`, `user_input.answered`,
`user_input.canceled`, `tool_call.requested`, `tool_call.resolved`,
`tool_call.timeout`, `tool_call.canceled`, `sandbox.denied`.

Agent-message and reasoning deltas are materialized into the item projection
before their corresponding `item.delta` event is sequenced. To avoid an fsync
for every provider fragment, adjacent deltas are coalesced to configured bounds
of at most 32 ms or approximately 16 KiB before publication (an indivisible
upstream chunk can itself exceed the byte target). A process crash inside that
unpublished window can lose the recent suffix; no durable event claims that
suffix existed. Once an `item.delta` is durable, snapshots at or beyond its
cursor include the same materialized prefix.

`approval.required` events may include a `matched_rule` string when an
execution-policy rule caused the prompt. This field is explanatory metadata for
clients and does not grant or persist permissions.

## Security boundary

- **Localhost by default**. The server binds to `127.0.0.1` by default.
  `--mobile` is also loopback-only and rejects a non-loopback host until a TLS
  or verified-overlay transport boundary exists. The runtime does not provide
  user isolation or TLS.
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
model is preserved. Cross-origin preflights advertise only `Authorization`,
`Content-Type`, `Accept`, `X-Codewhale-Runtime-Token`, and the compatibility
`X-DeepSeek-Runtime-Token` request header; custom request headers are not
allowed. Added in v0.8.10 (#561), tightened in v0.9.1 (#4454).

## Managed Fleet Runtime and SDK helpers

The Runtime SDK lives in `npm/runtime-sdk` and is exposed as
the `@codewhale/runtime-sdk` workspace package. It is deliberately thin: every
helper calls the local Rust Runtime API and therefore cannot bypass Codewhale's
sandbox, approval prompts, provider configuration, or fleet ledger authority.

```js
import { createRuntimeClient } from "@codewhale/runtime-sdk";

const client = createRuntimeClient({
  baseUrl: "http://127.0.0.1:7878",
  token: process.env.CODEWHALE_RUNTIME_TOKEN,
});

const created = await client.createFleetRun({
  target: "this_computer",
  roles: [{ name: "reviewer" }, { name: "verifier" }],
  workflow: {
    id: "release-check",
    kind: "parallel",
    tasks: [
      { id: "review", name: "Review", instructions: "Review locally.", worker: { role: "reviewer" } },
      { id: "verify", name: "Verify", instructions: "Verify locally.", worker: { role: "verifier" } },
    ],
  },
});

// POST /runs only prepares durable work. This call crosses the launch gate.
await client.startFleetRun(created.run.id);

let cursor;
for await (const event of client.fleetEvents(created.run.id, { after: cursor })) {
  if (event.cursor) cursor = event.cursor;
  if (event.event === "fleet.replay.cursor_unavailable") {
    // Reload getFleetRun(created.run.id), then reconnect without the old cursor.
  }
}
```

The managed path is deliberately two-step. `POST /v1/fleet/runs` validates and
persists the run and queue without starting a worker. A separate authenticated
`POST /start` activates it and schedules the executor driver; its `202` response
reports `leased: 0` because the driver performs all leasing after it owns the
run. Creation requires named roles, one task owner per role, a `parallel`
Workflow, and an explicit Runtime target. v0.9.4 executes
only `this_computer`; `another_computer` and `cloud` return `501` rather than
silently executing locally. Worker IDs are generated per run; caller-assigned
`worker_specs` return `501` until custom workers can be given collision-free
managed identities. Parallel tasks with overlapping effective write roots are
rejected before the run is journaled. Managed `security_policy` overrides also
fail closed until that document can be enforced end to end; executable
authority comes from each named role's tool posture and bounded task workspace
scope.

Fleet helpers cover this HTTP surface:

| Helper | Runtime API route |
|---|---|
| `createFleetRun(spec)` | `POST /v1/fleet/runs` |
| `startFleetRun(runId)` | `POST /v1/fleet/runs/{run_id}/start` |
| `listFleetRuns()` | `GET /v1/fleet/runs` |
| `getFleetRun(runId)` | `GET /v1/fleet/runs/{run_id}` |
| `listFleetWorkers(runId)` | `GET /v1/fleet/runs/{run_id}/workers` |
| `getFleetWorker(workerId)` | `GET /v1/fleet/workers/{worker_id}` |
| `interruptWorker(workerId)` | `POST /v1/fleet/workers/{worker_id}/interrupt` |
| `stopWorker(workerId)` | `POST /v1/fleet/workers/{worker_id}/stop` |
| `restartWorker(workerId)` | `POST /v1/fleet/workers/{worker_id}/restart` |
| `stopFleetRun(runId)` | `POST /v1/fleet/runs/{run_id}/stop` |
| `replayFleetEvents(runId, options)` | `GET /v1/fleet/runs/{run_id}/events/replay` |
| `fleetEvents(runId, options)` | `GET /v1/fleet/runs/{run_id}/events` (SSE) |

`stopWorker` durably cancels that worker's active task and leaves the rest of
the Fleet running. `interruptWorker` is the compatibility name for the same
attempt-fenced cancellation transition. `stopFleetRun` cancels every queued or
active task and marks the whole run cancelled.

Replay covers aggregate run/task transitions and privacy-bounded individual
worker transitions. Event bodies omit prompts, tool call IDs, completion text,
artifact paths/checksums, and cancellation identities; bounded failure reasons
pass through secret redaction. `cursor` is opaque and stable across ordinary
appends and Runtime restarts. Clients reconnect with `after=<cursor>`. A fresh
request returns a bounded newest tail and marks `history_truncated` when older
history exists. Ledger compaction can remove an old cursor; the JSON endpoint
then returns `409`, while the SSE endpoint emits
`fleet.replay.cursor_unavailable`, so the client reloads the current run
projection instead of accepting a silent gap.

`GET /v1/runtime/info` advertises `fleet_run_create`, `fleet_run_start`,
`fleet_event_replay`, `fleet_event_stream`, and `fleet_local_target`. Older
runtimes without a requested route still produce a typed SDK
`RuntimeCapabilityError`.

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
| List session summaries | `GET /v1/sessions/summary` |
| Get session | `GET /v1/sessions/{id}` |
| Rename / archive session | `PATCH /v1/sessions/{id}` |
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
