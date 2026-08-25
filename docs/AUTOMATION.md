# Automations

Durable, scheduled agent runs. An automation stores a prompt, a schedule,
and the task settings each run should use; the scheduler fires it at the
next scheduled time — across restarts — and records every run. You manage
what exists with `/automation`; the agent creates and edits them with the
`automation` tool.

Related docs:

- [Hooks](HOOKS.md) — fire-and-forget shell commands on lifecycle events
- [Automatic Workflows](AUTOMATIC_WORKFLOWS.md) — in-session orchestration
- [Configuration](CONFIGURATION.md) — `[workflow]` knobs, environment vars

## Quick start

Ask the agent, in plain language:

```
Every weekday at 09:30, open my work repo, pull the latest CI failures,
and file a summary task. Create it as an automation named "ci digest".
```

```
Tomorrow at 14:00, check whether the release tag exists and remind me if
not. One-shot automation, watcher mode.
```

The agent calls the `automation` tool with a `create` action (this needs
approval, like other mutating tools) and shows you the created record,
including the resolved `next_run_at`.

## Creating automations (the `automation` tool)

`action=create` parameters:

| Field | Type | Default | Meaning |
|-------|------|---------|---------|
| `name` | string | required | Human label; used by `/automation list` |
| `prompt` | string | required | The exact prompt submitted on each fire |
| `rrule` | string | required | Schedule (see below) |
| `cwds` | string[] | `[]` | Working directories for the scheduled run |
| `mode` | string | `agent` | Task mode for the run (e.g. `agent`, `act`) |
| `allow_shell` | bool | `false` | Shell access for the scheduled run |
| `trust_mode` | bool | `false` | Trust the workspace for the scheduled run |
| `auto_approve` | bool | `false` | Auto-approve tool calls in the scheduled run |
| `delivery_mode` | `task` \| `watcher` | `task` | See "Watcher mode" below |
| `paused` | bool | `false` | Create it in the paused state |

The same fields are accepted by `action=update` (with `automation_id`),
plus `status: "active" | "paused"`.

### Schedules (`rrule`)

Four forms are supported — ONCE, HOURLY, WEEKLY, and 5-field CRON — all in
**local time** unless the timestamp carries an explicit offset:

```
FREQ=ONCE;AT=2026-08-03T14:30          one-shot; local YYYY-MM-DDTHH:MM[:SS] or RFC3339
FREQ=HOURLY;INTERVAL=6                 every 6 hours from an anchored wall time
FREQ=HOURLY;INTERVAL=1;BYDAY=MO,TU;BYHOUR=9;BYMINUTE=30
FREQ=WEEKLY;BYDAY=MO,WE,FR;BYHOUR=9;BYMINUTE=0
FREQ=CRON;EXPR=*/17 * * * *            standard 5-field cron, local time
```

Notes, straight from the parser's contract:

- **ONCE** accepts `FREQ` and `AT` only. Local times that do not exist
  (spring-forward gap) are rejected; ambiguous times (fall-back overlap)
  use the first occurrence.
- **HOURLY** allows `FREQ,INTERVAL,BYDAY,BYHOUR,BYMINUTE`. `BYHOUR`/
  `BYMINUTE` choose the *initial wall-clock anchor*; `INTERVAL` then
  advances from that anchor. `BYHOUR` is **not** a daily-only filter —
  that is deliberate. Anchored wall times skip nonexistent clock times
  and take the first occurrence of ambiguous ones.
- **WEEKLY** requires `BYDAY`, `BYHOUR`, and `BYMINUTE` together.
- **CRON** allows `FREQ` and `EXPR` only; the expression is standard
  5-field cron (`minute hour day-of-month month day-of-week`) in local
  time, with names (`MON`, `JAN`) supported.

### Watcher mode

`delivery_mode: "watcher"` is for condition-shaped checks ("tell me only
if something changed"). The scheduled run receives your prompt; when there
is nothing to report, the run must return **exactly** `NOTHING_TO_REPORT`
— anything else is treated as a report and surfaced. A watcher run that
ends with the sentinel records no report and quietly consumes its run row.
Use watcher mode for watch-tasks; use the default `task` mode whenever the
run should always do its work and record a result.

## Managing automations (`/automation`)

```
/automation                    list everything (name, schedule, status, next/last run)
/automation show <id>          one automation's record + recent runs
/automation run <id>           fire it now (does not disturb the schedule)
/automation pause <id>         stop firing until resumed
/automation resume <id>        resume a paused automation
/automation delete <id> [--confirm <token>]
```

`delete` asks for a confirmation token it shows you first; nothing is
deleted silently. Every mutating verb of the `automation` tool
(create/update/pause/resume/delete/run) requires approval — a scheduled
run inherits only what you put in its record.

## Where automations live

- Records and run history: `~/.codewhale/automations/` (override with
  `CODEWHALE_AUTOMATIONS_DIR`). A legacy `~/.deepseek/automations/` is
  used only when it exists and the new directory does not.
- Schedules survive restarts: `next_run_at` is derived from the stored
  record, not a live timer, so a missed fire while Codewhale was closed is
  not replayed in a burst — the next fire is the next future occurrence.

## Safety model

- The tool's mutating actions carry both `RequiresApproval` and
  `ExecutesCode` capabilities: approval gates the *scheduling*, and the
  execution consequence stays visible to every capability-derived policy,
  including the child execution envelope.
- A scheduled run starts with the record's own `mode` / `allow_shell` /
  `trust_mode` / `auto_approve` — defaulting to no shell, no trust, and no
  auto-approve. Grant the minimum the task needs.
- Prompts run exactly as stored; treat an automation record like a stored
  credential for intent. `update` exists so you never have to delete and
  recreate around a typo.

## Automations vs hooks vs workflows

| Need | Use |
|------|-----|
| "Run this agent task on a schedule" | Automation |
| "Run this shell command when a session event fires" | [Hooks](HOOKS.md) |
| "Coordinate agents inside this session" | [Automatic Workflows](AUTOMATIC_WORKFLOWS.md) |
| "Emit machine-readable lifecycle events to a file/webhook" | `[lifecycle_outbox]` (see [Configuration](CONFIGURATION.md)) |

An automation can itself use hooks-triggered tooling and can be driven by
external supervisors through the lifecycle outbox — the surfaces compose
instead of overlapping.
