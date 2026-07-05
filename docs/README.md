# CodeWhale docs

Index of everything under `docs/`. Start with the [User Guide](GUIDE.md) if
you're new; the [root README](../README.md) is the short version of the whole
project.

## User guides

Installing, configuring, and running CodeWhale day to day.

- [GUIDE.md](GUIDE.md) — the user guide: first session to advanced workflows.
- [INSTALL.md](INSTALL.md) — every install path, checksums, China mirrors,
  Windows specifics, troubleshooting.
- [FAQ.md](FAQ.md) — frequently asked questions.
- [CONFIGURATION.md](CONFIGURATION.md) — the TOML config, env precedence,
  profiles, and every runtime knob.
- [CONSTITUTION.md](CONSTITUTION.md) — the nested constitution: how
  instructions are ranked, `/constitution` setup, and repo-local law.
- [PROVIDERS.md](PROVIDERS.md) — the provider registry: credentials, base
  URLs, routing, capability boundaries.
- [MODES.md](MODES.md) — Plan, Agent, and YOLO, and the approval model.
- [SANDBOX.md](SANDBOX.md) — sandbox threat model and OS-level isolation.
- [FLEET.md](FLEET.md) — the durable multi-worker control plane.
- [SUBAGENTS.md](SUBAGENTS.md) — in-session sub-agents: roles, lifecycle,
  delegation briefs.
- [MCP.md](MCP.md) — consuming MCP servers and exposing CodeWhale as one.
- [MEMORY.md](MEMORY.md) — user memory.
- [RESTORE.md](RESTORE.md) — rollback and checkpoints: side-git snapshots,
  `/restore`, `/undo`, and how they interact with your real Git history.
- [DIRECTORY_STRUCTURE.md](DIRECTORY_STRUCTURE.md) — the `~/.codewhale/` and
  repo-local `.codewhale/` layout reference.
- [workflows/](workflows/) — copy-pasteable recipes: first real task, fix
  failing tests, review a PR, Fleet run, local models, and more.
- [ADDING_CONTEXT.md](ADDING_CONTEXT.md) — every input surface, what the model
  sees (prompt assembly order), and context budget/compaction.
- [SESSIONS.md](SESSIONS.md) — session save/resume, what survives restarts,
  and Fleet vs. WhaleFlow vs. goal loop vs. sub-agents.
- [COST.md](COST.md) — the five cost states, why CodeWhale never invents a
  price, and route honesty.
- [KEYBINDINGS.md](KEYBINDINGS.md) — keys and how to rebind them.
- [ACCESSIBILITY.md](ACCESSIBILITY.md) — screen readers, contrast, motion.
- [DOCKER.md](DOCKER.md) — container usage; see also
  [examples/](examples/) for toolbox Dockerfile/compose samples.
- [CLASSROOM_INSTALL.md](CLASSROOM_INSTALL.md) — lab/classroom rollout
  checklist.
- [HarmonyOS.md](HarmonyOS.md) — HarmonyOS/OpenHarmony notes.
- [CNB_MIRROR.md](CNB_MIRROR.md) — the CNB mirror for users who can't reach
  GitHub reliably.
- [LSP_PHP_CUSTOM.md](LSP_PHP_CUSTOM.md) — PHP LSP support and custom language
  servers ([中文](LSP_PHP_CUSTOM.zh-CN.md)).
- [REBRAND.md](REBRAND.md) — migrating from the legacy `deepseek-tui` name.
- [LEGACY_PATHS.md](LEGACY_PATHS.md) — `.deepseek/` compatibility path audit.
- [LOCALIZATION.md](LOCALIZATION.md) — the localization matrix.

## Extending

Building on top of CodeWhale.

- [EXTENDING.md](EXTENDING.md) — the decision guide: when to use AGENTS.md,
  constitutions, skills, MCP, hooks, sub-agents, Fleet, WhaleFlow, the
  Runtime API, ACP, or bridges.
- [WHALEFLOW_AUTHORING.md](WHALEFLOW_AUTHORING.md) — authoring validated
  multi-agent workflows (JS/TS/Starlark/JSON).
- [RUNTIME_API.md](RUNTIME_API.md) — HTTP/SSE and ACP runtime APIs and the
  integration contract.
- [CLAUDE_PLUGIN_COMPAT.md](CLAUDE_PLUGIN_COMPAT.md) — using Claude Code
  skill folders as CodeWhale skills.
- [SKILL_INVOCATION_DESIGN.md](SKILL_INVOCATION_DESIGN.md) — the
  `$<skill-name>` inline syntax design.

## Contributor

Understanding and changing the codebase. Start with
[CONTRIBUTING.md](../CONTRIBUTING.md).

- [ARCHITECTURE.md](ARCHITECTURE.md) — crate layout, runtime flow, tool
  system, extension points, security model.
- [AGENT_RUNTIME.md](AGENT_RUNTIME.md) — the durable agent runtime substrate.
- [TOOL_SURFACE.md](TOOL_SURFACE.md) — the model-facing tool surface.
- [TOOL_LIFECYCLE.md](TOOL_LIFECYCLE.md) — tool-surface lifecycle policy.
- [PROMPT_MODE_MATRIX.md](PROMPT_MODE_MATRIX.md) — which prompt segments ship
  in which mode.
- [WORKROOM_ARCHITECTURE.md](WORKROOM_ARCHITECTURE.md) /
  [WORKROOM_SECURITY.md](WORKROOM_SECURITY.md) — workrooms.
- [RECEIPTS.md](RECEIPTS.md) — runtime receipts.
- [MODEL_LAB.md](MODEL_LAB.md) — model lab roadmap.
- [RECURSIVE_SELF_IMPROVEMENT.md](RECURSIVE_SELF_IMPROVEMENT.md) — the
  agent-assisted one-patch improvement loop.
- [CONTRIBUTORS.md](CONTRIBUTORS.md) — the full per-PR contributor record.
- [CREDIT.md](CREDIT.md) — how credit works here: harvest credit, AUTHOR_MAP,
  and the co-author CI gate.
- [architecture/](architecture/) — focused architecture notes.
- [rfcs/](rfcs/) — design RFCs, accepted and in-flight.

## Maintainer

Release engineering, triage, and stewardship records. These are working
documents for maintainers and release agents — useful history, but not user
documentation.

- [RELEASE_CHECKLIST.md](RELEASE_CHECKLIST.md) /
  [RELEASE_RUNBOOK.md](RELEASE_RUNBOOK.md) — cutting a release.
- [OPERATIONS_RUNBOOK.md](OPERATIONS_RUNBOOK.md) — operating the project.
- [ISSUE_TRIAGE.md](ISSUE_TRIAGE.md) — triage conventions.
- [AGENT_ETHOS.md](AGENT_ETHOS.md) — the stewardship posture for maintainer
  automation (referenced from CONTRIBUTING).
- [ACP_REGISTRY_SUBMISSION.md](ACP_REGISTRY_SUBMISSION.md) — ACP registry
  submission prep.
- Release ledgers and agent prompts (historical, per-lane):
  [V0866_RELEASE_LEDGER.md](V0866_RELEASE_LEDGER.md),
  [V0865_RELEASE_LEDGER.md](V0865_RELEASE_LEDGER.md),
  [V0865_REMAINING_AGENT_PROMPT.md](V0865_REMAINING_AGENT_PROMPT.md),
  [V0_8_61_EXECUTION.md](V0_8_61_EXECUTION.md).
- [CHANGELOG_ARCHIVE.md](CHANGELOG_ARCHIVE.md) — pre-0.8.5 changelog history.
- [evidence/](evidence/) — release QA matrices and verification evidence.
- [skills/](skills/) — maintainer workflow skills (GitHub triage, credit
  harvest, release QA sweeps).
