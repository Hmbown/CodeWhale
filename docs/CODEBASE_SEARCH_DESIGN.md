# `codebase_search` — Local-First Semantic Code Retrieval

> **Status:** Design note + planned eval scaffold. **Code is DEFERRED.**
> GitHub #2680 · Milestone **v0.9.0** · This DOC ships in **v0.8.53** (doc-only; no catalog code in this cycle).
> Related in-flight: PR #2684 (subagent role vocab / lifecycle signals / eval ergonomics), PR #2685 (git history active + RLM/field errors). This note must not contradict either.

This document specifies a model-visible `codebase_search` tool for concept-level code retrieval, the storage/index that backs it, a verifiable benchmark set, and a phased feature-flag plan. It also records the surrounding **tool lifecycle** decisions for v0.8.53 so the eventual catalog edit is a single deterministic change.

---

## 1. Problem

Today CodeWhale ships two complementary code-locating tools and one structure map:

- `file_search` — **filename** search (uses the `ignore` crate's `WalkBuilder` for vendor exclusion; default excludes at `crates/tui/src/tools/file_search.rs:210-219`).
- `grep_files` — **content** search (literal/regex token match).
- `project_map` — a deferred **structure** map.

None of these answer **concept-level** questions where the user does not know the exact token:

- "Where is provider auth resolved?"
- "What enforces the shell approval policy?"
- "Where do mode prompts get assembled?"
- "How does the subagent lifecycle close out a child?"

`grep_files` requires you to already know the literal string (`resolve_api_key`, `ApprovalRequirement`, …). When the concept and the identifier diverge — which is the normal case for an unfamiliar area of the tree — grep returns nothing useful and the agent burns turns guessing tokens.

**Goal.** Add a retrieval tool keyed on *intent*, not on exact lexemes, that returns ranked, **explainable** code locations.

**Non-goal / explicit complement.** `codebase_search` does **not** replace `grep_files` or `file_search`. Exact-token and filename lookups remain the right tool when you know what you're looking for. `codebase_search` is the "I don't know the token yet" entry point and always falls back to exact grep so it is never *worse* than grep for a literal query. (See §2 fallback, §6 non-goals.)

There is currently **no** FTS5/BM25, sparse, or dense index in the tree. `rusqlite` is already a workspace dependency (`crates/tui/Cargo.toml`), so the lexical core can be built with no new heavy dependencies.

---

## 2. Approach Comparison

| Approach | What it indexes | Local-first? | Recall on paraphrase | Cost / deps | Verdict for v0.9.0 |
|---|---|---|---|---|---|
| **Lexical FTS5 + `bm25()`** | tokenized code/comments/identifiers (camelCase/snake_case split) | Yes — SQLite built in via `rusqlite` | Medium (with tokenizer help) | Near-zero (existing dep) | **Phase 1 core** |
| **Symbol / path ranking** | extracted symbols (fn/struct/impl/const), path components | Yes | Medium-high for "where is X defined" | Low (regex/tree-sitter optional) | **Phase 1 core** |
| **Sparse encoders (SPLADE)** | learned term-expansion weights | Yes (model is local but heavy) | High | Model download + inference | Phase 3, feature-flagged |
| **Dense embeddings** | vector of chunk semantics | Optional — embedding model needed | Highest on paraphrase | Model + vector store; HF download | Phase 3, feature-flagged |
| **Cross-encoder reranker** | re-scores top-K candidates | Heavy | Boosts precision@k | Inference cost | Phase 4, feature-flagged |

### Recommended architecture: Hybrid via Reciprocal-Rank Fusion (RRF)

Each enabled signal produces an independent ranked list; results are merged with RRF
(`score(d) = Σ_signals 1/(k + rank_signal(d))`, conventional `k≈60`). RRF is chosen because it fuses heterogeneous scorers (BM25 scores, integer symbol ranks, path-depth ranks, cosine similarities) without needing score normalization across incomparable scales.

**v0.9.0 Phase 1 signal set (all local, no model downloads):**

1. **Lexical (FTS5 `bm25()`)** over chunk text with an identifier-aware tokenizer.
2. **Symbol rank** — boost chunks whose extracted symbol name fuzzy-matches query terms.
3. **Path rank** — boost chunks whose path components match (e.g. query "auth" → `…/auth/…`, `…/provider…`).
4. **Session-relevance boost** — recently read/edited files in the current session rank higher (mtime + session touch log). This mirrors how a human grounds "where is X" against what they were just looking at.
5. **Exact grep fallback** — the query is *also* run as a literal `grep_files`-equivalent pass; any exact hit is fused in and tagged, guaranteeing `codebase_search` ⊇ grep for literal queries.

**Optional later backends (feature-flagged, off by default):**

- `--features sparse-splade` — adds a SPLADE signal list to the RRF.
- `--features dense-embed` — adds a dense vector signal list (embedding model gated behind the same workset/feature flag as any HF download; see §3 Privacy).
- `--features rerank` — cross-encoder reranks the fused top-K.

Phase 1 deliberately omits all four ML backends so the tool ships with zero network/model dependency and is reproducible in CI.

---

## 3. Storage & Index

### Location

```
~/.codewhale/index/<workspace-hash>.db
```

`<workspace-hash>` is a stable hash of the canonical workspace root, so each checkout/worktree gets its own index and nothing is shared across unrelated projects. Backed by `rusqlite` (existing dep).

> Migration note (ties to the `/memory doctor` taxonomy in §7): older builds used `~/.deepseek`. The index path is `~/.codewhale` only; if a legacy `~/.deepseek/index` exists it is ignored (a future `doctor` may offer to migrate, never auto-read).

### Schema sketch

```sql
CREATE TABLE files (
  id            INTEGER PRIMARY KEY,
  path          TEXT NOT NULL UNIQUE,   -- workspace-relative
  mtime_ns      INTEGER NOT NULL,       -- invalidation
  size_bytes    INTEGER NOT NULL,
  content_hash  TEXT NOT NULL,          -- blake3; skip re-chunk if unchanged
  lang          TEXT,                   -- detected language
  branch        TEXT                    -- branch at last index (invalidation)
);

CREATE TABLE chunks (
  id          INTEGER PRIMARY KEY,
  file_id     INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  start_line  INTEGER NOT NULL,
  end_line    INTEGER NOT NULL,
  kind        TEXT,                     -- fn | struct | impl | const | doc | block
  symbol      TEXT,                     -- primary symbol name if any
  text        TEXT NOT NULL             -- chunk body (identifier-split copy for FTS)
);

-- Lexical index. external-content FTS so we don't duplicate bodies twice.
CREATE VIRTUAL TABLE chunks_fts USING fts5(
  text,
  symbol,
  content='chunks',
  content_rowid='id',
  tokenize = 'unicode61 remove_diacritics 2'   -- + identifier pre-split at index time
);

CREATE TABLE symbols (
  id        INTEGER PRIMARY KEY,
  file_id   INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
  chunk_id  INTEGER REFERENCES chunks(id) ON DELETE CASCADE,
  name      TEXT NOT NULL,
  kind      TEXT NOT NULL,              -- fn | struct | enum | trait | impl | const | macro
  line      INTEGER NOT NULL
);
CREATE INDEX symbols_name ON symbols(name);

-- Session relevance: lightweight touch log, written by the session, decayed on read.
CREATE TABLE session_touch (
  path        TEXT PRIMARY KEY,
  last_touch  INTEGER NOT NULL,         -- unix ns
  touch_count INTEGER NOT NULL DEFAULT 1
);
```

Identifier-aware tokenization (splitting `resolveApiKey` / `resolve_api_key` → `resolve api key`) is applied **at index time** into the FTS `text` column so the query side stays a plain FTS5 MATCH. SPLADE/dense backends, when enabled, add their own sidecar tables (`chunks_sparse`, `chunks_vec`) behind their feature flags.

### Chunking strategy (structure-aware)

Chunk on **syntactic boundaries**, not fixed windows: one chunk per top-level item (`fn`, `struct`, `impl` block, `const`, doc-comment block), falling back to a sliding window for unparseable files. Structure-aware chunks keep a function and its doc comment together, so a paraphrase query lands on a coherent unit rather than a mid-function slice. A tree-sitter grammar per language is the long-term plan; Phase 1 may start with a brace/indent + regex heuristic for Rust/TS and a line-window fallback elsewhere.

### Invalidation

- **mtime + content_hash:** on index/refresh, skip files whose `mtime_ns` and `content_hash` are unchanged.
- **Branch switch:** `files.branch` is recorded; on a branch change the affected files are re-checked (cheap because of content_hash).
- **Generated / vendor exclusion:** reuse the **same** `ignore`-crate `WalkBuilder` exclusion behavior as `file_search` (mirror the defaults at `crates/tui/src/tools/file_search.rs:210-219`: `target/**`, `node_modules/**`, `.git/**`, `DerivedData/**`, `dist/**`, `build/**`, `*.lock`, `*.plist`, plus `.gitignore`). One exclusion source of truth shared with `file_search` avoids index drift.

### Privacy / trust

- **Workspace-scoped, local-only.** The index lives under `~/.codewhale/index/` and never leaves the machine.
- **No cloud by default.** Phase 1 has zero network dependency.
- **Embeddings / Hugging Face downloads are gated.** Any SPLADE/dense backend (which may pull a model from HF) is behind a feature flag *and* an explicit workset/opt-in, consistent with how the rest of CodeWhale treats network model access. The core tool never downloads anything.

---

## 4. Model-Visible Tool Contract

```jsonc
// codebase_search
{
  "name": "codebase_search",
  "description": "Concept-level code retrieval. Find code by what it does, even without exact tokens. Complements grep_files (exact text) and file_search (filenames).",
  "parameters": {
    "query":      { "type": "string",  "description": "Natural-language or concept query, e.g. 'where is provider auth resolved'." },
    "max_results":{ "type": "integer", "default": 10 },
    "path_glob":  { "type": "string",  "description": "Optional path filter, e.g. 'crates/tui/**'." },
    "lang":       { "type": "string",  "description": "Optional language filter." },
    "kind":       { "type": "string",  "description": "Optional symbol-kind filter: fn|struct|impl|const|..." }
  }
}
```

**Result shape — ranked, explainable, auditable:**

```jsonc
{
  "results": [
    {
      "path": "crates/tui/src/config/provider.rs",
      "line": 142,
      "snippet": "fn resolve_api_key(provider: ApiProvider, env: &Env) -> Result<Secret> { ... }",
      "score": 0.91,
      "reasons": [
        "symbol: resolve_api_key matches 'auth/resolve'",
        "lexical: matched tokens [provider, api, key, resolve]",
        "path: component 'provider' matches query",
        "session: file read 2 turns ago"
      ]
    }
  ],
  "backend": "lexical+symbol+path+session",   // which signals were fused (RRF)
  "fallback_grep_hits": 1                       // exact-match hits folded in
}
```

`reasons[]` is **mandatory** and is the auditability contract: every result explains *why* it ranked — which tokens/symbols/path components matched and whether session-recency contributed. This makes retrieval debuggable and lets the model (and the human reviewing a transcript) judge trust. The `backend` field records which signals were active so results are reproducible given the feature set.

---

## 5. Benchmark / Eval Set

A fixed set of real CodeWhale concept queries, each with the **expected** file(s) verified against the current tree, so retrieval quality is measurable (recall@k / MRR). Line numbers are indicative anchors at time of writing; the eval matches on **file**, not line.

| # | Query (concept, no exact token) | Expected file(s) | Anchor |
|---|---|---|---|
| 1 | Where is provider auth / API key resolved? | `crates/tui/src/config/` provider auth path | provider/config module |
| 2 | What is the first-turn active tool set? | `crates/tui/src/core/engine/tool_catalog.rs` | `DEFAULT_ACTIVE_NATIVE_TOOLS` :37-64 |
| 3 | How are deferred tools hydrated / searched? | `crates/tui/src/core/engine/tool_catalog.rs` | tool_search regex/bm25 :26-35 |
| 4 | Why does Arcee get a reduced tool set? (WAF workaround) | `crates/tui/src/core/engine/tool_catalog.rs` | `ARCEE_FIRST_TURN_NATIVE_TOOLS` :106-115 |
| 5 | What keeps the tool catalog byte-stable for the KV prefix cache? | `crates/tui/src/core/engine/tool_catalog.rs` | catalog-head invariant :169-196 |
| 6 | Where is the shell approval / cancel policy? | `crates/tui/src/tools/shell.rs` + `tools/spec.rs` (`ApprovalRequirement`) | shell tools, `ShellWaitTool`/`ShellInteractTool` registry.rs:524-531 |
| 7 | Where are mode prompts (Plan/Agent/YOLO) assembled? | mode prompt / `AppMode` assembly in `crates/tui/src/tui/` | `AppMode` usage |
| 8 | How does the subagent lifecycle open/eval/close a child? | `crates/tui/src/tools/subagent/mod.rs`; registry registration | registry.rs:1017-1029; `send_input`/`cancel`/`resume` mod.rs:1495,1521,1605 |
| 9 | What is the RLM session surface and its default child model? | `crates/tui/src/tools/rlm.rs` | `DEFAULT_CHILD_MODEL = "deepseek-v4-flash"` :26 |
| 10 | Where is RLM eval / var_handle retrieval (`handle_read`)? | `crates/tui/src/tools/rlm.rs`, `tools/handle.rs` | `VarHandle` import rlm.rs:21 |
| 11 | Where are skills discovered and parsed in the workspace? | `crates/tui/src/tools/skills/mod.rs` | `discover_in_workspace` ~421; skill struct ~382-388 |
| 12 | Where is skill enable-state stored / checked? | `crates/tui/src/tools/skills/skill_state.rs` | `SkillStateStore::is_enabled` ~73 |
| 13 | How does vendor/generated exclusion work for file walking? | `crates/tui/src/tools/file_search.rs` | `ignore` WalkBuilder excludes :210-219 |
| 14 | Where is the queued user message built on submit? | `crates/tui/src/tui/ui.rs` | `build_queued_message` ~4721 |
| 15 | Where are speech / TTS tools registered? (duplicate names) | `crates/tui/src/tools/registry.rs` | `speech` ≡ `tts` :787-792 |

Each entry is intended to become a `(query, expected_paths[])` row in a fixture
(e.g. `crates/tui/tests/fixtures/codebase_search_eval.jsonl`). This PR ships
the design table only; the fixture and harness are deferred to Phase 1. The
Phase 1 harness runs all queries against the live index and reports recall@k
and MRR; a regression bar (e.g. recall@10 >= target) gates future ranking
changes.

---

## 6. Phasing, Feature Flags, and Non-Goals

### Phasing

- **Phase 0 (this cycle, v0.8.53):** this design note + benchmark table only. No fixture, harness, or catalog code.
- **Phase 1 (v0.9.0):** local lexical core — FTS5 `bm25()` + symbol + path + session-relevance + exact grep fallback, fused via RRF. SQLite index at `~/.codewhale/index/<workspace-hash>.db`. Eval harness wired into CI. **No network, no model downloads.** Tool registered as deferred (hydrated via tool-search) initially; promotion to the active first-turn set is a separate, deliberate decision (see lifecycle below) because of the prefix-cache invariant.
- **Phase 2:** incremental/background reindex, branch-aware invalidation hardening, richer chunkers (tree-sitter per language).
- **Phase 3 (feature-flagged, off by default):** `sparse-splade` and `dense-embed` RRF signals. Embedding/HF downloads behind the flag + workset opt-in (§3 Privacy).
- **Phase 4 (feature-flagged):** `rerank` cross-encoder over the fused top-K.

### Feature flags

```
codebase-search-core    # Phase 1, default-on once it lands
sparse-splade           # Phase 3, default-off
dense-embed             # Phase 3, default-off (gated HF download)
rerank                  # Phase 4, default-off
```

### Non-goals

- **No cloud index is required** for the core experience. Ever, for Phase 1.
- **Not a grep replacement.** Exact-token (`grep_files`) and filename (`file_search`) search stay first-class; `codebase_search` complements them and folds exact hits in as a fallback.
- Not a code-rewrite or navigation/LSP tool — it returns ranked locations, nothing more.

### Cross-link: WhaleFlow epic

`codebase_search` is a building block for the long-running multi-agent **WhaleFlow** (`/workflow` / `/whaleflow`) epic: a planning or executor lane can ground itself ("find where X is handled") without spending shell/grep turns, and the explainable `reasons[]` feed audit trails. Sequencing here must not regress PR #2684 (subagent lifecycle/eval ergonomics) or PR #2685 (git history active + RLM/field errors).

---

## Appendix A — Tool Lifecycle Decisions (v0.8.53, doc-only)

These are **design decisions for the eventual one-time catalog edit**; no catalog code changes this cycle. The active first-turn tool block is a DeepSeek KV prefix-cache invariant (`tool_catalog.rs:169-196`) — it must stay byte-identical run-to-run, so any change is a single deterministic edit, never incremental churn.

### Lifecycle states (represented as const name-sets + an alias table in `tool_catalog.rs`, NOT a per-`ToolSpec` field)

| State | Active first turn? | In tool-search? | Registered/dispatchable? | Result-metadata notice? |
|---|---|---|---|---|
| **active** | yes | yes | yes | no |
| **deferred** | no | yes | yes | no |
| **hidden-compatibility** | no | no | yes | no |
| **deprecated** | no | no | yes | yes (replacement notice, **metadata only**) |
| **removed** | no | no | no | — |

Deprecated/hidden tools stay **registered and dispatchable** so old transcripts always replay. A deprecated tool appends a replacement notice to **RESULT METADATA only** — never to the cached prefix (which would break the invariant).

### Planned diet (documented, not yet coded)

- **`exec_wait`, `exec_interact`, `tts` → hidden-compatibility.** These are exact duplicates of canonical tools:
  - `exec_wait` ≡ `exec_shell_wait` (same `ShellWaitTool`, `registry.rs:526,529`); router already unifies them at `crates/tui/src/tui/tool_routing.rs:1139-1140`.
  - `exec_interact` ≡ `exec_shell_interact` (same `ShellInteractTool`, `registry.rs:527,530`).
  - `tts` ≡ `speech` (same `SpeechTool`, `registry.rs:787-792`).
  - Action: drop from active + search, keep registered, identical behavior, **no notice**.
- **`todo_*` (`todo_write/add/update/list`) → deprecated → `checklist_*`.** They are deferred twins of `checklist_*` (same `TodoWriteTool::new` vs `::checklist`, `todo.rs:187,194`); `checklist_write` is active, and `todo_*` are **not** in the active set. Action: drop from tool-search, keep registered, **add replacement notice** (metadata only).
- **Legacy subagent names** (`agent_spawn`, `spawn_agent`, `agent_result`, `agent_wait`, `agent_send_input`, `send_input`, `agent_assign`, `agent_list`, `agent_cancel`, `resume_agent`, `delegate_to_agent`) are already `#[allow(dead_code)]` structs never instantiated outside tests (`crates/tui/src/tools/subagent/mod.rs`) → **already not model-visible.** Action: cleanup + guardrail tests, **rebased on PR #2684.** Note the live internal `SubAgentManager` methods `send_input`/`cancel`/`resume` (`mod.rs:1495,1521,1605`) are used by `agent_eval`/`agent_close` and **must be kept** — only the model-visible *tool* names are retired.

### Model-visible subagent surface (unchanged)

Only `agent_open`, `agent_eval`, `tool_agent`, `agent_close` are registered (`registry.rs:1017-1029`).

- **`tool_agent` — KEEP as a canonical subagent tool, GATED to DeepSeek-V4 models ONLY.** It is the fast non-thinking "Fin" executor lane built on `deepseek-v4-flash` (cf. RLM `DEFAULT_CHILD_MODEL = "deepseek-v4-flash"`, `rlm.rs:26`). On non-DeepSeek-V4 providers it must not be offered. This is a model/provider-gating decision recorded here for the eventual catalog edit.

### Explicitly NOT touched (distinct niches, per #2681 non-goals — doc-only canonical guidance)

`apply_patch` / `edit_file` / `write_file` / `fim_edit`; `grep_files` / `file_search` / `project_map`; `fetch_url` / `web.run` / `web_search`; `task_shell_*`; `handle_read` / `retrieve_tool_result`. These serve distinct purposes and stay as-is.

---

## Appendix B — Command-Surface Taxonomy (context)

Each name maps to exactly one thing; `codebase_search` slots in as concept-level code retrieval alongside these surfaces:

- `/memory` — small user prefs/facts only (subcommands `add`/`edit`/`search`/`clear`/`doctor`, plus later `promote`; `doctor` detects the legacy `~/.deepseek` path).
- `/context` — dashboard of all active layers.
- `/rules` — repo guidance.
- `/workflow` (`/whaleflow`) — long-running multi-agent (the WhaleFlow epic).
- `/overlay` — promoted cached-main lessons.
- `$<skill-name>` — skill invocation prefix; the token *is* the skill name (e.g. `$systematic-debugging`, `$github:gh-fix-ci`).
- `codebase_search` — concept-level code retrieval (this document).
