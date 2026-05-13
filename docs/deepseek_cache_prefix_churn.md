# DeepSeek Cache Prefix Churn Analysis

Date: 2026-05-13

## Problem statement

DeepSeek-TUI already includes several cache-stability improvements, but Agent mode
still allows the model-visible tool catalog to grow during a normal edit/test
loop. For DeepSeek, that matters because prompt cache reuse depends on repeated
request prefixes matching complete cache prefix units. When common tools are
initially deferred and only become active after first use, later requests carry a
different `tools` array than earlier ones, which reduces prompt-cache reuse.

This document explains why that happens, why users are noticing it, and why a
targeted Agent-mode preload of the core coding toolset is a good first fix.

## Official DeepSeek cache behavior

DeepSeek's official KV cache guide says:

- Prompt caching is enabled by default.
- The response includes `prompt_cache_hit_tokens` and
  `prompt_cache_miss_tokens`.
- Later requests only hit the cache when they reuse a repeated prefix.
- Because DeepSeek uses Sliding Window Attention, matching happens on complete
  cache prefix units, so a later request must fully match a cached unit to hit.

Source:
- <https://api-docs.deepseek.com/zh-cn/guides/kv_cache>

Practical consequence: if the request body changes near the front of the prompt,
even for something structurally "small" like the visible tool definitions, the
cache can miss from that point onward.

## Official Claude Code comparison

Claude Code's official "How Claude Code works" page documents several context
stability tactics:

- MCP tool definitions are deferred by default and loaded on demand via tool
  search, so only tool names consume context until Claude uses a specific tool.
- Skills load on demand.
- Subagents run with their own fresh context and return summaries.

Source:
- <https://code.claude.com/docs/en/how-claude-code-works>

That does not mean Claude Code and DeepSeek-TUI have identical architectures.
It does show a useful principle: keep the stable prefix stable, and push
late-bound detail to the tail or into separate contexts.

## Repo evidence that the issue is real

Open issues in the upstream repo show repeated user reports that DeepSeek cache
hit rate is worse than in Claude Code-like tools:

- Issue #1120: "There still seems to be some problems with cache hits缓存命中方面似乎还是有些问题"
  - <https://github.com/Hmbown/DeepSeek-TUI/issues/1120>
- Issue #1177: "输入缓存命中率太低了"
  - <https://github.com/Hmbown/DeepSeek-TUI/issues/1177>
- Issue #1253: "Feature: DeepSeek cache-aware prompt diagnostics and wire payload optimization"
  - <https://github.com/Hmbown/DeepSeek-TUI/issues/1253>

These issues are corroborating evidence, not the primary proof. The primary
proof is the combination of DeepSeek's documented cache behavior and the current
tool-catalog implementation.

## What the code already gets right

The current engine already contains several good cache-aware decisions:

- `crates/tui/src/core/engine/tool_catalog.rs`
  - `build_model_tool_catalog(...)` sorts native tools and MCP tools
    deterministically.
  - Built-ins stay ahead of MCP tools so MCP changes do not shift built-in tool
    positions.
  - `active_tool_list_from_catalog(...)` appends deferred-but-activated tools to
    the tail, which avoids reindexing always-loaded tools mid-session.
- Existing tests already guard these properties in
  `crates/tui/src/core/engine/tests.rs`.

So the cache problem is not "the tools array is random." The remaining issue is
that the active tool list still grows during normal coding sessions.

## The remaining root cause

Agent mode currently keeps these tools loaded by default:

- file/navigation/search basics such as `read_file`, `list_dir`, `grep_files`,
  `file_search`
- diagnostics/planning/task helpers
- shell execution tools such as `exec_shell`

But many tools that the project's own prompts describe as normal primary coding
tools are still deferred:

- File editing: `write_file`, `edit_file`, `apply_patch`
- Git inspection: `git_status`, `git_diff`, `git_show`, `git_log`, `git_blame`
- Verification: `run_tests`

Relevant prompt references:

- `crates/tui/src/prompts/base.md`
- `crates/tui/src/prompts/base.txt`
- `crates/tui/src/prompts/subagent_output_format.md`

That mismatch creates avoidable prefix churn in a common Agent-mode flow:

1. Request 1 starts with the base always-loaded tool set.
2. The first edit activates `edit_file` or `apply_patch`.
3. A later verification step activates `run_tests`.
4. A later inspection step activates `git_diff` or `git_status`.

Each activation changes the model-visible `tools` array for subsequent requests.
DeepSeek's cache rules make this expensive because later requests no longer
replay the same full cached prefix unit.

## Why this PR scope is the right first fix

The narrow fix is to preload the core coding toolset in Agent mode, while still
keeping less common tools deferred.

Why this is a good first step:

- It directly addresses a documented DeepSeek cache requirement: prefix reuse.
- It matches the repo's own prompts, which already treat these tools as
  first-class tools for normal coding work.
- It preserves the existing architecture and the existing tail-append behavior
  for truly rare tools.
- It is small, testable, and low-risk compared with a bigger redesign of tool
  hydration or request serialization.

## Expected effect

This change should not eliminate every cache miss. Other factors still matter:

- changing conversation content
- dynamic MCP availability
- compaction boundaries
- tool results and reasoning content

But it should reduce avoidable misses caused by the tool catalog changing during
the standard read -> edit -> diff/test loop that most coding tasks follow.

## Controlled API validation

I also ran a controlled reproduction directly against DeepSeek's official Chat
Completions API using `deepseek-v4-flash` and the documented cache metrics
fields from the KV cache guide.

Test design:

- Stable case:
  - Request 1: long repeated prefix + full coding toolset
  - Request 2: identical long repeated prefix + identical full coding toolset
- Churn case:
  - Request 1: identical long repeated prefix + smaller base toolset
  - Request 2: identical long repeated prefix + expanded coding toolset

The only intentional difference between the two cases was whether the visible
tool catalog grew between the first and second request.

Observed results:

- Stable repeat:
  - `prompt_cache_hit_tokens = 4096`
  - `prompt_cache_miss_tokens = 18`
  - hit ratio ~= `99.56%`
- Churn repeat:
  - `prompt_cache_hit_tokens = 3328`
  - `prompt_cache_miss_tokens = 791`
  - hit ratio ~= `80.80%`

Delta on the repeated request:

- `768` fewer cache-hit tokens
- `773` more cache-miss tokens

This is not a full end-to-end TUI benchmark, but it is strong mechanism-level
evidence that expanding the model-visible tool definitions between requests
materially reduces DeepSeek cache reuse.

## Non-goals for this PR

This PR does not attempt to solve:

- MCP tool churn beyond the current partitioning and deferral logic
- history compaction strategy
- tool-result payload size
- request-body diagnostics for cache hit/miss accounting

Those are valid follow-up areas, but they are intentionally out of scope for a
first targeted fix.
