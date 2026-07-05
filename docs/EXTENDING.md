# Extend CodeWhale

CodeWhale has several extension points, each with a distinct job. This page is
the decision guide: what each layer is, when to reach for it, and when not to.
Every row links to the full reference document.

One rule frames all of them: **guidance layers shape model behavior; they never
override the runtime gates.** Approval, sandbox, network, and trust controls
are enforced in code (see [SANDBOX.md](SANDBOX.md), [MODES.md](MODES.md)).

## The decision table

| Extension point | What it is | Lives at | Use when | Don't use when |
|:---|:---|:---|:---|:---|
| **AGENTS.md** | Project working instructions the agent reads first: build/test commands, conventions, where to work. `codewhale init` creates one. | `<repo>/AGENTS.md` (with `CLAUDE.md` compat fallbacks) | You want every session in this repo to know its commands and conventions. | You need rules *ranked above* project instructions when they conflict — that is constitution territory. |
| **User constitution** | Your standing law across all projects: structured data, drafted with the model's help at `/setup`, ratified by you, rendered into a prose block. | `~/.codewhale/constitution.json`, managed via `/constitution` | Preferences and standing rules that should follow *you* to every repo. | Repo-specific policy — put that in the repo constitution so collaborators get it too. |
| **Repo constitution** | Committed repo authority policy; the invariants mechanism is mechanically enforced (repo-law) and can only tighten, never loosen. | `<repo>/.codewhale/constitution.json` | Protected invariants, branch policy, verification requirements, escalation rules for one repository. | You want to *grant* the agent more power — repo law can only restrict. |
| **Skills** | Named, reusable capability bundles (`SKILL.md`) invoked with `$<skill-id>` or `/skill`. | `~/.codewhale/skills/<id>/`, `<repo>/.codewhale/skills/<id>/` | A repeatable procedure with its own instructions/scripts (e.g. a review checklist). | The behavior should apply to *every* turn implicitly — that is AGENTS.md or constitution. |
| **MCP (consume)** | Attach external tool servers over stdio or HTTP/SSE (`/mcp add …`). | `~/.codewhale/mcp.json` | The agent needs tools that live outside the repo: databases, browsers, SaaS APIs. | A small script would do — a custom tool or skill is lighter than a server. |
| **MCP (expose)** | Serve CodeWhale itself as an MCP server: `codewhale mcp-server`. | CLI | Another agent/app should call CodeWhale as a tool. | You need full session control — use the Runtime API instead. |
| **Hooks** | Config-driven lifecycle hooks around tool calls that can allow, deny, or ask. Inspect with `/hooks`. | `[hooks]` in `~/.codewhale/config.toml`; per-repo `<repo>/.codewhale/hooks.toml` (trusted workspaces) | Mechanical policy on tool execution: block a command family, require confirmation, log. | You're encoding style guidance — hooks gate execution, they don't teach. (The broader lifecycle redesign is RFC-only: [rfcs/1364-hooks-lifecycle.md](rfcs/1364-hooks-lifecycle.md).) |
| **Sub-agents** | The `agent` tool launches focused child roles (`explore`, `plan`, `review`, `implementer`, `verifier`, `general`) with their own transcript handles. | invoked by the model; status via `/subagents` | Parallel or context-heavy legwork inside one session. | Work must survive restarts or needs an audit trail — use Fleet. |
| **Fleet profiles** | Reusable agent "party member" profiles (role, model class, ranked model preferences) authored with `/fleet setup`. | `<repo>/.codewhale/agents/*.toml` | You run recurring multi-worker jobs and want consistent roles/models per slot. | One-off fanout — plain sub-agents need zero setup. |
| **WhaleFlow** | Long-running workflow orchestration overlay (authored in JS or Starlark) that plans big tasks into resumable workflows on top of any mode. | authored per [WHALEFLOW_AUTHORING.md](WHALEFLOW_AUTHORING.md) | Repeatable multi-step workflows with progress you want to watch and resume. | A single conversation with approvals is enough. |
| **Runtime API** | The local HTTP/SSE + stdio control plane (`codewhale app-server`, `127.0.0.1:7878`): threads, turns, events, approvals, snapshots, usage. | CLI + [RUNTIME_API.md](RUNTIME_API.md) | Building a UI, bridge, or automation that drives CodeWhale programmatically. | You just want tool access from another agent — MCP-expose is simpler. |
| **ACP** | Agent Client Protocol server for editor clients: `codewhale serve --acp` (stdio). | CLI | Your editor speaks ACP and should host CodeWhale in-editor. | You're scripting headless runs — `codewhale exec` is the direct path. |
| **Bridges** | Chat-platform relays shipped as npm packages: Telegram, Feishu, WeCom, Weixin (`integrations/`), deployable via `codewhale remote-setup`. | `integrations/*-bridge` | You want to steer sessions from a chat app on your phone. | You need full programmatic control — talk to the Runtime API directly. |

## How they compose

A typical mature setup layers several of these, from most static to most live:

1. **Bundled constitution** (in the binary) fixes the authority order.
2. **User constitution** carries your standing rules everywhere.
3. **Repo constitution** adds this repo's protected invariants (enforced).
4. **AGENTS.md** teaches the repo's commands and conventions.
5. **Skills / MCP / hooks** add capabilities and mechanical gates.
6. **Sub-agents, Fleet, WhaleFlow** scale execution when tasks outgrow one
   transcript.
7. **Runtime API / ACP / MCP-expose / bridges** connect CodeWhale to other
   programs and surfaces.

Conflicts between the guidance layers resolve by constitutional rank, not
prompt order — see [CONSTITUTION.md](CONSTITUTION.md).

## Scope cheat sheet

- **Per-user, all repos:** `~/.codewhale/` — user constitution, global skills,
  `mcp.json`, `[hooks]` in config.toml.
- **Per-repo, committed:** `<repo>/.codewhale/` — repo constitution,
  `config.toml` overlay, skills, `hooks.toml`, `agents/` profiles; plus
  `AGENTS.md` at the repo root.
- Full path reference: [DIRECTORY_STRUCTURE.md](DIRECTORY_STRUCTURE.md).
