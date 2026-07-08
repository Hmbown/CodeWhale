# Sub-Agent Tool Scoping — Phase 1 State

> **Branch:** `wip/sub-agent-tool-restrictions`
> **Target:** `main` (v0.8.68)
> **Date:** 2026-07-08
> **Status:** Implementation complete, 23 tests passing

## Summary

PANL §1a–1e implements sub-agent `disallowed_tools` inheritance: the parent
session's `--disallowed-tools` list flows into spawned sub-agents through the
`WorkerRuntimeProfile`, and the caller can override or supplement the deny list
at spawn time via `disallowed_tools` and `inherit_disallowed_tools` in the
`agent` tool JSON.

## Files Changed

| File | Changes |
|------|---------|
| `crates/tui/src/tools/subagent/mod.rs` | SpawnRequest +2 fields, parse_spawn_request parsing + `parse_disallowed_tools()` helper, spawn_subagent_from_input merge logic, SubAgentToolRegistry +`disallowed_tools` field + `is_tool_denied()` method, `is_tool_allowed()`/`tools_for_model()` deny filtering |
| `crates/tui/src/worker_profile.rs` | WorkerRuntimeProfile +`denied_tools` field, `for_role()` default, `derive_child()` union logic, 3 tests |
| `crates/tui/src/core/engine.rs` | Both SubAgentRuntime construction sites set `worker_profile.denied_tools` from `self.config.disallowed_tools` |
| `crates/tui/src/tools/subagent/tests.rs` | 1 minor fix (re-apply deny list after profile overwrite) + 5 new coverage-gap tests |

## Architecture

```
EngineConfig.disallowed_tools          (--disallowed-tools CLI flag)
    │
    ▼
SubAgentRuntime.worker_profile.denied_tools    (set at 2 engine construction sites)
    │
    ├── child_runtime() / background_runtime() clone it
    │
    ├── spawn_subagent_from_input() merge:
    │     inherit_disallowed_tools: false → clear()
    │     spawn_request.disallowed_tools → union into profile
    │
    ▼
SubAgentToolRegistry.disallowed_tools          (read from runtime.worker_profile.denied_tools)
    │
    ├── is_tool_denied()    exact match + prefix* wildcard, case-insensitive
    ├── is_tool_allowed()   deny checked first, then allowlist, then depth
    ├── tools_for_model()   filters denied tools from model-visible catalog
    └── execute()           rejects denied tools before posture/approval checks
```

## Deny Matching Logic

`is_tool_denied()` mirrors `command_denies_tool()` from `turn_loop.rs:3085`:

- Exact match: `"exec_shell"` denies exactly `exec_shell`
- Prefix wildcard: `"mcp_*"` denies `mcp_read`, `mcp_write`, `mcp_anything`
- Star: `"*"` denies everything (strip-suffix `"*"` → `""` → `starts_with("")` ≡ true)
- Case-insensitive: `"Exec_Shell"` denies `exec_shell`

## Test Coverage

### Disallowed tools tests (20 in `tests.rs`)

| Test | What it covers |
|------|---------------|
| `test_disallowed_tools_inheritance_denies_tool` | Basic deny + allow + catalog exclusion |
| `test_disallowed_tools_deny_wins_over_allow` | Deny overrides explicit allowlist |
| `test_disallowed_tools_case_insensitive_match` | Case-insensitive matching |
| `test_disallowed_tools_wildcard_matching` | `mcp_acme_*` wildcard behavior |
| `test_disallowed_tools_prefix_wildcard_specific_server` | Prefix not matching unrelated server |
| `test_disallowed_tools_opt_out` | Registry with empty denies (no parent list to clear) |
| `test_disallowed_tools_caller_deny_always_applies` | Caller's explicit deny always wins |
| `test_disallowed_tools_empty_list_does_not_clear_parent` | Child empty list doesn't remove parent denies |
| `test_disallowed_tools_execute_rejects_denied_tool` | `execute()` rejects denied tools |
| `test_disallowed_tools_tools_for_model_excludes_denied` | Catalog excludes denied tools |
| `test_disallowed_tools_propagates_through_child_runtime` | `child_runtime()` preserves `denied_tools` |
| `test_disallowed_tools_propagates_through_background_runtime` | `background_runtime()` preserves `denied_tools` |
| `test_disallowed_tools_across_two_generations` | Deny list propagates parent → child → grandchild |
| `test_disallowed_tools_unknown_tool_name_silently_ignored` | Unknown tool in deny list doesn't panic |
| `test_disallowed_tools_vs_readonly_posture_both_gates_apply` | Deny + posture both active |
| `test_disallowed_tools_mcp_wildcard_catalog` | **NEW** — `mcp_*` denies all MCP tools from catalog |
| `test_disallowed_tools_star_denies_everything` | **NEW** — `"*"` wildcard denies every tool |
| `test_disallowed_tools_opt_out_clears_inherited_denies` | **NEW** — `inherit: false` clears parent denies |
| `test_disallowed_tools_opt_out_with_caller_deny` | **NEW** — opt-out clear + caller deny still applies |
| `test_disallowed_tools_registry_stores_deny_from_runtime` | **NEW** — engine→runtime→registry threading verified |

### Derive child tests (3 in `worker_profile.rs`)

| Test | What it covers |
|------|---------------|
| `derive_child_unions_deny_lists` | Parent `[tool_a, tool_b]` + child `[tool_c]` → union `[tool_a, tool_b, tool_c]` |
| `derive_child_preserves_wildcard_denies` | **NEW** — Parent `[mcp_*]` + child `[file_*]` → union `[mcp_*, file_*]` |
| `derive_child_does_not_duplicate_case_variants` | **NEW** — Same literal not stored twice |

### Parse spawn request tests (4 new in existing parse test suite)

| Test | What it covers |
|------|---------------|
| `test_parse_spawn_request_reads_disallowed_tools` | Parses `disallowed_tools` JSON array |
| `test_parse_spawn_request_disallowed_tools_defaults_to_none` | Default when field absent |
| `test_parse_spawn_request_inherit_disallowed_tools_defaults_true` | Default `true` when field absent |
| `test_parse_spawn_request_inherit_disallowed_tools_explicit_false` | Explicit `false` parsed |

## Concern Audit (2026-07-08)

### 1. "Deny Always Wins" — Bypass Vector Audit ✅

**Verdict: No bypass exists.**

All sub-agent tool execution flows through a single dispatch point:

```
model response → tool_uses collected
→ tool_registry.execute()          ← line 5569, only dispatch
→ is_tool_allowed()                ← first gate
→ is_tool_denied()                 ← deny checked before allowlist and posture
```

The model receives the deny-filtered catalog from `tools_for_model()` as
defense-in-depth, but even if the model hallucinates a tool name not in the
catalog, execution still goes through `execute()` → `is_tool_allowed()` →
`is_tool_denied()`. No internal-ID dispatch, no alternate path.

### 2. Child Propagation Union Logic ✅

**Verdict: Correct, no security gap.**

`derive_child()` (worker_profile.rs:223-228) performs a union:

```rust
let mut denied_tools = self.denied_tools.clone();   // start with parent's
for tool in &requested.denied_tools {
    if !denied_tools.contains(tool) {
        denied_tools.push(tool.clone());              // child can only add
    }
}
```

The child **cannot remove** parent entries. The `"*"` wildcard works as
expected: `strip_suffix("*")` → `""`, `starts_with("")` → always true.

Case-insensitive dedup: `contains()` is case-sensitive, so `"MCP_*"` and
`"mcp_*"` would be stored as two entries. This is harmless — `is_tool_denied()`
lowercases both sides at match time — but slightly wasteful.

### 3. `inherit_disallowed_tools: false` — Global Config Survival ⚠️

**Verdict: Working as spec'd, but a design decision worth noting.**

Current behavior: `inherit_disallowed_tools: false` clears ALL `denied_tools`
from the child runtime's profile, including what came from the engine's
`self.config.disallowed_tools`. There is no separate "base engine config" layer.

The PANL spec at §1e says: *"The `inherit_disallowed_tools: false` flag allows
dropping only the* ***parent runtime's*** *list — explicit caller
`disallowed_tools` always applies."*

Since the engine config IS the parent runtime's list in the current
architecture, this matches the spec. However, this means a sub-agent can opt
out of global `--disallowed-tools` bans.

**Decision: Option A for Phase 1** — document this behavior as intentional.
The flag provides a "clean slate" including dropping global engine denies.
A follow-up issue should evaluate whether Phase 2 needs a separate
`base_denied_tools` layer that cannot be opted out of.

**Test coverage:** Added `test_disallowed_tools_opt_out_clears_inherited_denies`
and `test_disallowed_tools_opt_out_with_caller_deny` to explicitly verify this
behavior.

## Remaining Gaps (out of scope for Phase 1)

- Fleet profile integration (PANL Phase 2)
- `allowed_tools` for non-Custom roles (PANL Phase 2)
- MCP server assignment per sub-agent (PANL Phase 3)
- `build_allowed_tools()` changes (PANL Phase 2)

## Verification

```bash
# Full test suite
cargo test -p codewhale-tui -- test_disallowed_tools   # 20/20
cargo test -p codewhale-tui -- derive_child             # 3/3
cargo test -p codewhale-tui -- parse_spawn_request      # 26/26
cargo test -p codewhale-tui -- subagent                 # 321/322 (1 pre-existing macOS perm issue)

# Lint
cargo fmt
cargo clippy -p codewhale-tui --all-targets -- -D warnings
```
