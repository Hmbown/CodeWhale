# RFC: Lifecycle Event Outbox

**Issue:** #1365 (provisional — the upstream issue is not filed yet; see the
note at the end of the header)
**Status:** Implemented; pending upstream PR
**Date:** 2026-08-20
**Reference:** the shipped contract lives in
[docs/CONFIGURATION.md](../CONFIGURATION.md) ("Lifecycle Outbox") and the
build log is [docs/changelog-lifecycle-outbox.md](../changelog-lifecycle-outbox.md).
This RFC converts that changelog into the review artifact, modeled
on [1364-hooks-lifecycle.md](1364-hooks-lifecycle.md). Where the three
disagree, this RFC is the proposal, `CONFIGURATION.md` is the shipped
contract, and the changelog is the build log.

**Scope:** everything below covers the **TUI runtime** and headless
**`codewhale exec`** v1. The app-server / ACP surfaces are **out of scope**
and the CLI subcommands and the `workflow` tool do not fire the outbox
(see §2). This is the same boundary upstream drew for shell hooks in
RFC 1364, one row further out: where 1364 made lifecycle *hooks* a TUI
feature, this RFC makes the machine-readable *outbox* a TUI + `exec`
feature.

**Issue number note:** the upstream issue has not been filed; `1365` is the
next free RFC number in-tree after 1364 (verified: no collision in
`docs/rfcs/`). When the issue lands, rename this file and re-point the
header.

## 0. Implementation status

Verified against the tree:

| Item | Status | Where |
| --- | --- | --- |
| `[lifecycle_outbox]` config table | implemented | `crates/config/src/lib.rs:1553-1575`, wired in `crates/tui/src/tui/app/init.rs:574-586` |
| JSONL writer (seq recovery, bounded payloads) | implemented | `crates/hooks/src/lifecycle_outbox.rs` (whole module) |
| `WebhookHookSink` bearer-token transport | implemented | `crates/hooks/src/lib.rs:207-277` |
| TUI emit sites (7 events) | implemented | §4.6 table |
| `exec` emit sites (turn pair + failure pair) | implemented | `crates/tui/src/lib.rs:11673,12082,12279` |
| `workspace` on every event, `subagent` on sub-agent events | implemented | every site in §4.6 |
| `codewhale doctor` posture row | implemented | `crates/tui/src/lib.rs:6031-6044` |
| Tests (unit + config + integration harness) | implemented | §7 |

One deliberate delta from the issue draft's ask, shipped: `turn_failed` is
not a separate event name. A failed turn emits `turn_end` with
`kind: "turn.failed"` (projected from the turn status), on both surfaces
and on both terminal paths. This keeps the envelope's `event`/`kind` split
clean — one event, many kinds — and gives supervisors a single
`turn_start` → `turn_end` pair to correlate.

## 1. Problem

Running codewhale under an external supervisor — a terminal multiplexer
wrapper, an automation harness, an alerting setup — requires a
machine-readable stream of *what happened*: when a turn started, finished,
failed, or stalled; what a sub-agent did; when a session began and ended.
Today a supervisor can only scrape the terminal screen. Specifically:

- **Hooks are TUI-only and per-event shell commands.** The `[hooks]`
  surface (RFC 1364) fires 11 events but runs a configured shell command
  per event, in an interactive TUI session only. Headless
  `codewhale exec` runs fire no hooks at all, and a supervisor must
  install and maintain per-hook commands to get any signal.
- **There is no `turn_start` hook event and no stall signal anywhere.**
  When the stall watchdog recovers a wedged turn, nothing scriptable is
  emitted — the only visibility is a TUI status toast.
- **The durable event store is not populated for interactive sessions.**
  The runtime thread manager keeps a durable per-thread event store with a
  global monotonic `seq` cursor, but it is only written for threads driven
  through the runtime `/v1` HTTP+SSE API — ordinary interactive sessions
  never populate it.
- **Nothing writes lifecycle events to a plain local JSONL file.**

The issue draft's key observation holds: **the event data model and the
JSONL sink plumbing are already there.** `RuntimeEventEnvelope`
(`schema_version, seq, event, kind, thread_id, turn_id, item_id,
timestamp, created_at, payload`) exists in
`crates/protocol/src/runtime/mod.rs:11-27`, and `WebhookHookSink` exists
in `crates/hooks/src/lib.rs:207` but is wired to no config surface. The
outbox composes those two primitives with a config-gated file append and
the existing hook emit sites — no new event model, no new transport.

## 2. Scope

| Surface | Fires the outbox? | Note |
| --- | --- | --- |
| TUI interactive runtime | **Yes** | all seven events (§4.6) |
| `codewhale exec` (headless) | **Yes** | `turn_start` + `turn_end` (both terminal paths) |
| app-server / runtime HTTP API | **No** | durable event store already covers API-driven threads (RFC 1364 precedent: hooks ruled TUI-only) |
| ACP | **No** | same boundary as RFC 1364; extension point for a later RFC |
| CLI subcommands (`doctor`, `/config`, …) | **No** | no lifecycle events fire there |
| `workflow` tool | **No** | same as RFC 1364's ruling for hooks |

Hooks parity, stated explicitly: the outbox covers the observer-event
family of the shell-hook system (`turn_end`, `subagent_spawn`,
`subagent_complete`, `session_start`, `session_end`) **plus the two
events the hook surface lacks** — `turn_start` and `turn_stalled`. The
outbox is independent of the hook command list: sub-agent events fire from
the observer site even when no shell hook is configured for them
(`crates/tui/src/tui/ui.rs:1014-1017`), because the outbox is
configuration-gated once at startup, not per event. This RFC cites the
RFC 1364 scope ruling deliberately: upstream has already drawn the
TUI-only boundary for the shell-hook lifecycle surface; the TUI + `exec`
outbox is the next cell in that same matrix, and the app-server column
stays empty because the durable store already covers it.

## 3. Scope and structure

One PR, layered for review along these seams.

### Config + writer + `WebhookHookSink` token/transport refactor

- add the `[lifecycle_outbox]` table to `crates/config/src/lib.rs`
  (`LifecycleOutboxToml`: `path`, `webhook_url`, `webhook_token`) with the
  absent/empty-`path` = off opt-in idiom, and parse it in the TUI config
  (`crates/tui/src/config.rs:2784-2788`)
- add the JSONL writer module `crates/hooks/src/lifecycle_outbox.rs`
  (envelope assembly, seq recovery, bounded payloads, single writer task)
- refactor `WebhookHookSink` to accept an optional bearer token and share
  its transport with the outbox fan-out (`crates/hooks/src/lib.rs:207-277`);
  the sink was previously dead code with no config surface — this PR gives
  it one

Non-goals: no new runtime dependencies (tokio fs append; the sink's existing
`reqwest` client), no per-event config switches (a
`[lifecycle_outbox.events]` table like `[notifications.events]` is a
plausible later extension, not v1).

### Emit sites per surface — TUI, then exec

Two groups, one per surface, both reviewable against the event table in
§4.6:

- **TUI**: `turn_start`, `turn_end` (completion **and** the
  engine-disconnect folded `turn.failed`), `turn_stalled` at the stall
  watchdog, `subagent_spawn`/`subagent_complete` at the observer site,
  `session_start`/`session_end` at the hook fire sites — every payload
  carrying `workspace`, sub-agent payloads carrying `subagent` (§4.4)
- **exec**: `turn_start` at `Op::SendMessage` dispatch and `turn_end` at
  the terminal `TurnComplete`, **plus** the folded `turn.failed` on the
  "engine channel closed before a terminal receipt" path so every emitted
  `turn_start` has a matching `turn_end`

Non-goals: no app-server/ACP sites (scope table), no transcript or tool
payload content — payloads are bounded and pre-redacted only.

### Integration tests

- the consolidated harness module
  `crates/tui/tests/integration/lifecycle_outbox_exec.rs` (registered in
  `crates/tui/tests/integration/main.rs:105`): real `exec` subprocess, a
  `wiremock` OpenAI-compatible stub, isolated `$HOME`/XDG envs, outbox
  line assertions, seq-recovery-across-processes — the recipe
  `exec_turn_usage.rs` / `exec_persistent_service.rs` established
- TUI-side focused tests: stall emit-site tests
  (`crates/tui/src/tui/ui/session_state.rs:1120+`) and the
  engine-disconnect folded-`turn_end` tests

## 4. Design / contract

### 4.1 Config: `[lifecycle_outbox]`

```toml
[lifecycle_outbox]
path = "~/.codewhale/notifications/outbox.jsonl" # unset/empty = OFF
webhook_url = ""     # optional; POSTs events as JSON when set
webhook_token = ""   # optional bearer token for webhook_url
```

- **Opt-in, additive.** Unset or empty `path` = feature off = behavior
  identical to a release without the table (`LifecycleOutboxToml` at
  `crates/config/src/lib.rs:1553-1575`; handle construction at
  `crates/tui/src/tui/app/init.rs:574-586` returns
  `LifecycleOutbox::disabled()` when no path resolves).
- **Table family, not feature flag.** `[lifecycle_outbox]` belongs to
  upstream's config-table family (`[hook_sinks]`, `[notifications]`,
  `[hooks]`, `[transcript]`), not `[features]`, because it carries data
  (path/url/token), not a boolean.
- **Documented example** in `config.example.toml:1192-1203` sits beside the
  `[notifications]` block; the config key drives the real path.

### 4.2 JSONL file contract: `RuntimeEventEnvelope`

Each line is one complete envelope
(`crates/protocol/src/runtime/mod.rs:11-27`):

```json
{"schema_version": 1, "seq": 3, "event": "turn_start", "kind": "turn.started",
 "thread_id": "…", "turn_id": "…", "item_id": null, "timestamp": "…",
 "created_at": "…", "payload": {…}}
```

- **`schema_version`** is `RUNTIME_EVENT_ENVELOPE_SCHEMA_VERSION = 1`
  (`crates/protocol/src/runtime/mod.rs:7`), the same envelope the runtime
  event store and `/v1` replay API already use — the outbox introduces no
  second model.
- **`seq` recovery.** Monotonic per file, assigned by the writer in drain
  order (`crates/hooks/src/lifecycle_outbox.rs:264-273`). On first write
  the writer recovers the last complete line's `seq` from a **bounded
  64 KiB tail scan** (`recover_last_seq`,
  `crates/hooks/src/lifecycle_outbox.rs:336-389`): outbox lines are
  bounded far below that window, so the last complete line is always
  inside it, and a torn trailing line from a crash mid-write is ignored
  (the previous newline-terminated line wins).
- **Single writer.** `emit` never blocks the caller: it enqueues on an
  unbounded channel and a single lazily-spawned writer task serializes
  appends in order (`crates/hooks/src/lifecycle_outbox.rs:156-169`,
  `ensure_writer_spawned` at `:188-227`). Appends use O_APPEND with line +
  newline in a single write, mirroring `JsonlHookSink`; two processes
  sharing one file can interleave *lines* but never splice one. With no
  tokio runtime available (or after the writer task is gone) events are
  dropped with a warning — the outbox is observability, not control flow.
- **Cross-process caveat.** Seq uniqueness is per process recovery, so
  processes sharing one file can repeat seq values — use one file per
  process for strict uniqueness.

### 4.3 Bounded payloads

Payloads are constructed by the emit sites from bounded, pre-redacted
fields only — never raw tool arguments, environment, or full transcript
text. Free-form fields pass through `bounded_text`
(`crates/hooks/src/lifecycle_outbox.rs:391+`), which strips control bytes
and ANSI escapes, collapses whitespace, truncates to a character ceiling
(Unicode-scalar counted, UTF-8 safe) with the `…` marker, and enforces
the same ceilings as the desktop notification payloads:
`OUTBOX_HEADLINE_MAX_CHARS` 80, `OUTBOX_DETAIL_MAX_CHARS` 120,
`OUTBOX_PREVIEW_MAX_CHARS` 200 (`crates/hooks/src/lifecycle_outbox.rs:57-59`).
This is the invariant that makes the 64 KiB tail-scan recovery correct:
a line can never approach that window.

### 4.4 Routing fields: `workspace` on every event, `subagent` on sub-agent events

Post-fix state, and a review checkpoint
below guards it: **every** emit site puts `"workspace"` (the resolved
workspace path) in the payload so a consumer can route each event to its
project without guessing, and sub-agent events
(`subagent_spawn`, `subagent_complete`) additionally carry `"subagent"`
(the sub-agent id) alongside the existing `"agent_id"`
(`crates/tui/src/tui/ui.rs:1026-1029`). Before this fix only TUI
`session_start` and exec `turn_start` carried the routing field and a
consumer dropped everything else fail-closed — the RFC's own test plan
(§7) now asserts the field on every event type end to end.

### 4.5 Webhook transport

With `webhook_url` set, every event is additionally POSTed as
`{"at": "<ISO 8601>", "event": {…}}` by `WebhookHookSink`
(`crates/hooks/src/lib.rs:207-277`) — the previously unwired sink, now
shared between the shell-hook sinks and the outbox fan-out:

- **Bearer token**: `new_with_token` attaches
  `Authorization: Bearer <token>` when `webhook_token` is set; ignored
  when `webhook_url` is unset.
- **Bounded retries**: delivery uses **bounded retries inside the sink**
  (two retries with exponential back-off, 200 ms / 400 ms, 10 s request
  timeout). After the retries are exhausted the failure is logged and
  dropped, **never fed back into the agent loop**, and a failing webhook
  never blocks the local append — the webhook fan-out runs after the file
  append and is independent of its result
  (`crates/hooks/src/lifecycle_outbox.rs:283-297`). This wording
  ("bounded retries inside the sink; failures logged and dropped") is the
  one the docs and changelog must use — the issue draft's "never retried"
  would misdescribe the transport.

### 4.6 Events emitted

| Event | Kind | Site (file:line) |
| --- | --- | --- |
| `turn_start` | `turn.started` | TUI `crates/tui/src/tui/ui/event_loop.rs:1452`; exec `crates/tui/src/lib.rs:11673` |
| `turn_end` | `turn.completed` / `turn.failed` / `turn.interrupted` | TUI `TurnComplete` `crates/tui/src/tui/ui/event_loop.rs:1984`; exec `crates/tui/src/lib.rs:12082` (kind projected from terminal status) |
| `turn_end` (failure path) | `turn.failed` | TUI engine-disconnect `crates/tui/src/tui/ui/session_state.rs:387`; exec channel-closed `crates/tui/src/lib.rs:12279` |
| `turn_stalled` | `turn.stalled` | stall watchdog `recover_stalled_runtime_turn` `crates/tui/src/tui/ui/session_state.rs:253`, emit at `:290` |
| `subagent_spawn` | `subagent.spawned` | observer site `crates/tui/src/tui/ui.rs:1017` (fires even with no hooks configured) |
| `subagent_complete` | `subagent.completed` | observer site `crates/tui/src/tui/ui.rs:1037` |
| `session_start` | `session.started` | hook fire site `crates/tui/src/tui/ui/event_loop.rs:479` |
| `session_end` | `session.ended` | hook fire site `crates/tui/src/tui/ui/event_loop.rs:596` |

Guarantees both surfaces uphold: **every `turn_start` has a matching
`turn_end`.** A turn killed mid-flight by a disconnected engine (stream
idle/error, crash) emits the folded `turn_end` (`turn.failed`) from the
failure path — the TUI from `handle_turn_orphan`-style recovery in
`session_state.rs:387`, exec from the channel-closed path in
`crates/tui/src/lib.rs:12279`. A supervisor never sees an orphaned
in-progress turn.

## 5. Known limitations

- **`turn_stalled` vs the client stream-idle clock race.** The stall
  watchdog fires only when a turn outlives the stall threshold, but the
  client's stream-idle timeout can terminate a silent turn first —
  observed in the bridge E2E harness (§9.4 of the field report): the idle
  timeout always won and `turn_stalled` was not triggerable live; the
  signal was proven via its emit-site test instead. If stall detection is
  a hard product requirement, the watchdog threshold and the client idle
  clock need reconciliation (watchdog below the idle timeout, or an
  engine-level no-event clock). This RFC ships the signal and names the
  race; it does not retune either clock.
- **No app-server / ACP coverage** (§2). The durable event store already
  covers API-driven threads; wiring the outbox into the app-server is a
  deliberate future RFC, not this one.
- **Tailer cursor state is in-memory.** The outbox writes the file; how a
  consumer tails it is the consumer's contract. A consumer that keeps its
  cursor only in memory and the outbox file being deleted + recreated can
  skip events below the stale cursor (observed in the bridge E2E harness,
  §8.5.4 of the field report) — the recovery is restart-replay, or the
  consumer should reset its cursor on `NotFound` / detect a seq rewind.
- **Cross-process seq uniqueness** (§4.2): per-process recovery means one
  file per process for strict `seq` uniqueness.
- **Fresh `exec` runs have an empty `thread_id` on `turn_start`.** The
  session id is minted only at persistence time; a supervisor correlating
  runs can key on process + file.
- **Webhook-only configuration (url without `path`) parses losslessly but
  does not activate the handle.** The file path is the feature gate;
  webhook-only delivery can be lifted later if wanted.

## 6. Compatibility

- With `[lifecycle_outbox]` unset or empty `path`, zero behavior change:
  the handle is a disabled no-op and no file or HTTP request is ever made
  (`LifecycleOutbox::disabled`, `crates/hooks/src/lifecycle_outbox.rs:140-144`).
- No new runtime dependencies (JSONL append uses tokio fs; the webhook
  reuses the sink's existing `reqwest` client).
- No change to the hook system: shell hooks keep their config shape,
  payloads, and TUI-only scope; the outbox rides alongside the existing
  hook fire sites without altering them.
- The envelope is the existing `RuntimeEventEnvelope` — no schema fork,
  no second event model.

## 7. Test plan

Unit (in-module `#[cfg(test)]`), config, and integration harness layers,
matching upstream's test layout:

- **`crates/hooks`** — `crates/hooks/src/lifecycle_outbox.rs:423+`:
  append/schema shape; seq recovery across reopen; missing/empty file;
  torn trailing line ignored; emit ordering under the writer task;
  disabled-outbox no-ops (incl. webhook-only-without-path);
  `bounded_text` ceilings incl. UTF-8 boundaries and control-byte strip;
  the routing-field contract surviving the envelope round trip
  (`payload_workspace_and_subagent_fields_survive_the_round_trip`,
  `:487`); wiremock webhook bearer-token delivery (`:758`) and webhook
  failure as an error, not a panic (`:791`).
- **`crates/config`** — `crates/config/src/tests.rs:63`
  (off-by-default + full-table parse) and `:102` (webhook optional);
  **`crates/tui`** — `crates/tui/src/config/tests.rs:711`
  (`tui_config_parses_lifecycle_outbox_table`).
- **TUI focused** — `crates/tui/src/tui/ui/session_state.rs:1120+`
  (`stall_outbox_tests`): enabled outbox writes one `turn_stalled` line
  naming the wedged turn; disabled outbox writes nothing and recovery is
  unchanged. Engine-disconnect tests: an in-progress turn gets the folded
  `turn.failed` `turn_end`; a disconnect with no in-progress turn
  fabricates nothing.
- **Integration harness** —
  `crates/tui/tests/integration/lifecycle_outbox_exec.rs` (registered
  `crates/tui/tests/integration/main.rs:105`), filter names
  `integration::lifecycle_outbox_exec::…`: exec `turn_start`/`turn_end`
  assert `payload.workspace` equals the `--workspace` directory end to
  end; no-config writes no file; `outbox_seq_recovers_across_processes`.
  Uses the established harness recipe (wiremock OpenAI-compatible stub,
  `CARGO_BIN_EXE_codewhale-tui`, isolated `$HOME`/XDG, `lock_test_env`).
- **Posture** — `crates/tui/src/lib.rs:6031-6044`
  (`doctor_lifecycle_outbox_posture_line`): `off (default)` /
  `on (path: …)`, tested for both states.

## 8. Review checkpoints

PR 1 is accepted only if:

- unset/empty `path` leaves the feature off with a no-op handle (tested)
- seq recovery is bounded (tail scan, not full re-read) and torn trailing
  lines are ignored (tested)
- `emit` never blocks the caller and ordering is preserved under the
  writer task (tested)
- payload bounds are character-counted and UTF-8 safe, with the
  truncation marker (tested)
- the `WebhookHookSink` refactor keeps the existing `HookSink` behavior
  for shell hooks and adds the bearer token only as an optional parameter
  (existing sink tests still pass)
- the docs use "bounded retries inside the sink; failures logged and
  dropped, never fed back into the agent loop" — not "never retried"

PR 2 is accepted only if:

- every emit site's payload carries `workspace`, and sub-agent events
  carry `subagent` alongside `agent_id` (the integration tests assert
  `workspace` end to end)
- `turn_end` fires after completion state is settled, with the kind
  projected from the turn status
- the TUI engine-disconnect path and the exec channel-closed path both
  emit the folded `turn.failed` only when an in-progress turn actually
  exists — no fabricated pairs
- no payload contains raw tool arguments, environment, or transcript text

PR 3 is accepted only if:

- the integration harness runs under the consolidated
  `integration::lifecycle_outbox_exec::…` filter names and both surfaces'
  assertions pass against a real subprocess
- the stall and disconnect tests cover both the enabled-outbox write and
  the disabled-outbox unchanged-behavior sides
