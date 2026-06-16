# Hippocampal Memory System — Design Document

> **PR**: #2933 (v2 update)
> **Discussion**: #3234
> **Author**: @cy2311

## 1. Motivation

CodeWhale's 1M-token context provides ample short-term workspace, but no mechanism for
**cross-session recall**. Every `/compact` or new session starts blank — architecture
decisions, user preferences, and project conventions learned in previous sessions are lost.

The Hippocampal Memory System provides a persistent, structured, SQLite-backed memory store
that survives compaction and spans sessions.

## 2. Design Principles

| Principle | Explanation |
|---|---|
| **Local-first** | Memory lives in a local SQLite file. No cloud dependency. |
| **Explicit + Automatic** | Agent can explicitly `memorize`/`recall`, AND the system auto-injects context. |
| **Structured, not flat** | Entity-relation graph + FTS5 full-text search. |
| **Tiered importance** | Facts scored 0.0–1.0. High-importance facts survive pruning. |
| **Opt-in** | Disabled by default, enabled via `memory_db_path` config. |

## 3. Architecture

```
Layer 3: System Integration
  ├─ Prompt Injection: top-8 facts into `<memory_context>` on every refresh
  └─ Memory Daemon: tokio background task, 6h prune + stats

Layer 2: Agent Tools
  ├─ memorize(content, importance, entity, glossary_tags[], namespace)
  ├─ recall(query, namespace, glossary_tag, include_graph, limit)
  └─ consolidate(stats|rollback|prune|merge)

Layer 1: Storage (crates/memory/)
  ├─ entities + relations (entity graph)
  ├─ facts + facts_fts (FTS5 full-text search)
  ├─ glossary + fact_glossary + entity_glossary (keyword system)
  ├─ namespaces (workspace isolation)
  ├─ fact_versions (rollback support)
  └─ SQLite (bundled via rusqlite, zero extra deps)
```

## 4. Storage Design

### 4.1 Why SQLite + FTS5 (not Vector DB)

| Approach | Pros | Cons |
|---|---|---|
| **SQLite + FTS5** (selected) | Zero deps (bundled via rusqlite), ACID, FTS5 fast keyword search, <10MB | No semantic search |
| **Vector DB** | Semantic search | Heavy deps, service needed, overkill for structured facts |
| **Flat JSON** | Simple | No indexing, no concurrent access |

**Decision**: For an AI coding agent, memory retrieval is primarily keyword-driven
("what was the decision about database schema?"). FTS5 provides instant prefix-match search.
Vector search can be added later as an optional layer on top.

### 4.2 Schema Maps

**v1 (original - preserved)**:
- `entities(id, kind, name, description, created_at, updated_at)`
- `relations(id, source_id, target_id, kind, strength, created_at, session_id)`
- `facts(id, entity_id, content, source, importance, created_at, session_id)`
- `facts_fts` — FTS5 virtual table

**v2 additions**:
- `namespaces(id, name, description, created_at, updated_at)` — workspace isolation
- `glossary(id, term, definition, category, namespace_id)` — keyword/tag system
- `fact_glossary(fact_id, glossary_id)` + `entity_glossary(entity_id, glossary_id)` — M:N links
- `fact_versions(id, fact_id, content, source, importance, version, session_id)` — rollback

### 4.3 Migration System

Schema changes via `schema_version` table. Each migration is a numbered function
that runs exactly once (safe for existing databases).

### 4.4 Namespace Isolation

Workspaces get namespaces (`workspace:/path/to/project`). Facts/entities can be scoped,
enabling multi-project isolation from a single DB file.

### 4.5 Fact Versioning & Rollback

Every `update_fact` saves the previous version. The `consolidate rollback` action
restores any version. Version numbers are monotonic; rollback preserves the full audit trail.

## 5. Tool Design

### memorize — Explicit Storage

Input: `{ content, importance (0.0-1.0), entity_kind?, entity_name?, glossary_tags?, namespace? }`
- Creates/updates entity, creates fact (version 1), links glossary tags
- Auto-approved (low risk)

### recall — Structured Retrieval

Input: `{ query, limit, namespace?, glossary_tag?, include_graph? }`
- FTS5 full-text search ordered by importance DESC
- Optional namespace + glossary tag filtering
- Returns facts with linked entities, relations, and tags
- Fallback: top important facts when query returns empty

### consolidate — Maintenance

Input: `{ action (stats|rollback|prune|merge), fact_id?, target_version?, importance_threshold?, older_than_days? }`
- stats: memory usage report
- prune: delete low-importance facts (active forgetting)
- rollback: restore fact to previous version
- merge: deduplicate identical facts

## 6. System Integration

### Prompt Injection

On every `refresh_system_prompt()`, the engine queries the top 8 facts and injects:
```
<memory_context>
1. [imp=0.9] Service X uses PostgreSQL...
   tags: [database, postgresql]
...
Automatically call memorize when you discover architecture decisions...
</memory_context>
```

### Auto-Memorize Guidance

System prompt tells the model to auto-call `memorize` for architecture decisions,
user preferences, project conventions, etc. Zero extra API calls.

### Background Daemon

`tokio::spawn` task every 6 hours: prune facts (importance < 0.3, age > 30 days) + log stats.

## 7. Comparison with Alternatives

| Feature | Codewhale v2 | Mem0 | Nocturne Memory | Awareness-Local |
|---|---|---|---|---|
| Storage | SQLite+FTS5 | SQLite+Vector | SQLite+FTS5 | SQLite |
| Entity Graph | ✅ | ❌ | ✅ | ❌ |
| Importance Scoring | ✅ 0.0-1.0 | ❌ | ❌ | ❌ |
| Active Forgetting | ✅ prune+daemon | ❌ | ❌ | ❌ |
| Fact Versioning | ✅ versions+rollback | ❌ | ❌ | ❌ |
| Glossary/Tags | ✅ glossary table | ❌ | ✅ keywords | ❌ |
| Namespace Isolation | ✅ | ❌ | ✅ v2.0 | ❌ |
| Migration System | ✅ versioned | ❌ | ✅ 13 migrations | ❌ |
| Background Daemon | ✅ 6h interval | ❌ | ❌ | ✅ daemon.mjs |
| Prompt Injection | ✅ memory_context | API only | MCP auto | MCP auto |

## 8. Future Roadmap

**Short-term**: Vector search layer, MCP Server mode (OpenClaw compatible),
config integration (`[memory.hippocampal]` in config.toml), `/memory` CLI.

**Medium-term**: Semantic merge (LLM dedup), access-frequency importance scoring,
memory decay curves, export/import, web UI.

**Long-term**: Multi-user shared namespaces, adaptive pruning, temporal reasoning.
