# CODEMAP v0.8.25 Dead Code Analysis

## Scope

Target files:

1. `crates/tui/src/cycle_manager.rs`
2. `crates/tui/src/seam_manager.rs`
3. `crates/tui/src/core/coherence.rs`
4. `crates/tui/src/core/capacity.rs`
5. `crates/tui/src/core/capacity_memory.rs`
6. `crates/tui/src/core/engine/capacity_flow.rs`
7. `crates/tui/src/commands/cycle.rs`
8. `crates/tui/src/tools/recall_archive.rs`

## Pre-flight baseline

- **Branch**: `work/v0.8.25...origin/work/v0.8.25`.
- **Working tree before codemap write**: pre-existing untracked files under `.claude/`, `pass/`, `scripts/maintenance/`, and `smoke.txt`; no tracked code edits observed before this report.
- **Baseline test command**: `cargo test --workspace --all-features`.
- **Baseline result**: failed with one observed test failure: `mcp::tests::mcp_connection_supports_streamable_http_event_stream_responses` panicked on `Connection reset by peer` against localhost. Observed summary: `2518 passed; 1 failed; 2 ignored`.
- **Interpretation**: baseline is not green. The failing test is in MCP streamable HTTP, outside the target cycle/seam/coherence/capacity files. Treat subsequent claims as source-trace findings, not as a green-CI proof.

## Executive summary

| File | Verdict | Phase-1 classification | Short answer |
|---|---|---|---|
| `cycle_manager.rs` | LIVE | LIVE, STATE-MUTATING, DESIGN-LOAD-BEARING; PRACTICAL LOAD-BEARING UNPROVEN | Hard-cycle restart is wired into the engine, UI, session state, and archive/recall path. It is not ordinary-turn behavior because the default trigger is near the 1M-token wall. |
| `seam_manager.rs` | PARTIALLY LIVE | GHOST / LIVE BUT REPLACEABLE | The engine constructs and calls it, but `[context].enabled` defaults false. When opted in, it appends `<archived_context>` blocks and can supply Flash cycle briefings. |
| `core/coherence.rs` | LIVE | LIVE BUT REPLACEABLE | Pure reducer feeding footer/runtime state from compaction and capacity events. Small, visible, and actively referenced. |
| `core/capacity.rs` | PARTIALLY LIVE | GHOST / LIVE BUT REPLACEABLE | Controller is constructed and checkpoint methods are called every turn, but default config disables observations and interventions. Opt-in path is real and destructive. |
| `core/capacity_memory.rs` | PARTIALLY LIVE | GHOST support module | Only writes after capacity interventions, which are opt-in. However startup/resume rehydrate reads the latest record unconditionally. |
| `core/engine/capacity_flow.rs` | PARTIALLY LIVE | GHOST / LIVE BUT REPLACEABLE | The turn loop calls all checkpoints, event helpers, and rehydration. With default capacity disabled, most intervention paths no-op except compaction/coherence event helpers. |
| `commands/cycle.rs` | LIVE | LIVE BUT REPLACEABLE | `/cycles`, `/cycle <n>`, and `/recall <query>` are registered built-in slash commands and produce user-visible output. |
| `tools/recall_archive.rs` | PARTIALLY LIVE | LIVE BUT STRANDED FROM PARENT TOOL SURFACE | `/recall` uses it directly and sub-agent full surface registers it. The parent Agent/Plan registry does not appear to expose `recall_archive` as a model-callable tool. |

## Cross-cutting call graph

### Ordinary TUI turn

1. `tui/ui.rs::build_engine_config` forwards `app.cycle_config()` and `CapacityControllerConfig::from_app_config(config)` into `EngineConfig`.
2. `Engine::new` constructs:
   - `CapacityController::new(config.capacity.clone())`.
   - `seam_manager: Option<SeamManager>` when a `DeepSeekClient` exists; its `enabled` flag comes from `[context].enabled.unwrap_or(false)`.
3. `Engine::handle_send_message` increments `turn_counter`, calls `capacity_controller.mark_turn_start`, then calls `handle_deepseek_turn`.
4. `handle_deepseek_turn` calls:
   - auto/manual compaction checks;
   - `run_capacity_pre_request_checkpoint`;
   - hard context-overflow recovery;
   - `layered_context_checkpoint`;
   - streaming/model/tool execution;
   - `run_capacity_post_tool_checkpoint`;
   - `run_capacity_error_escalation_checkpoint`.
5. After a completed turn, `handle_send_message` calls `maybe_advance_cycle`.

### UI event flow

- `EngineEvent::CycleAdvanced` updates `app.cycle_count`, pushes `CycleBriefing`, inserts a system separator into history, and sets a status message.
- `EngineEvent::CoherenceState` updates `app.coherence_state`; `footer_coherence_spans` renders only active intervention states, suppressing `Healthy` and `GettingCrowded`.
- `EngineEvent::CapacityDecision` is telemetry-only in the TUI.
- `EngineEvent::CapacityIntervention` and `CapacityMemoryPersistFailed` become status messages.
- Compaction events set `app.is_compacting`, status messages, and also drive coherence transitions through `capacity_flow` helpers.

### Runtime API event flow

`runtime_threads.rs` consumes the same engine events and persists/streams them as runtime records:

- `CycleAdvanced` becomes a runtime `context_cycle` item.
- `CoherenceState` updates `ThreadRecord.coherence_state` and emits `coherence.state`.
- `CapacityDecision`, `CapacityIntervention`, and `CapacityMemoryPersistFailed` are persisted as runtime items.
- Compaction events become context-compaction lifecycle items.

## Strings and config surface

### User-visible strings and commands

- `/cycles`, `/cycle <n>`, and `/recall <query>` are registered in `commands/mod.rs` and dispatched to `commands/cycle.rs`.
- Sidebar shows `cycles: N (active: N+1)` once `app.cycle_count > 0`.
- The TUI history renderer recognizes assistant `<archived_context>` blocks and renders them as `HistoryCell::ArchivedContext`.
- Footer status item `coherence` is in default `StatusItem::default_footer`, but visible spans are intentionally empty for `Healthy` and `GettingCrowded`.
- `docs/CONFIGURATION.md` documents a compact `coherence` chip, although current footer code only renders intervention states.
- `docs/CONFIGURATION.md`, `config.example.toml`, and `docs/capacity_controller.md` document `[capacity]` as opt-in.
- `docs/CONFIGURATION.md` and `config.example.toml` document `[context]` seam keys as opt-in.

### Config schema findings

- `[capacity]` is a real top-level config table: `Config.capacity: Option<CapacityConfig>`, parsed from TOML and environment-overridden via `DEEPSEEK_CAPACITY_*`.
- `[context]` is a real top-level config table: `Config.context: ContextConfig`, including `enabled`, `project_pack`, thresholds, `cycle_threshold`, `seam_model`, and `per_model`.
- `cycle_manager.rs` claims `[cycle.per_model]` in comments, but current `Config` does not expose a top-level `[cycle]` table. Active cycle config comes from `CycleConfig::default()` in `App::new`, direct CLI `main.rs`, and runtime-thread engine construction.
- `ContextConfig.per_model` exists but the observed `Engine::new` construction of `SeamConfig` reads only the top-level `[context]` threshold fields; no observed path applies per-model context threshold overrides.
- The hard cycle threshold used by `maybe_advance_cycle` comes from `EngineConfig.cycle`, not `[context].cycle_threshold`. TUI uses `app.cycle_config()`, which is initialized to `CycleConfig::default()`.

---

# File findings

## 1. `crates/tui/src/cycle_manager.rs`

### Entry points

- **Engine hard-cycle path**: `Engine::maybe_advance_cycle` calls `should_advance_cycle`, `produce_briefing`, `archive_cycle`, `StructuredState::capture`, and `build_seed_messages`.
- **Seam integration**: `maybe_advance_cycle` prefers `SeamManager::produce_flash_briefing` when a seam manager exists, falling back to `cycle_manager::produce_briefing`.
- **Recall path**: `tools/recall_archive.rs` uses `open_archive` to read JSONL archives written by `archive_cycle`.
- **UI command path**: `commands/cycle.rs` depends on `CycleBriefing` and `CycleConfig::threshold_for` through `App` state.

### UI surface

- **Direct**: no direct ratatui rendering in this file.
- **Indirect**:
  - `CycleBriefing` is sent via `Event::CycleAdvanced`.
  - TUI displays a cycle separator and status message.
  - Sidebar displays cycle count once nonzero.
  - `/cycles` and `/cycle <n>` display stored briefings.

### State mutation

- `archive_cycle` writes `~/.deepseek/sessions/<session_id>/cycles/<cycle_n>.jsonl`.
- `maybe_advance_cycle` uses this module to build seed messages, then replaces `self.session.messages`, increments `session.cycle_count`, updates `session.current_cycle_started`, pushes `session.cycle_briefings`, clears `session.compaction_summary_prompt`, refreshes system prompt, and emits `SessionUpdated`/`CycleAdvanced`.
- `StructuredState::capture` reads todo/plan/sub-agent/working-set state but does not mutate it.

### Runtime activation

- **Default active but rare**. `CycleConfig::default()` has `enabled = true` and threshold `768_000` tokens.
- The engine calls `maybe_advance_cycle` after every completed turn, but `should_advance_cycle` returns false until the active input estimate reaches the smaller of configured threshold and model-window-minus-response-headroom.
- In practice, this is a near-wall safety path for long sessions, not normal-turn behavior.

### Tests

- Unit tests cover default config, per-model override logic, trigger threshold behavior, in-flight guard, carry-forward extraction, briefing cap, archive write/open, and seed-message construction.
- Tests are mostly isolated unit tests, but they validate on-disk archive format consumed by `recall_archive`.
- No observed integration test forces a full live `maybe_advance_cycle` through the streaming engine.

### Strings/keys

- User-visible concepts: `Cycle State (Auto-Preserved)`, `<carry_forward>`, cycle archive JSONL, cycle handoff status strings in engine, `/cycle`, `/cycles`, `/recall` consumers.
- Comment claims `[cycle.per_model]` config.

### Config schema

- `CycleConfig` is serializable and has `per_model`, but current `Config` does not include a top-level cycle field.
- TUI/runtime/CLI construction uses `CycleConfig::default()` rather than parsed `[cycle]` config.
- `ContextConfig.cycle_threshold` is separate and feeds `SeamConfig`, not `CycleConfig`.

### Verdict

**LIVE**.

This is not dead code: it is wired into post-turn engine behavior, mutates live session state, writes archives used by recall, and has UI/runtime event surfaces. It is rare-path and partly misdocumented/config-stranded, but not safe to delete.

### Recommendation

- **Keep for now** unless the product decision is to remove hard-cycle restart entirely.
- Fix or remove the stale `[cycle.per_model]` documentation/comment, or add real `[cycle]` config parsing.
- Consider adding an integration test that lowers the cycle threshold and verifies `maybe_advance_cycle` swaps messages and emits `CycleAdvanced`.

## 2. `crates/tui/src/seam_manager.rs`

### Entry points

- `Engine::new` constructs `SeamManager` when a `DeepSeekClient` exists, using `[context]` config values.
- `Engine::layered_context_checkpoint` calls `seam_level_for`, `verbatim_window_start`, `collect_seam_texts`, `produce_soft_seam`, and `recompact`.
- `Engine::maybe_advance_cycle` calls `collect_seam_texts`, `produce_flash_briefing`, and `reset`.
- TUI history parsing consumes the `<archived_context>` blocks produced by this file.

### UI surface

- Produces assistant text blocks of the form `<archived_context ...>...</archived_context>`.
- `tui/history.rs` parses those blocks into `HistoryCell::ArchivedContext`, rendered as dim/italic archived context rows.
- Engine emits status messages while producing and completing seams.
- If used for cycle briefing, output indirectly appears in `/cycles` and `/cycle <n>` through `CycleBriefing`.

### State mutation

- Appends assistant messages to `self.session.messages` via `layered_context_checkpoint`.
- Tracks `SeamMetadata` in `active_seams` and clears it on hard-cycle reset.
- Reports cost through `cost_status::report` for seam/briefing calls.
- Does not remove original messages; design is append-only.

### Runtime activation

- **Default inactive**. `SeamConfig::default()` has `enabled = true`, but actual engine construction overrides this from `api_config.context.enabled.unwrap_or(false)`.
- `config.example.toml` and docs set `[context].enabled = false` by default.
- `layered_context_checkpoint` is called before each API request, but returns immediately when no seam manager exists, `enabled` is false, thresholds are not reached, or there is not enough history before the verbatim window.
- When `[context].enabled = true`, it is real runtime behavior.

### Tests

- Tests cover pure seam threshold ordering, lifetime-vs-active request token distinction, hard-cycle threshold constants, and verbatim window logic.
- Tests do not appear to run the full engine with `[context].enabled = true` and a mocked Flash response.

### Strings/keys

- User/model-visible XML: `<archived_context level="..." range="msg ..." tokens="...">`.
- Config keys documented under `[context]`: `enabled`, `verbatim_window_turns`, `l1_threshold`, `l2_threshold`, `l3_threshold`, `cycle_threshold`, `seam_model`.
- `ContextConfig.per_model` exists, but no observed wiring applies per-model seam thresholds in `Engine::new`.

### Config schema

- `[context]` is real and documented.
- `enabled` defaults false in app behavior.
- `seam_model` and thresholds are read by `Engine::new` into `SeamConfig`.
- `per_model` appears parsed but stranded from the observed `SeamConfig` construction.

### Verdict

**PARTIALLY LIVE**.

The manager is wired and functional when opted in, but default sessions do not produce seams. It is a ghost subsystem by default, not dead.

### Recommendation

- Do not delete without an explicit product decision to remove experimental seams.
- If keeping, wire or remove `[context].per_model` and add an engine-level opt-in test.
- If replacing, preserve the `<archived_context>` UI parser compatibility until saved sessions with seam blocks are considered disposable.

## 3. `crates/tui/src/core/coherence.rs`

### Entry points

- `capacity_flow.rs` imports `CoherenceState`, `CoherenceSignal`, and `next_coherence_state`.
- `emit_coherence_signal` calls `next_coherence_state` and emits `Event::CoherenceState`.
- Compaction events and capacity decision/intervention events generate coherence signals.
- TUI `App` stores `coherence_state`; runtime `ThreadRecord` persists it.

### UI surface

- `CoherenceState::label()` and `description()` provide user-facing labels/descriptions.
- TUI footer renders active intervention states via `footer_coherence_spans`:
  - `RefreshingContext`
  - `VerifyingRecentWork`
  - `ResettingPlan`
- `Healthy` and `GettingCrowded` are suppressed in the footer, though runtime state still stores them.
- Runtime API emits `coherence.state` events and persists thread coherence.

### State mutation

- This module is pure. It mutates no state itself.
- Engine stores the reducer output in `self.coherence_state` and emits events.
- TUI and runtime persistence then update app/thread state.

### Runtime activation

- Active in ordinary sessions through compaction events: manual compaction, auto compaction, and emergency context recovery all call compaction event helpers that emit coherence signals.
- Capacity-driven states are only active when the capacity controller produces snapshots/interventions; default capacity is disabled, so capacity-specific coherence transitions are mostly dormant.

### Tests

- Unit tests cover reducer transitions for capacity decisions/interventions and compaction start/complete/fail.
- Runtime-thread tests cover persistence of `CoherenceState` in thread detail.

### Strings/keys

- Labels: `healthy`, `getting crowded`, `refreshing context`, `verifying recent work`, `resetting plan`.
- Descriptions are user-facing and flow through `Event::CoherenceState`.
- Config/UI key: `StatusItem::Coherence` / `tui.status_items = ["coherence"]`.

### Config schema

- No direct config table in this module.
- UI visibility is controlled indirectly by `tui.status_items`; `coherence` is part of the default footer item list.

### Verdict

**LIVE**.

Small live reducer with runtime and UI surfaces. Capacity-specific branches are partially dormant by default, but compaction-driven coherence is live.

### Recommendation

- Keep.
- If simplifying, fold the reducer into event handling only after verifying runtime API/state compatibility.
- Align docs with the footer behavior: `Healthy`/`GettingCrowded` may exist in state but are not normally rendered as chips.

## 4. `crates/tui/src/core/capacity.rs`

### Entry points

- `CapacityControllerConfig::from_app_config` converts parsed `[capacity]` TOML to runtime config.
- `Engine::new` constructs `CapacityController::new(config.capacity.clone())`.
- `handle_send_message` calls `capacity_controller.mark_turn_start` each turn.
- `capacity_flow.rs` calls `observe_pre_turn`, `observe_post_tool`, `last_snapshot`, `decide`, `mark_intervention_applied`, and `mark_replay_failed`.

### UI surface

- No direct UI rendering in this file.
- Decisions/interventions emitted by `capacity_flow` become TUI status messages, coherence state changes, and runtime API items.
- `GuardrailAction::as_str()` and `RiskBand::as_str()` feed event payloads.

### State mutation

- Mutates controller runtime state: rolling slack/tool/ref windows, last snapshot, cooldown/intervention/replay counters.
- Does not mutate session messages directly; `capacity_flow` performs message/session mutations based on decisions.

### Runtime activation

- **Default inert**. `CapacityControllerConfig::default().enabled = false`.
- `observe_pre_turn`/`observe_post_tool` return `None` when disabled, making decisions `NoIntervention`.
- Engine still calls the checkpoints every turn, so it is wired but no-ops in default sessions.
- Opt-in via `[capacity].enabled = true` or `DEEPSEEK_CAPACITY_ENABLED` re-arms the policy.

### Tests

- Unit tests cover disabled behavior, opt-in behavior, risk policy decisions, cooldowns, model priors, and config defaults.
- Engine tests cover opt-in pre-request refresh, post-tool replay, error escalation, and disabled-by-default behavior preserving messages.
- Tests explicitly document why default disabled exists: active interventions can clear or rewrite the transcript.

### Strings/keys

- Guardrail action strings include `no_intervention`, `targeted_context_refresh`, `verify_with_tool_replay`, `verify_and_replan`.
- Risk strings include low/medium/high.
- Config keys documented and parsed under `[capacity]`; env overrides use `DEEPSEEK_CAPACITY_*`.

### Config schema

- Real and documented in `config.example.toml`, `docs/CONFIGURATION.md`, and `docs/capacity_controller.md`.
- Default is disabled in code, docs, and tests.

### Verdict

**PARTIALLY LIVE**.

The code is wired into every turn but intentionally inert by default. Opt-in path is real and load-bearing for users who enable it, but default product behavior treats it as an experimental guardrail.

### Recommendation

- Do not delete blindly; it has explicit opt-in docs/tests.
- If product direction is “capacity should stay off forever,” deprecate config first, then remove after a compatibility window.
- If keeping, consider isolating destructive behavior behind clearer UI warnings and add integration tests for event emission.

## 5. `crates/tui/src/core/capacity_memory.rs`

### Entry points

- `capacity_flow::persist_capacity_record` calls `append_capacity_record` after opt-in capacity interventions.
- `capacity_flow::rehydrate_latest_canonical_state` calls `load_last_k_capacity_records` on engine startup and resume.
- Engine calls `rehydrate_latest_canonical_state` in `Engine::new` and after loading a session.
- Tests call path-specific append/load helpers.

### UI surface

- No direct UI rendering.
- Failed writes emit `Event::CapacityMemoryPersistFailed` from `capacity_flow`, which becomes a TUI status message and runtime item.
- Successful writes only affect future system prompt rehydration through a `memory://<session>/<record>` pointer.

### State mutation

- Writes JSONL records to:
  - `DEEPSEEK_CAPACITY_MEMORY_DIR` when set;
  - otherwise `~/.deepseek/memory/<session_id>.jsonl`;
  - fallback `<cwd>/.deepseek/memory/<session_id>.jsonl`.
- Reads latest records and returns deserialized `CapacityMemoryRecord` values.
- Ignores malformed individual JSONL lines while reading.

### Runtime activation

- Writes are active only after capacity interventions, and those interventions are disabled by default.
- Reads are attempted on startup/resume regardless of whether capacity is currently enabled. If no records exist, no-op.
- Therefore default sessions without prior opt-in capacity records will never observe visible behavior.

### Tests

- Unit tests cover JSONL round trip, candidate fallback writes, and newest-candidate selection.
- Engine tests verify an opt-in replan intervention persists a record.

### Strings/keys

- Environment variable: `DEEPSEEK_CAPACITY_MEMORY_DIR`.
- Memory pointer string format: `memory://<session_id>/<record_id>` generated in `capacity_flow`.
- JSONL fields: `id`, `ts`, `turn_index`, `action_trigger`, `h_hat`, `c_hat`, `slack`, `risk_band`, `canonical_state`, `source_message_ids`, optional `replay_info`.

### Config schema

- No TOML config table directly in this module.
- Path override is environment-only and documented in `docs/capacity_controller.md`.

### Verdict

**PARTIALLY LIVE**.

Support code for an opt-in subsystem. It is not dead because engine startup/resume calls rehydration and opt-in interventions write records, but in default sessions it is usually dormant.

### Recommendation

- Keep if capacity controller remains.
- If capacity controller is removed, remove this module with the rehydration hook and docs together.
- If keeping, consider whether rehydration should check capacity enablement or whether historical capacity memory should intentionally survive after disabling.

## 6. `crates/tui/src/core/engine/capacity_flow.rs`

### Entry points

- `turn_loop.rs` calls:
  - `run_capacity_pre_request_checkpoint` before request budget checks;
  - `run_capacity_post_tool_checkpoint` after tool result handling;
  - `run_capacity_error_escalation_checkpoint` after error streak accounting.
- `engine.rs` manual/auto/emergency compaction paths call `emit_compaction_started`, `emit_compaction_completed`, and `emit_compaction_failed`.
- `Engine::new` and session-load paths call `rehydrate_latest_canonical_state`.

### UI surface

- Emits:
  - `CapacityDecision` telemetry;
  - `CapacityIntervention` status-driving events;
  - `CapacityMemoryPersistFailed` status-driving events;
  - `CoherenceState` events;
  - compaction lifecycle events.
- TUI renders capacity interventions as status messages and active coherence states in the footer.
- Runtime API persists these events as thread/item records.

### State mutation

- `apply_targeted_context_refresh` can run compaction, trim messages, persist canonical state, merge a canonical prompt into the system prompt, refresh system prompt, and mark intervention cooldown.
- `apply_verify_with_tool_replay` can replay read-only tools, append verification tool-result messages, persist canonical state, merge canonical prompt, refresh system prompt, and mark replay/intervention state.
- `apply_verify_and_replan` persists canonical state, clears `session.messages`, preserves latest user and verification messages, injects replan prompt, refreshes system prompt, and marks intervention.
- `rehydrate_latest_canonical_state` can merge the latest persisted canonical state into the system prompt on startup/resume.

### Runtime activation

- Checkpoint functions are called during ordinary turns.
- With default capacity disabled, observations return `None`, decisions no-op, and intervention methods do not run from capacity decisions.
- Compaction event helpers and coherence transitions are live even when capacity is disabled, because compaction paths call them.
- Rehydration is attempted on every engine construction/resume.

### Tests

- Engine tests cover opt-in pre-request refresh, opt-in post-tool replay, opt-in error replan, disabled-by-default no mutation, and controller disabled unchanged behavior.
- These tests exercise engine state mutation directly rather than only pure functions.

### Strings/keys

- Status strings include capacity refresh failure, verification replay notes, canonical prompt section names, replan instruction, and memory pointers.
- Uses `GuardrailAction` strings from `capacity.rs`.
- Emits user-visible statuses like `Capacity guardrail: context reset to canonical state; replanning step.`

### Config schema

- Behavior gated by `EngineConfig.capacity`, which comes from `[capacity]` via `CapacityControllerConfig::from_app_config`.
- Also uses `self.config.compaction` for targeted refresh and `self.config.capacity.profile_window` for observations.

### Verdict

**PARTIALLY LIVE**.

This is central wiring, not isolated dead code. However, its capacity-specific destructive branches are ghost behavior by default due to `[capacity].enabled = false`. The compaction/coherence helper half is live.

### Recommendation

- Do not delete without removing or rewriting the capacity controller contract.
- If simplifying default behavior, split live compaction/coherence helpers from opt-in capacity intervention logic so ghost code is easier to reason about.
- Add event-level tests for default compaction coherence if preserving the footer/runtime coherence contract.

## 7. `crates/tui/src/commands/cycle.rs`

### Entry points

- `commands/mod.rs` registers and dispatches:
  - `/cycles` -> `cycle::list_cycles`;
  - `/cycle` -> `cycle::show_cycle`;
  - `/recall` -> `cycle::recall_archive`.
- Command metadata exposes these commands in help/autocomplete.

### UI surface

- `/cycles` returns a human-readable list of cycle handoffs or a “No cycle boundaries” message.
- `/cycle <n>` returns a full briefing or validation errors.
- `/recall <query>` returns the JSON payload from `RecallArchiveTool` or an error message.
- All are user-invoked slash-command output.

### State mutation

- `list_cycles` and `show_cycle` are read-only.
- `recall_archive` is read-only against archive files.
- No App state mutation observed in these functions.

### Runtime activation

- Active whenever the user types the commands.
- `/cycles` and `/cycle` only become useful after a hard cycle fires; before then they still produce meaningful empty-state/error output.
- `/recall` depends on archived cycle files; if none exist, the tool reports no prior archives.

### Tests

- Unit tests cover empty list, nonexistent cycle, valid list/show rendering, and argument validation.
- Tests do not cover `/recall` wrapper directly, but `RecallArchiveTool` has its own tests.

### Strings/keys

- Command names: `/cycles`, `/cycle <n>`, `/recall <query>`.
- User-visible output includes “No cycle boundaries”, “Cycle handoffs”, and `recall_archive failed`.

### Config schema

- No direct config reads.
- Reads `app.cycle.threshold_for(&app.model)` for empty-state messaging, but `app.cycle` is currently initialized from default `CycleConfig`.

### Verdict

**LIVE**.

Registered slash commands with user-visible output. Utility depends on rare hard-cycle archives, but command dispatch is live.

### Recommendation

- Keep if hard-cycle archives or recall remain.
- If parent model should also get `recall_archive`, add it to `build_turn_tool_registry_builder`; otherwise document `/recall` as the primary parent-session surface.

## 8. `crates/tui/src/tools/recall_archive.rs`

### Entry points

- `/recall <query>` directly instantiates `RecallArchiveTool` and calls `execute`.
- `ToolRegistryBuilder::with_recall_archive_tool` registers it.
- `with_full_agent_surface` includes `with_recall_archive_tool`, which is used by sub-agent runtime registry construction.
- No observed call to `with_recall_archive_tool` in the parent `Engine::build_turn_tool_registry_builder`; parent Agent mode uses `with_agent_tools`, then review/user-input/parallel/RLM/FIM/web/shell/etc.

### UI surface

- Via `/recall`, output is returned to the user as JSON text.
- As a model-callable tool, returns JSON content with `query`, optional `cycle`, `max_results`, `archive_count`, and `hits`.
- Empty archives produce a human-readable no-archive message in the tool result content.

### State mutation

- Read-only. Lists archive JSONL files, opens them through `cycle_manager::open_archive`, tokenizes/scans messages, and returns BM25-ranked excerpts.
- Does not write state.

### Runtime activation

- User-invoked `/recall` is active in the parent TUI.
- Model-callable parent Agent/Plan surface appears stranded: `with_recall_archive_tool` is not part of the observed parent registry builder.
- Sub-agents using `with_full_agent_surface` do get the tool.
- Requires prior cycle archives written by `archive_cycle`; without archives it returns no-archive output.

### Tests

- Tests cover archive listing order, no-archive behavior, matching messages, cycle filter, max result cap, empty query rejection, UTF-8 boundary handling, BM25 relevance, and archive read integration.
- Tests are direct tool tests plus archive-writer integration, not full parent model-call integration.

### Strings/keys

- Tool name: `recall_archive`.
- Input keys: `query`, `cycle`, `max_results`.
- Output keys: `query`, `cycle`, `max_results`, `archive_count`, `hits`, `message_index`, `role`, `score`, `excerpt`.
- User command string: `/recall <query>`.

### Config schema

- No TOML config.
- Depends on archive path convention from `cycle_manager`: `~/.deepseek/sessions/<session_id>/cycles/*.jsonl`.
- Session namespace comes from `ToolContext::state_namespace`; `/recall` uses `app.current_session_id.unwrap_or("workspace")`.

### Verdict

**PARTIALLY LIVE**.

Live through `/recall` and sub-agent tool registration, but stranded from the observed parent model-callable tool registry. It is not dead, but the model-visible parent path promised by some comments/docs is incomplete.

### Recommendation

- If model-call recall is desired, add `with_recall_archive_tool()` to `Engine::build_turn_tool_registry_builder` for appropriate modes and test registry membership.
- If only user-invoked recall is desired, update comments/docs to avoid implying the parent agent can call it.
- Keep archive reader compatibility while hard-cycle archives exist.

---

# Phase-1 opinion

## Classification table

| Module | Classification | Why |
|---|---|---|
| Cycle manager | LIVE, STATE-MUTATING, DESIGN-LOAD-BEARING; PRACTICAL LOAD-BEARING UNPROVEN | It owns hard-cycle restart, archive writing, seed-message construction, and carry-forward state. Rare but central when triggered. |
| Seam manager | GHOST / LIVE BUT REPLACEABLE | Opt-in by default; real engine path and UI parser exist. Can be removed only with explicit removal of experimental `[context].enabled` behavior. |
| Coherence reducer | LIVE BUT REPLACEABLE | Visible footer/runtime state, small pure reducer. Easy to refactor, not dead. |
| Capacity controller | GHOST / LIVE BUT REPLACEABLE | Every-turn wiring exists, but disabled by default. Opt-in path is tested and destructive. |
| Capacity memory | GHOST support module | Support for opt-in capacity interventions; startup/resume rehydration is live but usually no-op without prior records. |
| Capacity flow | GHOST / LIVE BUT REPLACEABLE | Every-turn checkpoint calls and compaction/coherence helpers are live; capacity interventions are default-off. |
| Cycle commands | LIVE BUT REPLACEABLE | Registered slash commands; no safe deletion while cycle/recall UX remains. |
| Recall archive tool | LIVE BUT STRANDED FROM PARENT TOOL SURFACE | Live as `/recall` and sub-agent tool, but parent agent registry appears not to include it. |

## Hypothesis assessment

Hunter's hypothesis is **partly supported**, but not in the strongest “dead code” form.

- **Not dead**: The files are compiled, referenced, tested, and in several cases called by the live engine loop or command registry.
- **Quietly stranded/default-off**: Capacity and seam behavior are largely inactive in default sessions. Their expensive/destructive behaviors are opt-in or threshold-gated.
- **Config mismatch**: Cycle configuration is the clearest stranded surface: code comments describe `[cycle.per_model]`, but active construction uses `CycleConfig::default()` and no observed top-level `[cycle]` config schema exists.
- **Recall mismatch**: `recall_archive` is live via `/recall` and sub-agents, but appears absent from the parent model-callable Agent/Plan registry.

## Deletion/refactor recommendations

### Safe immediate actions

- **Documentation/code-comment cleanup**:
  - Correct `cycle_manager.rs` comments about `[cycle.per_model]` unless adding real `[cycle]` config parsing.
  - Clarify whether `[context].cycle_threshold` affects hard cycles; currently it appears to affect only `SeamConfig`, not `CycleConfig`.
  - Clarify that `recall_archive` is available via `/recall` and sub-agents, but not observed in the parent registry.

### Small refactors worth doing before deletion

- **Split capacity flow**: Separate always-live compaction/coherence event helpers from default-off capacity interventions.
- **Registry test**: Add tests asserting which registries include `recall_archive`.
- **Config tests**: Add tests proving `[context].per_model` is applied or remove/deprecate it.
- **Cycle integration test**: Add a low-threshold cycle handoff test to validate engine event/state behavior end-to-end.

### Do not delete yet

- Do not delete `cycle_manager.rs` or `commands/cycle.rs` unless removing hard-cycle restart and archive UX as a product feature.
- Do not delete `seam_manager.rs` while `[context].enabled` remains documented.
- Do not delete capacity files while `[capacity]` remains documented opt-in behavior.
- Do not delete `recall_archive.rs` while `/recall` remains registered.

## Phase-2 action gate

No deletion is recommended from this codemap alone.

Recommended next step: choose one of these explicit product decisions:

1. **Keep and fix wiring**: preserve all subsystems, fix config/registry drift, and add integration tests.
2. **Deprecate opt-in ghosts**: mark `[capacity]` and/or `[context].enabled` as deprecated, keep compatibility for one release, then remove.
3. **Remove hard-cycle architecture**: delete cycle, recall, and archive UX together only after accepting loss of saved archive recall and hard-wall restart behavior.

