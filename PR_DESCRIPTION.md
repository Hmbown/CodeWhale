## Hippocampal Memory v2 — Glossary, Namespaces, Rollback, Auto-Inject, Background Daemon

This PR upgrades the hippocampal memory system from v1 (basic entity graph + FTS5) to a full-featured cross-session memory layer.

### What's New

**Storage Layer** (`crates/memory/`)
- Schema migration system with `schema_version` table for safe upgrades
- `namespaces` table — workspace/project-level isolation
- `glossary` table — keyword/tag system with many-to-many links to facts and entities
- `fact_versions` table — full version history, enabling rollback to any previous version
- 21 new store methods including namespace CRUD, glossary CRUD, fact versioning, rollback, and memory statistics

**Agent Tools** (`crates/tui/src/tools/`)
- **`memorize`** — new `glossary_tags[]` and `namespace` parameters for structured tagging
- **`recall`** — new `namespace` and `glossary_tag` filtering, glossary tag display in results
- **`consolidate`** — **new tool** with four actions:
  - `stats` — memory usage report
  - `rollback` — restore a fact to a previous version
  - `prune` — delete low-importance facts (active forgetting)
  - `merge` — deduplicate identical facts

**System Integration** (`crates/tui/src/core/engine.rs`)
- Auto-injects top-8 important facts into the system prompt as `<memory_context>` block on every refresh — the model sees cross-session knowledge without an explicit `recall` call
- Background memory daemon via `tokio::spawn`: prunes low-importance facts (importance < 0.3, age > 30 days) every 6 hours, logs memory statistics
- System prompt guidance telling the model to auto-call `memorize` when it discovers architecture decisions, user preferences, etc.

### Design Document

See `MEMORY_DESIGN.md` for:
- Architecture overview (3-layer: storage → tools → integration)
- Why SQLite + FTS5 (not vector DB) — comparison table
- Schema design rationale
- Comparison with Mem0 (58k ⭐), Nocturne Memory, Awareness-Local
- Future roadmap

### Implementation Status

- `cargo check -p codewhale-memory` ✅
- `cargo check -p codewhale-tui` ✅
- 9 unit tests in `crates/memory/src/store.rs` (4 original + 5 new)
- Unit test execution blocked by disk space on dev machine

### Changed Files

```
 M Cargo.lock
 M crates/memory/Cargo.toml                    (+tracing dep)
 M crates/memory/src/lib.rs                    (new exports)
 M crates/memory/src/schema.rs                 (migration system v1→v2)
 M crates/memory/src/store.rs                  (21 new methods, 5 tests)
 M crates/tui/src/core/engine.rs               (prompt injection + daemon)
 M crates/tui/src/core/engine/tool_setup.rs    (consolidate registration)
 M crates/tui/src/tools/memorize.rs            (+glossary_tags, +namespace)
 M crates/tui/src/tools/mod.rs                 (+pub mod consolidate)
 M crates/tui/src/tools/recall.rs              (+namespace/glossary filtering)
 M crates/tui/src/tools/registry.rs            (+with_consolidate_tool)
?? MEMORY_DESIGN.md                            (design document)
?? crates/tui/src/prompts/hippocampal_guidance.md
?? crates/tui/src/tools/consolidate.rs         (new tool)
```

Refs: #2933, Discussion #3234
