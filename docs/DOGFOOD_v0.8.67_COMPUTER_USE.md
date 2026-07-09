# CodeWhale v0.8.67 — Computer-Use / Terminal Agent Dogfood Test Prompt

**Release:** v0.8.67 (2026-07-06) — Fleet/Workflow usability + sub-agent reliability lane  
**Audience:** A model with terminal/computer-use ability that can drive `codewhale-tui` interactively or semi-interactively.  
**Milestone issues covered:** #4050, #4051, #4052, #4053, #4054, #4056, #4057, #4058, #4059, #4062, #4063, plus post-candidate checks #4071-#4076, #4078, #4081, partial #4027, and Constitution / `/workflow` / `/fleet` basics. #4077 remains open/not covered.

---

## Preamble for the testing agent

### What CodeWhale is

CodeWhale is a **terminal-native TUI coding agent**. It runs multi-turn sessions with tool use (read/search/patch/shell), sub-agents (Fleet/delegate), optional workflow orchestration, constitution-driven setup, and provider-agnostic model routing. The shipped runtime binary is **`codewhale-tui`**; `codewhale` is the dispatcher CLI.

### Install and version gate

```bash
codewhale-tui --version
# Expected: codewhale-tui 0.8.67  (or codewhale 0.8.67 if only the dispatcher is on PATH)
```

If the version is wrong, install v0.8.67 before dogfooding:

```bash
curl -fsSL https://codewhale.net/install.sh | sh
# or: npm i -g codewhale@0.8.67
# or: cargo install codewhale-tui --version 0.8.67 --locked --force
```

Record the exact version string in your report.

### Workspace choice

Use **one** of these (pick at start; note which in the report):

| Option | When to use | Setup |
| --- | --- | --- |
| **A — Hermetic temp repo** | Safest default; avoids polluting a real install | See [Hermetic environment](#hermetic-environment) below |
| **B — CodeWhale repo** | Fleet/workflow/sub-agent tests against a real codebase | `cd /path/to/codewhale` with configured provider keys |
| **C — Harness parent + nested clone** | Required for #4052 worktree discovery | Parent dir with nested git checkout one level down (e.g. `Harness/CW/CodeWhale/`) |

### Hermetic environment

Isolate config so dogfood does not touch `~/.codewhale`:

```bash
export DOGFOOD_ROOT="$(mktemp -d)"
export CODEWHALE_HOME="$DOGFOOD_ROOT/codewhale-home"
export HOME="$DOGFOOD_ROOT/home"
export USERPROFILE="$DOGFOOD_ROOT/home"
export DEEPSEEK_CONFIG_PATH="$CODEWHALE_HOME/config.toml"
mkdir -p "$CODEWHALE_HOME" "$HOME"

# Optional: temp git workspace
export WORKSPACE="$DOGFOOD_ROOT/workspace"
mkdir -p "$WORKSPACE" && cd "$WORKSPACE"
git init -q && git commit --allow-empty -q -m "init"
```

For tests needing a real API route, set **one** provider key in the environment (e.g. `DEEPSEEK_API_KEY`, `OPENROUTER_API_KEY`) before launch. Do not commit keys.

### Safety rules

1. **Do not `git push`** to any remote during dogfood.
2. **Do not use YOLO mode** unless a test explicitly says so (YOLO enables shell + trust + auto-approve).
3. Prefer **Plan mode** for read-only UI checks; use **Agent mode** only when a test requires tool execution.
4. Use **`--skip-onboarding`** only after you have intentionally tested onboarding (#4062); otherwise run fresh onboarding at least once.
5. Tear down temp dirs when finished: `rm -rf "$DOGFOOD_ROOT"`.

### How to launch

```bash
# Interactive TUI (primary dogfood surface)
codewhale-tui --workspace "$WORKSPACE"

# Fresh session, skip first-run gates (after onboarding tests)
codewhale-tui --workspace "$WORKSPACE" --skip-onboarding --fresh

# Pre-seed a prompt (still interactive for tools)
codewhale-tui --workspace "$WORKSPACE" --skip-onboarding -p "List files in the workspace"

# Headless probes (non-TUI)
codewhale-tui doctor --json | jq '.setup'
codewhale-tui features list
codewhale exec --help
```

---

## Test matrix

Mark each row **PASS**, **FAIL**, or **SKIP** (with reason). For FAIL, capture a one-line observation and, if possible, a screenshot or transcript snippet.

### Preflight

| ID | Objective | Steps | Expected | Pass criteria |
| --- | --- | --- | --- | --- |
| P0 | Version gate | `codewhale-tui --version` | Prints **0.8.67** | Version matches |
| P1 | Doctor boots | `codewhale-tui doctor --json \| jq '.setup'` | Valid JSON; no crash | Command exits 0; `.setup` object present |
| P2 | Hermetic home | With `CODEWHALE_HOME` set, run doctor | No reads from real `~/.codewhale` unless intentional | Doctor reports isolated paths |

---

### #4050 — Sub-agent empty completion must fail, not succeed

**Issue:** A child that stops on max-steps, tool error, or missing final summary must **not** appear as a silent successful `Completed (no output)`.

| ID | Objective | Steps | Expected | Pass criteria |
| --- | --- | --- | --- | --- |
| 4050-A | Unit regression (headless) | From codewhale repo: `for f in summarize_subagent_result_diagnoses_missing_completed_payload child_hit_max_steps; do cargo test -p codewhale-tui --bin codewhale-tui --locked "$f" -- --nocapture; done` | Tests pass | No test failures |
| 4050-B | Live delegate with forced short budget | In TUI (Agent mode, configured provider): send a message that spawns a sub-agent with a very small scope, e.g. `"Use the agent tool to summarize README.md in one sentence. Set max_steps to 1 if the tool allows."` | History shows **failed** or diagnostic text containing **"no final summary"** — not bare `Completed` with empty payload | No row reads as successful completion with zero useful output |
| 4050-C | Workflow fan-in | Run `/workflow Audit: read Cargo.toml and report the workspace name` and wait for workflow receipt | Failed/missing child slots surface as failures or null with explicit handling; parent receipt does not show `results: []` as if all children succeeded | Parent aggregation distinguishes failed children |

**Headless fallback:** `cargo test -p codewhale-tui --bin codewhale-tui --locked subagent -- --nocapture`

---

### #4051 — Delegate rows show identity, not ellipsis

**Issue:** Delegate/history rows must show role, task prefix, agent id, or `unknown child` — never a bare `…` identity.

| ID | Objective | Steps | Expected | Pass criteria |
| --- | --- | --- | --- | --- |
| 4051-A | Unit regression | `for f in extract_agent_id delegate; do cargo test -p codewhale-tui --bin codewhale-tui --locked "$f" -- --nocapture; done` | Pass | Tests green |
| 4051-B | Burst fan-out UI | In Agent mode: `"Spawn 3 agent tools in parallel: (1) list top-level files, (2) count lines in README, (3) report git status. Use short prompts."` | Each delegate row shows **running** before **done**; identity column is never only `…` | Readable rows with agent id, role, or task slug |
| 4051-C | Completion-before-start recovery | Observe fast completions during burst | If completion arrives early, row still shows meaningful identity (not dropped) | No empty ellipsis-only delegate rows |

---

### #4052 — Worktree nested repo discovery

**Issue:** `worktree: true` from a harness parent directory must discover a one-level nested checkout and return friendly errors when none exists.

| ID | Objective | Steps | Expected | Pass criteria |
| --- | --- | --- | --- | --- |
| 4052-A | Unit regression | `for f in git_repo_root_discovers create_isolated_worktree_discovers; do cargo test -p codewhale-tui --bin codewhale-tui --locked "$f" -- --nocapture; done` | Pass | Tests green |
| 4052-B | Harness layout (manual/TUI) | Create layout: `Harness/` (not a git root) containing nested clone `Harness/CodeWhale/` (real repo). `cd Harness` and launch TUI. Ask agent: `"Use agent tool with worktree:true to run 'git rev-parse --show-toplevel' in an isolated worktree."` | Worktree creation succeeds; discovers nested repo | No raw `not a git repository` from parent harness cwd alone |
| 4052-C | Friendly miss | `cd $(mktemp -d)` (no `.git`), same agent request | Structured error listing **Tried:** paths | Actionable error, not bare git stderr |

---

### #4053 — Budget exhaustion messaging

**Issue:** Token budget exhaustion is a **managed failure** (`budget_exhausted`), not ordinary `done`/success text.

| ID | Objective | Steps | Expected | Pass criteria |
| --- | --- | --- | --- | --- |
| 4053-A | Unit regression | `cargo test -p codewhale-tui --bin codewhale-tui --locked summarize_subagent_result_budget_exhaustion -- --nocapture` | Pass | Summary mentions **partial output preserved** or **retry with a smaller scoped task**; not raw `Token budget exhausted` alone |
| 4053-B | Live exhausted child | Spawn sub-agent with tight token budget (if tool exposes `token_budget`) on a large read task | History card shows budget-exhausted diagnostic; status not plain success | User can tell partial work exists vs total failure |
| 4053-C | Parent aggregation | After workflow with budget-limited children | Parent receipt treats exhaustion as failure/recovery, not verified completion | No "verification passed" framing on exhausted children |

---

### #4054 — Goal `not_applicable` completion

**Issue:** Non-verifiable goals (docs/research/writing) can complete with `verification.status: "not_applicable"` and must stop continuation loops.

| ID | Objective | Steps | Expected | Pass criteria |
| --- | --- | --- | --- | --- |
| 4054-A | Unit regression | `cargo test -p codewhale-tui --bin codewhale-tui --locked update_goal_accepts_not_applicable -- --nocapture` | Pass | Test green |
| 4054-B | Live goal close | `/goal Summarize what CodeWhale is in two sentences for a README intro` → let agent work → ensure it calls goal completion with `not_applicable` verification | Goal sidebar shows **complete**; elapsed timer frozen | Goal becomes inactive |
| 4054-C | No continuation loop | After 4054-B, wait one idle cycle (do not send new messages) | No automatic continuation turn re-injected | Session stays idle after accepted completion |

---

### #4056 — Stable features not labeled experimental

**Issue:** Session Configuration must not mark shipped tools (`mcp`, `web_search`, `apply_patch`, `exec_policy`, `subagents`) as experimental; `vision_model` is **beta**.

| ID | Objective | Steps | Expected | Pass criteria |
| --- | --- | --- | --- | --- |
| 4056-A | Unit regression | `cargo test -p codewhale-tui --bin codewhale-tui --locked config_view_experimental -- --nocapture` | Pass | Tests green |
| 4056-B | Config UI | In TUI: `/config` → scroll to **Experimental** section | Only **beta/experimental** features listed (`vision_model` = beta); stable tools absent from Experimental | No `mcp`/`subagents`/etc. under Experimental |
| 4056-C | Goal/workflow copy | Filter config for `goal` and `workflow` | Copy describes live commands; no "preview placeholder" wording | Professional, accurate descriptions |
| 4056-D | CLI features table | `codewhale-tui features list` (or `codew features list`) | `shell_tool`, `subagents`, `mcp`, etc. show stage **stable** | Matches shipped reality |

---

### #4062 — Provider onboarding (not DeepSeek-only)

**Issue:** First-run onboarding must let users pick a provider and route keys through `save_api_key_for(provider, …)`.

| ID | Objective | Steps | Expected | Pass criteria |
| --- | --- | --- | --- | --- |
| 4062-A | Fresh onboarding flow | New `CODEWHALE_HOME`; launch **without** `--skip-onboarding` | Welcome → Language → **Provider picker** (keys 1–8: DeepSeek, OpenAI, Anthropic, OpenRouter, Z.ai, Moonshot, SiliconFlow, Ollama) | Provider step exists; copy is not DeepSeek-only |
| 4062-B | Non-DeepSeek key routing | Pick provider `4` (OpenRouter) or `2` (OpenAI); paste a test key; complete onboarding | Key stored under correct provider slot in config/secrets | `doctor --json` shows chosen provider ready; key not in `deepseek` slot only |
| 4062-C | Provider-neutral copy | Read API key step title/body | No "Connect your DeepSeek API key" as the only path | Neutral or provider-specific copy matches selection |

**Keystrokes (onboarding):** `Enter` advance · `Esc` back · `1`–`7` language · Provider step: `1`–`8` select provider · Trust: `y`/`n` (plain `Enter` must **not** silently grant trust)

---

### #4063 — Setup wizard scroll (PageDown)

**Issue:** Long setup step bodies must scroll with PageUp/PageDown; scroll resets on step change.

| ID | Objective | Steps | Expected | Pass criteria |
| --- | --- | --- | --- | --- |
| 4063-A | Unit regression | `cargo test -p codewhale-tui --bin codewhale-tui --locked setup_wizard_body_scroll -- --nocapture` | Pass | Test green |
| 4063-B | Open wizard | `/setup` or finish onboarding into setup checkpoint | Setup wizard modal opens | Wizard visible |
| 4063-C | Scroll long step | Select **Constitution** or **Runtime Posture** step; press `PageDown` repeatedly (terminal ≥ 80×24) | Body content scrolls; earlier lines move out of view | Content below fold becomes readable |
| 4063-D | Reset on step change | Scroll down, then `Down`/`Right`/`n` to next step | Scroll position resets to top | New step starts at offset 0 |
| 4063-E | PageUp | After PageDown, press `PageUp` | Scroll moves back up | Bidirectional scroll works |

**Setup wizard keys:** `Esc`/`q` close · `Left`/`b` back step · `Right`/`n` next step · `Up`/`Down` also change steps · `PageUp`/`PageDown` body scroll · `s` skip step · Constitution: `1`–`6` cycle answers, `g` guided save, `a` model draft, `u` bundled/default

---

### #4057 — Locale packs

**Issue:** Shipped UI locales (en, ja, zh-Hans, es-419, pt-BR, vi) must be complete vs `en.json`; **zh-Hant** is intentionally partial and falls back to English.

| ID | Objective | Steps | Expected | Pass criteria |
| --- | --- | --- | --- | --- |
| 4057-A | JSON validity | `jq empty crates/tui/locales/*.json` (from repo) | All files valid JSON | Exit 0 |
| 4057-B | Parity tests | `for f in localization missing_message; do cargo test -p codewhale-tui --bin codewhale-tui --locked "$f" -- --nocapture; done` | Pass | No missing keys for complete packs |
| 4057-C | Non-English setup | Onboarding language: pick `3` (zh-Hans) or `2` (ja); open `/setup` | Setup wizard title/steps in chosen language | UI not English for complete packs |
| 4057-D | zh-Hant partial | Select zh-Hant (Traditional Chinese); open `/setup` | Mixed zh-Hant + English fallback acceptable; not advertised as fully localized | Document if fallback strings appear; no crash |
| 4057-E | Workflow wording | In ja or zh-Hans, run `/workflow` help or slash menu description | Uses workflow terminology; no stale "swarm" wording | Terminology consistent |

**Complete packs (v0.8.67):** en, ja, zh-Hans, es-419, pt-BR, vi. **Partial:** zh-Hant (`Locale::is_partial_pack()`).

---

### #4058 — Model pricing hints (`glm-5.2`, `kimi-k2.7-code`)

**Issue:** Model picker and registry expose current models with pricing metadata where known.

| ID | Objective | Steps | Expected | Pass criteria |
| --- | --- | --- | --- | --- |
| 4058-A | Unit regression | `for f in pricing model_catalog; do cargo test -p codewhale-tui --bin codewhale-tui --locked "$f" -- --nocapture; done` | Pass | glm-5.2 and kimi-k2.7-code pricing tests green |
| 4058-B | Model picker hints | `/model` → search or scroll to `glm-5.2` and `kimi-k2.7-code` (or provider-qualified ids like `z-ai/glm-5.2`, `moonshotai/kimi-k2.7-code`) | Row hint includes **`priced`** (not `price unknown`) when catalog has data | Both models show priced hints |
| 4058-C | Bundled catalog | `jq '.entries["glm-5.2"], .entries["kimi-k2.7-code"]' crates/tui/assets/model_catalog.bundled.json` | Entries exist with metadata | Non-null catalog entries |
| 4058-D | LongCat label | Open provider picker `/provider`; locate LongCat | Labeled as Meituan LongCat (or equivalent professional label) | Provider facts not stale |

---

### #4059 — Spinner on running tool (manual)

**Issue:** A lone running tool should show visible status animation (whale-spout/braille spinner), not a frozen row.

| ID | Objective | Steps | Expected | Pass criteria |
| --- | --- | --- | --- | --- |
| 4059-A | Unit regression | `cargo test -p codewhale-tui --bin codewhale-tui --locked status_animation -- --nocapture` | Pass | Tests green |
| 4059-B | Live slow tool | Agent mode: `"Run a shell command that sleeps 5 seconds: sleep 5"` (approve if prompted) | While running, tool row shows animated spinner / elapsed badge after ~3s (`running (3s)`) | Visible motion or elapsed indicator during run |
| 4059-C | Post-complete | After tool finishes | Spinner stops; final status shown | No permanent spinner |

**Note:** Animation may be subtle in low-motion or screenshot capture; observe live terminal for 2–3 seconds.

---

### Constitution basics

| ID | Objective | Steps | Expected | Pass criteria |
| --- | --- | --- | --- | --- |
| CON-A | Manager opens | `/constitution` | Overview: bundled, user-global, repo-local, preview/maintenance actions | Command works |
| CON-B | Preview | `/constitution preview` | Rendered constitution block (structured) | Non-empty preview |
| CON-C | Setup integration | `/setup` → Constitution step → `u` bundled/default **or** guided `g` flow | Checkpoint can complete; report at `/setup report` or `doctor --json .setup` | `constitution.choice` recorded |
| CON-D | Secret-free doctor | `codewhale-tui doctor --json \| jq '.setup'` | No raw API keys in output | Redaction holds |

**Headless:** `scripts/v0867-setup-qa.sh` (from repo) or `for f in setup constitution doctor_setup; do cargo test -p codewhale-tui --bin codewhale-tui --locked "$f" -- --nocapture; done`

---

### `/workflow` basics

| ID | Objective | Steps | Expected | Pass criteria |
| --- | --- | --- | --- | --- |
| WF-A | Bare invoke | `/workflow` (no args) with prior conversation context | Orchestration message injected; model uses `workflow` tool | Not a one-line dead end |
| WF-B | Explicit objective | `/workflow List all markdown files under docs/` | Run starts; **run card** appears in transcript/history | Visible workflow run UI |
| WF-C | Status | `/workflow status` | Typed status receipt | Returns run info without starting new run |
| WF-D | Cancel copy | `/workflow cancel <run_id>` (if a run exists) | Localized cancel messaging; no "swarm" legacy terms | Professional wording |
| WF-E | Schema mismatch | (Optional) workflow with bad `responseSchema` | Run receipt fails loudly; not null success | Loud failure per #4059 review scope |

**Headless:** `cargo test -p codewhale-workflow -p codewhale-workflow-js --locked` and `cargo test -p codewhale-tui --bin codewhale-tui --locked workflow -- --nocapture`

---

### `/fleet` basics

| ID | Objective | Steps | Expected | Pass criteria |
| --- | --- | --- | --- | --- |
| FL-A | Roster | `/fleet` or `/fleet roster` | Roster view with operator + built-in roles; operator not duplicated | Single operator row |
| FL-B | Setup wizard | `/fleet setup` | Role list has no `main`; model step lists real models + `inherit` | Matches v0.8.67 fleet setup UX |
| FL-C | Status | `/fleet status` | Live worker status view | Opens without error |
| FL-D | Sidebar detail | During active sub-agent/Fleet worker, open Agents sidebar (`Alt-@`) | Detail/hover shows worker **model** route | Model visible in sidebar |
| FL-E | Operator routing | `/fleet roster` | Operator inherits session `/model` route | Not hardcoded `auto` only |

**Headless:** `for f in fleet_roster fleet_setup; do cargo test -p codewhale-tui --bin codewhale-tui --locked "$f" -- --nocapture; done`

---

## TUI interaction notes

### Sending messages and quitting

| Action | Chord / input |
| --- | --- |
| Send message or run slash command | `Enter` |
| Newline without send | `Alt-Enter` or `Ctrl-J` |
| Force steer during running turn | `Ctrl-Enter` (when supported) |
| Quit (empty composer) | `Ctrl-D` |
| Cancel / arm quit | `Ctrl-C` (may require second confirm) |
| Open help | `F1` or `Ctrl+/` |
| Command palette | `Ctrl-K` or type `/` |
| Cycle mode | `Tab` (Plan → Agent → YOLO) |
| Resume session | `Ctrl-R` |

Full catalog: `docs/KEYBINDINGS.md`.

### Slash commands used in this matrix

| Command | Purpose |
| --- | --- |
| `/setup` | Constitution-first setup wizard |
| `/constitution` | Constitution manager |
| `/config` | Session configuration browser |
| `/model` | Model picker + pricing hints |
| `/provider` | Provider picker |
| `/goal` | Session objective tracking |
| `/workflow` | Workflow orchestration opt-in |
| `/fleet` | Fleet roster / setup / status |
| `/mode plan` | Read-only investigation mode |

### Modal focus

When a modal is open (setup wizard, approval, picker), **global hotbar and composer shortcuts may be blocked**. Dismiss with `Esc` before testing global keys.

### Limitations for non-interactive testing

Some behaviors **cannot** be fully verified without a live TUI + model route:

| Area | Interactive TUI | Headless alternative |
| --- | --- | --- |
| Spinner animation (#4059) | Required for visual confirmation | `cargo test … status_animation` |
| Delegate row ordering (#4051) | Required during live fan-out | `cargo test … agent_activity history` |
| Onboarding provider UX (#4062) | Required once | Inspect `ONBOARDING_PROVIDER_OPTIONS` + `save_api_key_for` tests |
| Setup scroll (#4063) | Required for overflow UX | `setup_wizard_body_scroll_resets_on_step_change` test |
| Locale visuals (#4057) | Required for copy QA | `localization`/`missing_message` tests + `jq empty locales/*.json` |
| Workflow/Fleet end-to-end | Requires provider + Agent mode | `codewhale exec --auto` with `--output-format stream-json` for partial traces; unit tests for command routing |
| Sub-agent child completion (#4050–#4053) | Requires Agent + API | `cargo test -p codewhale-tui --bin codewhale-tui --locked -- subagent` |

**CLI automation path:**

```bash
codewhale exec --auto --output-format stream-json "your prompt here"
```

Plain `codewhale exec` is one-shot text only (no tools). Use `--auto` for tool-backed non-interactive runs.

**Recommended regression bundle (from repo root):**

```bash
cargo test -p codewhale-tui --bin codewhale-tui --locked
cargo test -p codewhale-tui --bin codewhale-tui --locked tools::workflow::tests -- --nocapture
for f in subagent setup constitution localization experimental_config pricing model_catalog status_animation fleet_roster fleet_setup; do
  cargo test -p codewhale-tui --bin codewhale-tui --locked "$f" -- --nocapture
done
scripts/v0867-setup-qa.sh
```

---

## Reporting template

Copy this section into your dogfood report and fill it in.

### Run metadata

| Field | Value |
| --- | --- |
| Date | |
| Tester (agent/human) | |
| `codewhale-tui --version` | |
| Platform (OS/arch/terminal) | |
| Workspace option (A/B/C) | |
| Provider used (if any) | |
| `CODEWHALE_HOME` isolated? (y/n) | |

### Results summary

| ID | Result (PASS/FAIL/SKIP) | Notes |
| --- | --- | --- |
| P0 | | |
| P1 | | |
| P2 | | |
| 4050-A/B/C | | |
| 4051-A/B/C | | |
| 4052-A/B/C | | |
| 4053-A/B/C | | |
| 4054-A/B/C | | |
| 4056-A/B/C/D | | |
| 4062-A/B/C | | |
| 4063-A/B/C/D/E | | |
| 4057-A/B/C/D/E | | |
| 4058-A/B/C/D | | |
| 4059-A/B/C | | |
| CON-A/B/C/D | | |
| WF-A/B/C/D/E | | |
| FL-A/B/C/D/E | | |

**Totals:** ___ PASS / ___ FAIL / ___ SKIP

### Blockers

| Severity | ID | Summary | Repro steps | Evidence |
| --- | --- | --- | --- | --- |
| P0 / P1 / P2 | | | | |

### Observations (non-blocking)

- 
- 

### Headless test log (optional)

```
Paste cargo test / scripts/v0867-setup-qa.sh output summary here
```

---

## References

- [CHANGELOG.md](../CHANGELOG.md) — v0.8.67 release notes (2026-07-06)
- [docs/evidence/v0867-constitution-setup-qa-matrix.md](evidence/v0867-constitution-setup-qa-matrix.md)
- [docs/KEYBINDINGS.md](KEYBINDINGS.md)
- [docs/MODES.md](MODES.md)
- [docs/FLEET.md](FLEET.md)
- GitHub milestone issues: #4050–#4054, #4056–#4059, #4062–#4063
