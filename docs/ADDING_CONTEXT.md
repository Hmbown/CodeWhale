# Adding Context

What the model sees is not a mystery in CodeWhale. This page names every input
surface, the exact order the system prompt is assembled in, and what happens
when the context window fills up. `/context` opens a live inspector for the
current session.

## Input surfaces

| Surface | How | What enters context |
|:---|:---|:---|
| **`@` file mentions** | Type `@` in the composer; an autocomplete popup walks the workspace, ranked by how recently/frequently you've used each file. Tab completes. | The file **content is inlined** after your message in a `Local context from @mentions` block, each file wrapped in a `<file mention="@path">` tag. Directories inject a listing; missing paths inject an explicit `<missing-file>` marker. |
| **Paste** | Bracketed paste, with a paste-burst fallback for terminals that lack it (`paste_burst_detection`, default on). | Pasted text lands in the composer as-is. Pasting an **image** from the clipboard saves a PNG into the workspace and inserts an `[Attached image: …]` reference. |
| **`/attach <path>`** | Aliases `/image`, `/media`. | Attaches image/video bytes to your next message. This is the surface that actually ships media bytes; an `@`-mentioned image is *not* inlined — it injects a hint telling the model to request `/attach`. |
| **Vision tools** | `image_analyze`, `image_ocr` — model-invoked. | `image_analyze` sends a workspace image to a separately configured OpenAI-compatible vision model. `image_ocr` extracts text locally (macOS Vision framework or `tesseract`) — including image-only/scanned PDFs — and returns it inline. |
| **Terminal output** | Shell tool results. | Enter context as tool results, capped at 30 KB per result: over the cap, the first 22 KB and last 8 KB are kept with an explicit truncation marker. Oversized outputs are retrievable in full via the tool-result retrieval path. |
| **Web** | `web_search`, `fetch_url`, `web.run` — model-invoked, network-policy gated. | `web_search` returns ranked snippets with `ref_id`s (DuckDuckGo default, Bing fallback; Tavily, Bocha, Metaso, SearXNG, Baidu and others selectable). `fetch_url` GETs a known URL, HTML stripped to text. `web.run` drives a browser for JS-rendered pages. |
| **MCP resources** | `list_mcp_resources`, `list_mcp_resource_templates`, `mcp_read_resource` — model-invoked. | Resource content enters context as a tool result when the model reads it; nothing is auto-injected. The read-only resource/prompt helpers can run without approval prompts in suggestive modes. See [MCP.md](MCP.md). |
| **Documents** | `pandoc_convert` (requires `pandoc` installed). | Converts docx/epub/odt and ~30 other formats to text the model can read. Scanned documents go through `image_ocr` instead. |
| **Memory** | Opt-in: `[memory] enabled = true` or `DEEPSEEK_MEMORY=on`. | `~/.codewhale/memory.md` is injected as a `<user_memory>` block every turn — declarative facts only, lowest authority tier. See [MEMORY.md](MEMORY.md). |
| **Project instructions** | Automatic. | `AGENTS.md` is canonical, with `.claude/instructions.md`, `CLAUDE.md`, `.codewhale/instructions.md`, and legacy `.deepseek/instructions.md` as fallbacks; parent directories are walked, then user-global paths (`~/.codewhale/AGENTS.md`, `~/.agents/AGENTS.md`). `WHALE.md` is ignored with a warning. |
| **Skills** | Discovered from `<repo>/.codewhale/skills/` and global skill roots. | Only the **catalog** (name + description per skill) is in the prompt; bodies load on demand when the model calls `load_skill` or reads the `SKILL.md`. |

`@`-mention limits, exact: 8 mentions per message, 128 KiB per file (larger
files are truncated and marked), 80 entries per directory listing. Deeper
workspace walks: `/config set mention_walk_depth 0`.

## What the model sees: prompt assembly order

The system prompt is assembled in a fixed order (enforced by tests in
`crates/tui/src/prompts.rs`), most-static content first so provider prompt
caches stay warm:

1. **Constitution + mode prompt** — the bundled law compiled into the binary,
   plus the active mode's composer.
2. **Project instructions** — the `AGENTS.md` block.
3. **User-global constitution** — your ratified standing rules
   (`~/.codewhale/constitution.json`).
4. **Project context pack** — generated workspace orientation.
5. **Skills catalog** — one line per skill.
6. **Context-management and compaction guidance.**
7. — *volatile-content boundary* (everything below may change per session) —
8. **Environment** — platform, shell, working directory, locale.
9. **Configured `instructions = [...]` files** from config.
10. **User memory** (`<user_memory>`, if enabled).
11. **Current session goal** (`<session_goal>`, if `/goal` is set).
12. **Previous-session handoff** (`.codewhale/handoff.md`, if present).
13. **Authority recap** — the final tier reminder before your messages.

After that come the conversation messages: your input, the `@`-mention context
block appended to it, attachments, and tool results. Per-turn working-set
metadata rides in a `<turn_meta>` block on the latest user message rather than
the system prompt.

When instructions conflict, they resolve by the nested-constitution ranking —
bundled law, then user constitution, then repo constitution, then project
instructions, with memory and handoffs below all of those; your current request
and live tool evidence control the active turn. [CONSTITUTION.md](CONSTITUTION.md)
is the full treatment.

## Context budget and compaction

The context window is **route-aware**: when the resolver knows the active
route's concrete limits, those are used; otherwise the provider capability
table for the model applies. You can override per provider with
`context_window` under `[providers.<name>]` — this feeds the context notes,
compaction thresholds, context-pressure checks, and request output caps
(see [CONFIGURATION.md](CONFIGURATION.md)).

- The footer shows `active ctx N%`; `/tokens` prints the full breakdown
  (active context vs. window, last API input/output, cache hit/miss).
- `/compact` summarizes the conversation to reclaim the window.
- `auto_compact` is on by default for models with known context windows and
  triggers before send at `auto_compact_threshold_percent` (default `80`).
  The prompt also tells the model to *suggest* compaction above ~60% usage.

## Not supported (so you don't go looking)

- **Drag-and-drop** files into the TUI — not supported; use `@` or `/attach`.
- **Automatic URL ingestion** — pasting a URL just pastes text; the model must
  call `fetch_url`/`web.run` to read it.
- **Inline `$<skill>` invocation** — design-only, targeted at 0.9.0
  ([SKILL_INVOCATION_DESIGN.md](SKILL_INVOCATION_DESIGN.md)). Today the model
  loads skills from the catalog, or you point it at one explicitly.

Related: [SESSIONS.md](SESSIONS.md) for what persists across restarts,
[TOOL_SURFACE.md](TOOL_SURFACE.md) for the full tool catalog.
