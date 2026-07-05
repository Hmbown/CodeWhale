# Directory Structure

Where CodeWhale keeps things on disk: the per-user home directory
(`~/.codewhale`), the repo-local `.codewhale/` directory, and how legacy
`~/.deepseek` paths migrate. All paths below are verified against the current
code.

## Resolution rules

- **`$CODEWHALE_HOME`** overrides the user home directory. When it is set
  explicitly, it is a hard isolation boundary: legacy `~/.deepseek` fallbacks
  are disabled entirely.
- Otherwise the home is **`~/.codewhale`** (`$HOME`, or `%USERPROFILE%` on
  Windows).
- State is **read** from `~/.codewhale/<subdir>` with a fallback to
  `~/.deepseek/<subdir>` only when the legacy directory exists, and always
  **written** to `~/.codewhale/`. First write to a state subdir relocates the
  legacy copy and prints a notice.
- The config file can also be pointed at directly: `--config <path>` or
  `CODEWHALE_CONFIG_PATH` (legacy alias `DEEPSEEK_CONFIG_PATH`).

## User home: `~/.codewhale/`

```text
~/.codewhale/
├── config.toml            # main config (providers, hooks, tools, …)
├── settings.toml          # saved UI settings
├── tui.toml               # TUI preferences
├── constitution.json      # your user constitution (edited via /constitution, /setup)
├── setup_state.json       # guided-setup progress sidecar
├── AGENTS.md              # global fallback project instructions (optional)
├── instructions.md        # lower-ranked global instructions (optional)
├── prompts/
│   └── constitution.md    # expert override of the bundled base prompt (optional)
├── mcp.json               # MCP server registry (also `codewhale mcp register`)
├── skills/                # installed skills, one directory per skill id
├── skills_state.toml      # per-skill enable/disable state
├── tools/                 # custom tool scripts (auto-discovered)
├── plugins/               # plugins + overrides.json
├── sessions/              # persisted sessions (/sessions, /resume, /fork)
├── snapshots/             # side git repos for /restore, keyed by project/worktree hash
├── tasks/                 # durable background tasks
├── automations/           # saved automations
├── logs/                  # tui-YYYY-MM-DD-<pid>.log
├── audit.log              # append-only audit events
├── cache/skills/          # skill install cache
├── catalog/               # model catalog cache (e.g. openrouter.json)
├── secrets/secrets.json   # file-backend secrets (0600; used when no OS keyring)
├── tool_outputs/          # spillover for oversized tool results
├── composer_history.txt   # composer input history
├── composer_stash.jsonl   # stashed drafts
├── workspace-trust.json   # per-workspace trust decisions
├── memory.md              # memory file
├── notes.txt              # /note storage
└── .onboarded             # first-run marker
```

Notes:

- There is **no user-level `agents/` directory** — Fleet agent profiles are
  workspace-scoped only (see below).
- Managed/enforced config on Unix still reads from `/etc/deepseek/`
  (`managed_config.toml`, `requirements.toml`); on other platforms it falls
  back to `~/.codewhale/`.

## Repo-local: `<workspace>/.codewhale/`

Everything here is per-repository and (except runtime state) intended to be
committable so collaborators share it:

```text
<workspace>/
├── AGENTS.md                    # project instructions (repo root, highest-ranked)
└── .codewhale/
    ├── config.toml              # project config overlay (safe keys only unless trusted)
    ├── constitution.json        # repo law — enforced invariants, authority policy
    ├── instructions.md          # compat project instructions (read-only fallback)
    ├── rules/*.md               # auto-loaded project rules
    ├── skills/<id>/SKILL.md     # repo-scoped skills
    ├── hooks.toml               # project hooks (loaded only for trusted workspaces)
    ├── agents/*.toml            # Fleet agent profiles (/fleet setup, /fleet party)
    ├── fleet.jsonl              # append-only Fleet run ledger
    ├── fleet/<run>/<task>/…     # Fleet worker logs and artifacts
    ├── state/subagents.v1.json  # sub-agent persistence
    └── handoff.md               # session handoff artifact (/relay)
```

Sub-agent worktrees are created in a **sibling** directory of the checkout
named `.codewhale-worktrees/`, not inside the repo.

Compat inputs CodeWhale also reads if present: `CLAUDE.md`,
`.claude/instructions.md`, `.claude/rules/`, `.claude/skills/`,
`.agents/skills/`, and legacy `.deepseek/` mirrors. `WHALE.md` is deprecated
and ignored (a migration warning is shown).

## Skill discovery order

First match wins on name conflicts:

1. Workspace: `.agents/skills` → `skills/` → `.opencode/skills` →
   `.claude/skills` → `.cursor/skills` → `.codewhale/skills`
2. User: `~/.agents/skills` → `~/.claude/skills` → `~/.codewhale/skills` →
   `~/.deepseek/skills` (legacy)

`~/.codewhale/skills` is the default install target.

## Legacy `~/.deepseek` migration

CodeWhale was renamed from DeepSeek-TUI. Migration is *read-with-fallback,
write-to-new*: existing `~/.deepseek` state keeps working, is relocated to
`~/.codewhale` on first write per subdirectory, and new state is only ever
written under `~/.codewhale`. `DEEPSEEK_CONFIG_PATH` and `DEEPSEEK_TASKS_DIR`
remain as env aliases. The full audit of remaining legacy fallbacks lives in
[LEGACY_PATHS.md](LEGACY_PATHS.md).

Related: [CONFIGURATION.md](CONFIGURATION.md) (precedence and keys),
[CONSTITUTION.md](CONSTITUTION.md) (the constitution layers),
[FLEET.md](FLEET.md) (fleet state), [RESTORE.md](RESTORE.md) (snapshots).
