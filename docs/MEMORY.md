# User Memory

The user-memory feature gives the model a small persistent note file
that's injected into the system prompt on every turn. It's the place
to put preferences and conventions that should survive across
sessions — "I prefer pytest over unittest", "this codebase uses
4-space indentation", "always run `cargo fmt` before committing" —
without having to repeat them in every conversation.

Memory is **opt-in**. When disabled (the default), nothing is loaded,
nothing is intercepted, and the `remember` tool isn't surfaced to the
model. This keeps zero-overhead behavior for users who haven't asked
for the feature.

## Enabling memory

Either set the env var:

```bash
export DEEPSEEK_MEMORY=on
```

Accepted truthy values are `1`, `on`, `true`, `yes`, `y`, and
`enabled`.

…or add to `~/.deepseek/config.toml`:

```toml
[memory]
enabled = true
```

Restart the TUI after toggling. Disabling is the same in reverse.

The memory file lives at `~/.deepseek/memory.md` by default; override
with `memory_path` in `config.toml` or `DEEPSEEK_MEMORY_PATH` in
the environment. `DEEPSEEK_MEMORY_PATH` wins over the config file when
both are set.

## Quick examples

```text
# remember that this repo prefers cargo fmt before commits
/memory
/memory path
/memory edit
/memory help
```

- Type `# remember that this repo prefers cargo fmt before commits` in
  the composer to append a timestamped bullet without firing a turn.
- Run `/memory` to confirm where the feature is writing and what is
  currently stored.
- Run `/memory edit` when you want to groom the file manually in your
  editor.

## What gets injected

When memory is enabled and the file exists, every turn's system
prompt carries an extra block:

```xml
<user_memory source="/Users/you/.deepseek/memory.md">
- (2026-05-03 22:14 UTC) prefer pytest over unittest
- (2026-05-03 22:31 UTC) this codebase uses 4-space indentation
…
</user_memory>
```

The block sits above the volatile-content boundary in the prompt
assembly so it stays inside DeepSeek's prefix cache turn-over-turn.
The file is read at every prompt-build call — edits via `/memory`
or external editors land on the next turn, no restart needed.

Files larger than 100 KiB are loaded but truncated, with a marker
appended so you can see the cut.

## Three ways to add to memory

### 1. The `# ` composer prefix (#492)

Type a single line that starts with `#` (but not `##` or `#!`) in
the composer:

```
# remember to use 4-space indentation in this repo
```

The TUI intercepts the input and appends a timestamped bullet to
your memory file. **No turn fires** — your input is consumed, the
status line confirms the path it wrote to, and you can keep typing
your real question.

Multi-`#` prefixes deliberately fall through to normal turn
submission so you can paste Markdown headings without surprise.

### 2. The `/memory` slash command (#491)

Inspect, clear, or get hints about editing the file:

| Subcommand          | Effect                                                 |
|---------------------|--------------------------------------------------------|
| `/memory`           | Show the resolved path and current contents inline    |
| `/memory show`      | Alias for the no-arg form                              |
| `/memory path`      | Print just the resolved path                          |
| `/memory clear`     | Replace the file with an empty marker                 |
| `/memory edit`      | Print the `${VISUAL:-${EDITOR:-vi}} <path>` shell line |
| `/memory help`      | Show command-specific help and the current path       |

The `/memory edit` form intentionally just prints the command rather
than spawning the editor in-process — that keeps the slash-command
handler simple and consistent regardless of which editor you use.

You can also discover the feature from the general help surfaces:

- `/help memory` shows the slash-command summary and usage line.
- `/memory help` prints the memory-specific subcommands plus the
  resolved path.

### 3. The `remember` tool (auto-update, #489)

When memory is enabled the model gets a `remember` tool with this
shape:

```json
{
  "name": "remember",
  "description": "Append a durable note to the user memory file...",
  "input_schema": {
    "type": "object",
    "properties": {
      "note": { "type": "string", ... }
    },
    "required": ["note"]
  }
}
```

The model uses this when it notices a durable preference, convention,
or fact worth keeping across sessions. The tool is auto-approved
because writes are scoped to the user's own memory file — gating
them behind the standard write-approval flow would defeat the point
of automatic memory capture.

If the model uses `remember` for transient task state ("I'm
currently editing foo.rs") the result is harmless but wastes
context. The tool's description explicitly tells the model **not**
to do that — durable, single-sentence notes only.

## File format

Memory is plain Markdown with timestamped bullets:

```markdown
- (2026-05-03 22:14 UTC) prefer pytest over unittest
- (2026-05-03 22:31 UTC) this codebase uses 4-space indentation
- (2026-05-04 09:02 UTC) all PRs need 2 reviewers before merge
```

You can hand-edit the file in any editor — the loader doesn't care
about the timestamp format; it just reads the whole file as the
memory block. The timestamp is convention so you can tell when each
note was added when grooming the file.

## Hierarchy and imports

Memory is intentionally **user-scoped** rather than repo-scoped. It
sits alongside — not inside — project instruction sources such as
`AGENTS.md`, `.deepseek/instructions.md`, and `instructions = [...]`.

- Use **memory** for durable personal preferences that should follow
  you across repos and sessions.
- Use **project instructions** for repo-specific conventions that
  should travel with the codebase.

The memory loader currently reads one resolved file path verbatim.
`@path` imports / includes are **not** supported today; if you need a
larger reusable instruction bundle, put it in a project instruction
file or a skill instead.

## What stays out of memory

Memory is for **durable** signal. Things that should NOT live there:

- **Secrets** — no API keys, tokens, passwords. The file is plain
  text on disk and gets injected verbatim into the system prompt.
- **Transient task state** — "I'm currently working on the parser"
  changes every session; it doesn't belong in cross-session memory.
- **Conversation snippets** — quote-style notes belong in the notes
  tool (`note`), not memory.
- **Long instructions** — anything over a few sentences should live
  in `AGENTS.md` (project-level) or in a [skill](../crates/tui/src/skills/mod.rs)
  (reusable instruction packs).

## Privacy and scope

The memory file lives entirely on your machine in `~/.deepseek/`.
It's never uploaded to any cloud service — the TUI only ever
includes it inline in the system prompt that the LLM provider
receives, and only when memory is enabled. If you switch providers
(DeepSeek / NVIDIA NIM / Fireworks / etc.) the same memory file is
used; the file is provider-agnostic.

The file is per-user, not per-project. If you want project-specific
memory, use the project-level `AGENTS.md` or
`.deepseek/instructions.md` files instead — those are loaded by
`project_context` and live in the repo (or wherever you commit
them).

## Configuration reference

```toml
# ~/.deepseek/config.toml
[memory]
enabled = true                    # default false; or set DEEPSEEK_MEMORY=on
# Path is configured at the top-level (next to skills_dir, notes_path):
memory_path = "~/.deepseek/memory.md"
```

| Setting               | Default                       | Override                              |
|-----------------------|-------------------------------|---------------------------------------|
| Memory enabled        | `false`                       | `[memory] enabled = true` or `DEEPSEEK_MEMORY=on` |
| Memory file path      | `~/.deepseek/memory.md`       | `memory_path = "..."` or `DEEPSEEK_MEMORY_PATH=`  |
| Max file size         | 100 KiB                       | (none today; truncation marker shows the cut)     |

## Related

- `docs/SUBAGENTS.md` — sub-agents inherit memory and can use the
  `remember` tool too.
- `docs/CONFIGURATION.md` — full config reference.
- Issue [#489](https://github.com/Hmbown/DeepSeek-TUI/issues/489)
  — phase-1 EPIC tracking the work.
