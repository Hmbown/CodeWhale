# DeepMap -- AI-Native Codebase Mapping for DeepSeek-TUI

## Origin & Motivation

The concept of automatic repository mapping was pioneered by
[aider](https://github.com/Aider-AI/aider), the first CLI coding assistant to
recognize that an AI agent needs a structural understanding of a project before
it can make meaningful edits. Aider introduced a pipeline that combined
tree-sitter parsing with PageRank to produce a compact "project map" -- a
single Markdown document of roughly 1,000 tokens that captures the entry points,
hot files, and key symbols ranked by their structural importance.

Aider's key insight was both simple and profound: a 1,000-token structural map
consistently outperforms 50,000 tokens of raw source code when the agent needs
to understand where to make a change. Raw code dumps overwhelm the model's
attention budget; a ranked map tells it exactly where to look first.

Building on aider's foundation, we developed
[repomap](https://github.com/gjczone/repomap) as an independent open-source
research project. Repomap extended the original concept in several directions:
growing language coverage from 8 to 15 languages, adding incremental scanning
for large projects, introducing impact analysis (given a set of changed files,
which other files need attention?), post-edit verification suggestions, LSP
integration for real-time symbol resolution, and structured JSON reports
designed for AI agent consumption rather than human reading.

DeepMap is the natural evolution of this work. It takes the core engine that
was proven in repomap and rewrites it in Rust, integrating it natively into
DeepSeek-TUI as a first-class analysis capability. Where repomap was a
standalone Python CLI tool, DeepMap lives inside the same process as the
coding agent -- no subprocess, no serialization overhead, no separate
deployment.

Both repomap and DeepMap share a common origin story: they were built by
@gjczone, a non-programmer, using DeepSeek-V4-Pro, with GLM-5.1 and
MIMO-V2.5-Pro for cross-validation and review. The entire
project is evidence that the tools we are building -- AI-native code assistants
with repository awareness -- can also help their own creators build meaningful
infrastructure. The dogfooding is not accidental; it is the point.

## The Problem We Solve

Modern AI-powered coding tools fall into two broad categories with very
different relationships to codebase understanding.

**IDE-based coding assistants** (Cursor, Trae, Qoder, GitHub Copilot) run
inside a full development environment with access to Language Server Protocol
(LSP) servers. They know, natively and in real time, what symbols exist, where
they are defined, how they reference each other, and what errors are present.
The IDE provides omni-completion, go-to-definition, find-all-references, and
rename -- all backed by a live index that is always up to date. For these tools,
codebase awareness is a built-in feature of the platform.

**CLI-based coding assistants** (aider, Claude Code, DeepSeek-TUI) have
historically operated without this advantage. They run in a terminal against a
working tree. They do not control the editor; they do not have a running LSP
server. Their default strategy for understanding a codebase has been to
fall back on general-purpose Unix tools: `grep` for text search, `find` or `fd`
for file discovery, and brute-force file reads to understand dependencies.
This approach has three fundamental problems.

First, **token waste is severe**. An agent that does not know which files matter
will read many files that are irrelevant to the task at hand. A single large
file with 1,500 lines of boilerplate can consume 5,000-10,000 tokens just to
confirm that it has nothing useful. Multiplied across a project of hundreds of
files, the waste can easily reach hundreds of thousands of tokens per session.

Second, **dependencies are invisible to grep**. Text search can find
occurrences of a function name, but it cannot tell you whether those
occurrences are calls or comments. It cannot tell you that `foo()` in module A
resolves to the `foo` defined in module B through a chain of re-exports. It
cannot construct a call graph, identify orphaned code, or trace the impact of
changing a shared data structure.

Third, **change impact assessment is purely manual**. When a developer
modifies three files, the agent needs to know which other files depend on the
changed symbols and should be re-verified. Without a dependency graph, the
agent must either guess (and risk missing important regressions) or read every
file in the project (and waste unbounded tokens).

DeepMap bridges this gap. It gives CLI-based agents the same kind of structural
awareness that IDE-based tools have, without requiring an IDE, an MCP server,
a running LSP process, or any external infrastructure. It is a single library
compiled into the DeepSeek-TUI binary that produces a compact, high-signal
"map" of any project in a few seconds.

## What DeepMap Provides in DeepSeek-TUI

DeepMap exposes its capabilities through two channels: **TUI tools** that the
AI agent can call proactively during a session, and **CLI commands** that the
developer can invoke directly.

### TUI Tools (model-visible, the AI agent calls them during conversation)

These tools are registered in the DeepSeek-TUI tool registry and advertised to
the model through the system prompt. The model decides when to invoke them
based on the task context -- for example, calling `deepmap_overview` at the
start of a new project to understand the codebase layout, or calling
`deepmap_call_chain` when tracing how a bug propagates through the system.

| Tool | Description |
|------|-------------|
| `deepmap_overview` | Full project map report: entry points, hot files ranked by PageRank scan statistics, recommended reading order, module summary, and key symbols grouped by file. This is the primary entry point for agents that need to understand an unfamiliar codebase. |
| `deepmap_call_chain` | Trace both callers and callees for any named symbol up to a configurable depth. Returns the chain as a structured depth-grouped list sorted by PageRank score, which helps the agent focus on the most important nodes first. Useful for debugging, refactoring scoping, and impact analysis. |
| `deepmap_file_detail` | List every symbol defined in a single file with its signature, kind, visibility, line range, and PageRank score. The agent can use this to understand a specific file's API surface without reading the entire file. |
| `deepmap_query` | Topic-based code search that combines keyword matching, identifier splitting (camelCase, snake_case, kebab-case), file-role classification, and IDF-like keyword weighting. Returns ranked file matches, related test files, and highlighted key symbols. More precise than raw grep because it understands code structure. |

All TUI tools are **ReadOnly** and **Sandboxable**, meaning they never modify
the filesystem and can run inside a sandboxed environment. They use the same
**Auto approval** safety model as the existing `project_map` tool -- no user
approval prompt is needed because the tools cannot cause side effects.

### CLI Commands (developer-facing, invoked from the shell)

These commands let the developer use DeepMap directly without going through the
AI agent. They are especially useful for CI/CD pipelines, pre-commit hooks, and
ad-hoc exploration.

| Command | Purpose |
|---------|---------|
| `deepseek deepmap overview` | Generate and print a full project map to stdout. Equivalent to the `deepmap_overview` tool. |
| `deepseek deepmap call-chain --symbol <name>` | Trace the call chain for a specific symbol by name. Supports optional `--direction` (callers, callees, or both) and `--depth` arguments. |
| `deepseek deepmap file-detail --file <path>` | Inspect a single file's symbol table. Shows all symbols with their kinds, visibilities, line ranges, and PageRank scores in a formatted table. |
| `deepseek deepmap query --keywords <words>` | Topic search across the entire codebase. Accepts natural language or code terms, returns ranked results with related tests. |
| `deepseek deepmap impact --files <a,b>` | Pre-edit impact analysis: given a comma-separated list of changed file paths, lists all files that depend on them (directly and transitively) and provides per-file metrics. |
| `deepseek deepmap diff-risk` | Full diff risk assessment: combines impact analysis with keyword-based risk classification (auth, db, config patterns score higher) and verification suggestions including related test files and recommended test commands. |

### Session Cache

DeepMap uses a two-level caching strategy to make repeated calls fast:

1. **In-memory session cache**: The first `deepmap_overview` call in a session
   triggers a full scan, which typically takes 20-40 seconds for a
   medium-sized project (5,000-20,000 files). The resulting `RepoGraph`
   (symbols, edges, PageRank scores) is stored in a `LazyLock<Mutex<HashMap>>`
   keyed by the canonical project root path. All subsequent tool calls in the
   same session -- `deepmap_call_chain`, `deepmap_file_detail`,
   `deepmap_query`, etc. -- reuse this in-memory cache and return in under
   10 milliseconds.

2. **On-disk persistent cache**: The graph is serialized to
   `~/.cache/deepmap/{project_name}_{path_hash}/symbol_cache.json` with a
   schema version guard. On the next session, if the cache exists and the
   schema version matches, the scan is skipped entirely and the graph is loaded
   from disk. The cache entry includes a SHA-256 fingerprint of file paths,
   sizes, and modification times so that changes to the working tree
   automatically invalidate the cache. The write is atomic (write to temp file,
   rename over final path) with automatic backup of the previous cache.

This design means that an agent working in a project for the first time pays
the scan cost once, and subsequent sessions are nearly instant.

## Public API Surface

DeepMap exposes a focused public API through the `deepmap` crate. The following types and methods form the primary interface for both TUI tool integration and CLI command implementation.

### `RepoMapEngine` -- Main Entry Point

The central orchestrator that owns the scan pipeline, the graph data, and the analysis queries.

| Method | Description |
|--------|-------------|
| `new(project_root)` | Create a new engine instance for the given project root path |
| `get_or_scan(project_root, max_files, max_scan_secs)` | Constructor with session cache; returns cached engine if the project was already scanned in this process, otherwise runs a full scan |
| `scan(max_files, max_scan_secs)` | Execute the full three-phase pipeline: file traversal, tree-sitter parsing, and PageRank ranking |
| `query_symbol(name)` | Case-insensitive substring search over symbol names, returns ranked matches sorted by PageRank descending |
| `call_chain(symbol_id, direction, max_depth)` | BFS traversal of the call graph; supports caller, callee, and bidirectional walks |
| `hotspots(limit)` | Files ranked by `symbol_count * average_PageRank`, identifying the most maintenance-critical files |
| `entry_points()` | Heuristic detection of well-known entry-point files by filename stem or path pattern |
| `suggested_reading_order(limit)` | Files scored for onboarding priority, excluding test and noise files |
| `module_summary(limit)` | Symbols grouped by top-level directory, sorted by total PageRank descending |
| `is_scanned()` | Returns `true` if a scan has been completed |
| `scan_summary_lines()` | Returns formatted summary lines describing scan duration, file counts, and symbol counts |

### `TreeSitterAdapter` -- Multi-Language Parser

Manages per-language tree-sitter parsers and compiled S-expression queries for 8 languages.

| Method | Description |
|--------|-------------|
| `new()` | Initialize all 8 language parsers (Rust, Python, JavaScript, TypeScript, Go, HTML, CSS, JSON) |
| `parse(content, lang)` | Parse source content with the appropriate language grammar |
| `symbols()` | Extract all function, class, import, and call captures from parsed trees |
| `imports()` | Extract import declarations with source module and imported names |
| `calls()` | Extract call expressions with callee name and location |
| `import_bindings()` (JS/TS only) | Structured import records for ES module and CommonJS imports |
| `exports()` (JS/TS only) | Structured export records for ES module and CommonJS exports |

### `ImportResolver` -- Import Resolution

Resolves import statements to target files and builds the dependency edges between symbols.

| Method | Description |
|--------|-------------|
| `new(project_root, graph)` | Create a resolver with project root and reference to the partially-built graph |
| `resolve_import_targets(source_file, import_path)` | Resolve an import path to candidate target files using relative resolution, tsconfig aliases, and stem-based lookup |

### Key Data Types

- **`Symbol`** -- Carries `id` (`file:name:line` composite key), `name`, `kind` (function, class, struct, method, arrow_function, etc.), `file_path`, `line_range`, `column`, `visibility`, `docstring`, `signature`, and `pagerank_score`.
- **`Edge`** -- A directed edge between two symbol IDs with a `weight` field (0.50 for calls, 0.35 for imports).
- **`RepoGraph`** -- The complete graph container: `symbols`, `outgoing`, `incoming`, `file_symbols`, `file_imports`, `file_calls`, `file_import_bindings`, `file_exports`.
- **`ScanStats`** -- Scan metadata: `files_found`, `files_parsed`, `files_failed`, `filtered_large_files`, `total_symbols`, `total_edges`, `scan_duration_ms`, `timeout_triggered`, `failed_files`.

### Renderer Functions

Six Markdown report generators in the `renderer` module:

- `render_overview_report()` -- Full project map with entry points, hotspots, reading order, module summary, and key symbols
- `render_call_chain_report()` -- Caller/callee chain for a symbol with depth grouping
- `render_file_detail_report()` -- Per-file symbol table as a Markdown table
- `render_query_report()` -- Topic search results with related test discovery
- `render_impact_report()` -- Pre-edit impact analysis with transitive dependency listing
- `render_diff_risk_report()` -- Full diff risk assessment with risk classification and verification suggestions

## Failure Model

DeepMap is designed to be resilient in the face of real-world codebases that are incomplete, misconfigured, or contain pathological files. The following contract describes how every failure mode is handled.

| Scenario | Behavior |
|----------|----------|
| **No parser available for language** | File is silently skipped; scanning continues with other files. No error is raised. |
| **File too large (>10 MiB)** | File is skipped and counted in `filtered_large_files`. |
| **Excessive AST nesting (>1,000 depth)** | File is skipped as a safety guard against pathological or generated files. |
| **Parse error / malformed source** | File is skipped; path and error message are recorded in `failed_files` (up to 5 entries). |
| **Scan timeout (default 300s)** | Partial results are returned; `timeout_triggered` is set to `true` in `ScanStats`. |
| **Empty project / no supported files** | Returns an empty `RepoGraph`; all reports remain valid but minimal. |
| **Session cache miss** | A full scan runs automatically; results are cached for subsequent calls. |
| **Symbol not found (query-symbol / call-chain)** | Returns empty results. This is normal behaviour, not an error. |
| **Git operations unavailable (impact / diff-risk)** | Reports "not a git repository" clearly in the output; no crash. |
| **LSP server not installed** | `lsp doctor` reports the server as skipped; `--with-lsp` on other commands degrades gracefully with a clear message. |

In all cases, DeepMap never panics or crashes due to per-file processing failures. Errors are collected, logged, and surfaced through the `ScanStats` metadata or the report output, allowing the calling code to decide how to handle them.

## Architecture

DeepMap's architecture follows a clean three-phase pipeline with a shared
data model, designed so that each phase can be tested, optimized, and replaced
independently.

### Phase 1: File Traversal and Filtering

The entry point is `RepoMapEngine::scan()` in `engine.rs`. File discovery uses
the `ignore` crate (the library behind `ripgrep`) to walk the project tree
respecting `.gitignore` rules, filter entries against `SKIP_DIR_NAMES`
(35 common noise directories such as `node_modules`, `.git`, `target`, `venv`,
`dist`, `build`, `vendor`, etc.) and `SKIP_FILE_NAMES` (lock files, shrinkwrap
files), and keep only files with recognized extensions.

The recognized extension-to-language mapping in `types.rs::ext_to_lang()`
covers 17 extensions across 9 language families: Python (`.py`, `.pyi`),
JavaScript (`.js`, `.jsx`, `.mjs`, `.cjs`), TypeScript (`.ts`, `.tsx`, `.mts`,
`.cts`), Go (`.go`), Rust (`.rs`), Java (`.java`), Kotlin (`.kt`, `.kts`),
Swift (`.swift`), C/C++ (`.cpp`, `.cc`, `.cxx`, `.hpp`, `.h`), C# (`.cs`),
PHP (`.php`), Ruby (`.rb`), HTML (`.html`, `.htm`), CSS (`.css`), and JSON
(`.json`). Files larger than 512 KB (configurable via the `DEEPMAP_MAX_FILE_BYTES`
environment variable) are skipped. Excessively nested ASTs (depth > 1,000) are
skipped as a safety guard against pathological files.

The walker respects a `max_files` limit and a `max_scan_secs` timeout (hard
cap at 300 seconds), so even very large monorepos cannot hang the agent
indefinitely.

### Phase 2: Tree-Sitter Parsing and Symbol Extraction

Each candidate file is parsed with the appropriate tree-sitter parser. DeepMap
bundles 8 tree-sitter grammars compiled as native Rust libraries:

- `tree-sitter-rust` (0.24)
- `tree-sitter-python` (0.23)
- `tree-sitter-javascript` (0.23)
- `tree-sitter-typescript` (0.23, both TypeScript and TSX)
- `tree-sitter-go` (0.23)
- `tree-sitter-html` (0.23)
- `tree-sitter-css` (0.23)
- `tree-sitter-json` (0.23)

The `TreeSitterAdapter` (in `parser.rs`) manages per-language parsers and
compiled S-expression queries. The queries are defined in `queries.rs` and
cover four query types per language:

- **function**: Function declarations, method definitions, arrow functions,
  lambda expressions, and variable declarations whose right-hand side is a
  function expression.
- **class**: Class declarations, struct definitions, enum definitions, trait
  definitions, interface declarations, and impl blocks.
- **import**: Import statements, use declarations, require calls, and
  module-path references. The Rust import extractor has special handling for
  `scoped_use_list` and nested `scoped_identifier` paths, concatenating path
  and name segments (e.g., `std::collections::HashMap`).
- **call**: Call expressions, method calls, member function calls, and
  scoped/qualified calls.

Each query produces `RawCapture` entries with name, kind, byte range, and line
range. For JavaScript and TypeScript, three additional extraction passes run:

1. **Import bindings**: Structured records of every `import` statement (ES
   modules) and `require()` call (CommonJS), capturing the local name, the
   imported name, the source module, and the import kind (default, named,
   namespace, CJS default, CJS destructured). This is critical for call-chain
   resolution because it lets the engine map `foo()` in code back to where
   `foo` was actually defined, across re-exports.

2. **Export bindings**: Structured records of every `export` statement (ES
   modules) and `module.exports` / `exports.xxx` assignments (CommonJS),
   capturing the exported name, the source name, the re-export module, and the
   export kind. This enables the engine to resolve `import { X } from 'Y'`
   chains through intermediate modules.

3. **Extra symbol passes**: Three passes (`extract_object_literal_methods`,
   `extract_anonymous_symbols`, `extract_exported_function_expressions`) catch
   symbols that standard queries miss: method definitions inside object
   literals, anonymous functions assigned to variables (e.g., `const foo =
   function() {}`), and default-exported function/class expressions.

The extracted data is stored in a `RepoGraph` (defined in `types.rs`):

```
RepoGraph {
  symbols:          HashMap<symbol_id, Symbol>,
  outgoing:         HashMap<symbol_id, Vec<Edge>>,
  incoming:         HashMap<symbol_id, Vec<Edge>>,
  file_symbols:     HashMap<file_path, Vec<symbol_id>>,
  file_imports:     HashMap<file_path, Vec<import_string>>,
  file_calls:       HashMap<file_path, Vec<(call_name, line, kind)>>,
  file_import_bindings: HashMap<file_path, Vec<JsImportBinding>>,
  file_exports:     HashMap<file_path, Vec<JsExportBinding>>,
}
```

A `Symbol` carries its id (a composite of `file:name:line`), name, kind
(function, class, struct, method, arrow_function, etc.), file path, line range,
column, visibility, docstring, signature, and PageRank score.

### Phase 2b: Import Resolution and Edge Construction

After parsing all files, `ImportResolver` (in `resolver.rs`) builds indices:

- **file_map**: filename stem -> candidate file paths (e.g., `"helper"` ->
  `["src/utils/helper.ts", "tests/helper_test.ts"]`).
- **name_index**: symbol name -> symbol IDs (e.g., `"parse"` -> all parse
  functions across the project).
- **known_paths**: a `HashSet` of every known file path for O(1) existence
  checks.

The resolver also discovers `tsconfig.json` and `jsconfig.json` files in the
project root and immediate subdirectories, parsing them (with JSONC comment
and trailing-comma stripping) to extract `compilerOptions.baseUrl` and
`compilerOptions.paths` alias rules.

When building edges, the engine processes two kinds of relationships:

1. **Call edges** (weight: 0.50): For every call expression found during
   parsing, the resolver attempts to map it to a target symbol. It first
   consults the local-name map (built from JS/TS import bindings) to resolve
   renames and re-exports, then looks up the resolved name in the global
   `name_index`. The calling symbol is identified by finding the enclosing
   symbol whose line range contains the call line.

2. **Import edges** (weight: 0.35): For every import statement, the resolver
   tries to find target files using a three-step strategy: first try relative
   path resolution (for imports starting with `.`), then try tsconfig alias
   pattern matching, and finally fall back to stem-based lookup in the
   `file_map`. For each resolved target file, import edges are created from
   every symbol in the source file to every symbol in the target file.

A deduplication set (`HashSet<(source_id, target_id)>`) prevents redundant
edges. Constants control the edge weights, with calls weighted higher than
imports because calls represent tighter coupling.

### Phase 3: PageRank Computation and Analysis

With the graph fully built, a standard power-iteration PageRank algorithm
(in `ranking.rs::GraphAnalyzer::calculate_pagerank`) computes structural
importance for every symbol:

1. Initialise all nodes with equal probability `1/N`.
2. For each iteration: distribute each node's current probability to its
   outgoing neighbours proportionally to edge weights. Dangling nodes (no
   outgoing edges) contribute to a uniform teleport term. Apply damping factor
   (default: 0.85, matching the classic PageRank paper).
3. Terminate when the maximum per-node delta falls below `1e-6`, or after
   50 iterations at most.
4. Normalise scores so the sum equals 1.0.

The damping factor and iteration count are chosen to balance precision against
computation time. Project-scale graphs typically converge in 20-30 iterations.

After PageRank, the `GraphAnalyzer` provides these analytical queries:

- **query_symbol**: Case-insensitive substring search over symbol names,
  filtered to exclude low-signal kinds (CSS selectors, JSON keys, HTML
  elements), sorted by PageRank descending.
- **call_chain**: BFS traversal of the call graph, depth-limited (default:
  unlimited, hard capped at 10,000 nodes in the BFS queue and 1,000 results),
  supporting caller, callee, or bidirectional walks.
- **hotspots**: Files ranked by `symbol_count * average_PageRank`, identifying
  high-density, high-importance files that are likely to be the most
  maintenance-critical.
- **entry_points**: Heuristic detection of well-known entry-point files by
  filename stem (`main`, `app`, `index`, `server`, `run`, `setup`, `cli`,
  `__main__`) or path pattern (`/src/main.tsx`, `/lib.rs`).
- **file_analysis**: Per-file metrics including symbol count, outgoing edges,
  incoming edges, and average PageRank.
- **module_summary**: Symbols grouped by top-level directory, sorted by total
  PageRank descending, for a high-level view of which modules carry the most
  structural weight.
- **suggested_reading_order**: Files scored by
  `average_PR * ln(symbol_count) * entry_boost(2.0 if entry_point)`, excluding
  test and noise files. This is the list an agent should read first when
  onboarding to a project.
- **summary_symbols**: For the top-N files by reading order, the top-M symbols
  sorted by composite importance (`incoming_calls * 3 + outgoing_calls * 2 +
  kind_weight`, where functions and methods score 5, classes and structs score
  4, modules score 3, variables score 2, and everything else scores 1).

### Report Rendering

All analysis results are rendered into Markdown by the `renderer.rs` module.
Six report types are available, each corresponding to a TUI tool or CLI
command:

1. **Overview report** (`render_overview_report`): Combines scan statistics,
   entry points, recommended reading order (top 20), module summary (top 20),
   hot spots (top 10), and key symbols (top 30 files, 5 symbols each) into a
   single comprehensive document. Truncated to a caller-specified character
   limit with word-boundary-aware truncation.

2. **Call chain report** (`render_call_chain_report`): Queries the best-matching
   symbol, then walks callers and callees to the specified depth. Reports the
   symbol's kind, file, and PageRank, followed by flat lists of callers and
   callees sorted by PageRank.

3. **File detail report** (`render_file_detail_report`): Lists all symbols in
   a file as a Markdown table with line number, kind, name, visibility,
   PageRank score, and signature. Sorted by line number.

4. **Query report** (`render_query_report`): Combines topic scoring, related
   test discovery, and key symbol highlighting into a single response.
   Uses `topic_score` from the `topic.rs` module, which applies identifier
   splitting, file-role classification, noise penalties, test-weight
   adjustments, and IDF-like keyword weighting.

5. **Impact report** (`render_impact_report`): For each changed file, lists
   direct dependents (files that import it) and per-file metrics. Aggregates a
   summary of total transitively affected files.

6. **Diff risk report** (`render_diff_risk_report`): Combines impact analysis
   with risk assessment. The `assess_risk` heuristic scores changed files by
   keyword patterns: `auth`/`login`/`token` (+3 each), `db`/`sql` (+3 each),
   `config` (+2). Scores map to risk levels: 0 = low, 1-2 = medium, 3-5 =
   high, 6+ = critical. The report also suggests verification commands and
   related test files.

### Topic Search Engine

The `topic.rs` module implements a lightweight code-search engine specifically
designed for AI agent consumption. It does not use embeddings or vector search;
instead it relies on a set of well-tuned heuristic strategies:

- **Identifier splitting**: Splits camelCase, PascalCase, snake_case, and
  kebab-case identifiers into their constituent tokens. For example,
  `getUserPermissions` becomes `["get", "user", "permissions"]`.
- **File-role classification**: Classifies each file as `test`, `frontend-ui`,
  `frontend-state`, `backend`, or `config` based on path patterns, so agents
  can filter by file role.
- **Weighted topic scoring**: Combines path score (30%), symbol-name score
  (25%), and symbol-kind/docstring score (15%) with noise penalties (5%
  reduction for generated/cache files) and test-weight adjustments (45%
  reduction for test files).
- **IDF-like keyword weighting**: Tokens that appear in many files receive
  lower weight; rare tokens receive higher weight. This is computed as
  `ln(N/df) + 0.5` where N is the total file count and df is the document
  frequency.
- **Fuzzy symbol suggestions**: Levenshtein distance <= 3 for typo-tolerant
  symbol lookup, returning up to `max_results` candidates sorted by distance
  then PageRank.
- **Related-test discovery**: Three strategies applied in order: same-directory
  test files (confidence 0.9), name-convention matching (confidence 0.75), and
  import-reference matching (confidence 0.6).

## Relationship to repomap

DeepMap and repomap share the same design philosophy but serve different
purposes and operate at different levels of maturity.

[repomap](https://github.com/gjczone/repomap) is the upstream research project,
written in Python and licensed under MIT. It is the more feature-complete of
the two, supporting 15 languages (versus DeepMap's 8), offering LSP
integration for real-time symbol resolution, implementing a full
"edit -> verify" workflow that can automatically check whether an AI edit
achieved its stated goal, and providing incremental scan support for very large
monorepos where a full rescan is impractical. Repomap exposes a Python API and
a CLI, and its design prioritises extensibility and experimentation -- new
language support, new query strategies, and new analytical passes are added to
repomap first.

DeepMap is repomap's engine, rewritten in Rust and integrated natively into
DeepSeek-TUI. It is not a port of every feature; it is a focused implementation
of the core scanning, ranking, and reporting pipeline that matters most for
TUI agent workflows. By shipping as a compiled library inside the DeepSeek-TUI
binary, DeepMap eliminates the overhead of subprocess calls, JSON
serialization, and Python runtime dependency management. The trade-off is that
extending DeepMap requires a Rust compilation step and is more involved than
extending repomap.

The intended workflow is bidirectional:

- When new query strategies or language grammars prove useful in repomap, they
  are ported to DeepMap for production use in DeepSeek-TUI.
- When DeepMap's Rust engine discovers edge cases or performance bottlenecks
  that are easier to prototype in Python, the experiments happen in repomap.

Both projects are actively maintained and evolve in tandem.

## Language Support

DeepMap currently bundles tree-sitter parsers for 8 languages:

| Language | Parsers | Coverage notes |
|----------|---------|----------------|
| Rust | `tree-sitter-rust` | Functions, structs, enums, traits, impl blocks, type aliases, modules, use declarations (scoped and glob), call expressions |
| Python | `tree-sitter-python` | Functions, decorated functions, classes, decorated classes, methods, lambdas, imports (absolute, relative, aliased), call expressions |
| JavaScript | `tree-sitter-javascript` | Functions, arrow functions, classes, methods, ES module imports/exports, CommonJS require, call expressions, object literal methods, anonymous functions |
| TypeScript | `tree-sitter-typescript` (TypeScript + TSX) | Same as JavaScript plus TSX support for React/JSX syntax |
| Go | `tree-sitter-go` | Functions, methods, structs, interfaces, imports (interpreted string literals), call expressions |
| HTML | `tree-sitter-html` | Tag names as symbols |
| CSS | `tree-sitter-css` | Class selectors, ID selectors, tag names as symbols |
| JSON | `tree-sitter-json` | Key-value pairs as symbols |

The `ext_to_lang` function in `types.rs` maps 17 file extensions to these 8
parsers plus 7 additional languages (Java, Kotlin, Swift, C/C++, C#, PHP,
Ruby) that are recognised for file discovery but do not yet have tree-sitter
queries -- they will pass through filtering and contribute to file counts but
produce no symbols or edges. These are candidates for future parser integration.

### About tree-sitter-tsx

TypeScript parser support includes the separate TSX grammar
(`tree_sitter_typescript::LANGUAGE_TSX`) in addition to the standard TypeScript
grammar (`tree_sitter_typescript::LANGUAGE_TYPESCRIPT`). This is important for
React/JSX projects because the TSX grammar understands JSX syntax nodes that
the plain TypeScript grammar does not. Without TSX support, a `component.tsx`
file would fail to parse on any JSX expression, producing no symbols for the
entire file.

## Design Decisions

### Why PageRank and not something else?

PageRank was chosen for three reasons. First, it is well-understood and has
known convergence properties -- 50 iterations with a damping factor of 0.85
guarantees a stable ranking for any directed graph. Second, it naturally
handles the "importance flows through edges" intuition that maps well to
software dependencies: if a file is imported by many important files, it is
itself important. Third, the computation is cheap for project-scale graphs
(typically 5,000-50,000 nodes), completing in a few milliseconds after the
graph is built.

Alternatives considered include centrality measures (betweenness, closeness)
which are more expensive to compute, and machine learning approaches (node
embeddings, graph neural networks) which require training data and are
impractical for a general-purpose tool that must work on any codebase without
prior training.

### Why three-phase and not streaming?

The three-phase design (traverse -> parse -> rank) requires holding all
parsed data in memory before ranking can begin. A streaming approach that
ranked incrementally would use less memory but would sacrifice the global view
that PageRank requires -- a symbol's importance depends on the full graph
structure, not just local properties.

For a typical project, the `RepoGraph` uses 50-200 MB of memory, which is
acceptable for a development tool that runs ephemerally. The session cache
avoids paying this cost more than once per workspace per process lifetime.

### Why not use the system language server?

An LSP-based approach would give richer symbol information (type annotations,
documentation, diagnostics) and would not require bundling tree-sitter
grammars. However, LSP servers are language-specific, require installation
and configuration, and may not be available for every language in a polyglot
project. DeepMap's tree-sitter approach works offline, with zero configuration,
and supports every bundled language uniformly. The trade-off is shallower
semantic understanding -- DeepMap knows what is defined and what calls what,
but it does not know types.

## License

MIT -- same as repomap and DeepSeek-TUI. Both repomap and deepmap are
permissively licensed and free to use in any project, commercial or otherwise.
Contributions are welcome through the standard GitHub fork-and-PR workflow.

## Future Directions

The following capabilities are under consideration for future releases:

- **Incremental scanning**: Re-scan only changed files instead of the entire
  project, using the mtime cache and SHA-256 fingerprint as change detectors.
  The mtime cache infrastructure is already in place in the engine; what is
  needed is a "diff and merge" pass that updates the existing graph rather
  than rebuilding from scratch.

- **LSP integration as a plugin**: An optional plugin that connects to one or
  more LSP servers for richer symbol information (type annotations, hover
  documentation, diagnostics) when available, falling back to tree-sitter when
  they are not.

- **Additional language parsers**: Java, Kotlin, Swift, C/C++, C#, PHP, and
  Ruby parsers would bring the language count to 15, matching repomap's
  coverage.

- **Post-edit verification**: Given a set of edited files and the original
  dependency graph, automatically check whether all affected files still
  satisfy their structural invariants (exports match imports, call targets
  still exist, etc.).

- **Time-series ranking**: Track PageRank scores across git history to
  identify modules that are growing in structural importance, flagging them
  as candidates for refactoring before they become maintenance bottlenecks.
