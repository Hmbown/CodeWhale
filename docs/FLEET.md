# Agent Fleet

Agent Fleet is the local-first control plane for durable multi-worker runs. It
is **not** a separate execution engine: a fleet worker is a headless
`codewhale exec` run that the fleet launches and tracks durably. See
[AGENT_RUNTIME.md](AGENT_RUNTIME.md) for how sub-agents, `exec`, and the fleet
converge on one durable runtime. In product language, a user may still "open a
sub-agent"; in architecture language, durable nested work should be a
fleet-backed worker with a role.

Use Fleet rather than short-lived `agent` fanout whenever the work
needs retry, sleep/restart survival, remote execution, receipts, or a ledgered
audit trail. The initial CLI surface is:

For a guided start-to-monitor walkthrough that combines Fleet task specs with
Workflow authoring, see [Fleet + Workflow Tutorial](FLEET_WORKFLOW_TUTORIAL.md).

```sh
codewhale fleet init
codewhale fleet run tasks.json --max-workers 4
codewhale fleet status
codewhale fleet inspect <worker-id>
codewhale fleet logs <worker-id>
codewhale fleet artifacts <worker-id>
codewhale fleet interrupt <worker-id>
codewhale fleet restart <worker-id>
codewhale fleet resume <run-id>
codewhale fleet stop --all
```

`codewhale fleet resume <run-id>` is the restart-recovery verb: it replays the
ledger, reconciles any in-flight lease whose worker stopped heartbeating
(retrying within the task's budget, else failing and escalating per the alert
policy), and prints the post-resume status. It launches no new work and is
idempotent, so it is safe to run after a manager exit, laptop sleep, or runtime
restart.

Fleet state is stored under the workspace in `.codewhale/fleet.jsonl`. Worker
logs and adapter logs are stored under `.codewhale/fleet/` and
`.codewhale/fleet-host/`.

## Authoring agent profiles (`/fleet setup`)

`/fleet setup` (also `/fleet setup edit` / `new`) opens an in-TUI wizard for
authoring a reusable agent-team profile. Bare `/fleet` and the
`roster`/`roles`/`profiles`/`party` aliases open the roster (the saved profiles).
`/fleet status` opens the worker-status view; `/subagents` is a
compatibility shortcut for that status view.

The wizard is progressive: you make one focused choice at a time — a **role**,
then a **model** (`inherit`, or a concrete model from *any configured
provider*, not only the one the parent session is currently using), then a
**thinking tier** (`inherit`, `off`, `low`, `medium`, `high`, `max`, or `auto`)
— and then review the full posture (route, thinking, permissions, tools,
scope, and review policy) before doing anything. On Review, press **`s`** to
choose where the profile lives:

- **Project** (default) writes `.codewhale/agents/<role>.toml` and can travel
  with this repository.
- **Personal** writes `$CODEWHALE_HOME/agents/<role>.toml` and is available in
  every repository on this machine. A project profile with the same id still
  overrides the personal profile for that project.

Profile scope controls where a role definition is reusable; it does not widen
the authority of a running operation. To coordinate several nearby
repositories, start Codewhale from their shared parent directory so that parent
is the workspace. Explicit trusted external paths or Full Access can still
change what tools may reach; workers inherit the active trust and permission
posture, never the profile's storage scope.

Picking a concrete model pins its provider explicitly: the saved profile records both
`model` and `provider` fields, so the route it names doesn't depend on
whichever provider happens to be active when the profile is later loaded.
Pressing **Enter** ("start") on the review step previews the exact starter
profile TOML inline on that same screen; nothing is written until you save it.
The `provider` field may be a built-in provider id such as `openrouter` or a
user-named OpenAI-compatible provider configured under `[providers.<name>]`
such as `lm-studio`; the launch path preserves that id and fails closed if the
provider is not configured.

When a provider is configured, the review step also offers model-assisted
drafting behind an explicit preview-before-save gate:

- Press **`m`** to have your first configured model draft the profile. The
  draft arrives sanitized and bounded — permissions stay at the **fleet floor**
  (no shell, no trust, approval required) regardless of what the model
  proposes.
- **Drafting is not saving.** The exact rendered TOML preview renders
  inline on the review step (not in a separate scrollable viewer), so nothing
  is saved until you press **`g`** or **Enter** to save (or press `m` again
  to redraft). Saving writes the profile to the project or personal scope
  shown in the preview.

## Naming: Modes, Workflow, and Fleet

These names describe different layers, not competing systems. Plan and Act are
the everyday work modes. Operate accepts ordinary messages and keeps the
parent's normal tool surface under the same approval, sandbox, shell, ask-rule,
and repository protections as Act. It prefers background Fleet workers for
independent, parallel, isolated, or long-running work, but does not require a
worker for every executable step. Workflow is an optional orchestration overlay
for work that needs ordering, gates, shared budgets, replay, or deterministic
fan-in.

- **Workflow** is the repeatable plan and user-facing orchestration
  overlay: a script/IR that decides which phases and agents run next, keeps
  intermediate results out of the main conversation, and can be inspected or
  rerun. A Workflow run should have a visible progress view and a clear active
  header state instead of feeling like a hidden background task.
- **Fleet** is the durable sub-agent configuration and execution substrate:
  slots, profiles, per-slot models, tool posture, local/SSH hosts, trust
  policy, leases, heartbeats, logs, receipts, and status APIs.
- **High fan-out** is a behavior of a Workflow run, not a separate system:
  when a phase needs many workers at once, Workflow dispatches them as a
  Fleet-backed run (durable workers, receipts, goal re-dispatch) rather than
  reviving prompt-only sub-agent fanout.
- **Fan-in is explicit:** when the user needs one combined result, an owner
  aggregates, verifies, and synthesizes the worker receipts. Independent tasks
  may finish separately; dispatch is never presented as completion.

UI guidance: keep the main transcript calm. A Workflow run should appear as a
compact progress card plus Work/Agents sidebar rows with phase names, worker
counts, receipts, and nested indentation for child workers. Use the whale mark
sparingly as an active header/status signal; avoid repeating emoji-heavy rows
for every worker.

## Manager-owned operations

When parallel work must return one combined answer, use a manager-owned
operation instead of a flat `agent` fan-out:

1. **Cast one manager** (operator or workflow orchestrator).
2. **Fan out** child tasks through `workflow` (`task()`, `parallel()`,
   `pipeline()`, `phase()`) or a single manager session that owns the children.
3. **Wait** for child receipts or completion events.
4. **Aggregate and verify** load-bearing claims before treating them as facts.
5. **Synthesize** one result the operator can depend on.

Raw `agent` fan-out is appropriate only for independent, fire-and-forget work
where no single fan-in result is required. If results must be merged, compared,
or verified, route through `workflow` so the manager owns fan-in.

## Workflow on Fleet

The intended high-capability path is agent-authored. When the main agent
decides a task needs more durable coordination than turn-by-turn sub-agent
calls, it drafts a Workflow script/IR, presents the run plan according to the
active permission mode, and the runtime compiles it into typed Fleet work.

Fleet remains the sub-agent config surface. It owns slot count, role profiles,
saved route pins or inheritance, tool posture, launch concurrency, and the ledger.
Workflow owns only the orchestration plan: branch, sequence, loop, expand,
review, and reduce decisions. The workflow script must not get direct shell,
filesystem, network, provider-secret, cancellation, or TUI authority; workers
perform real work as `codewhale exec` processes.

Default Workflow-to-Fleet validation is intentionally bounded:

- 100 total worker agents per workflow run;
- 5 recursive Fleet rings;
- bounded loops only (`max_iterations` required);
- bounded dynamic expansion only (`max_children` plus a template required).

These are population limits, not a demand to launch everything at once. A
100-agent workflow should still drain through the configured Fleet worker pool.
Recommended model layouts, such as a DeepSeek Pro orchestrator with Flash
workers in the first ring and cheaper workers farther out, are presets only.
Every slot can inherit the active model or carry an explicit model override.
Inheritance is literal: the model you select in `/model` is the **operator**
(the pinned first row in `/fleet roster`), and any worker whose task spec and
roster profile pin no model runs on that session model. Task-level `model` and
profile `model` overrides still win; route receipts record which source
applied (`task.model`, `agent_profile.model`, or `run.model`).

The setup UI should render this as an expanding grid: an orchestrator plus a
small number of visible sub-agent slots, with Right/Enter drilling into a slot's
next recursive ring rather than trying to show the whole tree at once.

## Task Spec

`codewhale fleet run` accepts JSON or TOML. A minimal JSON spec:

```json
{
  "name": "local smoke",
  "tasks": [
    {
      "id": "lint",
      "name": "Lint",
      "instructions": "Run the lint check and report failures.",
      "expected_artifacts": ["log"]
    }
  ]
}
```

Workers are optional. If omitted, Codewhale creates local worker slots up to
`--max-workers`.

Task specs are typed in Rust and keep verification data separate from worker
transcripts. A task can declare:

- `id`, `name`, `description`, `objective`, and `instructions`
- `worker` role, tool profile, tools, and required capabilities
- `workspace` root, required files, writable paths, and environment allowlist
- `input_files`, extra `context`, `budget`, `timeout_seconds`, and `retry_policy`
- `expected_artifacts`, `scorer`, `tags`, and free-form `metadata`

Workers write bounded artifact files under `.codewhale/fleet/` and ledger only
the artifact refs: kind, path, checksum, MIME type, and size. Receipts record
`pass`, `fail`, `partial`, `skip`, or `timeout`; failed receipts may also mark
the source as `transport`, `task`, or `verifier`. `codewhale fleet status`
surfaces those failure-source counts separately.

Deterministic built-in scorers are `exit_code`, `file_exists`, `regex_match`,
and `json_path`. Specs may also declare `command`,
`code_whale_verifier_prompt`, or `manual`; those record a partial receipt until
an explicit verifier pass completes.

### Using Role Presets

Tasks can reference a role name, and the fleet manager fills in defaults
from the role registry. Built-in roles (`smoke-runner`, `reviewer`, `builder`,
`read-only`) are always available; define your own in `[fleet.roles]`.

```json
{
  "name": "smoke check",
  "tasks": [
    {
      "id": "lint",
      "name": "Lint check",
      "instructions": "Run lint and report failures.",
      "worker": { "role": "smoke-runner" },
      "expected_artifacts": ["log"]
    }
  ]
}
```

The task inherits the role's tool profile, budget, and timeout. You can
override any field in the task spec:

```json
{
  "id": "deep-review",
  "name": "Deep review",
  "instructions": "Review the entire crate for soundness issues.",
  "worker": {
    "role": "reviewer",
    "tools": ["cargo", "rg", "git"],
    "capabilities": ["rust"]
  },
  "input_files": ["crates/**/*.rs"],
  "budget": { "max_tokens": 32000 },
  "expected_artifacts": ["log", "report"],
  "scorer": { "kind": "regex_match", "path": ".codewhale/fleet/report.md", "pattern": "finding|all clear" }
}
```

### Multi-Task Run Example

A single fleet run can dispatch several independent tasks in parallel:

```json
{
  "name": "CI gate",
  "tasks": [
    {
      "id": "check",
      "name": "Compile check",
      "instructions": "Run cargo check --workspace and report errors.",
      "worker": { "role": "builder" },
      "expected_artifacts": ["log"],
      "scorer": { "kind": "exit_code" }
    },
    {
      "id": "clippy",
      "name": "Clippy lint",
      "instructions": "Run cargo clippy --workspace and report warnings.",
      "worker": { "role": "reviewer", "tools": ["cargo", "cargo-clippy"] },
      "expected_artifacts": ["log"],
      "scorer": { "kind": "exit_code" }
    },
    {
      "id": "security",
      "name": "Secret audit",
      "instructions": "Search for plaintext secrets and report any matches.",
      "worker": { "role": "read-only", "tools": ["rg"] },
      "input_files": ["crates/**/*.rs"],
      "expected_artifacts": ["log", "report"],
      "retry_policy": { "max_attempts": 1 }
    }
  ]
}
```

## Alerts

Fleet alerting is disabled by default. A caller must supply an enabled alert
config before anything is sent. Routes match typed fleet event classes, not log
strings:

- `stale`
- `restart_exhausted`
- `needs_human`
- `budget_exceeded`
- `verifier_failed`
- `run_completed`

Adapter config stores environment variable names, not secret values. Send-time
code resolves those names from the environment or a future secrets provider.
Ledger records store only audit labels such as `slack`, `webhook`, or
`pagerduty`; task specs persisted in the ledger redact webhook URLs and routing
keys.

Example alert config shape:

```json
{
  "enabled": true,
  "dry_run": true,
  "routes": [
    {
      "events": ["stale", "restart_exhausted", "verifier_failed"],
      "adapter": "ops-slack"
    },
    {
      "events": ["restart_exhausted"],
      "adapter": "pager"
    }
  ],
  "adapters": {
    "ops-slack": {
      "kind": "slack",
      "webhook_env": "CODEWHALE_FLEET_SLACK_WEBHOOK",
      "channel": "#codewhale-fleet"
    },
    "pager": {
      "kind": "pager_duty",
      "routing_key_env": "CODEWHALE_FLEET_PAGERDUTY_ROUTING_KEY",
      "severity": "critical"
    }
  }
}
```

Use dry-run to inspect a redacted adapter payload without sending:

```sh
codewhale fleet alert-dry-run \
  --event stale \
  --run-id fleet-demo \
  --worker-id fleet-demo-local-1 \
  --task-id release-triage \
  --reason "worker heartbeat stale since 2026-06-13T02:00:00Z" \
  --adapter slack
```

The payload includes the run id, worker id, task id, status, short reason, and
safe inspection commands such as `codewhale fleet status` and
`codewhale fleet inspect <worker-id>`. Endpoints, webhook secrets, and
PagerDuty routing keys are shown as `<redacted:env:...>`.

## Status Surfaces

`codewhale fleet status` shows compact counts for queued, running, completed,
partial, failed, restarted, escalated, cancelled, stale, and verifier/transport
failure sources. `inspect` shows the worker state plus the current task
objective, role, host, heartbeat, latest event, artifact refs, latest error, and
alert state. `logs` prints bounded log artifact contents, and `artifacts` lists
artifact refs without embedding large payloads.

The Runtime API exposes the same ledger-backed projection behind the existing
runtime auth middleware:

```text
GET  /v1/fleet/runs
GET  /v1/fleet/runs/{run_id}
GET  /v1/fleet/runs/{run_id}/workers
GET  /v1/fleet/workers/{worker_id}
POST /v1/fleet/workers/{worker_id}/interrupt
POST /v1/fleet/workers/{worker_id}/restart
POST /v1/fleet/runs/{run_id}/stop
```

Action endpoints call the same manager controls as the CLI and record their
decisions in the fleet ledger.

## Manager-Agent Runbook

Manager agents should treat Fleet operations as typed, ledgered control-plane
work. Start with `codewhale fleet status`, then inspect one run or worker with
`codewhale fleet inspect <worker-id>`, `logs`, and `artifacts`. Use direct
reads of `.codewhale/fleet.jsonl`, host logs, or remote files only when the
typed CLI/API surface cannot provide the required evidence.

Classify the worker before taking action:

- `transient failure`: stale heartbeat, host timeout, interrupted transport,
  retryable provider/network error, or an adapter status that can plausibly
  recover without changing the task.
- `task failure`: the worker completed but produced an incorrect result,
  domain failure, missing required artifact, or explicit task-level error.
- `verifier failure`: the worker result exists, but the scorer/verifier failed,
  timed out, or disagrees with the receipt.
- `needs-human`: missing authority, secret request, destructive operation,
  repeated restart exhaustion, ambiguous product decision, or conflicting
  evidence that the manager cannot resolve from typed artifacts.

Choose one typed action:

- Restart a worker only when the failure is transient, retry budget remains,
  the task is idempotent or retry-safe, and no permission or secret boundary is
  involved: `codewhale fleet restart <worker-id>`.
- Interrupt or stop only when the current task is unsafe to continue or the
  operator explicitly asks for cancellation: `codewhale fleet interrupt
  <worker-id>` or `codewhale fleet stop --all`.
- Do not restart pure task failures by default; preserve artifacts and hand the
  receipt to the task owner unless the task spec says retrying can produce new
  evidence.
- For verifier failures, inspect scorer inputs and artifact refs first. If the
  verifier cannot be corrected through typed fleet actions, escalate for human
  review.
- For `needs-human`, draft an escalation instead of sending it unless alert
  config explicitly authorizes sending.

Safe Slack or PagerDuty draft:

```text
Codewhale fleet needs attention
Run: <run-id>
Worker: <worker-id>
Task: <task-id or unknown>
Classification: <transient failure | task failure | verifier failure | needs-human>
Reason: <one sentence, no secrets>
Latest typed evidence: codewhale fleet inspect <worker-id>; codewhale fleet artifacts <worker-id>
Safe log excerpt: <3 lines max or "see artifact <ref>">
Requested decision: <restart approval | verifier review | task owner review | permission decision>
```

Post-run summaries should include the run id, workers checked, classification,
typed action taken or drafted, expected ledger effect, artifact refs reviewed,
and next owner. Keep summaries bounded; link artifact refs instead of copying
full logs or transcripts.

The bundled `fleet-manager` skill mirrors this runbook for manager agents. It
is a first-party system skill and should be discoverable through the normal
skill registry after system skills are installed or refreshed.

## Host Adapters

The host adapter boundary supports local child processes and explicit SSH
workers. Adapters expose the same operations: start, read status, read bounded
logs, interrupt, restart, stop, and cleanup.

Local workers run as child processes with stdin closed and stdout/stderr written
to bounded fleet host logs. They inherit only a small safe base environment
such as `PATH` and explicitly allowlisted variables.

SSH workers run through the system `ssh` client with `BatchMode=yes` and a
bounded connect timeout. Remote environment variables are sent with OpenSSH
`SendEnv`; values are not embedded in the local ssh argv or fleet logs.

Example SSH worker spec:

```json
{
  "id": "builder-1",
  "name": "Builder 1",
  "host": {
    "kind": "ssh",
    "host": "builder.example.com",
    "user": "codewhale",
    "port": 22,
    "identity": "~/.ssh/codewhale_fleet",
    "working_directory": "/srv/codewhale/work",
    "env_allowlist": ["CODEWHALE_PROFILE"],
    "codewhale_binary": "/usr/local/bin/codewhale"
  },
  "capabilities": ["local", "linux", "tests"],
  "max_concurrent_tasks": 1
}
```

Defaults are intentionally conservative:

- no hosted control plane or cloud provisioning is enabled;
- SSH requires an explicit host, working directory, and Codewhale binary path;
- secret-like environment names such as `TOKEN`, `SECRET`, `PASSWORD`,
  `API_KEY`, and `PRIVATE_KEY` are rejected from adapter allowlists;
- secrets should remain in Codewhale config providers or remote host config,
  not in task instructions, argv, or fleet logs.

## Security and Trust Boundaries

Agent Fleet enforces a trust-level model that separates workers into four tiers.
The trust level determines what a worker can access (secrets, network, workspace
writes) and how it must prove its identity before being granted those privileges.

### Trust Levels

| Level | Access | Requires |
|-------|--------|----------|
| `sandbox` | No network, no secrets, writes only to `.codewhale/fleet/` | Nothing — default for new workers |
| `local` | Workspace reads, gated writes, configured secrets | Local process (same uid) |
| `remote-verified` | Network access, bounded capability grants, configured secrets | SSH host-key verification or equivalent attestation |
| `operator` | Full access to all secrets, unrestricted writes, any action | Operator-owned machine |

The default trust level is `sandbox`. Operators must explicitly raise trust for
SSH or container workers through the security policy.

### Security Policy

A fleet run may carry an optional `security_policy` block that defines the
default trust level, which secrets workers may resolve, what capabilities are
granted, and a ceiling on the maximum trust level:

```json
{
  "security_policy": {
    "default_trust_level": "sandbox",
    "allowed_secrets": [
      {"key": "GH_TOKEN", "source": "env"},
      {"key": "CODEWHALE_API_KEY", "source": "keyring"}
    ],
    "capability_grants": [
      {
        "capability": "network",
        "scope": "github.com",
        "reason": "PR review needs GitHub API access"
      }
    ],
    "max_trust_level": "remote_verified",
    "require_identity_verification": true
  }
}
```

When a run has no explicit `security_policy`, workers inherit conservative
defaults: `sandbox` trust, no secrets, no capability grants, and no identity
verification requirement.

### Secret References

Secrets are never stored as plaintext in task specs, alert configs, or worker
definitions. Instead, every secret is a `FleetSecretRef` — a key name plus an
optional source hint that tells the fleet manager where to resolve the value:

```json
{"key": "GH_TOKEN", "source": "env"}
```

Supported sources:
- `"env"` — resolve from a process environment variable
- `"keyring"` — resolve from the OS keyring (macOS Keychain, Windows Credential Manager, Linux Secret Service)
- `"file"` — resolve from `~/.codewhale/secrets/`
- absent — try all sources in default order (store first, then env)

Secret refs are redacted in logs and ledger entries: `<secret:env.GH_TOKEN>`.

### Worker Authentication

Workers authenticate to the fleet manager using one of three methods:

- **None** — local workers sharing the same uid (default)
- **SSH key** — with optional host-key fingerprint pinning and known-hosts
  verification. The `host_key_fingerprint` field (SHA256:...) pins the expected
  server key, preventing MITM attacks on first connection.
- **Token** — a bearer token resolved from a `FleetSecretRef`, useful for remote
  workers behind a fleet proxy.
- **mTLS** — mutual TLS with a client certificate and a secret-backed private key.

SSH workers should always set `host_key_fingerprint` in production:

```json
{
  "id": "builder-1",
  "name": "Builder 1",
  "trust_level": "remote_verified",
  "host": {
    "kind": "ssh",
    "host": "builder.example.com",
    "user": "codewhale",
    "port": 22,
    "identity": "~/.ssh/codewhale_fleet",
    "host_key_fingerprint": "SHA256:aLGqZo1M6c...",
    "known_hosts": "~/.ssh/known_hosts",
    "working_directory": "/srv/codewhale/work",
    "env_allowlist": ["CODEWHALE_PROFILE"],
    "codewhale_binary": "/usr/local/bin/codewhale"
  },
  "capabilities": ["local", "linux", "tests"],
  "max_concurrent_tasks": 1
}
```

### Alert Channel Secrets

Alert channels (Slack, generic webhook, PagerDuty) use `FleetAlertEndpoint`
instead of raw URLs. The webhook URL can be provided inline for non-sensitive
endpoints, or as a secret reference:

```json
{
  "kind": "slack",
  "webhook": {
    "url_ref": {"key": "CODEWHALE_FLEET_SLACK_WEBHOOK", "source": "env"},
    "secret_ref": {"key": "CODEWHALE_FLEET_SLACK_SIGNING_SECRET", "source": "keyring"}
  }
}
```

The `secret_ref` field provides an optional HMAC secret for webhook payload
signing, never stored in plaintext.

### Config File

The `[fleet]` table in `config.toml` sets global trust policy defaults:

```toml
[fleet]
default_trust_level = "sandbox"
require_identity_verification = true
max_trust_level = "operator"

[fleet.exec]
# Recursion depth shares ONE axis with standalone sub-agents — a fleet worker
# IS a headless sub-agent. 0 blocks child agents (the root worker still runs);
# 3 is the default; explicit config clamps to the shared safety ceiling.
max_spawn_depth = 3
```

These defaults apply to fleet runs that don't carry their own `security_policy`.
Per-run policies always override the config defaults.

### Capability Grants

Capability grants are additive, scoped permissions that authorize specific
actions. By default, workers get no grants (least privilege). Common grants:

- `"network"` with scope `"github.com"` — allow outbound HTTP to GitHub
- `"git-push"` — allow `git push` to remotes
- `"provider-secrets"` — allow accessing provider API keys
- `"release"` — allow release-related operations (tagging, publishing)
- `"workspace-write"` with scope `"crates/tui/**"` — allow writes within a path

### Environment Sanitization

The host adapter layer enforces environment sanitization at worker start:

- Only `HOME`, `PATH`, and platform-specific vars (`SYSTEMROOT`, `COMSPEC`) are
  injected into worker processes by default
- Environment allowlists reject any key containing `SECRET`, `TOKEN`, `PASSWORD`,
  `PASSWD`, `API_KEY`, `CREDENTIAL`, or `PRIVATE_KEY`
- SSH workers only send explicitly allowlisted variables via OpenSSH `SendEnv`
- Secret values are never embedded in worker argv, task instructions, or fleet
  logs — only secret refs appear, and they are always redacted
