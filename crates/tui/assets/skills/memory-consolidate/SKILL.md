---
name: memory-consolidate
description: Reflective maintenance pass over the user memory file — merge duplicate facts, fix stale or contradicted information, drop obsolete entries, and keep the file short enough to stay useful on every turn. Use when the user asks to "consolidate memory", "clean up memory", "tidy my notes", or when the memory file has grown long, repetitive, or contradictory.
metadata:
  short-description: Merge, fix, and prune the user memory file
allowed-tools: read_file, edit_file, write_file, exec_shell
---

# Memory Consolidate

The user memory file is loaded into the system prompt on every turn (wrapped in a
`<user_memory>` block). It earns its place only if it stays short, current, and
non-redundant. This skill is a periodic reflective pass that keeps it that way.

## When to run

- The user explicitly asks to consolidate / clean up / tidy memory.
- The memory file has grown long (many bullets), repetitive, or self-contradictory.
- You just added several related entries and want to fold them together.

Do **not** run this unprompted in the middle of unrelated work — it rewrites a file
the user owns. Mention you're about to consolidate, then do it.

## Procedure

1. **Locate the file.** Run `/memory` to see the resolved path (default
   `~/.deepseek/memory.md`, overridable via `memory_path` in `config.toml` or
   `DEEPSEEK_MEMORY_PATH`). Read it with `read_file`.

2. **Classify every entry.** For each bullet, decide which it is:
   - **Durable preference** — how the user wants you to behave (style, tooling, language).
   - **Project fact** — stable truth about a repo or environment.
   - **Reference** — a path, command, URL, or identifier worth remembering.
   - **Stale / one-off** — true only at a past moment, already resolved, or since contradicted.

3. **Merge duplicates.** Collapse bullets that say the same thing into one clear line.
   When two entries conflict, keep the one supported by the most recent evidence and
   drop the other — don't keep both.

4. **Fix what's wrong.** If an entry contradicts what you now know to be true
   (a renamed file, a changed preference, a corrected fact), update it. Memory records
   what was believed when it was written, not ground truth — verify before trusting.

5. **Prune.** Delete stale one-offs and anything no longer useful. A short, trusted
   file beats a long, half-stale one. Aim to keep it tight.

6. **Preserve the format.** Keep the existing Markdown bullet style. Retain the
   `- (YYYY-MM-DD HH:MM UTC) …` timestamp prefix on entries that have one; for merged
   entries, keep the most recent timestamp. Group related bullets under short `##`
   headings if that improves scannability, but don't over-structure a small file.

7. **Write it back** with `write_file` (full rewrite) or `edit_file` (surgical),
   then **show the user a brief diff summary**: how many entries before/after, what you
   merged, what you dropped, and anything you changed because it was contradicted.

## Guardrails

- Never invent facts. Only consolidate what is already there or what you can verify.
- Never remove a durable preference just because it's old — age is not staleness.
- If you're unsure whether an entry is stale, keep it and flag it to the user rather
  than deleting silently.
- Rewriting memory busts the prompt's prefix cache for the next turn — that's expected
  and worth it; don't avoid the cleanup to save a cache hit.
