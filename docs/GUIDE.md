# Codewhale User Guide

This guide is for your first hour with Codewhale. It explains the main
workflow, the important safety controls, and where to go next when you need a
complete reference.

Codewhale has deeper reference documents for installation, configuration,
providers, modes, keybindings, tools, and operations. Use this page as a guided
walkthrough, then follow the "Next" links when you need every option.

## 1. Welcome to Codewhale

Codewhale is a terminal coding agent. You run it from a workspace, give it a
task, and it can use structured tools to inspect files, run commands, edit
code, and report back with evidence.

The important difference from a normal chat model is that Codewhale is built
around a harness:

- It keeps the active workspace and session visible.
- It routes each turn through explicit modes and approval rules.
- It shows tool calls in the transcript instead of hiding the work.
- It can preserve sessions, fork conversations, and continue later.
- It can run sub-agents for focused background work.

You can use Codewhale for small questions:

```text
Explain the authentication flow in this repository.
```

You can also use it for multi-step work:

```text
Find the failing validation path, propose a fix, and wait for my approval
before editing files.
```

For a new repository, start conservatively. Ask Codewhale to explore and plan
before asking it to change files. That gives you a reviewable path and makes it
easier to catch wrong assumptions early.

Next: [ARCHITECTURE.md](ARCHITECTURE.md) explains the internal harness and
runtime model.

## 2. First Launch

Install Codewhale with the path that fits your machine. Each supported install
path provides both the `codewhale` dispatcher and the `codewhale-tui` runtime.

```bash
# npm
npm install -g codewhale

# Cargo
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked

# Homebrew, legacy installs only
# The tap/formula still uses the old deepseek-tui name. Prefer npm, Cargo,
# Docker, or direct downloads for new installs until the formula is renamed.
brew tap Hmbown/deepseek-tui
brew install deepseek-tui
```

Docker is also available when you want an isolated runtime:

```bash
docker volume create codewhale-home
docker run --rm -it \
  -e DEEPSEEK_API_KEY="$DEEPSEEK_API_KEY" \
  -v codewhale-home:/home/codewhale/.codewhale \
  -v "$PWD:/workspace" \
  -w /workspace \
  ghcr.io/hmbown/codewhale:latest
```

Launch Codewhale from the repository or directory you want it to work in:

```bash
codewhale
```

On first launch, Codewhale starts with a short constitution-first setup path:
choose language, review provider/model readiness, review runtime posture, then
create or confirm your Codewhale constitution. The bundled/default
constitution is valid, and you can revisit the setup hub later with `/setup`.

DeepSeek is the default provider. If you want to configure its key before or
after the first launch, the most direct setup path is:

```bash
codewhale auth set --provider deepseek
```

You can also provide a key through the environment:

```bash
export DEEPSEEK_API_KEY="your-key"
codewhale
```

New Codewhale config is stored under `~/.codewhale/config.toml`. Legacy
`~/.deepseek/config.toml` files are still supported for users migrating from
the old name.

Use `/constitution` to review or change standing guidance. After setup, run a
doctor check:

```bash
codewhale doctor
```

Use the JSON form when you need a machine-readable report for an issue:

```bash
codewhale doctor --json
```

If the doctor command reports that a rejected key came from the environment,
remove or replace that environment variable before testing saved config again.

Next: [INSTALL.md](INSTALL.md) covers platform-specific install paths,
[CONFIGURATION.md](CONFIGURATION.md) covers config resolution, and
[PROVIDERS.md](PROVIDERS.md) covers provider IDs and credentials.

## 3. Your First Task

Start with a read-only task in a real workspace:

```text
Map the repository structure and tell me where the CLI entrypoint lives.
```

Then ask for a focused plan:

```text
I want to add a small validation for empty config values. Inspect the relevant
code and propose the smallest safe change before editing anything.
```

When you are ready for edits, be specific about the acceptance criteria:

```text
Implement the validation you proposed. Keep the change scoped to config
parsing, add or update the narrowest test, and run the relevant check.
```

Good first prompts include four details:

- The outcome you want.
- The files, feature, or behavior you care about.
- What is out of scope.
- What verification should count as done.

For example:

```text
Fix the broken provider error message in the config loader. Do not change the
provider registry. Add a regression test and run only the config crate tests.
```

If you are not sure where the bug is, say that:

```text
Investigate why `codewhale doctor` reports the wrong provider. Do not edit
files yet. Return the likely cause, evidence, and a proposed patch plan.
```

Codewhale works best when you let investigation and implementation happen in
separate steps for unfamiliar code. For small, well-understood changes, a
single implementation request is fine.

Next: [MODES.md](MODES.md) explains when to use Plan, Act, and Operate.

## 4. Understanding the Interface

The interactive TUI has a few stable regions:

- Header: current session, active model, mode, and high-level status.
- Transcript: the conversation, tool calls, command output summaries, and
  model responses.
- Composer: where you type prompts, slash commands, and file mentions.
- Sidebar: contextual panels for work state, tasks, agents, or related
  session information.
- Status and footer areas: live activity, queued follow-ups, and short command
  hints.

The footer status line is configurable. Run `/statusline` to choose which
footer chips are visible, or set `[tui].status_items` in `config.toml` to
control both selection and order. Supported keys currently include `mode`,
`model`, `cost`, `balance` (DeepSeek / DeepSeekCN only), `status`, `agents`,
`reasoning_replay`, `prefix_stability`, `cache`, `context_percent`,
`git_branch`, `last_tool_elapsed` (reserved), `rate_limit` (reserved),
and `tokens`. Omit `status_items` to keep the built-in default order; set it to
`[]` to hide configurable chips.

The transcript is the audit trail. When Codewhale reads files, runs commands,
or edits code, the action appears there. If a command fails, use the visible
failure output as part of your next instruction instead of starting over.

The composer accepts normal prompts and slash commands. Type `/` to discover
available commands. Use file mentions when you want the model to focus on a
specific file or directory instead of searching broadly.

The sidebar is useful when a turn spans multiple steps. It can keep goals,
agent state, and contextual information visible while the transcript continues
to grow.

Keyboard shortcuts vary by context, terminal, and platform. This guide avoids
duplicating the full shortcut catalog so it does not drift from the TUI.

Next: [KEYBINDINGS.md](KEYBINDINGS.md) is the complete shortcut reference.

## 5. Modes

Codewhale has three visible TUI modes:

| Mode | Use it for | Default posture |
| --- | --- | --- |
| Plan | Exploration, design, and review before changes | Read-only investigation |
| Act | Normal multi-step coding work | Tool use with approval gates |
| Operate | Direct work plus parallel or background coordination | Tools follow the active posture; delegate when useful |

Switch modes from the TUI with the mode picker:

```text
/mode
```

Or switch directly:

```text
/mode plan
/mode act
/mode operate
```

Plan mode is the safest place to start in an unfamiliar repository. It is for
inspection and decision-making, not file edits.
For non-trivial work, Plan mode's confirmation prompt can show a grounded
PlanArtifact: objective, context, sources used, critical files, constraints,
approach, verification plan, risks, and handoff notes. Empty sections are
visible when the agent uses the rich artifact shape, so you can ask for a
revision instead of accepting an under-specified plan.

Act mode is the default for most contribution work. It lets Codewhale read,
run checks, and edit files while keeping risky actions behind approval gates.

Operate keeps that direct tool surface and its approval, sandbox, shell,
ask-rule, and repository protections. Its difference is orchestration emphasis:
Codewhale prefers Fleet workers for independent, parallel, background, or
long-running work, while small or tightly coupled work can remain in the parent.

For trusted workspaces where you intentionally want actions to proceed without
approval prompts, select the Full Access permission posture with `Shift+Tab`.
Do not use Full Access in a repository you do not trust.

Modes are separate from model routing. `Tab` cycles visible modes when the
composer is idle, while `/model auto` controls model and thinking selection for
turns.

You can also change approval behavior from `/config` by editing the approval
mode. Use this only when you understand how it changes tool execution.

Next: [MODES.md](MODES.md) has the full mode, approval, and trust-mode
reference.

## 6. Slash Commands

Slash commands are typed into the composer. They are useful when you want to
change Codewhale state directly instead of asking the model in natural
language.

Common commands for first-time users:

| Command | Use |
| --- | --- |
| `/mode` | Open the mode picker or switch with `/mode agent` |
| `/model` | Select a model or use `/model auto` |
| `/provider` | Pick the active API provider |
| `/fleet` | Configure Fleet roles or open worker status |
| `/config` | Edit runtime and provider settings |
| `/statusline` | Choose which footer status chips are visible |
| `/compact` | Summarize long context to recover token budget |
| `/review` | Ask for a structured review workflow |
| `/memory` | Inspect or manage memory when enabled |
| `/mcp` | Configure or inspect MCP server integration |

Toolbox commands stay searchable when you type them directly: `/models`
fetches live endpoint IDs, `/modeldb` opens the bundled model reference, and
`/rlm` opens a manual persistent RLM context.

Use `/provider` when you want to switch away from the default DeepSeek route.
Provider IDs, environment variables, model defaults, and capability notes are
kept in the provider registry document.

Soft-auto multi-agent work: [AUTOMATIC_WORKFLOWS.md](AUTOMATIC_WORKFLOWS.md).

Next for durable multi-worker work: [FLEET_WORKFLOW_TUTORIAL.md](FLEET_WORKFLOW_TUTORIAL.md)
walks through Fleet task specs, monitoring, and Workflow authoring.

Use `/model auto` when you want Codewhale to choose the model and thinking
level per turn. Use a fixed model when you need repeatable comparisons or a
strict cost profile.

Use `/compact` when a session gets long and the model starts carrying too much
history. Compaction trades raw transcript detail for a concise working summary.

This guide intentionally does not list every command. The command surface
changes more often than the onboarding flow, and the TUI command palette is the
source of truth while you are inside a session.

Next: [CONFIGURATION.md](CONFIGURATION.md) covers runtime settings and
[MCP.md](MCP.md) covers Model Context Protocol integration.

## 7. Working with Tools

Codewhale tools are structured actions. Instead of only producing prose, the
model can call tools to inspect and change the workspace.

Examples of tool-backed work include:

- Reading a file before explaining it.
- Searching for call sites before proposing a refactor.
- Running a focused test command.
- Applying a small patch.
- Opening a sub-agent for parallel investigation.

Tool use is governed by mode, approvals, and sandbox policy. The exact behavior
depends on the current mode and config, but the basic rule is simple: start in
Plan for read-only exploration, use Act for normal changes, and reserve Full
Access for trusted automation.

The workspace boundary matters. Codewhale is expected to work in the directory
you launched it from or the workspace you configured. Be explicit when a task
should stay inside a repo:

```text
Only inspect and edit files under this repository. Do not touch parent
directories or global config.
```

When a command needs network, writes outside the workspace, or a risky shell
operation, expect an approval prompt unless you have configured more permissive
behavior.

Good tool instructions are concrete:

```text
Run the narrowest test that covers this parser change. If it fails, report the
failure and stop before broadening the test scope.
```

Avoid asking for broad cleanup during a focused fix. Smaller tool scopes make
the transcript easier to review and the final diff easier to merge.

Next: [TOOL_SURFACE.md](TOOL_SURFACE.md) lists the tool surface and
[SANDBOX.md](SANDBOX.md) explains sandbox behavior.

## 8. Sub-agents and Parallel Work

Sub-agents are background child agents. The parent session gives a child a
focused task, receives an agent id, and can continue working while the child
runs.

The main orchestration tool is:

- `agent`: start a focused child with a task and role. The child runs in the
  background and returns a compact receipt plus transcript handle.

You normally do not need to call these tools directly. Ask for parallel work in
plain language:

```text
Open one read-only explorer for the config crate and another for the TUI
provider picker. Have both return file references and risks before we plan the
fix.
```

Useful roles include:

| Role | Good for |
| --- | --- |
| `general` | Multi-step tasks; the default when no role is specified |
| `explore` | Read-only code mapping |
| `plan` | Design and migration planning |
| `review` | Bug-focused review of an existing change |
| `implementer` | A tightly specified edit |
| `verifier` | Running checks and reporting pass/fail evidence |

Sub-agents are most useful when work can be separated cleanly. Do not use them
for tiny edits, and do not ask multiple agents to write the same files at the
same time.

Next: [SUBAGENTS.md](SUBAGENTS.md) covers roles, lifecycle, concurrency, and
output contracts.

## 9. Skills

Skills are reusable instruction packs. A skill is usually a `SKILL.md` file
that teaches Codewhale how to perform a recurring workflow, use a tool family,
or follow a project convention.

Use skills when a task has a repeatable process:

- Reviewing a specific kind of PR.
- Working with a document or spreadsheet format.
- Following a team release checklist.
- Using a project-specific memory or wiki workflow.

Inside the TUI, `/skill` activates a skill when one is available, and `/skills`
lists installed skills. The command palette can also surface skill entries
alongside normal slash commands.

Good skills are narrow. They should tell the model what workflow to follow,
what evidence to collect, and what to avoid. They should not hide credentials
or replace normal repository documentation.

If a repository has its own instructions, treat them as part of the active
work. Read the local guidance before editing, and keep any contribution within
the repository's conventions.

Next: see [CLAUDE_PLUGIN_COMPAT.md](CLAUDE_PLUGIN_COMPAT.md) for
Claude Code skill/plugin compatibility, and [CONFIGURATION.md](CONFIGURATION.md)
for config paths and project authority.

## 10. Getting Help

Start with doctor output:

```bash
codewhale doctor
```

Use JSON when filing a detailed issue:

```bash
codewhale doctor --json
```

For authentication problems, check which source is winning: saved config,
keyring, environment, or an explicit launch flag. A stale `DEEPSEEK_API_KEY`
environment variable can override what you expected to use.

For provider problems, confirm the active provider and model:

```text
/provider
/model
```

For long or confusing sessions, use `/compact` to reduce context pressure, or
start a fresh session in the same workspace and summarize what you need.

When reporting an issue, include:

- Codewhale version.
- Install method.
- Operating system and terminal.
- Provider and model.
- The exact command or prompt.
- Relevant doctor output.
- Whether the problem happens in a fresh workspace.

Do not paste API keys, private source code, or secrets into a public issue.

Next: [OPERATIONS_RUNBOOK.md](OPERATIONS_RUNBOOK.md) has operational triage and
recovery steps.

## FAQ

### Is Codewhale only for DeepSeek?

DeepSeek is the default and first-class route, but Codewhale also supports
other hosted and local OpenAI-compatible providers. Use `/provider` or
`codewhale --provider <id>` to choose a provider. Keep the provider registry
open when configuring a non-default route.

### Which mode should I use first?

Use Plan for unfamiliar code, Act for normal implementation, and Full Access
only for trusted repositories where automatic execution is acceptable.

### Why does Codewhale ask before running commands?

Approvals are part of the safety model. Shell commands, paid tools, writes, and
actions outside the expected workspace can have side effects. Approval prompts
let you keep control while still letting the model do useful work.

### How do I run a Python file on macOS?

Open Terminal in the folder that contains the file and run:

```bash
python3 your_file.py
```

If macOS says `python3` is missing, install Python from
[python.org](https://www.python.org/downloads/macos/) or with Homebrew:

```bash
brew install python
```

Inside Codewhale, ask the agent to inspect the file and run it with
`python3 your_file.py`. If the script needs packages, install them in a virtual
environment first:

```bash
python3 -m venv .venv
source .venv/bin/activate
python3 -m pip install -r requirements.txt
python3 your_file.py
```

### Where is my config stored?

New Codewhale config uses `~/.codewhale/config.toml`. Legacy
`~/.deepseek/config.toml` remains supported for compatibility. Project overlays
can also affect behavior when a workspace config exists.

### How do I keep costs predictable?

Use `/model auto` for routing, choose a fixed model when you need a strict
profile, and compact long sessions. For larger tasks, ask Codewhale to plan
before implementing so you do not spend tokens on the wrong path.

### How do I continue previous work?

Codewhale saves sessions. Use the session picker or resume/continue CLI paths
documented in the README and modes guide. For a risky experiment, fork the
session before changing direction.

The `/sessions` picker starts scoped to the current workspace so resumes stay
attached to the project you opened. Press `a` in the picker to show sessions
from every workspace, or run `codewhale sessions` to list all saved sessions
with last-updated timestamps before resuming a specific id.

### What should I do when the model gets confused?

Stop and restate the goal, constraints, and current evidence. If the transcript
is long, use `/compact` or start a fresh session with a short handoff. If the
problem is operational, run `codewhale doctor` and inspect the reported config
and provider state.

### Should I put project rules in prompts or files?

Use repository files for durable project rules and prompts for turn-specific
intent. If a workflow repeats across projects, consider turning it into a
skill.

### Can Codewhale edit files outside the current repository?

That depends on workspace boundaries, sandbox settings, trust mode, and
approval policy. For contribution work, keep instructions scoped to the current
repository unless you intentionally need something else.

### Where should I go after this guide?

Read the focused reference for the thing you are changing. For most users, the
next pages are install, configuration, providers, modes, keybindings, tools,
and sub-agents.

Next: [INSTALL.md](INSTALL.md), [CONFIGURATION.md](CONFIGURATION.md),
[PROVIDERS.md](PROVIDERS.md), [MODES.md](MODES.md), and
[TOOL_SURFACE.md](TOOL_SURFACE.md).
