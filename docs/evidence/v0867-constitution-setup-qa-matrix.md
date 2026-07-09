# v0.8.67 Constitution Setup QA Matrix

This matrix is the release evidence checklist for the v0.8.67
constitution-first setup lane. It ties `/setup`, `/constitution`, doctor,
context reports, and docs to one shared setup-state vocabulary instead of
checking each surface in isolation.

Current-build automated text/render evidence is recorded in
`docs/evidence/v0867-constitution-setup-current-build-evidence.md`.
Guided constitution output examples are recorded in
`docs/evidence/v0867-guided-constitution-examples.md`.

## Gate Commands

Run these before claiming the setup lane is ready:

```sh
cargo fmt --all -- --check
git diff --check
jq empty crates/tui/locales/en.json crates/tui/locales/es-419.json crates/tui/locales/ja.json crates/tui/locales/pt-BR.json crates/tui/locales/vi.json crates/tui/locales/zh-Hans.json
cargo test -p codewhale-tui --bin codewhale-tui --locked setup -- --nocapture
cargo test -p codewhale-tui --bin codewhale-tui --locked constitution -- --nocapture
cargo test -p codewhale-tui --bin codewhale-tui --locked context_report -- --nocapture
cargo test -p codewhale-tui --bin codewhale-tui --locked doctor_setup -- --nocapture
cargo test -p codewhale-tui --bin codewhale-tui --locked tui::onboarding -- --nocapture
RUSTFLAGS="-D warnings" cargo test -p codewhale-tui --bin codewhale-tui --locked --no-run
cargo test -p codewhale-config --lib
```

## Automated Headless Probe

`scripts/v0867-setup-qa.sh` runs the noninteractive contracts below against
isolated temp homes and exits non-zero on any regression (requires `jq`):

```sh
scripts/v0867-setup-qa.sh                       # builds release if needed
CODEWHALE_BIN=target/release/codewhale-tui scripts/v0867-setup-qa.sh
```

It verifies: the `doctor --json .setup` block shape and
`next_actions.constitution`, that a configured key never appears in
`doctor --json`, that a repo `.codewhale/constitution.json` surfaces in
`--context-json`, and that a legacy `WHALE.md` body is never loaded. It
prints the remaining human-visual checks it cannot cover. This shrinks the
manual pass to the visual items enumerated in the Text Snapshot Checklist.

## Hermetic Local Setup

Use temp homes so the matrix does not read or mutate a real install:

```sh
tmp="$(mktemp -d)"
export CODEWHALE_HOME="$tmp/codewhale-home"
export HOME="$tmp/home"
export USERPROFILE="$tmp/home"
export DEEPSEEK_CONFIG_PATH="$CODEWHALE_HOME/config.toml"
mkdir -p "$CODEWHALE_HOME" "$HOME"
```

Useful noninteractive probes:

```sh
cargo run -p codewhale-tui --locked -- doctor --json | jq '.setup'
cargo run -p codewhale-tui --locked -- doctor --context-json | jq '.entries[] | select(.source_kind | test("constitution|project_context_warning"))'
```

## Matrix

| Scenario | Expected behavior | Evidence |
| --- | --- | --- |
| Clean home, bundled/default constitution | First-run can complete by choosing language, recording provider readiness as ready or needs-action, reviewing runtime posture, choosing bundled/default, and opening the setup report. | `/setup` `U` on Constitution step; `crates/tui/src/tui/setup/mod.rs::bundled_constitution_commit_marks_checkpoint_complete`; `doctor --json .setup.constitution.choice == "bundled"` |
| Clean home, guided user-global constitution | Guided custom save writes `$CODEWHALE_HOME/constitution.json`, records source/validity/hash/version/authoring in `setup_state.json`, and previews the rendered block before the ratifying second `G`. | `crates/tui/src/tui/setup/mod.rs::guided_constitution_requires_preview_before_save`; `guided_constitution_answers_shape_preview_and_saved_payload`; `deterministic_ratification_records_guided_authoring`; `persist_user_constitution_choice_writes_constitution_and_state`; `/constitution preview` |
| Model-assisted draft offer gating | The `A` "ask your model to draft" action appears and responds only when the first provider/model route is ready (key/local runtime present); without a ready route the key is inert and the deterministic guided flow is unchanged. | `crates/tui/src/tui/setup/mod.rs::model_draft_key_is_inert_without_a_ready_provider`; `model_draft_key_requests_drafting_with_current_answers`; `constitution_card_gates_the_model_draft_invitation` |
| Model-assisted draft request payload | The one-shot drafting request carries only the six guided answer labels and the UI language tag — no secrets, env, config, repo contents, or memory — plus injection-resistance and advisory-only guardrails in the system prompt. | `crates/tui/src/tui/setup/model_draft.rs::drafting_request_sends_only_answers_and_language`; `drafting_prompts_carry_the_safety_guardrails` |
| Model-assisted draft ingestion | Model output is untrusted: fenced/prose-wrapped JSON parses, invalid or empty output is rejected with a reason, oversized fields are bounded before preview/save, unknown (runtime-policy) keys cannot persist, thinking blocks never reach the parser, and constitution-tag forgery is neutralized. | `crates/tui/src/tui/setup/model_draft.rs` ingestion tests; `crates/config/src/user_constitution.rs::untrusted_draft_*` tests |
| Model-assisted draft failure fallback | Provider construction failure, timeout, request error, or bad JSON degrade to a status line; the deterministic guided draft still previews and ratifies. Decline is the default: not pressing `A` (or tuning `1-6`, which discards a stale draft) keeps the guided path. | `crates/tui/src/tui/ui.rs::handle_setup_constitution_model_draft` error arm; `crates/tui/src/tui/setup/mod.rs::cycling_answers_discards_the_model_draft` |
| Model-assisted ratification | An installed model draft opens the ratification preview immediately; saving still requires the explicit `G`, records `constitution_authoring = model_drafted` plus the bounded draft's preview hash, and persists through the same single `SetupTransaction`. | `crates/tui/src/tui/setup/mod.rs::installed_model_draft_previews_then_ratifies_with_provenance`; `model_drafted_commit_round_trips_through_the_setup_transaction` |
| Existing user update checkpoint | If the v0.8.67 checkpoint is incomplete, interactive launch opens `/setup`; choosing bundled/default is a valid completion. | `crates/tui/src/tui/setup/mod.rs::wizard_resumes_at_constitution_checkpoint_when_update_incomplete`; `crates/tui/src/tui/ui/tests.rs::setup_checkpoint_opens_after_onboarding_when_due` |
| First-run onboarding handoff | Finishing the legacy Welcome/Language/API/trust gates opens setup when the checkpoint is due, instead of landing straight in chat. | `crates/tui/src/tui/ui/tests.rs::setup_checkpoint_opens_after_onboarding_when_due`; onboarding copy tests |
| Existing valid user-global constitution | `/constitution` reports it as active when setup state does not select bundled/deferred/expert override; prompt assembly injects it as a separate block. | `crates/tui/src/prompts.rs::user_global_constitution_block_is_injected_separately`; `/constitution status` |
| Invalid, empty, or unreadable user-global constitution | Invalid data is not injected, `/constitution preview` points to repair, and setup can reopen the Constitution step. | `crates/tui/src/prompts.rs::invalid_user_global_constitution_is_skipped`; `crates/tui/src/commands/groups/core/constitution.rs::constitution_preview_renders_structured_block` |
| Advanced full base-prompt override | Expert override is labeled separately from guided user-global constitution and can suppress stale user-global injection when selected. | `docs/CONFIGURATION.md` expert override section; prompt/setup-state tests for bundled/deferred/expert choices |
| Headless or skip-onboarding launch | Noninteractive/skip-onboarding paths do not hang on the setup checkpoint; doctor/setup JSON reports incomplete state. | `crates/tui/src/tui/ui/tests.rs::setup_checkpoint_waits_for_onboarding_and_skip_flag`; `doctor_setup_report_json_derives_state_without_sidecar` |
| Non-English setup checkpoint | zh-Hans setup/checkpoint copy is usable enough to complete the checkpoint; other full locale files keep setup tips aligned with `/setup` and `/constitution`. | `crates/tui/src/tui/setup/mod.rs::zh_hans_checkpoint_copy_is_localized`; locale JSON `jq empty` gate |
| Runtime posture boundary | Constitution autonomy guidance never mutates `default_mode`, approval policy, sandbox, network, shell, trust, or MCP permissions. | `crates/config/src/user_constitution.rs::autonomy_renders_as_guidance_not_runtime_control`; `crates/tui/src/tui/setup/mod.rs::runtime_posture_review_confirms_without_config_mutation` |
| Provider/model readiness ready | Setup records provider/model as `verified` when auth or local runtime is ready, and the result is a secret-free summary. | `crates/tui/src/tui/setup/mod.rs::provider_model_review_records_ready_route_and_continues` |
| Provider/model missing key | Setup records provider/model as `needs_action` and continues; final report points to `/provider` or `/model`. | `crates/tui/src/tui/setup/mod.rs::provider_model_review_records_missing_auth_as_needs_action`; `doctor --json .setup.next_actions.provider_model` |
| Failed provider health check | A route whose health probe fails records provider/model as `needs_action` with a secret-free `health=needs action` summary; constitution checkpoint completion is not blocked and the report points at the fix. | `crates/tui/src/tui/setup/mod.rs` health derivation (`SetupRuntimeFacts`, `provider_result`); `provider_model_review_records_missing_auth_as_needs_action`; `first_run_ready()` accepts needs-action |
| Migrated legacy `.deepseek` config | A legacy `~/.deepseek` config keeps comments and disabled keys through setup writes; inherited setup state derives from the existing install without regressing configured surfaces; setup stages only user-global paths. | `crates/config/src/tests.rs::config_store_rendered_body_preserves_comments_at_legacy_deepseek_path`; `crates/config/src/setup_state.rs::derive_inherited` tests; hermetic env sets `DEEPSEEK_CONFIG_PATH` above |
| Custom provider/model route | `/model` can record provider-qualified custom routes without confusing them with the active provider only. | `cargo test -p codewhale-tui --bin codewhale-tui --locked model_picker -- --nocapture` |
| MCP/tools configured or skipped | Optional tools/MCP readiness never blocks constitution checkpoint completion and remains represented with shared setup-step status. | `/setup` Tools/MCP row; setup filter gate |
| Hotbar defaulted or customized | Hotbar setup remains independent of constitution setup; setup/hotbar tests cover defaulted and saved bindings. | `docs/evidence/hotbar-qa-matrix.md`; `cargo test -p codewhale-tui --bin codewhale-tui --locked hotbar -- --nocapture` |
| Remote/runtime skipped | Remote runtime remains optional; skipped/deferred state is recorded through `SetupState` rather than blocking first-run. | `/setup` Remote Runtime row; `skip_and_retry_emit_setup_state_commits` |
| WHALE.md migration | Legacy `WHALE.md` is ignored, reported as migration-needed, and its body is not loaded into prompt or context report. | `context_report_marks_whale_md_ignored_without_loading_body`; `constitution_manager_marks_whale_md_ignored` |
| Final setup report is secret-free | Report names constitution choice, provider readiness, runtime posture, skipped/deferred/needs-action steps, and no raw secrets. | `doctor --json .setup`; `verification_report_records_ready_after_bundled_checkpoint`; `step_result_carries_no_secret_by_construction` |

## Text Snapshot Checklist

Capture these snippets in release notes or PR evidence when cutting the release
candidate:

1. Welcome screen opens with the dual meaning of "code" ("Code means two
   things here"), walks the setup arc (choose the model, let it draft the
   constitution it will live under, read and ratify), and states "Nothing
   becomes law until you confirm."
2. `/setup` Provider and Model card shows provider, model, auth state, and
   health without secrets.
3. `/setup` Runtime Posture card says constitution guidance does not change
   runtime policy silently.
4. `/setup` Constitution step shows bundled/default and guided custom actions,
   and — once the provider route is ready — the `A` model-draft invitation
   naming the first configured model.
5. `/constitution` overview shows bundled, user-global, repo-local, AGENTS,
   memory/handoff, preview, and maintenance actions.
6. `/setup report` or `codewhale doctor --json | jq '.setup'` shows
   `constitution`, `runtime_posture_source`, `steps`, and `next_actions`.
7. `doctor --context-json` shows repo constitution or WHALE.md migration
   diagnostics without legacy file bodies.
