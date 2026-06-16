## Hippocampal Memory — Cross-Session Recall

You have access to a long-term memory system (`memorize` / `recall` / `consolidate` tools)
that persists facts across sessions and survives compaction.

### When to memorize

Automatically call `memorize` when you discover:
- **Architecture decisions**: "Service X uses PostgreSQL with read replicas"
- **User preferences**: "User prefers 4-space indentation, type hints in Python"
- **Project conventions**: "Tests go in `tests/` mirroring source structure"
- **Configuration details**: "API rate limit is 100 req/min for the free tier"
- **Important relationships**: "Module A depends on module B's internal API"
- **Bug root causes**: "The crash was caused by null pointer in dispatch.rs"

Use importance=0.9+ for critical decisions, 0.7 for useful context, 0.3 for transient notes.
Optionally add `glossary_tags` for cross-referencing (e.g. ["database", "config"]).

### When to recall

Call `recall` at the start of a session to refresh cross-session context,
or whenever you need information that might have been stored in a previous
session. The system also auto-injects your top important facts into the
prompt, but `recall` gives you full-text search over all stored facts.

### When to consolidate

Use `consolidate` periodically to keep the memory store healthy:
- `consolidate action=stats` to check memory usage
- `consolidate action=prune importance_threshold=0.3` to clean low-importance facts
- `consolidate action=rollback fact_id=... target_version=...` to undo a change
- `consolidate action=merge` to deduplicate identical facts
