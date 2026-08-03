# User Memory

User memory gives the model a small, persistent, local store of
preferences and conventions that should survive across sessions —
"I prefer pytest over unittest", "this codebase uses 4-space
indentation" — without repeating them in every conversation.

As of v0.9.4 the **native memory store** is the only memory system.
It is Markdown files indexed by SQLite FTS5, fully offline, scoped by
a hash of the repo's git origin. The legacy single-file
(`~/.deepseek/memory.md`) push/inject path and the planned Moraine MCP
backend were both removed: no Moraine server ever shipped in-repo,
and the native store already provides the same architecture (durable
Markdown source of truth plus a rebuildable search index).

Memory is **opt-in**. When disabled (the default), nothing is loaded,
nothing is intercepted, and the `remember` tool isn't surfaced to the
model.

## Enabling memory

Either set the env var:

```bash
export DEEPSEEK_MEMORY=on
```

Accepted truthy values are `1`, `on`, `true`, `yes`, `y`, and
`enabled`.

…or add to `~/.codewhale/config.toml`:

```toml
[memory]
enabled = true
```

Restart the TUI after toggling. Disabling is the same in reverse.

## Layout

The store lives under a `memory/` directory next to where the legacy
`memory_path` would have pointed — by default `~/.codewhale/memory/`:

```text
~/.codewhale/memory/
├── global/MEMORY.md        # user-scoped notes (follow you everywhere)
├── workspaces/<id>/MEMORY.md  # repo-scoped notes (hash of git origin)
└── index.db                # rebuildable SQLite FTS5 cache
```

Markdown is the durable source of truth; `index.db` is a disposable
full-text cache (`/memory reindex` rebuilds it). A configured
`memory_path` is re-rooted into this layout: whatever file you point
at, its parent gains the `memory/global/MEMORY.md` tree.

## What gets injected

When memory is enabled, the system prompt carries a bounded,
provenance-bearing block of memory entries (up to 32 entries /
12,000 chars, global plus current-workspace scope). The block is
wrapped to mark it as **untrusted user data**, not a second
instruction layer. For depth beyond the injected head, the model can
call the `memory_search` / `memory_get` tools against the FTS5 index.

## Three ways to add to memory

### 1. The `# ` composer prefix (#492)

Type a single line that starts with `#` (but not `##` or `#!`) in
the composer:

```
# remember to use 4-space indentation in this repo
```

The TUI intercepts the input and appends the note to the **global**
native store via the same `NativeMemoryStore::remember` path the
model's tool uses. **No turn fires** — your input is consumed, the
status line confirms the file it wrote to, and you can keep typing
your real question.

Multi-`#` prefixes deliberately fall through to normal turn
submission so you can paste Markdown headings without surprise.

### 2. The `/memory` slash command

Inspect and maintain the native store:

| Subcommand        | Effect                                            |
|-------------------|---------------------------------------------------|
| `/memory`         | Show the store root, active source, and index     |
| `/memory add …`   | Append a note (global scope)                      |
| `/memory reindex` | Rebuild the FTS5 index from the Markdown sources  |
| `/memory help`    | Show command-specific help                        |

### 3. The `remember` tool (auto-capture, #489)

When memory is enabled the model gets a `remember` tool:

```json
{
  "name": "remember",
  "input_schema": {
    "type": "object",
    "properties": {
      "note":  { "type": "string" },
      "scope": { "type": "string", "enum": ["global", "workspace"] }
    },
    "required": ["note"]
  }
}
```

The model uses this when it notices a durable preference, convention,
or fact worth keeping across sessions. The tool is auto-approved
because writes are scoped to the user's own memory files — gating
them behind the standard write-approval flow would defeat the point
of automatic memory capture. Workspace scope requires a git
repository with an `origin` remote (the scope id is a hash of it).

## What stays out of memory

Memory is for **durable** signal. Things that should NOT live there:

- **Secrets** — no API keys, tokens, passwords. The files are plain
  text on disk and entries are injected into the system prompt.
- **Transient task state** — "I'm currently working on the parser"
  changes every session; it doesn't belong in cross-session memory.
- **Conversation snippets** — quote-style notes belong in the notes
  tool (`note`), not memory.
- **Long instructions** — anything over a few sentences should live
  in `AGENTS.md` (project-level) or in a skill.

## Privacy and scope

The store lives entirely on your machine. It is never uploaded to any
cloud service — the TUI only ever includes entries inline in the
system prompt that the LLM provider receives, and only when memory is
enabled. Workspace-scoped memory is keyed by a hash of the repo's git
origin, so notes from one repo never leak into another repo's prompt.

## Configuration reference

```toml
# ~/.codewhale/config.toml
[memory]
enabled = true                    # default false; or set DEEPSEEK_MEMORY=on
# Optional explicit backend selection:
# backend = "native"              # "native" or "off" (default: off)
```

| Setting               | Default                       | Override                              |
|-----------------------|-------------------------------|---------------------------------------|
| Memory enabled        | `false`                       | `[memory] enabled = true` or `DEEPSEEK_MEMORY=on` |
| Backend               | `off`                         | `[memory] backend = "native"`         |
| Store root            | `~/.codewhale/memory/`        | derived from `memory_path`            |

## Related

- `docs/SUBAGENTS.md` — sub-agents inherit memory and can use the
  `remember` tool too.
- `docs/CONFIGURATION.md` — full config reference.
- Issue [#489](https://github.com/Hmbown/CodeWhale/issues/489)
  — phase-1 EPIC tracking the work.
