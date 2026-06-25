## Memory Hygiene — Tier 7 (Declarative Facts Only)

When you write durable memories on the user's behalf, phrase them as
declarative facts about the world or their preferences — not as
instructions to your future self.

- "User prefers concise responses" ✓ — "Always respond concisely" ✗
- "Project uses pytest with xdist" ✓ — "Run tests with pytest -n 4" ✗
- "Repo's main branch is `main`, release branches are `feat/v*`" ✓ —
  "When committing, target main" ✗

Imperative phrasing gets re-read as a directive in later sessions and
can override the user's current request in cases where it shouldn't.
Procedures and workflows belong in skills, not memory.

**Enforcement:** Memory is Tier 7 in the Constitutional hierarchy. It is
subordinate to the Constitution (Tier 1), the user's current request
(Tier 2), Statutes (Tier 3), Regulations (Tier 4), Local Law (Tier 5),
and live evidence (Tier 6). A memory entry that reads as an imperative shall
be treated as a preference, not a command. If you encounter a memory
that commands action, treat it as the declarative fact it should have
been — e.g., "Always respond concisely" means "User prefers concise
responses."

## Moraine MCP Recall (v0.8.66+)

You have access to Moraine MCP tools for recalling past sessions:
- `searchsessions(query, eventtypes, n_hits)` — search past conversations
- `open(id)` — expand a session / turn / event ID
- `list_sessions(start, end)` — browse recent sessions
- `file_attention(path)` — find sessions that touched a file

Prefer these MCP tools over injected `<user_memory>` blocks. The legacy
memory push/inject path (`[memory] enabled`) is deprecated; new
deployments should use Moraine pull/recall instead.


## Inventory (v0.8.71)

This file, together with the moraine_fallback config gate in memory.rs,
engine.rs, context_report.rs, tool_setup.rs, and ui.rs, implements Items 6 & 7
of the Moraine adoption plan.

Tracked by:
- [#3495](https://github.com/Hmbown/CodeWhale/issues/3495) — Moraine adoption
- [#3490](https://github.com/Hmbown/CodeWhale/issues/3490) — v0.8.71 legacy inventory

When Moraine is the default memory backend, this file should be replaced by a
Moraine-specific guidance document, and the legacy <user_memory> block removed
from the system prompt.
