# v0.7.6 Legacy Rust Audit

Status date: 2026-04-29

This audit is deliberately non-destructive. No compatibility code is removed in v0.7.6 unless tests prove public CLI, saved-session, tool-schema, and documented command paths no longer depend on it.

## Summary

| Surface | Owner module | Current consumer | Reference check | Compatibility reason | Current warning | Recommended action |
|---|---|---|---|---|---|---|
| Legacy MCP sync API (`McpServerInput`, `list`, `add`, `remove`, `call_tool`, `load_legacy`) | `crates/tui/src/mcp.rs` | Not wired into current `/mcp` command path; retained behind `#[allow(dead_code)]` | Direct Rust references and current MCP command path inspected; saved/config JSON compatibility still needs a dedicated smoke | Preserves old JSON shape including `mcpServers` alias and sync call helpers while the async MCP manager is the active path | Code TODO only | Gate behind an explicit legacy module or remove after CLI/runtime parity tests prove no caller uses it. Tracked by #218. |
| Legacy prompt constants/functions (`AGENT_PROMPT`, `YOLO_PROMPT`, `PLAN_PROMPT`, `base_system_prompt`, `normal_system_prompt`, etc.) | `crates/tui/src/prompts.rs` | Tests and older callers that still import prompt constants directly | Direct Rust references remain; public-crate and older harness imports are not proven absent | Layered prompt API replaced monolithic prompts, but older call sites may still compile against constants | None | Keep for v0.7.6; add deprecation annotations only after internal callers are migrated. Tracked by #219. |
| `/compact` slash command positioning | `crates/tui/src/commands/mod.rs` | Public slash-command registry and help overlay | Public command registry/docs path still references it | Users may still run `/compact` manually when they want an immediate replacement-style summary | Description is intentionally explicit about manual compaction | Keep as a manual compatibility command; do not remove until context/token issues are resolved. |
| `todo_*` compatibility tools | `crates/tui/src/tools/todo.rs` | Tool registry/model calls that still use `todo_add`, `todo_update`, `todo_list`, `todo_write` | Tool registry compatibility and saved tool-call risk remain | `checklist_*` is canonical, but old tool names may appear in saved prompts, traces, or model priors | Metadata marks `compat_alias: true`; descriptions say compatibility alias | Add explicit deprecation metadata with target version, then remove only after tool-schema migration evidence. Tracked by #220. |
| Deprecated sub-agent alias tools (`spawn_agent`, `send_input`, delegate aliases) | `crates/tui/src/tools/subagent/mod.rs` | Tool registry and model/tool-call compatibility | Tool registry compatibility and saved tool-call risk remain | Canonical names are `agent_spawn`, `agent_send_input`, etc.; alias names preserve older tool-call compatibility | `_deprecation` metadata and tracing warn; removal target is `v0.8.0` | Keep through v0.7.x; removal already has metadata. Tracked by #221. |
| Legacy root/provider TOML `api_key` compatibility | `crates/tui/src/config.rs`, `crates/config/src/lib.rs` | Config resolver; users with existing `api_key` in config files | Public config loading and docs still mention migration behavior | Keyring migration is preferred, but breaking existing configs would block startup/auth | Tracing warnings point to `deepseek auth set` / `deepseek auth migrate` | Keep; warnings are user-actionable. Removal should wait for a migration command and release-note window. |
| Model alias canonicalization (`deepseek-chat`, `deepseek-reasoner`, older V3/R1 aliases) | `crates/tui/src/config.rs`, `crates/config/src/lib.rs` | Config/env/model picker normalization | Public docs and existing configs may still use aliases | Preserves old documented DeepSeek aliases and maps them to `deepseek-v4-flash` | Silent alias by design | Keep; removing aliases would break configs without meaningful benefit. |
| Deprecated palette constants and aliases | `crates/tui/src/palette.rs`, `crates/tui/tests/palette_audit.rs` | Existing call sites plus audit tests | Palette audit enforces the remaining allowlist | Semantic aliases are preferred, but old constants exist to prevent broad style churn | Palette audit blocks direct deprecated uses outside allowlist | Keep aliases; continue moving call sites to semantic roles opportunistically. |

## Follow-Up Removal Candidates

These are not safe to remove in v0.7.6:

1. #218 Legacy MCP sync API: requires a call-graph check and explicit CLI/runtime parity tests for `/mcp`, `deepseek mcp`, and MCP server validation flows.
2. #219 Legacy prompt constants/functions: requires proving no public crate or older test harness imports them.
3. #220 `todo_*` tool aliases: requires deprecation metadata and a saved-trace/tool-schema migration window.
4. #221 Deprecated sub-agent alias tools: removal target is already encoded as `v0.8.0`, but the actual removal should be tracked and tested separately.

## Verification Checklist

Before removing any compatibility surface:

1. Search direct Rust references with `rg`.
2. Search docs and README command examples.
3. Run workspace tests with all features.
4. Run a saved-session/tool-call compatibility smoke if the surface affects tool schemas or persisted history.
5. Keep a release-note entry and, for user-visible config/tool changes, a migration hint for at least one minor release.
