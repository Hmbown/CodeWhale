# CodeWhale 0.8.68 — Workflow + Stability Patch Release

> **What this is.** Local tracker + handoff for the CodeWhale 0.8.68 release
> candidate. It documents what's in scope, what's deferred to 0.9.0, and the
> current verification state.
>
> **2026-07-07 kickoff — READ THIS FIRST.** This release follows the 0.8.67
> Fleet/Workflow usability pass (PR #4047). Implementation work continues on
> `work/v0.9.0-cutover` in worktree
> `.cw-worktrees/v0867-pr4047`. Issue inventory lives in
> `milestone-audit-20260622/buckets/v0.8.68.md` (30 issues in bucket).
>
> **Sub-agent sweep completed 2026-07-07.** Six parallel scouts produced the
> release plan below. Workflow file:
> `CodeWhale/workflows/v0868_issue_sweep.workflow.js`.
>
> **2026-07-07 cutover correction — source of truth.** Work for this release
> belongs in `.cw-worktrees/v0867-pr4047` on `work/v0.9.0-cutover`. The
> durable PR base was `883f94df6`; the quick-win layer that followed was
> briefly stranded as uncommitted working-tree changes. This tracker update
> travels with that quick-win layer: OpenRouter live catalog parsing, provider
> picker search, Status/ToolDescriptor concept-map cleanup, copy/perf fixes,
> foreground shell compacting, and test alignment are locally verified here.
> Do not mark work complete from another checkout unless the symbols and tests
> are verified in this worktree.

## VERDICT: **partial** — can ship a narrow 0.8.68 patch after Wave 1–2

Wave 1 (three workflow correctness hazards) + Wave 2 (TUI P0 stability + core
workflow UX) is the minimum credible release. The full 30-issue bucket is
3–4 weeks; scope must be trimmed via closes and defers.

---

## SCOPE (ship in 0.8.68)

### P0 bugs (must-fix before tag)

| # | Title | Fix train commit |
|---|-------|------------------|
| **1830** | Input freeze / progress loss | Commits 1 + 4 (input fairness + periodic snapshot) |
| **1338** | Enter causes GUI crash (Windows) | Commit 2 (Enter-during-busy hardening) |

### P1 bugs (target for release)

| # | Title | Fix train commit |
|---|-------|------------------|
| **2317** | Long reply blocks further input | Commits 1 + 5 (fairness + queue UX toasts) |
| **1198** | No response on key input (approval/git) | Commits 1 + 3 (fairness + modal submit errors) |
| **1862** | TUI read "Reading" stuck | Tool hang watchdog tuning (15min → shorter + footer clear) |

### Workflow correctness (release-blocking)

| Hazard | Location | Fix |
|--------|----------|-----|
| `completion_from_manager` fabricates Completed | `workflow.rs:1412–1443` | Fail closed after 1s poll timeout |
| Cancel doesn't interrupt VM | `workflow.rs:380–404` | Wire `CancelHandle` + abort `run_workflow_vm` |
| `budget.spent()` stub | `workflow.rs:1121–1125` | Delegate to manager `aggregate_budget_spent` |

### P1 features (target for release)

| # | Title | Notes |
|---|-------|-------|
| **4038** | Workflow run view / phase progress UI | `SharedWorkflowRuns` exists; TUI never reads it |
| **4011** | Durable runs + journal/resume | In-memory only today; `codewhale-state` schema exists |
| **4013** | Verification gates | Reuse `task_gate_run` / Fleet verifier infra |
| **3380** | Approval modal key hints | **Good-first issue** — badges landed (#3799); footer contrast remains |

### Provider / model picker (2026-07-07)

**Source:** User-reported `/provider` and `/model` menu bugs (dogfood).
**Priority:** P1 · **Status:** Partial (P0 done 2026-07-07) · **GH:** new local scope (no GH # yet).
Related prior work: #3830, #3831 (0.8.67 configured-provider manager), #3083
(provider→model handoff), #3385 (live catalog — deferred).

| # | Acceptance criterion | Priority | Status |
|---|---------------------|----------|--------|
| 1 | **Catalog vs picker audit** — Together AI missing from `/model`; audit `models_dev.bundled.json` / `bundled_catalog_offerings()` vs picker sources (`model_completion_names_for_provider`); fix provider/model gaps | P1 | **Partial** — bundled flash rows added; full 13-provider audit done for bundled set |
| 2 | **Provider section UX** — `/provider` lists **configured** providers only; press `a` to add/browse **remaining unconfigured** providers (full catalog minus configured) | P1 | **Done** (#3830) |
| 3 | **Model section UX** — `/model` shows models from **configured providers only** — no more, no less | P1 | **Done** (#3830 + lake) |
| 4 | **Model search** — search matches **provider name OR model name** (substring across both) | P1 | **Done** (pre-existing) |
| 5 | **`ConfiguredProviderLake` facade** — single seam over `catalog.rs` + config auth: `configured_providers`, `models_for_provider`, `all_catalog_*` for expand | P0 | **Done** — `crates/tui/src/provider_lake.rs` |
| 6 | **Replace `model_completion_names_for_provider` consumers** — pickers, hotbar, `ModelInventory`, slash completions, subagent validation | P0 | **Done** — legacy table kept as fallback for unbundled providers |
| 7 | **Model picker `A` toggle** — `ModelListView { Configured, Catalog }` parity with `/provider` | P0 | **Done** |
| 8 | **Bundled↔picker audit** — sync `models_dev.bundled.json` with picker rows (Together Flash drift vs hardcoded list) | P0 | **Done** — Together/OpenRouter/Novita/SiliconFlow flash rows added (31 offerings) |

**Likely files:** `crates/tui/src/tui/model_picker.rs`, `provider_picker.rs`,
`crates/tui/src/config.rs`, `crates/config/src/models_dev.rs`,
`crates/config/assets/models_dev.bundled.json`, `crates/config/src/catalog.rs`,
`crates/tui/src/provider_lake.rs`.

### Progressive disclosure / provider lake (ethos audit) — 2026-07-07

**Source:** Agent d7e2f642 ethos audit.
**Verdict:** **Mostly aligned (2026-07-07 Wave 5b).** Lake = `provider_lake.rs` + `models_dev.bundled.json` + configured predicate (#3830). Pickers, hotbar, `ModelInventory`, slash completions, and subagent hints now read the lake; legacy `model_completion_names_for_provider` remains fallback for providers not yet in bundled JSON. Live catalog (#3385) still unwired.

| Surface | Score | Fix |
|---------|-------|-----|
| Provider picker | ✅ Aligned | Slim configured rows + catalog model counts via lake |
| Model picker | ✅ Aligned | `A` expand + catalog lake rows |
| Fleet roster | ⚠️ Partial | Loadout/model pins via lake |
| Workflow tool schema | ✅ Aligned | — |
| Mode footer (Operate/Multitask) | ✅ Aligned | — |
| Hotbar route slots | ✅ Aligned | Lake enumeration |
| Operate/Multitask prompts | ✅ Aligned | Conductor guidance; no catalog leak |
| ModelInventory / auto-router | ✅ Aligned | Lake-backed inventory |

**P0 fixes**

1. **`ConfiguredProviderLake` facade** (extend `catalog.rs` or thin `provider_lake.rs`) — `configured_providers`, `models_for_provider` from merged catalog snapshot, `all_catalog_*` for `A` expand.
2. **Replace `model_completion_names_for_provider` consumers** — pickers, hotbar, `ModelInventory`, slash completions, subagent validation (**subagent spawn validation done 2026-07-07**).
3. **Model picker `A` toggle** — `ModelListView { Configured, Catalog }` + footer hint (parity with provider picker).
4. **Bundled↔picker audit** — sync `models_dev.bundled.json` with picker expectations (Together Flash drift).

**P1 fixes**

1. **Provider picker disclosure trim** — Configured view: display name + auth chip; move `compact_hint` internals to detail panel / `A` catalog view only.
2. **Unify `bundled_offerings` seeds** into catalog-derived offerings (remove `OFFERING_SEEDS` drift).
3. **Wire live cache read** into lake (refresh async; stale fallback to bundled) — completes #3385 when ready.

**Wave alignment:** Wave 7 Operate/Multitask ✅ ethos-aligned (thin footer, no catalog leak). Waves 1–4 neutral. **Wave 5b P0 done 2026-07-07** (`provider_lake.rs`, model/provider `A` toggles, bundled flash sync); P1 live-cache + `OFFERING_SEEDS` dedupe remain.

### Subagent route validation (2026-07-07)

**Source:** User screenshot — sub-agent failure:
`Failed: [model] Model error: Model "deepseek-v4-flash" not found (provider 'Sakana AI (Fugu)' …)`.
**Priority:** P0 · **Status:** Fixed (local, uncommitted) · **Worktree:** `.cw-worktrees/v0867-pr4047`.

**Root cause:** Sub-agent spawn resolved a DeepSeek-only model id (`deepseek-v4-flash`) against a
different active provider (Sakana AI / Fugu). `validate_route` (#3227) existed but was
`#[cfg(test)]`-only; inherit/faster routing and permissive `requested_model_for_provider` let
stale operator models or fleet profile pins cross namespaces before the upstream 400.

| # | Acceptance criterion | Priority | Status |
|---|---------------------|----------|--------|
| 1 | **Spawn-time model↔provider validation** — `ensure_subagent_model_for_provider` calls production `validate_route` before spawn | P0 | Done |
| 2 | **Operator inheritance must not cross namespaces** — inherit/faster/auto remap to operator catalog default when parent model is foreign | P0 | Done |
| 3 | **Explicit pins fail fast** — fixed model / role override / spawn `model=` rejected locally with diagnostic naming the pair | P0 | Done |
| 4 | **`normalize_requested_subagent_model` uses lake + `validate_route`** — not just permissive pass-through | P0 | Done |

**Likely files:** `crates/tui/src/tools/subagent/mod.rs`, `crates/tui/src/config.rs`,
`crates/tui/src/core/engine.rs`, `crates/tui/src/provider_lake.rs`.

**Tests added:** `inherit_route_remaps_stale_deepseek_model_for_sakana_provider`,
`faster_route_remaps_stale_deepseek_model_for_sakana_provider`,
`fixed_route_rejects_deepseek_model_for_sakana_provider`,
`normalize_requested_subagent_model_rejects_cross_namespace_for_sakana`,
`validate_route_rejects_mismatched_provider_model_tuple` (Sakana case).

**Symptom link:** screenshot error pairs `deepseek-v4-flash` with Sakana AI (Fugu) — exactly the
#3227 route-isolation contamination class; fix prevents spawn instead of upstream model-not-found.

### UI/UX copy slop audit (2026-07-07)

**Source:** Agent 9a53917c copy-slop audit (worktree `v0867-pr4047`).
**Verdict:** **18 findings** — 7 P1 (same-screen status/mode repetition + foreground shell wait verbosity), 11 P2 (toast vs chip, approval/setup boilerplate).
**Ethos:** **Disclose once, not thrice** — each fact at one highest-signal layer; drop redundant chrome over rephrasing.
**Wave:** **5c** (TUI copy dedupe) — **Done 2026-07-07** (original P1 #1–#6 + P2 #7/#8/#9/#10/#12; #17/#18 tracked separately).

| # | Location | What repeats | Sev | Suggested fix (dedupe one layer) | Status |
|---|----------|--------------|-----|----------------------------------|--------|
| 1 | `header.rs:534` + `footer.rs:307-314` | Mode in **header left** (`Plan`/`Act`/…) and **footer left** (`plan`/`act`/…) simultaneously | **P1** | Keep mode in **one** chrome row only (header *or* footer); footer keeps model/cost/status | **Done** — footer blanks `mode_label` |
| 2 | `header.rs:377-393` + `footer_ui.rs:67-97,1127-1146` | Header `● Live` while footer shows `busy` / animated `working...` / tool detail during same turn | **P1** | Header owns live pulse; footer shows **action detail only** (tool name, stall reason) — drop coarse `busy`/`working` when header streams | **Done** — `header_owns_live_pulse`, action-only footer |
| 3 | `history.rs:808-868` | Explore card: header state `done`/`running` + summary `{N} done, {M} running` + per-entry KV prefix `done`/`live` | **P1** | Header = aggregate glyph only; **either** counts line **or** per-entry prefixes, not both | **Done** — multi-entry: glyph header + dot counts, label-only rows |
| 4 | `agent_card.rs:318-356` | Fanout card: header `[done]`/`[running]` + dot grid + `FanoutCounts` (`{done} done · {running} running · …`) | **P1** | Drop `FanoutCounts` when header+grid present; or header shows role/title only, counts line owns status words | **Done** — counts line removed |
| 5 | `sidebar.rs:2770-2810` + `footer_ui.rs:474-504` | Agents panel: `N running / M` or `N done` header + rows `… is working`/`… is done` while footer may show `agents N/M running · …` | **P1** | Footer **or** sidebar owns fanout summary; rows show **name + objective**, not status verb | **Done** — sidebar rows name—objective; footer suppresses when agents panel visible |
| 6 | `app.rs:3043` + header/footer mode chips | `Switched to ACT mode` toast while mode already visible in header+footer | **P1** | Suppress mode-switch toast when mode chips visible; toast only on picker `/mode` or first session | **Done** — no status_message on Tab cycle; `/mode` command message retained |
| 7 | `history.rs:1525-1539` | Workflow run card (Wave 3): header `tool_status_label` (`done`/`running`) + body KV `status: <same>` | **P2** | Header owns lifecycle; body shows goal/children/progress only | **Done** |
| 8 | `footer_ui.rs:566-578` + tool cards in `history.rs` | Footer `read foo · 2 active · 1 done` while transcript cards already show per-tool `done`/`running` | **P2** | Footer = **primary running action + elapsed**; drop `active`/`done` counts | **Done** — `include_counts=false` when header streams |
| 9 | `app.rs:3183-3187` + `footer.rs:326-337` | Shift+Tab toast `Permissions: Ask` + footer `perm Ask` chip | **P2** | Chip is canonical; drop permission toast (or toast on lock-denial only) | **Done** |
| 10 | `app.rs:3157-3165` + `header.rs:314-338` | Ctrl+T toast `Thinking: high` + header effort chip `◆ high` | **P2** | Header chip only; toast on first post-migration session only | **Done** |
| 11 | `header.rs:396-407` + `footer_ui.rs:857-871` + `sidebar.rs:3175-3203` | Context % in header, optional footer `active ctx N%`, sidebar `context: X/Y tokens … N%` | **P2** | **Disclose once:** header default; hide sidebar bar when header shows %; footer chip off by default | Open |
| 12 | `widgets/mod.rs:1595-1617` + `en.json:520-522` | Approval: per-row `[1 / y]` + `Choose: Enter selected option, or press y/a/d directly` + `v: full params · Esc: abort` | **P2** | Rows keep key badges; collapse footer to `v` pager + `Esc` only | **Done** |
| 13 | `mode_picker.rs:102-130` + `en.json:484` | Modal title `Mode` + prompt `Choose how CodeWhale should operate:` | **P2** | Title carries intent; drop `ModePickerPrompt` body line | Open |
| 14 | `en.json:147,151,245,262` | Fleet status phrasing repeated: `/fleet status`, `Fleet worker status`, `Fetching Fleet worker status...` | **P2** | One canonical phrase in slash help; home/quick lines reference command name only | Open |
| 15 | `en.json:389-422` (setup hints) | Six near-identical `Enter records…` hints differing only by step noun | **P2** | Single shared hint template + step-specific **one-word** action (`P`/`M`/`R`) | Open |
| 16 | `en.json:494` + `app.rs:3102-3106` | YOLO deprecation in picker hint **and** one-shot compat toast | **P2** | Picker hint for discoverability; suppress repeat toast after first sighting per install | Open |
| 17 | `history.rs:659-770` (`ExecCell::render`) | Foreground shell wait: main transcript shows header + `command:` + live output/artifact paths + separate Ctrl+B line (4+ lines) | **P1** | Live foreground wait = one header line (`▶ run running (Ns) · Ctrl+B → /jobs`); command/output/artifacts in Tasks sidebar + `/jobs` only; Transcript mode keeps full body |
| 18 | `footer_ui.rs:600-601,831-870` + `history.rs` compact wait | Footer `shell fg: <cmd>` + `Ctrl+B /jobs` duplicates transcript/sidebar shell detail | **P2** | Footer keeps primary action chip; drop redundant counts (see #8) |

**Likely files:** `crates/tui/src/tui/widgets/header.rs`, `footer.rs`, `footer_ui.rs`, `history.rs`, `agent_card.rs`, `sidebar.rs`, `widgets/mod.rs`, `views/mode_picker.rs`, `app.rs`, `crates/tui/locales/en.json`.

**Acceptance (finding #17 — foreground shell wait):** While a foreground `exec_shell` blocks the turn, the **main transcript** shows only spinner + `running` (+ elapsed badge) + `Ctrl+B → /jobs`. Command line, live stdout, spillover/artifact paths (`call_*.txt`), and call IDs appear in the **Tasks/jobs sidebar scroll** and `/jobs show` detail — not in the live transcript card.

### Modes & permissions (Multitask) — Wave 7

**Source:** Multitask mode design (agent 776bb3c0, 2026-07-07).
**Priority:** P1 · **Status:** Done (2026-07-07) · **Depends on:** Wave 3 workflow UX (done),
authority baseline (#3386 shipped in 0.8.67).

**2026-07-07 correction:** Operate AppMode = **Fleet operator posture** — session `/model` route is the operator slot (pinned first row in `/fleet roster`); operator decomposes into workflow/Fleet, workers execute, operator monitors. Multitask = lighter delegation; Operate = full conductor. See `prompts/modes/operate.md`.

| Epic | ID | Acceptance criteria | Status |
|------|----|---------------------|--------|
| Tab 4-mode cycle | **M1** | Tab cycles Plan → Act → Multitask → Operate → Plan; YOLO removed from cycle; `/mode` + hotbar accept new modes; footer shows **Act** label for Agent | **Done** — `app.rs` CYCLE/CHOICES, footer, hotbar, `KEYBINDINGS.md` |
| Permission on Shift+Tab | **M2** | Shift+Tab cycles Suggest → Auto → Bypass (Ask / Auto-Review / Full Access) with trust/sandbox projection; footer permission chip separate from mode chip; locked while turn running (#2982) | **Done** — `cycle_approval_posture`, `footer_permission_chip` |
| Thinking on Ctrl+T | **M3** | Ctrl+T cycles reasoning effort (moved from Shift+Tab); live-transcript overlay relocated (e.g. `Ctrl+Shift+T` or `Alt+T`); KEYBINDINGS.md + in-app migration toast | **Done (verified 2026-07-07)** — `ui.rs` Ctrl+T/Ctrl+Shift+T, `KEYBINDINGS.md`; review pass added the missing one-shot Shift+Tab rebinding toast (`notify_keybinding_migration_once` + test) and fixed stale Ctrl+T doc comments in `live_transcript.rs` |
| Multitask mode behavior | **M4** | `multitask.md` prompt delta; higher default subagent fan-out; Agents sidebar auto-focus; non-blocking `workflow start`; operator-vs-worker spawn contract | **Done** — `multitask.md`, `apply_session_spawn_policy` (Multitask→Faster default), `mode_delegation_launch_floor`, Multitask→Agents sidebar |
| Operate mode (thin) | **M5** | Operate AppMode = Fleet operator: session model route, `operate.md` conductor prompt, workflow run cards, `operate_ready` hints; full Operation value-stream → 0.9.0 | **Done** — `operate.md`, `SubAgentRuntime.parent_mode`, Operate spawn_policy metadata on `agent` start; full value-stream → 0.9.0 |
| YOLO → permissions migration | **M6** | `--yolo` / `default_mode=yolo` / hotbar `mode.yolo` map to Agent + `ApprovalMode::Bypass` via shim; deprecation notice in MODES.md; `AppMode::Yolo` kept for parse/back-compat only | **Done** — `set_mode` shim + one-shot toast |

**Linked GH:**

| Issue | Action |
|-------|--------|
| **#3386** | **Close** — mode/permission untangle shipped 0.8.67 (`authority.rs`, `base_policy_for_mode`) |
| **#3387** | **Close** — prompt-as-mode-switch fixed 0.8.67 (#3491) |
| **#3211** | **Defer** full permission profiles + `/permissions` UX to 0.9.0; M2 ships Shift+Tab chord slice only |

**Likely files:** `crates/tui/src/tui/app.rs` (`AppMode`, `CYCLE`), `ui.rs` (Tab/Shift+Tab/Ctrl+T),
`core/authority.rs`, `prompts/modes/{multitask,operate}.md`, `widgets/footer.rs`,
`hotbar/actions.rs`, `tools/subagent/mod.rs`, `tools/workflow.rs`, `docs/KEYBINDINGS.md`,
`docs/MODES.md`.

**Risks:**

1. **Muscle memory:** Shift+Tab (thinking → permission) and Ctrl+T (overlay → thinking) churn — ship one-release migration toast + KEYBINDINGS.md banner.
2. **Back-compat:** `default_mode = "yolo"`, hotbar `mode.yolo`, `--yolo` CLI must keep working via shim through 0.8.68.
3. **Operate vs cutover tension:** 0.9.0 cutover doc treats Operate as orchestration structure, not `AppMode`; 0.8.68 Operate is a **thin AppMode** — document scope boundary.
4. **Ctrl+T conflict:** live-transcript overlay already binds Ctrl+T — relocation required before thinking migration.

### Hotbar (partial — close what's done)

| # | Title | Status |
|---|-------|--------|
| **2067** | Slash commands source | **Done** — close |
| **2068** | MCP tools source | **Done 2026-07-07** — `McpToolHotbarActionSource`, prefill-only dispatch (never executes; agent approval flow untouched); lists enabled-server tools from discovery snapshot |
| **2069** | Skills source | **Done 2026-07-07** — `SkillHotbarActionSource` from startup skill cache; dispatch via existing `$skill` alias |
| **2070** | Plugins source | **Audited 2026-07-07 → close** — no TUI-side plugin registry/snapshot exists; a source would need side-effectful disk scans + config reloads; descriptor stays Deferred with tests enforcing it |

---

## DEFER (explicitly out of 0.8.68)

| Issues | Reason |
|--------|--------|
| **1890–1897** | Refactor epics (workbench, truth surface, slash suite) — 0.9.0+ |
| **1754** | Shell-aware AI commands — L-sized, cross-cutting |
| **1708** | `tui_help` tool — net-new surface |
| **1682** | Output/thinking preview redesign — open-ended UX epic |
| **2342** | File click preview — needs product design |
| **4039** | Background task phase ledger UI — UX polish |
| **4010, #4012, #4014–#4016** | Conductor, topology, lag, context budget, worktree pool — 0.9.0 architecture |

---

## CLOSE

| # | Title | Reason |
|---|-------|--------|
| **3324** | mosaic-compress promo | Third-party recommendation, no codebase tie-in |
| **1607** | More currency units | CNY already implemented (`cost_currency = "cny"`) |
| **1678** | Version check + GitHub link | Already exists (`UpdateConfig`, `/links`) |
| **1853** | Terminal copy line breaks | Documented behavior; `/copy` + mouse_capture handle it |
| **2067** | Hotbar slash source | Implemented in v0867-pr4047 |

---

## WAVES (execution order)

| Wave | Theme | Issues / hazards | Owner lane |
|------|-------|------------------|------------|
| **1** | Workflow correctness | H1 completion_from_manager, H2 cancel→VM, H3 budget.spent() | `.cw-worktrees/v0867-pr4047/crates/tui/src/tools/workflow.rs` |
| **2a** | TUI input fairness | #1830, #2317, #1198 foundation | `crates/tui/src/tui/ui.rs` event loop |
| **2b** | TUI P0 hardening | #1338 Windows Enter, #1830 persistence | `ui.rs`, `app.rs` |
| **3** | Workflow UX | #4038 run view, #4011 journal, #4013 gates | `history.rs`, `workflow.rs`, `verifier.rs` — **Done** (minimal slices) |
| **4a** | Deep-dive security & infra | DD #1, #2, #4, #5, #14, #15, #19–#21, #34, #36–#37, #39–#40, #42 | `app-server/`, `execpolicy/`, `config/`, `secrets/`, `cli/`, `hooks/` |
| **4b** | Deep-dive core/runtime | DD #10–#13, #16–#18, #23, #25, #33, #50 | `core/`, `state/`, `mcp/`, `workflow-js/` |
| **5** | Hotbar + polish | #3380 approval UX, #2068/#2069 adapters | `approval.rs`, `hotbar/actions.rs` |
| **5b** | Provider/model picker UX + provider lake | Local #1–#8: P0 `ConfiguredProviderLake` facade, replace `model_completion_names_for_provider` consumers, model picker `A` toggle, bundled↔picker audit; P1 configured-only lists, search, disclosure trim | `model_picker.rs`, `provider_picker.rs`, `config.rs`, `catalog.rs`, `provider_lake.rs` (new) |
| **5c** | TUI copy dedupe | Header/footer mode dup, done×3 on explore/fanout cards, toast vs chip, workflow status KV, approval footer trim (18 findings: P1 #1–#6 **done**, P2 #7/#8/#9/#10/#12 **done**) | `header.rs`, `footer.rs`, `footer_ui.rs`, `history.rs`, `agent_card.rs`, `sidebar.rs`, `widgets/mod.rs`, `app.rs` — **Done 2026-07-07** |
| **6** | Platform investigate | #1327 FreeBSD, #1675 CJK, #1854 Windows .bat | Platform-specific |
| **7** | Modes & permissions (Multitask) | M1–M6 (Tab cycle, Shift+Tab permission, Ctrl+T thinking, Multitask MVP, Operate thin, YOLO shim) | `app.rs`, `ui.rs`, `authority.rs`, `prompts/modes/`, `footer.rs` |

### TUI fix train detail (Wave 2)

1. **Commit 1** — Event-loop input fairness (`ui.rs` — break engine drain every 8–16 events)
2. **Commit 2** — #1338 Windows Enter-during-busy hardening
3. **Commit 3** — #1198 modal submit error handling (stop swallowing `submit_user_input` errors)
4. **Commit 4** — #1830 periodic recovery snapshot (30–60s while loading)
5. **Commit 5** — #2317 queue UX toasts during streaming

---

## Control board

| Lane | Status | Constraint |
|------|--------|------------|
| Core workflow (#4038, #4011, #4013) | **Done** — Wave 3 | Transcript run cards + `.codewhale/workflow-runs.jsonl` journal + optional `verify` completion gates |
| Workflow hazards (H1–H3) | **Verified 2026-07-07** — Wave 1 | `workflow.rs` + `vm.rs`; `cargo test … --locked workflow` 20/20, `codewhale-workflow` 73/73, `codewhale-workflow-js` pass; regression tests `completion_from_manager_fails_closed_when_status_stays_running`, `workflow_cancel_interrupts_vm_and_blocks_further_spawns`, `workflow_budget_spent_delegates_to_manager_scope`; uncommitted in worktree |
| TUI stability (#1830, #1338, #1198, #2317) | **Done** — Wave 2 | Input fairness + steer hardening + modal submit + snapshots + queue toasts; **verified 2026-07-07** — `cargo check -p codewhale-tui --bin codewhale-tui` clean; Wave 2 targeted tests 4/4 pass after Wave 7 `AppMode::Multitask`/`Operate` match exhaustiveness (`status.rs`, `core.rs`, `config_ui.rs`, `header.rs`, `authority.rs`, `engine.rs`, `footer.rs`, `widgets/mod.rs`) |
| Deep-dive security/infra (DD #1–#5, app-server) | **Done** — Wave 4A | #1 auth proxy, #2 HTTP status, #4 execpolicy layer, #5 atomic save, #14–#15, #20–#21 |
| Deep-dive core/runtime (DD #10–#18, MCP) | **In motion** — Wave 4b | Paused jobs, checkpoints, tool timeout |
| Hotbar adapters (#2068–2070, #3380) | **Done 2026-07-07** — Wave 5 | #2067 done (close); #2068/#2069 implemented (prefill/skill-alias dispatch); #3380 footer contrast TEXT_MUTED + regression test; #2070 audited → close; `cargo test … -- hotbar approval` 258/258 |
| Provider/model picker + lake (local #1–#8) | **Partial** — Wave 5b P0 done | Lake facade + picker wiring + bundled flash sync; P1 live cache + `OFFERING_SEEDS` dedupe remain — see [ethos audit](#progressive-disclosure--provider-lake-ethos-audit--2026-07-07) |
| TUI copy dedupe (copy slop audit) | **Done** — Wave 5c | P1 #1–#6 + P2 #7/#8/#9/#10/#12 shipped in `v0867-pr4047`; open P2: context % (#11), mode picker (#13), fleet phrasing (#14), setup hints (#15), YOLO repeat toast (#16), shell footer dup (#18) — see [copy slop audit](#uiux-copy-slop-audit-2026-07-07) |
| Platform (#1327, #1675, #1854) | **Investigated 2026-07-07** — Wave 6 | #1327: already fixed by #2468, reporter-confirmed — close, no action. #1675: no code bug found (stream pipeline is grapheme/width-safe end-to-end); needs live CJK repro — defer 0.9.0. #1854: fix = `wt.exe`-preferring `.bat` launcher in release packaging; needs Hunter approval; not a 0.8.68 blocker |
| Modes & permissions (Multitask, M1–M6) | **Done** — Wave 7 | M1–M6 shipped; Operate = Fleet operator (`operate.md` + spawn policy); all `AppMode` match arms exhaustive; close #3386/#3387 |
| Defer/close (#3324, #1890-series) | **Done** — 15 issues trimmed | Scope reduction |

---

## Issue inventory (v0.8.68-tagged)

| # | Title | Type | Priority | Status |
|---|-------|------|----------|--------|
| 3380 | Approval modal key hints more prominent | UX | P2 | **Done (Wave 5)** — footer hints TEXT_HINT→TEXT_MUTED |
| 3324 | mosaic-compress library recommendation | Community | — | **Close** |
| 2342 | File preview on click | Enhancement | P3 | Defer |
| 2317 | Long reply blocks further input | Bug | P1 | Wave 2 |
| 2070 | Hotbar: plugins source (exploratory) | Enhancement | P3 | Audit → close |
| 2069 | Hotbar: skills source | Enhancement | P2 | **Done (Wave 5)** — `SkillHotbarActionSource` via `$skill` alias |
| 2068 | Hotbar: MCP tools source | Enhancement | P2 | **Done (Wave 5)** — prefill-only MCP source |
| 2067 | Hotbar: slash commands source | Enhancement | P2 | **Close (done)** |
| 2061 | Hotbar umbrella | Epic | — | Update checklist |
| 1862 | TUI read stuck | Bug | P1 | Wave 2 |
| 1830 | Input freeze / progress loss | Bug | P0 | Wave 2 |
| 1338 | Enter causes GUI crash (Windows) | Bug | P0 | Wave 2 |
| 1327 | FreeBSD dispatch timeout | Bug | P2 | Wave 6 investigate |
| 1198 | No response on key input | Bug | P1 | Wave 2 |
| 1165 | Settings border rendering (Windows) | Bug | P2 | P2 cosmetic — defer if time |

---

## Deep-dive additions (2026-07-07)

Full report: [`CODEWHALE_0_8_68_DEEP_DIVE.md`](CODEWHALE_0_8_68_DEEP_DIVE.md)

Parallel scouts across all 17 crates + web frontend found **64 numbered items**
(~75 raw bugs) independent of the 30-issue milestone bucket. Deduped against
Waves 1–3:

| Wave overlap | Deep-dive # | Tracker link |
|--------------|-------------|--------------|
| Wave 1 (workflow correctness) | **#3**, **#8**, **#9** | H1–H3 hazards |
| Wave 2 (TUI stability) | **#6**, **#7**, **#45** | #1830, #1338, #2317 |
| Wave 3 (workflow UX) | **#4038**, **#4011**, **#4013** | Done — transcript cards, JSONL journal, completion gates |

### Deep-dive disposition (all 64 items)

| # | Sev | Crate | Disposition |
|---|-----|-------|-------------|
| 1 | C | app-server | **Done (Wave 4A)** |
| 2 | C | app-server | **Done (Wave 4A)** |
| 3 | C | tui/workflow | **Done (Wave 1, verified 2026-07-07)** — fail-closed + regression test |
| 4 | C | execpolicy | **Done (Wave 4A)** |
| 5 | C | config | **Done (Wave 4A)** |
| 6 | C | tui | **Done (Wave 2 gap-fill 2026-07-07)** — `restart_detached()` un-gated from Windows; Unix stall recovery now restarts pump (ui.rs) + tests |
| 7 | C | tui | **Done (Wave 2 gap-fill 2026-07-07)** — raw-mode probe handshake disables raw mode on abandoned startup probe (ui.rs) + tests |
| 8 | H | tui/workflow | **Done (Wave 1, verified 2026-07-07)** — WorkflowRunController cancels VM + aborts run handle |
| 9 | H | tui/workflow | **Done (Wave 1, verified 2026-07-07)** — budget.spent() wired to manager scope |
| 10 | H | core | **Done (Wave 4b, verified 2026-07-07)** — `JobStateStatus::Paused` variant + mapping; test `paused_job_persists_as_paused_not_running` |
| 11 | H | core | **Done (Wave 4b, verified 2026-07-07)** — unarchive refreshes `running_threads` cache; test `unarchive_thread_updates_running_threads_cache` |
| 12 | H | core | **Done (Wave 4b, verified 2026-07-07)** — `tool_dispatch_timeout()` wrapper (300s prod) + timeout error frame; test `invoke_tool_returns_timeout_status_for_slow_tools` |
| 13 | H | mcp | **Done (Wave 4b, verified 2026-07-07)** — notifications (id-less) get no JSON-RPC response; test `jsonrpc_notifications_do_not_require_responses` |
| 14 | H | execpolicy | **Done (Wave 4A)** |
| 15 | H | secrets | **Done (Wave 4A)** |
| 16 | H | state | **Done (Wave 4b, verified 2026-07-07)** — checkpoint parse errors propagate; test `load_checkpoint_propagates_invalid_state_json` |
| 17 | H | state/config | **Done (Wave 4b, verified 2026-07-07)** — `ProviderChain::current()` no longer indexes empty list; test `current_on_empty_chain_returns_default_provider` |
| 18 | H | state | **Done (Wave 4b, verified 2026-07-07)** — session index compacts at threshold; test `session_index_compacts_after_threshold` |
| 19 | H | app-server | **Done (Wave 4a gap-fill 2026-07-07)** — `with_graceful_shutdown` (ctrl_c + SIGTERM) |
| 20 | H | app-server | **Done (Wave 4A)** |
| 21 | H | app-server | **Done (Wave 4A)** |
| 22 | H | tui | **Deferred 0.9.0 (2026-07-07)** — Wave 2 landed; OSC 52 off main loop is polish |
| 23 | H | tui/workflow | **Done (Wave 1/4b)** — interrupt load Relaxed→Acquire, cancel store SeqCst (vm.rs) |
| 24 | H | protocol | **Deferred 0.9.0** |
| 25 | H | app-server | **Done (Wave 4b gap-fill 2026-07-07)** — child reaped on detached thread; Drop no longer blocks runtime |
| 26 | M | core | **Deferred 0.9.0** |
| 27 | M | core | **Deferred 0.9.0** |
| 28 | M | core | **Deferred 0.9.0** |
| 29 | M | execpolicy | **Deferred 0.9.0** |
| 30 | M | mcp | **Deferred 0.9.0** |
| 31 | M | state | **Deferred 0.9.0** |
| 32 | M | state | **Deferred 0.9.0** |
| 33 | M | secrets | **Done (Wave 4b gap-fill 2026-07-07)** — `sync_all` before tempfile persist |
| 34 | M | app-server | **Done (Wave 4a gap-fill 2026-07-07)** — constant-time bearer token compare |
| 35 | M | app-server | **Deferred 0.9.0** |
| 36 | M | app-server | **Done (Wave 4a gap-fill 2026-07-07)** — 16 MiB SSE frame bound in `stream_turn_events` |
| 37 | M | app-server | **Done (Wave 4a gap-fill 2026-07-07)** — stdio `shutdown` kills runtime child via `shutdown_child()` |
| 38 | M | app-server | **Deferred 0.9.0** |
| 39 | M | cli | **Deferred 0.9.0 (2026-07-07)** — login provider-switch is a product-behavior decision, not a patch fix |
| 40 | M | cli | **Deferred 0.9.0 (2026-07-07)** — flag validation needs scoping; no crash/security impact |
| 41 | M | cli | **Deferred 0.9.0** (known plaintext storage) |
| 42 | M | hooks | **Done (Wave 4a gap-fill 2026-07-07)** — 10s reqwest timeout on WebhookHookSink client |
| 43 | M | workflow | **Deferred 0.9.0** |
| 44 | M | workflow | **Deferred 0.9.0** |
| 45 | M | tui | **Deferred 0.9.0 (2026-07-07)** — paste-burst infra untouched by Wave 2; no regression, tuning only |
| 46 | M | tui | **Deferred 0.9.0** |
| 47 | M | tui | **Deferred 0.9.0** |
| 48 | M | tui | **Deferred 0.9.0** |
| 49 | M | tui | **Already tracked (#1678)** |
| 50 | M | config | **Done (Wave 4b, verified 2026-07-07)** — `ConfigStore::save` uses `atomic_write` + one-time backup on all platforms |
| 51 | M | tools | **Deferred 0.9.0** |
| 52 | L | workflow | **Deferred 0.9.0** |
| 53 | L | workflow | **Deferred 0.9.0** |
| 54 | L | workflow | **Deferred 0.9.0** |
| 55 | L | tui | **Already tracked (#1165)** |
| 56 | L | tui | **Already tracked (#1338)** |
| 57 | L | tui | **Already tracked (#1338)** |
| 58 | L | tui | **Deferred 0.9.0** |
| 59 | L | core | **Deferred 0.9.0** |
| 60 | L | core | **Deferred 0.9.0** |
| 61 | L | web | **Deferred 0.9.0** |
| 62 | L | web | **Deferred 0.9.0** |
| 63 | L | web | **Deferred 0.9.0** |
| 64 | L | web | **Deferred 0.9.0** |

**Wave 4 uncovered count: 27** (4 Critical + 15 High + 8 Medium; excludes Wave 1–3
in-motion items and deferred/tracked overlap).

---

## OPEN RISKS

1. **Full bucket overload:** 30 issues is 3–4 weeks; Wave 1–3 is the realistic ship set.
2. **Windows conhost cluster:** #1338, #1165, #1830 share legacy conhost paths — WT launcher (#1854) mitigates but doesn't fix.
3. **Workflow integration test flake:** `completion_from_manager` race likely root cause (H1 fix should deflake).
4. **Dogfood gap:** `v0867-main-dogfood` has transcript workflow card polish not in `v0867-pr4047` — port before #4038.
5. **Deep-dive surface area:** 64 new items expand scope beyond the 30-issue bucket; Wave 4 (security + core) is required for a credible ship alongside Waves 1–2.

---

## Implementation lane

- **Worktree:** `/Users/hunter/Desktop/Harnesses/CW/.cw-worktrees/v0867-pr4047`
- **Branch:** `work/v0.9.0-cutover`
- **Base version:** `0.8.67` (from PR #4047)
- **Target version:** `0.8.68`

## Verification gate

```bash
cargo fmt --all --check
cargo clippy --workspace --all-features --locked -D warnings
cargo test -p codewhale-tui --bin codewhale-tui --locked
cargo test -p codewhale-workflow --locked javascript
./scripts/release/check-versions.sh
```

Manual: Win11 Enter-during-busy (#1338), 15+ min turn follow-up (#2317), approval Enter (#1198), `/workflow cancel` mid-run.

---


## Quick-win cutover verification (2026-07-07)

Verified in `.cw-worktrees/v0867-pr4047` on `work/v0.9.0-cutover`.
Topology before the quick-win commit: `origin/main...HEAD = 0 behind / 9 ahead`
with `origin/main` at `cdb52ee48` and branch head `883f94df6`.

**Verified complete in this layer**

- **S1.1 OpenRouter live parser:** `parse_openrouter_models_response` maps
  limits, pricing, reasoning support, and modalities into `CatalogOffering`.
- **Provider lake live bridge:** `refresh_catalog_cache` now publishes fresh
  cache snapshots into `provider_lake`; the remaining #3385 work is scheduling
  or invoking refresh from UI/runtime surfaces.
- **S3 picker search:** provider picker search matches provider name and model
  ids across configured/catalog views.
- **S4 concept-map cleanup:** `ThreadStatus` dedupe, `ToolDescriptor` rename,
  `Status` trait, and `MODEL_ALIAS_PRECEDENCE.md` are present.
- **S5 copy cleanup:** verified items 5.1-5.6 plus compact foreground shell
  wait and stale test copy updates are in this layer; remaining copy items stay
  open in the audit table.
- **S6 perf quick wins:** the verified subset is present; do not mark the full
  perf list complete until the remaining unchecked S6 items are implemented.
- **Wave 2 wait-state behavior:** streaming Enter queues follow-up; model
  waiting Enter steers immediately; double-tap still steers while streaming.

**Local verification**

- `cargo fmt --all --check` — pass
- `cargo clippy --workspace --all-features --locked -- -D warnings ...` — pass
- `cargo test -p codewhale-tools --locked` — pass, 18 tests
- `cargo test -p codewhale-tui --bin codewhale-tui --locked --quiet` — pass,
  **5976 passed / 0 failed / 2 ignored**
- `cargo test -p codewhale-workflow --locked javascript --quiet` — pass,
  6 tests
- `./scripts/release/check-versions.sh` — pass, workspace/npm/lockfile in sync

**Still open before merge to main**

- Push the quick-win commit to `origin/work/v0.9.0-cutover` so PR #4099 runs CI
  on this exact state.
- Check PR #4099 macOS CI after push; the earlier red macOS job ran against the
  old committed head and is not evidence for or against this quick-win layer.
- Fleet/AgentProfile cutover remains open: Fleet should keep execution
  durability (`manager.rs`, `ledger.rs`, `executor.rs`, `task_spec.rs`) while
  consuming canonical AgentProfiles instead of maintaining a separate
  loadout/model-class profile system.
- Catalog consumer migration beyond S1.1/S3, Section 2 workflow UI/launch, and
  unchecked copy/perf items remain open unless separately verified in code.

*Last updated: 2026-07-07 after quick-win layer verification and tracker
correction.*
