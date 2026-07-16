# CodeWhale 0.8.68 Handoff — Finish the Landing

> **Status (2026-07-15): Historical handoff superseded by v0.9.0, not
> completion proof.** v0.8.68 was never tagged or publicly published. Paths,
> branch counts, checkmarks, and
> verification claims below describe the checkout that produced this handoff;
> do not use them as evidence for the current candidate. Use the current
> [v0.9.0 release-candidate ledger](docs/releases/v0.9.0-release-candidate.md)
> instead. Some listed work is now present in the candidate and some was
> superseded — notably, #4132 keeps `todo_*`/`checklist_*` as hidden replay
> aliases, so do not execute the old instruction to remove them. The
> DO-NOT-DELETE table remains a standing guardrail. The companion
> `opportunities.md` was never committed; its link is dangling by design.
>
> ~~**Companion document:** [`opportunities.md`](../../opportunities.md)~~ (not
> in the repository) — this file was the *what to do next*; the catalog behind
> it stayed private.

## Branch & Location
- **Worktree:** `/Users/hunter/Desktop/Harnesses/CW/.cw-worktrees/v0867-pr4047`
- **Branch:** `work/v0.9.0-cutover` (7 ahead of origin/main, 0 behind)
- **PR:** #4099 open to main
- **Remote:** `Hmbown/CodeWhale`

## Current State (verified)
- `cargo check --workspace` ✅ PASSES
- `cargo clippy --workspace --all-features -- -D warnings` ✅ PASSES
- 16 files changed in working tree (unstaged, not committed yet)

## ⛔ DO NOT DELETE — Verified Active

These six modules were flagged as "dead code" by the original scout audit.
**They are all actively imported and used.** Previous agents deleted them and
caused 19+ compile errors. Do not touch them under any circumstances.

| Module | Active consumers |
|--------|-----------------|
| `tui/src/memory.rs` | `prompts.rs`, `engine.rs`, `context_report.rs`, `ui.rs` |
| `tui/src/context_budget.rs` | `core/engine/context.rs:9`, `engine/tests.rs` |
| `tui/src/model_registry.rs` | `tui/model_picker.rs:737`, `model_profile.rs:149` |
| `tui/src/prompt_zones.rs` | `core/session.rs:8`, `core/engine/turn_loop.rs:10` |
| `tui/src/tools/remember.rs` | `tools/registry.rs:890`, `tools/mod.rs:41` |
| `config/src/route/` (entire dir) | `catalog.rs:38`, `models_dev.rs:20`, `pricing.rs:25` |

---

## What's Actually Done (verified in working tree)

| Item | Status | Evidence |
|------|--------|----------|
| B1.1 — mimalloc for CLI | ✅ Done | `crates/cli/Cargo.toml:38` + `crates/cli/src/main.rs:1-2` |
| B1.6 — pdf-extract feature-gated | ✅ Done | `Cargo.toml:17` (`pdf = ["dep:pdf-extract"]`), `web_run.rs` `#[cfg(feature = "pdf")]` |
| B9.1 — `to_vec` in app-server | ✅ Done | `app-server/src/lib.rs:293,309,325,336,1155` |
| B9.2 — compact JSON tool output | ✅ Done | `tools/src/lib.rs`, `tool_execution.rs`, `registry.rs` — no `to_string_pretty` in these paths |
| B8.2 — spawn_blocking in tasks.rs | ✅ Done | `tasks.rs:769,848,935` |
| file.rs perf tweaks | ✅ Done | `tools/file.rs` modified |
| Palette migration (partial) | ✅ 3 files | `logging.rs`, `remote_setup/mod.rs`, `palette_audit.rs` — done |
| Double-enter tests | ✅ Written | `tui/app/tests.rs` — `enter_with_double_tap()`, `last_enter_instant` |

## What the PREVIOUS Handoff Claimed Was Done — But ISN'T

| Claim | Reality |
|-------|---------|
| B1.4 — reqwest multipart removed | ❌ **Still in Cargo.toml**: `features = [..., "multipart", ...]` |
| B1.5 — reqwest brotli removed | ❌ **Still in Cargo.toml**: `features = [..., "brotli"]` |
| allowed_tools() deleted | ❌ **Still present** at `subagent/mod.rs:448` |
| smoothness.md deleted | ❌ **Still present** (29,613 bytes) |

---

## Remaining Work (in priority order, compile after each)

### 1. Remove reqwest `multipart` + `brotli` features
- **File:** `crates/tui/Cargo.toml` — remove `"multipart"` and `"brotli"` from
  the `reqwest` features array. Zero uses of `reqwest::multipart` exist. `gzip`
  alone is sufficient.
- **Risk:** None. Verify `cargo check` after.

### 2. Palette brand migration (`DEEPSEEK_*` → `WHALE_*`)
- See the 🎨 section in `opportunities.md` for full details.
- **⚠️ main.rs is NOT part of this.** Its 98 `DEEPSEEK_*` refs are **environment
  variable names** (`DEEPSEEK_API_KEY`, `DEEPSEEK_BASE_URL`, etc.). Renaming
  them breaks every user's config. Leave them alone.
- The actual migration targets ~216 `palette::DEEPSEEK_*` color references
  across 40+ files in `crates/tui/src/`.
- **Missing prerequisite:** No `WHALE_INFO`/`WHALE_BG`/`WHALE_PANEL`/`WHALE_ERROR`
  `Color` constants exist (only `_RGB` tuples). Add them to `tokens.rs` first.
- **Plan:** Add Color constants → `sed` replace `_RGB` variants first, then bare
  names → add `#[deprecated]` to now-unused aliases → compile.

### 3. Delete `allowed_tools()` method
- **File:** `crates/tui/src/tools/subagent/mod.rs:448` — delete the method and
  its match arms (deprecated since v0.6.6). Keep the struct fields at lines 1261
  and 1363.

### 4. Delete `smoothness.md`
- **File:** `smoothness.md` (29,613 bytes) — unreferenced doc file.

### 5. Remove `tools_file` config field
- `config.rs:1881` — remove `pub tools_file: Option<String>` field
- `config.rs:5513` — remove merge line
- `config.example.toml:162` — remove commented `# tools_file` line

### 6. Remove `removed_messages` dead field
- `compaction.rs:918` — remove field with `#[allow(dead_code)]` + `TODO(v0.8.71)`
  comment. Dead in production.

### 7. Remove `todo_*` alias scaffolding (v0.9.0 gate)
- `tools/todo.rs:177-178` — remove `TODO_ALIAS_FIRST_DEPRECATED_VERSION`,
  `TODO_ALIAS_REMOVAL_VERSION` constants and `is_compat_alias()` (line 182)
- `tools/registry.rs` — remove `todo_*` registrations (keep `checklist_*`)

### 8. B8.1 — spawn_blocking for blocking cmd.output()
- `tools/git_history.rs:485` — wrap `cmd.output()` in `tokio::task::spawn_blocking`
- `tools/review.rs:645,672` — same

### 9. B5.2 — clippy await_holding_lock audit
- Run `cargo clippy --workspace --all-features -- -W clippy::await_holding_lock -W clippy::await_holding_refcell_ref`
- Fix any warnings (guards held across `.await`).

### 10. Double-Enter for Steer (feature implementation)
Tests exist in `tui/app/tests.rs` but the feature code does not:
1. Add `last_enter_instant: Option<Instant>` field to `App` struct in `tui/app.rs`
2. Add `enter_with_double_tap(&mut self) -> Option<SubmitDisposition>` method
3. In `decide_submit_disposition()`, change busy-waiting arm to return `Queue`
   instead of `Steer`
4. Wire up in `tui/ui.rs` Enter handler
5. Update composer hint text in `tui/widgets/mod.rs`

---

## Final Steps
1. Commit all changes with descriptive message
2. Push to `work/v0.9.0-cutover`
3. Verify PR #4099 CI passes (fix any failures)
4. Run `cargo test --workspace --locked` to verify tests pass
5. Build release binary: `cargo build --release -p codewhale-tui`

## Guidelines
- Work file-by-file, compile after each change
- NEVER delete a file without first grepping for ALL imports of that module
- For dead code removal: prefer adding `#[allow(dead_code)]` over deleting files
  with active imports
- Commit in logical chunks, not one giant commit
