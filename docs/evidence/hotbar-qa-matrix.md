# Hotbar QA Matrix

This matrix is the v0.8.66 release gate for #3401. It ties the shipped Hotbar
MVP to repeatable checks instead of treating isolated unit tests as sufficient
coverage.

## Support Level

| Source | Support level | Release statement |
| --- | --- | --- |
| Built-in app actions | Supported | Default slots dispatch existing app paths through `AppAction` or in-app state mutation. |
| Slash commands | Supported | Argument-free and optional-argument commands dispatch through `commands::execute`; required-argument commands prefill the composer. |
| MCP tools/resources/prompts | Deferred | Visible through the command palette/MCP manager only until argument and approval gates are wired. |
| Skills | Deferred | Visible through command palette and slash skill activation; direct Hotbar binding is deferred until activation receipts are wired. |
| Plugins | Deferred | Visible through `/plugins`; direct Hotbar binding is deferred until plugin approval gates are wired. |

## Config States

| Scenario | Expected behavior | Evidence |
| --- | --- | --- |
| No hotbar config | Default slots resolve to the shipped eight-slot bar. | `crates/config/src/tests.rs::hotbar_defaults_when_config_is_absent` |
| Empty hotbar config | `hotbar = []` disables all default slots. | `crates/config/src/tests.rs::hotbar_empty_array_disables_default_slots`; `crates/tui/src/config_persistence.rs::persist_hotbar_bindings_writes_empty_array_to_disable_defaults` |
| Partial config | Missing slots render empty without filling from defaults. | `crates/tui/src/tui/sidebar.rs::hotbar_panel_slots_handle_empty_partial_and_unknown_config` |
| Unknown actions | Unknown configured actions stay visible as unknown instead of being dropped silently. | `crates/config/src/tests.rs::hotbar_validation_warns_without_dropping_unknown_actions`; `crates/tui/src/tui/sidebar.rs::hotbar_panel_slots_handle_empty_partial_and_unknown_config` |
| Custom labels | Configured labels render and persist with bindings. | `crates/tui/src/tui/hotbar/actions.rs::recommended_hotbar_bindings_serialize_action_ids_and_labels`; `crates/tui/src/tui/ui/tests.rs::hotbar_setup_save_persists_bindings_to_config_path` |
| Workspace overlay | Project config does not replace user-owned Hotbar bindings. | `crates/config/src/tests.rs::project_merge_does_not_replace_user_hotbar_bindings` |
| Legacy/user config path | Fresh setup writes the primary config path; existing comments survive replacement. | `crates/tui/src/config_persistence.rs::persist_hotbar_bindings_writes_primary_config_path_for_fresh_installs`; `crates/tui/src/config_persistence.rs::persist_hotbar_bindings_preserves_comments_and_replaces_existing_tables` |
| Failed persistence | Live config and config file remain unchanged and an error is surfaced. | `crates/tui/src/tui/ui/tests.rs::hotbar_setup_save_error_leaves_live_config_and_file_unchanged` |

## UI States

| Scenario | Expected behavior | Evidence |
| --- | --- | --- |
| Normal TUI/composer | `Alt-1` through `Alt-8` dispatch configured slots; bare digits remain text input. | `crates/tui/src/tui/ui/tests.rs::hotbar_alt_digit_fires_from_composer_and_sidebar_states`; `crates/tui/src/tui/ui/tests.rs::hotbar_bare_digit_inserts_text_even_when_composer_empty` |
| Hidden/sidebar focus states | Hotbar dispatch is still available from hidden, auto, pinned, and focused sidebar states. | `crates/tui/src/tui/ui/tests.rs::hotbar_alt_digit_fires_from_composer_and_sidebar_states` |
| Narrow sidebar | Hotbar panel keeps fixed two-row layout and bounded hover/status text. | `crates/tui/src/tui/sidebar.rs::hotbar_panel_lines_keep_two_fixed_rows_and_hover_status`; `docs/evidence/terminal-visual-regression-matrix.md` |
| Modal/overlay open | Modal, approval, picker, decision-card, and onboarding states block Hotbar numeric ownership. | `crates/tui/src/tui/ui/tests.rs::hotbar_digits_are_blocked_while_modal_or_onboarding_is_active`; `crates/tui/src/tui/ui/tests.rs::hotbar_alt_digit_is_blocked_while_inline_selectors_are_open`; `crates/tui/src/tui/ui/tests.rs::hotbar_alt_digit_is_blocked_while_decision_card_is_active` |
| Setup wizard open/save | Setup lists supported source categories, updates draft bindings, saves, and persists. | `crates/tui/src/tui/hotbar/setup.rs::wizard_sources_follow_registered_action_categories`; `crates/tui/src/tui/hotbar/setup.rs::wizard_save_emits_bindings_but_escape_only_closes`; `crates/tui/src/tui/ui/tests.rs::hotbar_setup_save_persists_bindings_to_config_path` |
| Restart/re-dispatch | Persisted bindings parse back into config and resolve through the same dispatch path. | `crates/config/src/tests.rs::hotbar_tables_parse_and_round_trip`; `crates/tui/src/tui/ui/tests.rs::hotbar_dispatches_bound_slot_and_ignores_empty_slot` |

## Dispatch Outcomes

| Outcome | Expected behavior | Evidence |
| --- | --- | --- |
| Handled in-app | Local UI/state actions mutate app state and mark redraw when needed. | `crates/tui/src/tui/hotbar/actions.rs::sidebar_toggle_reports_visibility_and_dispatches`; `crates/tui/src/tui/hotbar/actions.rs::trust_toggle_reports_trust_state_and_dispatches` |
| `AppAction` return | Actions that must be handled by the event loop return the existing `AppAction`. | `crates/tui/src/tui/hotbar/actions.rs::compact_action_emits_existing_app_action`; `crates/tui/src/tui/ui/tests.rs::hotbar_dispatches_bound_slot_and_ignores_empty_slot` |
| Composer prefill | Required-argument slash commands prefill the composer instead of firing empty args. | `crates/tui/src/tui/hotbar/actions.rs::slash_hotbar_action_prefills_required_argument_command` |
| Disabled reason | Disabled actions are excluded from recommendations and report a reason if manually bound. | `crates/tui/src/tui/hotbar/actions.rs::hotbar_recommendations_exclude_disabled_actions`; `crates/tui/src/tui/ui/tests.rs::hotbar_bound_disabled_action_reports_reason_without_dispatching` |
| Unknown action | Unknown configured action is visible and does not dispatch. | `crates/tui/src/tui/sidebar.rs::hotbar_panel_slots_handle_empty_partial_and_unknown_config`; `crates/tui/src/tui/ui.rs::dispatch_hotbar_slot` |
| Approval-gated/deferred source | Source is explicitly deferred and must not register bindable actions before gates exist. | `crates/tui/src/tui/hotbar/actions.rs::source_descriptors_cover_dispatch_boundaries`; `crates/tui/src/tui/hotbar/actions.rs::deferred_sources_cannot_register_dispatchable_actions` |

## Release Smoke Checklist

Run before claiming Hotbar MVP readiness:

1. `cargo test -p codewhale-config hotbar -- --nocapture`
2. `cargo test -p codewhale-tui --bin codewhale-tui --locked hotbar::actions -- --nocapture`
3. `cargo test -p codewhale-tui --bin codewhale-tui --locked hotbar_setup -- --nocapture`
4. `cargo test -p codewhale-tui --bin codewhale-tui --locked hotbar_panel -- --nocapture`
5. `cargo test -p codewhale-tui --bin codewhale-tui --locked hotbar_alt_digit -- --nocapture`
6. `cargo test -p codewhale-tui --bin codewhale-tui --locked hotbar_dispatch -- --nocapture`

Manual pass, if a release candidate binary is available:

1. Start with no `[hotbar]` config and verify the default eight slots render in
   the sidebar with visible `Alt1` through `Alt8` accelerator labels.
2. Open `/hotbar`, bind a slash command, save, restart, and verify the binding
   persists.
3. Press `Alt-1` through `Alt-8` from composer/sidebar states and verify only
   `Alt` chords dispatch.
4. Open command palette, slash menu, setup wizard, decision card, and an
   approval modal; verify Hotbar digits are blocked while those surfaces own
   input.
5. Confirm MCP, skill, and plugin entries remain discoverable through their
   existing command-palette or slash-command paths and are not offered as direct
   Hotbar bindable actions.
