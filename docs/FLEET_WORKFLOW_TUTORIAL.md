# Fleet + Workflow Tutorial

Fleet and Workflow are meant to work together, but they solve different parts
of the problem:

- **Fleet** runs durable workers, records a ledger, keeps logs and artifacts,
  and exposes status/restart/stop controls.
- **Workflow** describes orchestration: phases, branches, reducers, loops, and
  agent leaves that can dispatch through the Fleet/sub-agent runtime.

**Default product path:** ask in natural language. Operate can use direct tools
under the active posture, and prefers one or more background Fleet workers when
work is independent, parallel, isolated, or long-running. Background work keeps
the composer available for more messages. It chooses Workflow only when
ordered phases, gates, shared budgets, or deterministic fan-in add real value;
you do not need to write workflow files for ordinary multi-agent work. Details:
[Automatic Workflows](AUTOMATIC_WORKFLOWS.md).

This tutorial covers the **manual** Fleet task-spec / checked-in Workflow path
for operators who want durable host workers and reviewable specs. A
one-sentence request should still not silently generate `tasks.json`; worker
cards and approval posture make dispatch visible without exposing authoring
mechanics.

## 1. Prepare The Workspace

Run Fleet from the workspace you want workers to inspect or modify:

```sh
codewhale fleet init
```

This creates the workspace ledger at `.codewhale/fleet.jsonl`. Worker logs and
bounded artifacts live under `.codewhale/fleet/`; host adapter logs live under
`.codewhale/fleet-host/`.

If you want named reusable workers, open the TUI and run:

```text
/fleet setup
```

Pick a role, choose whether that profile inherits the operator route or pins a
specific provider/model/thinking tier, review the permissions/tools/route
posture, and save the rendered TOML. Project profiles are saved under
`.codewhale/agents/<role>.toml`. On Review, press `s` before previewing to save
a personal profile under `$CODEWHALE_HOME/agents/<role>.toml`; it is available
across repositories, while a same-id project profile remains the higher-priority
override. Fleet task specs can reference either resolved profile with
`worker.agent_profile` or the shorter `worker.profile` alias.

This makes the Fleet definition cross-repository, not the authority of one
running session. For a multi-repository operation, launch Codewhale from a
shared parent workspace. Profile availability does not grant filesystem access;
the session's workspace, explicit trusted paths, trust mode, and permission
posture remain authoritative.

## 2. Write A Fleet Task Spec

`codewhale fleet run` accepts JSON or TOML. The checked-in
`docs/examples/fleet-dogfood.toml` file is the realistic manual smoke example;
the JSON below shows the same authoring shape with one read-only reviewer and
one bounded docs-note worker. It keeps secrets disabled and caps trust at
`sandbox`.

```json
{
  "name": "docs readiness check",
  "labels": {
    "kind": "tutorial"
  },
  "security_policy": {
    "default_trust_level": "sandbox",
    "max_trust_level": "sandbox",
    "allowed_secrets": [],
    "capability_grants": [],
    "require_identity_verification": true
  },
  "tasks": [
    {
      "id": "map-docs",
      "name": "Map current docs",
      "objective": "Find the docs that describe Fleet and Workflow.",
      "instructions": "Read docs/FLEET.md and docs/WORKFLOW_AUTHORING.md. Report the command surfaces, current limitations, and any confusing gaps.",
      "worker": {
        "role": "reviewer",
        "profile": "reviewer",
        "tools": ["rg", "sed", "git"],
        "model": "deepseek-v4-flash"
      },
      "workspace": {
        "required_files": ["docs/FLEET.md", "docs/WORKFLOW_AUTHORING.md"],
        "writable_paths": [],
        "environment": {
          "required": [],
          "allowlist": []
        }
      },
      "input_files": ["docs/FLEET.md", "docs/WORKFLOW_AUTHORING.md"],
      "expected_artifacts": ["log", "report"],
      "scorer": {
        "kind": "manual"
      },
      "retry_policy": {
        "max_attempts": 1
      }
    },
    {
      "id": "draft-gap-note",
      "name": "Draft gap note",
      "objective": "Draft a short local note for any missing tutorial steps.",
      "instructions": "Write a concise Markdown note with the missing Fleet + Workflow tutorial steps. Do not edit public docs unless explicitly asked.",
      "worker": {
        "role": "builder",
        "tools": ["rg", "sed"]
      },
      "workspace": {
        "required_files": ["docs/FLEET.md"],
        "writable_paths": [".codewhale/fleet"],
        "environment": {
          "allowlist": []
        }
      },
      "expected_artifacts": ["log", "report"],
      "scorer": {
        "kind": "manual"
      }
    }
  ]
}
```

Save it as `tasks.json`.

Common task fields:

| Field | Purpose |
| --- | --- |
| `id`, `name` | Stable task identity and display name. |
| `objective`, `instructions` | The worker goal and exact operating instructions. |
| `worker.role` | Built-in or custom role intent, such as `reviewer`, `builder`, `read-only`, or `smoke-runner`. |
| `worker.profile` / `worker.agent_profile` | Saved Fleet roster profile resolved from project `.codewhale/agents/`, personal `$CODEWHALE_HOME/agents/`, or `[fleet.profiles]`. |
| `worker.tools` | Tool names the task expects the worker to use. |
| `worker.model` | Preferred explicit model pin. Route resolution still owns provider/model validation. |
| `worker.model_class`, `worker.loadout` | Compatibility routing hints for older task specs; prefer `worker.profile` plus saved profile route pins for new specs. |
| `workspace.required_files` | Files that must exist before the task starts. |
| `workspace.writable_paths` | Paths the task is allowed to write when the effective runtime posture allows writing. |
| `workspace.environment` | Required or allowlisted environment variables, by name only. |
| `input_files`, `context` | Extra files and strings to thread into the task prompt. |
| `expected_artifacts` | Artifact kinds to expect: `log`, `report`, `patch`, `test_result`, `checkpoint`, or `receipt`. |
| `scorer` | Deterministic or manual verification rule. |
| `retry_policy`, `timeout_seconds`, `budget` | Retry and budget controls. |

Security policy fields:

| Field | Purpose |
| --- | --- |
| `default_trust_level` | Default worker trust level. `sandbox` is the conservative default. |
| `max_trust_level` | Ceiling for any worker in the run. |
| `allowed_secrets` | Secret names workers may resolve; never put secret values here. |
| `capability_grants` | Scoped grants such as `network`, `git-push`, `provider-secrets`, `release`, or `workspace-write`. |
| `require_identity_verification` | Requires remote workers to pass host identity checks before elevated trust. |
| `allow_parallel_reads` | Allows conservative batching of independent read-only operations. |

## 3. Start And Monitor Fleet

Launch the run:

```sh
codewhale fleet run tasks.json --max-workers 4
```

The command prints the run id and worker ids. In another terminal, monitor the
ledgered state:

```sh
codewhale fleet status
codewhale fleet inspect <worker-id>
codewhale fleet logs <worker-id>
codewhale fleet artifacts <worker-id>
```

Use typed controls when a worker needs intervention:

```sh
codewhale fleet interrupt <worker-id>
codewhale fleet restart <worker-id>
codewhale fleet resume <run-id>
codewhale fleet stop --all
```

`resume` is for restart recovery after a manager exit, laptop sleep, or stale
lease. It replays the ledger and reconciles stale work without creating a new
run.

## 4. Author A Workflow

Workflow source is declarative JavaScript or TypeScript that lowers to typed
Rust `WorkflowSpec`. It is not a general JavaScript runtime: imports, process
access, filesystem reads/writes, network calls, `eval`, `async`, and `await`
are rejected.

Create a checked-in file such as `workflows/docs_readiness.workflow.js`. The
repo also includes `workflows/issue_audit.workflow.js` as a maintained example.

```js
export default workflow({
  "id": "docs-readiness",
  "goal": "Inspect Fleet and Workflow docs, then synthesize a readiness note",
  "nodes": [
    {
      "branch": {
        "id": "parallel-docs-audit",
        "parallel": true,
        "children": [
          {
            "agent": {
              "id": "fleet-docs",
              "prompt": "Inspect docs/FLEET.md for command and task-spec coverage.",
              "agent_type": "review",
              "mode": "read_only",
              "profile": "reviewer",
              "file_scope": ["docs/FLEET.md"]
            }
          },
          {
            "agent": {
              "id": "workflow-docs",
              "prompt": "Inspect docs/WORKFLOW_AUTHORING.md for Workflow authoring coverage.",
              "agent_type": "review",
              "mode": "read_only",
              "profile": "reviewer",
              "file_scope": ["docs/WORKFLOW_AUTHORING.md"]
            }
          }
        ]
      }
    },
    {
      "reduce": {
        "id": "readiness-summary",
        "inputs": ["fleet-docs", "workflow-docs"],
        "prompt": "Summarize the exact docs gaps and the safest next edit."
      }
    }
  ]
});
```

Current Workflow node wrappers are `agent`, `branch`, `sequence`, `reduce`,
`teacher_review`, `loop_until`, `cond`, and `expand`. `agent.profile` names a
Fleet roster profile; explicit agent fields override profile defaults.

The model-facing `workflow` tool can start, run, inspect, or cancel a workflow
from inline source or a `source_path`. When Codewhale uses this path, ask it to
show the plan first if the workflow will launch multiple workers or touch files.

## 5. Natural Language Intake

A good prompt today is:

```text
Draft a Fleet task spec for this goal, but do not run it yet.
Show the proposed tasks, worker profiles, writable paths, expected artifacts,
scorers, and security policy. Keep secrets disabled unless I explicitly grant
them.
```

After reviewing the generated spec, save it as `tasks.json` and run the Fleet
commands above. For workflows, ask Codewhale to draft a `.workflow.js` file,
show the plan, and use the workflow tool path only after approval.

This review step is intentional. It keeps provider routing, DeepSeek or other
model support, writable paths, network access, and secret use explicit before
durable workers start.
