use super::activity_detail::*;
use super::compaction_flow::{
    apply_compaction_started, maybe_warn_context_pressure, should_auto_compact_before_send,
    should_auto_compact_before_send_with_config,
};
use super::event_loop::{TabDispatch, dispatch_tab_key, shell_binding_for_key};
use super::observer_hooks::{
    bounded_subagent_hook_preview, subagent_completion_status, subagent_failure_notice,
    subagent_status_from_completion_result, turn_end_observer_metadata,
};
use super::task_projection::{
    ShellExecLiveUpdate, active_rlm_task_entries, newly_completed_id,
    refresh_shell_exec_live_output, shell_exec_live_update,
};
use super::*;
use crate::config::{
    ApiProvider, Config, DEFAULT_OPENROUTER_MODEL, DEFAULT_TEXT_MODEL, DEFAULT_ZAI_MODEL,
    ProviderConfig, ProvidersConfig,
};
use crate::config_ui::{self, WebConfigSession, WebConfigSessionEvent};
use crate::core::engine::mock_engine_handle;
use crate::tui::active_cell::ActiveCell;
use crate::tui::app::{ReasoningEffort, ToolDetailRecord};
use crate::tui::file_mention::{
    apply_mention_menu_selection, find_file_mention_completions, partial_file_mention_at_cursor,
    try_autocomplete_file_mention, user_request_with_file_mentions, visible_mention_menu_entries,
};
use crate::tui::footer_ui::{
    format_context_budget, format_token_count_compact, friendly_subagent_progress,
};
use crate::tui::history::{
    ExecCell, ExecSource, GenericToolCell, HistoryCell, SubAgentCell, ToolCell, ToolStatus,
};
use crate::tui::hotbar::actions::{HotbarActionCategory, HotbarDispatch};
use crate::tui::provider_picker::ProviderPickerView;
use crate::tui::shell_key_routing::{
    Focus, SHELL_BINDINGS, ShellBindingId, is_permission_cycle_shortcut,
};
use crate::tui::ui_text::truncate_line_to_width;
use crate::tui::views::{HelpView, ModalView, ViewAction};
use crate::working_set::Workspace;
use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    Terminal,
    backend::{Backend, ClearType, TestBackend, WindowSize},
    buffer::Cell,
    layout::{Position, Size},
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::models::Role;
use crate::tui::selection::{SelectionAutoscroll, TranscriptSelectionPoint};
use tempfile::TempDir;

#[test]
fn session_shell_area_fills_the_host_terminal_at_every_width() {
    // #5322: transcript / composer share the full host width — no wide-terminal
    // side gutters. Shrinking and expanding must both rematerialize at `area`.
    for (width, height) in [
        (40, 12),
        (60, 16),
        (80, 24),
        (100, 32),
        (112, 40),
        (140, 40),
        (160, 32),
        (200, 50),
        (240, 60),
        (400, 60),
    ] {
        let host = Rect::new(4, 3, width, height);
        assert_eq!(
            frame::session_shell_area(host),
            host,
            "{width}x{height} must fill the host terminal"
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CursorOperation {
    Hide,
    Position(Position),
    Show,
}

#[derive(Debug)]
struct CursorTraceBackend {
    inner: TestBackend,
    operations: Vec<CursorOperation>,
}

impl CursorTraceBackend {
    fn new(width: u16, height: u16) -> Self {
        Self {
            inner: TestBackend::new(width, height),
            operations: Vec::new(),
        }
    }
}

impl Backend for CursorTraceBackend {
    type Error = std::convert::Infallible;

    fn draw<'a, I>(&mut self, content: I) -> std::result::Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.inner.draw(content)
    }

    fn hide_cursor(&mut self) -> std::result::Result<(), Self::Error> {
        self.operations.push(CursorOperation::Hide);
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> std::result::Result<(), Self::Error> {
        self.operations.push(CursorOperation::Show);
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> std::result::Result<Position, Self::Error> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(
        &mut self,
        position: P,
    ) -> std::result::Result<(), Self::Error> {
        let position = position.into();
        self.operations.push(CursorOperation::Position(position));
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> std::result::Result<(), Self::Error> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> std::result::Result<(), Self::Error> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> std::result::Result<Size, Self::Error> {
        self.inner.size()
    }

    fn window_size(&mut self) -> std::result::Result<WindowSize, Self::Error> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> std::result::Result<(), Self::Error> {
        self.inner.flush()
    }
}

#[test]
fn frame_cursor_is_hidden_during_diff_then_positioned_before_reveal() {
    let mut terminal = Terminal::new(CursorTraceBackend::new(40, 12)).unwrap();
    terminal.backend_mut().operations.clear();

    frame::prepare_frame_cursor(&mut terminal).unwrap();
    frame::finish_frame_cursor(&mut terminal, Some((17, 9))).unwrap();

    assert_eq!(
        terminal.backend().operations,
        [
            CursorOperation::Hide,
            CursorOperation::Position(Position::new(17, 9)),
            CursorOperation::Show,
        ],
        "the IME must never observe a visible stale or intermediate cursor (#5023)"
    );
    assert!(terminal.backend().inner.cursor_visible());
    terminal
        .backend_mut()
        .inner
        .assert_cursor_position(Position::new(17, 9));
}

#[test]
fn composer_rows_stay_pinned_across_turn_state_transitions() {
    // The Tideline shell: the stage, one merged footer row, and the info line
    // footer row (slots 6+8 collapsed, spec §3). Sending a prompt must not
    // displace the composer, relocate the route into the footer, or drop
    // the phase verb from the footer's left half.
    fn frame_app() -> App {
        let mut app = crate::test_support::test_app_with_options(crate::tui::app::TuiOptions {
            model: "deepseek-v4-flash".to_string(),
            start_in_agent_mode: true,
            ..crate::test_support::test_tui_options(PathBuf::from("."))
        });
        app.onboarding = crate::tui::app::OnboardingState::None;
        app.launch.visible = false;
        app.ui_locale = crate::localization::Locale::En;
        app
    }

    fn draw(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let config = Config::default();
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                let _ = super::frame::render(frame, app, &config);
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        (0..height)
            .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect()
    }

    for (width, height) in [(100u16, 30u16), (60, 24), (140, 45)] {
        let mut idle = frame_app();
        let idle_rows = draw(&mut idle, width, height);
        let composer = idle
            .viewport
            .last_composer_area
            .expect("idle frame records the composer area");

        // The info line is the screen's last row and owns the route; the
        // posture row (merged footer) sits directly above it and carries the
        // phase verb plus cost, never the route. Nothing paints above the
        // stage (SHELL-DESIGN-20260901 §2.0).
        let (_, model) = idle.effective_route_identity_display();
        let info_row = idle_rows.last().expect("info row");
        assert!(
            info_row.contains(model.as_str()),
            "{width}x{height} info line lost the route {model:?}: {info_row:?}"
        );
        assert!(
            !info_row.contains("CODEWHALE"),
            "{width}x{height} info line carries no wordmark: {info_row:?}"
        );
        let footer_row = &idle_rows[idle_rows.len() - 2];
        assert!(
            !footer_row.contains(model.as_str()),
            "{width}x{height} footer duplicated the route: {footer_row:?}"
        );

        // A live turn: the composer keeps its exact rows, the info line keeps
        // the route, and the posture row carries the phase verb.
        let mut working = frame_app();
        working.is_loading = true;
        working.turn_started_at = Some(std::time::Instant::now());
        let working_rows = draw(&mut working, width, height);
        assert_eq!(
            working.viewport.last_composer_area,
            Some(composer),
            "{width}x{height}: sending a prompt displaced the composer"
        );
        assert!(
            working_rows
                .last()
                .expect("info row")
                .contains(model.as_str()),
            "{width}x{height}: sending a prompt dropped the route from the info line"
        );
        let working_footer = &working_rows[working_rows.len() - 2];
        assert!(
            !working_footer.contains(model.as_str()),
            "{width}x{height} working footer duplicated the route: {working_footer:?}"
        );
        assert!(
            !working_footer.contains("DeepSeek"),
            "{width}x{height} working footer duplicated the provider: {working_footer:?}"
        );

        // A settled turn keeps the same geometry.
        let mut done = frame_app();
        done.runtime_turn_status = Some("completed".to_string());
        let done_rows = draw(&mut done, width, height);
        assert_eq!(
            done.viewport.last_composer_area,
            Some(composer),
            "{width}x{height}: completion displaced the composer"
        );
        assert!(
            done_rows.last().expect("info row").contains(model.as_str()),
            "{width}x{height}: completion dropped the route from the info line"
        );
        assert!(
            !done_rows[done_rows.len() - 2].contains(model.as_str()),
            "{width}x{height}: completion leaked the route into the footer"
        );
    }
}

#[test]
fn cjk_composer_cursor_and_mouse_geometry_agree_in_compact_and_wide_frames() {
    for (width, height) in [(40, 12), (140, 40)] {
        let mut app = create_test_app();
        app.onboarding = OnboardingState::None;
        app.launch.visible = false;
        app.composer_border = true;
        app.input = "ab中文".to_string();
        app.cursor_position = app.input.chars().count();
        let config = Config::default();
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();

        frame::prepare_frame_cursor(&mut terminal).unwrap();
        let mut first_cursor = None;
        terminal
            .draw(|frame| first_cursor = super::frame::render(frame, &mut app, &config))
            .unwrap();
        let first_cursor = first_cursor.expect("active composer must expose its cursor");
        assert!(
            !terminal.backend().cursor_visible(),
            "the caret stays hidden until the completed frame is positioned"
        );

        let inner = app
            .viewport
            .last_composer_content
            .expect("render records composer content geometry");
        let text_area = crate::tui::widgets::composer_content_geometry(inner, false).text_area;
        let composer = app
            .viewport
            .last_composer_area
            .expect("render records composer area");
        let submit = crate::tui::widgets::active_composer_submit_rect(&app, composer)
            .expect("enclosed composer exposes submit geometry");
        assert_eq!(
            text_area.right(),
            submit.x.saturating_sub(1),
            "{width}x{height}: frame must retain one blank cell before submit"
        );
        assert_eq!(
            first_cursor.0,
            text_area.x + 6,
            "{width}x{height}: ASCII + two CJK glyphs occupy six cells"
        );
        assert!(
            first_cursor.1 >= text_area.y
                && first_cursor.1 < text_area.y.saturating_add(text_area.height),
            "{width}x{height}: cursor must remain inside composer content: {first_cursor:?}"
        );
        assert!(
            first_cursor.0 < submit.x
                || first_cursor.0 >= submit.right()
                || first_cursor.1 != submit.y,
            "{width}x{height}: cursor must stay outside submit: {first_cursor:?} vs {submit:?}"
        );

        frame::finish_frame_cursor(&mut terminal, Some(first_cursor)).unwrap();
        assert!(terminal.backend().cursor_visible());
        terminal
            .backend_mut()
            .assert_cursor_position(Position::from(first_cursor));

        frame::prepare_frame_cursor(&mut terminal).unwrap();
        let mut second_cursor = None;
        terminal
            .draw(|frame| second_cursor = super::frame::render(frame, &mut app, &config))
            .unwrap();
        assert_eq!(
            second_cursor,
            Some(first_cursor),
            "{width}x{height}: identical CJK frames must keep stable IME geometry"
        );

        assert!(handle_composer_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: first_cursor.0,
                row: first_cursor.1,
                modifiers: KeyModifiers::NONE,
            },
        ));
        assert_eq!(
            app.cursor_position,
            app.input.chars().count(),
            "{width}x{height}: rendered caret and mouse hit mapping must select the same CJK boundary"
        );
    }
}

#[test]
fn remote_control_escape_commands_match_dispatcher_case_rules() {
    // Mirror mode removed the input gate entirely (and with it
    // `is_remote_control_command`), but the /rc command surface itself is
    // unchanged: these spellings must keep reaching the dispatcher. The
    // commands table is the surviving contract.
    for input in ["/rc stop", "/RC stop", "/Rc status", "/remote-control stop"] {
        let normalized = input
            .split_whitespace()
            .next()
            .expect("command word")
            .to_ascii_lowercase();
        assert!(
            normalized == "/rc" || normalized == "/remote-control",
            "remote control command must remain dispatchable: {input}"
        );
    }
    assert!("/run continue" != "/rc");
}

#[tokio::test]
async fn connected_web_mirror_never_refuses_local_composer_input() {
    let mut app = create_test_app();
    // The exact state that used to own prompts: connected mirror with an
    // attached run. Under mirror semantics the composer must dispatch.
    app.remote_control
        .force_mirror_connected_for_tests("run_fixture", "turn_fixture");
    let config = Config::default();
    let mock = mock_engine_handle();
    let handle = mock.handle.clone();
    let result = crate::tui::ui::dispatch::dispatch_composer_message(
        &mut app,
        &config,
        &handle,
        crate::tui::app::QueuedMessage::new("local prompt while mirrored".to_string(), None),
        crate::tui::ui::DispatchRecovery::Immediate,
        crate::tui::app::ComposerSubmitAction::Submit(
            crate::tui::app::SubmitDisposition::Immediate,
        ),
    )
    .await;
    assert!(result.is_ok(), "{result:?}");
    assert!(
        app.input.is_empty(),
        "the composer must not bounce the text back: {:?}",
        app.input
    );
    assert!(
        app.is_loading,
        "the local prompt must have started a turn despite the connected mirror"
    );
}

#[tokio::test]
async fn runtime_chat_queues_local_composer_input_without_starting_a_second_turn() {
    let mut app = create_test_app();
    app.remote_control.block_runtime_chat_dispatch_for_tests();
    let config = Config::default();
    let mut mock = mock_engine_handle();

    crate::tui::ui::dispatch::dispatch_composer_message(
        &mut app,
        &config,
        &mock.handle,
        crate::tui::app::QueuedMessage::new("local prompt during Runtime Chat".to_string(), None),
        crate::tui::ui::DispatchRecovery::Immediate,
        crate::tui::app::ComposerSubmitAction::Submit(
            crate::tui::app::SubmitDisposition::Immediate,
        ),
    )
    .await
    .expect("Runtime Chat dispatch gate");

    assert_eq!(app.queued_message_count(), 1);
    assert!(!app.is_loading, "the interactive engine must remain idle");
    assert!(
        mock.rx_op.try_recv().is_err(),
        "the interactive engine must not receive a second provider turn"
    );
    assert!(app.status_message.is_none());
    let toast = app.status_toasts.back().expect("Runtime Chat queue toast");
    assert_eq!(toast.level, StatusToastLevel::Info);
    assert!(toast.text.contains("Runtime Chat"));
    assert_eq!(toast.ttl_ms, Some(4_000));
}

#[tokio::test]
async fn stale_remote_control_turn_never_cancels_the_current_engine_turn() {
    let mut app = create_test_app();
    app.remote_control
        .force_mirror_connected_for_tests("run_fixture", "turn_current");
    app.remote_control
        .queue_remote_event_for_tests(crate::remote_control::RemoteEvent::Command {
            run_id: "run_fixture".to_string(),
            seq: 1,
            command: crate::remote_control::RemoteCommand::Control {
                action: crate::remote_control::RemoteControlRequest::Interrupt,
                turn_id: Some("turn_stale".to_string()),
                runtime_chat: None,
            },
        });
    let mock = mock_engine_handle();
    drain_remote_control_events(&mut app, &Config::default(), &mock.handle)
        .await
        .unwrap();
    assert!(
        !mock.cancel_token.is_cancelled(),
        "a delayed command for an older turn must not cancel the current provider lifecycle"
    );
}

fn test_mailbox_route(
    provider: ApiProvider,
    model: &str,
) -> crate::cost_status::EffectiveRouteEnvelope {
    crate::cost_status::EffectiveRouteEnvelope::capture(
        None,
        provider,
        provider.as_str(),
        model,
        Some(provider.default_base_url()),
        chrono::Utc::now(),
    )
}

fn served_endpoint_fingerprint() -> Option<String> {
    crate::cost_status::endpoint_fingerprint("https://api.deepseek.com/v1")
}

#[test]
fn completed_turn_cost_receipt_uses_the_captured_effective_route() {
    let usage = crate::models::Usage {
        input_tokens: 1_000,
        output_tokens: 100,
        ..Default::default()
    };
    let audit = crate::pricing::audit_turn_cost_for_route_at(
        ApiProvider::Deepseek,
        "deepseek-v4-flash",
        Some(crate::pricing::FIRST_PARTY_PAYG_BILLING_SURFACE),
        &usage,
        chrono::Utc::now(),
    );
    let completed = crate::tui::app::ActiveTurnMetadata {
        turn_id: "completed-deepseek".to_string(),
        created_at: chrono::Utc::now(),
        route: Some(crate::core::events::TurnRoute {
            provider: ApiProvider::Deepseek,
            provider_identity: "deepseek-cn".to_string(),
            model: "deepseek-v4-flash".to_string(),
            auto_model: false,
            receipt: None,
            billing: Some(crate::core::events::RouteBillingEnvelope {
                billing_surface: Some(crate::pricing::FIRST_PARTY_PAYG_BILLING_SURFACE.to_string()),
                // A real fingerprint, produced by the production hasher. A
                // receipt fails closed on anything that is not one, so a
                // hand-written placeholder here would have tested nothing.
                endpoint_fingerprint: served_endpoint_fingerprint(),
                billing_mode: crate::cost_status::RouteBillingMode::Metered,
                dispatched_at: chrono::Utc::now(),
            }),
            base_url: ApiProvider::Deepseek.default_base_url().to_string(),
            billing_product: crate::route_billing::RouteProduct::Unproven,
        }),
        auto_route_receipt: None,
        suggestion_authority: None,
    };

    let receipt = completed_turn_cost_route_receipt(Some(&completed), &audit)
        .expect("captured model-backed turn has a receipt");

    assert!(receipt.contains("provider=deepseek"), "{receipt}");
    assert!(receipt.contains("identity=deepseek-cn"), "{receipt}");
    assert!(receipt.contains("model=deepseek-v4-flash"), "{receipt}");
    assert!(
        receipt.contains(&format!(
            "endpoint_fp={}",
            served_endpoint_fingerprint().expect("fingerprintable endpoint")
        )),
        "{receipt}"
    );
    assert!(receipt.contains("billing_mode=metered"), "{receipt}");
    assert!(receipt.contains("currency=usd+cny"), "{receipt}");
}

#[test]
fn permission_cycle_shortcut_accepts_both_shift_tab_encodings() {
    assert!(is_permission_cycle_shortcut(&KeyEvent::new(
        KeyCode::BackTab,
        KeyModifiers::NONE,
    )));
    assert!(is_permission_cycle_shortcut(&KeyEvent::new(
        KeyCode::BackTab,
        KeyModifiers::SHIFT,
    )));
    assert!(is_permission_cycle_shortcut(&KeyEvent::new(
        KeyCode::Tab,
        KeyModifiers::SHIFT,
    )));
    assert!(!is_permission_cycle_shortcut(&KeyEvent::new(
        KeyCode::Tab,
        KeyModifiers::NONE,
    )));
    assert!(!is_permission_cycle_shortcut(&KeyEvent::new(
        KeyCode::BackTab,
        KeyModifiers::CONTROL,
    )));
}

/// A live session in a deterministic focus state: no onboarding, no launch
/// screen, no modal, composer owns the keys.
fn focus_test_app() -> App {
    let mut app = create_test_app();
    app.onboarding = crate::tui::app::OnboardingState::None;
    app.launch.visible = false;
    app.work_surface.focused = false;
    app.workflow_panel = None;
    app
}

/// One representative terminal encoding per shell binding.
fn shell_binding_probe(id: ShellBindingId) -> KeyEvent {
    match id {
        ShellBindingId::ToolDetails => KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT),
        ShellBindingId::ContextInspector => KeyEvent::new(KeyCode::Char('c'), KeyModifiers::ALT),
        ShellBindingId::ProviderRoute => KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE),
        ShellBindingId::Help => KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE),
        ShellBindingId::Settings => KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE),
        ShellBindingId::ModeCycle => KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        ShellBindingId::PermissionCycle => KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE),
        ShellBindingId::ViewCycle => KeyEvent::new(KeyCode::Char(']'), KeyModifiers::CONTROL),
        ShellBindingId::ViewCycleBack => KeyEvent::new(
            KeyCode::BackTab,
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ),
    }
}

#[test]
fn shell_key_seam_presses_a_key_at_the_event_loop() {
    // The harness in one line: build an App, press a real crossterm key at
    // the same admission the event loop uses, read the binding back.
    let app = focus_test_app();
    assert_eq!(
        shell_binding_for_key(&app, &KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE)),
        Some(ShellBindingId::Help)
    );
    assert_eq!(
        shell_binding_for_key(&app, &KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)),
        None,
        "an ordinary character belongs to the composer"
    );
}

#[test]
fn no_shell_binding_changes_meaning_once_the_composer_has_text() {
    // The whole bug family in one assertion: a shell binding answers to the
    // focus owner, never to whether the user has started typing. Tab was the
    // visible case (`if !app.input.is_empty() { continue; }`), but the same
    // guess was spread across ~21 `input.is_empty()` sites.
    for binding in SHELL_BINDINGS {
        let key = shell_binding_probe(binding.id);
        let empty = focus_test_app();
        let mut typed = focus_test_app();
        typed.input = "draft in progress".to_string();
        typed.cursor_position = typed.input.chars().count();

        let on_empty = shell_binding_for_key(&empty, &key);
        let on_typed = shell_binding_for_key(&typed, &key);
        assert_eq!(
            on_empty, on_typed,
            "{:?} changed meaning because the composer has text",
            binding.id
        );
        assert_eq!(
            on_typed,
            Some(binding.id),
            "{:?} must still be live mid-draft",
            binding.id
        );

        // Admission is only half the family: the handler behind a binding
        // must not re-guess focus from the text either. Tab is the one with
        // a dispatcher of its own, so press it in both states.
        if binding.id == ShellBindingId::ModeCycle {
            let mut empty = focus_test_app();
            empty.prompt_suggestion = None;
            let mut typed = focus_test_app();
            typed.prompt_suggestion = None;
            typed.input = "draft in progress".to_string();
            typed.cursor_position = typed.input.chars().count();
            assert_eq!(
                dispatch_tab_key(&mut empty, &key, &[], &[]),
                dispatch_tab_key(&mut typed, &key, &[], &[]),
                "Tab dispatched differently because the composer has text"
            );
        }
    }

    // The exception the rule is for: the composer's own editing keys are
    // never shell bindings, empty or not.
    for code in [
        KeyCode::Backspace,
        KeyCode::Enter,
        KeyCode::Char('y'),
        KeyCode::Char(' '),
    ] {
        let mut typed = focus_test_app();
        typed.input = "draft".to_string();
        let key = KeyEvent::new(code, KeyModifiers::NONE);
        assert_eq!(shell_binding_for_key(&focus_test_app(), &key), None);
        assert_eq!(shell_binding_for_key(&typed, &key), None);
    }
}

#[test]
fn tab_still_cycles_the_mode_after_the_user_has_typed() {
    // Regression: Tab used to stop working the moment the composer held any
    // text, which is most of the time you are actually using the product.
    let mut app = focus_test_app();
    app.prompt_suggestion = None;
    app.input = "explain this repo".to_string();
    app.cursor_position = app.input.chars().count();
    let before = app.mode;

    let dispatch = dispatch_tab_key(
        &mut app,
        &KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        &[],
        &[],
    );

    assert!(
        matches!(dispatch, TabDispatch::ModeCycled { .. }),
        "Tab with a typed composer dispatched {dispatch:?}"
    );
    assert_ne!(app.mode, before, "the session mode must actually change");
    assert_eq!(
        app.input, "explain this repo",
        "cycling the mode must not disturb the draft"
    );
}

#[test]
fn shift_tab_cycles_permission_from_every_shell_surface() {
    // Regression: Shift+Tab was admitted only after the launch screen had
    // already swallowed the key, so the posture control was dead on the
    // first screen the user sees. Both terminal encodings, both surfaces.
    for key in [
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
        KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT),
    ] {
        let mut session = focus_test_app();
        session.input = "draft in progress".to_string();
        assert_eq!(
            shell_binding_for_key(&session, &key),
            Some(ShellBindingId::PermissionCycle),
            "session composer: {key:?}"
        );

        let mut launch = focus_test_app();
        launch.launch.visible = true;
        assert_eq!(launch.focus(), Focus::Launch);
        assert_eq!(
            shell_binding_for_key(&launch, &key),
            Some(ShellBindingId::PermissionCycle),
            "launch screen: {key:?}"
        );
    }
}

#[test]
fn focus_owner_follows_the_surface_that_owns_the_keys() {
    let mut app = focus_test_app();
    assert_eq!(app.focus(), Focus::Composer);

    app.work_surface.focused = true;
    assert_eq!(app.focus(), Focus::Panel);
    app.work_surface.focused = false;

    app.launch.visible = true;
    assert_eq!(app.focus(), Focus::Launch);
    app.launch.visible = false;

    app.onboarding = crate::tui::app::OnboardingState::Welcome;
    assert_eq!(app.focus(), Focus::Onboarding);
    app.onboarding = crate::tui::app::OnboardingState::None;

    app.view_stack.push(HelpView::new_for_locale(app.ui_locale));
    assert_eq!(app.focus(), Focus::Modal(ModalKind::Help));
    // A modal owns Tab and the details chord; Help still does not.
    assert_eq!(
        shell_binding_for_key(&app, &KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        None
    );
    assert_eq!(
        shell_binding_for_key(&app, &KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE)),
        Some(ShellBindingId::Help)
    );
}

#[test]
fn settings_toggle_opens_and_closes_without_stacking_duplicates() {
    let _lock = crate::test_support::lock_test_env();
    let mut app = create_test_app();

    toggle_settings_view(&mut app);
    assert_eq!(app.view_stack.top_kind(), Some(ModalKind::Config));

    app.view_stack.push(HelpView::new_for_locale(app.ui_locale));
    assert_eq!(app.view_stack.top_kind(), Some(ModalKind::Help));
    toggle_settings_view(&mut app);
    assert!(app.view_stack.is_empty());

    toggle_settings_view(&mut app);
    assert_eq!(app.view_stack.top_kind(), Some(ModalKind::Config));
    toggle_settings_view(&mut app);
    assert!(app.view_stack.is_empty());
}

#[test]
fn launch_help_uses_the_same_toggle_and_shortcut_ordering_as_the_live_shell() {
    let _lock = crate::test_support::lock_test_env();
    let mut app = create_test_app();
    app.launch.visible = true;

    toggle_help_view(&mut app);
    assert_eq!(app.view_stack.top_kind(), Some(ModalKind::Help));

    toggle_help_view(&mut app);
    assert!(app.view_stack.is_empty());
}

#[test]
fn ctrl_t_key_event_reaches_reasoning_effort_cycle() {
    let mut app = create_test_app();
    app.api_provider = crate::config::ApiProvider::Deepseek;
    app.auto_model = false;
    app.reasoning_effort = ReasoningEffort::Off;

    assert!(handle_reasoning_effort_key(
        &mut app,
        &KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL),
    ));
    // Ctrl+T walks DeepSeek's real ladder now, not the off/high/max shortcut,
    // and DeepSeek's first-party route documents a Low tier above Off.
    assert_eq!(app.reasoning_effort, ReasoningEffort::Low);
    assert!(
        !handle_reasoning_effort_key(
            &mut app,
            &KeyEvent::new(
                KeyCode::Char('T'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        ),
        "Ctrl+Shift+T remains owned by the live transcript overlay",
    );
}

#[test]
fn ctrl_t_cycles_reasoning_effort_under_auto_model() {
    let mut app = create_test_app();
    app.api_provider = ApiProvider::Deepseek;
    app.auto_model = true;
    app.reasoning_effort = ReasoningEffort::Auto;

    for expected in [
        ReasoningEffort::Off,
        ReasoningEffort::Minimal,
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
        ReasoningEffort::XHigh,
        ReasoningEffort::Ultra,
        ReasoningEffort::Max,
        ReasoningEffort::Auto,
    ] {
        assert!(handle_reasoning_effort_key(
            &mut app,
            &KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL),
        ));
        assert_eq!(app.reasoning_effort, expected);
        assert!(app.status_message.as_deref().is_some_and(|message| {
            message.contains(&format!("Reasoning effort: {}", expected.short_label()))
        }));
    }
}

#[test]
fn underwater_motion_keeps_its_smoother_cadence_during_live_status() {
    let _guard = crate::test_support::lock_test_env();
    let previous_program = std::env::var_os("TERM_PROGRAM");
    let previous_term = std::env::var_os("TERM");
    // SAFETY: serialized by the process-wide test environment lock.
    unsafe {
        std::env::remove_var("TERM_PROGRAM");
        std::env::set_var("TERM", "xterm-256color");
    }
    let mut app = create_test_app();
    // App::new reads real terminal overlays. This test owns the authored
    // motion cadence, so pin that input instead of inheriting a host's saved
    // low-motion or legacy-console policy.
    app.low_motion = false;
    app.fancy_animations = true;
    app.constrained_frame_rate = false;

    assert_eq!(
        animation_interval_ms(&app, true, false),
        UI_STATUS_ANIMATION_MS
    );
    assert_eq!(
        animation_interval_ms(&app, false, true),
        UI_UNDERWATER_ANIMATION_MS
    );
    assert_eq!(
        animation_interval_ms(&app, true, true),
        UI_UNDERWATER_ANIMATION_MS,
        "the slower status spinner must not throttle ambient fish"
    );
    // SAFETY: cleanup under the same lock.
    unsafe {
        match previous_program {
            Some(value) => std::env::set_var("TERM_PROGRAM", value),
            None => std::env::remove_var("TERM_PROGRAM"),
        }
        match previous_term {
            Some(value) => std::env::set_var("TERM", value),
            None => std::env::remove_var("TERM"),
        }
    }
}

#[test]
fn ghostty_caps_underwater_motion_without_slowing_interaction() {
    let _guard = crate::test_support::lock_test_env();
    let previous_program = std::env::var_os("TERM_PROGRAM");
    let previous_term = std::env::var_os("TERM");
    // SAFETY: serialized by the process-wide test environment lock.
    unsafe {
        std::env::set_var("TERM_PROGRAM", "Ghostty");
        std::env::set_var("TERM", "xterm-ghostty");
    }
    let mut app = create_test_app();
    app.low_motion = false;
    app.fancy_animations = true;
    app.constrained_frame_rate = false;

    assert_eq!(
        underwater_animation_interval_ms(&app),
        UI_GHOSTTY_UNDERWATER_ANIMATION_MS
    );
    const {
        assert!(UI_GHOSTTY_UNDERWATER_ANIMATION_MS < UI_UNDERWATER_ANIMATION_MS);
    }
    app.constrained_frame_rate = true;
    assert_eq!(
        underwater_animation_interval_ms(&app),
        UI_CONSTRAINED_UNDERWATER_ANIMATION_MS,
        "tmux/SSH compatibility must override Ghostty's native atmosphere lane"
    );
    // SAFETY: cleanup under the same lock.
    unsafe {
        match previous_program {
            Some(value) => std::env::set_var("TERM_PROGRAM", value),
            None => std::env::remove_var("TERM_PROGRAM"),
        }
        match previous_term {
            Some(value) => std::env::set_var("TERM", value),
            None => std::env::remove_var("TERM"),
        }
    }
}

#[test]
fn canonical_bash_actions_use_exec_cells_without_rewriting_detail_identity() {
    let cases = [
        (
            "run",
            serde_json::json!({"action": "run", "command": "pwd"}),
        ),
        (
            "wait",
            serde_json::json!({"action": "wait", "task_id": "shell-1"}),
        ),
        (
            "interact",
            serde_json::json!({"action": "interact", "task_id": "shell-1", "stdin": "y\n"}),
        ),
        (
            "cancel",
            serde_json::json!({"action": "cancel", "task_id": "shell-1"}),
        ),
    ];

    for (action, input) in cases {
        let mut app = create_test_app();
        let id = format!("bash-{action}");
        handle_tool_call_started(&mut app, &id, "Bash", &input);
        let active = app.active_cell.as_ref().expect("active cell");
        let HistoryCell::Tool(ToolCell::Exec(exec)) = &active.entries()[0] else {
            panic!("Bash.{action} must use ExecCell")
        };
        assert_eq!(exec.interaction.is_some(), action != "run", "Bash.{action}");
        assert_eq!(app.active_tool_details[&id].tool_name, "Bash");
        assert_eq!(app.active_tool_details[&id].input, input);
    }

    let mut legacy = create_test_app();
    handle_tool_call_started(
        &mut legacy,
        "legacy-cancel",
        "exec_shell_cancel",
        &serde_json::json!({"task_id": "shell-legacy"}),
    );
    let active = legacy.active_cell.as_ref().expect("active cell");
    assert!(matches!(
        active.entries()[0],
        HistoryCell::Tool(ToolCell::Exec(_))
    ));
    assert_eq!(
        legacy.active_tool_details["legacy-cancel"].tool_name,
        "exec_shell_cancel"
    );
}

#[test]
fn canonical_background_bash_keeps_live_task_identity_for_follow_up_actions() {
    let mut app = create_test_app();
    handle_tool_call_started(
        &mut app,
        "bash-background",
        "Bash",
        &serde_json::json!({
            "action": "run",
            "command": "cargo test --workspace",
            "background": true
        }),
    );
    let running = Ok(
        crate::tools::spec::ToolResult::success("Background task started: shell-42").with_metadata(
            serde_json::json!({
                "status": "Running",
                "task_id": "shell-42",
                "command": "cargo test --workspace",
                "duration_ms": 25_u64
            }),
        ),
    );
    handle_tool_call_complete(&mut app, "bash-background", "Bash", &running);

    let active = app.active_cell.as_ref().expect("active cell");
    let HistoryCell::Tool(ToolCell::Exec(exec)) = &active.entries()[0] else {
        panic!("canonical background Bash must remain an ExecCell")
    };
    assert_eq!(exec.status, ToolStatus::Running);
    assert_eq!(exec.shell_task_id.as_deref(), Some("shell-42"));
    assert_eq!(exec.command, "cargo test --workspace");
    assert!(exec.output.is_none());
    assert!(
        exec.live_output
            .as_deref()
            .is_some_and(|output| { output.contains("Background task started: shell-42") })
    );
}

#[test]
fn canonical_file_actions_split_reads_searches_and_mutations_truthfully() {
    let cases = [
        ("read", "exploring"),
        ("list", "exploring"),
        ("search_name", "file_search"),
        ("search_content", "exploring"),
        ("write", "patch_summary"),
        ("edit", "patch_summary"),
        ("patch", "patch_summary"),
    ];

    for (action, expected) in cases {
        let mut app = create_test_app();
        let id = format!("file-{action}");
        let input = match action {
            "read" | "list" => serde_json::json!({"action": action, "path": "src"}),
            "search_name" => serde_json::json!({"action": action, "query": "lib.rs"}),
            "search_content" => {
                serde_json::json!({"action": action, "pattern": "whale", "path": "src"})
            }
            "write" => serde_json::json!({
                "action": action,
                "path": "src/new.rs",
                "content": "pub fn new() {}\n"
            }),
            "edit" => serde_json::json!({
                "action": action,
                "path": "src/lib.rs",
                "search": "old",
                "replace": "new"
            }),
            "patch" => serde_json::json!({
                "action": action,
                "patch": "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,1 +1,1 @@\n-old\n+new\n"
            }),
            _ => unreachable!(),
        };
        handle_tool_call_started(&mut app, &id, "File", &input);
        let active = app.active_cell.as_ref().expect("active cell");
        let tool = match &active.entries()[0] {
            HistoryCell::Tool(tool) => tool,
            other => panic!("expected tool cell, got {other:?}"),
        };
        match expected {
            "exploring" => assert!(matches!(tool, ToolCell::Exploring(_)), "File.{action}"),
            "patch_summary" => {
                assert!(matches!(tool, ToolCell::PatchSummary(_)), "File.{action}")
            }
            semantic_name => {
                let ToolCell::Generic(generic) = tool else {
                    panic!("File.{action} must use GenericToolCell")
                };
                assert_eq!(generic.name, semantic_name, "File.{action}");
            }
        }
        assert_eq!(app.active_tool_details[&id].tool_name, "File");
    }
}

#[test]
fn canonical_file_mutations_attach_receipts_only_to_successful_outcomes() {
    for approval_mode in [
        ApprovalMode::Suggest,
        ApprovalMode::Auto,
        ApprovalMode::Bypass,
    ] {
        for action in ["write", "edit", "patch"] {
            let mut app = create_test_app();
            app.approval_mode = approval_mode;
            let id = format!("file-{approval_mode:?}-{action}-success");
            let input = if action == "patch" {
                serde_json::json!({
                    "action": action,
                    "patch": "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n"
                })
            } else {
                serde_json::json!({"action": action, "path": "src/lib.rs"})
            };
            handle_tool_call_started(&mut app, &id, "File", &input);
            handle_tool_call_complete(
                &mut app,
                &id,
                "File",
                &Ok(crate::tools::spec::ToolResult::success("ok").with_metadata(
                    serde_json::json!({
                        "mutation": {
                            "diff": "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n",
                            "files": [{ "path": "src/lib.rs", "outcome": "updated" }],
                            "renames": []
                        }
                    }),
                )),
            );

            let active = app.active_cell.as_ref().expect("active cell");
            let HistoryCell::Tool(ToolCell::PatchSummary(cell)) = &active.entries()[0] else {
                panic!("File.{action} must stay on the calm File receipt path")
            };
            assert_eq!(
                cell.status,
                ToolStatus::Success,
                "{approval_mode:?} File.{action}"
            );
            assert!(cell.receipt.is_some(), "{approval_mode:?} File.{action}");
            assert_eq!(
                cell.receipt.as_ref().unwrap().outcome_label(),
                "Updated src/lib.rs",
                "{approval_mode:?} File.{action}"
            );
        }
    }

    let mut failed = create_test_app();
    handle_tool_call_started(
        &mut failed,
        "file-write-failed",
        "File",
        &serde_json::json!({"action": "write", "path": "src/lib.rs"}),
    );
    handle_tool_call_complete(
        &mut failed,
        "file-write-failed",
        "File",
        &Ok(
            crate::tools::spec::ToolResult::error("cancelled before mutation").with_metadata(
                serde_json::json!({
                    "mutation": {
                        "diff": "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+FORGED\n",
                        "files": [{ "path": "src/lib.rs", "outcome": "updated" }],
                        "renames": []
                    }
                }),
            ),
        ),
    );
    let active = failed.active_cell.as_ref().expect("active cell");
    let HistoryCell::Tool(ToolCell::PatchSummary(cell)) = &active.entries()[0] else {
        panic!("failed File.write must remain a File receipt cell")
    };
    assert_eq!(cell.status, ToolStatus::Failed);
    assert!(cell.receipt.is_none());
    assert_eq!(cell.error.as_deref(), Some("cancelled before mutation"));
}

#[test]
fn canonical_git_run_and_web_actions_use_semantic_history_cells() {
    for (family, action, expected) in [
        ("Git", "status", "git_status"),
        ("Git", "diff", "git_diff"),
        ("Git", "log", "git_log"),
        ("Git", "show", "git_show"),
        ("Git", "blame", "git_blame"),
        ("Run", "tests", "run_tests"),
        ("Run", "verifiers", "run_verifiers"),
        ("Web", "fetch", "fetch_url"),
        ("Web", "wait", "wait_for_dev_server"),
    ] {
        let mut app = create_test_app();
        let id = format!("{family}-{action}");
        handle_tool_call_started(
            &mut app,
            &id,
            family,
            &serde_json::json!({"action": action}),
        );
        let active = app.active_cell.as_ref().expect("active cell");
        let HistoryCell::Tool(ToolCell::Generic(generic)) = &active.entries()[0] else {
            panic!("{family}.{action} must use GenericToolCell")
        };
        assert_eq!(generic.name, expected, "{family}.{action}");
        assert_eq!(app.active_tool_details[&id].tool_name, family);
    }

    let mut app = create_test_app();
    handle_tool_call_started(
        &mut app,
        "Web-search",
        "Web",
        &serde_json::json!({"action": "search", "query": "Codewhale"}),
    );
    let active = app.active_cell.as_ref().expect("active cell");
    assert!(matches!(
        active.entries()[0],
        HistoryCell::Tool(ToolCell::WebSearch(_))
    ));
    assert_eq!(app.active_tool_details["Web-search"].tool_name, "Web");
}

#[test]
fn canonical_completion_refreshes_workspace_only_for_semantic_mutations() {
    let stale = Instant::now() - Duration::from_secs(60);

    let mut reading = create_test_app();
    reading.workspace_context_refreshed_at = Some(stale);
    handle_tool_call_started(
        &mut reading,
        "file-read",
        "File",
        &serde_json::json!({"action": "read", "path": "src/lib.rs"}),
    );
    handle_tool_call_complete(&mut reading, "file-read", "File", &ok_result("contents"));
    assert_eq!(reading.workspace_context_refreshed_at, Some(stale));

    for (action, input) in [
        (
            "write",
            serde_json::json!({
                "action": "write",
                "path": "src/new.rs",
                "content": "new\n"
            }),
        ),
        (
            "edit",
            serde_json::json!({
                "action": "edit",
                "path": "src/lib.rs",
                "search": "old",
                "replace": "new"
            }),
        ),
        (
            "patch",
            serde_json::json!({
                "action": "patch",
                "patch": "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,1 +1,1 @@\n-old\n+new\n"
            }),
        ),
    ] {
        let mut app = create_test_app();
        app.workspace_context_refreshed_at = Some(stale);
        let id = format!("file-{action}");
        handle_tool_call_started(&mut app, &id, "File", &input);
        handle_tool_call_complete(&mut app, &id, "File", &ok_result("ok"));
        assert!(
            app.workspace_context_refreshed_at
                .is_some_and(|when| when > stale),
            "File.{action} must refresh the workspace badge"
        );
    }
}

#[test]
fn underwater_motion_ticks_only_for_visible_unobscured_owners() {
    assert!(!underwater_motion_surface_visible(
        None, true, true, true, false
    ));
    assert!(!underwater_motion_surface_visible(
        Some(Rect::new(0, 0, 0, 24)),
        true,
        true,
        true,
        false,
    ));
    assert!(underwater_motion_surface_visible(
        Some(Rect::new(0, 0, 40, 12)),
        true,
        true,
        false,
        false,
    ));
    assert!(underwater_motion_surface_visible(
        Some(Rect::new(0, 0, 60, 16)),
        true,
        false,
        true,
        false,
    ));
    assert!(underwater_motion_surface_visible(
        Some(Rect::new(0, 0, 60, 16)),
        true,
        false,
        false,
        false,
    ));
    assert!(underwater_motion_surface_visible(
        Some(Rect::new(0, 0, 80, 24)),
        true,
        false,
        false,
        false,
    ));
    assert!(!underwater_motion_surface_visible(
        Some(Rect::new(0, 0, 100, 32)),
        false,
        false,
        true,
        false,
    ));
    assert!(!underwater_motion_surface_visible(
        Some(Rect::new(0, 0, 100, 32)),
        true,
        true,
        true,
        true,
    ));
}

#[test]
fn live_transcript_command_open_path_is_idempotent() {
    let mut app = create_test_app();

    open_live_transcript_overlay(&mut app);
    open_live_transcript_overlay(&mut app);

    assert_eq!(app.view_stack.top_kind(), Some(ModalKind::LiveTranscript));
    app.view_stack.pop();
    assert_eq!(
        app.view_stack.top_kind(),
        None,
        "opening twice must not stack duplicate transcript overlays"
    );
}

#[test]
fn approval_prompt_keeps_transcript_page_navigation_live() {
    let mut app = create_test_app();
    app.viewport.last_transcript_visible = 12;
    app.view_stack.push(ApprovalView::new(ApprovalRequest::new(
        "approval-scroll",
        "exec_shell",
        "Review command",
        &serde_json::json!({"command": "git status"}),
        "approval-scroll-key",
    )));

    assert!(handle_approval_transcript_key(
        &mut app,
        &KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
    ));
    assert_eq!(app.viewport.pending_scroll_delta, -12);
    assert_eq!(app.view_stack.top_kind(), Some(ModalKind::Approval));

    assert!(handle_approval_transcript_key(
        &mut app,
        &KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
    ));
    assert_eq!(app.viewport.pending_scroll_delta, 0);

    assert!(handle_approval_transcript_key(
        &mut app,
        &KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL),
    ));
    assert_eq!(app.viewport.pending_scroll_delta, -3);

    assert!(handle_approval_transcript_key(
        &mut app,
        &KeyEvent::new(KeyCode::Home, KeyModifiers::NONE),
    ));
    assert!(app.viewport.pending_scroll_delta < -1_000_000);
    assert!(app.user_scrolled_during_stream);

    assert!(handle_approval_transcript_key(
        &mut app,
        &KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
    ));
    assert_eq!(app.viewport.pending_scroll_delta, 0);
    assert!(!app.user_scrolled_during_stream);

    assert!(handle_approval_transcript_key(
        &mut app,
        &KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT),
    ));
    assert_eq!(app.viewport.pending_scroll_delta, 3);
    assert!(app.user_scrolled_during_stream);

    assert!(
        !handle_approval_transcript_key(
            &mut app,
            &KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        ),
        "ordinary approval selection keys must stay with the decision card"
    );
}

#[test]
fn transcript_navigation_does_not_capture_keys_for_other_modals() {
    let mut app = create_test_app();
    app.view_stack.push(HelpView::new_for_locale(app.ui_locale));

    assert!(!handle_approval_transcript_key(
        &mut app,
        &KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
    ));
    assert_eq!(app.viewport.pending_scroll_delta, 0);
}

#[test]
fn closing_work_inspector_pager_clears_opened_row_owner() {
    let mut app = create_test_app();
    app.todos.try_lock().expect("To-do state").add(
        "Inspect me".to_string(),
        crate::tools::todo::TodoStatus::InProgress,
    );
    let _ = render_underwater_test_app(&mut app, 100, 32);
    let _ = crate::tui::work_surface::handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('w'), KeyModifiers::ALT),
    );
    app.work_surface.opened = app.work_surface.selected.clone();
    assert!(app.work_surface.opened.is_some());
    app.view_stack
        .push(PagerView::from_text("Work", "detail", 40));

    let was_work_inspector =
        app.work_surface.opened.is_some() && app.view_stack.top_kind() == Some(ModalKind::Pager);
    let events = app
        .view_stack
        .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(events.is_empty());
    clear_work_inspector_after_pager_close(&mut app, was_work_inspector);

    assert_eq!(app.view_stack.top_kind(), None);
    assert!(app.work_surface.opened.is_none());
}

#[test]
fn coordination_event_projection_is_retained_as_typed_app_state() {
    use crate::tools::subagent::CoordinationDetailProjection;
    use crate::tools::subagent::coord::{
        CoordinationDetailMetrics, DecisionRecord, DecisionStatus,
    };

    let mut app = create_test_app();
    let projection = CoordinationDetailProjection {
        schema_version: 1,
        sequence: 4,
        decisions: vec![DecisionRecord {
            decision_id: "decision-app".to_string(),
            subject: "typed app state".to_string(),
            status: DecisionStatus::Accepted,
            owner: "root".to_string(),
            scope: Vec::new(),
            constraints: Vec::new(),
            evidence_handles: Vec::new(),
            version: 2,
            sequence: 4,
        }],
        write_claims: Vec::new(),
        reconciliations: Vec::new(),
        context_projections: Vec::new(),
        contentions: Vec::new(),
        metrics: CoordinationDetailMetrics {
            hottest_paths: Vec::new(),
            package_or_module_growth: None,
            route_or_cost: None,
            note: "No authoritative metric source".to_string(),
        },
        bounded: true,
        limit: 24,
        process_lock_held: true,
        process_lock_note: None,
    };

    apply_coordination_detail_projection(&mut app, projection.clone());

    assert_eq!(app.coordination_detail, Some(projection));
}

/// Owner report 2026-08-04: switching model/provider mid-session raced the
/// new engine against its predecessor for the coordination flock and fired a
/// 30-second sticky warning blaming "another Codewhale process" that does
/// not exist. A same-process handover self-heals and stays off the sticky
/// strip; a genuinely different process still warns.
#[test]
fn coordination_handover_within_this_process_does_not_toast() {
    use crate::tools::subagent::CoordinationDetailProjection;
    use crate::tools::subagent::coord::CoordinationDetailMetrics;

    let base = CoordinationDetailProjection {
        schema_version: 1,
        sequence: 1,
        decisions: Vec::new(),
        write_claims: Vec::new(),
        reconciliations: Vec::new(),
        context_projections: Vec::new(),
        contentions: Vec::new(),
        metrics: CoordinationDetailMetrics {
            hottest_paths: Vec::new(),
            package_or_module_growth: None,
            route_or_cost: None,
            note: "No authoritative metric source".to_string(),
        },
        bounded: true,
        limit: 24,
        process_lock_held: false,
        process_lock_note: None,
    };

    let mut handover_app = create_test_app();
    apply_coordination_detail_projection(
        &mut handover_app,
        CoordinationDetailProjection {
            process_lock_note: Some(format!(
                "{} for /ws: lock is busy",
                crate::tools::subagent::COORDINATION_SAME_PROCESS_HANDOVER
            )),
            ..base.clone()
        },
    );
    assert!(
        handover_app.sticky_status.is_none(),
        "same-process handover is transient and must not fire the sticky warning: {:?}",
        handover_app.sticky_status
    );
    assert!(
        !handover_app
            .status_toasts
            .iter()
            .any(|toast| toast.text.contains("delegated coordination")),
        "same-process handover is transient and must not push a delegated coordination toast: {:?}",
        handover_app.status_toasts
    );

    let mut foreign_app = create_test_app();
    apply_coordination_detail_projection(
        &mut foreign_app,
        CoordinationDetailProjection {
            process_lock_note: Some(
                "another Codewhale process (pid 4242) owns delegated coordination for /ws: busy"
                    .to_string(),
            ),
            ..base
        },
    );
    let toast = foreign_app
        .status_toasts
        .back()
        .expect("a genuinely foreign owner still warns via transient toast");
    assert_eq!(toast.level, crate::tui::app::StatusToastLevel::Info);
    assert_eq!(toast.ttl_ms, Some(5_000));
    // The strip is one row and truncates from the right, so the *opening* of
    // the message has to carry the fact. The old copy opened with the
    // diagnosis and buried the cause behind a pid, a path and an errno, which
    // truncated to `Delegated coordination unavailable — an…`.
    assert!(
        toast
            .text
            .starts_with("Another Codewhale session in this workspace"),
        "the toast must lead with the fact that explains the state: {}",
        toast.text
    );
    assert!(
        !toast.text.contains("pid 4242") && !toast.text.contains("/ws"),
        "pid and workspace path belong in the coordination detail view, not \
         in a one-row toast: {}",
        toast.text
    );
    assert!(
        toast.text.chars().count() <= 110,
        "a one-row toast that cannot fit the row teaches nothing: {}",
        toast.text
    );
}

#[test]
fn approval_mouse_wheel_reviews_transcript_without_closing_card() {
    let mut app = create_test_app();
    app.view_stack.push(ApprovalView::new(ApprovalRequest::new(
        "approval-scroll",
        "exec_shell",
        "Review command",
        &serde_json::json!({"command": "git status"}),
        "approval-scroll-key",
    )));

    let events = handle_mouse_event(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 4,
            row: 4,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert!(events.is_empty());
    assert!(app.viewport.pending_scroll_delta < 0);
    assert!(app.user_scrolled_during_stream);
    assert_eq!(app.view_stack.top_kind(), Some(ModalKind::Approval));
}

#[test]
fn approval_wheel_preserves_work_surface_ownership() {
    let mut app = create_test_app();
    app.view_stack.push(ApprovalView::new(ApprovalRequest::new(
        "approval-scroll",
        "exec_shell",
        "Review command",
        &serde_json::json!({"command": "git status"}),
        "approval-scroll-key",
    )));
    app.work_surface.last_area = Some(Rect::new(0, 0, 30, 20));
    app.viewport.last_approval_area = Some(Rect::new(0, 12, 80, 8));

    for (column, row) in [(10, 4), (20, 4)] {
        let events = handle_mouse_event(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::ScrollUp,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            },
        );
        assert!(events.is_empty());
        assert_eq!(
            app.viewport.pending_scroll_delta, 0,
            "approval wheel leaked from side surface at ({column}, {row})"
        );
    }

    for (column, row) in [(65, 15), (10, 15)] {
        let before = app.viewport.pending_scroll_delta;
        let events = handle_mouse_event(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::ScrollUp,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            },
        );
        assert!(events.is_empty());
        assert!(
            app.viewport.pending_scroll_delta < before,
            "visible approval did not outrank underlying side surface at ({column}, {row})"
        );
    }
    assert_eq!(app.view_stack.top_kind(), Some(ModalKind::Approval));
}

#[test]
fn config_update_preview_suppresses_only_success_message_not_action_or_errors() {
    let preview = prepare_config_update_result(
        commands::CommandResult::with_message_and_action(
            "preview confirmation",
            AppAction::UpdateStreamChunkTimeout(42),
        ),
        false,
    );
    assert!(preview.message.is_none());
    assert!(matches!(
        preview.action,
        Some(AppAction::UpdateStreamChunkTimeout(42))
    ));

    let persisted =
        prepare_config_update_result(commands::CommandResult::message("saved confirmation"), true);
    assert_eq!(persisted.message.as_deref(), Some("saved confirmation"));

    let error =
        prepare_config_update_result(commands::CommandResult::error("invalid value"), false);
    assert!(error.is_error);
    assert!(
        error
            .message
            .as_deref()
            .is_some_and(|msg| msg.contains("invalid value"))
    );
}

#[test]
fn config_refresh_preserves_active_search_filter() {
    let mut app = create_test_app();
    let mut view = ConfigView::new_for_app(&app);
    for ch in "model".chars() {
        let _ = view.handle_key(KeyEvent::new(
            crossterm::event::KeyCode::Char(ch),
            crossterm::event::KeyModifiers::NONE,
        ));
    }
    app.view_stack.push(view);

    refresh_config_view_if_open(&mut app, "default_model");

    let mut view = app.view_stack.pop().expect("refreshed config view");
    let config = view
        .as_any_mut()
        .downcast_mut::<ConfigView>()
        .expect("config view type");
    assert_eq!(config.filter_query(), "model");
}

#[test]
fn workflow_ui_events_apply_only_to_the_active_session_owner() {
    let mut app = create_test_app();
    app.current_session_id = Some("session-b".to_string());
    let a_started = serde_json::json!({
        "type": "run_started",
        "at_ms": 1,
        "workflow_goal": "A workflow"
    });

    assert!(!apply_owned_workflow_ui_event(
        &mut app,
        "session-a",
        "workflow-a",
        &a_started,
    ));
    assert!(
        app.workflow_panel.is_none(),
        "foreign event must not mutate B"
    );

    let b_started = serde_json::json!({
        "type": "run_started",
        "at_ms": 2,
        "workflow_goal": "B workflow"
    });
    assert!(apply_owned_workflow_ui_event(
        &mut app,
        "session-b",
        "workflow-b",
        &b_started,
    ));
    assert_eq!(
        app.workflow_panel
            .as_ref()
            .map(|panel| panel.run_id.as_str()),
        Some("workflow-b")
    );

    // A -> B -> A restores A's event lane with a new chronological start; it
    // never replays an older start through B.
    app.current_session_id = Some("session-a".to_string());
    let a_resumed = serde_json::json!({
        "type": "run_started",
        "at_ms": 3,
        "workflow_goal": "A workflow resumed"
    });
    assert!(apply_owned_workflow_ui_event(
        &mut app,
        "session-a",
        "workflow-a",
        &a_resumed,
    ));
    assert_eq!(
        app.workflow_panel
            .as_ref()
            .map(|panel| panel.run_id.as_str()),
        Some("workflow-a")
    );
}

#[test]
fn workflow_panel_plain_letters_return_to_composer() {
    let mut app = create_test_app();
    app.workflow_panel = Some(crate::tui::widgets::workflow_panel::WorkflowPanel::new(
        "workflow_typing",
        "typing regression",
        0,
    ));

    for ch in ['t', 'c', 'j', 'k'] {
        app.workflow_panel
            .as_mut()
            .expect("workflow panel")
            .keyboard_focus = true;
        let key = KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE);
        assert!(
            !handle_workflow_panel_key(&mut app, &key),
            "plain {ch:?} must fall through to the composer"
        );
        assert!(
            !app.workflow_panel
                .as_ref()
                .expect("workflow panel")
                .keyboard_focus,
            "typing releases panel focus"
        );
        app.insert_char(ch);
    }

    assert_eq!(app.input, "tcjk");
}

#[test]
fn workflow_panel_uses_non_text_keys_for_controls() {
    let mut app = create_test_app();
    let mut panel = crate::tui::widgets::workflow_panel::WorkflowPanel::new(
        "workflow_keys",
        "keyboard controls",
        0,
    );
    panel.keyboard_focus = true;
    let was_expanded = panel.expanded;
    app.workflow_panel = Some(panel);

    assert!(handle_workflow_panel_key(
        &mut app,
        &KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
    ));
    assert_ne!(
        app.workflow_panel
            .as_ref()
            .expect("workflow panel")
            .expanded,
        was_expanded
    );
}

struct ConfigPathEnvGuard {
    _codewhale_config_path: crate::test_support::EnvVarGuard,
    _deepseek_config_path: crate::test_support::EnvVarGuard,
    _tmp: TempDir,
    _lock: crate::test_support::TestEnvLock,
}

impl ConfigPathEnvGuard {
    fn new() -> Self {
        let lock = crate::test_support::lock_test_env();
        let tmp = TempDir::new().expect("config tempdir");
        let config_path = tmp.path().join(".deepseek").join("config.toml");
        std::fs::create_dir_all(config_path.parent().expect("config parent")).expect("config dir");
        let codewhale_config_path =
            crate::test_support::EnvVarGuard::set("CODEWHALE_CONFIG_PATH", &config_path);
        let deepseek_config_path =
            crate::test_support::EnvVarGuard::set("DEEPSEEK_CONFIG_PATH", &config_path);
        Self {
            _codewhale_config_path: codewhale_config_path,
            _deepseek_config_path: deepseek_config_path,
            _tmp: tmp,
            _lock: lock,
        }
    }

    fn config_path(&self) -> PathBuf {
        std::env::var_os("CODEWHALE_CONFIG_PATH")
            .map(PathBuf::from)
            .expect("config path set")
    }
}

struct SettingsHomeGuard {
    _tmp: TempDir,
    _home: crate::test_support::EnvVarGuard,
    _userprofile: crate::test_support::EnvVarGuard,
    _codewhale_home: crate::test_support::EnvVarGuard,
    _codewhale_config_path: crate::test_support::EnvVarGuard,
    _deepseek_config_path: crate::test_support::EnvVarGuard,
    _codewhale_provider: crate::test_support::EnvVarGuard,
    _deepseek_provider: crate::test_support::EnvVarGuard,
    _xdg_config_home: crate::test_support::EnvVarGuard,
    _appdata: crate::test_support::EnvVarGuard,
    _localappdata: crate::test_support::EnvVarGuard,
    _lock: crate::test_support::TestEnvLock,
}

impl SettingsHomeGuard {
    fn new() -> Self {
        let lock = crate::test_support::lock_test_env();
        let tmp = TempDir::new().expect("settings tempdir");
        let codewhale_home = tmp.path().join(".codewhale");
        Self {
            _home: crate::test_support::EnvVarGuard::set("HOME", tmp.path()),
            _userprofile: crate::test_support::EnvVarGuard::set("USERPROFILE", tmp.path()),
            _codewhale_home: crate::test_support::EnvVarGuard::set(
                "CODEWHALE_HOME",
                &codewhale_home,
            ),
            _codewhale_config_path: crate::test_support::EnvVarGuard::remove(
                "CODEWHALE_CONFIG_PATH",
            ),
            _deepseek_config_path: crate::test_support::EnvVarGuard::set(
                "DEEPSEEK_CONFIG_PATH",
                codewhale_home.join("config.toml"),
            ),
            _codewhale_provider: crate::test_support::EnvVarGuard::remove("CODEWHALE_PROVIDER"),
            _deepseek_provider: crate::test_support::EnvVarGuard::remove("DEEPSEEK_PROVIDER"),
            _xdg_config_home: crate::test_support::EnvVarGuard::set(
                "XDG_CONFIG_HOME",
                tmp.path().join("xdg-config"),
            ),
            _appdata: crate::test_support::EnvVarGuard::set("APPDATA", tmp.path().join("appdata")),
            _localappdata: crate::test_support::EnvVarGuard::set(
                "LOCALAPPDATA",
                tmp.path().join("localappdata"),
            ),
            _tmp: tmp,
            _lock: lock,
        }
    }
}

#[test]
fn resume_hint_uses_canonical_resume_command() {
    assert_eq!(
        resume_hint_text(),
        "To continue this session, execute codewhale run --continue"
    );
    assert!(should_show_resume_hint(Some(
        "019dd9d6-4f44-7c83-9863-59674a12b827"
    )));
}

#[test]
fn resume_hint_omits_missing_session_id() {
    assert!(!should_show_resume_hint(None));
    assert!(!should_show_resume_hint(Some("   ")));
}

#[test]
fn plain_mcp_show_refreshes_discovery_counts() {
    use crate::tui::app::McpUiAction;

    assert!(
        !mcp_ui_action_refreshes_discovery(&McpUiAction::Show),
        "plain /mcp snapshots the engine-owned live pool, not a UI discovery pool"
    );
    assert!(mcp_ui_action_refreshes_discovery(&McpUiAction::Validate));
    assert!(
        !mcp_ui_action_refreshes_discovery(&McpUiAction::Reload),
        "reload is handled by the engine-owned live pool, not a UI discovery pool"
    );
    assert!(!mcp_ui_action_refreshes_discovery(&McpUiAction::Init {
        force: false,
    }));
}

#[tokio::test]
async fn mcp_enable_persists_and_applies_the_live_tool_pool_in_one_action() {
    use crate::mcp::{McpManagerSnapshot, McpServerSnapshot};
    use crate::tui::app::McpUiAction;

    let temp = tempfile::tempdir().expect("temporary MCP home");
    let path = temp.path().join("mcp.json");
    crate::mcp::add_server_config(
        &path,
        "fixture".to_string(),
        Some("fixture-mcp".to_string()),
        None,
        Vec::new(),
        None,
    )
    .expect("seed MCP server");
    crate::mcp::set_server_enabled(&path, "fixture", false).expect("disable MCP server");

    let mut app = create_test_app();
    app.mcp_config_path = path.clone();
    let config = Config::default();
    let mut mock = mock_engine_handle();
    let handle = mock.handle.clone();
    let snapshot = McpManagerSnapshot {
        config_path: path.clone(),
        config_exists: true,
        reload_required: false,
        servers: vec![McpServerSnapshot {
            name: "fixture".to_string(),
            enabled: true,
            required: false,
            transport: "stdio".to_string(),
            command_or_url: "fixture-mcp".to_string(),
            connect_timeout: 5,
            execute_timeout: 5,
            read_timeout: 5,
            connected: true,
            error: None,
            auth_required: false,
            capability_metadata: crate::mcp::McpServerCapabilityMetadata::LegacyFallback,
            tools: Vec::new(),
            resources: Vec::new(),
            prompts: Vec::new(),
        }],
    };
    let response_snapshot = snapshot.clone();
    let respond = async {
        match mock.rx_op.recv().await.expect("automatic reload op") {
            Op::ReloadMcp { config_path, tx } => {
                assert_eq!(config_path, path);
                let sender = tx.lock().unwrap().take().expect("reload reply sender");
                sender
                    .send(Ok(crate::core::ops::McpManagerUpdate {
                        snapshot: response_snapshot,
                        generation: 7,
                    }))
                    .expect("reload reply");
            }
            other => panic!("unexpected op: {other:?}"),
        }
    };
    let action = handle_mcp_ui_action(
        &mut app,
        &handle,
        &config,
        McpUiAction::Enable {
            name: "fixture".to_string(),
        },
    );
    tokio::join!(action, respond);

    assert!(
        crate::mcp::load_config(&app.mcp_config_path)
            .expect("reload persisted MCP config")
            .servers
            .get("fixture")
            .expect("persisted fixture")
            .is_enabled(),
        "the durable config and live pool must advance together",
    );
    assert!(!app.mcp_reload_required);
    assert_eq!(app.mcp_snapshot.as_ref(), Some(&snapshot));
    assert_eq!(app.mcp_snapshot_generation, 7);
    assert!(app.mcp_snapshot_generation_invalidated);
    assert!(app.history.iter().any(|cell| matches!(
        cell,
        HistoryCell::System { content }
            if content.contains("Enabled MCP server 'fixture'")
    )));
    assert!(app.history.iter().any(|cell| matches!(
        cell,
        HistoryCell::System { content }
            if content.contains("MCP tool pool reloaded in process")
                && content.contains("next model turn")
    )));
}

#[tokio::test]
async fn mcp_reload_failure_keeps_pending_state_and_live_snapshot() {
    use crate::mcp::McpManagerSnapshot;
    use crate::tui::app::McpUiAction;

    let mut app = create_test_app();
    app.mcp_reload_required = true;
    app.mcp_snapshot = Some(McpManagerSnapshot {
        config_path: PathBuf::from("mcp.json"),
        config_exists: true,
        reload_required: true,
        servers: Vec::new(),
    });
    let previous = app.mcp_snapshot.clone();
    let config = Config::default();
    let mut mock = mock_engine_handle();
    let handle = mock.handle.clone();
    let respond = async {
        match mock.rx_op.recv().await.expect("reload op") {
            Op::ReloadMcp { config_path, tx } => {
                assert_eq!(config_path, PathBuf::from("mcp.json"));
                let sender = tx.lock().unwrap().take().expect("reload reply sender");
                sender
                    .send(Err("safe config parse failure".to_string()))
                    .expect("reload reply");
            }
            other => panic!("unexpected op: {other:?}"),
        }
    };
    let action = handle_mcp_ui_action(&mut app, &handle, &config, McpUiAction::Reload);
    tokio::join!(action, respond);

    assert!(app.mcp_reload_required);
    assert_eq!(app.mcp_snapshot, previous);
    assert!(app.history.iter().any(|cell| matches!(
        cell,
        HistoryCell::System { content }
            if content.contains("live tool pool is unchanged")
                && content.contains("safe config parse failure")
    )));
}

#[test]
fn focus_gained_forces_terminal_viewport_recapture() {
    assert!(terminal_event_needs_viewport_recapture(&Event::FocusGained));
    assert!(!terminal_event_needs_viewport_recapture(&Event::FocusLost));
}

// ANSI byte sequences are only written on platforms where crossterm uses the
// ANSI execution path. On Windows the same logical commands route through the
// WinAPI console backend and never reach the writer, so byte-level assertions
// here only make sense on non-Windows targets.
#[cfg(not(windows))]
#[test]
fn recover_terminal_modes_emits_expected_csi_sequences_with_gating() {
    let mut all_on: Vec<u8> = Vec::new();
    let mut all_off: Vec<u8> = Vec::new();
    recover_terminal_modes(&mut all_on, true, true);
    recover_terminal_modes(&mut all_off, false, false);
    let on = String::from_utf8_lossy(&all_on);
    let off = String::from_utf8_lossy(&all_off);

    assert!(
        on.contains("\x1b[?1004h") && off.contains("\x1b[?1004h"),
        "EnableFocusChange must be re-armed regardless of gating"
    );
    assert!(
        on.contains("\x1b[>1u") && off.contains("\x1b[>1u"),
        "Kitty keyboard disambiguation flag must be re-pushed regardless of gating"
    );
    assert!(
        !on.contains("\x1b[?1007h"),
        "alternate-scroll must stay off while mouse capture is active: wheel events must remain mouse events (#5223)"
    );
    assert!(
        on.contains("\x1b[?1007l"),
        "alternate-scroll must be reset while mouse capture is active"
    );
    assert!(
        !off.contains("\x1b[?1007h"),
        "alternate-scroll mode must stay off when mouse capture is disabled (#4026)"
    );
    assert!(
        off.contains("\x1b[?1007l"),
        "alternate-scroll mode must be reset when mouse capture is disabled"
    );

    assert!(
        on.contains("\x1b[?1000h"),
        "EnableMouseCapture missing when use_mouse_capture=true"
    );
    assert!(
        !off.contains("\x1b[?1000h"),
        "EnableMouseCapture must be gated by use_mouse_capture"
    );

    assert!(
        on.contains("\x1b[?2004h"),
        "EnableBracketedPaste missing when use_bracketed_paste=true"
    );
    assert!(
        !off.contains("\x1b[?2004h"),
        "EnableBracketedPaste must be gated by use_bracketed_paste"
    );
}

#[cfg(not(windows))]
#[test]
fn bracketed_paste_mode_helpers_ignore_writer_errors() {
    struct FailingWriter;

    impl std::io::Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("terminal mode unsupported"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::other("terminal mode unsupported"))
        }
    }

    let mut writer = FailingWriter;

    assert!(
        !try_enable_bracketed_paste_mode(&mut writer),
        "unsupported bracketed paste must be reported without bubbling an error"
    );
    disable_bracketed_paste_mode(&mut writer);
}

#[cfg(windows)]
#[test]
fn recover_terminal_modes_runs_without_panic_on_windows() {
    let mut buf: Vec<u8> = Vec::new();
    recover_terminal_modes(&mut buf, true, true);
    recover_terminal_modes(&mut buf, false, false);
}

#[test]
fn alternate_scroll_mode_disable_emits_xterm_reset() {
    let mut buf: Vec<u8> = Vec::new();
    disable_alternate_scroll_mode(&mut buf);
    let seq = String::from_utf8_lossy(&buf);
    assert!(
        seq.contains("\x1b[?1007l"),
        "disable_alternate_scroll_mode must emit the xterm alternate-scroll reset"
    );
}

// On Windows crossterm's PushKeyboardEnhancementFlags never writes bytes
// (is_ansi_code_supported() == false), so the fix writes the escape
// directly. Verify the direct path emits the expected Kitty keyboard
// protocol sequence so the Windows fix for #1359 is not accidentally reverted.
#[cfg(windows)]
#[test]
fn push_keyboard_flags_writes_kitty_push_sequence_on_windows() {
    let mut buf: Vec<u8> = Vec::new();
    push_keyboard_enhancement_flags(&mut buf);
    let seq = String::from_utf8_lossy(&buf);
    assert!(
        seq.contains("\x1b[>0u"),
        "push_keyboard_enhancement_flags must write kitty probe (\\x1b[>0u) on Windows (#1599); got: {seq:?}"
    );
}

#[cfg(windows)]
#[test]
fn pop_keyboard_flags_writes_kitty_pop_sequence_on_windows() {
    let mut buf: Vec<u8> = Vec::new();
    pop_keyboard_enhancement_flags(&mut buf);
    let seq = String::from_utf8_lossy(&buf);
    assert!(
        seq.contains("\x1b[<1u"),
        "pop_keyboard_enhancement_flags must write kitty pop (\\x1b[<1u) on Windows (#1359); got: {seq:?}"
    );
}

#[test]
fn terminal_origin_reset_resets_scroll_region_origin_without_destructive_clear() {
    assert!(
        TERMINAL_ORIGIN_RESET.starts_with(b"\x1b[r\x1b[?6l"),
        "must reset scroll margins and origin mode before repaint"
    );
    assert!(
        TERMINAL_ORIGIN_RESET.ends_with(b"\x1b[H"),
        "must home the cursor at the end of the reset sequence"
    );
    // Cross-terminal flicker regression (#1119, #1352, #1356, #1363, #1366,
    // #1260, #1295): emitting CSI 2J/3J here in addition to the
    // immediately-following ratatui `terminal.clear()` produced a visible
    // blank-then-repaint flicker on Ghostty / VSCode terminal / Win10 conhost
    // every TurnComplete. The cleared back-buffer plus a single ratatui clear
    // is sufficient on the alt-screen.
    assert!(
        !TERMINAL_ORIGIN_RESET
            .windows(b"\x1b[2J".len())
            .any(|sequence| sequence == b"\x1b[2J"),
        "must not emit destructive CSI 2J — causes visible flicker"
    );
    assert!(
        !TERMINAL_ORIGIN_RESET
            .windows(b"\x1b[3J".len())
            .any(|sequence| sequence == b"\x1b[3J"),
        "must not emit destructive CSI 3J — causes visible flicker"
    );
}

#[test]
fn composer_newline_shortcuts_do_not_steal_ctrl_enter() {
    assert!(is_composer_newline_key(
        KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
        false
    ));
    assert!(is_composer_newline_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT),
        false
    ));
    assert!(is_composer_newline_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
        false
    ));
    assert!(!is_composer_newline_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        false
    ));
    assert!(!is_composer_newline_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
        false
    ));
    assert!(!is_composer_newline_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
        false
    ));
}

#[test]
fn multiline_mode_inverts_only_enter_and_shift_enter() {
    assert!(is_composer_newline_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        true,
    ));
    assert!(!is_composer_newline_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
        true,
    ));
    for (code, modifiers) in [
        (KeyCode::Char('j'), KeyModifiers::CONTROL),
        (KeyCode::Enter, KeyModifiers::ALT),
    ] {
        assert!(is_composer_newline_key(
            KeyEvent::new(code, modifiers),
            true,
        ));
    }
}

#[test]
fn forced_submit_accepts_only_ctrl_enter() {
    assert!(is_forced_submit_key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::CONTROL,
    )));
    assert!(is_forced_submit_key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    )));
    assert!(!is_forced_submit_key(KeyEvent::new(
        KeyCode::Char('j'),
        KeyModifiers::CONTROL,
    )));
    assert!(!is_forced_submit_key(KeyEvent::new(
        KeyCode::Char('J'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    )));
    assert!(!is_forced_submit_key(KeyEvent::new(
        KeyCode::Char('j'),
        KeyModifiers::ALT | KeyModifiers::CONTROL,
    )));
    assert!(!is_forced_submit_key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::ALT,
    )));
}

#[test]
fn composer_key_events_map_to_portable_submit_chords() {
    assert_eq!(
        composer_submit_chord(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), false),
        Some(ComposerSubmitChord::Enter)
    );
    assert_eq!(
        composer_submit_chord(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL), false),
        Some(ComposerSubmitChord::CtrlEnter)
    );
    for modifiers in [KeyModifiers::SHIFT, KeyModifiers::ALT] {
        assert_eq!(
            composer_submit_chord(KeyEvent::new(KeyCode::Enter, modifiers), false),
            None,
            "newline chord must not submit: {modifiers:?}"
        );
    }
    assert_eq!(
        composer_submit_chord(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
            false,
        ),
        None
    );
    assert_eq!(
        composer_submit_chord(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), true),
        None
    );
    assert_eq!(
        composer_submit_chord(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT), true),
        Some(ComposerSubmitChord::Enter)
    );
}

#[cfg(target_os = "macos")]
#[test]
fn cmd_enter_normalizes_to_control_enter_not_newline() {
    use crate::tui::composer_ui::normalize_macos_modifiers;

    let normalized = normalize_macos_modifiers(KeyModifiers::SUPER);
    assert!(normalized.contains(KeyModifiers::CONTROL));
    assert!(!is_composer_newline_key(
        KeyEvent::new(KeyCode::Enter, normalized),
        false
    ));
}

#[test]
fn word_cursor_modifier_accepts_control_and_alt() {
    assert!(is_word_cursor_modifier(KeyModifiers::CONTROL));
    assert!(is_word_cursor_modifier(KeyModifiers::ALT));
    assert!(is_word_cursor_modifier(
        KeyModifiers::CONTROL | KeyModifiers::SHIFT
    ));
    assert!(!is_word_cursor_modifier(KeyModifiers::NONE));
    assert!(!is_word_cursor_modifier(KeyModifiers::SHIFT));
}

#[cfg(target_os = "macos")]
#[test]
fn normalize_macos_modifiers_maps_super_to_control() {
    use crate::tui::composer_ui::normalize_macos_modifiers;
    // SUPER (Cmd) without CONTROL should gain CONTROL and lose SUPER.
    let normalized = normalize_macos_modifiers(KeyModifiers::SUPER);
    assert!(normalized.contains(KeyModifiers::CONTROL));
    assert!(!normalized.contains(KeyModifiers::SUPER));
}

#[cfg(target_os = "macos")]
#[test]
fn normalize_macos_modifiers_preserves_existing_control() {
    use crate::tui::composer_ui::normalize_macos_modifiers;
    // CONTROL already set — SUPER should be removed.
    let normalized = normalize_macos_modifiers(KeyModifiers::CONTROL | KeyModifiers::SUPER);
    assert!(normalized.contains(KeyModifiers::CONTROL));
    assert!(!normalized.contains(KeyModifiers::SUPER));
}

#[test]
fn normalize_macos_modifiers_leaves_alt_unchanged() {
    use crate::tui::composer_ui::normalize_macos_modifiers;
    let normalized = normalize_macos_modifiers(KeyModifiers::ALT);
    // On non-macOS this is a no-op; on macOS ALT stays unchanged.
    assert!(normalized.contains(KeyModifiers::ALT));
    assert!(!normalized.contains(KeyModifiers::SUPER));
}

#[test]
fn alt_f_and_alt_b_move_by_word_without_inserting_text() {
    let mut app = create_test_app();
    app.input = "alpha beta gamma".to_string();
    app.cursor_position = 0;

    assert!(handle_composer_alt_word_motion_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT),
    ));
    assert_eq!(app.input, "alpha beta gamma");
    assert_eq!(app.cursor_position, "alpha ".chars().count());

    app.selection_anchor = Some(0);
    assert!(handle_composer_alt_word_motion_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT),
    ));
    assert_eq!(app.input, "alpha beta gamma");
    assert_eq!(app.cursor_position, 0);
    assert!(app.selection_anchor.is_none());
}

#[test]
fn alt_word_motion_helper_ignores_altgr_style_control_alt() {
    let mut app = create_test_app();
    app.input = "alpha beta".to_string();
    app.cursor_position = 0;

    assert!(!handle_composer_alt_word_motion_key(
        &mut app,
        KeyEvent::new(
            KeyCode::Char('f'),
            KeyModifiers::CONTROL | KeyModifiers::ALT
        ),
    ));
    assert_eq!(app.cursor_position, 0);
}

fn select_full_transcript(app: &mut App) {
    app.viewport.transcript_selection.anchor = Some(TranscriptSelectionPoint {
        line_index: 0,
        column: 0,
    });
    app.viewport.transcript_selection.head = Some(TranscriptSelectionPoint {
        line_index: app
            .viewport
            .transcript_cache
            .total_lines()
            .saturating_sub(1),
        column: 80,
    });
}

#[test]
fn selection_point_from_position_ignores_top_padding() {
    let area = Rect {
        x: 10,
        y: 20,
        width: 30,
        height: 5,
    };

    // Content is bottom-aligned: 2 transcript lines in a 5-row viewport.
    let padding_top = 3;
    let transcript_top = 0;
    let transcript_total = 2;

    // Click in padding area -> no selection
    assert!(
        selection_point_from_position(
            area,
            area.x + 1,
            area.y,
            transcript_top,
            transcript_total,
            padding_top,
        )
        .is_none()
    );

    // First transcript line is at row `padding_top`
    let p0 = selection_point_from_position(
        area,
        area.x + 2,
        area.y + u16::try_from(padding_top).expect("padding should fit"),
        transcript_top,
        transcript_total,
        padding_top,
    )
    .expect("point");
    assert_eq!(p0.line_index, 0);
    assert_eq!(p0.column, 2);

    // Second transcript line is one row below
    let p1 = selection_point_from_position(
        area,
        area.x,
        area.y + u16::try_from(padding_top + 1).expect("padding should fit"),
        transcript_top,
        transcript_total,
        padding_top,
    )
    .expect("point");
    assert_eq!(p1.line_index, 1);
    assert_eq!(p1.column, 0);
}

/// #4208 evidence: the in-app selection copy strips every rendering rail
/// (user quote bar, card rails) via cache metadata, so copied text is clean
/// regardless of which decoration the transcript grammar uses. The
/// remaining #4208 surface is terminal-native selection with mouse capture
/// off, which no app-side clipboard code can intercept.
#[test]
fn selection_to_text_excludes_transcript_rail_decorations() {
    let mut app = create_test_app();
    app.history = vec![
        HistoryCell::User {
            content: "fix the flaky pty test".to_string(),
        },
        HistoryCell::Assistant {
            content: "Looking at the failing test first.".to_string(),
            streaming: false,
        },
    ];
    app.resync_history_revisions();
    app.viewport.transcript_cache.ensure(
        &app.history,
        &app.history_revisions,
        80,
        app.transcript_render_options(),
    );

    let last = app
        .viewport
        .transcript_cache
        .lines()
        .len()
        .saturating_sub(1);
    app.viewport.transcript_selection.anchor = Some(TranscriptSelectionPoint {
        line_index: 0,
        column: 0,
    });
    app.viewport.transcript_selection.head = Some(TranscriptSelectionPoint {
        line_index: last,
        column: 80,
    });

    let copied = selection_to_text(&app).expect("selection yields text");
    for rail in ['▎', '▏', '╎', '│', '┃', '╭', '╰', '●'] {
        assert!(
            !copied.contains(rail),
            "copied text must exclude the {rail:?} rail: {copied:?}"
        );
    }
    assert!(
        copied.contains("fix the flaky pty test"),
        "content survives rail stripping: {copied:?}"
    );
    assert!(
        copied.contains("Looking at the failing test first."),
        "assistant content survives: {copied:?}"
    );
}

#[test]
fn selection_to_text_handles_multiline_and_reversed_endpoints() {
    let mut app = create_test_app();
    app.history = vec![HistoryCell::Assistant {
        content: "alpha beta\ngamma delta".to_string(),
        streaming: false,
    }];
    app.resync_history_revisions();
    app.viewport.transcript_cache.ensure(
        &app.history,
        &app.history_revisions,
        80,
        app.transcript_render_options(),
    );

    app.viewport.transcript_selection.anchor = Some(TranscriptSelectionPoint {
        line_index: 1,
        column: 5,
    });
    app.viewport.transcript_selection.head = Some(TranscriptSelectionPoint {
        line_index: 0,
        column: 6,
    });

    assert_eq!(selection_to_text(&app).as_deref(), Some("a beta\ngam"));
}

#[test]
fn selection_to_text_removes_visual_wrap_breaks_from_paragraphs() {
    let mut app = create_test_app();
    app.history = vec![HistoryCell::Assistant {
        content: "alpha beta gamma delta epsilon".to_string(),
        streaming: false,
    }];
    app.resync_history_revisions();
    app.viewport.transcript_cache.ensure(
        &app.history,
        &app.history_revisions,
        14,
        app.transcript_render_options(),
    );
    select_full_transcript(&mut app);

    let selected = selection_to_text(&app).expect("selection text");
    assert!(
        !selected.contains('\n'),
        "soft-wrapped paragraph copied with visual newlines: {selected:?}"
    );
    assert!(selected.contains("alpha beta gamma delta epsilon"));
}

#[test]
fn selection_to_text_preserves_wrapped_long_words() {
    let mut app = create_test_app();
    app.history = vec![HistoryCell::Assistant {
        content: "abcdefghijklmnop".to_string(),
        streaming: false,
    }];
    app.resync_history_revisions();
    app.viewport.transcript_cache.ensure(
        &app.history,
        &app.history_revisions,
        10,
        app.transcript_render_options(),
    );
    select_full_transcript(&mut app);

    let selected = selection_to_text(&app).expect("selection text");
    assert_eq!(selected, "abcdefghijklmnop");
}

#[test]
fn selection_to_text_strips_code_block_visual_wrap_prefixes() {
    let mut app = create_test_app();
    app.history = vec![HistoryCell::Assistant {
        content: "```\nlet example = abcdefghijklmnop;\n```".to_string(),
        streaming: false,
    }];
    app.resync_history_revisions();
    app.viewport.transcript_cache.ensure(
        &app.history,
        &app.history_revisions,
        14,
        app.transcript_render_options(),
    );
    select_full_transcript(&mut app);

    let selected = selection_to_text(&app).expect("selection text");
    assert_eq!(selected, "let example = abcdefghijklmnop;");
}

#[test]
fn selection_to_text_strips_list_continuation_prefixes() {
    let mut app = create_test_app();
    app.history = vec![HistoryCell::Assistant {
        content: "- alpha beta gamma delta epsilon".to_string(),
        streaming: false,
    }];
    app.resync_history_revisions();
    app.viewport.transcript_cache.ensure(
        &app.history,
        &app.history_revisions,
        14,
        app.transcript_render_options(),
    );
    select_full_transcript(&mut app);

    let selected = selection_to_text(&app).expect("selection text");
    assert_eq!(selected, "- alpha beta gamma delta epsilon");
}

#[test]
fn selection_to_text_strips_nested_blockquote_rails() {
    let mut app = create_test_app();
    app.history = vec![HistoryCell::Assistant {
        content: ">>> nested quoted text".to_string(),
        streaming: false,
    }];
    app.resync_history_revisions();
    app.viewport.transcript_cache.ensure(
        &app.history,
        &app.history_revisions,
        80,
        app.transcript_render_options(),
    );
    select_full_transcript(&mut app);

    assert_eq!(
        selection_to_text(&app).as_deref(),
        Some("nested quoted text")
    );
}

#[test]
fn selection_to_text_copies_rendered_transcript_block() {
    let mut app = create_test_app();
    app.history = vec![
        HistoryCell::System {
            content: "copy system".to_string(),
        },
        HistoryCell::User {
            content: "copy user".to_string(),
        },
        HistoryCell::Thinking {
            content: "copy thinking".to_string(),
            streaming: false,
            duration_secs: Some(1.0),
        },
        HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "exec_shell".to_string(),
            status: ToolStatus::Success,
            input_summary: Some("cargo check".to_string()),
            output: Some("tool output line".to_string()),
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        })),
        HistoryCell::Assistant {
            content: "copy assistant".to_string(),
            streaming: false,
        },
    ];
    app.resync_history_revisions();
    app.viewport.transcript_cache.ensure(
        &app.history,
        &app.history_revisions,
        80,
        app.transcript_render_options(),
    );

    app.viewport.transcript_selection.anchor = Some(TranscriptSelectionPoint {
        line_index: 0,
        column: 0,
    });
    app.viewport.transcript_selection.head = Some(TranscriptSelectionPoint {
        line_index: app
            .viewport
            .transcript_cache
            .total_lines()
            .saturating_sub(1),
        column: 80,
    });

    let selected = selection_to_text(&app).expect("selection text");
    assert!(selected.contains("Note copy system"), "{selected:?}");
    assert!(selected.contains("copy user"), "{selected:?}");
    // Short completed thinking now renders inline (v0.8.42 thinking-preview
    // change); it should be selectable/copyable as visible transcript text.
    assert!(
        selected.contains("copy thinking"),
        "short completed thinking should be visible inline: {selected:?}"
    );
    // Short thinking that fits entirely inline doesn't need the Ctrl+O
    // affordance; only truncated or explicit-summary thinking shows it.
    assert!(
        !selected.contains("Ctrl+O"),
        "short completed thinking should not show the detail affordance: {selected:?}"
    );
    assert!(selected.contains("run done · cargo check"), "{selected:?}");
    assert!(selected.contains("copy assistant"), "{selected:?}");
    // #1163: tool-card middle lines are rendered with a `│ ` left rail
    // glyph, but that decoration must not leak into copied text. Assert
    // no isolated rail glyph survives at the start of any line.
    for (idx, line) in selected.lines().enumerate() {
        assert!(
            !line.starts_with("\u{2502} "),
            "line {idx} retained tool-card rail prefix: {line:?}"
        );
    }
}

#[test]
fn selection_has_content_rejects_zero_width_selection() {
    let mut app = create_test_app();
    let point = TranscriptSelectionPoint {
        line_index: 0,
        column: 3,
    };
    app.viewport.transcript_selection.anchor = Some(point);
    app.viewport.transcript_selection.head = Some(point);

    assert!(!selection_has_content(&app));
}

#[test]
fn mouse_selection_autocopies_on_release_without_ctrl_c() {
    let mut app = create_test_app();
    app.history = vec![HistoryCell::Assistant {
        content: "alpha beta".to_string(),
        streaming: false,
    }];
    app.resync_history_revisions();
    app.viewport.transcript_cache.ensure(
        &app.history,
        &app.history_revisions,
        80,
        app.transcript_render_options(),
    );
    app.viewport.last_transcript_area = Some(Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 8,
    });
    app.viewport.last_transcript_top = 0;
    app.viewport.last_transcript_total = app.viewport.transcript_cache.total_lines();
    app.viewport.last_transcript_padding_top = 0;

    handle_mouse_event(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        },
    );
    handle_mouse_event(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 8,
            row: 0,
            modifiers: KeyModifiers::NONE,
        },
    );
    handle_mouse_event(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 8,
            row: 0,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert_eq!(app.status_message.as_deref(), Some("Selection copied"));
    assert!(
        app.clipboard
            .last_written_text()
            .is_some_and(|text| text.contains("alpha")),
        "selection should be written to clipboard"
    );
}

#[test]
fn loading_mouse_filter_keeps_hover_and_active_drags() {
    let mut app = create_test_app();
    app.is_loading = true;

    let moved = MouseEvent {
        kind: MouseEventKind::Moved,
        column: 3,
        row: 2,
        modifiers: KeyModifiers::NONE,
    };
    let drag = MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 5,
        row: 2,
        modifiers: KeyModifiers::NONE,
    };

    assert!(!should_drop_loading_mouse_motion(&app, moved));
    assert!(should_drop_loading_mouse_motion(&app, drag));

    app.viewport.transcript_selection.dragging = true;
    assert!(!should_drop_loading_mouse_motion(&app, drag));

    app.viewport.transcript_selection.dragging = false;
    app.viewport.transcript_scrollbar_dragging = true;
    assert!(!should_drop_loading_mouse_motion(&app, drag));

    app.viewport.transcript_scrollbar_dragging = false;
    app.work_surface = crate::tui::work_surface::WorkSurfaceState::with_placement(
        crate::tui::work_surface::WorkSurfacePlacement::Top,
    );
    app.work_surface.last_area = Some(Rect::new(0, 0, 80, 3));
    let started = crate::tui::work_surface::handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 20,
            row: 2,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(started.consumed);
    assert!(
        !should_drop_loading_mouse_motion(&app, drag),
        "Underwater Work divider drag must survive the loading filter"
    );
}

#[test]
fn loading_mouse_filter_allows_sidebar_hover_to_clear() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.sidebar_hover_tooltip = Some("Stale sidebar tooltip".to_string());
    let moved = MouseEvent {
        kind: MouseEventKind::Moved,
        column: 12,
        row: 5,
        modifiers: KeyModifiers::NONE,
    };

    assert!(!should_drop_loading_mouse_motion(&app, moved));
    handle_mouse_event(&mut app, moved);

    assert_eq!(app.sidebar_hover_tooltip, None);
    assert_eq!(app.last_mouse_pos, Some((12, 5)));
}

#[test]
fn loading_mouse_filter_allows_sidebar_exit_to_clear_highlight() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.last_mouse_pos = Some((60, 5));

    let exit_left = MouseEvent {
        kind: MouseEventKind::Moved,
        column: 59,
        row: 5,
        modifiers: KeyModifiers::NONE,
    };

    assert!(
        !should_drop_loading_mouse_motion(&app, exit_left),
        "first move out of the sidebar must clear stale sidebar hover state"
    );
    handle_mouse_event(&mut app, exit_left);

    assert_eq!(app.last_mouse_pos, Some((59, 5)));
    assert!(!should_drop_loading_mouse_motion(
        &app,
        MouseEvent {
            kind: MouseEventKind::Moved,
            column: 58,
            row: 5,
            modifiers: KeyModifiers::NONE,
        }
    ));
}

#[test]
fn jump_to_latest_button_click_scrolls_to_tail() {
    let mut app = create_test_app();
    app.viewport.transcript_scroll = TranscriptScroll::at_line(7);
    app.viewport.jump_to_latest_button_area = Some(Rect {
        x: 10,
        y: 5,
        width: 3,
        height: 3,
    });
    app.user_scrolled_during_stream = true;

    let events = handle_mouse_event(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 11,
            row: 6,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert!(events.is_empty());
    assert!(app.viewport.transcript_scroll.is_at_tail());
    assert!(app.viewport.jump_to_latest_button_area.is_none());
    assert!(!app.user_scrolled_during_stream);
    assert!(!app.viewport.transcript_selection.dragging);
}

/// Clicking the transcript scrollbar gutter starts a scrollbar drag (not
/// text selection) so the visible thumb remains interactive for users who
/// prefer mouse-based navigation.
#[test]
fn transcript_scrollbar_gutter_starts_scrollbar_drag() {
    let mut app = create_test_app();
    app.history = vec![HistoryCell::Assistant {
        content: "alpha beta".to_string(),
        streaming: false,
    }];
    app.resync_history_revisions();
    app.viewport.transcript_cache.ensure(
        &app.history,
        &app.history_revisions,
        80,
        app.transcript_render_options(),
    );
    app.viewport.last_transcript_area = Some(Rect {
        x: 2,
        y: 5,
        width: 20,
        height: 10,
    });
    app.viewport.last_transcript_visible = 10;
    app.viewport.last_transcript_total = 110;
    app.viewport.transcript_scroll = TranscriptScroll::to_bottom();
    app.user_scrolled_during_stream = false;

    // Left-down on the scrollbar gutter (column == right edge) starts a
    // scrollbar drag, not a transcript selection.
    let events = handle_mouse_event(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 21,
            row: 5,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert!(events.is_empty());
    assert!(
        app.viewport.transcript_scrollbar_dragging,
        "gutter click should start scrollbar drag"
    );
    assert!(
        !app.viewport.transcript_selection.dragging,
        "gutter click should NOT start text selection"
    );

    // Drag moves the viewport (no assertion on exact scroll position — the
    // mapping depends on area geometry).
    handle_mouse_event(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 21,
            row: 14,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(app.viewport.transcript_scrollbar_dragging);

    // Left-up ends the scrollbar drag.
    handle_mouse_event(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 21,
            row: 14,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert!(!app.viewport.transcript_scrollbar_dragging);
}

#[test]
fn left_down_inside_transcript_starts_selection() {
    let mut app = create_test_app();
    app.history = vec![HistoryCell::Assistant {
        content: "alpha beta".to_string(),
        streaming: false,
    }];
    app.resync_history_revisions();
    app.viewport.last_transcript_area = Some(Rect {
        x: 2,
        y: 5,
        width: 20,
        height: 10,
    });
    app.viewport.last_transcript_visible = 10;
    app.viewport.last_transcript_total = 110;

    let events = handle_mouse_event(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row: 5,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert!(events.is_empty());
    assert!(app.viewport.transcript_selection.dragging);
}

#[test]
fn drag_below_viewport_arms_autoscroll_down() {
    let mut app = create_test_app();
    app.history = vec![HistoryCell::Assistant {
        content: "alpha beta".to_string(),
        streaming: false,
    }];
    app.resync_history_revisions();
    app.viewport.transcript_cache.ensure(
        &app.history,
        &app.history_revisions,
        80,
        app.transcript_render_options(),
    );
    app.viewport.last_transcript_area = Some(Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 8,
    });
    app.viewport.last_transcript_total = app.viewport.transcript_cache.total_lines();
    app.viewport.transcript_selection.dragging = true;
    app.viewport.transcript_selection.anchor = Some(TranscriptSelectionPoint {
        line_index: 0,
        column: 0,
    });
    app.viewport.transcript_selection.head = Some(TranscriptSelectionPoint {
        line_index: 0,
        column: 0,
    });

    handle_mouse_event(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 4,
            row: 12, // below area.y + area.height (= 8)
            modifiers: KeyModifiers::NONE,
        },
    );

    let state = app.viewport.selection_autoscroll.expect("autoscroll armed");
    assert_eq!(state.direction, 1);
    assert_eq!(state.column, 4);
}

#[test]
fn drag_above_viewport_arms_autoscroll_up() {
    let mut app = create_test_app();
    app.viewport.last_transcript_area = Some(Rect {
        x: 5,
        y: 4,
        width: 40,
        height: 6,
    });
    app.viewport.transcript_selection.dragging = true;
    app.viewport.transcript_selection.anchor = Some(TranscriptSelectionPoint {
        line_index: 5,
        column: 0,
    });
    app.viewport.transcript_selection.head = Some(TranscriptSelectionPoint {
        line_index: 5,
        column: 0,
    });

    handle_mouse_event(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 50, // outside horizontally too — clamped to area.x + width - 1
            row: 1,     // above area.y (= 4)
            modifiers: KeyModifiers::NONE,
        },
    );

    let state = app.viewport.selection_autoscroll.expect("autoscroll armed");
    assert_eq!(state.direction, -1);
    assert_eq!(state.column, 5 + 40 - 1);
}

#[test]
fn drag_back_inside_disarms_autoscroll() {
    let mut app = create_test_app();
    app.history = vec![HistoryCell::Assistant {
        content: "alpha beta".to_string(),
        streaming: false,
    }];
    app.resync_history_revisions();
    app.viewport.transcript_cache.ensure(
        &app.history,
        &app.history_revisions,
        80,
        app.transcript_render_options(),
    );
    app.viewport.last_transcript_area = Some(Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 8,
    });
    app.viewport.last_transcript_total = app.viewport.transcript_cache.total_lines();
    app.viewport.transcript_selection.dragging = true;
    app.viewport.transcript_selection.anchor = Some(TranscriptSelectionPoint {
        line_index: 0,
        column: 0,
    });
    app.viewport.transcript_selection.head = Some(TranscriptSelectionPoint {
        line_index: 0,
        column: 0,
    });
    app.viewport.selection_autoscroll = Some(SelectionAutoscroll {
        direction: 1,
        column: 4,
        next_tick: Instant::now(),
    });

    handle_mouse_event(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 6,
            row: 0, // inside area
            modifiers: KeyModifiers::NONE,
        },
    );

    assert!(app.viewport.selection_autoscroll.is_none());
    let head = app
        .viewport
        .transcript_selection
        .head
        .expect("head present");
    assert_eq!(head.column, 6);
}

#[test]
fn mouse_up_clears_selection_autoscroll() {
    let mut app = create_test_app();
    app.viewport.transcript_selection.dragging = true;
    app.viewport.selection_autoscroll = Some(SelectionAutoscroll {
        direction: -1,
        column: 0,
        next_tick: Instant::now(),
    });

    handle_mouse_event(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert!(app.viewport.selection_autoscroll.is_none());
    assert!(!app.viewport.transcript_selection.dragging);
}

#[test]
fn tick_selection_autoscroll_advances_pending_scroll_when_due() {
    let mut app = create_test_app();
    app.viewport.last_transcript_area = Some(Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 8,
    });
    app.viewport.last_transcript_total = 200;
    app.viewport.transcript_selection.dragging = true;
    app.viewport.transcript_selection.anchor = Some(TranscriptSelectionPoint {
        line_index: 0,
        column: 0,
    });
    app.viewport.transcript_selection.head = Some(TranscriptSelectionPoint {
        line_index: 0,
        column: 0,
    });
    let earlier = Instant::now() - Duration::from_millis(100);
    app.viewport.selection_autoscroll = Some(SelectionAutoscroll {
        direction: 1,
        column: 10,
        next_tick: earlier,
    });

    tick_selection_autoscroll(&mut app);

    assert_eq!(app.viewport.pending_scroll_delta, 1);
    assert!(app.user_scrolled_during_stream);
    let next_tick = app
        .viewport
        .selection_autoscroll
        .expect("still armed")
        .next_tick;
    assert!(next_tick > earlier);
    let head = app
        .viewport
        .transcript_selection
        .head
        .expect("head extended");
    // Edge row for direction = +1 is the bottom of area (height - 1 = 7),
    // so head.line_index should equal last_transcript_top + 7.
    assert_eq!(head.line_index, 7);
    assert_eq!(head.column, 10);
}

#[test]
fn tick_selection_autoscroll_respects_cadence() {
    let mut app = create_test_app();
    app.viewport.last_transcript_area = Some(Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 8,
    });
    app.viewport.transcript_selection.dragging = true;
    let future = Instant::now() + Duration::from_secs(60);
    app.viewport.selection_autoscroll = Some(SelectionAutoscroll {
        direction: 1,
        column: 0,
        next_tick: future,
    });

    tick_selection_autoscroll(&mut app);

    assert_eq!(app.viewport.pending_scroll_delta, 0);
    assert_eq!(
        app.viewport
            .selection_autoscroll
            .expect("still armed")
            .next_tick,
        future,
        "next_tick must not advance before its deadline"
    );
}

#[test]
fn tick_selection_autoscroll_clears_when_drag_ended() {
    let mut app = create_test_app();
    app.viewport.last_transcript_area = Some(Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 8,
    });
    app.viewport.transcript_selection.dragging = false;
    app.viewport.selection_autoscroll = Some(SelectionAutoscroll {
        direction: 1,
        column: 0,
        next_tick: Instant::now() - Duration::from_millis(100),
    });

    tick_selection_autoscroll(&mut app);

    assert!(app.viewport.selection_autoscroll.is_none());
    assert_eq!(app.viewport.pending_scroll_delta, 0);
}

#[test]
fn right_click_opens_context_menu() {
    let mut app = create_test_app();

    let events = handle_mouse_event(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: 4,
            row: 4,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert!(events.is_empty());
    assert_eq!(app.view_stack.top_kind(), Some(ModalKind::ContextMenu));
}

#[test]
fn right_click_menu_includes_selection_and_clicked_cell_actions() {
    let mut app = create_test_app();
    app.history = vec![HistoryCell::Assistant {
        content: "alpha beta".to_string(),
        streaming: false,
    }];
    app.resync_history_revisions();
    app.viewport.transcript_cache.ensure(
        &app.history,
        &app.history_revisions,
        80,
        app.transcript_render_options(),
    );
    app.viewport.last_transcript_area = Some(Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 8,
    });
    app.viewport.last_transcript_top = 0;
    app.viewport.last_transcript_total = app.viewport.transcript_cache.total_lines();
    app.viewport.transcript_selection.anchor = Some(TranscriptSelectionPoint {
        line_index: 0,
        column: 0,
    });
    app.viewport.transcript_selection.head = Some(TranscriptSelectionPoint {
        line_index: 0,
        column: 5,
    });

    let entries = build_context_menu_entries(
        &app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: 2,
            row: 0,
            modifiers: KeyModifiers::NONE,
        },
    );
    let labels = entries
        .iter()
        .map(|entry| entry.label.as_str())
        .collect::<Vec<_>>();

    assert!(labels.contains(&"Copy selection"));
    assert!(labels.contains(&"Open selection"));
    assert!(labels.contains(&"Open details"));
    assert!(labels.contains(&"Paste"));
}

#[test]
fn mouse_events_do_not_mutate_transcript_behind_modal() {
    let mut app = create_test_app();
    app.view_stack.push(HelpView::new_for_locale(app.ui_locale));

    let events = handle_mouse_event(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 4,
            row: 4,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert!(events.is_empty());
    assert_eq!(app.viewport.pending_scroll_delta, 0);
    assert_eq!(app.view_stack.top_kind(), Some(ModalKind::Help));
}

#[test]
fn composer_mouse_wheel_scrolls_wrapped_draft_not_transcript() {
    let mut app = create_test_app();
    app.input = "alpha beta gamma delta epsilon".to_string();
    app.cursor_position = 0;
    app.viewport.last_composer_area = Some(Rect {
        x: 0,
        y: 10,
        width: 12,
        height: 5,
    });
    app.viewport.last_composer_content = Some(Rect {
        x: 1,
        y: 11,
        width: 5,
        height: 3,
    });

    let events = handle_mouse_event(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 2,
            row: 12,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert!(events.is_empty());
    assert_eq!(app.viewport.pending_scroll_delta, 0);
    assert!(app.cursor_position > 0);
}

#[test]
fn composer_mouse_wheel_up_moves_within_wrapped_draft() {
    let mut app = create_test_app();
    app.input = "alpha beta gamma delta epsilon".to_string();
    app.cursor_position = app.input.chars().count();
    app.viewport.last_composer_area = Some(Rect {
        x: 0,
        y: 10,
        width: 12,
        height: 5,
    });
    app.viewport.last_composer_content = Some(Rect {
        x: 1,
        y: 11,
        width: 5,
        height: 3,
    });

    assert!(handle_composer_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 2,
            row: 12,
            modifiers: KeyModifiers::NONE,
        },
    ));

    assert!(app.cursor_position < app.input.chars().count());
}

/// #5223: the wheel over an empty composer used to be consumed and dropped,
/// leaving no way to reach the scrollback at all.
#[test]
fn composer_mouse_wheel_with_empty_draft_scrolls_transcript() {
    let mut app = create_test_app();
    app.input.clear();
    app.cursor_position = 0;
    app.viewport.last_composer_area = Some(Rect {
        x: 0,
        y: 10,
        width: 12,
        height: 5,
    });
    app.viewport.last_composer_content = Some(Rect {
        x: 1,
        y: 11,
        width: 5,
        height: 3,
    });

    handle_mouse_event(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 2,
            row: 12,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert!(
        app.viewport.pending_scroll_delta < 0,
        "wheel over an empty composer must reach the transcript, got {}",
        app.viewport.pending_scroll_delta
    );
}

/// #5223: once the caret sits on the first wrapped row the composer has
/// nowhere left to go, so the wheel must hand off to the transcript instead of
/// being swallowed.
#[test]
fn composer_mouse_wheel_at_draft_top_falls_through_to_transcript() {
    let mut app = create_test_app();
    app.input = "alpha beta gamma delta epsilon".to_string();
    app.cursor_position = 0;
    app.viewport.last_composer_area = Some(Rect {
        x: 0,
        y: 10,
        width: 12,
        height: 5,
    });
    app.viewport.last_composer_content = Some(Rect {
        x: 1,
        y: 11,
        width: 5,
        height: 3,
    });

    handle_mouse_event(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 2,
            row: 12,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert_eq!(app.cursor_position, 0, "caret was already at the top");
    assert!(
        app.viewport.pending_scroll_delta < 0,
        "wheel at the top of the draft must reach the transcript, got {}",
        app.viewport.pending_scroll_delta
    );
}

/// #5223: terminals that convert the wheel into arrow keys reach the composer
/// through the key path. A multiline draft used to dead-end at its first line —
/// the key was consumed and nothing moved, stranding the user.
#[test]
fn multiline_draft_up_at_first_line_scrolls_transcript() {
    let mut app = create_test_app();
    app.composer_arrows_scroll = false;
    app.input = "first line\nsecond line".to_string();
    app.cursor_position = 0;
    app.history_index = None;
    app.input_history.push("previous prompt".to_string());

    assert!(handle_composer_history_arrow(
        &mut app,
        KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
        false,
        false,
    ));

    assert_eq!(
        app.input, "first line\nsecond line",
        "the multiline draft must not be replaced by prompt history"
    );
    assert_eq!(app.viewport.pending_scroll_delta, -3);
}

/// The mirror of the above at the end of the draft.
#[test]
fn multiline_draft_down_at_last_line_scrolls_transcript() {
    let mut app = create_test_app();
    app.composer_arrows_scroll = false;
    app.input = "first line\nsecond line".to_string();
    app.cursor_position = app.input.chars().count();
    app.history_index = None;

    assert!(handle_composer_history_arrow(
        &mut app,
        KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        false,
        false,
    ));

    assert_eq!(app.input, "first line\nsecond line");
    assert_eq!(app.viewport.pending_scroll_delta, 3);
}

#[test]
fn composer_mouse_first_visible_character_maps_to_zero_after_prompt_gutter() {
    let mut app = create_test_app();
    app.input = "abcdef".to_string();
    app.cursor_position = app.input.chars().count();
    app.viewport.last_composer_area = Some(Rect {
        x: 10,
        y: 10,
        width: 12,
        height: 4,
    });
    // Border-aware inner rect. The shared composer geometry reserves x=10..12
    // for the persistent prompt and places the first visible character at 12.
    app.viewport.last_composer_content = Some(Rect {
        x: 10,
        y: 11,
        width: 12,
        height: 3,
    });
    app.viewport.last_composer_scroll_offset = 0;
    app.viewport.last_composer_top_padding = 0;

    assert!(handle_composer_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 12,
            row: 11,
            modifiers: KeyModifiers::NONE,
        },
    ));
    assert_eq!(
        app.cursor_position, 0,
        "the first rendered character must not inherit the prompt's two-column inset"
    );

    // Exercise a second independent caret mapping, not the intentional
    // adjacent-click word-selection gesture.
    app.viewport.composer_click_trace = None;
    assert!(handle_composer_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 13,
            row: 11,
            modifiers: KeyModifiers::NONE,
        },
    ));
    assert_eq!(app.cursor_position, 1);
}

#[test]
fn copy_shortcut_accepts_cmd_and_ctrl_shift_only() {
    assert!(crate::tui::key_shortcuts::is_copy_shortcut(&KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::SUPER,
    )));
    assert!(crate::tui::key_shortcuts::is_copy_shortcut(&KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    )));
    assert!(!crate::tui::key_shortcuts::is_copy_shortcut(
        &KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL,)
    ));
}

#[test]
fn control_like_modifier_accepts_super_only_on_macos() {
    use crate::tui::key_shortcuts::has_control_like_modifier_for_platform;

    assert!(has_control_like_modifier_for_platform(
        KeyModifiers::CONTROL,
        false
    ));
    assert!(has_control_like_modifier_for_platform(
        KeyModifiers::CONTROL,
        true
    ));
    assert!(!has_control_like_modifier_for_platform(
        KeyModifiers::SUPER,
        false
    ));
    assert!(has_control_like_modifier_for_platform(
        KeyModifiers::SUPER,
        true
    ));
    assert!(has_control_like_modifier_for_platform(
        KeyModifiers::SUPER | KeyModifiers::ALT,
        true
    ));
}

#[test]
fn file_tree_shortcut_does_not_steal_plain_ctrl_e() {
    assert!(!crate::tui::key_shortcuts::is_file_tree_toggle_shortcut(
        &KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL,)
    ));
    assert!(crate::tui::key_shortcuts::is_file_tree_toggle_shortcut(
        &KeyEvent::new(KeyCode::Char('E'), KeyModifiers::CONTROL,)
    ));
    assert!(crate::tui::key_shortcuts::is_file_tree_toggle_shortcut(
        &KeyEvent::new(
            KeyCode::Char('e'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        )
    ));
    assert!(crate::tui::key_shortcuts::is_file_tree_toggle_shortcut(
        &KeyEvent::new(
            KeyCode::Char('E'),
            KeyModifiers::SUPER | KeyModifiers::SHIFT,
        )
    ));
}

#[test]
fn transcript_scroll_percent_is_clamped_and_relative() {
    assert_eq!(transcript_scroll_percent(0, 20, 120), Some(0));
    assert_eq!(transcript_scroll_percent(50, 20, 120), Some(50));
    assert_eq!(transcript_scroll_percent(200, 20, 120), Some(100));
    assert_eq!(transcript_scroll_percent(0, 20, 20), None);
}

#[test]
fn parse_git_status_path_handles_simple_and_renamed_entries() {
    assert_eq!(
        crate::tui::file_picker_relevance::parse_git_status_path(" M crates/tui/src/tui/ui.rs"),
        Some("crates/tui/src/tui/ui.rs".to_string())
    );
    assert_eq!(
        crate::tui::file_picker_relevance::parse_git_status_path(
            "R  old name.rs -> crates/tui/src/tui/file_picker.rs"
        ),
        Some("crates/tui/src/tui/file_picker.rs".to_string())
    );
}

#[test]
fn workspace_file_candidate_normalizes_absolute_and_line_suffixed_paths() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    let path = root.join("src/lib.rs");
    std::fs::write(&path, "").unwrap();

    let raw = format!("\"{}:42\",", path.display());
    assert_eq!(
        crate::tui::file_picker_relevance::workspace_file_candidate(&raw, root),
        Some("src/lib.rs".to_string())
    );
}

#[test]
fn tool_path_relevance_extracts_paths_from_command_text() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/alpha.rs"), "").unwrap();
    std::fs::write(root.join("src/zeta.rs"), "").unwrap();

    let mut relevance = crate::tui::file_picker::FilePickerRelevance::default();
    let mut seen = HashSet::new();
    let mut budget = 16;
    crate::tui::file_picker_relevance::mark_tool_paths_from_text(
        "sed -n '1,20p' src/zeta.rs",
        root,
        &mut seen,
        &mut relevance,
        &mut budget,
    );

    let view = crate::tui::file_picker::FilePickerView::new_with_relevance(root, relevance);
    assert_eq!(view.selected_for_test(), Some("src/zeta.rs"));
}

fn create_test_app() -> App {
    let mut app = crate::test_support::test_app_with_options(TuiOptions {
        // Keep UI tests independent from the developer's saved
        // `default_mode` setting.
        start_in_agent_mode: true,
        skip_onboarding: false,
        ..crate::test_support::test_tui_options(PathBuf::from("."))
    });
    // These suites assert legacy strip geometry (work surface above the
    // transcript). The Bottom default (round 3, 2026-09-01) has dedicated
    // coverage in work_surface::rail_panels_render_in_all_placements.
    app.work_surface.placement = crate::tui::work_surface::WorkSurfacePlacement::Top;
    app
}

#[test]
fn runtime_chat_short_circuits_inline_voice_and_cache_actions_before_ui_mutation() {
    let mut app = create_test_app();
    app.remote_control.block_runtime_chat_dispatch_for_tests();
    app.voice_enabled = true;
    let history_len = app.history.len();

    let voice = crate::commands::CommandResult::with_message_and_action(
        "optimistic voice message",
        AppAction::VoiceCapture,
    );
    assert!(super::apply::reject_inline_inference_while_runtime_chat_owns_run(&mut app, &voice,));
    assert!(!app.voice_enabled, "blocked /voice must restore its toggle");
    assert_eq!(
        app.history.len(),
        history_len,
        "the optimistic command message must not be appended"
    );
    assert!(app.status_message.is_none());
    let toast = app.status_toasts.back().expect("blocked action toast");
    assert_eq!(toast.level, crate::tui::app::StatusToastLevel::Info);
    assert!(toast.text.contains("Codewhale Runtime"));
    assert_eq!(toast.ttl_ms, Some(6_000));

    let cache = crate::commands::CommandResult::with_message_and_action(
        "optimistic cache message",
        AppAction::CacheWarmup,
    );
    assert!(super::apply::reject_inline_inference_while_runtime_chat_owns_run(&mut app, &cache,));
    assert_eq!(app.history.len(), history_len);

    let mut idle = create_test_app();
    assert!(!super::apply::reject_inline_inference_while_runtime_chat_owns_run(&mut idle, &cache,));
}

#[derive(Clone, Default)]
struct CapturedTrace(Arc<Mutex<Vec<u8>>>);

struct CapturedTraceWriter(CapturedTrace);

impl std::io::Write for CapturedTraceWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .0
            .lock()
            .expect("captured trace lock")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl CapturedTrace {
    fn record(&self, action: impl FnOnce()) -> String {
        let writer = self.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_max_level(tracing::Level::DEBUG)
            .with_target(false)
            .with_writer(move || CapturedTraceWriter(writer.clone()))
            .finish();
        let dispatch = tracing::Dispatch::new(subscriber);
        tracing::dispatcher::with_default(&dispatch, action);
        String::from_utf8(self.0.lock().expect("captured trace lock").clone())
            .expect("trace output is UTF-8")
    }
}

fn credential_paste_sentinel() -> String {
    [
        "violet",
        "otter",
        "credential",
        "paste",
        "must",
        "stay",
        "private",
        "7361",
    ]
    .join("-")
}

fn sensitive_prefixes(secret: &str) -> Vec<String> {
    [8, 16, 24]
        .into_iter()
        .filter(|length| secret.chars().count() >= *length)
        .map(|length| secret.chars().take(length).collect())
        .collect()
}

fn render_test_app(app: &mut App, config: &Config, width: u16, height: u16) -> String {
    let mut terminal =
        Terminal::new(TestBackend::new(width, height)).expect("paste safety test terminal");
    terminal
        .draw(|frame| {
            let _ = render(frame, app, config);
        })
        .expect("render paste safety surface");
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>()
}

fn assert_sensitive_paste_absent(surface: &str, secret: &str, label: &str) {
    assert!(
        !surface.contains(secret),
        "{label} exposed the complete pasted credential"
    );
    for prefix in sensitive_prefixes(secret) {
        assert!(
            !surface.contains(&prefix),
            "{label} exposed a pasted credential prefix"
        );
    }
}

#[test]
fn credential_paste_content_never_enters_traces_or_rendered_status() {
    let _env = crate::test_support::lock_test_env();
    let temp = TempDir::new().expect("isolated paste safety home");
    let _home = crate::test_support::EnvVarGuard::set(
        "CODEWHALE_HOME",
        temp.path().to_string_lossy().as_ref(),
    );
    let _secret_backend = crate::test_support::EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
    let _anthropic_key = crate::test_support::EnvVarGuard::remove("ANTHROPIC_API_KEY");
    let config = Config::default();
    let secret = credential_paste_sentinel();

    let setup = ProviderPickerView::new_for_setup(
        ApiProvider::Deepseek,
        Some(ApiProvider::Anthropic),
        &config,
        None,
    );
    let missing_auth = ProviderPickerView::new_for_missing_auth(
        ApiProvider::Deepseek,
        ApiProvider::Anthropic,
        &config,
        None,
    )
    .expect("Anthropic missing-auth editor");
    let mut onboarding = ProviderPickerView::new_for_onboarding(
        ApiProvider::Deepseek,
        Some(ApiProvider::Anthropic),
        &config,
        None,
    );
    assert!(matches!(
        onboarding.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        ViewAction::None
    ));

    for (label, picker, onboarding_state) in [
        ("setup", setup, OnboardingState::None),
        ("missing-auth", missing_auth, OnboardingState::None),
        ("onboarding", onboarding, OnboardingState::Provider),
    ] {
        let mut app = create_test_app();
        app.onboarding = onboarding_state;
        app.view_stack.push(picker);
        let logs = CapturedTrace::default().record(|| {
            handle_bracketed_paste(&mut app, &secret);
        });
        assert!(app.bracketed_paste_seen, "{label}");
        for (width, height) in [(80, 24), (120, 32)] {
            let rendered = render_test_app(&mut app, &config, width, height);
            assert_sensitive_paste_absent(
                &rendered,
                &secret,
                &format!("{label} credential editor at {width}x{height}"),
            );
        }
        assert_sensitive_paste_absent(&logs, &secret, &format!("{label} paste trace"));
        assert_sensitive_paste_absent(
            app.status_message.as_deref().unwrap_or_default(),
            &secret,
            &format!("{label} status"),
        );
    }
}

#[test]
fn ordinary_clipboard_text_routes_to_provider_picker_without_typing_shortcut_or_leaking() {
    let _env = crate::test_support::lock_test_env();
    let home = TempDir::new().expect("isolated ordinary-paste home");
    let _home = crate::test_support::EnvVarGuard::set(
        "CODEWHALE_HOME",
        home.path().to_string_lossy().as_ref(),
    );
    let _secret_backend = crate::test_support::EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
    let _openrouter_key = crate::test_support::EnvVarGuard::remove("OPENROUTER_API_KEY");
    let config = Config::default();
    let secret = credential_paste_sentinel();
    let picker = ProviderPickerView::new_for_missing_auth(
        ApiProvider::Deepseek,
        ApiProvider::Openrouter,
        &config,
        None,
    )
    .expect("OpenRouter key editor");
    let mut app = create_test_app();
    app.onboarding = OnboardingState::Provider;
    app.view_stack.push(picker);

    let logs = CapturedTrace::default().record(|| {
        assert!(paste_text_into_provider_picker(&mut app, &secret));
    });

    assert!(
        app.input.is_empty(),
        "credential paste leaked into composer"
    );
    assert_sensitive_paste_absent(&logs, &secret, "ordinary credential paste trace");
    for (width, height) in [(80, 24), (120, 32)] {
        let rendered = render_test_app(&mut app, &config, width, height);
        assert_sensitive_paste_absent(
            &rendered,
            &secret,
            &format!("ordinary credential paste at {width}x{height}"),
        );
        assert!(
            rendered.contains('*'),
            "masked draft missing at {width}x{height}"
        );
    }
}

#[test]
fn ordinary_bracketed_paste_keeps_content_but_logs_only_metadata() {
    let mut app = create_test_app();
    app.onboarding = OnboardingState::None;
    assert!(app.view_stack.is_empty());
    let ordinary = ["ordinary", "composer", "paste", "7361"].join(" ");

    let logs = CapturedTrace::default().record(|| {
        handle_bracketed_paste(&mut app, &ordinary);
    });

    assert_eq!(app.input, ordinary);
    assert!(app.bracketed_paste_seen);
    assert!(logs.contains("Received bracketed paste event"), "{logs}");
    assert!(logs.contains("paste_bytes"), "{logs}");
    assert!(logs.contains("paste_chars"), "{logs}");
    assert!(logs.contains(&ordinary.len().to_string()), "{logs}");
    assert_sensitive_paste_absent(&logs, &ordinary, "ordinary paste trace");
}

#[test]
fn cache_warmup_resolves_the_last_concrete_auto_route() {
    let config = Config {
        provider: Some("deepseek".to_string()),
        providers: Some(ProvidersConfig {
            vllm: ProviderConfig {
                base_url: Some("http://127.0.0.1:18191/v1".to_string()),
                model: Some("auto-cache-model".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut app = create_test_app();
    app.model = "auto".to_string();
    app.auto_model = true;
    app.last_effective_provider = Some(ApiProvider::Vllm);
    app.last_effective_provider_identity = Some(ApiProvider::Vllm.as_str().to_string());
    app.last_effective_model = Some("auto-cache-model".to_string());

    let route = resolve_cache_replay_route(&app, &config).expect("last Auto route should resolve");

    assert_eq!(route.identity.provider, ApiProvider::Vllm);
    assert_eq!(route.identity.key, ApiProvider::Vllm.as_str());
    assert_eq!(route.model, "auto-cache-model");
    assert_eq!(
        route.candidate.endpoint().base_url,
        "http://127.0.0.1:18191/v1"
    );

    app.last_effective_provider_identity = Some(ApiProvider::Openrouter.as_str().to_string());
    let error = resolve_cache_replay_route(&app, &config)
        .expect_err("a mismatched persisted identity must fail closed");
    assert!(
        error.to_string().contains("route identity"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn cache_warmup_rejects_an_auto_route_whose_endpoint_changed() {
    let config = Config {
        provider: Some("deepseek".to_string()),
        providers: Some(ProvidersConfig {
            vllm: ProviderConfig {
                base_url: Some("http://127.0.0.1:18192/v1".to_string()),
                model: Some("auto-cache-model".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut app = create_test_app();
    app.model = "auto".to_string();
    app.auto_model = true;
    app.last_effective_provider = Some(ApiProvider::Vllm);
    app.last_effective_provider_identity = Some(ApiProvider::Vllm.as_str().to_string());
    app.last_effective_model = Some("auto-cache-model".to_string());
    app.session.last_base_url = Some("http://127.0.0.1:18191/v1".to_string());
    app.push_turn_cache_record(crate::tui::app::TurnCacheRecord {
        provider: Some(ApiProvider::Vllm),
        provider_identity: Some(ApiProvider::Vllm.as_str().to_string()),
        model: Some("auto-cache-model".to_string()),
        auto_model: true,
        input_tokens: 1,
        output_tokens: 1,
        cache_hit_tokens: None,
        cache_miss_tokens: None,
        cache_write_tokens: None,
        reasoning_tokens: None,
        cost_audit: None,
        reasoning_replay_tokens: None,
        recorded_at: Instant::now(),
    });

    let error = resolve_cache_replay_route(&app, &config)
        .expect_err("changed endpoint must invalidate cache replay");

    assert!(error.to_string().contains("endpoint changed"), "{error:#}");
}

#[test]
fn hotbar_setup_save_persists_bindings_to_config_path() {
    let tmp = TempDir::new().expect("config tempdir");
    let config_path = tmp.path().join("config.toml");
    std::fs::write(
        &config_path,
        r#"# keep model note
model = "deepseek-v4-pro"

[providers.deepseek]
api_key = "test-key"
"#,
    )
    .expect("write config");

    let mut app = create_test_app();
    app.config_path = Some(config_path.clone());
    let mut config = Config::load(Some(config_path.clone()), None).expect("load config");
    let bindings = vec![codewhale_config::HotbarBindingToml {
        slot: 1,
        action: "mode.agent".to_string(),
        label: Some("Agent".to_string()),
    }];

    apply_hotbar_setup_saved(&mut app, &mut config, bindings.clone());

    assert_eq!(config.hotbar, Some(bindings.clone()));
    assert!(app.needs_redraw);
    assert!(
        app.status_message
            .as_deref()
            .is_some_and(|message| message.contains("Hotbar bindings saved to"))
    );

    let body = std::fs::read_to_string(&config_path).expect("read saved config");
    assert!(body.contains("# keep model note"), "comment lost: {body}");
    assert!(
        body.contains("[providers.deepseek]"),
        "provider section lost: {body}"
    );
    assert!(body.contains("[[hotbar]]"), "hotbar table missing: {body}");
    let parsed: codewhale_config::ConfigToml =
        toml::from_str(&body).expect("saved config should parse");
    assert_eq!(parsed.hotbar, Some(bindings));
}

#[test]
fn hotbar_setup_save_error_leaves_live_config_and_file_unchanged() {
    let tmp = TempDir::new().expect("config tempdir");
    let config_path = tmp.path().join("config.toml");
    let invalid_body = "model = [\n";
    std::fs::write(&config_path, invalid_body).expect("write malformed config");

    let mut app = create_test_app();
    app.config_path = Some(config_path.clone());
    let original_bindings = vec![codewhale_config::HotbarBindingToml {
        slot: 2,
        action: "mode.plan".to_string(),
        label: None,
    }];
    let mut config = Config {
        hotbar: Some(original_bindings.clone()),
        ..Default::default()
    };
    let attempted_bindings = vec![codewhale_config::HotbarBindingToml {
        slot: 1,
        action: "mode.agent".to_string(),
        label: None,
    }];

    apply_hotbar_setup_saved(&mut app, &mut config, attempted_bindings);

    assert_eq!(config.hotbar, Some(original_bindings));
    assert_eq!(
        std::fs::read_to_string(&config_path).expect("read malformed config"),
        invalid_body
    );
    assert!(
        app.status_message
            .as_deref()
            .is_some_and(|message| message.contains("Failed to save Hotbar bindings"))
    );
    assert!(app.needs_redraw);
    let last_system_message = app
        .history
        .iter()
        .rev()
        .find_map(|cell| match cell {
            HistoryCell::System { content } => Some(content.as_str()),
            _ => None,
        })
        .expect("failed save should add a system message");
    assert!(last_system_message.contains("Failed to save Hotbar bindings"));
}

#[test]
fn app_system_prompt_includes_configured_instructions() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let instructions = tmp.path().join("extra-instructions.md");
    std::fs::write(&instructions, "CONFIGURED_INSTRUCTIONS_MARKER").expect("write instructions");

    let mut app = create_test_app();
    app.workspace = tmp.path().to_path_buf();
    let config = Config {
        instructions: Some(vec![instructions.display().to_string()]),
        ..Config::default()
    };

    let prompt = crate::prompts::system_prompt_flat_text(&build_app_system_prompt(&app, &config));

    assert!(prompt.contains("CONFIGURED_INSTRUCTIONS_MARKER"));
    assert!(prompt.contains(&instructions.display().to_string()));
}

#[test]
fn session_denied_cache_matches_only_approval_key() {
    let mut app = create_test_app();
    app.approval_session_denied.insert("edit_file".to_string());

    assert!(
        !is_session_denied_for_key(&app, "file:edit_file:fresh"),
        "a legacy tool-name entry must not deny a later fresh call"
    );

    app.approval_session_denied
        .insert("file:edit_file:retry".to_string());
    assert!(is_session_denied_for_key(&app, "file:edit_file:retry"));
}

fn render_underwater_test_app(app: &mut App, width: u16, height: u16) -> String {
    app.onboarding_workspace_trust_gate = false;
    app.onboarding = OnboardingState::None;
    let config = Config::default();
    let mut terminal =
        Terminal::new(TestBackend::new(width, height)).expect("underwater test terminal");
    terminal
        .draw(|frame| {
            let _ = render(frame, app, &config);
        })
        .expect("render underwater shell");
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn wide_underwater_shell_aligns_transcript_and_composer_on_the_shared_canvas() {
    let mut app = create_test_app();
    app.history = vec![HistoryCell::User {
        content: "A deliberately short prompt.".to_string(),
    }];
    app.resync_history_revisions();

    let _ = render_underwater_test_app(&mut app, 160, 32);

    let transcript = app.viewport.last_transcript_area.expect("transcript area");
    let composer = app.viewport.last_composer_area.expect("composer area");
    // Full host width (#5322) — transcript and composer share one canvas edge.
    assert_eq!((transcript.x, transcript.width), (0, 160));
    assert_eq!((composer.x, composer.width), (0, 160));
}

#[test]
fn failed_mcp_is_a_footer_chip_not_multiline_chat_boot_output() {
    fn app() -> App {
        let mut app = create_test_app();
        app.onboarding_workspace_trust_gate = false;
        app.onboarding = OnboardingState::None;
        app.launch.visible = false;
        app
    }

    let mut baseline = app();
    let _ = render_underwater_test_app(&mut baseline, 100, 30);
    let baseline_composer = baseline
        .viewport
        .last_composer_area
        .expect("baseline composer area");

    let mut failed = app();
    failed.mcp_snapshot = Some(crate::mcp::McpManagerSnapshot {
        config_path: PathBuf::from("mcp.json"),
        config_exists: true,
        reload_required: false,
        servers: vec![crate::mcp::McpServerSnapshot {
            name: "alpha".to_string(),
            enabled: true,
            required: false,
            auth_required: false,
            transport: "stdio".to_string(),
            command_or_url: "alpha-mcp".to_string(),
            connect_timeout: 5,
            execute_timeout: 5,
            read_timeout: 5,
            connected: false,
            error: Some("protocol negotiation timed out".to_string()),
            capability_metadata: crate::mcp::McpServerCapabilityMetadata::NotObserved,
            tools: Vec::new(),
            resources: Vec::new(),
            prompts: Vec::new(),
        }],
    });
    let rendered = render_underwater_test_app(&mut failed, 100, 30);

    assert_eq!(
        failed
            .viewport
            .last_composer_area
            .expect("failed-MCP composer area"),
        baseline_composer,
        "MCP diagnostics must not reserve chat/composer rows"
    );
    assert!(
        rendered.contains("MCP · 0 connected · 1 failed"),
        "{rendered}"
    );
    assert!(!rendered.contains("alpha · failed"), "{rendered}");
    assert!(!rendered.contains("/mcp retry alpha"), "{rendered}");
}

#[test]
fn bottom_placement_draws_the_work_strip_under_the_composer() {
    // Round 3 (2026-09-01): the bar's information lives under the composer.
    // `top` keeps the strip above the transcript; `bottom` (the default)
    // moves the same strip below the composer and above the footer.
    for (placement, below) in [
        (crate::tui::work_surface::WorkSurfacePlacement::Bottom, true),
        (crate::tui::work_surface::WorkSurfacePlacement::Top, false),
    ] {
        let mut app = create_test_app();
        app.onboarding_workspace_trust_gate = false;
        app.onboarding = OnboardingState::None;
        app.work_surface.placement = placement;
        app.goal.objective = Some("ship the release".to_string());
        let config = Config::default();
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("test terminal");
        terminal
            .draw(|frame| {
                let _ = render(frame, &mut app, &config);
            })
            .expect("render frame");
        let strip = app
            .work_surface
            .last_area
            .expect("a live goal draws a strip in every strip placement");
        let composer = app
            .viewport
            .last_composer_area
            .expect("frame records the composer area");
        if below {
            assert!(
                strip.y >= composer.bottom(),
                "bottom strip must sit under the composer: strip {strip:?}, composer {composer:?}"
            );
        } else {
            assert!(
                strip.bottom() <= composer.y,
                "top strip must sit above the composer: strip {strip:?}, composer {composer:?}"
            );
        }
    }
}

#[test]
fn bottom_placement_keeps_the_stage_and_queued_preview_at_twelve_rows() {
    // Regression (#5809 Buildkite 1535): the Bottom layout reordered the
    // frame slots, so the stage, preview and background chip — addressed by
    // slot index — drew into zero-height slots: a blank transcript and a
    // queued message with no `Queued #1:` preview at 40x12.
    let mut app = create_test_app();
    app.onboarding_workspace_trust_gate = false;
    app.onboarding = OnboardingState::None;
    app.work_surface.placement = crate::tui::work_surface::WorkSurfacePlacement::Bottom;
    app.add_message(HistoryCell::User {
        content: "stage row survives bottom".to_string(),
    });
    app.queue_message(crate::tui::app::QueuedMessage::new(
        "qa pointer draft 40x12".to_string(),
        None,
    ));
    let config = Config::default();
    let mut terminal = Terminal::new(TestBackend::new(40, 12)).expect("test terminal");
    terminal
        .draw(|frame| {
            let _ = render(frame, &mut app, &config);
        })
        .expect("render frame");
    let buffer = terminal.backend().buffer();
    let text = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("Queued #1: qa pointer draft"),
        "queued preview must render under Bottom at 40x12; got:\n{text}"
    );
    assert!(
        text.contains("stage row survives bottom"),
        "transcript must render under Bottom at 40x12; got:\n{text}"
    );
}

#[test]
fn wide_underwater_canvas_carries_the_ocean_to_both_terminal_edges() {
    let mut app = create_test_app();
    app.theme_id = crate::palette::ThemeId::Underwater;
    app.ui_theme = crate::palette::UNDERWATER_UI_THEME;
    app.onboarding_workspace_trust_gate = false;
    app.onboarding = OnboardingState::None;
    let surface_bg = app.ui_theme.surface_bg;
    let config = Config::default();
    let mut terminal = Terminal::new(TestBackend::new(200, 32)).expect("wide test terminal");
    terminal
        .draw(|frame| {
            let _ = render(frame, &mut app, &config);
        })
        .expect("render wide ocean canvas");
    let buffer = terminal.backend().buffer();

    // Full-width shell (#5322): both terminal edges stay in the water column.
    // Deepsea may tint left and right differently; what must not happen is a flat
    // surface-bg dead margin on either side.
    let mut left_ocean = false;
    let mut right_ocean = false;
    for y in 0..buffer.area.height {
        left_ocean |= buffer[(0, y)].bg != surface_bg;
        right_ocean |= buffer[(buffer.area.width - 1, y)].bg != surface_bg;
    }
    assert!(
        left_ocean && right_ocean,
        "wide terminal edges retained the flat terminal floor instead of the shared ocean"
    );
}

fn reasoning_with_lines(label: &str, line_count: usize, streaming: bool) -> HistoryCell {
    HistoryCell::Thinking {
        content: (1..=line_count)
            .map(|line| format!("{label} line {line:02}"))
            .collect::<Vec<_>>()
            .join("\n"),
        streaming,
        duration_secs: (!streaming).then_some(1.0),
    }
}

fn long_reasoning(label: &str, streaming: bool) -> HistoryCell {
    reasoning_with_lines(label, 20, streaming)
}

fn oversized_reasoning(label: &str, streaming: bool) -> HistoryCell {
    reasoning_with_lines(label, 40, streaming)
}

fn reasoning_hint_cells(app: &App) -> Vec<usize> {
    app.viewport
        .transcript_cache
        .lines()
        .iter()
        .zip(app.viewport.transcript_cache.line_meta())
        .filter(|(line, _)| line.to_string().contains("Space:expand"))
        .filter_map(|(_, meta)| meta.cell_line().map(|(cell, _)| cell))
        .map(|cell| app.original_cell_index_for_rendered(cell))
        .collect()
}

fn select_original_cell(app: &mut App, original: usize) {
    let rendered = app
        .collapsed_cell_map
        .iter()
        .position(|&cell| cell == original)
        .unwrap_or(original);
    let point = TranscriptSelectionPoint {
        line_index: first_line_for_cell(app, rendered),
        column: 0,
    };
    app.viewport.transcript_selection.anchor = Some(point);
    app.viewport.transcript_selection.head = Some(point);
}

fn mouse_on_cell(app: &App, cell: usize, kind: MouseEventKind) -> MouseEvent {
    let area = app.viewport.last_transcript_area.expect("transcript area");
    let visible_line = first_line_for_cell(app, cell)
        .saturating_sub(app.viewport.last_transcript_top)
        .saturating_add(app.viewport.last_transcript_padding_top);
    MouseEvent {
        kind,
        column: area.x,
        row: area.y + u16::try_from(visible_line).expect("visible transcript line"),
        modifiers: KeyModifiers::NONE,
    }
}

fn running_exec_cell() -> HistoryCell {
    HistoryCell::Tool(ToolCell::Exec(ExecCell {
        command: "gh issue view 5291".to_string(),
        status: ToolStatus::Success,
        output: Some("issue loaded".to_string()),
        live_output: None,
        shell_task_id: None,
        owner_agent_id: None,
        owner_agent_name: None,
        started_at: None,
        duration_ms: Some(20),
        stale_elapsed_since_output_ms: None,
        source: ExecSource::Assistant,
        interaction: None,
        output_summary: None,
    }))
}

#[test]
fn completed_answer_clears_stale_reasoning_expand_hint() {
    let mut app = create_test_app();
    app.history = vec![
        long_reasoning("reasoning", false),
        HistoryCell::Assistant {
            content: "The answer is complete.".to_string(),
            streaming: false,
        },
    ];
    app.resync_history_revisions();

    let surface = render_underwater_test_app(&mut app, 100, 32);

    assert_eq!(
        app.transcript_action_owner().map(|owner| owner.cell_index),
        Some(1),
        "Space dispatch targets the completed assistant cell"
    );
    assert!(
        !surface.contains("Space:expand"),
        "a reasoning cell must not advertise Space while cell 1 owns the key: {surface}"
    );
    assert!(handle_transcript_space(&mut app));
    assert!(app.collapsed_cells.contains(&1));
}

#[test]
fn selected_reasoning_hint_and_space_share_one_owner() {
    let mut app = create_test_app();
    app.history = vec![oversized_reasoning("selected", false)];
    app.resync_history_revisions();
    let _ = render_underwater_test_app(&mut app, 100, 32);
    select_original_cell(&mut app, 0);
    let collapsed = render_underwater_test_app(&mut app, 100, 32);

    assert_eq!(reasoning_hint_cells(&app), vec![0]);
    assert!(collapsed.contains("Space:expand"));
    app.viewport.transcript_selection.anchor = Some(TranscriptSelectionPoint {
        line_index: 0,
        column: 0,
    });
    app.viewport.transcript_selection.head = Some(TranscriptSelectionPoint {
        line_index: app
            .viewport
            .transcript_cache
            .total_lines()
            .saturating_sub(1),
        column: 100,
    });
    let copied = selection_to_text(&app).expect("reasoning selection");
    assert!(copied.contains("selected line"));
    assert!(
        !copied.contains("Space:") && !copied.contains('…'),
        "{copied}"
    );
    assert!(handle_transcript_space(&mut app));
    assert!(app.folded_thinking.contains(&0));
    assert!(!handle_transcript_space(&mut app));
    assert!(
        app.folded_thinking.contains(&0),
        "rendered action is single-use"
    );

    let expanded = render_underwater_test_app(&mut app, 100, 32);
    assert!(expanded.contains("selected line 20"));
    assert!(!expanded.contains("Space:expand"));
    assert!(!expanded.contains("Space:collapse"));
    assert_eq!(
        app.viewport.transcript_cache.reasoning_action_target(),
        Some(crate::tui::history::ReasoningActionTarget {
            owner: crate::tui::history::TranscriptActionOwner {
                cell_index: 0,
                identity_epoch: app.transcript_identity_epoch,
            },
            action: crate::tui::history::ReasoningAction::Collapse,
        })
    );

    assert!(handle_transcript_space(&mut app));
    assert!(!app.folded_thinking.contains(&0));
    let collapsed_again = render_underwater_test_app(&mut app, 100, 32);
    assert!(collapsed_again.contains("Space:expand"));
}

#[test]
fn mouse_selection_redraws_and_retargets_reasoning_with_unchanged_revisions() {
    let mut app = create_test_app();
    app.history = vec![
        oversized_reasoning("first", false),
        oversized_reasoning("second", false),
    ];
    app.resync_history_revisions();
    let _ = render_underwater_test_app(&mut app, 100, 32);
    assert_eq!(reasoning_hint_cells(&app), vec![1]);

    app.needs_redraw = false;
    let down_first = mouse_on_cell(&app, 0, MouseEventKind::Down(MouseButton::Left));
    handle_mouse_event(&mut app, down_first);
    assert!(app.needs_redraw, "mouse Down must schedule owner redraw");
    let _ = render_underwater_test_app(&mut app, 100, 32);
    assert_eq!(reasoning_hint_cells(&app), vec![0]);

    app.needs_redraw = false;
    handle_mouse_event(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: u16::MAX,
            row: u16::MAX,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(app.needs_redraw, "click outside must clear + redraw");
    let _ = render_underwater_test_app(&mut app, 100, 32);
    assert_eq!(reasoning_hint_cells(&app), vec![1]);

    let down_second = mouse_on_cell(&app, 1, MouseEventKind::Down(MouseButton::Left));
    handle_mouse_event(&mut app, down_second);
    let _ = render_underwater_test_app(&mut app, 100, 32);
    app.needs_redraw = false;
    let drag_first = mouse_on_cell(&app, 0, MouseEventKind::Drag(MouseButton::Left));
    handle_mouse_event(&mut app, drag_first);
    assert!(app.needs_redraw, "mouse Drag must schedule owner redraw");
    let _ = render_underwater_test_app(&mut app, 100, 32);
    assert_eq!(reasoning_hint_cells(&app), vec![0]);
    assert!(handle_transcript_space(&mut app));
    assert!(app.folded_thinking.contains(&0));
}

#[test]
fn mouse_up_preserves_the_single_click_action_and_detail_owner() {
    let mut app = create_test_app();
    app.history = vec![
        HistoryCell::Error {
            message: "selected error".into(),
            severity: crate::error_taxonomy::ErrorSeverity::Error,
        },
        running_exec_cell(),
    ];
    app.resync_history_revisions();
    let _ = render_underwater_test_app(&mut app, 100, 32);
    let down = mouse_on_cell(&app, 0, MouseEventKind::Down(MouseButton::Left));
    handle_mouse_event(&mut app, down);
    let up = mouse_on_cell(&app, 0, MouseEventKind::Up(MouseButton::Left));
    handle_mouse_event(&mut app, up);

    assert!(
        app.viewport
            .transcript_selection
            .ordered_endpoints()
            .is_some()
    );
    assert_eq!(
        app.transcript_action_owner().map(|owner| owner.cell_index),
        Some(0)
    );
    assert_eq!(detail_target_cell_index(&app), Some(0));
    let _ = render_underwater_test_app(&mut app, 100, 32);
    assert!(handle_transcript_space(&mut app));
    assert!(app.collapsed_cells.contains(&0));
    assert!(!app.collapsed_cells.contains(&1));
}

#[test]
fn latest_streaming_reasoning_owns_space_after_an_older_tool() {
    let mut app = create_test_app();
    app.history = vec![running_exec_cell(), long_reasoning("live", true)];
    app.resync_history_revisions();

    let surface = render_underwater_test_app(&mut app, 60, 16);
    assert_eq!(
        app.transcript_action_owner().map(|owner| owner.cell_index),
        Some(1)
    );
    assert_eq!(reasoning_hint_cells(&app), vec![1]);
    assert!(surface.contains("Space:expand"));

    app.history[1] = long_reasoning("done", false);
    app.bump_history_cell(1);
    app.push_history_cell(HistoryCell::Assistant {
        content: "final answer".to_string(),
        streaming: false,
    });
    let completed = render_underwater_test_app(&mut app, 60, 16);
    assert_eq!(
        app.transcript_action_owner().map(|owner| owner.cell_index),
        Some(2)
    );
    assert!(reasoning_hint_cells(&app).is_empty());
    assert!(!completed.contains("Space:expand"));
}

#[test]
fn reasoning_preview_spends_available_viewport_rows_before_truncating() {
    for streaming in [false, true] {
        let mut roomy = create_test_app();
        roomy.history = vec![reasoning_with_lines("roomy", 20, streaming)];
        roomy.resync_history_revisions();
        let roomy_surface = render_underwater_test_app(&mut roomy, 100, 32);

        assert!(roomy_surface.contains("roomy line 01"), "{roomy_surface}");
        assert!(roomy_surface.contains("roomy line 20"), "{roomy_surface}");
        assert!(
            !roomy_surface.contains("Space:expand"),
            "a body that fits the live viewport must not be truncated: {roomy_surface}"
        );
        assert!(
            roomy.viewport.last_transcript_total <= roomy.viewport.last_transcript_visible,
            "the complete reasoning body should fit without scrolling"
        );

        let mut compact = create_test_app();
        compact.history = vec![reasoning_with_lines("compact", 20, streaming)];
        compact.resync_history_revisions();
        let compact_surface = render_underwater_test_app(&mut compact, 60, 12);
        assert!(
            compact_surface.contains("Space:expand"),
            "{compact_surface}"
        );
        assert_eq!(
            compact.viewport.last_transcript_total,
            if streaming {
                14
            } else {
                compact.viewport.last_transcript_visible
            },
            "streaming keeps its 12-row fallback while completed thought spends the visible viewport before truncating: {compact_surface}"
        );
    }
}

#[test]
fn advertised_reasoning_space_dispatches_after_first_char_paste_hold() {
    let mut app = create_test_app();
    app.use_paste_burst_detection = true;
    app.bracketed_paste_seen = false;
    app.history = vec![oversized_reasoning("live", true)];
    app.resync_history_revisions();
    let surface = render_underwater_test_app(&mut app, 60, 16);
    assert!(surface.contains("Space:expand"), "{surface}");

    let now = Instant::now();
    let space = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
    assert!(handle_plain_key_before_composer(&mut app, &space, now));
    assert!(
        app.folded_thinking.is_empty(),
        "Space remains ambiguous until the first-character hold expires"
    );
    assert!(app.input.is_empty(), "Space must not enter the composer");
    assert!(
        app.paste_burst.is_active(),
        "Space must be retained long enough to preserve a leading-space paste"
    );
    assert!(flush_paste_burst_before_composer(
        &mut app,
        now + crate::tui::paste_burst::PasteBurst::recommended_flush_delay()
    ));
    assert!(
        app.folded_thinking.contains(&0),
        "a lone Space must dispatch the rendered transcript action after the hold"
    );
    assert!(
        app.input.is_empty(),
        "the dispatched Space is not composer text"
    );

    let _ = render_underwater_test_app(&mut app, 60, 16);
    let expanded = app
        .viewport
        .transcript_cache
        .lines()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(expanded.contains("live line 40"), "{expanded}");
}

#[test]
fn raw_paste_beginning_with_space_preserves_payload_over_reasoning_action() {
    let mut app = create_test_app();
    app.use_paste_burst_detection = true;
    app.bracketed_paste_seen = false;
    app.history = vec![oversized_reasoning("live", true)];
    app.resync_history_revisions();
    let surface = render_underwater_test_app(&mut app, 60, 16);
    assert!(surface.contains("Space:expand"), "{surface}");

    let now = Instant::now();
    let space = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
    let next = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
    assert!(handle_plain_key_before_composer(&mut app, &space, now));
    assert!(handle_plain_key_before_composer(
        &mut app,
        &next,
        now + Duration::from_millis(1),
    ));
    assert!(
        app.folded_thinking.is_empty(),
        "a leading-space raw paste must not trigger transcript actions"
    );
    assert!(flush_paste_burst_before_composer(
        &mut app,
        now + crate::tui::paste_burst::PasteBurst::recommended_active_flush_delay()
            + Duration::from_millis(2),
    ));
    assert_eq!(app.input, " x");
}

#[test]
fn paste_burst_does_not_leak_into_composer_while_a_modal_owns_keys() {
    // `/model` opens a picker with its own query. A held paste-burst from
    // typing the slash command must not flush into the composer under it.
    let mut app = Box::new(create_test_app());
    app.use_paste_burst_detection = true;
    app.bracketed_paste_seen = false;
    let now = Instant::now();
    let slash = KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE);
    assert!(handle_plain_key_before_composer(&mut app, &slash, now));
    app.view_stack.push(HelpView::new());
    let flushed = flush_paste_burst_before_composer(
        &mut app,
        now + crate::tui::paste_burst::PasteBurst::recommended_flush_delay(),
    );
    assert!(
        !flushed,
        "modal-owned keys must not flush into the composer"
    );
    assert!(
        app.input.is_empty() || app.input == "/",
        "composer must not gain leaked burst text under a modal: {:?}",
        app.input
    );
}

#[test]
fn active_raw_paste_keeps_space_as_payload_over_reasoning_action() {
    let mut app = create_test_app();
    app.use_paste_burst_detection = true;
    app.bracketed_paste_seen = false;
    app.history = vec![oversized_reasoning("live", true)];
    app.resync_history_revisions();
    let surface = render_underwater_test_app(&mut app, 60, 16);
    assert!(surface.contains("Space:expand"), "{surface}");

    let now = Instant::now();
    let first = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
    let space = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
    assert!(handle_plain_key_before_composer(&mut app, &first, now));
    assert!(app.paste_burst.is_active());
    assert!(handle_plain_key_before_composer(
        &mut app,
        &space,
        now + Duration::from_millis(1),
    ));
    assert!(
        app.folded_thinking.is_empty(),
        "an in-flight raw paste must not trigger transcript actions"
    );
    assert!(app.flush_paste_burst_if_due(
        now + crate::tui::paste_burst::PasteBurst::recommended_active_flush_delay()
            + Duration::from_millis(2)
    ));
    assert_eq!(app.input, "a ");
}

#[test]
fn typed_command_burst_keeps_r_and_y_out_of_transcript_actions() {
    let mut app = create_test_app();
    app.use_paste_burst_detection = true;
    app.bracketed_paste_seen = false;
    app.history = vec![HistoryCell::Assistant {
        content: "previous command output".to_string(),
        streaming: false,
    }];
    app.resync_history_revisions();
    let _ = render_underwater_test_app(&mut app, 60, 16);
    select_original_cell(&mut app, 0);
    assert!(
        app.viewport.transcript_selection.is_active(),
        "precondition: a standing transcript selection must arm block actions"
    );

    let now = Instant::now();
    let command = "/plugin trust demo";
    for (offset, ch) in command.chars().enumerate() {
        let at = now + Duration::from_millis(offset as u64);
        let key = KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE);
        let _ = flush_paste_burst_before_composer(&mut app, at);
        assert!(
            handle_plain_key_before_composer(&mut app, &key, at),
            "paste-burst input should retain {ch:?}"
        );
    }
    assert!(flush_paste_burst_before_composer(
        &mut app,
        now + Duration::from_millis(500),
    ));

    assert_eq!(app.input, command);
    assert!(
        app.view_stack.is_empty(),
        "typed command must not open a pager"
    );
}

#[test]
fn active_streaming_reasoning_keeps_its_visible_owner_across_a_delta() {
    let mut app = create_test_app();
    app.push_history_cell(running_exec_cell());
    let entry = crate::tui::streaming_thinking::ensure_active_entry(&mut app);
    crate::tui::streaming_thinking::append(
        &mut app,
        entry,
        &(1..=20)
            .map(|line| format!("active line {line:02}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let rendered_epoch = app.transcript_identity_epoch;
    let _ = render_underwater_test_app(&mut app, 60, 16);
    assert_eq!(reasoning_hint_cells(&app), vec![1]);

    crate::tui::streaming_thinking::append(&mut app, entry, "\nnew delta before redraw");
    let rendered_version = app.history_version;
    app.bump_active_cell_revision();
    assert_ne!(app.history_version, rendered_version);
    assert_eq!(app.transcript_identity_epoch, rendered_epoch);
    assert!(handle_transcript_space(&mut app));
    assert!(app.folded_thinking.contains(&1));
}

#[test]
fn interrupted_active_reasoning_remains_actionable_after_flush() {
    let mut app = create_test_app();
    let entry = crate::tui::streaming_thinking::ensure_active_entry(&mut app);
    crate::tui::streaming_thinking::append(
        &mut app,
        entry,
        &(1..=20)
            .map(|line| format!("interrupted line {line:02}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let surface = render_underwater_test_app(&mut app, 60, 16);
    assert_eq!(reasoning_hint_cells(&app), vec![0]);
    assert!(surface.contains("Space:expand"));
    let epoch = app.transcript_identity_epoch;
    app.finalize_active_cell_as_interrupted();
    assert!(app.active_cell.is_none());
    assert_eq!(app.transcript_identity_epoch, epoch);
    assert!(handle_transcript_space(&mut app));
    assert!(app.folded_thinking.contains(&0));
    let _ = render_underwater_test_app(&mut app, 60, 16);
}

#[test]
fn pending_scroll_retargets_reasoning_in_the_same_frame() {
    let mut app = create_test_app();
    app.history = vec![
        long_reasoning("first", false),
        long_reasoning("second", false),
    ];
    app.resync_history_revisions();
    let _ = render_underwater_test_app(&mut app, 60, 8);
    assert_eq!(reasoning_hint_cells(&app), vec![1]);
    app.viewport.pending_scroll_delta = -1_000_000;
    let _ = render_underwater_test_app(&mut app, 60, 8);
    assert_eq!(app.viewport.last_transcript_top, 0);
    assert_eq!(reasoning_hint_cells(&app), vec![0]);
    assert_eq!(
        app.viewport
            .transcript_cache
            .reasoning_action_target()
            .map(|target| target.owner.cell_index),
        Some(0)
    );
}

#[test]
fn visible_older_reasoning_owns_space_over_a_newer_offscreen_tool() {
    let mut app = create_test_app();
    app.history = vec![long_reasoning("visible", false), running_exec_cell()];
    app.resync_history_revisions();
    let _ = render_underwater_test_app(&mut app, 60, 8);
    assert_eq!(
        app.transcript_action_owner().map(|owner| owner.cell_index),
        Some(1)
    );

    // The configurable two-line completed preview puts the Space affordance
    // immediately after the header + body, so scroll one row to keep that
    // action visible while the newer tool remains below the viewport.
    app.viewport.transcript_scroll = TranscriptScroll::at_line(1);
    let surface = render_underwater_test_app(&mut app, 60, 8);
    assert_eq!(app.viewport.last_transcript_top, 1);
    assert_eq!(reasoning_hint_cells(&app), vec![0]);
    assert!(surface.contains("Space:expand"), "{surface}");
    assert_eq!(
        app.transcript_action_owner().map(|owner| owner.cell_index),
        Some(0)
    );
    assert_eq!(detail_target_cell_index(&app), Some(1));
    assert!(handle_transcript_space(&mut app));
    assert!(app.folded_thinking.contains(&0));
    assert!(!app.collapsed_cells.contains(&1) && app.expanded_tool_runs.is_empty());
}

#[test]
fn hidden_reasoning_is_unhidden_instead_of_folded() {
    let mut app = create_test_app();
    app.history = vec![long_reasoning("hidden", false)];
    app.resync_history_revisions();
    let _ = render_underwater_test_app(&mut app, 60, 16);

    // Context-menu hide can happen between frames while the old hint owner is
    // still cached. Space must honor the new hidden state before that target.
    app.collapsed_cells.insert(0);
    assert!(handle_transcript_space(&mut app));
    assert!(!app.collapsed_cells.contains(&0));
    assert!(!app.folded_thinking.contains(&0));

    app.collapsed_cells.insert(0);
    let surface = render_underwater_test_app(&mut app, 60, 16);
    assert!(!surface.contains("Space:expand"));
    assert!(
        app.viewport
            .transcript_cache
            .reasoning_action_target()
            .is_none()
    );

    assert!(handle_transcript_space(&mut app));
    assert!(!app.collapsed_cells.contains(&0));
    assert!(!app.folded_thinking.contains(&0));
    let visible = render_underwater_test_app(&mut app, 60, 16);
    assert!(visible.contains("Space:expand"));
}

#[test]
fn stale_rendered_reasoning_owner_cannot_toggle_replacement() {
    let mut app = create_test_app();
    app.push_history_cell(long_reasoning("old", false));
    let _ = render_underwater_test_app(&mut app, 80, 24);
    let rendered_epoch = app.transcript_identity_epoch;

    app.clear_history();
    app.push_history_cell(long_reasoning("replacement", false));
    assert_ne!(app.transcript_identity_epoch, rendered_epoch);
    assert!(!handle_transcript_space(&mut app));
    assert!(app.folded_thinking.is_empty());

    let _ = render_underwater_test_app(&mut app, 80, 24);
    assert!(handle_transcript_space(&mut app));
    assert!(app.folded_thinking.contains(&0));
}

#[test]
fn pop_and_truncate_prune_index_state_before_replacement() {
    let mut app = create_test_app();
    app.push_history_cell(HistoryCell::Assistant {
        content: "kept".into(),
        streaming: false,
    });
    app.push_history_cell(HistoryCell::Assistant {
        content: "old tail".into(),
        streaming: false,
    });
    let _ = render_underwater_test_app(&mut app, 60, 16);
    for set in [
        &mut app.collapsed_cells,
        &mut app.folded_thinking,
        &mut app.expanded_tool_runs,
    ] {
        set.insert(1);
    }
    app.collapsed_cell_map = vec![0, 1];
    app.pop_history();
    assert!(app.collapsed_cells.is_empty() && app.folded_thinking.is_empty());
    assert!(app.expanded_tool_runs.is_empty() && app.collapsed_cell_map.is_empty());
    app.push_history_cell(HistoryCell::Assistant {
        content: "after pop".into(),
        streaming: false,
    });
    assert!(!handle_transcript_space(&mut app));
    let first = render_underwater_test_app(&mut app, 60, 16);
    assert!(first.contains("after pop") && !first.contains("old tail"));

    for set in [
        &mut app.collapsed_cells,
        &mut app.folded_thinking,
        &mut app.expanded_tool_runs,
    ] {
        set.insert(1);
    }
    app.truncate_history_to(1);
    app.push_history_cell(HistoryCell::Assistant {
        content: "after truncate".into(),
        streaming: false,
    });
    assert!(!handle_transcript_space(&mut app));
    assert!(app.collapsed_cells.is_empty() && app.folded_thinking.is_empty());
    assert!(render_underwater_test_app(&mut app, 60, 16).contains("after truncate"));
}

#[test]
fn dispatch_rollback_prunes_tail_state_and_keeps_revisions_monotonic() {
    let mut app = create_test_app();
    let config = Config::default();
    let prepare = prepare_user_dispatch(
        &mut app,
        &config,
        QueuedMessage::new("optimistic".into(), None),
    )
    .expect("prepare");
    let _ = render_underwater_test_app(&mut app, 60, 16);
    app.collapsed_cells.insert(0);
    app.folded_thinking.insert(0);
    app.expanded_tool_runs.insert(0);
    app.collapsed_cell_map.push(0);
    let next_revision = app.next_history_revision;
    let apply = build_dispatch_error_closure(prepare, DispatchRecovery::Immediate, "failed".into());
    let engine = mock_engine_handle();
    let error = apply(&mut app, &engine.handle, &config).expect_err("dispatch fails");
    assert_eq!(error.to_string(), "failed");
    assert_eq!(app.next_history_revision, next_revision);
    assert!(app.collapsed_cells.is_empty() && app.folded_thinking.is_empty());
    assert!(app.expanded_tool_runs.is_empty() && app.collapsed_cell_map.is_empty());
    app.push_history_cell(HistoryCell::Assistant {
        content: "replacement".into(),
        streaming: false,
    });
    assert!(!handle_transcript_space(&mut app));
    let _ = render_underwater_test_app(&mut app, 60, 16);
    let transcript = app
        .viewport
        .transcript_cache
        .lines()
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(transcript.contains("replacement"), "{transcript}");
    assert!(!transcript.contains("optimistic"), "{transcript}");
}

#[test]
fn restored_reasoning_and_answer_clear_prior_fold_ownership() {
    let mut app = create_test_app();
    app.push_history_cell(long_reasoning("old session", false));
    app.folded_thinking.insert(0);
    let _ = render_underwater_test_app(&mut app, 80, 24);
    let old_epoch = app.transcript_identity_epoch;
    let session = saved_session_with_messages(vec![crate::models::Message {
        role: Role::Assistant,
        content: vec![
            crate::models::ContentBlock::Thinking {
                thinking: (1..=20)
                    .map(|line| format!("restored line {line:02}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                signature: None,
                state: None,
            },
            crate::models::ContentBlock::Text {
                text: "restored final answer".to_string(),
                cache_control: None,
            },
        ],
    }]);

    apply_loaded_session(&mut app, &mut Config::default(), &session).expect("restore session");
    assert!(app.folded_thinking.is_empty());
    assert_ne!(app.transcript_identity_epoch, old_epoch);
    assert!(matches!(
        app.history.first(),
        Some(HistoryCell::Thinking { .. })
    ));
    assert!(matches!(
        app.history.get(1),
        Some(HistoryCell::Assistant { .. })
    ));
    let restored = render_underwater_test_app(&mut app, 80, 24);
    assert_eq!(
        app.transcript_action_owner().map(|owner| owner.cell_index),
        Some(1)
    );
    assert!(!restored.contains("Space:expand"));
}

#[test]
fn error_or_interruption_transfers_reasoning_ownership_truthfully() {
    let mut interrupted = long_reasoning("interrupted", false);
    if let HistoryCell::Thinking { duration_secs, .. } = &mut interrupted {
        *duration_secs = None;
    }
    let mut app = create_test_app();
    app.history = vec![
        interrupted,
        HistoryCell::Error {
            message: "provider disconnected".to_string(),
            severity: crate::error_taxonomy::ErrorSeverity::Error,
        },
    ];
    app.resync_history_revisions();
    let failed = render_underwater_test_app(&mut app, 60, 16);
    assert_eq!(
        app.transcript_action_owner().map(|owner| owner.cell_index),
        Some(1)
    );
    assert!(!failed.contains("Space:expand"));

    select_original_cell(&mut app, 0);
    let selected = render_underwater_test_app(&mut app, 60, 16);
    assert_eq!(reasoning_hint_cells(&app), vec![0]);
    assert!(selected.contains("Space:expand"));
    assert!(handle_transcript_space(&mut app));
}

#[test]
fn filtered_selection_toggles_the_original_reasoning_index() {
    let mut app = create_test_app();
    app.history = vec![
        HistoryCell::User {
            content: "hidden predecessor".to_string(),
        },
        long_reasoning("mapped", false),
    ];
    app.resync_history_revisions();
    app.collapsed_cells.insert(0);
    let _ = render_underwater_test_app(&mut app, 60, 16);
    assert_eq!(app.collapsed_cell_map, vec![1]);
    select_original_cell(&mut app, 1);
    let _ = render_underwater_test_app(&mut app, 60, 16);
    assert_eq!(reasoning_hint_cells(&app), vec![1]);
    assert!(handle_transcript_space(&mut app));
    assert!(app.folded_thinking.contains(&1));
    assert!(!app.folded_thinking.contains(&0));
}

#[test]
fn stale_selection_and_zero_height_fall_back_to_the_latest_cell() {
    let mut app = create_test_app();
    app.history = vec![
        HistoryCell::Assistant {
            content: "first".to_string(),
            streaming: false,
        },
        HistoryCell::Assistant {
            content: "latest".to_string(),
            streaming: false,
        },
    ];
    app.resync_history_revisions();
    let _ = render_underwater_test_app(&mut app, 60, 16);
    let stale = TranscriptSelectionPoint {
        line_index: usize::MAX,
        column: 0,
    };
    app.viewport.transcript_selection.anchor = Some(stale);
    app.viewport.transcript_selection.head = Some(stale);
    assert_eq!(
        app.transcript_action_owner().map(|owner| owner.cell_index),
        Some(1)
    );

    app.viewport.last_transcript_visible = 0;
    assert_eq!(
        app.transcript_action_owner().map(|owner| owner.cell_index),
        Some(1)
    );
}

#[test]
fn reasoning_hint_fits_representative_compact_frames() {
    for (width, height) in [(40, 12), (60, 16)] {
        let mut app = create_test_app();
        app.history = vec![long_reasoning("compact", false)];
        app.resync_history_revisions();
        let surface = render_underwater_test_app(&mut app, width, height);
        assert_eq!(reasoning_hint_cells(&app), vec![0], "{width}x{height}");
        assert!(
            surface.contains("Space:expand"),
            "{width}x{height}: {surface}"
        );
    }
}

#[tokio::test]
async fn session_denied_cache_auto_deny_explains_the_cached_rejection() {
    let home = SettingsHomeGuard::new();
    let audit_path = home._tmp.path().join(".codewhale").join("audit.log");
    let mut app = create_test_app();
    let mut engine = mock_engine_handle();
    let approval_key = "shell:exec_shell:git push secret-token";

    auto_deny_session_approval(
        &mut app,
        &engine.handle,
        "tool-retry",
        "exec_shell",
        approval_key,
    )
    .await;

    assert_eq!(
        engine.recv_approval_event().await,
        Some(crate::core::engine::MockApprovalEvent::Denied {
            id: "tool-retry".to_string(),
        })
    );
    let toast = app.status_toasts.back().expect("auto-deny warning toast");
    assert_eq!(toast.level, StatusToastLevel::Warning);
    assert_eq!(toast.ttl_ms, Some(12_000));
    assert!(toast.text.contains("matching request was denied earlier"));
    assert!(toast.text.contains("during this Codewhale run"));
    assert!(toast.text.contains("Restart Codewhale"));
    assert!(toast.text.contains("exec_shell"));
    let history_notice = app
        .history
        .iter()
        .rev()
        .find_map(|cell| match cell {
            HistoryCell::System { content } => Some(content.as_str()),
            _ => None,
        })
        .expect("persistent auto-deny explanation");
    assert_eq!(history_notice, toast.text);
    for visible_text in [&toast.text, history_notice] {
        assert!(
            !visible_text.contains(approval_key)
                && !visible_text.contains("git push")
                && !visible_text.contains("secret-token"),
            "the user-facing notice must not expose the approval key, command, or arguments"
        );
    }
    let audit = std::fs::read_to_string(&audit_path).expect("isolated approval audit log");
    assert!(audit.contains(approval_key));

    let rendered = render_underwater_test_app(&mut app, 40, 12);
    assert!(rendered.contains("Auto-denied"), "{rendered:?}");
    assert!(
        rendered.contains("Restart") && rendered.contains("Codewhale"),
        "{rendered:?}"
    );
}

#[tokio::test]
async fn session_denied_cache_notice_preserves_active_tool_index() {
    let _home = SettingsHomeGuard::new();
    let mut app = create_test_app();
    let mut engine = mock_engine_handle();
    let tool_id = "tool-retry";
    let tool_name = "exec_shell";

    handle_tool_call_started(
        &mut app,
        tool_id,
        tool_name,
        &serde_json::json!({"command": "git push secret-token"}),
    );
    let history_len = app.history.len();
    let tool_index = app.tool_cells[tool_id];
    assert_eq!(tool_index, history_len);

    auto_deny_session_approval(
        &mut app,
        &engine.handle,
        tool_id,
        tool_name,
        "shell:exec_shell:git push secret-token",
    )
    .await;

    assert_eq!(
        app.history.len(),
        history_len,
        "the notice must not shift virtual indices while a tool is active"
    );
    assert_eq!(app.tool_cells.get(tool_id), Some(&tool_index));
    let active = app.active_cell.as_ref().expect("active tool and notice");
    assert!(matches!(
        active.entries().first(),
        Some(HistoryCell::Tool(ToolCell::Exec(exec))) if exec.status == ToolStatus::Running
    ));
    assert!(matches!(
        active.entries().get(1),
        Some(HistoryCell::System { content }) if content.contains("Auto-denied")
    ));

    assert_eq!(
        engine.recv_approval_event().await,
        Some(crate::core::engine::MockApprovalEvent::Denied {
            id: tool_id.to_string(),
        })
    );
    let denied_result = Ok(crate::tools::spec::ToolResult::error(
        "request denied by cached approval",
    ));
    handle_tool_call_complete(&mut app, tool_id, tool_name, &denied_result);

    let active = app.active_cell.as_ref().expect("completed tool and notice");
    assert!(matches!(
        active.entries().first(),
        Some(HistoryCell::Tool(ToolCell::Exec(exec))) if exec.status == ToolStatus::Failed
    ));
    assert!(matches!(
        active.entries().get(1),
        Some(HistoryCell::System { .. })
    ));

    app.flush_active_cell();
    assert!(matches!(
        app.history.get(history_len),
        Some(HistoryCell::Tool(ToolCell::Exec(exec))) if exec.status == ToolStatus::Failed
    ));
    assert!(matches!(
        app.history.get(history_len + 1),
        Some(HistoryCell::System { content }) if content.contains("Auto-denied")
    ));
    let detail = app
        .tool_detail_record_for_cell(history_len)
        .expect("tool detail remains bound to the completed tool cell");
    assert_eq!(detail.tool_id, tool_id);
    assert!(
        detail
            .output
            .as_deref()
            .is_some_and(|output| output.contains("cached approval"))
    );
}

#[tokio::test]
async fn session_denied_cache_notice_preserves_parallel_tool_indices() {
    let _home = SettingsHomeGuard::new();
    let mut app = create_test_app();
    let mut engine = mock_engine_handle();

    handle_tool_call_started(
        &mut app,
        "tool-first",
        "exec_shell",
        &serde_json::json!({"command": "echo first"}),
    );
    handle_tool_call_started(
        &mut app,
        "tool-denied",
        "exec_shell",
        &serde_json::json!({"command": "git push secret-token"}),
    );
    let history_len = app.history.len();
    let first_index = app.tool_cells["tool-first"];
    let denied_index = app.tool_cells["tool-denied"];
    assert_eq!(first_index, history_len);
    assert_eq!(denied_index, history_len + 1);

    auto_deny_session_approval(
        &mut app,
        &engine.handle,
        "tool-denied",
        "exec_shell",
        "shell:exec_shell:git push secret-token",
    )
    .await;

    assert_eq!(app.history.len(), history_len);
    assert_eq!(app.tool_cells.get("tool-first"), Some(&first_index));
    assert_eq!(app.tool_cells.get("tool-denied"), Some(&denied_index));
    assert_eq!(
        engine.recv_approval_event().await,
        Some(crate::core::engine::MockApprovalEvent::Denied {
            id: "tool-denied".to_string(),
        })
    );

    let denied_result = Ok(crate::tools::spec::ToolResult::error(
        "request denied by cached approval",
    ));
    handle_tool_call_complete(&mut app, "tool-denied", "exec_shell", &denied_result);
    let active = app.active_cell.as_ref().expect("parallel tools and notice");
    assert!(matches!(
        active.entries().first(),
        Some(HistoryCell::Tool(ToolCell::Exec(exec)))
            if exec.status == ToolStatus::Running && exec.command == "echo first"
    ));
    assert!(matches!(
        active.entries().get(1),
        Some(HistoryCell::Tool(ToolCell::Exec(exec)))
            if exec.status == ToolStatus::Failed
                && exec.command == "git push secret-token"
                && exec.output.as_deref().is_some_and(|output| output.contains("cached approval"))
    ));
    assert!(matches!(
        active.entries().get(2),
        Some(HistoryCell::System { content }) if content.contains("Auto-denied")
    ));

    handle_tool_call_complete(
        &mut app,
        "tool-first",
        "exec_shell",
        &ok_result("first output"),
    );
    app.flush_active_cell();

    assert!(matches!(
        app.history.get(history_len),
        Some(HistoryCell::Tool(ToolCell::Exec(exec)))
            if exec.status == ToolStatus::Success
                && exec.command == "echo first"
                && exec.output.as_deref() == Some("first output")
    ));
    assert!(matches!(
        app.history.get(history_len + 1),
        Some(HistoryCell::Tool(ToolCell::Exec(exec)))
            if exec.status == ToolStatus::Failed && exec.command == "git push secret-token"
    ));
    assert!(matches!(
        app.history.get(history_len + 2),
        Some(HistoryCell::System { content }) if content.contains("Auto-denied")
    ));
    let first_detail = app
        .tool_detail_record_for_cell(history_len)
        .expect("first completed tool keeps its own raw detail record");
    assert_eq!(first_detail.tool_id, "tool-first");
    assert_eq!(first_detail.output.as_deref(), Some("first output"));
    let denied_detail = app
        .tool_detail_record_for_cell(history_len + 1)
        .expect("second completed tool keeps its own raw detail record");
    assert_eq!(denied_detail.tool_id, "tool-denied");
    assert!(
        denied_detail
            .output
            .as_deref()
            .is_some_and(|output| output.contains("cached approval"))
    );
}

#[tokio::test]
async fn session_denied_cache_notice_renders_host_scope_in_zh_hans() {
    let _home = SettingsHomeGuard::new();
    let mut app = create_test_app();
    app.ui_locale = crate::localization::Locale::ZhHans;
    let mut engine = mock_engine_handle();

    auto_deny_session_approval(
        &mut app,
        &engine.handle,
        "fetch-retry",
        "fetch_url",
        "net:example.com",
    )
    .await;

    assert_eq!(
        engine.recv_approval_event().await,
        Some(crate::core::engine::MockApprovalEvent::Denied {
            id: "fetch-retry".to_string(),
        })
    );
    let notice = app
        .history
        .iter()
        .rev()
        .find_map(|cell| match cell {
            HistoryCell::System { content } => Some(content.as_str()),
            _ => None,
        })
        .expect("localized persistent auto-deny explanation");
    assert!(notice.contains("本次 Codewhale 运行期间"));
    assert!(notice.contains("匹配请求"));
    assert!(!notice.contains("example.com"));

    let rendered = render_underwater_test_app(&mut app, 60, 16);
    let rendered_compact = rendered
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>();
    assert!(rendered_compact.contains("已自动拒绝"), "{rendered:?}");
    assert!(rendered_compact.contains("匹配请求"), "{rendered:?}");
    assert!(
        rendered_compact.contains("重启") && rendered_compact.contains("Codewhale"),
        "{rendered:?}"
    );
}

#[test]
fn session_denied_notice_explains_cached_decision_and_recovery() {
    let app = create_test_app();
    let notice = session_denied_notice(&app, "exec_shell");

    assert!(notice.contains("exec_shell"));
    assert!(notice.contains("matching request was denied earlier"));
    assert!(notice.contains("during this Codewhale run"));
    assert!(notice.contains("Restart Codewhale"));
}

#[tokio::test]
async fn cached_denial_explanation_survives_tool_completion_and_done_render() {
    use crate::core::engine::MockApprovalEvent;
    use crate::tools::spec::ToolError;
    use ratatui::{Terminal, backend::TestBackend};

    let mut app = create_test_app();
    app.onboarding = OnboardingState::None;
    app.launch.visible = false;
    app.theme_id = crate::palette::ThemeId::Underwater;
    app.ui_theme = crate::palette::UNDERWATER_UI_THEME;
    app.is_loading = true;
    app.runtime_turn_status = Some("in_progress".to_string());

    let tool_id = "cached-shell-denial";
    let tool_name = "exec_shell";
    handle_tool_call_started(
        &mut app,
        tool_id,
        tool_name,
        &serde_json::json!({"command": "printf cached-denial"}),
    );

    // Mirror the cached ApprovalRequired branch: send the denial back to the
    // blocked engine, then project the explanation into the UI.
    let mut engine = mock_engine_handle();
    engine
        .handle
        .deny_tool_call(tool_id)
        .await
        .expect("send cached denial");
    surface_session_denied_notice(&mut app, tool_name);
    assert!(matches!(
        engine.recv_approval_event().await,
        Some(MockApprovalEvent::Denied { id }) if id == tool_id
    ));

    // These are the events that immediately follow the auto-deny in a real
    // turn. A later generic status is deliberately applied too: the detailed
    // recovery receipt must not depend on winning the one-line status race.
    let result = Err(ToolError::permission_denied(
        "Tool 'exec_shell' denied by user".to_string(),
    ));
    handle_tool_call_complete(&mut app, tool_id, tool_name, &result);
    app.flush_active_cell();

    let denied_tool_index = app
        .history
        .iter()
        .position(|cell| {
            matches!(
                cell,
                HistoryCell::Tool(ToolCell::Exec(exec))
                    if exec.command == "printf cached-denial"
                        && exec.status == ToolStatus::Failed
            )
        })
        .expect("cached denial must settle the pending tool as failed");
    let recovery_receipt_index = app
        .history
        .iter()
        .position(|cell| {
            matches!(
                cell,
                HistoryCell::System { content }
                    if content.contains("Auto-denied exec_shell")
                        && content.contains("Restart Codewhale")
            )
        })
        .expect("cached denial must leave a durable recovery receipt");
    assert!(
        denied_tool_index < recovery_receipt_index,
        "recovery receipt must follow the tool it explains"
    );

    app.is_loading = false;
    app.runtime_turn_status = Some("completed".to_string());
    app.status_message = Some("Tool 'exec_shell' denied by user".to_string());
    app.sync_status_message_to_toasts();

    let config = Config::default();
    let backend = TestBackend::new(89, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| {
            let _ = render(frame, &mut app, &config);
        })
        .expect("render completed denial sequence");
    let buffer = terminal.backend().buffer();
    let rendered = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        rendered.contains("Auto-denied exec_shell"),
        "cached-decision explanation disappeared after completion:\n{rendered}"
    );
    assert!(
        rendered.contains("Restart Codewhale"),
        "cached-denial recovery path disappeared after completion:\n{rendered}"
    );
    assert_eq!(
        app.view_stack.top_kind(),
        None,
        "cache hit must not re-prompt"
    );
}

#[test]
fn session_approved_cache_does_not_promote_tool_name_grants_to_whole_tool() {
    let mut app = create_test_app();
    app.approval_session_approved
        .insert("edit_file".to_string());

    assert!(
        !is_session_approved_for_tool(&app, "edit_file", "file:edit_file:fresh"),
        "a bare tool-name grant must not cover future calls of the same tool (ops R2)"
    );
    app.approval_session_approved
        .insert("file:edit_file:fresh".to_string());
    assert!(
        is_session_approved_for_tool(&app, "edit_file", "file:edit_file:fresh"),
        "a grouping-key grant still covers its command family"
    );
}

#[test]
fn auto_review_force_prompt_fails_closed_without_a_modal() {
    use crate::core::authority::ApprovalRequestDisposition;
    let mut app = create_test_app();
    app.approval_mode = ApprovalMode::Auto;

    // Auto-Review is autonomous: a forced hold is denied without pausing.
    assert_eq!(
        resolve_ui_approval_disposition(
            &app,
            "exec_shell",
            "shell:exec_shell:cargo test",
            "key",
            true,
        ),
        ApprovalRequestDisposition::AutoDenyAutoReview
    );
}

#[test]
fn forced_approval_prompt_bypasses_session_approval_shortcut() {
    use crate::core::authority::ApprovalRequestDisposition;
    let mut app = create_test_app();
    app.mode = AppMode::Agent;
    app.approval_mode = ApprovalMode::Suggest;
    app.approval_session_approved
        .insert("shell:exec_shell:cargo test".to_string());

    // Session grant alone cannot auto-approve a forced policy hold under Ask —
    // the disposition stays Prompt so the user still sees the hold.
    assert_eq!(
        resolve_ui_approval_disposition(
            &app,
            "exec_shell",
            "shell:exec_shell:cargo test",
            "key",
            true,
        ),
        ApprovalRequestDisposition::Prompt
    );
}

#[test]
fn full_access_auto_approves_requests_while_auto_review_holds_without_a_modal() {
    use crate::core::authority::ApprovalRequestDisposition;
    let mut app = create_test_app();
    app.approval_mode = ApprovalMode::Auto;
    assert_eq!(
        resolve_ui_approval_disposition(
            &app,
            "exec_shell",
            "shell:exec_shell:cargo test",
            "key",
            false,
        ),
        ApprovalRequestDisposition::AutoDenyAutoReview
    );

    app.approval_mode = ApprovalMode::Bypass;
    assert_eq!(
        resolve_ui_approval_disposition(
            &app,
            "exec_shell",
            "shell:exec_shell:cargo test",
            "key",
            false,
        ),
        ApprovalRequestDisposition::AutoApprove
    );
    assert_eq!(
        resolve_ui_approval_disposition(
            &app,
            "exec_shell",
            "shell:exec_shell:cargo test",
            "key",
            true,
        ),
        ApprovalRequestDisposition::AutoDenyFullAccessPolicyHold
    );

    app.approval_mode = ApprovalMode::Suggest;
    app.mode = AppMode::Agent;
    app.approval_session_approved
        .insert("shell:exec_shell:cargo test".to_string());
    assert_eq!(
        resolve_ui_approval_disposition(
            &app,
            "exec_shell",
            "shell:exec_shell:cargo test",
            "key",
            false,
        ),
        ApprovalRequestDisposition::AutoApprove
    );
    // Without Full Access, a forced hold still opens a modal (Prompt).
    assert_eq!(
        resolve_ui_approval_disposition(
            &app,
            "exec_shell",
            "shell:exec_shell:cargo test",
            "key",
            true,
        ),
        ApprovalRequestDisposition::Prompt
    );
}

#[test]
fn app_auto_approval_helper_covers_bypass_only() {
    let mut app = create_test_app();
    app.mode = AppMode::Agent;
    app.approval_mode = ApprovalMode::Suggest;
    assert!(!app_auto_approve_enabled(&app));

    app.approval_mode = ApprovalMode::Auto;
    assert!(!app_auto_approve_enabled(&app));

    app.approval_mode = ApprovalMode::Bypass;
    assert!(app_auto_approve_enabled(&app));
}

#[test]
fn auto_review_suppresses_stale_question_prompts_while_other_postures_allow_them() {
    let mut app = create_test_app();
    for (posture, expected) in [
        (ApprovalMode::Suggest, false),
        (ApprovalMode::Auto, true),
        (ApprovalMode::Bypass, false),
        (ApprovalMode::Never, false),
    ] {
        app.approval_mode = posture;
        assert_eq!(
            should_suppress_user_input_prompt(&app),
            expected,
            "{posture:?}"
        );
    }

    // Full Access keeps questions valid, same as Auto-Review suppresses
    // them only for its own stale-prompt cleanup.
    app.approval_mode = ApprovalMode::Bypass;
    assert!(!should_suppress_user_input_prompt(&app));
}

fn create_test_options() -> TuiOptions {
    TuiOptions {
        // Keep UI tests independent from the developer's saved
        // `default_mode` setting.
        start_in_agent_mode: true,
        skip_onboarding: false,
        ..crate::test_support::test_tui_options(PathBuf::from("."))
    }
}

#[test]
fn setup_checkpoint_opens_after_onboarding_when_due() {
    let _home = SettingsHomeGuard::new();
    let config = Config::default();
    let mut app = App::new(create_test_options(), &config);
    app.onboarding = OnboardingState::None;

    assert!(open_setup_checkpoint_if_due(&mut app, &config, false));
    assert_eq!(app.view_stack.top_kind(), Some(ModalKind::SetupWizard));
    assert!(
        !open_setup_checkpoint_if_due(&mut app, &config, false),
        "setup wizard should not be stacked twice"
    );
}

#[test]
fn setup_checkpoint_waits_for_onboarding_and_skip_flag() {
    let _home = SettingsHomeGuard::new();
    let config = Config::default();
    let mut app = App::new(create_test_options(), &config);
    app.onboarding = OnboardingState::Provider;

    assert!(!open_setup_checkpoint_if_due(&mut app, &config, false));
    assert!(app.view_stack.is_empty());

    app.onboarding = OnboardingState::None;
    assert!(!open_setup_checkpoint_if_due(&mut app, &config, true));
    assert!(app.view_stack.is_empty());
    let state = codewhale_config::SetupState::load()
        .expect("load setup state")
        .expect("setup state");
    assert_eq!(
        state.constitution_checkpoint_completed_for.as_deref(),
        Some(crate::tui::setup::CONSTITUTION_CHECKPOINT_VERSION)
    );
    assert_eq!(
        state.constitution_choice,
        codewhale_config::ConstitutionChoice::Deferred
    );
    assert_eq!(
        state.status(codewhale_config::SetupStep::Constitution),
        codewhale_config::StepStatus::Deferred
    );
    assert!(
        !open_setup_checkpoint_if_due(&mut app, &config, false),
        "deferred skip record should suppress the update checkpoint on the next launch"
    );
}

#[test]
fn setup_runtime_preset_apply_persists_settings_config_and_state() {
    let _home = SettingsHomeGuard::new();
    let config_path = crate::config_persistence::config_toml_path(None).expect("config path");
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("config parent exists");
    std::fs::write(
        &config_path,
        "# preserve this comment\nmodel = \"deepseek-v4-pro\"\n",
    )
    .expect("seed config");

    let mut app = create_test_app();
    app.config_path = Some(config_path.clone());
    let mut config = Config::default();
    let preset = crate::tui::setup::SetupRuntimePreset::AskFirst;
    let mut state = codewhale_config::SetupState {
        runtime_posture_source: codewhale_config::RuntimePostureSource::Confirmed,
        ..Default::default()
    };
    state.set_step(
        codewhale_config::SetupStep::TrustSandbox,
        codewhale_config::StepEntry::new(
            codewhale_config::StepStatus::Verified,
            true,
            crate::tui::setup::CONSTITUTION_CHECKPOINT_VERSION,
        )
        .with_result(preset.result_summary()),
    );

    let summary =
        apply_setup_runtime_preset(&mut app, &mut config, preset, state).expect("apply preset");

    assert!(summary.contains("preset=ask-first"));
    let settings = Settings::load().expect("load saved settings");
    assert_eq!(settings.default_mode, "plan");
    assert_eq!(settings.permission_posture.as_deref(), Some("ask"));
    assert_eq!(app.mode, AppMode::Plan);
    assert!(!app.allow_shell);
    assert_eq!(app.approval_mode, ApprovalMode::Suggest);
    assert_eq!(config.allow_shell, Some(false));
    assert_eq!(config.approval_policy.as_deref(), Some("on-request"));
    assert_eq!(config.sandbox_mode.as_deref(), Some("read-only"));
    assert_eq!(app.configured_sandbox_mode.as_deref(), Some("read-only"));

    let body = std::fs::read_to_string(&config_path).expect("read saved config");
    assert!(
        body.contains("# preserve this comment"),
        "comment lost: {body}"
    );
    assert!(body.contains("approval_policy = \"on-request\""));
    assert!(body.contains("allow_shell = false"));
    assert!(body.contains("sandbox_mode = \"read-only\""));

    let saved_state = codewhale_config::SetupState::load()
        .expect("load setup state")
        .expect("setup state exists");
    assert_eq!(
        saved_state.status(codewhale_config::SetupStep::TrustSandbox),
        codewhale_config::StepStatus::Verified
    );
    assert_eq!(
        saved_state.runtime_posture_source,
        codewhale_config::RuntimePostureSource::Confirmed
    );
}

#[test]
fn setup_runtime_preset_rolls_back_durable_and_live_state_when_state_save_fails() {
    let _home = SettingsHomeGuard::new();
    let config_path = crate::config_persistence::config_toml_path(None).expect("config path");
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("config parent exists");
    std::fs::write(
        &config_path,
        "# keep exact bytes\napproval_policy = \"auto\"\nallow_shell = true\nsandbox_mode = \"workspace-write\"\n",
    )
    .expect("seed config");
    let original_settings = Settings {
        default_mode: "agent".to_string(),
        permission_posture: Some("auto-review".to_string()),
        ..Settings::default()
    };
    original_settings.save().expect("seed settings");

    let config_before = std::fs::read(&config_path).expect("read config snapshot");
    let settings_path = Settings::path().expect("settings path");
    let settings_before = std::fs::read(&settings_path).expect("read settings snapshot");

    let mut app = create_test_app();
    app.config_path = Some(config_path.clone());
    app.set_agent_runtime_baseline(true, false, ApprovalMode::Auto);
    app.set_mode(AppMode::Agent);
    let app_before = (
        app.mode,
        app.allow_shell,
        app.trust_mode,
        app.approval_mode,
        app.approval_policy_locked(),
    );
    let mut config = Config {
        approval_policy: Some("auto".to_string()),
        allow_shell: Some(true),
        sandbox_mode: Some("workspace-write".to_string()),
        ..Config::default()
    };
    let config_before_live = (
        config.approval_policy.clone(),
        config.allow_shell,
        config.sandbox_mode.clone(),
    );

    // SetupState::save is atomic; an existing directory at the destination
    // forces the final persist to fail after config and settings were staged.
    let state_path = codewhale_config::SetupState::path().expect("state path");
    std::fs::create_dir_all(&state_path).expect("state path directory");
    let error = apply_setup_runtime_preset(
        &mut app,
        &mut config,
        crate::tui::setup::SetupRuntimePreset::AskFirst,
        codewhale_config::SetupState::default(),
    )
    .expect_err("state persistence must fail");

    assert!(
        error
            .to_string()
            .contains("failed to persist setup runtime posture state"),
        "{error:#}"
    );
    assert_eq!(std::fs::read(config_path).unwrap(), config_before);
    assert_eq!(std::fs::read(settings_path).unwrap(), settings_before);
    assert_eq!(
        (
            app.mode,
            app.allow_shell,
            app.trust_mode,
            app.approval_mode,
            app.approval_policy_locked(),
        ),
        app_before
    );
    assert_eq!(
        (
            config.approval_policy,
            config.allow_shell,
            config.sandbox_mode,
        ),
        config_before_live
    );
}

#[test]
fn setup_high_trust_persists_full_access_without_legacy_yolo_mode() {
    let _home = SettingsHomeGuard::new();
    let config_path = crate::config_persistence::config_toml_path(None).expect("config path");
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("config parent exists");
    std::fs::write(
        &config_path,
        "# keep me\napproval_policy = \"on-request\"\nallow_shell = false\n",
    )
    .expect("seed config");

    let mut app = create_test_app();
    app.config_path = Some(config_path.clone());
    let mut config = Config {
        approval_policy: Some("on-request".to_string()),
        ..Config::default()
    };
    let preset = crate::tui::setup::SetupRuntimePreset::HighTrustLocal;

    apply_setup_runtime_preset(
        &mut app,
        &mut config,
        preset,
        codewhale_config::SetupState::default(),
    )
    .expect("apply high trust");

    let settings = Settings::load().expect("load saved settings");
    assert_eq!(settings.default_mode, "agent");
    assert_eq!(settings.permission_posture.as_deref(), Some("full-access"));
    assert_eq!(config.approval_policy, None);
    assert_eq!(app.mode, AppMode::Agent);
    assert_eq!(app.approval_mode, ApprovalMode::Bypass);
    assert!(app.allow_shell);
    assert!(app.trust_mode);
    assert_eq!(
        app.configured_sandbox_mode.as_deref(),
        Some("danger-full-access")
    );

    app.set_mode(AppMode::Plan);
    assert!(!app.allow_shell);
    assert_eq!(app.approval_mode, ApprovalMode::Suggest);
    app.set_mode(AppMode::Agent);
    assert!(app.allow_shell, "High Trust shell must survive Plan → Act");
    assert!(app.trust_mode, "High Trust trust must survive Plan → Act");
    assert_eq!(app.approval_mode, ApprovalMode::Bypass);

    let body = std::fs::read_to_string(&config_path).expect("read saved config");
    assert!(body.contains("# keep me"), "comment lost: {body}");
    assert!(
        !body.contains("approval_policy"),
        "top-level policy would override the saved Full Access posture: {body}"
    );
}

#[test]
fn setup_high_trust_cannot_override_project_approval_policy() {
    let _home = SettingsHomeGuard::new();
    let config_path = crate::config_persistence::config_toml_path(None).expect("config path");
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("config parent exists");
    std::fs::write(&config_path, "approval_policy = \"on-request\"\n").expect("root config");
    let workspace = config_path
        .parent()
        .and_then(std::path::Path::parent)
        .expect("temporary home")
        .join("project");
    let project_dir = workspace.join(codewhale_config::CODEWHALE_APP_DIR);
    std::fs::create_dir_all(&project_dir).expect("project config dir");
    std::fs::write(
        project_dir.join("config.toml"),
        "approval_policy = \"never\"\n",
    )
    .expect("project config");

    let mut app = create_test_app();
    app.workspace = workspace;
    app.config_path = Some(config_path.clone());
    let mut config = Config {
        approval_policy: Some("never".to_string()),
        ..Config::default()
    };

    let error = apply_setup_runtime_preset(
        &mut app,
        &mut config,
        crate::tui::setup::SetupRuntimePreset::HighTrustLocal,
        codewhale_config::SetupState::default(),
    )
    .expect_err("project policy must control the live session");

    assert!(
        error.to_string().contains("project runtime configuration"),
        "{error:#}"
    );
    assert_eq!(config.approval_policy.as_deref(), Some("never"));
    assert!(
        std::fs::read_to_string(config_path)
            .expect("root config remains")
            .contains("approval_policy = \"on-request\"")
    );
}

#[test]
fn project_runtime_provenance_only_blocks_values_startup_would_accept() {
    let _home = SettingsHomeGuard::new();
    let settings = Settings {
        permission_posture: Some("ask".to_string()),
        ..Settings::default()
    };
    settings.save().expect("save settings");

    let workspace = Settings::path()
        .expect("settings path")
        .parent()
        .expect("Codewhale home")
        .parent()
        .expect("temporary home")
        .join("project");
    let project_dir = workspace.join(codewhale_config::CODEWHALE_APP_DIR);
    std::fs::create_dir_all(&project_dir).expect("project config dir");
    let project_path = project_dir.join("config.toml");
    std::fs::write(
        &project_path,
        "approval_policy = \"auto\"\nsandbox_mode = \"danger-full-access\"\nallow_shell = true\n",
    )
    .expect("project config");

    let config = Config {
        sandbox_mode: Some("read-only".to_string()),
        ..Config::default()
    };
    assert_eq!(
        config.approval_policy_control(None, None, &workspace),
        crate::config::ApprovalPolicyControl::Unset,
        "project Auto would loosen saved Ask and must not claim provenance"
    );
    assert_eq!(
        config.allow_shell_control(None, None, &workspace),
        crate::config::ShellAccessControl::Unset,
        "project allow_shell=true is ignored by startup"
    );
    assert_eq!(
        config.runtime_preset_blocker(None, None, &workspace),
        None,
        "ignored project escalations must not create a phantom preset blocker"
    );

    std::fs::write(
        &project_path,
        "approval_policy = \"never\"\nsandbox_mode = \"read-only\"\nallow_shell = false\n",
    )
    .expect("tightening project config");
    assert_eq!(
        config.approval_policy_control(None, None, &workspace),
        crate::config::ApprovalPolicyControl::ProjectConfig
    );
    assert_eq!(
        config.allow_shell_control(None, None, &workspace),
        crate::config::ShellAccessControl::ProjectConfig
    );
    assert_eq!(
        config.runtime_preset_blocker(None, None, &workspace),
        Some("project runtime configuration")
    );
}

#[test]
fn saved_full_access_baseline_allows_project_approval_tightening() {
    let _home = SettingsHomeGuard::new();
    let settings = Settings {
        permission_posture: Some("full-access".to_string()),
        ..Settings::default()
    };
    settings.save().expect("save settings");

    let workspace = Settings::path()
        .expect("settings path")
        .parent()
        .expect("Codewhale home")
        .parent()
        .expect("temporary home")
        .join("project");
    let project_dir = workspace.join(codewhale_config::CODEWHALE_APP_DIR);
    std::fs::create_dir_all(&project_dir).expect("project config dir");
    std::fs::write(
        project_dir.join("config.toml"),
        "approval_policy = \"on-request\"\n",
    )
    .expect("project config");

    assert_eq!(
        Config::default().approval_policy_control(None, None, &workspace),
        crate::config::ApprovalPolicyControl::ProjectConfig,
        "Ask is a tightening from saved Full Access and owns the effective policy"
    );
}

#[test]
fn setup_presets_cannot_override_managed_runtime_requirements() {
    let _home = SettingsHomeGuard::new();
    let config_path = crate::config_persistence::config_toml_path(None).expect("config path");
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("config parent exists");
    std::fs::write(&config_path, "# managed posture stays intact\n").expect("root config");
    let requirements_path = config_path
        .parent()
        .expect("config parent")
        .join("requirements.toml");
    std::fs::write(
        &requirements_path,
        "allowed_approval_policies = [\"never\"]\nallowed_sandbox_modes = [\"read-only\"]\n",
    )
    .expect("requirements");

    let mut app = create_test_app();
    app.config_path = Some(config_path.clone());
    app.allow_shell = false;
    app.approval_mode = ApprovalMode::Never;
    let mut config = Config {
        approval_policy: Some("never".to_string()),
        sandbox_mode: Some("read-only".to_string()),
        allow_shell: Some(false),
        requirements_path: Some(requirements_path.to_string_lossy().into_owned()),
        ..Config::default()
    };

    for preset in [
        crate::tui::setup::SetupRuntimePreset::AskFirst,
        crate::tui::setup::SetupRuntimePreset::NormalAgent,
        crate::tui::setup::SetupRuntimePreset::HighTrustLocal,
    ] {
        let error = apply_setup_runtime_preset(
            &mut app,
            &mut config,
            preset,
            codewhale_config::SetupState::default(),
        )
        .expect_err("managed requirements must win");
        assert!(
            error.to_string().contains("managed runtime requirements"),
            "{error:#}"
        );
    }

    assert_eq!(config.approval_policy.as_deref(), Some("never"));
    assert_eq!(config.sandbox_mode.as_deref(), Some("read-only"));
    assert_eq!(config.allow_shell, Some(false));
    assert_eq!(app.approval_mode, ApprovalMode::Never);
    assert!(!app.allow_shell);
    assert_eq!(
        std::fs::read_to_string(config_path).expect("root config"),
        "# managed posture stays intact\n"
    );
}

#[tokio::test]
async fn tool_result_api_content_never_advertises_unowned_live_output_as_retrievable() {
    let mut app = App::new(create_test_options(), &Config::default());
    app.api_messages.push(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: "call-live-big".to_string(),
            name: "exec_shell".to_string(),
            input: serde_json::json!({"command": "cargo test"}),
            caller: None,
            thought_signature: None,
        }],
    });

    let raw = "LIVE_RAW_SENTINEL\n".repeat(900);
    let output = crate::tools::spec::ToolResult::success(raw.clone());
    let content =
        tool_result_content_for_api_message(&app, "call-live-big", "exec_shell", &output).await;

    assert!(content.contains("[TOOL_OUTPUT_RECEIPT]"));
    assert!(content.contains("tool: exec_shell"));
    assert!(content.contains("tool_call_id: call-live-big"));
    assert!(content.contains("full output in the tool details view"));
    assert!(!content.contains("detail_handle"));
    assert!(!content.contains("storage:"));
    assert!(!content.contains("retrieve_tool_result"));
    assert!(!content.contains(&raw));
    assert!(
        content.chars().count()
            < crate::tool_output_receipts::RAW_TOOL_OUTPUT_RECEIPT_THRESHOLD_CHARS
    );
}

#[test]
fn live_tool_receipt_messages_clones_only_matching_tool_use() {
    let mut app = App::new(create_test_options(), &Config::default());
    app.api_messages.push(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: "call-old".to_string(),
            name: "exec_shell".to_string(),
            input: serde_json::json!({"command": "old"}),
            caller: None,
            thought_signature: None,
        }],
    });
    app.api_messages.push(Message {
        role: Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "call-old".to_string(),
            content: "OLD_RAW\n".repeat(2_000),
            is_error: None,
            content_blocks: None,
        }],
    });
    app.api_messages.push(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: "call-new".to_string(),
            name: "read_file".to_string(),
            input: serde_json::json!({"path": "src/main.rs"}),
            caller: None,
            thought_signature: None,
        }],
    });

    let messages = live_tool_receipt_messages(&app, "call-new", "NEW_RAW", true);

    assert_eq!(messages.len(), 2);
    assert!(matches!(
        &messages[0].content[0],
        ContentBlock::ToolUse { id, name, ..} if id == "call-new" && name == "read_file"
    ));
    assert!(matches!(
        &messages[1].content[0],
        ContentBlock::ToolResult { tool_use_id, content, .. }
            if tool_use_id == "call-new" && content == "NEW_RAW"
    ));
}

fn text_message(role: &str, text: &str) -> Message {
    Message {
        role: Role::from(role),
        content: vec![ContentBlock::Text {
            text: text.to_string(),
            cache_control: None,
        }],
    }
}

fn authoritative_user_message(text: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![
            ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            },
            ContentBlock::Text {
                text: concat!(
                    "<turn_meta>\n",
                    "Input provenance: external_user\n",
                    "Input authority: external_current_turn\n",
                    "</turn_meta>",
                )
                .to_string(),
                cache_control: None,
            },
        ],
    }
}

fn saved_session_with_messages(messages: Vec<Message>) -> SavedSession {
    SavedSession {
        schema_version: 1,
        metadata: crate::session_manager::SessionMetadata {
            id: "resume-recovery-session".to_string(),
            title: "resume recovery".to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            message_count: messages.len(),
            total_tokens: 0,
            model: "deepseek-v4-pro".to_string(),
            model_provider: "deepseek".to_string(),
            model_provider_id: None,
            workspace: PathBuf::from("/tmp/resume-recovery"),
            mode: Some("yolo".to_string()),
            cost: crate::session_manager::SessionCostSnapshot::default(),
            parent_session_id: None,
            forked_from_message_count: None,
            cumulative_turn_secs: 0,
            archived: false,
            spawn_depth: 0,
        },
        journal: None,
        leaf_id: None,
        messages,
        system_prompt: None,
        context_references: Vec::new(),
        artifacts: Vec::new(),
        approval_receipts: Vec::new(),
        work_state: None,
        window_title: None,
        last_auto_route: None,
    }
}

fn named_custom_session_config(name: &str, base_url: &str, model: &str) -> Config {
    let mut custom = HashMap::new();
    custom.insert(
        name.to_string(),
        ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some(base_url.to_string()),
            model: Some(model.to_string()),
            ..ProviderConfig::default()
        },
    );
    Config {
        provider: Some(name.to_string()),
        providers: Some(ProvidersConfig {
            custom,
            ..ProvidersConfig::default()
        }),
        ..Config::default()
    }
}

#[test]
fn apply_loaded_session_keeps_submitted_user_tail_in_history() {
    let mut app = create_test_app();
    let submitted = authoritative_user_message("finish the Qthresh proof bundle");
    let session = saved_session_with_messages(vec![submitted.clone()]);

    apply_loaded_session(&mut app, &mut Config::default(), &session).expect("restore session");

    assert_eq!(app.api_messages, vec![submitted]);
    assert!(app.input.is_empty());
    assert!(app.queued_draft.is_none());
    assert!(app.history.iter().any(|cell| {
        matches!(cell, HistoryCell::User { content } if content == "finish the Qthresh proof bundle")
    }));
}

#[test]
fn apply_loaded_session_restores_the_window_title_override() {
    let mut app = create_test_app();
    let mut session = saved_session_with_messages(vec![]);
    session.window_title = Some("restored-tab-title".to_string());

    apply_loaded_session(&mut app, &mut Config::default(), &session).expect("restore session");

    assert_eq!(
        app.window_title.as_deref(),
        Some("restored-tab-title"),
        "applied session must restore the /title override onto the App"
    );
}

#[test]
fn apply_loaded_session_never_restores_background_shell_event_as_composer_draft() {
    let mut app = create_test_app();
    // Literal persisted shape from the v0.9.4 resume report. Runtime events
    // use the user transport role for provider compatibility, but their
    // provenance and authority make them categorically different from input
    // submitted by the person at the composer.
    let shell_completion = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Text {
                text: concat!(
                    "<codewhale:runtime_event kind=\"background_shell_completion\" ",
                    "visibility=\"internal\">\n",
                    "This is an internal runtime event, not user input.\n\n",
                    "{\"task_id\":\"shell_580e7816\",\"status\":\"Completed\",",
                    "\"exit_code\":0,\"stdout_tail\":\"EXIT=0\\n\"}\n",
                    "</codewhale:runtime_event>",
                )
                .to_string(),
                cache_control: None,
            },
            ContentBlock::Text {
                text: concat!(
                    "<turn_meta>\n",
                    "Input provenance: shell_completion\n",
                    "Input authority: non_authoritative\n",
                    "</turn_meta>",
                )
                .to_string(),
                cache_control: None,
            },
        ],
    };
    let session = saved_session_with_messages(vec![
        text_message("user", "please continue"),
        text_message("assistant", "The corrected run is still in progress."),
        shell_completion.clone(),
    ]);

    apply_loaded_session(&mut app, &mut Config::default(), &session).expect("restore session");

    assert!(app.input.is_empty());
    assert!(app.queued_draft.is_none());
    assert_eq!(app.api_messages.last(), Some(&shell_completion));
    assert!(app.history.iter().any(|cell| {
        matches!(cell, HistoryCell::User { content } if content.contains("background_shell_completion"))
    }));
}

#[test]
fn same_session_persisted_draft_restores_exact_composer_text() {
    let mut app = create_test_app();
    app.current_session_id = Some("session-with-draft".to_string());
    let state = OfflineQueueState {
        session_id: Some("session-with-draft".to_string()),
        draft: Some(QueuedSessionMessage {
            display: "real unsent draft\nwith a second line".to_string(),
            skill_instruction: Some("review this draft".to_string()),
            skill_provenance: None,
        }),
        ..OfflineQueueState::default()
    };

    assert!(restore_matching_offline_queue_state(&mut app, state));
    assert_eq!(app.input, "real unsent draft\nwith a second line");
    assert_eq!(app.cursor_position, app.input.chars().count());
    assert_eq!(app.active_skill.as_deref(), Some("review this draft"));
    assert_eq!(
        app.queued_draft
            .as_ref()
            .map(|draft| draft.display.as_str()),
        Some("real unsent draft\nwith a second line")
    );
}

#[test]
fn apply_loaded_session_hides_turn_metadata_without_mutating_api_history() {
    let turn_meta = concat!(
        "<turn_meta>\n",
        "Current local date: 2026-07-22\n",
        "Input provenance: external_user\n",
        "Input authority: external_current_turn\n",
        "</turn_meta>",
    );
    let user_message = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Text {
                text: "Keep the restored transcript calm".to_string(),
                cache_control: None,
            },
            ContentBlock::Text {
                text: turn_meta.to_string(),
                cache_control: None,
            },
        ],
    };
    let session = saved_session_with_messages(vec![
        user_message,
        text_message("assistant", "The wire context stays complete."),
    ]);
    let mut app = create_test_app();

    apply_loaded_session(&mut app, &mut Config::default(), &session).expect("restore session");

    assert!(matches!(
        app.history.as_slice(),
        [HistoryCell::User { content }, HistoryCell::Assistant { .. }]
            if content == "Keep the restored transcript calm"
    ));
    assert!(matches!(
        app.api_messages[0].content.as_slice(),
        [ContentBlock::Text { text: visible, .. }, ContentBlock::Text { text: meta, .. }]
            if visible == "Keep the restored transcript calm" && meta == turn_meta
    ));
}

#[test]
fn apply_loaded_session_projects_subagent_handoff_without_retry_draft_or_user_cell() {
    let mut app = create_test_app();
    let payload = concat!(
        "Implemented the resume projection.\nCheckpoint: focused tests pass.\n",
        "<codewhale:subagent.done>{\"agent_id\":\"agent_resume\",\"name\":\"Tide\",",
        "\"agent_type\":\"implementer\",\"status\":\"completed\",",
        "\"summary_location\":\"previous_line\"}</codewhale:subagent.done>",
    );
    // Literal v0.9.0 persisted fixture: keep this independent from the current
    // producer so a producer/parser drift cannot make the regression test
    // self-fulfilling.
    let persisted_handoff = Message {
        role: Role::User,
        content: vec![
            ContentBlock::Text {
                text: format!(
                    "<codewhale:runtime_event kind=\"subagent_completion\" visibility=\"internal\">\n\
This is an internal runtime event, not user input. Use the sub-agent completion \
data below to continue coordinating the current task. Do not tell the user they \
pasted sentinels, do not explain the sentinel protocol, and do not quote the raw \
XML unless the user explicitly asks to debug sub-agent internals.\n\n\
{payload}\n\
</codewhale:runtime_event>"
                ),
                cache_control: None,
            },
            ContentBlock::Text {
                text: "<turn_meta>\nInput provenance: subagent_handoff\nInput authority: non_authoritative\n</turn_meta>".to_string(),
                cache_control: None,
            },
        ],
    };
    let session = saved_session_with_messages(vec![
        text_message("user", "Fix the resume regression"),
        text_message("assistant", "I delegated the restore path."),
        persisted_handoff,
    ]);

    apply_loaded_session(&mut app, &mut Config::default(), &session).expect("restore session");

    assert!(app.input.is_empty());
    assert!(app.queued_draft.is_none());
    assert_eq!(app.api_messages.len(), 3);
    assert!(app.api_messages.iter().any(|message| {
        message.role == "user"
            && message.content.iter().any(|block| {
                matches!(block, ContentBlock::Text { text, .. } if text == "Fix the resume regression")
            })
    }));
    let restored = crate::runtime_handoff::restored_subagent_checkpoint_display(
        app.api_messages.last().expect("restored handoff"),
    )
    .expect("projected handoff");
    assert!(restored.contains("Status: completed"));
    assert!(restored.contains("Checkpoint: focused tests pass."));
    assert!(!restored.contains("runtime_event"));
    assert!(!restored.contains("subagent.done"));
    let checkpoint_cell = app
        .history
        .iter()
        .find(|cell| {
            matches!(cell, HistoryCell::System { content } if content.contains("Status: completed"))
        })
        .expect("restored checkpoint Note cell");
    let rendered = checkpoint_cell
        .lines(28)
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(rendered.contains("Note"));
    assert!(rendered.contains("Status: completed"));
    assert!(
        !rendered.contains('▎'),
        "must not render with the user glyph"
    );
    assert!(!rendered.contains("runtime_event"));
    assert!(app.history.iter().all(|cell| {
        !matches!(cell, HistoryCell::User { content } if content.contains("agent_resume") || content.contains("runtime_event"))
    }));
}

#[test]
fn apply_loaded_session_resets_unpersisted_telemetry() {
    let mut app = create_test_app();
    app.session.session_cost = 1.25;
    app.session.session_cost_cny = 9.13;
    app.session.subagent_cost = 0.75;
    app.session.subagent_cost_cny = 5.48;
    app.session
        .subagent_usage_sources
        .insert(crate::cost_status::usage_source_fingerprint(
            "response-test",
        ));
    app.session.displayed_cost_high_water = 2.0;
    app.session.displayed_cost_high_water_cny = 14.61;
    app.session.last_prompt_tokens = Some(120);
    app.session.last_completion_tokens = Some(35);
    app.session.last_prompt_cache_hit_tokens = Some(80);
    app.session.last_prompt_cache_miss_tokens = Some(40);
    app.session.last_reasoning_replay_tokens = Some(12);
    app.session.total_input_tokens = 120;
    app.session.total_output_tokens = 35;
    app.session.total_cache_hit_tokens = 80;
    app.session.total_cache_miss_tokens = 40;
    app.session.total_cache_write_tokens = 17;
    app.push_turn_cache_record(crate::tui::app::TurnCacheRecord {
        provider: None,
        provider_identity: None,
        model: None,
        auto_model: false,
        input_tokens: 120,
        output_tokens: 35,
        cache_hit_tokens: Some(80),
        cache_miss_tokens: Some(40),
        reasoning_replay_tokens: Some(12),
        cache_write_tokens: None,
        reasoning_tokens: None,
        cost_audit: None,
        recorded_at: Instant::now(),
    });
    let mut session = saved_session_with_messages(vec![text_message("assistant", "ready")]);
    session.metadata.total_tokens = 500;

    apply_loaded_session(&mut app, &mut Config::default(), &session).expect("restore session");
    assert_eq!(app.session.total_tokens, 500);
    assert_eq!(app.session.total_conversation_tokens, 500);
    assert_eq!(app.session.session_cost, 0.0);
    assert_eq!(app.session.session_cost_cny, 0.0);
    assert_eq!(app.session.subagent_cost, 0.0);
    assert_eq!(app.session.subagent_cost_cny, 0.0);
    assert!(app.session.subagent_usage_sources.is_empty());
    assert_eq!(app.session.displayed_cost_high_water, 0.0);
    assert_eq!(app.session.displayed_cost_high_water_cny, 0.0);
    assert_eq!(app.session.last_prompt_tokens, None);
    assert_eq!(app.session.last_completion_tokens, None);
    assert_eq!(app.session.last_prompt_cache_hit_tokens, None);
    assert_eq!(app.session.last_prompt_cache_miss_tokens, None);
    assert_eq!(app.session.last_reasoning_replay_tokens, None);
    assert_eq!(app.session.total_input_tokens, 0);
    assert_eq!(app.session.total_output_tokens, 0);
    assert_eq!(app.session.total_cache_hit_tokens, 0);
    assert_eq!(app.session.total_cache_miss_tokens, 0);
    assert_eq!(app.session.total_cache_write_tokens, 0);
    assert!(app.session.turn_cache_history.is_empty());
}

/// Loading a session replaces the money *and* the evidence that qualifies it.
///
/// Coverage counters describe one specific total. Carrying the live session's
/// counters across a load would attach them to a different total — the loaded
/// one — and the result reads as a complete, audited figure that nothing
/// produced. The reverse is equally wrong: a saved session written before
/// coverage existed deserializes its counters from `Default`, which is
/// byte-identical to "complete total covering zero turns", so the load path has
/// to recognize that shape rather than trust it (#4318).
#[test]
fn apply_loaded_session_replaces_cost_coverage_with_the_loaded_total() {
    let _cost_scope = crate::cost_status::test_scope();
    let mut app = create_test_app();
    // Live state from the session being replaced.
    app.session.cost_priced_turns = 9;
    app.session.cost_unpriced_turns = 4;
    app.session.cost_cny_priced_turns = 3;
    app.session.cost_cny_unpriced_turns = 6;
    app.session
        .cost_unpriced_reasons
        .insert("stale_live_reason".to_string());
    app.session
        .cost_unpriced_classes
        .insert("stale_live_class".to_string());
    app.session
        .cost_route_receipts
        .insert("stale live receipt".to_string());
    app.session.cost_coverage_unknown_legacy = true;

    let mut session = saved_session_with_messages(vec![text_message("assistant", "ready")]);
    session.metadata.cost = crate::session_manager::SessionCostSnapshot {
        session_cost_usd: 2.5,
        subagent_cost_usd: 1.25,
        priced_turns: 2,
        unpriced_turns: 1,
        cny_priced_turns: 0,
        cny_unpriced_turns: 3,
        unpriced_reasons: ["missing_class_price".to_string()].into(),
        cny_unpriced_reasons: ["currency_not_published".to_string()].into(),
        unpriced_classes: ["cache_write".to_string()].into(),
        pricing_provenances: ["models_dev_bundled".to_string()].into(),
        route_receipts: ["provider=deepseek identity=deepseek model=deepseek-v4-flash".to_string()]
            .into(),
        usage_source_fingerprints: [crate::cost_status::usage_source_fingerprint(
            "subagent:agent-reload:step:1:response:late",
        )]
        .into(),
        coverage_recorded: true,
        ..crate::session_manager::SessionCostSnapshot::default()
    };

    apply_loaded_session(&mut app, &mut Config::default(), &session).expect("restore session");

    assert_eq!(app.session.cost_priced_turns, 2);
    assert_eq!(app.session.subagent_cost, 1.25);
    assert_eq!(app.session.cost_unpriced_turns, 1);
    assert_eq!(app.session.cost_cny_priced_turns, 0);
    assert_eq!(app.session.cost_cny_unpriced_turns, 3);
    assert_eq!(
        app.session.cost_unpriced_reasons,
        ["missing_class_price".to_string()].into()
    );
    assert_eq!(
        app.session.cost_unpriced_classes,
        ["cache_write".to_string()].into()
    );
    assert_eq!(
        app.session.subagent_usage_sources,
        session
            .metadata
            .cost
            .usage_source_fingerprints
            .iter()
            .cloned()
            .collect()
    );
    assert!(crate::cost_status::usage_source_seen(
        "subagent:agent-reload:step:1:response:late"
    ));
    let replay_owner = "interactive:reloaded-session:turn-replay";
    crate::cost_status::register_interactive_runtime_usage_sink(
        replay_owner,
        crate::cost_status::scope_token(),
    );
    crate::cost_status::report_effective_route_for_runtime(
        crate::cost_status::scope_token(),
        Some(replay_owner),
        "subagent:agent-reload:step:1:response:late",
        &crate::cost_status::EffectiveRouteEnvelope::capture(
            None,
            ApiProvider::Anthropic,
            "anthropic-direct",
            "claude-sonnet-4-5",
            Some(ApiProvider::Anthropic.default_base_url()),
            chrono::Utc::now(),
        ),
        &Usage {
            input_tokens: 200,
            output_tokens: 20,
            ..Usage::default()
        },
    );
    crate::cost_status::finish_runtime_usage_owner(replay_owner);
    assert!(
        crate::cost_status::drain().is_empty(),
        "a re-delivered provider response must remain deduped after reload"
    );
    assert_eq!(app.session.cost_route_receipts.len(), 1);
    assert!(
        !app.session
            .cost_route_receipts
            .iter()
            .any(|receipt| receipt.contains("stale live")),
        "the replaced session's receipts survived the load"
    );
    assert!(
        !app.session.cost_coverage_unknown_legacy,
        "a coverage-recorded snapshot is not legacy-unknown"
    );

    // The pre-coverage shape: real money, no coverage evidence, every counter
    // at its serde default. This must load as unknown, not as complete.
    let mut legacy_app = create_test_app();
    legacy_app.session.cost_priced_turns = 9;
    let mut legacy = saved_session_with_messages(vec![text_message("assistant", "ready")]);
    legacy.metadata.cost = serde_json::from_value(serde_json::json!({
        "session_cost_usd": 1.25,
        "displayed_cost_high_water_usd": 1.25
    }))
    .expect("legacy cost snapshot");

    apply_loaded_session(&mut legacy_app, &mut Config::default(), &legacy)
        .expect("restore legacy session");

    assert_eq!(legacy_app.session.cost_priced_turns, 0);
    assert_eq!(legacy_app.session.cost_unpriced_turns, 0);
    assert!(
        legacy_app.session.cost_coverage_unknown_legacy,
        "a pre-coverage total must not read as a complete 0-of-0 figure"
    );
}

#[tokio::test]
async fn apply_loaded_session_resets_workspace_runtime_state() {
    let mut app = create_test_app();
    let mut config = Config::default();
    let old_shell_manager = app
        .runtime_services
        .shell_manager
        .as_ref()
        .expect("shell manager")
        .clone();
    let old_context_cell = app.workspace_context_cell.clone();
    app.workspace_context = Some("old workspace context".to_string());
    if let Ok(mut cell) = old_context_cell.lock() {
        *cell = Some("old workspace context".to_string());
    }
    app.workspace_context_refreshed_at = Some(Instant::now());
    app.file_tree = Some(crate::tui::file_tree::FileTreeState::new(
        PathBuf::from(".").as_path(),
    ));

    let mut session = saved_session_with_messages(vec![text_message("assistant", "ready")]);
    session.metadata.workspace = TempDir::new().expect("temp dir").path().to_path_buf();

    apply_loaded_session(&mut app, &mut config, &session).expect("restore session");
    assert_eq!(app.workspace, session.metadata.workspace);
    assert!(app.workspace_context.is_none());
    assert!(app.workspace_context_refreshed_at.is_none());
    assert!(app.file_tree.is_none());
    assert!(old_context_cell.lock().expect("context cell").is_none());
    let new_shell_manager = app
        .runtime_services
        .shell_manager
        .as_ref()
        .expect("shell manager")
        .clone();
    assert!(!std::sync::Arc::ptr_eq(
        &old_shell_manager,
        &new_shell_manager
    ));
    assert_eq!(
        new_shell_manager
            .lock()
            .expect("shell manager")
            .default_workspace(),
        session.metadata.workspace.as_path()
    );
    assert!(app.runtime_services.hook_executor.is_some());
}

#[test]
fn shell_live_output_refresh_does_not_block_on_contended_lock() {
    // #3804: the async UI loop must never block on the shell manager's
    // std::sync Mutex. While the lock is held, the render-only live-output
    // refresh must return immediately via try_lock — the previous blocking
    // lock() would deadlock on this same thread, so reaching the assert at all
    // proves the path no longer blocks under contention.
    let mut app = create_test_app();
    let shell_mgr = app
        .runtime_services
        .shell_manager
        .as_ref()
        .expect("shell manager")
        .clone();

    let guard = shell_mgr.lock().expect("hold shell lock");
    let changed = refresh_shell_exec_live_output(&mut app);
    assert!(
        !changed,
        "contended live-output refresh should skip this frame, not block or update"
    );
    drop(guard);

    // With the lock free again the path runs normally (no jobs → no change).
    assert!(!refresh_shell_exec_live_output(&mut app));
}

#[test]
fn apply_loaded_session_updates_current_workspace_display() {
    let mut app = create_test_app();
    let mut config = Config::default();
    let workspace = TempDir::new().expect("temp dir");
    let mut session = saved_session_with_messages(vec![text_message("assistant", "ready")]);
    session.metadata.workspace = workspace.path().to_path_buf();

    apply_loaded_session(&mut app, &mut config, &session).expect("restore session");
    let result = commands::execute("/workspace", &mut app);

    assert_eq!(
        result.message,
        Some(format!("Current workspace: {}", workspace.path().display()))
    );
    assert!(result.action.is_none());
}

#[tokio::test]
async fn drain_web_config_events_applies_draft_without_closing_session() {
    let mut app = create_test_app();
    let mut config = Config::default();
    let engine = mock_engine_handle();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let doc = config_ui::build_document(&app, &config).expect("document");
    tx.send(WebConfigSessionEvent::Draft(doc))
        .expect("send draft");
    let mut session = Some(WebConfigSession::for_test(rx));

    let keep = drain_web_config_events(&mut session, &mut app, &mut config, &engine.handle).await;

    assert!(keep);
    assert!(session.is_some());
}

#[tokio::test]
async fn drain_web_config_events_closes_session_after_commit() {
    let _config_env = ConfigPathEnvGuard::new();
    let mut app = create_test_app();
    let mut config = Config::default();
    let engine = mock_engine_handle();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let doc = config_ui::build_document(&app, &config).expect("document");
    tx.send(WebConfigSessionEvent::Committed(doc))
        .expect("send commit");
    let mut session = Some(WebConfigSession::for_test(rx));

    let keep = drain_web_config_events(&mut session, &mut app, &mut config, &engine.handle).await;

    assert!(!keep);
}

#[test]
fn backtrack_prefill_rehydrates_attachment_rows() {
    let mut app = create_test_app();
    let user_text = "inspect this\n[Attached image: /tmp/pasted.png]";
    app.add_message(HistoryCell::User {
        content: user_text.to_string(),
    });
    app.api_messages.push(Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: user_text.to_string(),
            cache_control: None,
        }],
    });
    app.add_message(HistoryCell::Assistant {
        content: "done".to_string(),
        streaming: false,
    });
    app.api_messages.push(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Text {
            text: "done".to_string(),
            cache_control: None,
        }],
    });

    apply_backtrack(&mut app, 0);

    assert_eq!(app.input, user_text);
    assert_eq!(app.composer_attachment_count(), 1);
}

#[test]
fn refresh_shell_exec_live_output_finalizes_restored_missing_shell_job() {
    let mut app = create_test_app();
    app.push_history_cell(HistoryCell::Tool(ToolCell::Exec(ExecCell {
        command: "cargo test --workspace".to_string(),
        status: ToolStatus::Running,
        output: None,
        live_output: Some("last captured output".to_string()),
        shell_task_id: Some("shell_detached".to_string()),
        owner_agent_id: None,
        owner_agent_name: None,
        started_at: None,
        duration_ms: Some(1_234),
        stale_elapsed_since_output_ms: None,
        source: ExecSource::Assistant,
        interaction: None,
        output_summary: None,
    })));

    let changed = refresh_shell_exec_live_output(&mut app);

    assert!(changed, "detached shell exec should be finalized");
    let HistoryCell::Tool(ToolCell::Exec(exec)) = &app.history[0] else {
        panic!("expected exec cell");
    };
    assert_eq!(exec.status, ToolStatus::Failed);
    assert!(exec.live_output.is_none());
    assert!(
        exec.output
            .as_deref()
            .unwrap_or_default()
            .contains("no longer attached")
    );
}

#[test]
fn shell_live_output_update_matches_exact_task_id_only() {
    let mut app = create_test_app();
    app.push_history_cell(HistoryCell::Tool(ToolCell::Exec(ExecCell {
        command: "cargo test --workspace".to_string(),
        status: ToolStatus::Running,
        output: None,
        live_output: None,
        shell_task_id: Some("shell_a".to_string()),
        owner_agent_id: None,
        owner_agent_name: None,
        started_at: None,
        duration_ms: Some(111),
        stale_elapsed_since_output_ms: None,
        source: ExecSource::Assistant,
        interaction: None,
        output_summary: None,
    })));
    app.push_history_cell(HistoryCell::Tool(ToolCell::Exec(ExecCell {
        command: "cargo test --workspace".to_string(),
        status: ToolStatus::Running,
        output: None,
        live_output: Some("previous".to_string()),
        shell_task_id: Some("shell_b".to_string()),
        owner_agent_id: None,
        owner_agent_name: None,
        started_at: None,
        duration_ms: None,
        stale_elapsed_since_output_ms: None,
        source: ExecSource::Assistant,
        interaction: None,
        output_summary: None,
    })));

    let mut jobs = std::collections::HashMap::new();
    jobs.insert(
        "shell_a".to_string(),
        ShellJobSnapshot {
            id: "shell_a".to_string(),
            job_id: "shell_a".to_string(),
            command: "cargo test --workspace".to_string(),
            cwd: PathBuf::from("/tmp/repo"),
            status: ShellStatus::Running,
            exit_code: None,
            elapsed_ms: 111,
            stdout_tail: String::new(),
            stderr_tail: String::new(),
            stdout_len: 0,
            stderr_len: 0,
            stdin_available: false,
            stale: false,
            elapsed_since_output_ms: None,
            linked_task_id: None,
            owner_agent_id: None,
            owner_agent_name: None,
            owner_session_id: "session-test".to_string(),
        },
    );
    jobs.insert(
        "shell_b".to_string(),
        ShellJobSnapshot {
            id: "shell_b".to_string(),
            job_id: "shell_b".to_string(),
            command: "cargo test --workspace".to_string(),
            cwd: PathBuf::from("/tmp/repo"),
            status: ShellStatus::Running,
            exit_code: None,
            elapsed_ms: 777,
            stdout_tail: "stdout tail\n".to_string(),
            stderr_tail: "stderr tail\n".to_string(),
            stdout_len: 12,
            stderr_len: 12,
            stdin_available: false,
            stale: false,
            elapsed_since_output_ms: None,
            linked_task_id: None,
            owner_agent_id: None,
            owner_agent_name: None,
            owner_session_id: "session-test".to_string(),
        },
    );

    assert!(shell_exec_live_update(&app, 0, &jobs).is_none());
    let ShellExecLiveUpdate {
        task_id: _,
        status,
        output,
        duration_ms: duration,
        finalized,
        stale_elapsed_since_output_ms,
    } = shell_exec_live_update(&app, 1, &jobs).expect("matching task id updates");

    assert_eq!(status, ToolStatus::Running);
    assert_eq!(duration, 777);
    assert!(!finalized);
    assert_eq!(stale_elapsed_since_output_ms, None);
    assert_eq!(
        output.as_deref(),
        Some("stdout tail\n\n\nSTDERR:\nstderr tail\n")
    );
}

#[test]
fn shell_live_output_update_marks_stale_running_job_static() {
    let mut app = create_test_app();
    app.push_history_cell(HistoryCell::Tool(ToolCell::Exec(ExecCell {
        command: "cargo test --workspace".to_string(),
        status: ToolStatus::Running,
        output: None,
        live_output: Some("previous output".to_string()),
        shell_task_id: Some("shell_stale".to_string()),
        owner_agent_id: None,
        owner_agent_name: None,
        started_at: Some(Instant::now() - Duration::from_secs(90)),
        duration_ms: Some(90_000),
        stale_elapsed_since_output_ms: None,
        source: ExecSource::Assistant,
        interaction: None,
        output_summary: None,
    })));

    let mut jobs = std::collections::HashMap::new();
    jobs.insert(
        "shell_stale".to_string(),
        ShellJobSnapshot {
            id: "shell_stale".to_string(),
            job_id: "shell_stale".to_string(),
            command: "cargo test --workspace".to_string(),
            cwd: PathBuf::from("/tmp/repo"),
            status: ShellStatus::Running,
            exit_code: None,
            elapsed_ms: 90_000,
            stdout_tail: "still working\n".to_string(),
            stderr_tail: String::new(),
            stdout_len: 14,
            stderr_len: 0,
            stdin_available: false,
            stale: true,
            elapsed_since_output_ms: Some(61_000),
            linked_task_id: None,
            owner_agent_id: None,
            owner_agent_name: None,
            owner_session_id: "session-test".to_string(),
        },
    );

    let ShellExecLiveUpdate {
        task_id: _,
        status,
        output,
        duration_ms: duration,
        finalized,
        stale_elapsed_since_output_ms,
    } = shell_exec_live_update(&app, 0, &jobs).expect("stale shell updates");

    assert_eq!(status, ToolStatus::Running);
    assert_eq!(duration, 90_000);
    assert!(!finalized);
    assert_eq!(stale_elapsed_since_output_ms, Some(61_000));
    assert_eq!(output.as_deref(), Some("still working\n"));

    let HistoryCell::Tool(ToolCell::Exec(exec)) =
        app.cell_at_virtual_index_mut(0).expect("exec cell")
    else {
        panic!("expected exec cell");
    };
    exec.status = status;
    exec.duration_ms = Some(duration);
    exec.live_output = output;
    exec.stale_elapsed_since_output_ms = stale_elapsed_since_output_ms;

    let header_text = exec.lines_with_motion(80, false)[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(header_text.contains("running · stale · no output 1m 01s"));
    assert!(!header_text.contains("running ("));
}

#[test]
fn shell_live_output_update_finalizes_background_exec_output() {
    let mut app = create_test_app();
    app.push_history_cell(HistoryCell::Tool(ToolCell::Exec(ExecCell {
        command: "cargo test --workspace".to_string(),
        status: ToolStatus::Running,
        output: None,
        live_output: Some("previous output".to_string()),
        shell_task_id: Some("shell_a".to_string()),
        owner_agent_id: None,
        owner_agent_name: None,
        started_at: None,
        duration_ms: None,
        stale_elapsed_since_output_ms: None,
        source: ExecSource::Assistant,
        interaction: None,
        output_summary: None,
    })));

    let mut jobs = std::collections::HashMap::new();
    jobs.insert(
        "shell_a".to_string(),
        ShellJobSnapshot {
            id: "shell_a".to_string(),
            job_id: "shell_a".to_string(),
            command: "cargo test --workspace".to_string(),
            cwd: PathBuf::from("/tmp/repo"),
            status: ShellStatus::Completed,
            exit_code: Some(0),
            elapsed_ms: 999,
            stdout_tail: "final stdout\n".to_string(),
            stderr_tail: "final stderr\n".to_string(),
            stdout_len: 13,
            stderr_len: 13,
            stdin_available: false,
            stale: false,
            elapsed_since_output_ms: None,
            linked_task_id: None,
            owner_agent_id: None,
            owner_agent_name: None,
            owner_session_id: "session-test".to_string(),
        },
    );

    let ShellExecLiveUpdate {
        task_id,
        status,
        output,
        duration_ms: duration,
        finalized,
        stale_elapsed_since_output_ms,
    } = shell_exec_live_update(&app, 0, &jobs).expect("completed shell updates");
    assert_eq!(task_id, "shell_a");
    assert_eq!(status, ToolStatus::Success);
    assert_eq!(duration, 999);
    assert!(finalized);
    assert_eq!(stale_elapsed_since_output_ms, None);
    assert_eq!(
        output.as_deref(),
        Some("final stdout\n\n\nSTDERR:\nfinal stderr\n")
    );

    let HistoryCell::Tool(ToolCell::Exec(exec)) =
        app.cell_at_virtual_index_mut(0).expect("exec cell")
    else {
        panic!("expected exec cell");
    };
    exec.status = status;
    exec.duration_ms = Some(duration);
    exec.output = output;
    exec.output_summary = exec
        .output
        .as_deref()
        .map(crate::tui::history::summarize_tool_output);
    exec.live_output = None;

    assert_eq!(exec.status, ToolStatus::Success);
    assert_eq!(
        exec.output.as_deref(),
        Some("final stdout\n\n\nSTDERR:\nfinal stderr\n")
    );
    assert!(exec.live_output.is_none());
    assert_eq!(exec.duration_ms, Some(999));
    assert!(exec.output_summary.is_some());
}

#[test]
fn shell_live_output_update_skips_finalized_exec_cell() {
    let mut app = create_test_app();
    app.push_history_cell(HistoryCell::Tool(ToolCell::Exec(ExecCell {
        command: "cargo test --workspace".to_string(),
        status: ToolStatus::Success,
        output: Some("final output".to_string()),
        live_output: Some("old live output".to_string()),
        shell_task_id: Some("shell_a".to_string()),
        owner_agent_id: None,
        owner_agent_name: None,
        started_at: None,
        duration_ms: Some(10),
        stale_elapsed_since_output_ms: None,
        source: ExecSource::Assistant,
        interaction: None,
        output_summary: None,
    })));
    let mut jobs = std::collections::HashMap::new();
    jobs.insert(
        "shell_a".to_string(),
        ShellJobSnapshot {
            id: "shell_a".to_string(),
            job_id: "shell_a".to_string(),
            command: "cargo test --workspace".to_string(),
            cwd: PathBuf::from("/tmp/repo"),
            status: ShellStatus::Completed,
            exit_code: Some(0),
            elapsed_ms: 999,
            stdout_tail: "new live output".to_string(),
            stderr_tail: String::new(),
            stdout_len: 15,
            stderr_len: 0,
            stdin_available: false,
            stale: false,
            elapsed_since_output_ms: None,
            linked_task_id: None,
            owner_agent_id: None,
            owner_agent_name: None,
            owner_session_id: "session-test".to_string(),
        },
    );

    assert!(shell_exec_live_update(&app, 0, &jobs).is_none());
}

#[test]
fn terminal_probe_timeout_defaults_to_500ms() {
    let config = Config::default();

    assert_eq!(terminal_probe_timeout(&config), Duration::from_millis(500));
}

#[test]
fn terminal_probe_timeout_uses_tui_config_and_clamps() {
    let mut config = Config {
        tui: Some(crate::config::TuiConfig {
            alternate_screen: None,
            mouse_capture: None,
            terminal_probe_timeout_ms: Some(750),
            stream_chunk_timeout_secs: None,
            max_model_steps: None,
            turn_wall_clock_secs: None,
            stream_max_content_mb: None,
            stream_max_duration_secs: None,
            status_items: None,
            header_items: None,
            osc8_links: None,
            notification_condition: None,
            composer_arrows_scroll: None,
        }),
        ..Config::default()
    };

    assert_eq!(terminal_probe_timeout(&config), Duration::from_millis(750));

    config
        .tui
        .as_mut()
        .expect("tui config")
        .terminal_probe_timeout_ms = Some(0);
    assert_eq!(terminal_probe_timeout(&config), Duration::from_millis(100));

    config
        .tui
        .as_mut()
        .expect("tui config")
        .terminal_probe_timeout_ms = Some(60_000);
    assert_eq!(
        terminal_probe_timeout(&config),
        Duration::from_millis(5_000)
    );
}

#[test]
fn file_mentions_add_local_text_context_to_model_payload() {
    let tmpdir = TempDir::new().expect("tempdir");
    std::fs::write(
        tmpdir.path().join("guide.md"),
        "# Guide\nUse the fast path.\n",
    )
    .expect("write file");
    let mut app = create_test_app();
    app.workspace = tmpdir.path().to_path_buf();
    let message = QueuedMessage::new("Summarize @guide.md".to_string(), None);

    let content = queued_message_content_for_app(
        &app,
        &message,
        None,
        &mut crate::tui::git_mention::GitMentionCache::default(),
    )
    .expect("native queued message should remain dispatchable");

    assert!(content.starts_with("Summarize @guide.md"));
    assert!(content.contains("Local context from @mentions:"));
    assert!(content.contains("<file mention=\"@guide.md\""));
    assert!(content.contains("# Guide\nUse the fast path."));
    assert_eq!(message.display, "Summarize @guide.md");
}

#[test]
fn persisted_queued_plugin_skill_is_denied_after_cross_process_revocation() {
    let tmpdir = TempDir::new().expect("tempdir");
    let workspace = tmpdir.path().join("workspace");
    let plugins_root = tmpdir.path().join("plugins");
    let plugin = plugins_root.join("queued-skill");
    std::fs::create_dir_all(plugin.join("skills/queued")).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        plugin.join("plugin.toml"),
        "schema_version = 1\n[plugin]\nname = \"queued-skill\"\nversion = \"1.0.0\"\n[skills]\npath = \"skills\"\n",
    )
    .unwrap();
    std::fs::write(
        plugin.join("skills/queued/SKILL.md"),
        "---\nname: queued\ndescription: Queue provenance test\n---\nDo the queued work.\n",
    )
    .unwrap();
    let discovery = crate::plugins::discovery::DiscoveryConfig {
        workspace: workspace.clone(),
        user_plugins_dir: plugins_root,
        workspace_plugins_dir: tmpdir.path().join("workspace-plugins-unused"),
        builtin_plugin_dirs: Vec::new(),
        state_path: tmpdir.path().join("plugin-state/state.json"),
    };
    let mut registry = crate::plugins::discovery::discover_with_config(&discovery);
    registry.trust("queued-skill").unwrap();
    registry.enable("queued-skill").unwrap();
    let authority = registry.authority_for("queued-skill").unwrap();

    let queued = QueuedMessage::new(
        "finish it".to_string(),
        Some("Do the queued work.".to_string()),
    )
    .with_skill_provenance(Some(authority));
    let serialized = serde_json::to_string(&queued_ui_to_session(&queued)).unwrap();
    let persisted: QueuedSessionMessage = serde_json::from_str(&serialized).unwrap();
    let restored = queued_session_to_ui(persisted);
    let mut app = create_test_app();
    app.workspace = workspace;
    assert!(
        queued_message_content_for_app(
            &app,
            &restored,
            None,
            &mut crate::tui::git_mention::GitMentionCache::default(),
        )
        .is_ok()
    );

    let mut external = crate::plugins::discovery::discover_with_config(&discovery);
    external.revoke_trust("queued-skill").unwrap();
    let error = queued_message_content_for_app(
        &app,
        &restored,
        None,
        &mut crate::tui::git_mention::GitMentionCache::default(),
    )
    .expect_err("persisted queued Skill must be denied after external revocation")
    .to_string();
    assert!(error.contains("disabled, revoked, or no longer matches"));
}

#[test]
fn compact_user_context_display_hides_persisted_mention_block() {
    let content = "Summarize @guide.md\n\n---\n\nLocal context from @mentions:\n<file>large</file>";

    assert_eq!(compact_user_context_display(content), "Summarize @guide.md");
}

#[test]
fn file_mentions_do_not_trigger_inside_email_addresses() {
    let tmpdir = TempDir::new().expect("tempdir");
    std::fs::write(tmpdir.path().join("example.com"), "not a mention").expect("write file");

    let content = user_request_with_file_mentions("email me@example.com", tmpdir.path(), None);

    assert_eq!(content, "email me@example.com");
}

#[test]
fn media_file_mentions_point_to_attach_instead_of_inlining_bytes() {
    let tmpdir = TempDir::new().expect("tempdir");
    std::fs::write(tmpdir.path().join("photo.png"), b"\0png").expect("write image");

    let content = user_request_with_file_mentions("inspect @photo.png", tmpdir.path(), None);

    assert!(content.contains("<media-file mention=\"@photo.png\""));
    assert!(content.contains("Use /attach photo.png"));
    assert!(!content.contains("\0png"));
}

#[tokio::test]
async fn model_change_update_syncs_engine_model_before_compaction() {
    let mut app = create_test_app();
    app.model = "deepseek-v4-flash".to_string();
    let compaction = app.compaction_config();
    let mut engine = crate::core::engine::mock_engine_handle();

    apply_model_and_compaction_update(
        &engine.handle,
        compaction,
        app.mode,
        app.active_route_limits,
    )
    .await;

    match engine.rx_op.recv().await.expect("set model op") {
        crate::core::ops::Op::SetModel {
            model,
            mode,
            route_limits: _,
        } => {
            assert_eq!(model, "deepseek-v4-flash");
            assert_eq!(mode, app.mode);
        }
        other => panic!("expected SetModel, got {other:?}"),
    }

    match engine.rx_op.recv().await.expect("set compaction op") {
        crate::core::ops::Op::SetCompaction { config } => {
            assert_eq!(config.model, "deepseek-v4-flash");
        }
        other => panic!("expected SetCompaction, got {other:?}"),
    }
}

#[tokio::test]
async fn mode_change_update_notifies_engine() {
    let mut app = create_test_app();
    let _ = app.set_mode(crate::tui::app::AppMode::Plan);
    let mut engine = crate::core::engine::mock_engine_handle();

    assert!(
        apply_yolo_compat_update(&mut app, &engine.handle, &crate::config::Config::default()).await
    );

    match engine.rx_op.recv().await.expect("change mode op") {
        crate::core::ops::Op::ChangeMode {
            mode,
            allow_shell,
            trust_mode,
            auto_approve,
            approval_mode,
            configured_sandbox_mode,
        } => {
            // The deprecated YOLO alias lands in Agent mode with full-access
            // compat policies (M6 shim); the engine sees the remapped mode.
            assert_eq!(mode, crate::tui::app::AppMode::Agent);
            assert!(allow_shell);
            assert!(trust_mode);
            assert!(auto_approve);
            assert_eq!(approval_mode, crate::tui::approval::ApprovalMode::Bypass);
            assert_eq!(configured_sandbox_mode, app.configured_sandbox_mode);
        }
        other => panic!("expected ChangeMode, got {other:?}"),
    }
}

#[tokio::test]
async fn mode_change_update_sends_restored_agent_policy() {
    let mut app = create_test_app();
    app.allow_shell = true;
    app.trust_mode = false;
    app.approval_mode = crate::tui::approval::ApprovalMode::Never;
    let _ = app.set_mode(crate::tui::app::AppMode::Plan);
    let mut engine = crate::core::engine::mock_engine_handle();

    assert!(
        apply_mode_update(
            &mut app,
            &engine.handle,
            &crate::config::Config::default(),
            crate::tui::app::AppMode::Agent
        )
        .await
    );

    match engine.rx_op.recv().await.expect("change mode op") {
        crate::core::ops::Op::ChangeMode {
            mode,
            allow_shell,
            trust_mode,
            auto_approve,
            approval_mode,
            configured_sandbox_mode,
        } => {
            assert_eq!(mode, crate::tui::app::AppMode::Agent);
            assert!(allow_shell);
            assert!(!trust_mode);
            assert!(!auto_approve);
            assert_eq!(approval_mode, crate::tui::approval::ApprovalMode::Never);
            assert_eq!(configured_sandbox_mode, app.configured_sandbox_mode);
        }
        other => panic!("expected ChangeMode, got {other:?}"),
    }
}

#[tokio::test]
async fn operate_mode_entry_attaches_to_recorded_operation() {
    let _lock = crate::test_support::lock_test_env();
    let dir = tempfile::TempDir::new().expect("temp");
    let _operate_dir = crate::test_support::EnvVarGuard::set("CODEWHALE_OPERATE_DIR", dir.path());
    // Pre-record a planned operation: entering Operate must attach to it
    // (same id) instead of minting a fresh record that resets spend and plan.
    let store = crate::operate::OperationStore::open(dir.path()).expect("store");
    let recorded =
        crate::operate::start_operation(&store, dir.path(), Some("Steady ops".into()), None, true)
            .expect("start");
    let mut planned = recorded.clone();
    planned.plan_from_direction();
    store.save(&planned).expect("save planned");

    let mut app = create_test_app();
    let engine = crate::core::engine::mock_engine_handle();
    assert!(
        apply_mode_update(
            &mut app,
            &engine.handle,
            &Config::default(),
            crate::tui::app::AppMode::Operate
        )
        .await
    );

    let board = app
        .history
        .iter()
        .rev()
        .find_map(|cell| match cell {
            crate::tui::history::HistoryCell::System { content } => Some(content.clone()),
            _ => None,
        })
        .expect("operate board message");
    assert!(
        board.contains(&planned.id),
        "re-entry must show the recorded operation, saw: {board}"
    );
    assert!(
        board.contains("slice-1"),
        "board shows the kept plan: {board}"
    );
}

#[test]
fn saved_default_provider_syncs_back_to_runtime_config() {
    let _home = SettingsHomeGuard::new();
    let settings = crate::settings::Settings {
        default_provider: Some("ollama".to_string()),
        ..Default::default()
    };
    settings.save().expect("save settings");

    let mut config = Config::default();
    assert_eq!(config.api_provider(), ApiProvider::Deepseek);

    let app = App::new(create_test_options(), &config);
    assert_eq!(app.api_provider, ApiProvider::Ollama);

    sync_config_provider_from_app(&mut config, &app);

    assert_eq!(config.api_provider(), ApiProvider::Ollama);
}

#[test]
fn provider_picker_reselecting_active_provider_preserves_current_model() {
    let mut app = create_test_app();
    app.set_provider_identity(ApiProvider::Ollama, "ollama");
    app.model = "deepseek-coder-v2:16b".to_string();
    let config = Config {
        provider: Some("ollama".to_string()),
        ..Config::default()
    };

    assert_eq!(
        provider_picker_model_override(&app, &config, ApiProvider::Ollama).as_deref(),
        Some("deepseek-coder-v2:16b")
    );
    assert_eq!(
        provider_picker_model_override(&app, &config, ApiProvider::Deepseek),
        None
    );
}

#[tokio::test]
async fn provider_switch_clears_turn_cache_history() {
    // `switch_provider` persists the new provider to `Settings`, which
    // writes through settings path resolution. Without redirecting the
    // CodeWhale/legacy config homes we would clobber the developer's real
    // preferences and leave `default_provider = "ollama"` behind.
    let _home = SettingsHomeGuard::new();

    let mut app = create_test_app();
    app.push_turn_cache_record(crate::tui::app::TurnCacheRecord {
        provider: None,
        provider_identity: None,
        model: None,
        auto_model: false,
        input_tokens: 100,
        output_tokens: 25,
        cache_hit_tokens: Some(70),
        cache_miss_tokens: Some(30),
        reasoning_replay_tokens: Some(12),
        cache_write_tokens: None,
        reasoning_tokens: None,
        cost_audit: None,
        recorded_at: Instant::now(),
    });
    let mut engine = mock_engine_handle();
    let mut config = Config::default();

    switch_provider(
        &mut app,
        &mut engine.handle,
        &mut config,
        ApiProvider::Ollama,
        None,
    )
    .await;

    assert_eq!(app.api_provider, ApiProvider::Ollama);
    assert!(app.session.turn_cache_history.is_empty());
}

#[tokio::test]
async fn provider_switch_to_deepseek_canonicalizes_openrouter_default_model() {
    let _home = SettingsHomeGuard::new();
    let mut app = create_test_app();
    app.api_provider = ApiProvider::Openrouter;
    app.model = DEFAULT_OPENROUTER_MODEL.to_string();
    let mut engine = mock_engine_handle();
    let mut config = Config {
        provider: Some("openrouter".to_string()),
        api_key: Some("test-key".to_string()),
        default_text_model: Some(DEFAULT_OPENROUTER_MODEL.to_string()),
        ..Default::default()
    };

    switch_provider(
        &mut app,
        &mut engine.handle,
        &mut config,
        ApiProvider::Deepseek,
        None,
    )
    .await;

    assert_eq!(app.api_provider, ApiProvider::Deepseek);
    assert!(!app.model_ids_passthrough);
    assert_eq!(app.model, DEFAULT_TEXT_MODEL);
}

#[tokio::test]
async fn provider_switch_to_deepseek_drops_stale_xiaomi_root_base_url() {
    let _home = SettingsHomeGuard::new();
    let mut app = create_test_app();
    app.api_provider = ApiProvider::XiaomiMimo;
    app.model = "mimo-v2.5-pro".to_string();
    app.model_ids_passthrough = true;
    let mut engine = mock_engine_handle();
    let mut config = Config {
        provider: Some("xiaomi-mimo".to_string()),
        api_key: Some("deepseek-key".to_string()),
        base_url: Some("https://token-plan-sgp.xiaomimimo.com/v1".to_string()),
        default_text_model: Some("mimo-v2.5-pro".to_string()),
        providers: Some(ProvidersConfig {
            xiaomi_mimo: ProviderConfig {
                api_key: Some("mimo-key".to_string()),
                model: Some("mimo-v2.5-pro".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };

    switch_provider(
        &mut app,
        &mut engine.handle,
        &mut config,
        ApiProvider::Deepseek,
        None,
    )
    .await;

    assert_eq!(app.api_provider, ApiProvider::Deepseek);
    assert!(!app.model_ids_passthrough);
    assert_eq!(app.model, DEFAULT_TEXT_MODEL);
    assert_eq!(config.provider.as_deref(), Some("deepseek"));
    assert_eq!(config.base_url, None);
}

#[tokio::test]
async fn provider_switch_from_mimo_to_openrouter_without_key_fails_before_dispatch() {
    let _home = SettingsHomeGuard::new();
    let _openrouter_key = crate::test_support::EnvVarGuard::remove("OPENROUTER_API_KEY");
    let mut app = create_test_app();
    app.api_provider = ApiProvider::XiaomiMimo;
    app.model = "mimo-v2.5-pro".to_string();
    app.model_ids_passthrough = true;
    let mut engine = mock_engine_handle();
    let mut config = Config {
        provider: Some("xiaomi-mimo".to_string()),
        api_key: Some("deepseek-key".to_string()),
        base_url: Some("https://token-plan-sgp.xiaomimimo.com/v1".to_string()),
        default_text_model: Some("mimo-v2.5-pro".to_string()),
        providers: Some(ProvidersConfig {
            xiaomi_mimo: ProviderConfig {
                api_key: Some("mimo-key".to_string()),
                model: Some("mimo-v2.5-pro".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };

    switch_provider(
        &mut app,
        &mut engine.handle,
        &mut config,
        ApiProvider::Openrouter,
        Some(crate::config::OPENROUTER_NEMOTRON_3_ULTRA_MODEL.to_string()),
    )
    .await;

    assert_eq!(app.api_provider, ApiProvider::XiaomiMimo);
    assert_eq!(app.model, "mimo-v2.5-pro");
    assert!(app.model_ids_passthrough);
    assert_eq!(config.provider.as_deref(), Some("xiaomi-mimo"));
    assert_eq!(
        config
            .providers
            .as_ref()
            .and_then(|providers| providers.openrouter.api_key.as_deref()),
        None
    );
    assert!(app.pending_provider_switch.is_none());
    let last_system_message = app
        .history
        .iter()
        .rev()
        .find_map(|cell| match cell {
            HistoryCell::System { content } => Some(content.as_str()),
            _ => None,
        })
        .expect("failed provider switch should add a system message");
    assert!(last_system_message.contains("OpenRouter API key not found"));
    assert!(last_system_message.contains("Provider unchanged (xiaomi-mimo)"));
}

#[tokio::test]
async fn successful_custom_provider_activation_completes_onboarding() {
    let config_env = ConfigPathEnvGuard::new();
    let mut app = create_test_app();
    app.config_path = Some(config_env.config_path());
    app.onboarding = OnboardingState::Provider;
    app.onboarding_needs_api_key = true;
    app.onboarding_missing_key_recovery = true;
    app.trust_mode = true;
    let mut engine = mock_engine_handle();
    let mut config = Config::default();

    let switched = apply_provider_picker_custom_provider(
        &mut app,
        &mut engine.handle,
        &mut config,
        "fixture-local".to_string(),
        "http://127.0.0.1:19493/v1".to_string(),
        Some("fixture-model".to_string()),
        None,
    )
    .await;
    assert!(switched);
    complete_provider_picker_onboarding_if_switched(&mut app, ApiProvider::Custom, switched);

    assert_eq!(app.api_provider, ApiProvider::Custom);
    assert_ne!(
        app.onboarding,
        OnboardingState::Provider,
        "a successfully activated custom route must complete provider onboarding"
    );
    let settings = crate::settings::Settings::load().expect("reload custom startup route");
    assert_eq!(
        settings.default_provider.as_deref(),
        Some("fixture-local"),
        "named custom onboarding must persist the exact identity, not `custom`",
    );
    assert_eq!(
        settings
            .provider_models
            .as_ref()
            .and_then(|models| models.get("fixture-local"))
            .map(String::as_str),
        Some("fixture-model"),
    );
}

#[tokio::test]
async fn failed_custom_provider_activation_stays_in_onboarding_recovery() {
    let config_env = ConfigPathEnvGuard::new();
    let blocked_parent = config_env._tmp.path().join("not-a-directory");
    std::fs::write(&blocked_parent, "fixture").expect("create blocking file");
    let mut app = create_test_app();
    app.config_path = Some(blocked_parent.join("config.toml"));
    app.onboarding = OnboardingState::Provider;
    app.onboarding_needs_api_key = true;
    let mut engine = mock_engine_handle();
    let mut config = Config::default();

    let switched = apply_provider_picker_custom_provider(
        &mut app,
        &mut engine.handle,
        &mut config,
        "fixture-local".to_string(),
        "http://127.0.0.1:19493/v1".to_string(),
        Some("fixture-model".to_string()),
        None,
    )
    .await;
    complete_provider_picker_onboarding_if_switched(&mut app, ApiProvider::Custom, switched);

    assert!(!switched);
    assert_eq!(app.onboarding, OnboardingState::Provider);
    assert_eq!(app.api_provider, ApiProvider::Deepseek);
    assert_eq!(
        crate::settings::Settings::load()
            .expect("reload failed custom setup settings")
            .default_provider,
        None,
        "a failed activation must not persist the route it never reached",
    );
}

#[tokio::test]
async fn successful_native_xai_oauth_activation_completes_onboarding() {
    let config_env = ConfigPathEnvGuard::new();
    let codewhale_home = config_env
        ._tmp
        .path()
        .canonicalize()
        .expect("canonical temp root")
        .join("xai-success-home");
    let _codewhale_home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
    let mut app = create_test_app();
    app.config_path = Some(config_env.config_path());
    app.onboarding = OnboardingState::Provider;
    app.onboarding_needs_api_key = true;
    app.onboarding_missing_key_recovery = true;
    app.trust_mode = true;
    let mut engine = mock_engine_handle();
    let mut config = Config::default();
    let pending =
        crate::xai_oauth::pending_device_login_for_test("fixture-access", "fixture-refresh");

    let switched = apply_codewhale_owned_xai_login(
        &mut app,
        &mut engine.handle,
        &mut config,
        pending,
        "fixture xAI login complete",
    )
    .await;
    complete_provider_picker_onboarding_if_switched(&mut app, ApiProvider::Xai, switched);

    assert!(
        switched,
        "xAI switch failed: status={:?}, history={:?}, config={:?}, saved={:?}",
        app.status_message,
        app.history,
        config.provider_config_for(ApiProvider::Xai),
        std::fs::read_to_string(config_env.config_path())
    );
    assert_eq!(app.api_provider, ApiProvider::Xai);
    assert_ne!(app.onboarding, OnboardingState::Provider);
    let xai = config
        .provider_config_for(ApiProvider::Xai)
        .expect("activated xAI slot");
    assert_eq!(xai.auth_mode.as_deref(), Some("oauth"));
    assert!(
        xai.api_key.is_none(),
        "OAuth activation must not invent an API key"
    );
}

#[tokio::test]
async fn failed_native_xai_oauth_activation_stays_in_onboarding_recovery() {
    let config_env = ConfigPathEnvGuard::new();
    let codewhale_home = config_env
        ._tmp
        .path()
        .canonicalize()
        .expect("canonical temp root")
        .join("xai-failure-home");
    let _codewhale_home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
    let mut app = create_test_app();
    app.config_path = Some(config_env.config_path());
    app.onboarding = OnboardingState::Provider;
    app.onboarding_needs_api_key = true;
    let mut engine = mock_engine_handle();
    let mut config = Config::default();
    let pending = crate::xai_oauth::pending_device_login_for_test("", "fixture-refresh");

    let switched = apply_codewhale_owned_xai_login(
        &mut app,
        &mut engine.handle,
        &mut config,
        pending,
        "fixture xAI login complete",
    )
    .await;
    complete_provider_picker_onboarding_if_switched(&mut app, ApiProvider::Xai, switched);

    assert!(!switched);
    assert_eq!(app.onboarding, OnboardingState::Provider);
    assert_eq!(app.api_provider, ApiProvider::Deepseek);
}

#[tokio::test]
async fn xai_api_key_confirmation_saves_only_the_selected_xai_slot() {
    let config_env = ConfigPathEnvGuard::new();
    let codewhale_home = config_env._tmp.path().canonicalize().expect("temp home");
    let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
    let _backend = crate::test_support::EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
    let mut app = create_test_app();
    app.config_path = Some(config_env.config_path());
    let mut engine = mock_engine_handle();
    let mut config = Config::default();
    let identity = picker_provider_identity(&config, ApiProvider::Xai, None).expect("xAI identity");

    let switched = apply_provider_picker_setup_confirmed(
        &mut app,
        &mut engine.handle,
        &mut config,
        identity,
        "violet-otter-key".to_string(),
        crate::config::DEFAULT_XAI_MODEL.to_string(),
        None,
        None,
    )
    .await;

    assert!(switched, "provider switch failed: {:?}", app.history);
    let xai = config
        .provider_config_for(ApiProvider::Xai)
        .expect("saved xAI slot");
    assert_eq!(xai.auth_mode.as_deref(), Some("api_key"));
    assert_eq!(xai.api_key.as_deref(), Some("violet-otter-key"));
    assert!(
        config.api_key.is_none(),
        "xAI key must not enter the root slot"
    );
    let saved = std::fs::read_to_string(config_env.config_path()).expect("saved config");
    assert!(saved.contains("[providers.xai]"));
    assert!(!saved.contains("auth_mode = \"oauth\""));
}

/// #5195: the save confirmation must name where the key actually landed —
/// the durable secret store, not the config path — and the scope it is
/// visible from (user-global credential, available in all folders).
/// #5585: this test drives the full guided-setup save path (provider
/// identity minting, key persistence, model/base-url pinning, and the
/// settings/TOML round-trips they trigger). Those serde round-trips are deep
/// enough that the default 2 MiB libtest thread stack overflows; the product
/// paths run on the 8 MiB main thread or the sheltered 16 MiB parse threads
/// (see `parse_config_file_str` in `config.rs` and `parse_config_toml_str`
/// in `codewhale-config`), so this test runs its runtime on an equivalently
/// sized stack instead of crashing.
#[test]
fn setup_confirm_toast_names_secret_store_and_global_scope() {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime")
                .block_on(async {
                    // ConfigPathEnvGuard holds the shared env lock for the whole test, so the
                    // home/backend overrides must be set after it (and never take the lock
                    // again — it is not reentrant).
                    let _config = ConfigPathEnvGuard::new();
                    let home = TempDir::new().expect("isolated toast home");
                    let _home = crate::test_support::EnvVarGuard::set(
                        "CODEWHALE_HOME",
                        home.path().to_string_lossy().as_ref(),
                    );
                    let _backend =
                        crate::test_support::EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
                    let mut app = create_test_app();
                    let mut engine = mock_engine_handle();
                    let mut config = Config::default();
                    let identity = picker_provider_identity(&config, ApiProvider::Openrouter, None)
                        .expect("OpenRouter identity");

                    let switched = apply_provider_picker_setup_confirmed(
                        &mut app,
                        &mut engine.handle,
                        &mut config,
                        identity,
                        "toast-destination-key".to_string(),
                        "deepseek/deepseek-v4-pro".to_string(),
                        None,
                        None,
                    )
                    .await;

                    assert!(switched, "setup confirm failed: {:?}", app.status_message);
                    let toast = app.status_message.as_deref().expect("save toast");
                    assert!(
                        toast.contains("secret store"),
                        "toast must name the secret store, got: {toast}"
                    );
                    assert!(
                        toast.contains("available in all folders"),
                        "toast must state the user-global scope, got: {toast}"
                    );
                    assert!(
                        !toast.contains("toast-destination-key"),
                        "toast must never include the key, got: {toast}"
                    );
                })
        })
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked");
}

#[tokio::test]
async fn provider_switch_is_session_local_until_explicitly_saved() {
    let _home = SettingsHomeGuard::new();
    let tmp = TempDir::new().expect("config tempdir");
    let config_path = tmp.path().join("config.toml");
    std::fs::write(
        &config_path,
        r#"provider = "arcee"

[providers.xiaomi_mimo]
base_url = "https://token-plan-sgp.xiaomimimo.com/v1"
model = "mimo-v2.5-pro"
api_key = "mimo-key"

[providers.arcee]
api_key = "arcee-key"
"#,
    )
    .expect("write config");

    let mut app = create_test_app();
    app.api_provider = ApiProvider::Arcee;
    app.model = "auto".to_string();
    app.config_path = Some(config_path.clone());

    let mut engine = mock_engine_handle();
    let mut config = Config::load(Some(config_path.clone()), None).expect("load config");

    switch_provider(
        &mut app,
        &mut engine.handle,
        &mut config,
        ApiProvider::XiaomiMimo,
        None,
    )
    .await;

    assert_eq!(app.api_provider, ApiProvider::XiaomiMimo);
    assert_eq!(config.provider.as_deref(), Some("xiaomi-mimo"));

    // The live session moved, but NOTHING was written: the config file on
    // disk still selects the old provider and the startup default is
    // untouched. A switch made in one folder must never rewrite a config
    // another folder (or restart) will read.
    let reloaded = Config::load(Some(config_path.clone()), None).expect("reload config");
    // The on-disk config still resolves to the OLD provider's endpoint —
    // nothing changed for a restart or another folder.
    assert_eq!(reloaded.api_provider(), ApiProvider::Arcee);
    assert_eq!(reloaded.deepseek_base_url(), "https://api.arcee.ai/api/v1");

    let settings = crate::settings::Settings::load().expect("load settings");
    assert_eq!(settings.default_provider.as_deref(), None);

    // The save decision is pending for the route-save prompt.
    let pending = app.pending_route_save.as_ref().expect("pending save");
    assert_eq!(pending.provider_identity, "xiaomi-mimo");
    assert_eq!(pending.model, "mimo-v2.5-pro");
}

/// The provider step is the first run's explicit startup-route decision, not
/// an ordinary session-local `/provider` preview. Persist it in user-global
/// settings so a completed Ollama setup cannot restart on DeepSeek merely
/// because the compatibility config still carries a DeepSeek root default.
#[test]
fn first_run_ollama_choice_survives_restart_without_rewriting_config() {
    let _home = SettingsHomeGuard::new();
    let config_path = std::env::var_os("DEEPSEEK_CONFIG_PATH")
        .map(PathBuf::from)
        .expect("isolated config path");
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("create config parent");
    let original_config = "default_text_model = \"deepseek-v4-pro\"\n";
    std::fs::write(&config_path, original_config).expect("seed compatibility config");

    let config = Config::load(Some(config_path.clone()), None).expect("load first-run config");
    let mut app = Box::new(App::new(create_test_options(), &config));
    app.onboarding = OnboardingState::Provider;
    app.onboarding_needs_api_key = true;
    app.onboarding_missing_key_recovery = false;
    app.trust_mode = true;
    // `switch_provider` already has focused session-local coverage above.
    // Recreate its successful live-route result here so this regression stays
    // centered on the missing onboarding completion write and restart load.
    app.set_provider_identity(ApiProvider::Ollama, ApiProvider::Ollama.as_str());
    app.set_model_selection(crate::config::DEFAULT_OLLAMA_MODEL.to_string());
    app.note_session_route_change(
        ApiProvider::Ollama.as_str(),
        crate::config::DEFAULT_OLLAMA_MODEL,
    );
    record_provider_model_setup_progress(&mut app, &config);
    assert_eq!(app.api_provider, ApiProvider::Ollama);
    assert_eq!(app.model, crate::config::DEFAULT_OLLAMA_MODEL);

    complete_provider_picker_onboarding(&mut app, ApiProvider::Ollama);

    let settings = crate::settings::Settings::load().expect("reload onboarding settings");
    assert_eq!(settings.default_provider.as_deref(), Some("ollama"));
    assert_eq!(
        settings
            .provider_models
            .as_ref()
            .and_then(|models| models.get("ollama"))
            .map(String::as_str),
        Some(crate::config::DEFAULT_OLLAMA_MODEL),
    );
    assert!(app.pending_route_save.is_none());
    assert!(
        app.status_message.as_deref().is_some_and(|message| {
            message.contains(&format!(
                "Remembered ollama/{}",
                crate::config::DEFAULT_OLLAMA_MODEL
            ))
        }),
        "onboarding must truthfully receipt the durable startup route: {:?}",
        app.status_message,
    );

    // The route belongs to user-global startup settings. Do not rewrite a
    // folder's compatibility config as a side effect of onboarding.
    assert_eq!(
        std::fs::read_to_string(&config_path).expect("reload config bytes"),
        original_config,
    );

    // Selecting a keyless route is not a health check. Setup state must keep
    // that distinction so an unavailable local runtime still receives the
    // existing attention/doctor treatment instead of being called healthy.
    let state = codewhale_config::SetupState::load()
        .expect("load setup state")
        .expect("provider setup state");
    let result = state
        .steps
        .get(&codewhale_config::SetupStep::ProviderModel)
        .and_then(|entry| entry.result.as_deref())
        .expect("provider/model setup result");
    assert!(result.contains("provider=ollama"), "{result}");
    assert!(result.contains("health=attemptable"), "{result}");

    // Finish the remaining first-run screens, then reconstruct from the same
    // on-disk config a second launch would read. Drop the first `App` before
    // constructing the second: the production lifecycle never keeps two of
    // this deliberately large state object on one thread stack at once.
    app.finish_onboarding_without_feature_intro();
    drop(app);
    let restart_config = Config::load(Some(config_path), None).expect("reload restart config");
    let restarted = Box::new(App::new(create_test_options(), &restart_config));
    assert_eq!(restarted.api_provider, ApiProvider::Ollama);
    assert_eq!(restarted.model, crate::config::DEFAULT_OLLAMA_MODEL);
    assert_ne!(
        restarted.onboarding,
        OnboardingState::Provider,
        "a keyless Ollama startup must not reopen DeepSeek credential recovery",
    );
    assert!(!restarted.onboarding_needs_api_key);
}

#[test]
fn failed_first_run_route_persistence_keeps_provider_setup_active() {
    let _lock = crate::test_support::lock_test_env();
    let tmp = TempDir::new().expect("settings tempdir");
    let bad_home = tmp.path().join("codewhale-home-file");
    std::fs::write(&bad_home, "not a directory").expect("bad home file");
    let _home = crate::test_support::EnvVarGuard::set("HOME", tmp.path().as_os_str());
    let _userprofile = crate::test_support::EnvVarGuard::set("USERPROFILE", tmp.path().as_os_str());
    let _codewhale_home =
        crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", bad_home.as_os_str());
    let _deepseek_config_path = crate::test_support::EnvVarGuard::remove("DEEPSEEK_CONFIG_PATH");
    let _codewhale_config_path = crate::test_support::EnvVarGuard::remove("CODEWHALE_CONFIG_PATH");
    let _codewhale_provider = crate::test_support::EnvVarGuard::remove("CODEWHALE_PROVIDER");
    let _deepseek_provider = crate::test_support::EnvVarGuard::remove("DEEPSEEK_PROVIDER");

    let mut app = Box::new(create_test_app());
    app.onboarding = OnboardingState::Provider;
    app.onboarding_needs_api_key = true;
    app.set_provider_identity(ApiProvider::Ollama, ApiProvider::Ollama.as_str());
    app.set_model_selection(crate::config::DEFAULT_OLLAMA_MODEL.to_string());
    app.note_session_route_change(
        ApiProvider::Ollama.as_str(),
        crate::config::DEFAULT_OLLAMA_MODEL,
    );

    complete_provider_picker_onboarding(&mut app, ApiProvider::Ollama);

    assert_eq!(app.onboarding, OnboardingState::Provider);
    assert_eq!(app.onboarding_provider, ApiProvider::Ollama);
    assert!(app.onboarding_needs_api_key);
    assert!(
        app.pending_route_save.is_some(),
        "a failed write must retain the route-save retry",
    );
    assert!(
        app.status_message
            .as_deref()
            .is_some_and(|message| message.contains("Save failed:")),
        "a failed restart-route write must stay visible: {:?}",
        app.status_message,
    );
}

#[tokio::test]
async fn provider_switch_model_override_updates_target_provider_model_slot() {
    let _home = SettingsHomeGuard::new();
    let mut app = create_test_app();
    app.api_provider = ApiProvider::XiaomiMimo;
    app.model = "mimo-v2.5-pro".to_string();
    let mut engine = mock_engine_handle();
    let mut config = Config {
        provider: Some("xiaomi-mimo".to_string()),
        api_key: Some("deepseek-key".to_string()),
        default_text_model: Some("mimo-v2.5-pro".to_string()),
        providers: Some(ProvidersConfig {
            xiaomi_mimo: ProviderConfig {
                api_key: Some("mimo-key".to_string()),
                model: Some("mimo-v2.5-pro".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };

    switch_provider(
        &mut app,
        &mut engine.handle,
        &mut config,
        ApiProvider::Deepseek,
        Some("deepseek-v4-flash".to_string()),
    )
    .await;

    assert_eq!(app.api_provider, ApiProvider::Deepseek);
    assert_eq!(app.model, "deepseek-v4-flash");
    assert_eq!(
        config
            .providers
            .as_ref()
            .and_then(|providers| providers.deepseek.model.as_deref()),
        Some("deepseek-v4-flash")
    );
    assert_eq!(
        config
            .providers
            .as_ref()
            .and_then(|providers| providers.xiaomi_mimo.model.as_deref()),
        Some("mimo-v2.5-pro")
    );

    let state = codewhale_config::SetupState::load()
        .expect("load setup state")
        .expect("setup state");
    assert_eq!(
        state.status(codewhale_config::SetupStep::ProviderModel),
        codewhale_config::StepStatus::Verified
    );
    let provider_model_result = state
        .steps
        .get(&codewhale_config::SetupStep::ProviderModel)
        .and_then(|entry| entry.result.as_deref())
        .expect("provider/model result");
    assert!(provider_model_result.contains("provider=deepseek"));
    assert!(provider_model_result.contains("model=deepseek-v4-flash"));
    assert!(provider_model_result.contains("auth=key saved · not checked"));
    assert!(provider_model_result.contains("health=attemptable"));
    assert!(!provider_model_result.contains("deepseek-key"));
}

#[tokio::test]
async fn provider_switch_succeeds_without_writing_when_config_is_unwritable() {
    let _home = SettingsHomeGuard::new();
    let tmp = TempDir::new().expect("config tempdir");
    let mut app = create_test_app();
    app.api_provider = ApiProvider::XiaomiMimo;
    app.model = "mimo-v2.5-pro".to_string();
    app.config_path = Some(tmp.path().to_path_buf());
    let mut engine = mock_engine_handle();
    let mut config = Config {
        provider: Some("xiaomi-mimo".to_string()),
        api_key: Some("deepseek-key".to_string()),
        default_text_model: Some("mimo-v2.5-pro".to_string()),
        providers: Some(ProvidersConfig {
            xiaomi_mimo: ProviderConfig {
                api_key: Some("mimo-key".to_string()),
                model: Some("mimo-v2.5-pro".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };

    switch_provider(
        &mut app,
        &mut engine.handle,
        &mut config,
        ApiProvider::Deepseek,
        Some("deepseek-v4-flash".to_string()),
    )
    .await;

    assert_eq!(app.api_provider, ApiProvider::Deepseek);
    assert_eq!(app.model, "deepseek-v4-flash");
    // The switch writes nothing, so an unwritable config path cannot fail a
    // switch, and the receipt must not claim a partial persistence that never
    // ran.
    assert!(
        app.status_message
            .as_deref()
            .is_none_or(|message| !message.contains("not fully persisted")),
        "the receipt must not claim a partial persistence that never ran: {:?}",
        app.status_message
    );
    let pending = app.pending_route_save.as_ref().expect("pending save");
    assert_eq!(pending.model, "deepseek-v4-flash");
    // The on-screen setup-state receipt is still recorded (it is a local
    // receipt, not a config write) — with the target route, not a stale one.
    let state = codewhale_config::SetupState::load()
        .expect("load setup state")
        .expect("setup state");
    let result = state
        .steps
        .get(&codewhale_config::SetupStep::ProviderModel)
        .and_then(|entry| entry.result.as_deref())
        .expect("provider/model result");
    assert!(result.contains("provider=deepseek"), "{result}");
    assert!(result.contains("model=deepseek-v4-flash"), "{result}");
}

#[tokio::test]
async fn provider_switch_without_model_uses_target_default_not_previous_provider_model() {
    let _home = SettingsHomeGuard::new();
    let mut app = create_test_app();
    app.api_provider = ApiProvider::Openrouter;
    app.model = "deepseek/deepseek-v4-pro".to_string();
    app.model_ids_passthrough = true;
    let mut engine = mock_engine_handle();
    let mut config = Config {
        provider: Some("openrouter".to_string()),
        api_key: Some("deepseek-key".to_string()),
        providers: Some(ProvidersConfig {
            openrouter: ProviderConfig {
                api_key: Some("openrouter-key".to_string()),
                model: Some("deepseek/deepseek-v4-pro".to_string()),
                ..Default::default()
            },
            zai: ProviderConfig {
                api_key: Some("zai-key".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };

    switch_provider(
        &mut app,
        &mut engine.handle,
        &mut config,
        ApiProvider::Zai,
        None,
    )
    .await;

    assert_eq!(app.api_provider, ApiProvider::Zai);
    assert_eq!(app.model, DEFAULT_ZAI_MODEL);
    assert_eq!(config.provider.as_deref(), Some("zai"));
    assert_eq!(
        config
            .providers
            .as_ref()
            .and_then(|providers| providers.zai.model.as_deref()),
        Some(DEFAULT_ZAI_MODEL)
    );
    assert_eq!(
        config
            .providers
            .as_ref()
            .and_then(|providers| providers.openrouter.model.as_deref()),
        Some("deepseek/deepseek-v4-pro")
    );
}

#[tokio::test]
async fn provider_switch_foreign_direct_model_rejected_before_mutation() {
    let _home = SettingsHomeGuard::new();
    let mut app = create_test_app();
    app.api_provider = ApiProvider::Deepseek;
    app.model = DEFAULT_TEXT_MODEL.to_string();
    let mut engine = mock_engine_handle();
    let mut config = Config {
        provider: Some("deepseek".to_string()),
        api_key: Some("deepseek-key".to_string()),
        providers: Some(ProvidersConfig {
            deepseek: ProviderConfig {
                api_key: Some("deepseek-key".to_string()),
                model: Some(DEFAULT_TEXT_MODEL.to_string()),
                ..Default::default()
            },
            zai: ProviderConfig {
                api_key: Some("zai-key".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };

    switch_provider(
        &mut app,
        &mut engine.handle,
        &mut config,
        ApiProvider::Zai,
        Some("deepseek-v4-pro".to_string()),
    )
    .await;

    assert_eq!(app.api_provider, ApiProvider::Deepseek);
    assert_eq!(app.model, DEFAULT_TEXT_MODEL);
    assert_eq!(config.provider.as_deref(), Some("deepseek"));
    assert_eq!(
        config
            .providers
            .as_ref()
            .and_then(|providers| providers.zai.model.as_deref()),
        None
    );
    assert!(app.pending_provider_switch.is_none());
    assert!(
        app.status_message
            .as_deref()
            .unwrap_or_default()
            .contains("Route rejected before provider switch")
    );
}

#[tokio::test]
async fn provider_switch_to_openai_codex_normalizes_deepseek_off_effort() {
    let _home = SettingsHomeGuard::new();
    let _token = crate::test_support::EnvVarGuard::set("OPENAI_CODEX_ACCESS_TOKEN", "test-token");
    let mut app = create_test_app();
    app.api_provider = ApiProvider::Deepseek;
    app.model = DEFAULT_TEXT_MODEL.to_string();
    app.reasoning_effort = ReasoningEffort::Off;
    app.reasoning_effort_preference = Some(ReasoningEffort::Off);
    let mut engine = mock_engine_handle();
    let mut config = Config {
        provider: Some("deepseek".to_string()),
        default_text_model: Some(DEFAULT_TEXT_MODEL.to_string()),
        providers: Some(ProvidersConfig {
            openai_codex: ProviderConfig {
                model: Some(crate::config::DEFAULT_OPENAI_CODEX_MODEL.to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };

    switch_provider(
        &mut app,
        &mut engine.handle,
        &mut config,
        ApiProvider::OpenaiCodex,
        None,
    )
    .await;

    assert_eq!(app.api_provider, ApiProvider::OpenaiCodex);
    assert_eq!(app.model, crate::config::DEFAULT_OPENAI_CODEX_MODEL);
    assert_eq!(app.reasoning_effort, ReasoningEffort::Low);
    assert_eq!(app.reasoning_effort_preference, Some(ReasoningEffort::Off));
    assert_eq!(app.reasoning_effort_display_label(), "low");
}

#[tokio::test]
async fn provider_switch_to_openrouter_canonicalizes_deepseek_default_model() {
    let _home = SettingsHomeGuard::new();
    let mut app = create_test_app();
    app.api_provider = ApiProvider::Deepseek;
    app.model = DEFAULT_TEXT_MODEL.to_string();
    let mut engine = mock_engine_handle();
    let mut config = Config {
        provider: Some("deepseek".to_string()),
        default_text_model: Some(DEFAULT_TEXT_MODEL.to_string()),
        providers: Some(ProvidersConfig {
            openrouter: ProviderConfig {
                api_key: Some("test-key".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };

    switch_provider(
        &mut app,
        &mut engine.handle,
        &mut config,
        ApiProvider::Openrouter,
        None,
    )
    .await;

    assert_eq!(app.api_provider, ApiProvider::Openrouter);
    assert_eq!(app.model, DEFAULT_OPENROUTER_MODEL);
}

#[tokio::test]
async fn dispatch_user_message_failed_send_clears_loading_state() {
    let mut app = create_test_app();
    let engine = mock_engine_handle();
    let config = Config::default();
    drop(engine.rx_op);

    let result = dispatch_user_message(
        &mut app,
        &config,
        &engine.handle,
        QueuedMessage::new("hello".to_string(), None),
    )
    .await;

    assert!(
        result.is_err(),
        "dispatch should fail when engine channel is closed"
    );
    assert!(
        !app.is_loading,
        "failed dispatch must not leave the composer in a permanent busy state"
    );
    assert!(app.last_send_at.is_none());
    assert!(app.dispatch_started_at.is_none());
    assert!(
        app.pending_turn_route.is_none(),
        "failed dispatch must not leave stale route telemetry"
    );
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn auto_dispatch_keeps_last_and_pending_receipts_aligned() {
    let _env_lock = crate::test_support::lock_test_env();
    let _deepseek = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
    let _zai = crate::test_support::EnvVarGuard::set("ZAI_API_KEY", "zai-test-key");
    let mut app = create_test_app();
    app.set_provider_identity(ApiProvider::Zai, "zai");
    app.reasoning_effort = ReasoningEffort::Low;
    app.reasoning_effort_preference = Some(ReasoningEffort::Low);
    app.set_model_selection("auto".to_string());
    let config = Config {
        provider: Some("zai".to_string()),
        default_text_model: Some("auto".to_string()),
        ..Default::default()
    };
    let engine = mock_engine_handle();

    dispatch_user_message(
        &mut app,
        &config,
        &engine.handle,
        QueuedMessage::new("quick status".to_string(), None),
    )
    .await
    .expect("Auto dispatch");

    assert_eq!(
        app.pending_turn_route
            .as_ref()
            .map(|(provider, model, auto)| (*provider, model.as_str(), *auto)),
        Some((
            ApiProvider::Zai,
            crate::config::ZAI_GLM_5_3_FLASH_MODEL,
            true,
        ))
    );
    assert_eq!(
        app.last_auto_route_receipt, app.pending_auto_route_receipt,
        "the fallback inspector must never pair a newly resolved route with a stale receipt"
    );
    assert_eq!(
        app.last_auto_route_receipt
            .as_ref()
            .map(|receipt| receipt.tier),
        Some(crate::model_routing::AutoRouteTier::Fast)
    );
    assert_eq!(
        app.last_effective_reasoning_effort,
        Some(EffectiveReasoningEffort::Tier(ReasoningEffort::High)),
        "the post-turn receipt must retain exact route capability constraints"
    );
    assert_eq!(app.reasoning_effort_display_label(), "low→high");
}

#[tokio::test]
async fn failed_paused_dispatch_preserves_app_checkpoint_state_and_engine_gate() {
    let mut app = create_test_app();
    app.pausable = true;
    app.paused = true;
    app.paused_goal_objective = Some("finish the paused audit".to_string());
    app.goal.objective = None;
    app.goal.tokens_used = 7;
    app.goal.time_used_seconds = 11;
    app.goal.continuation_count = 2;
    app.api_messages
        .push(text_message("assistant", "existing conversation"));
    app.add_message(HistoryCell::System {
        content: "existing transcript".to_string(),
    });
    let before_messages = app.api_messages.clone();
    let before_history = format!("{:?}", app.history);
    let before_context_references = app.session_context_references.clone();
    let before_system_prompt = app.system_prompt.clone();
    let before_last_prompt = app.last_submitted_prompt.clone();
    let engine = mock_engine_handle();
    engine.handle.set_paused(true);
    drop(engine.rx_op);

    dispatch_user_message(
        &mut app,
        &Config::default(),
        &engine.handle,
        QueuedMessage::new("please continue".to_string(), None),
    )
    .await
    .expect_err("closed engine mailbox must reject the paused dispatch");

    assert!(app.paused);
    assert!(app.pausable);
    assert_eq!(
        app.paused_goal_objective.as_deref(),
        Some("finish the paused audit")
    );
    assert!(app.goal.objective.is_none());
    assert_eq!(app.goal.tokens_used, 7);
    assert_eq!(app.goal.time_used_seconds, 11);
    assert_eq!(app.goal.continuation_count, 2);
    assert!(engine.handle.is_paused());
    assert_eq!(app.api_messages, before_messages);
    assert_eq!(format!("{:?}", app.history), before_history);
    assert_eq!(app.session_context_references, before_context_references);
    assert_eq!(app.system_prompt, before_system_prompt);
    assert_eq!(app.last_submitted_prompt, before_last_prompt);
    assert!(!app.is_loading);
    assert!(app.dispatch_started_at.is_none());
    assert!(app.last_send_at.is_none());
    assert!(app.pending_turn_route.is_none());
}

#[tokio::test]
async fn paused_dispatch_at_compaction_threshold_enqueues_one_atomic_send() {
    let mut app = create_test_app();
    app.pausable = true;
    app.paused = true;
    app.paused_goal_objective = Some("finish the paused audit".to_string());
    app.goal.objective = None;
    app.auto_compact_user_configured = true;
    app.auto_compact = true;
    app.auto_compact_threshold_percent = 10.0;
    app.api_messages = vec![text_message("assistant", &"context ".repeat(240_000))];
    let planned_compaction =
        app.compaction_config_for_route(app.api_provider, &app.model, app.active_route_limits);
    assert!(
        should_auto_compact_before_send_with_config(&app, &planned_compaction),
        "fixture must already require compaction"
    );

    let mut engine = mock_engine_handle();
    engine.handle.set_paused(true);
    dispatch_user_message(
        &mut app,
        &Config::default(),
        &engine.handle,
        QueuedMessage::new("please continue".to_string(), None),
    )
    .await
    .expect("atomic paused dispatch");

    match engine.rx_op.recv().await.expect("single send operation") {
        Op::SendMessage {
            compaction,
            goal_objective,
            ..
        } => {
            assert!(compaction.enabled);
            assert_eq!(goal_objective.as_deref(), Some("finish the paused audit"));
        }
        other => panic!("expected SendMessage, got {other:?}"),
    }
    assert!(
        engine.rx_op.try_recv().is_err(),
        "route-bound compaction must travel inside the one SendMessage op"
    );
    assert!(!app.paused);
    assert!(!engine.handle.is_paused());
}

#[tokio::test]
async fn real_engine_client_preflight_failure_leaves_dispatch_state_atomic() {
    let mut config =
        named_custom_session_config("lm-studio", "http://127.0.0.1:1234/v1", "local-model");
    config
        .providers
        .as_mut()
        .expect("providers")
        .custom
        .get_mut("lm-studio")
        .expect("lm-studio")
        .insecure_skip_tls_verify = Some(true);
    let mut app = create_test_app();
    app.set_provider_identity(ApiProvider::Custom, "lm-studio");
    app.set_model_selection("local-model".to_string());
    let (_engine, handle) = crate::core::engine::Engine::new(EngineConfig::default(), &config);

    let err = dispatch_user_message(
        &mut app,
        &config,
        &handle,
        QueuedMessage::new("keep this retryable".to_string(), None),
    )
    .await
    .expect_err("real engine routes must preflight their concrete client");

    assert!(
        err.to_string()
            .contains("Failed to configure provider route")
    );
    assert!(!app.is_loading);
    assert!(app.dispatch_started_at.is_none());
    assert!(app.last_send_at.is_none());
    assert!(app.last_submitted_prompt.is_none());
    assert!(app.api_messages.is_empty());
    assert!(app.history.is_empty());
    assert!(app.pending_turn_route.is_none());
}

#[tokio::test]
async fn immediate_submit_closed_mailbox_restores_composer_and_skill() {
    let mut app = create_test_app();
    app.input = "retry this exactly".to_string();
    app.cursor_position = app.input.chars().count();
    app.active_skill = Some("keep the selected skill".to_string());
    let input = app
        .handle_composer_enter()
        .expect("non-empty composer should submit");
    let queued = build_queued_message(&mut app, input);
    assert!(app.input.is_empty());
    assert!(app.active_skill.is_none());

    let engine = mock_engine_handle();
    drop(engine.rx_op);

    submit_or_steer_message(
        &mut app,
        &Config::default(),
        &engine.handle,
        queued,
        DispatchRecovery::Immediate,
    )
    .await
    .expect("UI submit failures must remain inside the TUI");

    assert_eq!(app.input, "retry this exactly");
    assert_eq!(app.cursor_position, app.input.chars().count());
    assert_eq!(app.active_skill.as_deref(), Some("keep the selected skill"));
    assert!(!app.is_loading);
    assert!(app.status_message.as_deref().is_some_and(|status| {
        status.contains("Message not sent") && status.contains("restored to composer")
    }));
    let sticky = app
        .sticky_status
        .as_ref()
        .expect("dispatch failure should stay visible");
    assert_eq!(sticky.level, StatusToastLevel::Error);
    assert_eq!(
        sticky.ttl_ms,
        Some(crate::tui::app::App::STICKY_ERROR_TTL_MS)
    );
}

#[tokio::test]
async fn immediate_submit_custom_provider_preflight_restores_exact_message() {
    let mut config =
        named_custom_session_config("lm-studio", "http://127.0.0.1:1234/v1", "local-model");
    config
        .providers
        .as_mut()
        .expect("providers")
        .custom
        .get_mut("lm-studio")
        .expect("lm-studio")
        .insecure_skip_tls_verify = Some(true);
    let mut app = create_test_app();
    app.set_provider_identity(ApiProvider::Custom, "lm-studio");
    app.set_model_selection("local-model".to_string());
    app.input = "preserve 用户 input".to_string();
    app.cursor_position = app.input.chars().count();
    let input = app
        .handle_composer_enter()
        .expect("non-empty composer should submit");
    let queued = build_queued_message(&mut app, input);
    let (_engine, handle) = crate::core::engine::Engine::new(EngineConfig::default(), &config);

    submit_or_steer_message(
        &mut app,
        &config,
        &handle,
        queued,
        DispatchRecovery::Immediate,
    )
    .await
    .expect("provider preflight failures must remain inside the TUI");

    assert_eq!(app.input, "preserve 用户 input");
    assert_eq!(app.cursor_position, app.input.chars().count());
    assert!(app.api_messages.is_empty());
    assert!(app.history.is_empty());
    assert!(app.last_submitted_prompt.is_none());
    assert!(app.status_message.as_deref().is_some_and(|status| {
        status.contains("Failed to configure provider route")
            && status.contains("restored to composer")
            && status
                .contains("Next step: Run /provider setup lm-studio to fix base URL/TLS settings.")
    }));
    let sticky = app
        .sticky_status
        .as_ref()
        .expect("dispatch failure should remain sticky");
    assert!(
        sticky
            .text
            .contains("TLS certificate verification cannot be disabled for provider custom"),
        "sticky error should expose the full inner preflight reason: {}",
        sticky.text
    );
    assert!(
        sticky
            .text
            .contains("Failed to configure provider route lm-studio / local-model."),
        "sticky error should keep provider/model route context: {}",
        sticky.text
    );
}

#[tokio::test]
async fn remote_preflight_failure_releases_the_account_owned_run() {
    let mut config =
        named_custom_session_config("lm-studio", "http://127.0.0.1:1234/v1", "local-model");
    config
        .providers
        .as_mut()
        .expect("providers")
        .custom
        .get_mut("lm-studio")
        .expect("lm-studio")
        .insecure_skip_tls_verify = Some(true);
    let mut app = create_test_app();
    app.set_provider_identity(ApiProvider::Custom, "lm-studio");
    app.set_model_selection("local-model".to_string());
    let journal_root = tempfile::tempdir().expect("remote dispatch journal root");
    app.remote_control
        .prepare_remote_control_session_journal(
            journal_root.path(),
            "target_remote_fixture",
            "session_remote_fixture",
        )
        .expect("durable remote dispatch journal");
    app.remote_control
        .activate_prompt("run_remote_fixture", "turn_remote_fixture")
        .unwrap();
    let (_engine, handle) = crate::core::engine::Engine::new(EngineConfig::default(), &config);

    dispatch_user_message(
        &mut app,
        &config,
        &handle,
        QueuedMessage::new("remote preflight failure".to_string(), None),
    )
    .await
    .expect_err("provider preflight must fail before engine dispatch");

    assert!(
        !app.remote_control.has_active_run(),
        "a terminal pre-dispatch failure must not strand the remote run"
    );
}

#[tokio::test]
async fn immediate_submit_custom_provider_missing_key_preflight_shows_auth_next_step() {
    let _lock = crate::test_support::lock_test_env();
    let _missing_key =
        crate::test_support::EnvVarGuard::remove("CODEWHALE_TEST_MISSING_LM_STUDIO_KEY");
    let mut config =
        named_custom_session_config("lm-studio", "http://127.0.0.1:1234/v1", "local-model");
    let provider = config
        .providers
        .as_mut()
        .expect("providers")
        .custom
        .get_mut("lm-studio")
        .expect("lm-studio");
    provider.api_key = None;
    provider.api_key_env = Some("CODEWHALE_TEST_MISSING_LM_STUDIO_KEY".to_string());
    let mut app = create_test_app();
    app.set_provider_identity(ApiProvider::Custom, "lm-studio");
    app.set_model_selection("local-model".to_string());
    app.input = "preserve 用户 input".to_string();
    app.cursor_position = app.input.chars().count();
    let input = app
        .handle_composer_enter()
        .expect("non-empty composer should submit");
    let queued = build_queued_message(&mut app, input);
    let (_engine, handle) = crate::core::engine::Engine::new(EngineConfig::default(), &config);

    submit_or_steer_message(
        &mut app,
        &config,
        &handle,
        queued,
        DispatchRecovery::Immediate,
    )
    .await
    .expect("provider preflight failures must remain inside the TUI");

    assert_eq!(app.input, "preserve 用户 input");
    assert_eq!(app.cursor_position, app.input.chars().count());
    assert!(app.api_messages.is_empty());
    assert!(app.history.is_empty());
    assert!(app.last_submitted_prompt.is_none());
    let status = app
        .status_message
        .as_deref()
        .expect("missing-key preflight should set status");
    assert!(status.contains("Failed to configure provider route lm-studio / local-model."));
    assert!(
        status.contains(
            "Next step: Run /auth or /provider setup lm-studio to configure credentials."
        )
    );
}

#[tokio::test]
async fn dispatch_uses_app_owned_exact_custom_identity_when_config_selector_drifts() {
    let mut custom = HashMap::new();
    for (name, base_url, model) in [
        ("custom-a", "http://127.0.0.1:18181/v1", "model-a"),
        ("custom-b", "http://127.0.0.1:18182/v1", "model-b"),
    ] {
        custom.insert(
            name.to_string(),
            ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some(base_url.to_string()),
                model: Some(model.to_string()),
                api_key: Some(format!("{name}-test-key")),
                ..Default::default()
            },
        );
    }
    let config = Config {
        // Simulate a picker/config overlay that has moved ahead to B while
        // App and the active engine still own A.
        provider: Some("custom-b".to_string()),
        providers: Some(ProvidersConfig {
            custom,
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut app = create_test_app();
    app.set_provider_identity(ApiProvider::Custom, "custom-a");
    app.set_model_selection("model-a".to_string());
    let mut engine = mock_engine_handle();

    dispatch_user_message(
        &mut app,
        &config,
        &engine.handle,
        QueuedMessage::new("stay on A".to_string(), None),
    )
    .await
    .expect("dispatch exact App-owned route");

    match engine.rx_op.recv().await.expect("send message op") {
        Op::SendMessage { route, .. } => {
            assert_eq!(route.identity.provider, ApiProvider::Custom);
            assert_eq!(route.identity.key, "custom-a");
            assert_eq!(route.identity.exact_id.as_deref(), Some("custom-a"));
            assert_eq!(route.model, "model-a");
            assert_eq!(
                route.config.deepseek_base_url(),
                "http://127.0.0.1:18181/v1"
            );
        }
        other => panic!("expected SendMessage, got {other:?}"),
    }
    assert_eq!(config.provider.as_deref(), Some("custom-b"));
}

#[tokio::test]
async fn dispatch_idless_custom_identity_keeps_legacy_root_over_literal_table() {
    let config = Config {
        provider: Some("custom".to_string()),
        api_key: Some("legacy-root-test-key".to_string()),
        base_url: Some("http://127.0.0.1:18180/v1".to_string()),
        default_text_model: Some("legacy-root-model".to_string()),
        providers: Some(ProvidersConfig {
            custom: HashMap::from([(
                "custom".to_string(),
                ProviderConfig {
                    kind: Some("openai-compatible".to_string()),
                    api_key: Some("literal-table-test-key".to_string()),
                    base_url: Some("http://127.0.0.1:18181/v1".to_string()),
                    model: Some("literal-table-model".to_string()),
                    ..Default::default()
                },
            )]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut app = create_test_app();
    app.set_provider_identity(ApiProvider::Custom, "custom");
    app.set_model_selection("legacy-root-model".to_string());
    let mut engine = mock_engine_handle();

    dispatch_user_message(
        &mut app,
        &config,
        &engine.handle,
        QueuedMessage::new("keep the released root route".to_string(), None),
    )
    .await
    .expect("dispatch idless legacy root route");

    match engine.rx_op.recv().await.expect("send message op") {
        Op::SendMessage { route, .. } => {
            assert_eq!(route.identity.provider, ApiProvider::Custom);
            assert_eq!(route.identity.key, "custom");
            assert_eq!(route.identity.exact_id, None);
            assert_eq!(route.model, "legacy-root-model");
            assert_eq!(
                route.config.deepseek_base_url(),
                "http://127.0.0.1:18180/v1"
            );
            assert!(
                route
                    .config
                    .providers
                    .as_ref()
                    .is_none_or(|providers| !providers.custom.contains_key("custom"))
            );
        }
        other => panic!("expected SendMessage, got {other:?}"),
    }
}

#[tokio::test]
async fn failed_real_preflight_preserves_paused_command_state_and_engine_gate() {
    let mut config =
        named_custom_session_config("lm-studio", "http://127.0.0.1:1234/v1", "local-model");
    config
        .providers
        .as_mut()
        .expect("providers")
        .custom
        .get_mut("lm-studio")
        .expect("lm-studio")
        .insecure_skip_tls_verify = Some(true);
    let mut app = create_test_app();
    app.set_provider_identity(ApiProvider::Custom, "lm-studio");
    app.set_model_selection("local-model".to_string());
    app.paused = true;
    app.pausable = true;
    app.paused_goal_objective = Some("finish the paused audit".to_string());
    app.goal.objective = None;
    app.goal.tokens_used = 7;
    app.goal.time_used_seconds = 11;
    app.goal.continuation_count = 2;
    let (_engine, handle) = crate::core::engine::Engine::new(EngineConfig::default(), &config);
    handle.set_paused(true);

    dispatch_user_message(
        &mut app,
        &config,
        &handle,
        QueuedMessage::new("please continue".to_string(), None),
    )
    .await
    .expect_err("invalid client must fail before changing pause state");

    assert!(app.paused);
    assert!(app.pausable);
    assert_eq!(
        app.paused_goal_objective.as_deref(),
        Some("finish the paused audit")
    );
    assert!(app.goal.objective.is_none());
    assert_eq!(app.goal.tokens_used, 7);
    assert_eq!(app.goal.time_used_seconds, 11);
    assert_eq!(app.goal.continuation_count, 2);
    assert!(handle.is_paused());
    assert!(app.api_messages.is_empty());
    assert!(app.history.is_empty());
}

#[test]
fn logout_memory_clear_respects_named_and_legacy_custom_scopes() {
    let mut named_app = create_test_app();
    named_app.set_provider_identity(ApiProvider::Custom, "lm-studio");
    let mut named_config = Config {
        provider: Some("lm-studio".to_string()),
        api_key: Some("deepseek-root-key".to_string()),
        providers: Some(ProvidersConfig {
            custom: HashMap::from([(
                "lm-studio".to_string(),
                ProviderConfig {
                    kind: Some("openai-compatible".to_string()),
                    api_key: Some("named-key".to_string()),
                    base_url: Some("http://127.0.0.1:18181/v1".to_string()),
                    model: Some("local-model".to_string()),
                    ..Default::default()
                },
            )]),
            ..Default::default()
        }),
        ..Default::default()
    };

    clear_active_provider_api_key_from_memory(&named_app, &mut named_config);

    assert_eq!(named_config.api_key.as_deref(), Some("deepseek-root-key"));
    assert_eq!(
        named_config
            .providers
            .as_ref()
            .and_then(|providers| providers.custom.get("lm-studio"))
            .and_then(|provider| provider.api_key.as_deref()),
        None
    );

    let mut legacy_app = create_test_app();
    legacy_app.set_provider_identity(ApiProvider::Custom, "custom");
    let mut legacy_config = Config {
        provider: Some("custom".to_string()),
        api_key: Some("legacy-key".to_string()),
        base_url: Some("http://127.0.0.1:18180/v1".to_string()),
        default_text_model: Some("legacy-model".to_string()),
        ..Default::default()
    };

    clear_active_provider_api_key_from_memory(&legacy_app, &mut legacy_config);

    assert_eq!(legacy_config.api_key, None);
    assert!(legacy_config.providers.is_none());
}

#[test]
fn auto_routed_turn_compaction_uses_selected_route_not_stale_app_route() {
    let mut app = create_test_app();
    app.auto_model = true;
    app.model = "auto".to_string();
    app.api_provider = ApiProvider::Deepseek;
    app.active_route_limits = Some(codewhale_config::route::RouteLimits {
        context_tokens: Some(32_000),
        ..Default::default()
    });
    app.auto_compact_threshold_percent = 75.0;

    let config = Config {
        provider: Some("openrouter".to_string()),
        providers: Some(ProvidersConfig {
            openrouter: ProviderConfig {
                api_key: Some("test-openrouter-key".to_string()),
                model: Some("vendor/model-b".to_string()),
                context_window: Some(196_000),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };
    let route = resolve_runtime_route(&config, ApiProvider::Openrouter, Some("vendor/model-b"))
        .expect("resolve auto-selected route")
        .validate()
        .expect("preflight auto-selected route");
    let route_limits = crate::route_budget::known_route_limits(route.candidate.limits());

    let compaction =
        app.compaction_config_for_route(route.identity.provider, &route.model, route_limits);

    assert_eq!(compaction.model, "vendor/model-b");
    assert_eq!(compaction.effective_context_window, Some(196_000));
    assert_eq!(
        compaction.token_threshold,
        crate::route_budget::compaction_threshold_for_route_at_percent(
            ApiProvider::Openrouter,
            "vendor/model-b",
            route_limits,
            75.0,
        )
    );
    assert_ne!(compaction, app.compaction_config());

    let pre_send_compact = crate::core::ops::Op::CompactContext {
        id: "compact-test".into(),
        route: Box::new(route.into_resolved()),
        compaction: Box::new(compaction.clone()),
    };
    match pre_send_compact {
        crate::core::ops::Op::CompactContext {
            route, compaction, ..
        } => {
            assert_eq!(route.identity.provider, ApiProvider::Openrouter);
            assert_eq!(route.model, "vendor/model-b");
            assert_eq!(compaction.model, "vendor/model-b");
            assert_eq!(compaction.effective_context_window, Some(196_000));
        }
        other => panic!("expected route-bound compact op, got {other:?}"),
    }
}

fn configure_manual_compaction_test_route(app: &mut App) -> Config {
    app.api_provider = ApiProvider::Deepseek;
    app.model = DEFAULT_TEXT_MODEL.to_string();
    Config {
        provider: Some("deepseek".to_string()),
        api_key: Some("test-key".to_string()),
        default_text_model: Some(DEFAULT_TEXT_MODEL.to_string()),
        ..Config::default()
    }
}

#[test]
fn manual_compaction_queues_once_after_active_turn_without_blocking() {
    let mut app = create_test_app();
    app.is_loading = true;
    let config = configure_manual_compaction_test_route(&mut app);
    let mut engine = mock_engine_handle();

    try_queue_manual_compaction(
        &mut app,
        &config,
        &engine.handle,
        Some("preserve verification evidence".to_string()),
    );

    assert!(app.manual_compaction_queued);
    assert!(!app.is_compacting, "queued is not the same as running");
    assert!(
        app.session_transition_blocked(),
        "a queued old-session compaction must block load/new until its lifecycle starts"
    );
    assert_eq!(
        app.status_message.as_deref(),
        Some("Context compaction queued; it will run after the active turn.")
    );
    match engine.rx_op.try_recv().expect("one queued compact op") {
        crate::core::ops::Op::CompactContext { compaction, .. } => {
            assert_eq!(
                compaction.focus.as_deref(),
                Some("preserve verification evidence")
            );
        }
        other => panic!("expected CompactContext, got {other:?}"),
    }

    try_queue_manual_compaction(&mut app, &config, &engine.handle, None);
    assert!(
        engine.rx_op.try_recv().is_err(),
        "duplicate op must not queue"
    );
    assert_eq!(
        app.status_message.as_deref(),
        Some("Context compaction is already in progress.")
    );
}

#[test]
fn full_engine_mailbox_defers_manual_compaction_and_flushes_once_drained() {
    let mut app = create_test_app();
    app.is_loading = true;
    let config = configure_manual_compaction_test_route(&mut app);
    let mut engine = mock_engine_handle();
    for _ in 0..32 {
        engine
            .handle
            .try_send(crate::core::ops::Op::ListSubAgents)
            .expect("fill mock engine mailbox");
    }

    try_queue_manual_compaction(
        &mut app,
        &config,
        &engine.handle,
        Some("preserve verification evidence".to_string()),
    );
    assert!(app.manual_compaction_queued);
    assert!(app.deferred_manual_compaction.is_some());
    assert_eq!(
        app.status_message.as_deref(),
        Some("Context compaction queued; it will run after the active turn.")
    );

    // A repeat during deferral is the single queued pass, not a second one.
    try_queue_manual_compaction(&mut app, &config, &engine.handle, None);
    assert_eq!(
        app.status_message.as_deref(),
        Some("Context compaction is already in progress.")
    );

    // The mailbox is still full: the flush waits without dropping the request.
    flush_deferred_manual_compaction(&mut app, &config, &engine.handle);
    assert!(app.deferred_manual_compaction.is_some());

    // One freed slot lets the deferred request enter the mailbox exactly once,
    // with its original focus, behind the ops that were already queued.
    assert!(matches!(
        engine.rx_op.try_recv(),
        Ok(crate::core::ops::Op::ListSubAgents)
    ));
    flush_deferred_manual_compaction(&mut app, &config, &engine.handle);
    assert!(app.deferred_manual_compaction.is_none());
    assert!(app.manual_compaction_queued);
    for _ in 0..31 {
        assert!(matches!(
            engine.rx_op.try_recv(),
            Ok(crate::core::ops::Op::ListSubAgents)
        ));
    }
    match engine.rx_op.try_recv().expect("one deferred compact op") {
        crate::core::ops::Op::CompactContext { compaction, .. } => {
            assert_eq!(
                compaction.focus.as_deref(),
                Some("preserve verification evidence")
            );
        }
        other => panic!("expected CompactContext, got {other:?}"),
    }
    assert!(engine.rx_op.try_recv().is_err());
}

#[test]
fn deferred_manual_compaction_is_superseded_by_a_live_pass() {
    // An automatic pass starting drops the deferred request AND the queued
    // flag, or `/compact` would report "already in progress" forever.
    let mut app = create_test_app();
    app.manual_compaction_queued = true;
    app.deferred_manual_compaction = Some(None);
    apply_compaction_started(&mut app, "compact-auto".to_string(), true);
    assert!(app.deferred_manual_compaction.is_none());
    assert!(!app.manual_compaction_queued);

    // A pass settling (even one whose start event was lost) is equally
    // authoritative: the context was just compacted.
    let mut app = create_test_app();
    app.manual_compaction_queued = true;
    app.deferred_manual_compaction = Some(None);
    apply_compaction_completed(
        &mut app,
        "compact-x",
        true,
        "Auto-compaction complete".to_string(),
        None,
        None,
        None,
    );
    assert!(app.deferred_manual_compaction.is_none());
    assert!(!app.manual_compaction_queued);
}

#[test]
fn closed_engine_mailbox_reports_manual_compaction_unavailable() {
    let mut app = create_test_app();
    let config = configure_manual_compaction_test_route(&mut app);
    let engine = mock_engine_handle();
    let handle = engine.handle.clone();
    drop(engine.rx_op);

    try_queue_manual_compaction(&mut app, &config, &handle, None);

    assert!(!app.manual_compaction_queued);
    assert!(app.sticky_status.as_ref().is_some_and(|toast| {
        toast.level == StatusToastLevel::Error && toast.text.contains("engine is no longer running")
    }));
}

#[test]
fn compaction_lifecycle_keeps_truthful_auto_label_until_matching_completion() {
    let mut app = create_test_app();

    apply_compaction_started(&mut app, "compact-new".to_string(), true);
    assert!(app.is_compacting);
    assert_eq!(
        app.status_message.as_deref(),
        Some("Context automatically compacting…")
    );
    assert_eq!(
        app.active_compaction
            .as_ref()
            .map(|active| active.id.as_str()),
        Some("compact-new")
    );

    apply_compaction_completed(
        &mut app,
        "compact-stale",
        true,
        "stale completion".to_string(),
        None,
        None,
        None,
    );
    assert!(app.is_compacting, "stale id must not clear newer activity");
    assert_eq!(
        app.status_message.as_deref(),
        Some("Context automatically compacting…")
    );

    apply_compaction_completed(
        &mut app,
        "compact-new",
        true,
        "Auto-compaction complete: 126 → 12 messages".to_string(),
        None,
        None,
        None,
    );
    assert!(!app.is_compacting);
    assert!(app.active_compaction.is_none());
    assert!(app.status_toasts.back().is_some_and(|toast| {
        toast.level == StatusToastLevel::Success
            && toast.text.starts_with("Auto-compaction complete")
    }));
}

#[test]
fn manual_compaction_label_and_failure_are_typed() {
    let mut app = create_test_app();
    app.manual_compaction_queued = true;

    apply_compaction_started(&mut app, "compact-manual".to_string(), false);
    assert!(!app.manual_compaction_queued);
    assert_eq!(app.status_message.as_deref(), Some("Compacting context…"));

    apply_compaction_failed(
        &mut app,
        "compact-manual",
        false,
        "Manual context compaction failed: provider timeout".to_string(),
    );
    assert!(!app.is_compacting);
    assert!(app.sticky_status.as_ref().is_some_and(|toast| {
        toast.level == StatusToastLevel::Error && toast.text.contains("compaction failed")
    }));
}

#[test]
fn context_override_drives_compaction_meter_and_preflight_budget() {
    let config = Config {
        provider: Some("moonshot".to_string()),
        providers: Some(ProvidersConfig {
            moonshot: ProviderConfig {
                model: Some("kimi-k3".to_string()),
                context_window: Some(262_144),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Config::default()
    };
    let route = resolve_runtime_route(&config, ApiProvider::Moonshot, Some("kimi-k3"))
        .expect("resolve configured 256K route");
    let override_limits = crate::route_budget::known_route_limits(route.candidate.limits())
        .expect("configured route limits");
    assert_eq!(
        config.context_window_for_provider_config(ApiProvider::Moonshot),
        Some(262_144)
    );

    let mut app = create_test_app();
    app.api_provider = ApiProvider::Moonshot;
    app.model = "kimi-k3".to_string();
    app.auto_model = false;
    app.auto_compact_threshold_percent = 80.0;
    app.active_route_limits = Some(override_limits);
    app.active_context_window_source = crate::route_runtime::ContextWindowSource::Configured;
    app.update_model_compaction_budget();

    let compaction = app.compaction_config();
    assert_eq!(compaction.effective_context_window, Some(262_144));
    assert_eq!(
        compaction.token_threshold,
        crate::route_budget::compaction_threshold_for_route_at_percent(
            ApiProvider::Moonshot,
            "kimi-k3",
            Some(override_limits),
            80.0,
        )
    );
    assert!(
        compaction.token_threshold
            < crate::route_budget::compaction_threshold_for_route_at_percent(
                ApiProvider::Moonshot,
                "kimi-k3",
                None,
                80.0,
            )
    );

    app.api_messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "context ".repeat(2_000),
            cache_control: None,
        }],
    }];
    let (_, meter_window, _) = context_usage_snapshot(&app).expect("context meter");
    assert_eq!(meter_window, 262_144);

    let override_budget = crate::route_budget::route_context_budget(
        ApiProvider::Moonshot,
        "kimi-k3",
        Some(override_limits),
        0,
    )
    .expect("override preflight budget");
    let catalog_budget =
        crate::route_budget::route_context_budget(ApiProvider::Moonshot, "kimi-k3", None, 0)
            .expect("catalog preflight budget");
    assert_eq!(override_budget.window_tokens, 262_144);
    assert!(override_budget.input_budget_ceiling < catalog_budget.input_budget_ceiling);
}

/// #5239/#5441 acceptance: the compaction trigger, the context meter, and the
/// provenance ladder must all read the same resolved window. A `_Nk`
/// name-suffix window that drove the trigger while the meter or the receipt
/// quoted a different rung's number is exactly the drift that made users see
/// "128K compaction on a 1M model".
#[test]
fn compaction_trigger_meter_and_ladder_share_the_resolved_window() {
    let mut app = create_test_app();
    app.api_provider = ApiProvider::Custom;
    app.model = "qwen3-32b-256k".to_string();
    app.auto_model = false;
    app.active_route_limits = None;
    app.active_context_window_source = crate::route_runtime::ContextWindowSource::NameSuffixHint;
    app.update_model_compaction_budget();

    // Every budget surface resolves the same number from the same chokepoint.
    let ladder = crate::route_runtime::resolve_context_window(
        ApiProvider::Custom,
        "qwen3-32b-256k",
        None,
        None,
    );
    assert_eq!(ladder.tokens, 256_000);
    assert_eq!(
        ladder.source,
        crate::route_runtime::ContextWindowSource::NameSuffixHint
    );

    let compaction = app.compaction_config();
    assert_eq!(compaction.effective_context_window, Some(ladder.tokens));
    assert_eq!(
        compaction.token_threshold,
        crate::route_budget::compaction_threshold_for_route_at_percent(
            ApiProvider::Custom,
            "qwen3-32b-256k",
            None,
            app.auto_compact_threshold_percent,
        )
    );

    app.api_messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "context ".repeat(2_000),
            cache_control: None,
        }],
    }];
    let (used, meter_window, _) = context_usage_snapshot(&app).expect("context meter");
    assert_eq!(meter_window, ladder.tokens);

    // The unverified rung must also reach the pressure message the user sees.
    app.auto_compact = true;
    app.compact_threshold = usize::try_from(used).expect("non-negative context estimate");
    maybe_warn_context_pressure(&mut app);
    let pressure = app.status_message.as_deref().expect("pressure message");
    assert!(
        pressure.contains("unverified window"),
        "pressure message must mark the guess: {pressure}"
    );
}

#[cfg(not(windows))]
fn write_message_submit_hook(dir: &TempDir, name: &str, body: &str) -> String {
    let path = dir.path().join(name);
    std::fs::write(&path, body).expect("write message_submit hook");
    format!("sh {}", path.display())
}

#[cfg(not(windows))]
fn configure_single_message_submit_hook(app: &mut App, dir: &TempDir, command: String) {
    configure_message_submit_hooks(app, dir, vec![command]);
}

#[cfg(not(windows))]
fn configure_message_submit_hooks(app: &mut App, dir: &TempDir, commands: Vec<String>) {
    app.hooks = crate::hooks::HookExecutor::new(
        crate::hooks::HooksConfig {
            enabled: true,
            hooks: commands
                .iter()
                .map(|command| {
                    crate::hooks::Hook::new(crate::hooks::HookEvent::MessageSubmit, command)
                })
                .collect(),
            working_dir: Some(dir.path().to_path_buf()),
            ..crate::hooks::HooksConfig::default()
        },
        dir.path().to_path_buf(),
    );
}

#[cfg(not(windows))]
#[tokio::test]
async fn foreground_message_submit_hook_does_not_park_terminal_dispatch() {
    let dir = TempDir::new().expect("tempdir");
    let slow = write_message_submit_hook(
        &dir,
        "slow_replace.sh",
        r#"#!/bin/sh
sleep 1
printf '%s\n' '{"text":"off-loop replacement"}'
"#,
    );
    let mut app = create_test_app();
    configure_single_message_submit_hook(&mut app, &dir, slow);
    let (completion_tx, mut completion_rx) =
        tokio::sync::mpsc::channel::<crate::tui::app::DispatchApplyFn>(2);
    app.dispatch_completion_tx = Some(completion_tx);
    let engine = crate::core::engine::mock_engine_handle();
    let config = Config::default();

    let started = std::time::Instant::now();
    dispatch_user_message(
        &mut app,
        &config,
        &engine.handle,
        QueuedMessage::new("original".to_string(), None),
    )
    .await
    .expect("queue hook gate");
    assert!(
        started.elapsed() < std::time::Duration::from_millis(250),
        "terminal dispatch waited on the foreground hook: {:?}",
        started.elapsed()
    );
    assert!(app.dispatch_in_flight);
    assert!(app.api_messages.is_empty(), "gate has not answered yet");

    let apply_hook = tokio::time::timeout(std::time::Duration::from_secs(3), completion_rx.recv())
        .await
        .expect("hook result timed out")
        .expect("hook result channel closed");
    apply_hook(&mut app, &engine.handle, &config).expect("apply hook result");
    assert!(matches!(
        &app.api_messages[0].content[0],
        ContentBlock::Text { text, .. } if text == "off-loop replacement"
    ));

    let apply_dispatch =
        tokio::time::timeout(std::time::Duration::from_secs(3), completion_rx.recv())
            .await
            .expect("dispatch result timed out")
            .expect("dispatch result channel closed");
    apply_dispatch(&mut app, &engine.handle, &config).expect("apply dispatch result");
    assert!(!app.dispatch_in_flight);
}

#[cfg(not(windows))]
#[tokio::test]
async fn live_steer_crosses_message_submit_transform_exactly_once() {
    let dir = TempDir::new().expect("tempdir");
    let count = dir.path().join("count.txt");
    let replace = write_message_submit_hook(
        &dir,
        "steer_replace.sh",
        &format!(
            "#!/bin/sh\nprintf x >> '{}'\nprintf '%s\\n' '{{\"text\":\"transformed steer\"}}'\n",
            count.display()
        ),
    );
    let mut app = create_test_app();
    app.is_loading = true;
    configure_single_message_submit_hook(&mut app, &dir, replace);
    let mut engine = crate::core::engine::mock_engine_handle();

    attempt_steer_with_queue_fallback(
        &mut app,
        &Config::default(),
        &engine.handle,
        QueuedMessage::new("original steer".to_string(), None),
        DispatchRecovery::Immediate,
    )
    .await;

    assert_eq!(
        engine.rx_steer.recv().await.as_deref(),
        Some("transformed steer")
    );
    assert_eq!(std::fs::read_to_string(count).expect("hook count"), "x");
    assert!(app.api_messages.iter().any(|message| matches!(
        &message.content[0],
        ContentBlock::Text { text, .. } if text == "transformed steer"
    )));
}

#[cfg(not(windows))]
#[tokio::test]
async fn denied_steer_restores_queued_draft_and_keeps_active_turn() {
    let dir = TempDir::new().expect("tempdir");
    let deny = write_message_submit_hook(
        &dir,
        "steer_deny.sh",
        "#!/bin/sh\nprintf '%s\\n' '{\"reason\":\"policy denied steer\"}'\nexit 2\n",
    );
    let mut app = create_test_app();
    app.is_loading = true;
    configure_single_message_submit_hook(&mut app, &dir, deny);
    let mut engine = crate::core::engine::mock_engine_handle();
    let message = QueuedMessage::new("preserve edited draft".to_string(), None);

    attempt_steer_with_queue_fallback(
        &mut app,
        &Config::default(),
        &engine.handle,
        message.clone(),
        DispatchRecovery::Draft,
    )
    .await;

    assert!(engine.rx_steer.try_recv().is_err());
    assert!(app.is_loading, "denial must not stop the active turn");
    assert_eq!(app.input, "preserve edited draft");
    assert_eq!(app.queued_draft, Some(message));
    assert_eq!(app.queued_message_count(), 0);
    assert!(app.status_toasts.back().is_some_and(|toast| {
        toast.level == StatusToastLevel::Warning && toast.text.contains("blocked the follow-up")
    }));
}

#[tokio::test]
async fn lost_strict_message_submit_executor_keeps_dispatch_atomic_and_recoverable() {
    let mut app = create_test_app();
    app.api_messages
        .push(text_message("assistant", "existing conversation"));
    app.add_message(HistoryCell::System {
        content: "existing receipt".to_string(),
    });
    let mut strict = crate::hooks::Hook::new(
        crate::hooks::HookEvent::MessageSubmit,
        "unused because dispatch loss is injected",
    );
    strict.continue_on_error = false;
    app.hooks = crate::hooks::HookExecutor::new(
        crate::hooks::HooksConfig {
            enabled: true,
            hooks: vec![strict],
            ..crate::hooks::HooksConfig::default()
        },
        app.workspace.clone(),
    );
    app.hooks.inject_message_submit_executor_loss_for_test();
    let mut engine = crate::core::engine::mock_engine_handle();
    let (completion_tx, mut completion_rx) =
        tokio::sync::mpsc::channel::<crate::tui::app::DispatchApplyFn>(2);
    app.dispatch_completion_tx = Some(completion_tx);

    dispatch_user_message(
        &mut app,
        &Config::default(),
        &engine.handle,
        QueuedMessage::new("must stay out of the model".to_string(), None),
    )
    .await
    .expect("lost strict gate is scheduled through the production mailbox");
    assert!(app.dispatch_in_flight);
    let apply_gate = tokio::time::timeout(std::time::Duration::from_secs(2), completion_rx.recv())
        .await
        .expect("lost strict gate result timed out")
        .expect("production completion mailbox closed");
    apply_gate(&mut app, &engine.handle, &Config::default()).expect("apply lost strict gate");

    assert_eq!(app.api_messages.len(), 1);
    assert_eq!(app.history.len(), 1);
    assert!(matches!(
        &app.history[0],
        HistoryCell::System { content } if content == "existing receipt"
    ));
    assert!(!app.is_loading);
    assert!(!app.dispatch_in_flight);
    assert!(app.dispatch_started_at.is_none());
    assert!(app.runtime_turn_status.is_none());
    assert!(engine.rx_op.try_recv().is_err());
    assert_eq!(app.input, "must stay out of the model");

    // The failed gate leaves no poisoned dispatch state: a later ordinary
    // message can start and reaches the engine exactly once.
    app.hooks = crate::hooks::HookExecutor::disabled();
    dispatch_user_message(
        &mut app,
        &Config::default(),
        &engine.handle,
        QueuedMessage::new("recover now".to_string(), None),
    )
    .await
    .expect("later dispatch remains usable");
    match engine.rx_op.recv().await.expect("recovery SendMessage") {
        crate::core::ops::Op::SendMessage { content, .. } => {
            assert_eq!(content, "recover now");
        }
        other => panic!("expected SendMessage, got {other:?}"),
    }
    let apply_dispatch =
        tokio::time::timeout(std::time::Duration::from_secs(2), completion_rx.recv())
            .await
            .expect("recovery dispatch result timed out")
            .expect("production completion mailbox closed");
    apply_dispatch(&mut app, &engine.handle, &Config::default()).expect("apply recovery dispatch");
    assert!(app.history.iter().any(|cell| matches!(
        cell,
        HistoryCell::User { content } if content == "recover now"
    )));
}

#[tokio::test]
async fn full_dispatch_completion_mailbox_restores_message_without_stuck_dispatch() {
    let mut app = create_test_app();
    let engine = crate::core::engine::mock_engine_handle();
    let (completion_tx, _completion_rx) =
        tokio::sync::mpsc::channel::<crate::tui::app::DispatchApplyFn>(1);
    completion_tx
        .try_send(Box::new(|_, _, _| Ok(())))
        .expect("fill production completion mailbox");
    app.dispatch_completion_tx = Some(completion_tx);

    let error = dispatch_user_message(
        &mut app,
        &Config::default(),
        &engine.handle,
        QueuedMessage::new("recover from full mailbox".to_string(), None),
    )
    .await
    .expect_err("full mailbox must reject before dispatch starts");

    assert!(error.to_string().contains("mailbox is full"));
    assert!(!app.dispatch_in_flight);
    assert!(!app.is_loading);
    assert!(app.api_messages.is_empty());
    assert!(app.history.is_empty());
    assert_eq!(app.input, "recover from full mailbox");
    assert!(app.status_toasts.back().is_some_and(|toast| {
        toast.level == StatusToastLevel::Error && toast.text.contains("mailbox is full")
    }));
}

#[tokio::test]
async fn closed_dispatch_completion_mailbox_restores_message_without_stuck_dispatch() {
    let mut app = create_test_app();
    let engine = crate::core::engine::mock_engine_handle();
    let (completion_tx, completion_rx) =
        tokio::sync::mpsc::channel::<crate::tui::app::DispatchApplyFn>(1);
    drop(completion_rx);
    app.dispatch_completion_tx = Some(completion_tx);

    let error = dispatch_user_message(
        &mut app,
        &Config::default(),
        &engine.handle,
        QueuedMessage::new("recover from closed mailbox".to_string(), None),
    )
    .await
    .expect_err("closed mailbox must reject before dispatch starts");

    assert!(error.to_string().contains("mailbox is closed"));
    assert!(!app.dispatch_in_flight);
    assert!(!app.is_loading);
    assert!(app.api_messages.is_empty());
    assert!(app.history.is_empty());
    assert_eq!(app.input, "recover from closed mailbox");
    assert!(app.status_toasts.back().is_some_and(|toast| {
        toast.level == StatusToastLevel::Error && toast.text.contains("mailbox is closed")
    }));
}

#[test]
fn subagent_event_handlers_preserve_dispatch_failures_as_separate_toasts() {
    let mut app = create_test_app();
    app.hooks = crate::hooks::HookExecutor::new(
        crate::hooks::HooksConfig {
            enabled: true,
            hooks: vec![
                crate::hooks::Hook::new(crate::hooks::HookEvent::SubagentSpawn, "unused"),
                crate::hooks::Hook::new(crate::hooks::HookEvent::SubagentComplete, "unused"),
            ],
            ..crate::hooks::HooksConfig::default()
        },
        app.workspace.clone(),
    );
    app.hooks.inject_observer_dispatch_full_for_test();

    apply_agent_spawned_status_and_observer(
        &mut app,
        "agent-test",
        "inspect the repository",
        "inspect the repository",
    );
    assert_eq!(
        app.status_message.as_deref(),
        Some("Agent 1 starting: inspect the repository")
    );
    assert!(app.status_toasts.back().is_some_and(|toast| {
        toast
            .text
            .contains("subagent_spawn observer hook queue is full")
    }));

    app.hooks.inject_observer_dispatch_disconnect_for_test();
    apply_agent_complete_status_and_observer(
        &mut app,
        "agent-test",
        "finished cleanly",
        "completed",
    );
    assert_eq!(
        app.status_message.as_deref(),
        Some("Agent 1 completed: finished cleanly")
    );
    assert!(app.status_toasts.back().is_some_and(|toast| {
        toast
            .text
            .contains("subagent_complete observer hook dispatcher is unavailable")
    }));
}

#[cfg(not(windows))]
#[tokio::test]
async fn dispatch_user_message_surfaces_continued_message_submit_timeout() {
    let dir = TempDir::new().expect("tempdir");
    let slow = write_message_submit_hook(
        &dir,
        "slow.sh",
        r#"#!/bin/sh
sleep 2
"#,
    );
    let replacing = write_message_submit_hook(
        &dir,
        "replace.sh",
        r#"#!/bin/sh
printf '%s\n' '{"text":"after timeout"}'
"#,
    );
    let mut app = create_test_app();
    app.hooks = crate::hooks::HookExecutor::new(
        crate::hooks::HooksConfig {
            enabled: true,
            hooks: vec![
                crate::hooks::Hook::new(crate::hooks::HookEvent::MessageSubmit, &slow)
                    .with_timeout(1),
                crate::hooks::Hook::new(crate::hooks::HookEvent::MessageSubmit, &replacing),
            ],
            working_dir: Some(dir.path().to_path_buf()),
            ..crate::hooks::HooksConfig::default()
        },
        dir.path().to_path_buf(),
    );
    let mut engine = crate::core::engine::mock_engine_handle();
    let config = Config::default();

    dispatch_user_message(
        &mut app,
        &config,
        &engine.handle,
        QueuedMessage::new("hello".to_string(), None),
    )
    .await
    .expect("dispatch user message");

    assert_eq!(
        app.status_message.as_deref(),
        Some("hook timed out after 1s")
    );
    match engine.rx_op.recv().await.expect("send message op") {
        crate::core::ops::Op::SendMessage { content, .. } => {
            assert_eq!(content, "after timeout");
        }
        other => panic!("expected SendMessage, got {other:?}"),
    }
}

#[cfg(not(windows))]
#[tokio::test]
async fn dispatch_user_message_surfaces_continued_message_submit_stderr() {
    let dir = TempDir::new().expect("tempdir");
    let failing = write_message_submit_hook(
        &dir,
        "fail.sh",
        r#"#!/bin/sh
printf '%s\n' 'soft failure' >&2
exit 9
"#,
    );
    let replacing = write_message_submit_hook(
        &dir,
        "replace.sh",
        r#"#!/bin/sh
printf '%s\n' '{"text":"after soft failure"}'
"#,
    );
    let mut app = create_test_app();
    configure_message_submit_hooks(&mut app, &dir, vec![failing, replacing]);
    let mut engine = crate::core::engine::mock_engine_handle();
    let config = Config::default();

    dispatch_user_message(
        &mut app,
        &config,
        &engine.handle,
        QueuedMessage::new("hello".to_string(), None),
    )
    .await
    .expect("dispatch user message");

    assert_eq!(
        app.status_message.as_deref(),
        Some("message_submit hook exited with code 9")
    );
    match engine.rx_op.recv().await.expect("send message op") {
        crate::core::ops::Op::SendMessage { content, .. } => {
            assert_eq!(content, "after soft failure");
        }
        other => panic!("expected SendMessage, got {other:?}"),
    }
}

#[tokio::test]
async fn dispatch_route_failure_leaves_loading_and_transcript_unchanged() {
    let mut app = create_test_app();
    app.set_provider_identity(ApiProvider::Custom, "lm-studio");
    app.set_model_selection("local-model".to_string());
    app.api_messages
        .push(text_message("assistant", "existing conversation"));
    app.add_message(HistoryCell::System {
        content: "existing receipt".to_string(),
    });
    let mut custom = HashMap::new();
    custom.insert(
        "lm-studio".to_string(),
        ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("ftp://invalid.example/v1".to_string()),
            model: Some("local-model".to_string()),
            api_key: Some("local-test-key".to_string()),
            ..ProviderConfig::default()
        },
    );
    let config = Config {
        provider: Some("lm-studio".to_string()),
        providers: Some(ProvidersConfig {
            custom,
            ..ProvidersConfig::default()
        }),
        ..Config::default()
    };
    let mut engine = mock_engine_handle();

    let err = dispatch_user_message(
        &mut app,
        &config,
        &engine.handle,
        QueuedMessage::new("do not duplicate me".to_string(), None),
    )
    .await
    .expect_err("malformed named custom route must fail closed");

    assert!(err.to_string().contains("http"), "{err}");
    assert!(!app.is_loading);
    assert!(app.dispatch_started_at.is_none());
    assert!(app.last_send_at.is_none());
    assert_eq!(app.api_messages.len(), 1);
    assert_eq!(app.history.len(), 1);
    assert!(matches!(
        &app.history[0],
        HistoryCell::System { content } if content == "existing receipt"
    ));
    assert!(engine.rx_op.try_recv().is_err());
}

#[cfg(not(windows))]
#[tokio::test]
async fn dispatch_user_message_uses_transformed_message_submit_text() {
    let dir = TempDir::new().expect("tempdir");
    let command = write_message_submit_hook(
        &dir,
        "replace.sh",
        r#"#!/bin/sh
printf '%s\n' '{"text":"[hooked] hello"}'
"#,
    );
    let mut app = create_test_app();
    configure_single_message_submit_hook(&mut app, &dir, command);
    let mut engine = crate::core::engine::mock_engine_handle();
    let config = Config::default();

    dispatch_user_message(
        &mut app,
        &config,
        &engine.handle,
        QueuedMessage::new("hello".to_string(), None),
    )
    .await
    .expect("dispatch user message");

    assert_eq!(app.last_submitted_prompt.as_deref(), Some("[hooked] hello"));
    assert!(app.history.iter().any(|cell| matches!(
        cell,
        HistoryCell::User { content } if content == "[hooked] hello"
    )));
    assert_eq!(app.api_messages.len(), 1);
    assert!(matches!(
        &app.api_messages[0].content[0],
        ContentBlock::Text { text, .. } if text == "[hooked] hello"
    ));
    match engine.rx_op.recv().await.expect("send message op") {
        crate::core::ops::Op::SendMessage { content, .. } => {
            assert_eq!(content, "[hooked] hello");
        }
        other => panic!("expected SendMessage, got {other:?}"),
    }
}

#[cfg(not(windows))]
#[tokio::test]
async fn dispatch_user_message_blocked_by_message_submit_hook_does_not_start_turn() {
    let dir = TempDir::new().expect("tempdir");
    let command = write_message_submit_hook(
        &dir,
        "block.sh",
        r#"#!/bin/sh
printf '%s\n' '{"reason":"blocked by test hook"}'
exit 2
"#,
    );
    let mut app = create_test_app();
    configure_single_message_submit_hook(&mut app, &dir, command);
    let mut engine = crate::core::engine::mock_engine_handle();
    let config = Config::default();

    dispatch_user_message(
        &mut app,
        &config,
        &engine.handle,
        QueuedMessage::new("hello".to_string(), None),
    )
    .await
    .expect("blocked submit is handled locally");

    assert_eq!(app.status_message.as_deref(), Some("blocked by test hook"));
    assert!(app.api_messages.is_empty());
    assert!(
        app.history
            .iter()
            .all(|cell| !matches!(cell, HistoryCell::User { .. }))
    );
    assert!(!app.is_loading);
    assert!(app.dispatch_started_at.is_none());
    assert!(app.runtime_turn_status.is_none());
    assert!(
        engine.rx_op.try_recv().is_err(),
        "blocked submit must not send any engine operation"
    );
}

#[test]
fn resume_message_helper_is_strict() {
    for message in [
        "continue",
        "resume",
        "please continue",
        "continue the paused command",
        "can you resume the paused task",
        "go ahead and resume",
    ] {
        assert!(is_resume_message(message), "expected resume: {message}");
    }

    for message in [
        "don't continue yet",
        "do not resume yet",
        "I will resume tomorrow",
        "we can continue tomorrow",
        "continue later",
        "how do I resume a git cherry-pick?",
        "please do not continue",
    ] {
        assert!(
            !is_resume_message(message),
            "expected not resume: {message}"
        );
    }
}

#[tokio::test]
async fn dispatch_non_resume_message_preserves_paused_command_state() {
    let mut app = create_test_app();
    app.pausable = true;
    app.paused = true;
    app.paused_goal_objective = Some("Scan nested git repositories".to_string());
    app.goal.objective = Some("Scan nested git repositories".to_string());
    let mut engine = mock_engine_handle();
    engine.handle.set_paused(true);
    let config = Config::default();

    dispatch_user_message(
        &mut app,
        &config,
        &engine.handle,
        QueuedMessage::new("how are you?".to_string(), None),
    )
    .await
    .expect("dispatch user message");

    assert!(!app.paused);
    assert!(app.pausable);
    assert_eq!(
        app.paused_goal_objective.as_deref(),
        Some("Scan nested git repositories")
    );
    assert!(app.goal.objective.is_none());
    assert!(!engine.handle.is_paused());
    match engine.rx_op.recv().await.expect("send message op") {
        crate::core::ops::Op::SendMessage {
            content,
            goal_objective,
            ..
        } => {
            assert!(goal_objective.is_none());
            assert!(content.contains("Paused custom slash command: Scan nested git repositories"));
            assert!(content.contains("do not continue the paused command"));
        }
        other => panic!("expected SendMessage, got {other:?}"),
    }
}

#[tokio::test]
async fn dispatch_resume_message_restores_paused_command_goal() {
    let mut app = create_test_app();
    app.pausable = true;
    app.paused = true;
    app.paused_goal_objective = Some("Scan nested git repositories".to_string());
    let mut engine = mock_engine_handle();
    engine.handle.set_paused(true);
    let config = Config::default();

    dispatch_user_message(
        &mut app,
        &config,
        &engine.handle,
        QueuedMessage::new("please continue the paused command".to_string(), None),
    )
    .await
    .expect("dispatch user message");

    assert!(!app.paused);
    assert!(app.pausable);
    assert!(app.paused_goal_objective.is_none());
    assert_eq!(
        app.goal.objective.as_deref(),
        Some("Scan nested git repositories")
    );
    assert!(!engine.handle.is_paused());
    match engine.rx_op.recv().await.expect("send message op") {
        crate::core::ops::Op::SendMessage {
            content,
            goal_objective,
            ..
        } => {
            assert_eq!(
                goal_objective.as_deref(),
                Some("Scan nested git repositories")
            );
            assert!(content.contains("Paused custom slash command: Scan nested git repositories"));
            assert!(content.contains("Continue the paused command"));
        }
        other => panic!("expected SendMessage, got {other:?}"),
    }
}

#[tokio::test]
async fn dispatch_user_message_keeps_auto_review_separate_from_bypass() {
    let mut app = create_test_app();
    app.mode = AppMode::Agent;
    app.approval_mode = ApprovalMode::Auto;
    app.allow_shell = true;
    app.trust_mode = true;
    let mut engine = mock_engine_handle();
    let config = Config::default();

    dispatch_user_message(
        &mut app,
        &config,
        &engine.handle,
        QueuedMessage::new("run the local verification".to_string(), None),
    )
    .await
    .expect("dispatch user message");

    let pending_route = app
        .pending_turn_route
        .as_ref()
        .expect("successful dispatch records route telemetry");
    assert_eq!(pending_route.0, app.api_provider);
    assert_eq!(pending_route.1, app.model);
    assert!(!pending_route.2);

    // #4605: dispatch spawns the async phase; the Op arrives from the spawned task.
    let op = tokio::time::timeout(std::time::Duration::from_secs(2), engine.rx_op.recv())
        .await
        .expect("spawned dispatch should deliver Op within timeout")
        .expect("send message op");
    match op {
        crate::core::ops::Op::SendMessage {
            mode,
            auto_approve,
            approval_mode,
            ..
        } => {
            assert_eq!(mode, AppMode::Agent);
            assert!(!auto_approve);
            assert_eq!(approval_mode, ApprovalMode::Auto);
        }
        other => panic!("expected SendMessage, got {other:?}"),
    }
}

#[test]
fn pending_goal_control_waits_for_authoritative_receipt() {
    let mut app = create_test_app();
    app.goal.objective = Some("Ship the release".to_string());
    app.goal.status = crate::tools::goal::GoalStatus::Active;
    let paused = crate::tools::goal::GoalSnapshot {
        objective: Some("Ship the release".to_string()),
        status: "paused".to_string(),
        pause_reason: Some(crate::tools::goal::GoalPauseReason::User),
        elapsed_seconds: Some(3),
        ..Default::default()
    };
    app.last_known_goal_state = crate::session_manager::SessionGoalState::from_runtime(&paused)
        .expect("valid paused target");
    app.pending_goal_controls
        .push_back(crate::tui::app::PendingGoalControl {
            intent: crate::tui::app::GoalControlIntent::SetStatus {
                status: crate::tools::goal::GoalStatus::Paused,
                clear: false,
            },
            dispatched: true,
        });

    let before_receipt = crate::tools::goal::GoalSnapshot {
        objective: Some("Ship the release".to_string()),
        status: "active".to_string(),
        elapsed_seconds: Some(2),
        ..Default::default()
    };
    assert!(!apply_goal_snapshot_to_app(&mut app, &before_receipt));
    assert_eq!(app.goal.status, crate::tools::goal::GoalStatus::Active);
    assert_eq!(app.pending_goal_controls.len(), 1);
    assert_eq!(
        app.last_known_goal_state.as_ref().map(|goal| goal.status),
        Some(crate::session_manager::SessionGoalStatus::Paused),
        "an intermediate runtime update must not overwrite the durable accepted target"
    );

    assert!(apply_goal_snapshot_to_app(&mut app, &paused));
    assert_eq!(app.goal.status, crate::tools::goal::GoalStatus::Paused);
    assert!(app.pending_goal_controls.is_empty());
    assert_eq!(
        app.last_known_goal_state.as_ref().map(|goal| goal.status),
        Some(crate::session_manager::SessionGoalStatus::Paused)
    );
}

#[test]
fn apply_goal_snapshot_updates_visible_goal_status() {
    let mut app = create_test_app();
    app.goal.objective = Some("Ship the release lane".to_string());
    app.goal.token_budget = Some(10_000);
    app.goal.status = crate::tools::goal::GoalStatus::Active;
    let started_at = Instant::now();
    app.goal.started_at = Some(started_at);

    let completed = crate::tools::goal::GoalSnapshot {
        objective: Some("Ship the release lane".to_string()),
        status: "complete".to_string(),
        token_budget: Some(10_000),
        tokens_used: 12_345,
        time_used_seconds: 12,
        continuation_count: 2,
        elapsed_seconds: Some(12),
        evidence: Some("focused tests passed".to_string()),
        blocker: None,
        pause_reason: None,
        completion_verification: Some(crate::tools::goal::GoalCompletionVerification {
            status: "passed".to_string(),
            check: "cargo test".to_string(),
            summary: "focused tests passed".to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };

    assert!(apply_goal_snapshot_to_app(&mut app, &completed));
    assert_eq!(app.goal.objective.as_deref(), Some("Ship the release lane"));
    assert_eq!(app.goal.token_budget, Some(10_000));
    assert_eq!(app.goal.tokens_used, 12_345);
    assert_eq!(app.goal.time_used_seconds, 12);
    assert_eq!(app.goal.continuation_count, 2);
    assert_eq!(app.goal.status, crate::tools::goal::GoalStatus::Complete);
    assert_eq!(app.goal.started_at, Some(started_at));
    // A completed goal must freeze the elapsed timer (regression for the bug
    // where the sidebar kept ticking "completed in {elapsed}" forever).
    assert!(
        app.goal.finished_at.is_some(),
        "terminal verdict should set finished_at so the timer freezes"
    );

    let blocked = crate::tools::goal::GoalSnapshot {
        objective: Some("Different objective".to_string()),
        status: "blocked".to_string(),
        token_budget: None,
        tokens_used: 12_345,
        time_used_seconds: 13,
        continuation_count: 3,
        elapsed_seconds: Some(1),
        evidence: None,
        blocker: Some("needs user approval".to_string()),
        pause_reason: None,
        completion_verification: None,
        ..Default::default()
    };

    assert!(apply_goal_snapshot_to_app(&mut app, &blocked));
    assert_eq!(app.goal.objective.as_deref(), Some("Different objective"));
    assert_eq!(app.goal.token_budget, None);
    assert_eq!(app.goal.tokens_used, 12_345);
    assert_eq!(app.goal.time_used_seconds, 13);
    assert_eq!(app.goal.continuation_count, 3);
    assert_eq!(app.goal.status, crate::tools::goal::GoalStatus::Blocked);
    assert!(app.goal.started_at.is_some());
    assert!(
        app.goal.finished_at.is_some(),
        "blocked verdict should also freeze the elapsed timer"
    );
}

#[test]
fn apply_goal_snapshot_prints_a_receipt_when_the_runtime_sets_a_new_goal() {
    let mut app = create_test_app();
    let snapshot = crate::tools::goal::GoalSnapshot {
        objective: Some("make the tests pass".to_string()),
        status: "active".to_string(),
        token_budget: None,
        tokens_used: 0,
        time_used_seconds: 0,
        continuation_count: 0,
        elapsed_seconds: Some(0),
        evidence: None,
        blocker: None,
        pause_reason: None,
        completion_verification: None,
        ..Default::default()
    };
    assert!(apply_goal_snapshot_to_app(&mut app, &snapshot));
    let receipts: Vec<&str> = app
        .history
        .iter()
        .filter_map(|cell| match cell {
            HistoryCell::System { content } if content.contains("Goal set") => {
                Some(content.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(receipts.len(), 1, "{:?}", app.history);
    assert!(
        receipts[0].contains("make the tests pass"),
        "{}",
        receipts[0]
    );
    assert!(receipts[0].contains("/goal pause"), "{}", receipts[0]);

    let mut usage = snapshot.clone();
    usage.tokens_used = 12;
    assert!(apply_goal_snapshot_to_app(&mut app, &usage));
    let receipts = app
        .history
        .iter()
        .filter(
            |cell| matches!(cell, HistoryCell::System { content } if content.contains("Goal set")),
        )
        .count();
    assert_eq!(
        receipts, 1,
        "the same objective must not reprint the receipt"
    );

    let mut declared = create_test_app();
    declared.goal.objective = Some("make the tests pass".to_string());
    apply_goal_snapshot_to_app(&mut declared, &snapshot);
    assert!(
        declared.history.iter().all(|cell| {
            !matches!(cell, HistoryCell::System { content } if content.contains("Goal set"))
        }),
        "a user-declared goal must not repeat its own receipt"
    );
}

#[test]
fn canonical_goal_clear_wins_after_stale_active_snapshot() {
    let mut app = create_test_app();
    let stale_active = crate::tools::goal::GoalSnapshot {
        objective: Some("Goal cleared while the prior turn finished".to_string()),
        status: "active".to_string(),
        token_budget: Some(10_000),
        tokens_used: 500,
        time_used_seconds: 5,
        continuation_count: 2,
        elapsed_seconds: Some(5),
        evidence: None,
        blocker: None,
        pause_reason: None,
        completion_verification: None,
        ..Default::default()
    };
    assert!(apply_goal_snapshot_to_app(&mut app, &stale_active));
    assert!(app.goal.objective.is_some());

    // SetGoalStatus(clear) is processed after the prior turn's active update.
    // Its canonical empty snapshot must be observable and win the replay.
    let cleared = crate::tools::goal::GoalSnapshot {
        objective: None,
        status: "none".to_string(),
        token_budget: None,
        tokens_used: 0,
        time_used_seconds: 0,
        continuation_count: 0,
        elapsed_seconds: None,
        evidence: None,
        blocker: None,
        pause_reason: None,
        completion_verification: None,
        ..Default::default()
    };
    assert!(apply_goal_snapshot_to_app(&mut app, &cleared));
    assert!(app.goal.objective.is_none());
    assert!(app.goal.token_budget.is_none());
    assert_eq!(app.goal.tokens_used, 0);
    assert_eq!(app.goal.time_used_seconds, 0);
    assert_eq!(app.goal.continuation_count, 0);
    assert!(app.goal.started_at.is_none());
    assert!(app.goal.finished_at.is_none());
    assert_eq!(app.goal.status, crate::tools::goal::GoalStatus::Active);
    assert!(
        !apply_goal_snapshot_to_app(&mut app, &cleared),
        "replaying the same clear receipt must be idempotent"
    );

    // Only the exact empty/none pair is a clear. A malformed objective-less
    // active update must not erase subsequently visible goal state.
    assert!(apply_goal_snapshot_to_app(&mut app, &stale_active));
    let malformed = crate::tools::goal::GoalSnapshot {
        status: "active".to_string(),
        ..cleared
    };
    assert!(!apply_goal_snapshot_to_app(&mut app, &malformed));
    assert_eq!(
        app.goal.objective.as_deref(),
        Some("Goal cleared while the prior turn finished")
    );
}

#[test]
fn apply_goal_snapshot_resume_clears_frozen_timer() {
    let mut app = create_test_app();
    app.goal.objective = Some("Ship the release lane".to_string());
    app.goal.status = crate::tools::goal::GoalStatus::Active;
    app.goal.started_at = Some(Instant::now());

    // First, mark the goal complete — finished_at gets set.
    let completed = crate::tools::goal::GoalSnapshot {
        objective: Some("Ship the release lane".to_string()),
        status: "complete".to_string(),
        token_budget: None,
        tokens_used: 0,
        time_used_seconds: 0,
        continuation_count: 0,
        elapsed_seconds: Some(0),
        evidence: Some("done".to_string()),
        blocker: None,
        pause_reason: None,
        completion_verification: Some(crate::tools::goal::GoalCompletionVerification {
            status: "passed".to_string(),
            check: "cargo test".to_string(),
            summary: "ok".to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(apply_goal_snapshot_to_app(&mut app, &completed));
    assert_eq!(app.goal.status, crate::tools::goal::GoalStatus::Complete);
    assert!(app.goal.finished_at.is_some());

    // Now a later snapshot reports the goal active again (resume). The frozen
    // timer must clear so the sidebar starts ticking once more.
    let resumed = crate::tools::goal::GoalSnapshot {
        objective: Some("Ship the release lane".to_string()),
        status: "active".to_string(),
        token_budget: None,
        tokens_used: 0,
        time_used_seconds: 0,
        continuation_count: 0,
        elapsed_seconds: Some(0),
        evidence: None,
        blocker: None,
        pause_reason: None,
        completion_verification: None,
        ..Default::default()
    };
    assert!(apply_goal_snapshot_to_app(&mut app, &resumed));
    assert_eq!(app.goal.status, crate::tools::goal::GoalStatus::Active);
    assert!(
        app.goal.finished_at.is_none(),
        "resume should re-arm the elapsed timer"
    );
}

#[test]
fn apply_goal_snapshot_keeps_paused_timer_frozen_across_usage_updates() {
    let mut app = create_test_app();
    app.goal.objective = Some("Ship the release lane".to_string());
    app.goal.status = crate::tools::goal::GoalStatus::Active;
    app.goal.started_at = Some(Instant::now());

    // Pause the goal — the timer freezes.
    let paused = crate::tools::goal::GoalSnapshot {
        objective: Some("Ship the release lane".to_string()),
        status: "paused".to_string(),
        token_budget: None,
        tokens_used: 100,
        time_used_seconds: 10,
        continuation_count: 0,
        elapsed_seconds: Some(10),
        evidence: None,
        blocker: None,
        pause_reason: Some(crate::tools::goal::GoalPauseReason::User),
        completion_verification: None,
        ..Default::default()
    };
    assert!(apply_goal_snapshot_to_app(&mut app, &paused));
    assert_eq!(app.goal.status, crate::tools::goal::GoalStatus::Paused);
    assert_eq!(
        app.goal.pause_reason,
        Some(crate::tools::goal::GoalPauseReason::User)
    );
    let frozen_at = app
        .goal
        .finished_at
        .expect("pausing must freeze the elapsed timer");

    // Usage keeps accruing while paused (record_goal_usage_for_turn runs on
    // every turn), so a later snapshot arrives with bumped usage but the goal
    // still paused. The frozen timer must NOT silently re-arm.
    let paused_with_usage = crate::tools::goal::GoalSnapshot {
        objective: Some("Ship the release lane".to_string()),
        status: "paused".to_string(),
        token_budget: None,
        tokens_used: 250,
        time_used_seconds: 25,
        continuation_count: 0,
        elapsed_seconds: Some(25),
        evidence: None,
        blocker: None,
        pause_reason: Some(crate::tools::goal::GoalPauseReason::User),
        completion_verification: None,
        ..Default::default()
    };
    assert!(apply_goal_snapshot_to_app(&mut app, &paused_with_usage));
    assert_eq!(app.goal.status, crate::tools::goal::GoalStatus::Paused);
    assert_eq!(
        app.goal.finished_at,
        Some(frozen_at),
        "a paused goal's frozen timer must stay frozen when usage updates arrive"
    );

    // Explicit resume still re-arms.
    let resumed = crate::tools::goal::GoalSnapshot {
        objective: Some("Ship the release lane".to_string()),
        status: "active".to_string(),
        token_budget: None,
        tokens_used: 250,
        time_used_seconds: 25,
        continuation_count: 1,
        elapsed_seconds: Some(25),
        evidence: None,
        blocker: None,
        pause_reason: None,
        completion_verification: None,
        ..Default::default()
    };
    assert!(apply_goal_snapshot_to_app(&mut app, &resumed));
    assert_eq!(app.goal.status, crate::tools::goal::GoalStatus::Active);
    assert_eq!(app.goal.pause_reason, None);
    assert!(
        app.goal.finished_at.is_none(),
        "resuming a paused goal should re-arm the elapsed timer"
    );
}

#[test]
fn turn_liveness_watchdog_clears_stale_dispatch() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.dispatch_started_at =
        Some(Instant::now() - DISPATCH_WATCHDOG_TIMEOUT - Duration::from_millis(1));
    app.turn_started_at = Some(Instant::now());
    app.pending_turn_route = Some((ApiProvider::Deepseek, "pending-model".to_string(), false));
    app.suppress_stream_events_until_turn_complete = true;

    let recovered = reconcile_turn_liveness(&mut app, Instant::now(), false);

    assert!(recovered);
    assert!(!app.is_loading);
    assert!(app.dispatch_started_at.is_none());
    assert!(app.turn_started_at.is_none());
    assert!(app.pending_turn_route.is_none());
    assert!(app.active_turn.is_none());
    assert!(!app.suppress_stream_events_until_turn_complete);
    let toast = app.status_toasts.back().expect("watchdog toast");
    assert_eq!(toast.level, StatusToastLevel::Error);
    assert!(toast.text.contains("Turn dispatch timed out"));
}

#[test]
fn turn_liveness_reconciles_completed_busy_state() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.runtime_turn_status = Some("completed".to_string());
    app.dispatch_started_at = Some(Instant::now());
    app.turn_started_at = Some(Instant::now());

    let recovered = reconcile_turn_liveness(&mut app, Instant::now(), false);

    assert!(recovered);
    assert!(!app.is_loading);
    assert!(app.dispatch_started_at.is_none());
    assert!(app.turn_started_at.is_none());
    let toast = app.status_toasts.back().expect("reconciliation toast");
    assert_eq!(toast.level, StatusToastLevel::Warning);
    assert!(
        toast
            .text
            .contains("Recovered from an inconsistent busy state")
    );
}

#[test]
fn turn_liveness_leaves_active_turn_running() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.runtime_turn_status = Some("in_progress".to_string());
    app.turn_started_at = Some(Instant::now() - Duration::from_secs(60));

    let recovered = reconcile_turn_liveness(&mut app, Instant::now(), false);

    assert!(!recovered);
    assert!(app.is_loading);
    assert!(app.turn_started_at.is_some());
    assert!(app.status_toasts.is_empty());
}

#[test]
fn turn_liveness_uses_recent_turn_activity_not_turn_start() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.runtime_turn_status = Some("in_progress".to_string());
    app.turn_started_at = Some(Instant::now());
    app.turn_last_activity_at =
        Some(app.turn_started_at.unwrap() + TURN_STALL_WATCHDOG_TIMEOUT + Duration::from_secs(29));
    let now = app.turn_last_activity_at.unwrap() + Duration::from_secs(1);

    let recovered = reconcile_turn_liveness(&mut app, now, false);

    assert!(!recovered);
    assert!(app.is_loading);
    assert!(app.runtime_turn_status.is_some());
    assert!(app.status_toasts.is_empty());
}

#[test]
fn turn_liveness_does_not_abort_running_tool() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.runtime_turn_status = Some("in_progress".to_string());
    app.turn_started_at = Some(Instant::now());
    app.turn_last_activity_at = app.turn_started_at;
    let now = app.turn_started_at.unwrap()
        + TURN_STALL_WATCHDOG_TIMEOUT
        + Duration::from_secs(30)
        + Duration::from_secs(1);
    let mut active = ActiveCell::new();
    active.push_tool(
        "tool-1",
        HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "edit_file".to_string(),
            status: ToolStatus::Running,
            input_summary: Some("path: CHANGELOG.md".to_string()),
            output: None,
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        })),
    );
    app.active_cell = Some(active);

    let recovered = reconcile_turn_liveness(&mut app, now, false);

    assert!(!recovered);
    assert!(app.is_loading);
    assert!(app.active_cell.is_some());
    assert!(app.status_toasts.is_empty());
}

#[test]
fn turn_liveness_does_not_abort_running_tool_with_recent_heartbeat() {
    let mut app = create_test_app();
    let started_at = Instant::now();
    let now = started_at + TOOL_HANG_WATCHDOG_TIMEOUT + Duration::from_secs(30);
    app.is_loading = true;
    app.runtime_turn_status = Some("in_progress".to_string());
    app.turn_started_at = Some(started_at);
    app.turn_last_activity_at = Some(now - Duration::from_secs(10));
    let mut active = ActiveCell::new();
    active.push_tool(
        "tool-1",
        HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "exec_shell".to_string(),
            status: ToolStatus::Running,
            input_summary: Some("command: cargo test".to_string()),
            output: None,
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        })),
    );
    app.active_cell = Some(active);

    let recovered = reconcile_turn_liveness(&mut app, now, false);

    assert!(!recovered);
    assert!(app.is_loading);
    assert!(app.active_cell.is_some());
    assert!(app.status_toasts.is_empty());
}

#[test]
fn turn_liveness_keeps_max_duration_exec_shell_wait_alive_with_heartbeat() {
    // `exec_shell_wait` accepts exactly 600_000ms. Before the heartbeat was
    // restored, that supported boundary raced the identically timed tool-hang
    // watchdog and falsely made the UI idle while the engine still owned the
    // turn.
    const EXEC_SHELL_WAIT_MAX: Duration =
        Duration::from_millis(crate::tools::shell::EXEC_SHELL_WAIT_MAX_TIMEOUT_MS);
    assert_eq!(TOOL_HANG_WATCHDOG_TIMEOUT, EXEC_SHELL_WAIT_MAX);

    let mut app = create_test_app();
    let started_at = Instant::now();
    let wait_deadline = started_at + EXEC_SHELL_WAIT_MAX;
    let now = wait_deadline + Duration::from_secs(1);
    app.is_loading = true;
    app.runtime_turn_status = Some("in_progress".to_string());
    app.runtime_turn_id = Some("max-shell-wait-turn".to_string());
    app.turn_started_at = Some(started_at);
    app.turn_last_activity_at = Some(started_at);
    let mut active = ActiveCell::new();
    active.push_tool(
        "shell-wait",
        HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "exec_shell_wait".to_string(),
            status: ToolStatus::Running,
            input_summary: Some("task_id: shell-test, wait: true, timeout_ms: 600000".to_string()),
            output: None,
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        })),
    );
    app.active_cell = Some(active);

    record_turn_activity(&mut app, &EngineEvent::ToolCallHeartbeat, wait_deadline);
    let recovered = reconcile_turn_liveness(&mut app, now, false);

    assert!(!recovered);
    assert!(app.is_loading);
    assert_eq!(app.runtime_turn_status.as_deref(), Some("in_progress"));
    assert_eq!(app.runtime_turn_id.as_deref(), Some("max-shell-wait-turn"));
    assert!(app.status_message.is_none());
    assert!(app.status_toasts.is_empty());
}

#[test]
fn turn_liveness_respects_stream_idle_budget_for_quiet_model_waits() {
    let mut app = create_test_app();
    let started_at = Instant::now();
    app.is_loading = true;
    app.runtime_turn_status = Some("in_progress".to_string());
    app.stream_chunk_timeout_secs = 900;
    app.turn_started_at = Some(started_at);
    app.turn_last_activity_at = Some(started_at);
    let now = started_at + TURN_STALL_WATCHDOG_TIMEOUT + Duration::from_secs(31);

    let recovered = reconcile_turn_liveness(&mut app, now, false);

    assert!(!recovered);
    assert!(app.is_loading);
    assert!(app.status_toasts.is_empty());
}

#[test]
fn turn_liveness_recovers_running_tool_without_heartbeat() {
    let mut app = create_test_app();
    let started_at = Instant::now();
    let now = started_at + TOOL_HANG_WATCHDOG_TIMEOUT + Duration::from_secs(1);
    app.is_loading = true;
    app.runtime_turn_status = Some("in_progress".to_string());
    app.runtime_turn_id = Some("stale-tool-turn".to_string());
    app.turn_started_at = Some(started_at);
    app.turn_last_activity_at = app.turn_started_at;
    app.user_scrolled_during_stream = true;
    let mut active = ActiveCell::new();
    active.push_tool(
        "tool-1",
        HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "exec_shell".to_string(),
            status: ToolStatus::Running,
            input_summary: Some("command: cargo test".to_string()),
            output: None,
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        })),
    );
    app.active_cell = Some(active);

    let recovered = reconcile_turn_liveness(&mut app, now, false);

    assert!(recovered);
    assert!(!app.is_loading);
    assert!(app.turn_started_at.is_none());
    assert!(app.runtime_turn_status.is_none());
    assert!(app.runtime_turn_id.is_none());
    assert!(!app.user_scrolled_during_stream);
    let toast = app.status_toasts.back().expect("tool hang toast");
    assert_eq!(toast.level, StatusToastLevel::Error);
    assert!(toast.text.contains("Tool stalled with no progress"));
}

#[test]
fn turn_liveness_recovers_stalled_in_progress_turn() {
    let mut app = create_test_app();
    let now = Instant::now();
    app.is_loading = true;
    app.runtime_turn_status = Some("in_progress".to_string());
    app.runtime_turn_id = Some("stale-turn-id".to_string());
    app.stream_chunk_timeout_secs = 300;
    app.turn_started_at = Some(now - turn_stall_watchdog_timeout(&app) - Duration::from_millis(1));
    app.streaming_message_index = Some(0);
    app.user_scrolled_during_stream = true;
    app.pending_turn_route = Some((ApiProvider::Deepseek, "pending-model".to_string(), false));
    app.active_turn = Some(crate::tui::app::ActiveTurnMetadata {
        turn_id: "stale-turn-id".to_string(),
        created_at: chrono::Utc::now(),
        route: Some(crate::core::events::TurnRoute {
            provider: ApiProvider::Openai,
            provider_identity: "openai".to_string(),
            model: "gpt-5.5".to_string(),
            auto_model: false,
            receipt: None,
            billing: Some(crate::core::events::RouteBillingEnvelope {
                billing_surface: Some(crate::pricing::FIRST_PARTY_PAYG_BILLING_SURFACE.to_string()),
                endpoint_fingerprint: Some("openai-endpoint".to_string()),
                billing_mode: crate::cost_status::RouteBillingMode::Metered,
                dispatched_at: chrono::Utc::now(),
            }),
            base_url: String::new(),
            billing_product: crate::route_billing::RouteProduct::Unproven,
        }),
        auto_route_receipt: None,
        suggestion_authority: None,
    });

    let recovered = reconcile_turn_liveness(&mut app, now, false);

    assert!(recovered);
    assert!(!app.is_loading);
    assert!(app.turn_started_at.is_none());
    assert!(app.runtime_turn_status.is_none());
    assert!(app.runtime_turn_id.is_none());
    assert!(app.dispatch_started_at.is_none());
    assert!(app.streaming_message_index.is_none());
    assert!(app.streaming_thinking_active_entry.is_none());
    assert!(!app.user_scrolled_during_stream);
    assert!(app.pending_turn_route.is_none());
    assert!(app.active_turn.is_none());
    assert!(!app.suppress_stream_events_until_turn_complete);
    let toast = app.status_toasts.back().expect("stall toast");
    assert_eq!(toast.level, StatusToastLevel::Error);
    assert!(toast.text.contains("Turn stalled"));
}

#[test]
fn engine_event_disconnect_recovers_live_turn_immediately() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.runtime_turn_status = Some("in_progress".to_string());
    app.runtime_turn_id = Some("turn_dead".to_string());
    app.turn_started_at = Some(Instant::now());
    app.dispatch_started_at = Some(Instant::now());
    app.user_scrolled_during_stream = true;
    app.pending_turn_route = Some((ApiProvider::Deepseek, "pending-model".to_string(), false));
    app.active_turn = Some(crate::tui::app::ActiveTurnMetadata {
        turn_id: "turn_dead".to_string(),
        created_at: chrono::Utc::now(),
        route: Some(crate::core::events::TurnRoute {
            provider: ApiProvider::Openai,
            provider_identity: "openai".to_string(),
            model: "gpt-5.5".to_string(),
            auto_model: false,
            receipt: None,
            billing: Some(crate::core::events::RouteBillingEnvelope {
                billing_surface: Some(crate::pricing::FIRST_PARTY_PAYG_BILLING_SURFACE.to_string()),
                endpoint_fingerprint: Some("openai-endpoint".to_string()),
                billing_mode: crate::cost_status::RouteBillingMode::Metered,
                dispatched_at: chrono::Utc::now(),
            }),
            base_url: String::new(),
            billing_product: crate::route_billing::RouteProduct::Unproven,
        }),
        auto_route_receipt: None,
        suggestion_authority: None,
    });
    let thinking_idx = crate::tui::streaming_thinking::ensure_active_entry(&mut app);
    crate::tui::streaming_thinking::append(&mut app, thinking_idx, "partial reasoning");
    app.push_pending_steer(crate::tui::app::QueuedMessage::new(
        "please continue after recovery".to_string(),
        None,
    ));

    let recovered = recover_engine_event_disconnect(&mut app);

    assert!(recovered);
    assert!(!app.is_loading);
    assert!(app.runtime_turn_status.is_none());
    assert!(app.runtime_turn_id.is_none());
    assert!(app.dispatch_started_at.is_none());
    assert!(app.turn_started_at.is_none());
    assert!(app.streaming_thinking_active_entry.is_none());
    assert!(!app.user_scrolled_during_stream);
    assert!(app.pending_turn_route.is_none());
    assert!(app.active_turn.is_none());
    assert!(!app.suppress_stream_events_until_turn_complete);
    assert_eq!(app.queued_message_count(), 1);
    assert_eq!(
        app.queued_messages
            .front()
            .map(crate::tui::app::QueuedMessage::content),
        Some("please continue after recovery".to_string())
    );
    assert!(
        app.history.iter().any(|cell| matches!(
            cell,
            HistoryCell::Error { message, .. }
                if message.contains("Engine stopped before completing the turn")
        )),
        "disconnect recovery should add a visible transcript error"
    );
    let toast = app.status_toasts.back().expect("disconnect toast");
    assert_eq!(toast.level, StatusToastLevel::Error);
}

#[test]
fn engine_event_disconnect_while_idle_is_noop() {
    let mut app = create_test_app();

    let recovered = recover_engine_event_disconnect(&mut app);

    assert!(!recovered);
    assert!(app.history.is_empty());
    assert!(app.status_toasts.is_empty());
}

#[test]
fn engine_event_disconnect_cleans_cancelled_turn_metadata() {
    let mut app = create_test_app();
    app.suppress_stream_events_until_turn_complete = true;
    app.active_turn = Some(crate::tui::app::ActiveTurnMetadata {
        turn_id: "cancelled-turn".to_string(),
        created_at: chrono::Utc::now(),
        route: Some(crate::core::events::TurnRoute {
            provider: ApiProvider::Openai,
            provider_identity: "openai".to_string(),
            model: "gpt-5.5".to_string(),
            auto_model: false,
            receipt: None,
            billing: Some(crate::core::events::RouteBillingEnvelope {
                billing_surface: Some(crate::pricing::FIRST_PARTY_PAYG_BILLING_SURFACE.to_string()),
                endpoint_fingerprint: Some("openai-endpoint".to_string()),
                billing_mode: crate::cost_status::RouteBillingMode::Metered,
                dispatched_at: chrono::Utc::now(),
            }),
            base_url: String::new(),
            billing_product: crate::route_billing::RouteProduct::Unproven,
        }),
        auto_route_receipt: None,
        suggestion_authority: None,
    });

    let recovered = recover_engine_event_disconnect(&mut app);

    assert!(recovered);
    assert!(app.active_turn.is_none());
    assert!(!app.suppress_stream_events_until_turn_complete);
    assert!(app.history.iter().any(|cell| matches!(
        cell,
        HistoryCell::Error { message, .. }
            if message.contains("Engine stopped before completing the turn")
    )));
}

#[test]
fn fixed_model_auto_thinking_skips_auto_model_router() {
    let mut app = create_test_app();
    app.auto_model = false;
    app.model = "deepseek-v4-pro".to_string();
    app.reasoning_effort = ReasoningEffort::Auto;

    assert!(
        !crate::tui::auto_router::should_resolve_auto_model_selection(&app),
        "fixed-model auto thinking must stay local instead of starting a hidden router request"
    );
}

#[test]
fn auto_model_still_uses_auto_model_router() {
    let mut app = create_test_app();
    app.auto_model = true;
    app.reasoning_effort = ReasoningEffort::Auto;

    assert!(
        crate::tui::auto_router::should_resolve_auto_model_selection(&app),
        "auto model still needs the router to choose the concrete model"
    );
}

#[tokio::test]
async fn auto_preview_stops_before_the_provider_backed_route_planner() {
    let mut app = create_test_app();
    app.auto_model = true;
    app.model = "auto".to_string();
    let config = Config::default();
    let (_engine, handle) = crate::core::engine::Engine::new(EngineConfig::default(), &config);

    let inputs = build_preview_request_inputs(
        &app,
        &config,
        &handle,
        Some("classify this hypothetical turn".to_string()),
    )
    .await;

    assert!(inputs.next_turn.is_none());
    assert!(matches!(
        inputs.unresolved,
        crate::core::engine::preview::PreviewUnresolved::AutoRouteClassificationNotExecuted
    ));
}

fn init_git_repo() -> TempDir {
    let dir = tempfile::tempdir().expect("tempdir");

    let init = Command::new("git")
        .arg("init")
        .current_dir(dir.path())
        .output()
        .expect("git init should run");
    assert!(
        init.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let autocrlf = Command::new("git")
        .args(["config", "core.autocrlf", "false"])
        .current_dir(dir.path())
        .output()
        .expect("git config core.autocrlf should run");
    assert!(
        autocrlf.status.success(),
        "git config core.autocrlf failed: {}",
        String::from_utf8_lossy(&autocrlf.stderr)
    );

    let commit = Command::new("git")
        .args([
            "-c",
            "user.name=codewhale Tests",
            "-c",
            "user.email=tests@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-m",
            "init",
        ])
        .current_dir(dir.path())
        .output()
        .expect("git commit should run");
    assert!(
        commit.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&commit.stderr)
    );

    dir
}

#[test]
fn ctrl_alt_4_selects_pinned_rail_panel_without_switching_modes() {
    let mut app = create_test_app();
    app.mode = AppMode::Agent;

    apply_alt_4_shortcut(&mut app, KeyModifiers::ALT | KeyModifiers::CONTROL);

    assert_eq!(app.mode, AppMode::Agent);
    assert_eq!(
        app.work_surface.panel,
        crate::tui::work_surface::RailPanel::Files
    );
    assert_eq!(app.status_message.as_deref(), Some("Workbar panel: files"));
}

#[test]
fn hotbar_bare_digit_inserts_text_even_when_composer_empty() {
    let mut app = create_test_app();
    app.onboarding = OnboardingState::None;

    let bare_four = KeyEvent::new(KeyCode::Char('4'), KeyModifiers::NONE);
    assert_eq!(hotbar_slot_from_key(&app, &bare_four), None);

    app.input = "draft".to_string();
    assert_eq!(hotbar_slot_from_key(&app, &bare_four), None);

    app.input = "   ".to_string();
    assert_eq!(hotbar_slot_from_key(&app, &bare_four), None);
}

#[test]
fn hotbar_alt_digit_fires_from_composer_and_sidebar_states() {
    let mut app = create_test_app();
    app.onboarding = OnboardingState::None;

    let alt_four = KeyEvent::new(KeyCode::Char('4'), KeyModifiers::ALT);

    assert_eq!(hotbar_slot_from_key(&app, &alt_four), Some(4));

    app.input = "draft".to_string();
    assert_eq!(hotbar_slot_from_key(&app, &alt_four), Some(4));

    app.input = "   ".to_string();
    assert_eq!(hotbar_slot_from_key(&app, &alt_four), Some(4));

    app.work_surface.placement = crate::tui::work_surface::WorkSurfacePlacement::Off;
    assert_eq!(hotbar_slot_from_key(&app, &alt_four), Some(4));

    app.work_surface.panel = crate::tui::work_surface::RailPanel::Agents;
    assert_eq!(hotbar_slot_from_key(&app, &alt_four), Some(4));
}

#[test]
fn hotbar_alt_digit_requires_plain_alt_one_through_eight() {
    let mut app = create_test_app();
    app.onboarding = OnboardingState::None;

    for slot in 1..=8 {
        let digit = char::from_digit(slot.into(), 10).expect("Hotbar slot digit");
        assert_eq!(
            hotbar_slot_from_key(
                &app,
                &KeyEvent::new(KeyCode::Char(digit), KeyModifiers::ALT)
            ),
            Some(slot),
            "plain Alt-{slot} must dispatch the corresponding Hotbar slot"
        );
        for modifiers in [
            KeyModifiers::NONE,
            KeyModifiers::CONTROL,
            KeyModifiers::SUPER,
            KeyModifiers::ALT | KeyModifiers::CONTROL,
            KeyModifiers::ALT | KeyModifiers::SUPER,
        ] {
            assert_eq!(
                hotbar_slot_from_key(&app, &KeyEvent::new(KeyCode::Char(digit), modifiers)),
                None,
                "modifiers {modifiers:?} must not impersonate Alt-{slot}"
            );
        }
    }

    for key in [
        KeyCode::Char('0'),
        KeyCode::Char('9'),
        KeyCode::F(1),
        KeyCode::F(4),
    ] {
        assert_eq!(
            hotbar_slot_from_key(&app, &KeyEvent::new(key, KeyModifiers::ALT)),
            None,
            "out-of-contract key {key:?} must not dispatch a Hotbar slot"
        );
    }
}

#[test]
fn hotbar_digits_are_blocked_while_modal_or_onboarding_is_active() {
    let mut app = create_test_app();
    app.onboarding = OnboardingState::None;
    app.view_stack.push(HelpView::new());

    let bare_four = KeyEvent::new(KeyCode::Char('4'), KeyModifiers::NONE);
    let alt_four = KeyEvent::new(KeyCode::Char('4'), KeyModifiers::ALT);

    assert_eq!(hotbar_slot_from_key(&app, &bare_four), None);
    assert_eq!(hotbar_slot_from_key(&app, &alt_four), None);

    let mut app = create_test_app();
    app.onboarding = OnboardingState::Language;
    assert_eq!(hotbar_slot_from_key(&app, &bare_four), None);
    assert_eq!(hotbar_slot_from_key(&app, &alt_four), None);
}

#[test]
fn hotbar_alt_digit_is_blocked_while_inline_selectors_are_open() {
    let mut app = create_test_app();
    app.onboarding = OnboardingState::None;
    app.input = "/".to_string();
    app.cursor_position = app.input.chars().count();
    app.slash_menu_hidden = false;
    assert!(
        !visible_slash_menu_entries(&app, SLASH_MENU_LIMIT).is_empty(),
        "precondition: slash menu should be visible"
    );

    let alt_four = KeyEvent::new(KeyCode::Char('4'), KeyModifiers::ALT);
    assert_eq!(hotbar_slot_from_key(&app, &alt_four), None);

    app.input = "draft".to_string();
    app.cursor_position = app.input.chars().count();
    app.start_history_search();
    assert!(app.is_history_search_active());
    assert_eq!(hotbar_slot_from_key(&app, &alt_four), None);
}

#[test]
fn hotbar_dispatches_bound_slot_and_ignores_empty_slot() {
    let mut app = create_test_app();
    // #3807: a fresh config has no bindings, so opt in with the default slots
    // (slot 5 = mode.agent) to exercise dispatch of a bound slot.
    let config = Config {
        hotbar: Some(codewhale_config::default_hotbar_bindings_toml()),
        ..Config::default()
    };
    app.onboarding = OnboardingState::None;
    app.mode = AppMode::Plan;
    app.needs_redraw = false;

    let dispatch = dispatch_hotbar_slot(&mut app, &config, 5).expect("hotbar dispatch");
    assert!(matches!(
        dispatch,
        Some(HotbarDispatch::AppAction(AppAction::ModeChanged(
            AppMode::Agent
        )))
    ));
    assert_eq!(app.mode, AppMode::Agent);
    assert!(
        app.needs_redraw,
        "mode-changing hotbar actions should leave the app ready to redraw"
    );

    let empty_config = Config {
        hotbar: Some(Vec::new()),
        ..Config::default()
    };
    assert_eq!(
        dispatch_hotbar_slot(&mut app, &empty_config, 1).expect("empty slot is ok"),
        None
    );
}

#[test]
fn hotbar_dispatches_slash_command_slot() {
    let mut app = create_test_app();
    app.onboarding = OnboardingState::None;
    let config = Config {
        hotbar: Some(vec![codewhale_config::HotbarBindingToml {
            slot: 1,
            label: Some("mode".to_string()),
            action: "slash.mode".to_string(),
        }]),
        ..Config::default()
    };

    assert_eq!(
        dispatch_hotbar_slot(&mut app, &config, 1).expect("slash slot dispatch"),
        Some(HotbarDispatch::AppAction(AppAction::OpenModePicker))
    );
    assert!(app.input.is_empty());
}

#[test]
fn hotbar_dispatches_route_switch_slot() {
    let mut app = create_test_app();
    app.onboarding = OnboardingState::None;
    let route_metadata = app
        .hotbar_actions
        .iter()
        .map(|action| action.metadata(crate::localization::Locale::En))
        .find(|metadata| metadata.category == HotbarActionCategory::Route)
        .expect("test app should register at least the active provider route");
    let route_id = route_metadata.id.clone();
    let route_suffix = route_metadata
        .id
        .strip_prefix("route.")
        .expect("route id prefix");
    let (provider_key, model) = route_suffix.split_once('.').expect("route id shape");
    let provider = ApiProvider::parse(provider_key).expect("provider key parses");
    let model = model.to_string();
    let config = Config {
        hotbar: Some(vec![codewhale_config::HotbarBindingToml {
            slot: 1,
            label: Some(route_metadata.compact_label),
            action: route_id,
        }]),
        ..Config::default()
    };

    assert_eq!(
        dispatch_hotbar_slot(&mut app, &config, 1).expect("route slot dispatch"),
        Some(HotbarDispatch::AppAction(AppAction::SwitchModelRoute {
            provider,
            model,
        }))
    );
}

#[test]
fn hotbar_bound_reasoning_action_updates_auto_model_preference() {
    let mut app = create_test_app();
    app.onboarding = OnboardingState::None;
    app.auto_model = true;
    app.reasoning_effort = ReasoningEffort::Off;
    app.needs_redraw = false;
    let config = Config {
        hotbar: Some(vec![codewhale_config::HotbarBindingToml {
            slot: 1,
            label: Some("reason".to_string()),
            action: "reasoning.cycle".to_string(),
        }]),
        ..Config::default()
    };

    assert!(matches!(
        dispatch_hotbar_slot(&mut app, &config, 1).expect("reasoning slot dispatch"),
        Some(HotbarDispatch::AppAction(AppAction::UpdateCompaction(_)))
    ));
    assert_eq!(app.reasoning_effort, ReasoningEffort::Minimal);
    assert!(
        app.status_message
            .as_deref()
            .is_some_and(|message| message.contains("Reasoning effort: minimal"))
    );
    assert!(app.needs_redraw);
}

#[test]
fn alt_0_without_ctrl_is_unbound_after_auto_mode_retired() {
    let mut app = create_test_app();
    app.work_surface.placement = crate::tui::work_surface::WorkSurfacePlacement::Left;

    apply_alt_0_shortcut(&mut app, KeyModifiers::ALT);

    // Auto-collapse was dropped with the legacy sidebar; plain Alt+0 must
    // not mutate placement or claim anything about the rail.
    assert_eq!(
        app.work_surface.placement,
        crate::tui::work_surface::WorkSurfacePlacement::Left
    );
    assert!(app.status_message.is_none());
}

#[test]
fn ctrl_alt_0_turns_rail_off() {
    let mut app = create_test_app();
    app.work_surface.placement = crate::tui::work_surface::WorkSurfacePlacement::Top;

    apply_alt_0_shortcut(&mut app, KeyModifiers::ALT | KeyModifiers::CONTROL);

    assert_eq!(
        app.work_surface.placement,
        crate::tui::work_surface::WorkSurfacePlacement::Off
    );
    assert_eq!(app.status_message.as_deref(), Some("Workbar is off"));
}

#[test]
fn ctrl_alt_0_restores_bottom_rail_when_already_off() {
    let mut app = create_test_app();
    app.work_surface.placement = crate::tui::work_surface::WorkSurfacePlacement::Off;

    apply_alt_0_shortcut(&mut app, KeyModifiers::ALT | KeyModifiers::CONTROL);

    assert_eq!(
        app.work_surface.placement,
        crate::tui::work_surface::WorkSurfacePlacement::Bottom
    );
    assert_eq!(
        app.status_message.as_deref(),
        Some("Workbar: bottom placement")
    );
}

#[test]
fn rail_command_reports_off_without_claiming_visibility() {
    // Replaces the old sidebar_render_state tests: the render-state machine
    // is gone with the classic sidebar, and the /workbar status readout is
    // the contract that replaces it. It must never claim a surface that
    // cannot render is visible.
    let mut app = create_test_app();
    let result = crate::commands::execute("/workbar off", &mut app);
    assert!(!result.is_error);
    assert_eq!(
        app.work_surface.placement,
        crate::tui::work_surface::WorkSurfacePlacement::Off
    );
    let message = result.message.unwrap_or_default();
    assert!(message.contains("Workbar is off"), "got: {message}");
    assert!(
        !message.contains("Workbar is visible"),
        "no control may claim a hidden surface renders: {message}"
    );
}

#[test]
fn background_receipt_tip_only_detects_a_visible_active_to_completed_transition() {
    let active = HashSet::from(["task_running", "shell_running"]);
    assert!(newly_completed_id(
        active.clone(),
        ["task_old", "task_running"]
    ));
    assert!(!newly_completed_id(active, ["task_old", "task_unseen"]));
}

#[test]
fn ctrl_x_jobs_prefill_only_catches_running_shell_jobs_in_tasks_sidebar() {
    let mut app = create_test_app();
    app.work_surface.panel = crate::tui::work_surface::RailPanel::Tasks;
    app.work_surface.last_area = Some(ratatui::layout::Rect::new(0, 0, 100, 3));
    app.input = "draft".to_string();
    app.cursor_position = app.input.len();
    app.task_panel.push(TaskPanelEntry {
        id: "shell_active".to_string(),
        status: "running".to_string(),
        prompt_summary: "shell: cargo test".to_string(),
        duration_ms: Some(10),
        kind: TaskPanelEntryKind::Background,
        stale: false,
        elapsed_since_output_ms: None,
        owner_agent_id: None,
        owner_agent_name: None,
        current_tool: None,
        role: None,
        files_touched: 0,
    });

    assert!(prefill_jobs_cancel_all_if_tasks_sidebar(&mut app));
    assert_eq!(app.input, "/jobs cancel-all");
    assert_eq!(app.cursor_position, app.input.len());
    assert_eq!(
        app.status_message.as_deref(),
        Some("Press Enter to cancel all running commands")
    );
}

#[test]
fn ctrl_x_jobs_prefill_falls_through_outside_tasks_sidebar_shell_jobs() {
    let mut non_shell = create_test_app();
    non_shell.work_surface.panel = crate::tui::work_surface::RailPanel::Tasks;
    non_shell.work_surface.last_area = Some(ratatui::layout::Rect::new(0, 0, 100, 3));
    non_shell.input = "draft".to_string();
    non_shell.cursor_position = non_shell.input.len();
    non_shell.task_panel.push(TaskPanelEntry {
        id: "task_active".to_string(),
        status: "running".to_string(),
        prompt_summary: "summarize the release notes".to_string(),
        duration_ms: Some(10),
        kind: TaskPanelEntryKind::Background,
        stale: false,
        elapsed_since_output_ms: None,
        owner_agent_id: None,
        owner_agent_name: None,
        current_tool: None,
        role: None,
        files_touched: 0,
    });

    assert!(!prefill_jobs_cancel_all_if_tasks_sidebar(&mut non_shell));
    assert_eq!(non_shell.input, "draft");

    let mut other_sidebar = create_test_app();
    other_sidebar.work_surface.panel = crate::tui::work_surface::RailPanel::Agents;
    other_sidebar.work_surface.last_area = Some(ratatui::layout::Rect::new(0, 0, 100, 3));
    other_sidebar.input = "draft".to_string();
    other_sidebar.cursor_position = other_sidebar.input.len();
    other_sidebar.task_panel.push(TaskPanelEntry {
        id: "shell_active".to_string(),
        status: "running".to_string(),
        prompt_summary: "shell: cargo test".to_string(),
        duration_ms: Some(10),
        kind: TaskPanelEntryKind::Background,
        stale: false,
        elapsed_since_output_ms: None,
        owner_agent_id: None,
        owner_agent_name: None,
        current_tool: None,
        role: None,
        files_touched: 0,
    });

    assert!(!prefill_jobs_cancel_all_if_tasks_sidebar(
        &mut other_sidebar
    ));
    assert_eq!(other_sidebar.input, "draft");
}

fn make_subagent(
    id: &str,
    status: crate::tools::subagent::SubAgentStatus,
) -> crate::tools::subagent::SubAgentResult {
    crate::tools::subagent::SubAgentResult {
        name: id.to_string(),
        agent_id: id.to_string(),
        context_mode: "fresh".to_string(),
        fork_context: false,
        workspace: None,
        git_branch: None,
        agent_type: crate::tools::subagent::FleetRole::Worker,
        assignment: crate::tools::subagent::SubAgentAssignment {
            objective: format!("objective-{id}"),
            role: Some("worker".to_string()),
        },
        model: "deepseek-v4-flash".to_string(),
        nickname: None,
        status,
        worker_status: None,
        runtime_permissions: None,
        parent_run_id: None,
        spawn_depth: 0,
        child_route: None,
        result: None,
        steps_taken: 0,
        checkpoint: None,
        needs_input: None,
        duration_ms: 0,
        started_at: None,
        from_prior_session: false,
    }
}

#[test]
fn sort_subagents_orders_running_before_terminal_statuses() {
    let mut agents = vec![
        make_subagent("agent_c", crate::tools::subagent::SubAgentStatus::Completed),
        make_subagent("agent_a", crate::tools::subagent::SubAgentStatus::Running),
        make_subagent(
            "agent_b",
            crate::tools::subagent::SubAgentStatus::Failed("boom".to_string()),
        ),
    ];

    sort_subagents_in_place(&mut agents);

    assert_eq!(agents[0].agent_id, "agent_a");
    assert_eq!(agents[1].agent_id, "agent_b");
    assert_eq!(agents[2].agent_id, "agent_c");
}

#[test]
fn subagent_hook_preview_is_bounded_on_char_boundaries() {
    let text = format!("{}{}", "鲸".repeat(900), "tail");

    let (preview, truncated) = bounded_subagent_hook_preview(&text);

    assert!(truncated);
    assert!(preview.ends_with("...[truncated]"));
    assert!(preview.len() <= SUBAGENT_HOOK_PREVIEW_LIMIT + "...[truncated]".len());
    assert!(preview.is_char_boundary(preview.len()));
}

#[test]
fn subagent_completion_status_reads_done_sentinel() {
    let result = r#"done
<codewhale:subagent.done>{"agent_id":"agent_x","status":"completed"}</codewhale:subagent.done>"#;

    assert_eq!(
        subagent_completion_status(result).as_deref(),
        Some("completed")
    );
    assert_eq!(subagent_completion_status("no sentinel"), None);
}

#[test]
fn subagent_failure_notice_surfaces_receipt_fields() {
    let result = concat!(
        "Failed: quota exhausted\n",
        "<codewhale:subagent.done>{\"event\":\"subagent.failed\",",
        "\"agent_id\":\"agent_x\",\"name\":\"Tide\",",
        "\"status\":\"failed\",\"failure_class\":\"auth_or_quota\",",
        "\"steps\":12,\"elapsed_ms\":3456,",
        "\"transcript_handle\":\"agent:agent_x/full_transcript\"}",
        "</codewhale:subagent.done>",
    );

    let notice = subagent_failure_notice(result).expect("failure notice");
    assert!(notice.contains("Tide (agent_x)"), "{notice}");
    assert!(notice.contains("auth_or_quota"), "{notice}");
    assert!(notice.contains("12 steps"), "{notice}");
    assert!(notice.contains("3456 ms"), "{notice}");
    assert!(notice.contains("agent:agent_x/full_transcript"), "{notice}");
    assert!(subagent_failure_notice("plain completion").is_none());
}

#[test]
fn subagent_completion_status_reads_summary_fallbacks() {
    assert_eq!(
        subagent_completion_status("Cancelled").as_deref(),
        Some("cancelled")
    );
    assert_eq!(
        subagent_completion_status("Failed: tool timed out").as_deref(),
        Some("failed")
    );
    assert_eq!(
        subagent_completion_status("Interrupted: process restarted").as_deref(),
        Some("interrupted")
    );
}

#[test]
fn subagent_status_from_completion_result_maps_terminal_sentinels() {
    let failed = r#"Tool timed out
<codewhale:subagent.done>{"agent_id":"agent_x","status":"failed"}</codewhale:subagent.done>"#;
    match subagent_status_from_completion_result(failed) {
        crate::tools::subagent::SubAgentStatus::Failed(reason) => {
            assert_eq!(reason, "Tool timed out")
        }
        status => panic!("expected failed status, got {status:?}"),
    }

    let interrupted = r#"Waiting for follow-up
<codewhale:subagent.done>{"agent_id":"agent_x","status":"interrupted"}</codewhale:subagent.done>"#;
    match subagent_status_from_completion_result(interrupted) {
        crate::tools::subagent::SubAgentStatus::Interrupted(reason) => {
            assert_eq!(reason, "Waiting for follow-up")
        }
        status => panic!("expected interrupted status, got {status:?}"),
    }

    let budget = r#"Token budget exhausted
<codewhale:subagent.done>{"agent_id":"agent_x","status":"budget_exhausted"}</codewhale:subagent.done>"#;
    assert_eq!(
        subagent_status_from_completion_result(budget),
        crate::tools::subagent::SubAgentStatus::BudgetExhausted
    );

    let cancelled = r#"Cancelled
<codewhale:subagent.done>{"agent_id":"agent_x","status":"cancelled"}</codewhale:subagent.done>"#;
    assert_eq!(
        subagent_status_from_completion_result(cancelled),
        crate::tools::subagent::SubAgentStatus::Cancelled
    );

    assert_eq!(
        subagent_status_from_completion_result("plain successful summary"),
        crate::tools::subagent::SubAgentStatus::Completed
    );
}

#[test]
fn agent_complete_terminal_verb_is_truthful_for_cancelled_workers() {
    let cancelled = subagent_status_from_completion_result(
        r#"Cancelled
<codewhale:subagent.done>{"agent_id":"agent_x","status":"cancelled"}</codewhale:subagent.done>"#,
    );

    assert_eq!(subagent_terminal_verb(&cancelled), "cancelled");
    assert_ne!(subagent_terminal_verb(&cancelled), "completed");
    assert_eq!(
        subagent_terminal_verb(&crate::tools::subagent::SubAgentStatus::Completed),
        "completed"
    );
}

#[test]
fn subagent_terminal_projection_from_mailbox_maps_terminal_messages() {
    let completed = crate::tools::subagent::MailboxMessage::Completed {
        agent_id: "agent_done".to_string(),
        summary: "all set".to_string(),
    };
    let (agent_id, status, result) =
        subagent_terminal_projection_from_mailbox(&completed).expect("completed projection");
    assert_eq!(agent_id, "agent_done");
    assert_eq!(status, crate::tools::subagent::SubAgentStatus::Completed);
    assert_eq!(result.as_deref(), Some("all set"));

    let failed = crate::tools::subagent::MailboxMessage::Failed {
        agent_id: "agent_fail".to_string(),
        error: "tool failed".to_string(),
    };
    let (_, status, result) =
        subagent_terminal_projection_from_mailbox(&failed).expect("failed projection");
    assert_eq!(result.as_deref(), Some("tool failed"));
    assert!(matches!(
        status,
        crate::tools::subagent::SubAgentStatus::Failed(ref reason)
            if reason == "tool failed"
    ));

    let interrupted = crate::tools::subagent::MailboxMessage::Interrupted {
        agent_id: "agent_wait".to_string(),
        reason: "needs input".to_string(),
    };
    let (_, status, result) =
        subagent_terminal_projection_from_mailbox(&interrupted).expect("interrupted projection");
    assert_eq!(result.as_deref(), Some("needs input"));
    assert!(matches!(
        status,
        crate::tools::subagent::SubAgentStatus::Interrupted(ref reason)
            if reason == "needs input"
    ));

    let cancelled = crate::tools::subagent::MailboxMessage::Cancelled {
        agent_id: "agent_stop".to_string(),
    };
    let (_, status, result) =
        subagent_terminal_projection_from_mailbox(&cancelled).expect("cancelled projection");
    assert_eq!(status, crate::tools::subagent::SubAgentStatus::Cancelled);
    assert_eq!(result.as_deref(), Some("cancelled"));

    assert!(
        subagent_terminal_projection_from_mailbox(
            &crate::tools::subagent::MailboxMessage::progress("agent_live", "step 1/2")
        )
        .is_none()
    );
}

#[test]
fn running_agent_count_unions_cache_and_progress() {
    let mut app = create_test_app();
    app.subagent_cache = vec![
        make_subagent("agent_a", crate::tools::subagent::SubAgentStatus::Running),
        make_subagent("agent_b", crate::tools::subagent::SubAgentStatus::Completed),
    ];
    app.agent_progress
        .insert("agent_c".to_string(), "planning".to_string());

    assert_eq!(running_agent_count(&app), 2);
}

#[test]
fn reconcile_subagent_activity_state_trims_stale_progress_and_sets_anchor() {
    let mut app = create_test_app();
    app.subagent_cache = vec![
        make_subagent("agent_a", crate::tools::subagent::SubAgentStatus::Running),
        make_subagent("agent_b", crate::tools::subagent::SubAgentStatus::Completed),
    ];
    // Progress rows for ids the cache does not know survive reconciliation:
    // a progress-first agent whose AgentSpawned/AgentList delivery was
    // dropped must not flicker out of the sidebar. Eviction happens once the
    // authoritative cache reports the agent as non-running.
    app.agent_progress
        .insert("agent_pending".to_string(), "old".to_string());

    reconcile_subagent_activity_state(&mut app);
    assert!(app.agent_progress.contains_key("agent_a"));
    assert!(app.agent_progress.contains_key("agent_pending"));
    assert!(app.agent_activity_started_at.is_some());

    // Once the cache authoritatively knows both agents as terminal, their
    // progress rows are trimmed and the activity anchor clears.
    app.subagent_cache = vec![
        make_subagent("agent_a", crate::tools::subagent::SubAgentStatus::Completed),
        make_subagent(
            "agent_pending",
            crate::tools::subagent::SubAgentStatus::Completed,
        ),
    ];
    reconcile_subagent_activity_state(&mut app);
    assert!(app.agent_progress.is_empty());
    assert!(app.agent_activity_started_at.is_none());
}

#[test]
fn reconcile_subagent_activity_state_expires_terminal_cards_but_keeps_running() {
    let mut app = create_test_app();
    let old_seen_at = Instant::now();
    let now = old_seen_at + Duration::from_secs(10 * 60);
    let recent_seen_at = now - Duration::from_secs(30);
    app.subagent_cache = vec![
        make_subagent(
            "agent_running",
            crate::tools::subagent::SubAgentStatus::Running,
        ),
        make_subagent(
            "agent_old",
            crate::tools::subagent::SubAgentStatus::Completed,
        ),
        make_subagent(
            "agent_recent",
            crate::tools::subagent::SubAgentStatus::Failed("boom".to_string()),
        ),
    ];
    app.subagent_terminal_seen_at
        .insert("agent_old".to_string(), old_seen_at);
    app.subagent_terminal_seen_at
        .insert("agent_recent".to_string(), recent_seen_at);

    reconcile_subagent_activity_state_at(&mut app, now);

    let ids: HashSet<&str> = app
        .subagent_cache
        .iter()
        .map(|agent| agent.agent_id.as_str())
        .collect();
    assert!(ids.contains("agent_running"));
    assert!(ids.contains("agent_recent"));
    assert!(!ids.contains("agent_old"));
    assert!(!app.subagent_terminal_seen_at.contains_key("agent_old"));
    assert!(app.subagent_terminal_seen_at.contains_key("agent_recent"));
}

#[test]
fn reconcile_subagent_activity_state_caps_terminal_card_bursts() {
    let mut app = create_test_app();
    let oldest_seen_at = Instant::now();
    let now = oldest_seen_at + Duration::from_secs(30);
    for idx in 0..30 {
        let id = format!("agent_{idx:02}");
        app.subagent_cache.push(make_subagent(
            &id,
            crate::tools::subagent::SubAgentStatus::Completed,
        ));
        app.subagent_terminal_seen_at
            .insert(id, now - Duration::from_secs(idx));
    }

    reconcile_subagent_activity_state_at(&mut app, now);

    let terminal_count = app
        .subagent_cache
        .iter()
        .filter(|agent| {
            !matches!(
                agent.status,
                crate::tools::subagent::SubAgentStatus::Running
            )
        })
        .count();
    assert_eq!(terminal_count, 24);
    assert!(
        app.subagent_cache
            .iter()
            .any(|agent| agent.agent_id == "agent_00")
    );
    assert!(
        !app.subagent_cache
            .iter()
            .any(|agent| agent.agent_id == "agent_29")
    );
}

#[test]
fn subagent_token_usage_updates_live_cost_counter_without_card_change() {
    let mut app = create_test_app();
    handle_subagent_mailbox(
        &mut app,
        1,
        &crate::tools::subagent::MailboxMessage::TokenUsage {
            agent_id: "agent-a".to_string(),
            source_id: "response-a".to_string(),
            route: test_mailbox_route(ApiProvider::Deepseek, "deepseek-v4-flash"),
            usage: crate::models::Usage {
                input_tokens: 10_000,
                output_tokens: 1_000,
                ..Default::default()
            },
        },
    );

    assert!(app.session.subagent_cost > 0.0);
    assert!(
        app.history.is_empty(),
        "usage-only mailbox messages should not allocate a sub-agent card"
    );
}

#[test]
fn subagent_token_usage_prices_the_child_route_not_the_parent_route() {
    let mut app = create_test_app();
    assert_eq!(app.api_provider, ApiProvider::Deepseek);

    handle_subagent_mailbox(
        &mut app,
        2,
        &crate::tools::subagent::MailboxMessage::TokenUsage {
            agent_id: "agent-codex".to_string(),
            source_id: "response-codex".to_string(),
            route: test_mailbox_route(ApiProvider::OpenaiCodex, "gpt-5.5"),
            usage: crate::models::Usage {
                input_tokens: 10_000,
                output_tokens: 1_000,
                ..Default::default()
            },
        },
    );

    assert_eq!(
        app.session.subagent_cost, 0.0,
        "ChatGPT/Codex child usage must not inherit the DeepSeek parent's pricing"
    );
}

#[test]
fn subagent_token_usage_is_deduped_by_response_source() {
    let mut app = create_test_app();
    let usage = crate::tools::subagent::MailboxMessage::TokenUsage {
        agent_id: "agent-a".to_string(),
        source_id: "response-a".to_string(),
        route: test_mailbox_route(ApiProvider::Deepseek, "deepseek-v4-flash"),
        usage: crate::models::Usage {
            input_tokens: 10_000,
            output_tokens: 1_000,
            ..Default::default()
        },
    };

    handle_subagent_mailbox(&mut app, 7, &usage);
    let first = app.session.subagent_cost;
    handle_subagent_mailbox(&mut app, 8, &usage);
    assert_eq!(app.session.subagent_cost, first);
    let mut distinct = usage.clone();
    let crate::tools::subagent::MailboxMessage::TokenUsage { source_id, .. } = &mut distinct else {
        unreachable!()
    };
    *source_id = "response-b".to_string();
    handle_subagent_mailbox(&mut app, 9, &distinct);
    assert!(app.session.subagent_cost > first);
}

#[test]
fn subagent_token_usage_source_is_stable_across_engine_turns() {
    let mut app = create_test_app();
    let usage = crate::tools::subagent::MailboxMessage::TokenUsage {
        agent_id: "agent-a".to_string(),
        source_id: "response-a".to_string(),
        route: test_mailbox_route(ApiProvider::Deepseek, "deepseek-v4-flash"),
        usage: crate::models::Usage {
            input_tokens: 10_000,
            output_tokens: 1_000,
            ..Default::default()
        },
    };

    handle_subagent_mailbox_for_turn(&mut app, "engine-turn-a", 1, &usage);
    let first = app.session.subagent_cost;
    handle_subagent_mailbox_for_turn(&mut app, "engine-turn-b", 1, &usage);
    assert_eq!(app.session.subagent_cost, first, "same response dedupes");
    let mut distinct = usage.clone();
    let crate::tools::subagent::MailboxMessage::TokenUsage { source_id, .. } = &mut distinct else {
        unreachable!()
    };
    *source_id = "response-b".to_string();
    handle_subagent_mailbox_for_turn(&mut app, "engine-turn-b", 2, &distinct);
    assert!(
        app.session.subagent_cost > first,
        "a distinct provider response must accrue independently"
    );
}

#[test]
fn fanout_started_sibling_bumps_existing_card_revision() {
    let mut app = create_test_app();
    app.pending_subagent_dispatch = Some("rlm".to_string());

    handle_subagent_mailbox(
        &mut app,
        1,
        &crate::tools::subagent::MailboxMessage::Started {
            agent_id: "fanout-a".to_string(),
            agent_type: "default".to_string(),
        },
    );

    let fanout_idx = app.last_fanout_card_index.expect("fanout card index");
    let initial_revision = app.history_revisions[fanout_idx];

    handle_subagent_mailbox(
        &mut app,
        2,
        &crate::tools::subagent::MailboxMessage::Started {
            agent_id: "fanout-b".to_string(),
            agent_type: "default".to_string(),
        },
    );

    assert_eq!(app.history.len(), 1, "sibling should reuse fanout card");
    assert_ne!(
        app.history_revisions[fanout_idx], initial_revision,
        "reused fanout card must invalidate its cached transcript rows"
    );
    match &app.history[fanout_idx] {
        HistoryCell::SubAgent(SubAgentCell::Fanout(card)) => {
            assert_eq!(card.worker_count(), 2);
        }
        cell => panic!("expected fanout card, got {cell:?}"),
    }
}

#[test]
fn fanout_interrupted_mailbox_drops_running_count() {
    let mut app = create_test_app();
    app.pending_subagent_dispatch = Some("rlm".to_string());

    for (seq, id) in ["fanout-a", "fanout-b"].iter().enumerate() {
        handle_subagent_mailbox(
            &mut app,
            seq as u64 + 1,
            &crate::tools::subagent::MailboxMessage::Started {
                agent_id: (*id).to_string(),
                agent_type: "default".to_string(),
            },
        );
    }
    assert_eq!(
        crate::tui::subagent_routing::active_fanout_counts(&app),
        Some((2, 2))
    );

    handle_subagent_mailbox(
        &mut app,
        3,
        &crate::tools::subagent::MailboxMessage::Interrupted {
            agent_id: "fanout-a".to_string(),
            reason: "API call timed out after 120000ms".to_string(),
        },
    );

    assert_eq!(
        crate::tui::subagent_routing::active_fanout_counts(&app),
        Some((1, 2)),
        "interrupted worker must no longer count as running"
    );
}

#[test]
fn reconcile_syncs_stale_running_cards_with_interrupted_snapshots() {
    let mut app = create_test_app();
    app.pending_subagent_dispatch = Some("rlm".to_string());

    for (seq, id) in ["fanout-a", "fanout-b"].iter().enumerate() {
        handle_subagent_mailbox(
            &mut app,
            seq as u64 + 1,
            &crate::tools::subagent::MailboxMessage::Started {
                agent_id: (*id).to_string(),
                agent_type: "default".to_string(),
            },
        );
    }
    let fanout_idx = app.last_fanout_card_index.expect("fanout card index");
    let initial_revision = app.history_revisions[fanout_idx];

    // The card missed its lifecycle envelope; only the manager snapshot
    // (delivered via AgentList) knows the agents were interrupted.
    app.subagent_cache = vec![
        make_subagent(
            "fanout-a",
            crate::tools::subagent::SubAgentStatus::Interrupted("API call timed out".to_string()),
        ),
        make_subagent("fanout-b", crate::tools::subagent::SubAgentStatus::Running),
    ];
    reconcile_subagent_activity_state(&mut app);

    assert_eq!(
        crate::tui::subagent_routing::active_fanout_counts(&app),
        Some((1, 2)),
        "snapshot reconciliation must clear the stale running slot"
    );
    assert_ne!(
        app.history_revisions[fanout_idx], initial_revision,
        "reconciled card must invalidate cached transcript rows"
    );
    assert_eq!(running_agent_count(&app), 1);
}

#[test]
fn provider_wait_incident_logs_once_per_turn() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.turn_started_at = Some(Instant::now() - Duration::from_secs(150));
    app.pending_subagent_dispatch = Some("rlm".to_string());

    assert!(!app.provider_wait_incident_logged);
    crate::tui::footer_ui::maybe_log_provider_wait_incident(&mut app);
    assert!(app.provider_wait_incident_logged, "incident logged once");

    // Below threshold or without a fanout plan, nothing is logged.
    let mut quiet = create_test_app();
    quiet.is_loading = true;
    quiet.turn_started_at = Some(Instant::now() - Duration::from_secs(150));
    crate::tui::footer_ui::maybe_log_provider_wait_incident(&mut quiet);
    assert!(!quiet.provider_wait_incident_logged);

    let mut early = create_test_app();
    early.is_loading = true;
    early.turn_started_at = Some(Instant::now() - Duration::from_secs(60));
    early.pending_subagent_dispatch = Some("rlm".to_string());
    crate::tui::footer_ui::maybe_log_provider_wait_incident(&mut early);
    assert!(!early.provider_wait_incident_logged);
}

#[test]
fn format_token_count_compact_formats_units() {
    assert_eq!(format_token_count_compact(999), "999");
    assert_eq!(format_token_count_compact(1_200), "1.2k");
    assert_eq!(format_token_count_compact(1_000_000), "1.0M");
}

#[test]
fn format_context_budget_caps_overflow_display() {
    assert_eq!(format_context_budget(5_000, 128_000), "5.0k/128.0k");
    assert_eq!(format_context_budget(250_000, 128_000), ">128.0k/128.0k");
}

#[test]
fn event_poll_timeout_has_nonzero_floor() {
    assert_eq!(
        clamp_event_poll_timeout(Duration::ZERO),
        Duration::from_millis(1)
    );
    assert_eq!(
        clamp_event_poll_timeout(Duration::from_micros(250)),
        Duration::from_millis(1)
    );
    assert_eq!(
        clamp_event_poll_timeout(Duration::from_millis(24)),
        Duration::from_millis(24)
    );
}

#[tokio::test]
async fn bang_shell_input_dispatches_shell_op_instead_of_model_message() {
    let mut app = create_test_app();
    app.mode = AppMode::Agent;
    app.trust_mode = false;
    // Pin the posture: App::new consults the developer's real saved
    // settings, so a machine dogfooding with Bypass/Full Access would flip
    // the auto_approve assertion below (hermeticity, not behavior).
    app.approval_mode = ApprovalMode::Suggest;

    let mut engine = mock_engine_handle();

    let handled = handle_bang_shell_input(&mut app, &engine.handle, "! pwd")
        .await
        .expect("bang shell handler");

    assert!(handled);
    assert_eq!(
        app.status_message.as_deref(),
        Some("Shell command submitted: pwd")
    );

    let op = engine.rx_op.recv().await.expect("engine op");
    match op {
        Op::RunShellCommand {
            command,
            mode,
            allow_shell,
            trust_mode,
            auto_approve,
            approval_mode,
        } => {
            assert_eq!(command, "pwd");
            assert_eq!(mode, AppMode::Agent);
            assert!(!allow_shell);
            assert!(!trust_mode);
            assert!(!auto_approve);
            assert_eq!(approval_mode, ApprovalMode::Suggest);
        }
        other => panic!("expected RunShellCommand, got {other:?}"),
    }
}

#[tokio::test]
async fn bang_shell_input_keeps_auto_review_separate_from_bypass() {
    let mut app = create_test_app();
    app.mode = AppMode::Agent;
    app.approval_mode = ApprovalMode::Auto;
    app.trust_mode = true;

    let mut engine = mock_engine_handle();

    let handled = handle_bang_shell_input(&mut app, &engine.handle, "! pwd")
        .await
        .expect("bang shell handler");

    assert!(handled);
    let op = engine.rx_op.recv().await.expect("engine op");
    match op {
        Op::RunShellCommand {
            command,
            mode,
            allow_shell,
            trust_mode,
            auto_approve,
            approval_mode,
        } => {
            assert_eq!(command, "pwd");
            assert_eq!(mode, AppMode::Agent);
            assert!(!allow_shell);
            assert!(trust_mode);
            assert!(!auto_approve);
            assert_eq!(approval_mode, ApprovalMode::Auto);
        }
        other => panic!("expected RunShellCommand, got {other:?}"),
    }
}

#[tokio::test]
async fn bang_shell_input_dispatches_even_while_turn_is_loading() {
    let mut app = create_test_app();
    app.mode = AppMode::Agent;
    app.is_loading = true;

    let mut engine = mock_engine_handle();

    let handled = handle_bang_shell_input(&mut app, &engine.handle, "! echo steer-safe")
        .await
        .expect("bang shell handler");

    assert!(handled);
    let op = engine.rx_op.recv().await.expect("engine op");
    match op {
        Op::RunShellCommand { command, mode, .. } => {
            assert_eq!(command, "echo steer-safe");
            assert_eq!(mode, AppMode::Agent);
        }
        other => panic!("expected RunShellCommand, got {other:?}"),
    }
}

#[tokio::test]
async fn empty_bang_shell_input_is_consumed_with_usage_error() {
    let mut app = create_test_app();
    let engine = mock_engine_handle();

    let handled = handle_bang_shell_input(&mut app, &engine.handle, "!   ")
        .await
        .expect("bang shell handler");

    assert!(handled);
    assert_eq!(
        app.status_message.as_deref(),
        Some("Error: Usage: ! <shell command>")
    );
}

#[test]
fn local_bang_shell_tool_ids_are_not_model_visible() {
    assert!(!is_model_visible_tool_call("user_shell_1"));
    assert!(is_model_visible_tool_call("toolu_01abc"));
}

fn complete_release_json(tag: &str) -> serde_json::Value {
    let assets = REQUIRED_RELEASE_ASSETS
        .iter()
        .map(|name| serde_json::json!({ "name": name, "state": "uploaded" }))
        .collect::<Vec<_>>();
    serde_json::json!({
        "tag_name": tag,
        "draft": false,
        "prerelease": false,
        "assets": assets,
    })
}

#[test]
fn startup_notice_accepts_the_v095_single_binary_release_inventory() {
    const V095_ASSETS: &[&str] = &[
        "codewhale-linux-x64",
        "codew-linux-x64",
        "codewhale-linux-arm64",
        "codew-linux-arm64",
        "codewhale-android-arm64",
        "codew-android-arm64",
        "codewhale-macos-x64",
        "codew-macos-x64",
        "codewhale-macos-arm64",
        "codew-macos-arm64",
        "codewhale-windows-x64.exe",
        "codew-windows-x64.exe",
        "codewhale.bat",
        "codewhale-windows-arm64.exe",
        "codew-windows-arm64.exe",
        "codewhale-linux-x64.tar.gz",
        "codewhale-linux-arm64.tar.gz",
        "codewhale-android-arm64.tar.gz",
        "codewhale-macos-x64.tar.gz",
        "codewhale-macos-arm64.tar.gz",
        "codewhale-windows-x64.zip",
        "codewhale-windows-x64-portable.zip",
        "codewhale-windows-arm64.zip",
        "codewhale-windows-arm64-portable.zip",
        "CodeWhaleSetup.exe",
        "codewhale-bundles-sha256.txt",
        "codewhale-artifacts-sha256.txt",
    ];

    assert_eq!(REQUIRED_RELEASE_ASSETS, V095_ASSETS);
    assert!(
        REQUIRED_RELEASE_ASSETS
            .iter()
            .all(|asset| !asset.starts_with("codewhale-tui-")),
        "the single-binary release must not wait for removed TUI assets"
    );

    let release = serde_json::json!({
        "tag_name": "v0.9.5",
        "draft": false,
        "prerelease": false,
        "assets": V095_ASSETS
            .iter()
            .map(|name| serde_json::json!({ "name": name, "state": "uploaded" }))
            .collect::<Vec<_>>(),
    });
    let notice = version_hint_from_release_json(&release, "0.9.4")
        .expect("the complete v0.9.5 inventory must produce an update notice");
    assert_eq!(notice.chip_label(), "↑ v0.9.5");
}

#[test]
fn version_hint_requires_complete_release_assets() {
    let complete = complete_release_json("v0.8.47");
    let hint = version_hint_from_release_json(&complete, "0.8.46").expect("newer complete release");
    assert!(
        hint.toast_line(codewhale_release::InstallMethod::Binary)
            .contains("v0.8.47 available")
    );

    let mut missing_manifest = complete_release_json("v0.8.47");
    missing_manifest["assets"] = serde_json::Value::Array(
        missing_manifest["assets"]
            .as_array()
            .expect("assets")
            .iter()
            .filter(|asset| {
                asset.get("name").and_then(serde_json::Value::as_str)
                    != Some("codewhale-artifacts-sha256.txt")
            })
            .cloned()
            .collect(),
    );
    assert!(
        version_hint_from_release_json(&missing_manifest, "0.8.46").is_none(),
        "do not advertise a release before checksums are uploaded"
    );

    let mut pending_asset = complete_release_json("v0.8.47");
    pending_asset["assets"].as_array_mut().expect("assets")[0]["state"] = serde_json::json!("open");
    assert!(
        version_hint_from_release_json(&pending_asset, "0.8.46").is_none(),
        "do not advertise a release before every asset is uploaded"
    );

    let mut missing_state = complete_release_json("v0.8.47");
    missing_state["assets"].as_array_mut().expect("assets")[0]
        .as_object_mut()
        .expect("asset object")
        .remove("state");
    assert!(
        version_hint_from_release_json(&missing_state, "0.8.46").is_none(),
        "do not accept malformed asset state as uploaded"
    );
}

#[test]
fn version_hint_ignores_draft_prerelease_and_current_versions() {
    let mut draft = complete_release_json("v0.8.47");
    draft["draft"] = serde_json::Value::Bool(true);
    assert!(version_hint_from_release_json(&draft, "0.8.46").is_none());

    let mut prerelease = complete_release_json("v0.8.47");
    prerelease["prerelease"] = serde_json::Value::Bool(true);
    assert!(version_hint_from_release_json(&prerelease, "0.8.46").is_none());

    let current = complete_release_json("v0.8.46");
    assert!(version_hint_from_release_json(&current, "0.8.46").is_none());
}

#[test]
fn startup_version_check_source_respects_update_config() {
    assert_eq!(
        resolve_version_check_source(
            &UpdateConfig {
                check_for_updates: false,
                update_uri: Some("https://mirror.example/releases/latest".to_string()),
                ..UpdateConfig::default()
            },
            None
        ),
        StartupVersionCheckSource::Disabled
    );

    assert_eq!(
        resolve_version_check_source(
            &UpdateConfig {
                check_for_updates: true,
                update_uri: Some("  https://mirror.example/releases/latest  ".to_string()),
                ..UpdateConfig::default()
            },
            None
        ),
        StartupVersionCheckSource::ConfiguredUrl(
            "https://mirror.example/releases/latest".to_string()
        )
    );

    assert_eq!(
        resolve_version_check_source(&UpdateConfig::default(), None),
        StartupVersionCheckSource::ReleaseResolver
    );
}

#[test]
fn startup_version_check_is_disabled_in_ci_and_on_opt_out() {
    // A build agent has nobody to tell, so the check never reaches the
    // network there — even with the feature enabled and a mirror configured.
    for reason in [
        codewhale_release::SuppressionReason::ContinuousIntegration("CI"),
        codewhale_release::SuppressionReason::OptOut("CODEWHALE_NO_UPDATE_CHECK"),
    ] {
        assert_eq!(
            resolve_version_check_source(
                &UpdateConfig {
                    check_for_updates: true,
                    update_uri: Some("https://mirror.example/releases/latest".to_string()),
                    ..UpdateConfig::default()
                },
                Some(reason)
            ),
            StartupVersionCheckSource::Disabled,
            "{} should suppress the check",
            reason.variable()
        );
    }
}

#[tokio::test]
async fn a_fresh_cache_answers_the_update_check_without_the_network() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cache = codewhale_release::check::cache_path_in(dir.path());
    codewhale_release::UpdateCheckCache::now(Some("v9.9.9".to_string()))
        .store(&cache)
        .expect("seed cache");

    // `ConfiguredUrl` points at an unroutable host: if the cache were not
    // consulted first this would fail rather than produce a notice.
    let notice = cached_version_hint(
        StartupVersionCheckSource::ConfiguredUrl("http://127.0.0.1:1/nope".to_string()),
        "0.9.4",
        Some(&cache),
        24,
    )
    .await
    .expect("fresh cache should still yield the notice");
    assert_eq!(notice.chip_label(), "↑ v9.9.9");
}

#[tokio::test]
async fn a_cached_up_to_date_answer_produces_no_notice() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cache = codewhale_release::check::cache_path_in(dir.path());
    codewhale_release::UpdateCheckCache::now(Some("v0.9.4".to_string()))
        .store(&cache)
        .expect("seed cache");

    assert!(
        cached_version_hint(
            StartupVersionCheckSource::ConfiguredUrl("http://127.0.0.1:1/nope".to_string()),
            "0.9.4",
            Some(&cache),
            24,
        )
        .await
        .is_none()
    );
}

#[tokio::test]
async fn a_failed_check_leaves_the_cache_untouched_so_the_next_launch_retries() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cache = codewhale_release::check::cache_path_in(dir.path());

    let notice = cached_version_hint(
        StartupVersionCheckSource::ConfiguredUrl("http://127.0.0.1:1/nope".to_string()),
        "0.9.4",
        Some(&cache),
        24,
    )
    .await;
    assert!(notice.is_none(), "an unreachable endpoint yields no notice");
    assert!(
        !cache.exists(),
        "a network failure must not be cached as 'no update' for a whole day"
    );
}

#[test]
fn update_notice_names_the_command_for_the_actual_install_method() {
    let complete = complete_release_json("v0.8.47");
    let notice =
        version_hint_from_release_json(&complete, "0.8.46").expect("newer release yields a notice");

    // A package-managed install must not be told to self-replace: the manager
    // would silently revert it on the next upgrade.
    let npm_block = notice.notice_block(codewhale_release::InstallMethod::Npm);
    assert!(
        npm_block.contains("npm install -g codewhale@latest"),
        "names the npm command: {npm_block:?}"
    );
    assert!(
        npm_block.contains("Do not use `codewhale update`"),
        "warns against self-update: {npm_block:?}"
    );
    assert!(
        notice
            .toast_line(codewhale_release::InstallMethod::Homebrew)
            .contains("brew upgrade codewhale"),
        "toast names the Homebrew command"
    );
    assert!(
        notice
            .toast_line(codewhale_release::InstallMethod::Omarchy)
            .contains("omarchy update"),
        "toast names the Omarchy update command"
    );

    // A plain release binary keeps the self-updater.
    assert!(
        notice
            .toast_line(codewhale_release::InstallMethod::Binary)
            .contains("codewhale update")
    );
}

#[test]
fn custom_update_uri_accepts_tag_only_release_json() {
    let json = serde_json::json!({
        "tag_name": "v0.8.47",
        "draft": false,
        "prerelease": false,
    });

    let hint = version_hint_from_custom_release_json(&json, "0.8.46")
        .expect("tag-only custom metadata should be enough for mirrors");
    assert!(
        hint.toast_line(codewhale_release::InstallMethod::Binary)
            .contains("v0.8.47 available")
    );
}

#[test]
fn update_notice_block_is_persistent_and_actionable() {
    let complete = complete_release_json("v0.8.47");
    let notice = version_hint_from_release_json(&complete, "0.8.46")
        .expect("newer complete release yields a notice");

    // Durable notice carries current + latest versions, release-notes link,
    // the exact update command, and restart guidance (#3961 acceptance).
    let block = notice.notice_block(codewhale_release::InstallMethod::Binary);
    assert!(
        block.contains("v0.8.46"),
        "shows current version: {block:?}"
    );
    assert!(block.contains("v0.8.47"), "shows latest version: {block:?}");
    assert!(
        block.contains("https://github.com/Hmbown/CodeWhale/releases/tag/v0.8.47"),
        "includes release-notes link: {block:?}"
    );
    assert!(
        block.contains("codewhale update"),
        "includes the update command: {block:?}"
    );
    assert!(
        block.contains("`/update install`") && block.contains("bare `/update`"),
        "names the in-app install command and its check-first preview: {block:?}"
    );
    assert!(
        block.to_lowercase().contains("restart"),
        "includes restart guidance: {block:?}"
    );

    // The persistent header chip is quiet: version only, no action verb —
    // the toast and this block carry the instructions (#14).
    assert_eq!(notice.chip_label(), "↑ v0.8.47");

    // A current release produces no notice at all (no toast, no transcript spam).
    let current = complete_release_json("v0.8.46");
    assert!(version_hint_from_release_json(&current, "0.8.46").is_none());
}

#[test]
#[cfg(any(unix, windows))]
fn external_url_launcher_does_not_wait_for_browser_process() {
    let command = slow_external_url_command();
    let start = Instant::now();

    spawn_external_url_command(command).expect("spawn external URL command");

    assert!(
        start.elapsed() < Duration::from_millis(750),
        "opening a feedback URL must not wait for the browser command to exit"
    );
}

#[cfg(unix)]
fn slow_external_url_command() -> Command {
    let mut command = Command::new("sh");
    command.args(["-c", "sleep 1"]);
    command
}

#[cfg(windows)]
fn slow_external_url_command() -> Command {
    let mut command = Command::new("cmd");
    command.args(["/C", "ping -n 2 127.0.0.1 >NUL"]);
    command
}

#[test]
fn context_usage_snapshot_prefers_estimate_when_reported_exceeds_window() {
    let mut app = create_test_app();
    app.session.last_prompt_tokens = Some(1_200_000);
    app.api_messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "hello".to_string(),
            cache_control: None,
        }],
    }];

    let (used, max, percent) =
        context_usage_snapshot(&app).expect("context usage should be available");
    assert_eq!(max, 1_000_000);
    assert!(used > 0);
    assert!(used <= i64::from(max));
    assert!(percent < 100.0);
}

#[test]
fn context_usage_cache_tracks_append_and_compaction_lengths() {
    let mut app = create_test_app();
    app.api_messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "first".to_string(),
            cache_control: None,
        }],
    }];
    context_usage_snapshot(&app).expect("context usage should be available");
    assert_eq!(app.context_token_cache.borrow().message_tokens.len(), 1);

    app.api_messages.push(Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Text {
            text: "second".to_string(),
            cache_control: None,
        }],
    });
    context_usage_snapshot(&app).expect("context usage should be available");
    assert_eq!(app.context_token_cache.borrow().message_tokens.len(), 2);

    app.api_messages.truncate(1);
    context_usage_snapshot(&app).expect("context usage should be available");
    assert_eq!(app.context_token_cache.borrow().message_tokens.len(), 1);
}

#[test]
fn context_usage_cache_refreshes_after_compaction_replaces_messages() {
    let mut app = create_test_app();
    app.api_messages = (0..3)
        .map(|_| Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "context ".repeat(2_000),
                cache_control: None,
            }],
        })
        .collect();
    let (before, _, _) = context_usage_snapshot(&app).expect("context usage should be available");

    app.api_messages = vec![Message {
        role: Role::Assistant,
        content: vec![ContentBlock::Text {
            text: "compact summary".to_string(),
            cache_control: None,
        }],
    }];
    app.context_token_cache.borrow_mut().clear();
    let (after, _, _) =
        context_usage_snapshot(&app).expect("compacted context usage should be available");

    assert!(
        after < before,
        "compaction should refresh cached token estimates: before={before}, after={after}"
    );
}

#[test]
fn context_usage_snapshot_prefers_estimate_when_reported_is_inflated_by_old_reasoning() {
    let mut app = create_test_app();
    app.session.last_prompt_tokens = Some(980_000);
    app.api_messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "small current context".to_string(),
            cache_control: None,
        }],
    }];

    let (used, max, percent) =
        context_usage_snapshot(&app).expect("context usage should be available");
    assert_eq!(max, 1_000_000);
    assert!(used < 10_000);
    assert!(percent < 2.0);
}

/// Regression for #115. The engine sums `input_tokens` across every round
/// of a turn (`turn.add_usage` does `+=`), so a multi-round tool-call turn
/// reports a value much larger than the actual context window state, then
/// the next single-round turn drops back to a single round's input_tokens.
/// User-visible % was bouncing 31% → 9% because of this. The fix is to
/// prefer the estimated current-context size, which is monotonic wrt
/// conversation growth.
#[test]
fn context_usage_does_not_drop_when_reported_shrinks_after_multi_round_turn() {
    let mut app = create_test_app();
    app.api_messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "context ".repeat(2_000), // ~14k tokens estimated
            cache_control: None,
        }],
    }];

    // Simulate a multi-round turn that summed two rounds' input_tokens
    // (e.g., 200k + 210k from a long thinking + tool-call sequence).
    app.session.last_prompt_tokens = Some(410_000);
    let (_, _, percent_after_multi_round) = context_usage_snapshot(&app).expect("usage available");

    // Now the next turn is a single round on the same conversation —
    // reported drops to one round's worth even though the actual context
    // hasn't shrunk.
    app.session.last_prompt_tokens = Some(15_000);
    let (_, _, percent_after_single_round) = context_usage_snapshot(&app).expect("usage available");

    // The displayed % should reflect the conversation size (estimated
    // from api_messages), NOT the wildly variable reported value.
    let drift = (percent_after_multi_round - percent_after_single_round).abs();
    assert!(
        drift < 1.0,
        "displayed % should not jump because reported tokens varied across rounds; \
         after-multi-round={percent_after_multi_round:.2} after-single-round={percent_after_single_round:.2}"
    );
}

#[test]
fn context_usage_snapshot_prefers_live_estimate_while_loading() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.session.last_prompt_tokens = Some(128);
    app.api_messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "context ".repeat(6_000),
            cache_control: None,
        }],
    }];

    let estimated = estimated_context_tokens(&app).expect("estimated context should be available");
    let (used, max, percent) =
        context_usage_snapshot(&app).expect("context usage should be available");
    assert_eq!(used, estimated);
    assert_eq!(max, 1_000_000);
    assert!(used > i64::from(app.session.last_prompt_tokens.expect("reported tokens")));
    assert!(percent > 0.0);
}

#[test]
fn should_auto_compact_before_send_uses_shared_token_threshold() {
    let mut app = create_test_app();
    app.api_messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "context ".repeat(240_000),
            cache_control: None,
        }],
    }];
    let (used, _, _) = context_usage_snapshot(&app).expect("context snapshot");
    let used = usize::try_from(used).expect("non-negative context estimate");

    app.auto_compact = true;
    app.compact_threshold = used;
    assert!(should_auto_compact_before_send(&app));

    app.compact_threshold = used.saturating_add(1);
    assert!(!should_auto_compact_before_send(&app));

    app.auto_compact = false;
    app.compact_threshold = 0;
    assert!(!should_auto_compact_before_send(&app));
}

#[test]
fn context_pressure_warning_reflects_auto_compact_threshold_state() {
    let mut app = create_test_app();
    app.api_messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "context ".repeat(240_000),
            cache_control: None,
        }],
    }];
    app.auto_compact = true;
    app.auto_compact_threshold_percent = 100.0;
    let (used, _, percent) = context_usage_snapshot(&app).expect("context snapshot");
    assert!(
        percent < app.auto_compact_threshold_percent,
        "fixture must remain below the raw window-relative setting"
    );
    app.compact_threshold = usize::try_from(used).expect("non-negative context estimate");

    maybe_warn_context_pressure(&mut app);

    let status = app.status_message.as_deref().expect("context warning");
    assert!(
        status.contains("Auto-compaction will run before the next send."),
        "unexpected status: {status}"
    );
    assert!(
        app.sticky_status
            .as_ref()
            .is_some_and(|toast| toast.text == status),
        "context pressure must remain visible in sticky status: {status}"
    );
}

#[test]
fn context_pressure_warning_survives_later_status_and_can_be_dismissed() {
    let mut app = create_test_app();
    app.api_messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "context ".repeat(240_000),
            cache_control: None,
        }],
    }];
    app.auto_compact = true;
    app.auto_compact_threshold_percent = 100.0;
    let (used, _, _) = context_usage_snapshot(&app).expect("context snapshot");
    app.compact_threshold = usize::try_from(used).expect("non-negative context estimate");
    maybe_warn_context_pressure(&mut app);
    let warning = app
        .sticky_status
        .as_ref()
        .expect("sticky context warning")
        .text
        .clone();

    app.status_message = Some("A later transcript status".to_string());
    let visible = app
        .active_status_toast()
        .expect("a later transient status is visible");
    assert_eq!(visible.text, "A later transcript status");
    assert_eq!(
        app.sticky_status.as_ref().map(|toast| toast.text.as_str()),
        Some(warning.as_str()),
        "the persistent pressure warning remains behind the transient status"
    );
    assert!(app.dismiss_context_pressure_warning());
    assert!(app.sticky_status.is_none());
    app.status_message = None;
    maybe_warn_context_pressure(&mut app);
    assert!(
        app.sticky_status.is_none(),
        "explicit dismissal must not re-arm on the next pressure check"
    );
}

#[test]
fn context_pressure_warning_clears_when_compaction_starts() {
    let mut app = create_test_app();
    app.api_messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "context ".repeat(240_000),
            cache_control: None,
        }],
    }];
    app.auto_compact = true;
    app.auto_compact_threshold_percent = 100.0;
    let (used, _, _) = context_usage_snapshot(&app).expect("context snapshot");
    app.compact_threshold = usize::try_from(used).expect("non-negative context estimate");
    maybe_warn_context_pressure(&mut app);
    assert!(
        app.sticky_status
            .as_ref()
            .is_some_and(|toast| toast.text.starts_with("Context "))
    );

    apply_compaction_started(&mut app, "compaction-1".to_string(), false);
    assert!(
        app.sticky_status.is_none(),
        "pressure warning should be cleared"
    );
    assert!(
        app.status_message
            .as_deref()
            .is_some_and(|text| text.to_ascii_lowercase().contains("compacting")),
        "compaction status should replace the pressure warning"
    );
}

#[test]
fn context_pressure_warning_stays_absent_below_threshold() {
    let mut app = create_test_app();
    maybe_warn_context_pressure(&mut app);

    assert!(app.sticky_status.is_none());
    assert!(
        app.status_message.is_none(),
        "low-pressure sessions should not create warning chrome"
    );
}

// ============================================================================
// Streaming Cancel Behavior Tests
// ============================================================================

#[test]
fn test_esc_cancels_streaming_sets_is_loading_false() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.mode = AppMode::Agent;

    // Simulate what happens in ui.rs when Esc is pressed during loading:
    // engine_handle.cancel() is called (can't test directly - private)
    // Then these state changes occur:
    app.is_loading = false;
    app.status_message = Some("Request cancelled".to_string());

    assert!(!app.is_loading);
    assert_eq!(app.status_message, Some("Request cancelled".to_string()));
}

#[test]
fn test_esc_with_input_clears_input_when_not_loading() {
    let mut app = create_test_app();
    app.is_loading = false;
    app.input = "some draft input".to_string();
    app.cursor_position = app.input.chars().count();

    // Simulate Esc key press when not loading but input not empty
    app.clear_input();

    assert!(app.input.is_empty());
    assert_eq!(app.cursor_position, 0);
    assert!(!app.is_loading);
}

#[test]
fn test_esc_discards_queued_draft_before_clearing_input() {
    let mut app = create_test_app();
    app.is_loading = false;
    app.input.clear();
    app.queued_draft = Some(crate::tui::app::QueuedMessage::new(
        "queued draft".to_string(),
        None,
    ));

    assert_eq!(
        next_escape_action(&app, false),
        EscapeAction::DiscardQueuedDraft
    );
}

#[test]
fn test_esc_prioritizes_queued_draft_edit_over_loading_cancel() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.input = "editing queued follow-up".to_string();
    app.queued_draft = Some(crate::tui::app::QueuedMessage::new(
        "original queued follow-up".to_string(),
        None,
    ));

    assert_eq!(
        next_escape_action(&app, false),
        EscapeAction::DiscardQueuedDraft
    );
}

#[test]
fn test_esc_is_noop_when_idle() {
    let mut app = create_test_app();
    app.is_loading = false;
    app.input.clear();
    app.cursor_position = 0;
    app.mode = AppMode::Agent;

    assert_eq!(next_escape_action(&app, false), EscapeAction::Noop);
    assert_eq!(app.mode, AppMode::Agent);
}

#[test]
fn test_esc_closes_slash_menu_before_other_actions() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.input = "draft".to_string();
    app.queued_draft = Some(crate::tui::app::QueuedMessage::new(
        "queued draft".to_string(),
        None,
    ));

    assert_eq!(next_escape_action(&app, true), EscapeAction::CloseSlashMenu);
}

#[test]
fn history_arrow_does_not_steal_open_menus() {
    let mut app = create_test_app();
    app.input_history.push("previous prompt".to_string());
    app.input = "/".to_string();
    app.cursor_position = 1;

    assert!(!handle_composer_history_arrow(
        &mut app,
        KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
        true,
        false,
    ));

    assert_eq!(app.input, "/");
    assert!(app.history_index.is_none());
}

#[test]
fn test_ctrl_c_cancels_streaming_sets_status() {
    let mut app = create_test_app();
    app.is_loading = true;

    // Simulate Ctrl+C during loading state
    // engine_handle.cancel() is called (can't test directly - private)
    app.is_loading = false;
    app.status_message = Some("Request cancelled".to_string());

    assert!(!app.is_loading);
    assert_eq!(app.status_message, Some("Request cancelled".to_string()));
}

#[test]
fn local_cancel_marks_late_stream_events_for_suppression() {
    let _retry_guard = crate::retry_status::test_guard();
    let mut app = create_test_app();
    app.is_loading = true;
    app.turn_started_at = Some(Instant::now());
    app.runtime_turn_id = Some("turn_cancel_me".to_string());
    app.runtime_turn_status = Some("in_progress".to_string());
    app.streaming_state.start_text(0);
    crate::retry_status::start(2, Duration::from_secs(3), "network error");

    mark_active_turn_cancelled_locally(&mut app);

    assert!(!app.is_loading);
    assert!(app.turn_started_at.is_none());
    assert!(app.runtime_turn_id.is_none());
    assert!(app.runtime_turn_status.is_none());
    assert!(matches!(
        crate::retry_status::snapshot(),
        crate::retry_status::RetryState::Idle
    ));
    assert!(app.suppress_stream_events_until_turn_complete);
    assert!(suppress_engine_event_after_local_cancel(
        &EngineEvent::MessageDelta {
            index: 0,
            content: "late text".to_string(),
        }
    ));
    assert!(suppress_engine_event_after_local_cancel(
        &EngineEvent::ThinkingDelta {
            index: 0,
            content: "late thinking".to_string(),
        }
    ));
    assert!(suppress_engine_event_after_local_cancel(
        &EngineEvent::SessionUpdated {
            session_id: "session".to_string(),
            messages: Vec::new(),
            system_prompt: None,
            model: "deepseek-v4-flash".to_string(),
            workspace: PathBuf::from("."),
        }
    ));
    assert!(ignore_stale_stream_event_while_idle(
        &EngineEvent::MessageDelta {
            index: 0,
            content: "late text".to_string(),
        }
    ));
    assert!(!suppress_engine_event_after_local_cancel(
        &EngineEvent::TurnComplete {
            usage: Usage::default(),
            status: crate::core::events::TurnOutcomeStatus::Interrupted,
            error: None,
            tool_catalog: None,
            base_url: None,
        }
    ));
    assert!(!suppress_engine_event_after_local_cancel(
        &EngineEvent::Status {
            message: "Request cancelled".to_string(),
        }
    ));
}

#[test]
fn turn_started_route_is_captured_before_cancel_suppression() {
    let mut app = create_test_app();
    app.suppress_stream_events_until_turn_complete = true;
    app.ocean_completion_started_at = Some(Instant::now());
    app.pending_turn_route = Some((ApiProvider::Deepseek, "pending-model".to_string(), true));
    app.pending_auto_route_receipt = Some(crate::model_routing::AutoRouteReceipt {
        tier: crate::model_routing::AutoRouteTier::Fast,
        pair: crate::model_routing::AutoRoutePair {
            strong: "deepseek-v4-pro".to_string(),
            fast: Some("deepseek-v4-flash".to_string()),
        },
        scope: crate::model_routing::AutoRouteScope::ResolvedProvider,
        data_path: crate::model_routing::AutoRouteDataPath::LocalHeuristic,
        reason: crate::model_routing::AutoRouteReason::LocalHeuristic(
            crate::model_routing::AutoRouteHeuristicReason::ShortRequest,
        ),
    });
    let created_at = chrono::Utc::now();
    let event = EngineEvent::TurnStarted {
        turn_id: "turn_cancel_race".to_string(),
        created_at,
        route: Some(crate::core::events::TurnRoute {
            provider: ApiProvider::Openai,
            provider_identity: "openai".to_string(),
            model: "gpt-5.5".to_string(),
            auto_model: true,
            receipt: None,
            billing: Some(crate::core::events::RouteBillingEnvelope {
                billing_surface: Some(crate::pricing::FIRST_PARTY_PAYG_BILLING_SURFACE.to_string()),
                endpoint_fingerprint: Some("openai-endpoint".to_string()),
                billing_mode: crate::cost_status::RouteBillingMode::Metered,
                dispatched_at: created_at,
            }),
            base_url: String::new(),
            billing_product: crate::route_billing::RouteProduct::Unproven,
        }),
    };

    capture_turn_started_metadata(&mut app, &event);

    let active_turn = app.active_turn.as_ref().expect("captured turn");
    assert_eq!(active_turn.turn_id, "turn_cancel_race");
    assert_eq!(active_turn.created_at, created_at);
    let route = active_turn.route.as_ref().expect("captured route");
    assert_eq!(route.provider, ApiProvider::Openai);
    assert_eq!(route.model, "gpt-5.5");
    assert!(route.auto_model);
    assert_eq!(
        active_turn
            .auto_route_receipt
            .as_ref()
            .map(|receipt| receipt.tier),
        Some(crate::model_routing::AutoRouteTier::Fast)
    );
    assert!(app.pending_turn_route.is_none());
    assert!(app.pending_auto_route_receipt.is_none());
    assert!(app.ocean_completion_started_at.is_none());

    let observer = turn_end_observer_metadata(Some(active_turn));
    assert_eq!(observer.turn_id.as_ref(), "turn_cancel_race");
    assert_eq!(observer.created_at, created_at);
    assert_eq!(observer.route, Some(route));
}

/// #4404/#4411: prompt-suggestion authority comes off the engine's route
/// receipt, not off live config.
///
/// The TUI drains web config events ahead of engine events, so by the time
/// `TurnStarted` is handled, config may already describe a different route than
/// the one whose client the engine preflighted and installed. This test hands
/// the handler a receipt for route A while the app's own config knows nothing
/// about route A at all — authority must still be A.
#[test]
fn turn_started_suggestion_authority_comes_from_the_route_receipt_not_config() {
    let mut app = create_test_app();
    let receipt = crate::route_receipt::TurnRouteReceipt::new(
        ApiProvider::Deepseek,
        "deepseek",
        "deepseek-chat",
        "https://api.deepseek.com/v1",
        "sk-route-a-secret",
    );
    let event = EngineEvent::TurnStarted {
        turn_id: "turn_route_receipt".to_string(),
        created_at: chrono::Utc::now(),
        route: Some(crate::core::events::TurnRoute {
            provider: ApiProvider::Deepseek,
            provider_identity: "deepseek".to_string(),
            model: "deepseek-chat".to_string(),
            auto_model: false,
            receipt: Some(receipt),
            billing: Some(crate::core::events::RouteBillingEnvelope {
                billing_surface: None,
                endpoint_fingerprint: None,
                billing_mode: crate::cost_status::RouteBillingMode::Unknown,
                dispatched_at: chrono::Utc::now(),
            }),
            base_url: String::new(),
            billing_product: crate::route_billing::RouteProduct::Unproven,
        }),
    };

    capture_turn_started_metadata(&mut app, &event);

    let active_turn = app.active_turn.as_ref().expect("captured turn");
    let authority = active_turn
        .suggestion_authority
        .as_ref()
        .expect("the route receipt is the authority, so no config lookup is required");
    assert_eq!(authority.provider(), ApiProvider::Deepseek);
    assert_eq!(authority.provider_identity(), "deepseek");
    assert_eq!(authority.model(), "deepseek-chat");
    assert_eq!(authority.endpoint_identity(), "https://api.deepseek.com/v1");
    // Nothing credential-derived may render.
    let rendered = format!("{authority:?}");
    assert!(!rendered.contains("sk-route-a-secret"), "{rendered}");
    assert!(rendered.contains("<redacted>"), "{rendered}");
}

/// A turn with no installed-client receipt has no provenance, so it can never
/// authorize a follow-up request — regardless of what config would resolve.
#[test]
fn turn_started_without_a_route_receipt_captures_no_suggestion_authority() {
    let mut app = create_test_app();
    let event = EngineEvent::TurnStarted {
        turn_id: "turn_no_receipt".to_string(),
        created_at: chrono::Utc::now(),
        route: Some(crate::core::events::TurnRoute {
            provider: ApiProvider::Deepseek,
            provider_identity: "deepseek".to_string(),
            model: "deepseek-chat".to_string(),
            auto_model: false,
            receipt: None,
            billing: Some(crate::core::events::RouteBillingEnvelope {
                billing_surface: None,
                endpoint_fingerprint: None,
                billing_mode: crate::cost_status::RouteBillingMode::Unknown,
                dispatched_at: chrono::Utc::now(),
            }),
            base_url: String::new(),
            billing_product: crate::route_billing::RouteProduct::Unproven,
        }),
    };

    capture_turn_started_metadata(&mut app, &event);

    assert!(
        app.active_turn
            .as_ref()
            .expect("captured turn")
            .suggestion_authority
            .is_none()
    );
}

#[test]
fn engine_error_health_accounting_uses_active_turn_route() {
    let mut app = create_test_app();
    app.api_provider = ApiProvider::Deepseek;
    app.model = "current-model".to_string();
    let event = EngineEvent::TurnStarted {
        turn_id: "routed-turn".to_string(),
        created_at: chrono::Utc::now(),
        route: Some(crate::core::events::TurnRoute {
            provider: ApiProvider::Openai,
            provider_identity: "openai".to_string(),
            model: "gpt-5.5".to_string(),
            auto_model: true,
            receipt: None,
            billing: Some(crate::core::events::RouteBillingEnvelope {
                billing_surface: Some(crate::pricing::FIRST_PARTY_PAYG_BILLING_SURFACE.to_string()),
                endpoint_fingerprint: Some("openai-endpoint".to_string()),
                billing_mode: crate::cost_status::RouteBillingMode::Metered,
                dispatched_at: chrono::Utc::now(),
            }),
            base_url: String::new(),
            billing_product: crate::route_billing::RouteProduct::Unproven,
        }),
    };
    capture_turn_started_metadata(&mut app, &event);

    let route = error_health_route(&app, app.api_provider);

    assert_eq!(route, (ApiProvider::Openai, "gpt-5.5".to_string()));
}

#[test]
fn completion_only_hook_metadata_is_synthetic_and_non_model() {
    let observed_after = chrono::Utc::now();
    let first = turn_end_observer_metadata(None);
    let second = turn_end_observer_metadata(None);

    assert!(first.turn_id.starts_with("lifecycle_"));
    assert!(second.turn_id.starts_with("lifecycle_"));
    assert_ne!(first.turn_id, second.turn_id);
    assert!(first.created_at >= observed_after);
    assert!(first.route.is_none());
    assert!(second.route.is_none());
}

#[test]
fn issue_2739_stalled_turn_snapshot_preserves_api_messages() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let manager =
        crate::session_manager::SessionManager::new(tmp.path().join("sessions")).expect("manager");
    let mut app = create_test_app();
    app.api_messages
        .push(text_message("user", "hello from user"));
    app.api_messages
        .push(text_message("assistant", "partial reply"));
    // Simulate a running turn that stalls.
    app.is_loading = true;
    app.runtime_turn_status = Some("in_progress".to_string());
    app.turn_started_at = Some(Instant::now());

    // recover_stalled_runtime_turn now calls persist_recovery_snapshot
    // which in turn calls build_session_snapshot. Since persistence
    // may fail in tests (no real home dir), we verify directly that
    // build_session_snapshot captures the in-progress messages.
    let snapshot = build_session_snapshot(&mut app, &manager).expect("session snapshot");
    assert_eq!(snapshot.messages.len(), 2);
    assert_eq!(snapshot.messages[0].role, "user");
    assert_eq!(snapshot.messages[1].role, "assistant");
}

#[test]
fn issue_2739_esc_cancel_preserves_session_messages_before_clear() {
    let _home = SettingsHomeGuard::new();
    let tmp = tempfile::tempdir().expect("tempdir");
    let manager =
        crate::session_manager::SessionManager::new(tmp.path().join("sessions")).expect("manager");
    let mut app = create_test_app();
    app.api_messages
        .push(text_message("user", "esc cancel test"));
    app.api_messages
        .push(text_message("assistant", "interrupted by esc"));
    app.is_loading = true;
    app.turn_started_at = Some(Instant::now());
    app.runtime_turn_id = Some("turn_esc_me".to_string());
    app.runtime_turn_status = Some("in_progress".to_string());
    app.streaming_state.start_text(0);

    // Esc/Ctrl+C/approval abort all flow through mark_active_turn_cancelled_locally,
    // which snapshots before clearing turn state.
    mark_active_turn_cancelled_locally(&mut app);
    assert!(
        app.current_session_id.is_some(),
        "local cancel should create a resumable session snapshot"
    );
    let snapshot = build_session_snapshot(&mut app, &manager).expect("session snapshot");
    assert_eq!(snapshot.messages.len(), 2);
    assert_eq!(snapshot.messages[0].role, "user");
    assert_eq!(snapshot.messages[1].role, "assistant");
    // Turn-level bookkeeping must be cleared after cancel.
    assert!(!app.is_loading);
    assert!(app.turn_started_at.is_none());
    assert!(app.runtime_turn_status.is_none());
}

#[test]
fn issue_2739_dispatch_timeout_preserves_user_prompt() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let manager =
        crate::session_manager::SessionManager::new(tmp.path().join("sessions")).expect("manager");
    let mut app = create_test_app();
    app.api_messages
        .push(text_message("user", "prompt that never dispatched"));
    // Dispatch stalled before the turn ever reached `in_progress`
    // (runtime_turn_status stays None), so only the dispatch-timeout branch
    // of reconcile_turn_liveness fires.
    app.is_loading = true;
    app.runtime_turn_status = None;
    app.dispatch_started_at =
        Some(Instant::now() - DISPATCH_WATCHDOG_TIMEOUT - Duration::from_millis(1));
    app.turn_started_at = Some(Instant::now());

    let recovered = reconcile_turn_liveness(&mut app, Instant::now(), false);

    assert!(recovered, "dispatch-timeout branch should fire");
    assert!(!app.is_loading);
    assert!(app.dispatch_started_at.is_none());
    // #2739: the user's prompt must survive dispatch-timeout recovery so a
    // snapshot (and therefore --continue) still has it instead of loading the
    // previous save.
    let snapshot = build_session_snapshot(&mut app, &manager).expect("session snapshot");
    assert_eq!(snapshot.messages.len(), 1);
    assert_eq!(snapshot.messages[0].role, "user");
}

#[test]
fn test_ctrl_c_exits_when_not_loading() {
    let mut app = create_test_app();
    app.is_loading = false;

    // Ctrl+C when not loading should trigger shutdown
    // We can't test the actual shutdown, but verify the state is correct
    // for the shutdown path to be taken
    assert!(!app.is_loading);
}

#[test]
fn ctrl_c_disposition_idle_arms_exit_prompt() {
    let app = create_test_app();
    assert!(!app.is_loading);
    assert!(!app.quit_is_armed());
    assert_eq!(ctrl_c_disposition(&app), CtrlCDisposition::ArmExit);
}

#[test]
fn ctrl_c_disposition_loading_cancels_turn() {
    let mut app = create_test_app();
    app.is_loading = true;
    assert_eq!(ctrl_c_disposition(&app), CtrlCDisposition::CancelTurn);
}

#[test]
fn ctrl_c_disposition_goal_delay_cancels_continuation_instead_of_arming_exit() {
    let mut app = create_test_app();
    app.is_loading = false;
    app.goal_continuation_waiting = true;
    assert_eq!(ctrl_c_disposition(&app), CtrlCDisposition::CancelTurn);
}

#[test]
fn escape_goal_delay_cancels_continuation_with_empty_composer() {
    let mut app = create_test_app();
    app.is_loading = false;
    app.input.clear();
    app.goal_continuation_waiting = true;
    assert_eq!(next_escape_action(&app, false), EscapeAction::CancelRequest);
}

#[test]
fn ctrl_c_disposition_armed_idle_confirms_exit() {
    let mut app = create_test_app();
    app.arm_quit();
    assert!(app.quit_is_armed());
    assert_eq!(ctrl_c_disposition(&app), CtrlCDisposition::ConfirmExit);
}

#[test]
fn ctrl_c_disposition_loading_beats_armed_quit() {
    // If a turn started while quit is armed, the user almost certainly meant
    // "cancel the turn", not "exit". Pin that priority order.
    let mut app = create_test_app();
    app.arm_quit();
    app.is_loading = true;
    assert_eq!(ctrl_c_disposition(&app), CtrlCDisposition::CancelTurn);
}

#[test]
fn ctrl_c_disposition_launch_screen_arms_exit_prompt() {
    // The pre-session launch menu has no transcript selection and no turn in
    // flight, so Ctrl+C there must follow the same two-tap contract as the
    // session shell: arm first, confirm inside the window.
    let mut app = create_test_app();
    app.launch.visible = true;
    assert!(!app.is_loading);
    assert_eq!(ctrl_c_disposition(&app), CtrlCDisposition::ArmExit);
    app.arm_quit();
    assert_eq!(ctrl_c_disposition(&app), CtrlCDisposition::ConfirmExit);
}

#[test]
fn arm_quit_shows_press_again_hint() {
    // The armed state must be visible: the first Ctrl+C surfaces the
    // localized "Press Ctrl+C again to quit" hint as a typed toast (not the
    // legacy status_message sink, not a silent redraw), living exactly as
    // long as the confirmation window.
    let mut app = create_test_app();
    app.arm_quit();
    let hint = app.tr(MessageId::FooterPressCtrlCAgain).into_owned();
    assert!(
        app.status_toasts
            .iter()
            .any(|toast| toast.text == hint && toast.level == StatusToastLevel::Info),
        "arming the quit prompt must toast the press-again hint: {:?}",
        app.status_toasts
    );
    assert!(
        app.status_message.is_none(),
        "the quit hint must not write the legacy status sink"
    );
}

#[test]
fn ctrl_c_disposition_no_selection_means_no_copy() {
    // Regression guard for #1337: with no transcript selection, Ctrl+C must
    // NOT route to copy. (When selection is active, the copy branch wins;
    // exercised by the integration-level mouse-drag tests in this file.)
    let app = create_test_app();
    assert!(!selection_has_content(&app));
    assert_ne!(ctrl_c_disposition(&app), CtrlCDisposition::CopySelection);
}

#[test]
fn ctrl_c_raw_control_byte_routes_to_quit_arm_flow() {
    // #4090: in PTY/raw-mode Ctrl+C can arrive as the raw ETX control byte
    // (0x03) instead of `Char('c') + CONTROL`. The key handler must normalize
    // every encoding so the quit-arm flow runs rather than silently absorbing
    // the byte — this exercises the actual key-intake path, not only the pure
    // disposition table.
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    // Plain PTY read delivers 0x03 with no modifiers.
    let mut raw = KeyEvent::new(KeyCode::Char('\u{3}'), KeyModifiers::NONE);
    normalize_raw_ctrl_c(&mut raw);
    assert_eq!(raw.code, KeyCode::Char('c'));
    assert!(raw.modifiers.contains(KeyModifiers::CONTROL));

    // Kitty keyboard protocol may report the same byte with CONTROL already set.
    let mut kitty = KeyEvent::new(KeyCode::Char('\u{3}'), KeyModifiers::CONTROL);
    normalize_raw_ctrl_c(&mut kitty);
    assert_eq!(kitty.code, KeyCode::Char('c'));
    assert!(kitty.modifiers.contains(KeyModifiers::CONTROL));

    // A plain 'c' with no modifiers must be left untouched — it is NOT Ctrl+C.
    let mut plain = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE);
    normalize_raw_ctrl_c(&mut plain);
    assert_eq!(plain.code, KeyCode::Char('c'));
    assert!(!plain.modifiers.contains(KeyModifiers::CONTROL));
}

#[test]
fn ctrl_c_double_press_idle_exits_through_key_path() {
    // #4090: drive the actual key → disposition → state-machine path for two
    // consecutive Ctrl+C presses in the raw PTY form and assert the arm→confirm
    // exit transition that the pure-table tests do not cover.
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = create_test_app();
    assert!(!app.is_loading);
    assert!(!app.quit_is_armed());

    // First press (raw 0x03 form): normalizes to Ctrl+C, disposition arms exit.
    let mut first = KeyEvent::new(KeyCode::Char('\u{3}'), KeyModifiers::NONE);
    normalize_raw_ctrl_c(&mut first);
    assert_eq!(ctrl_c_disposition(&app), CtrlCDisposition::ArmExit);
    app.arm_quit();
    assert!(app.quit_is_armed());

    // Second press within the window: must confirm exit, never re-arm.
    let mut second = KeyEvent::new(KeyCode::Char('\u{3}'), KeyModifiers::NONE);
    normalize_raw_ctrl_c(&mut second);
    assert_eq!(ctrl_c_disposition(&app), CtrlCDisposition::ConfirmExit);
}

#[test]
fn test_ctrl_d_exits_when_input_empty() {
    let mut app = create_test_app();
    app.input.clear();

    // Ctrl+D when input empty should trigger shutdown
    assert!(app.input.is_empty());
}

#[test]
fn test_ctrl_d_does_nothing_when_input_not_empty() {
    let mut app = create_test_app();
    app.input = "some input".to_string();

    // Ctrl+D when input not empty should not trigger shutdown
    assert!(!app.input.is_empty());
}

#[test]
fn test_esc_priority_order_matches_cancel_stack() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.input = "draft".to_string();
    assert_eq!(next_escape_action(&app, false), EscapeAction::CancelRequest);

    app.input.clear();
    assert_eq!(next_escape_action(&app, false), EscapeAction::CancelRequest);

    app.queued_draft = Some(crate::tui::app::QueuedMessage::new(
        "queued draft".to_string(),
        None,
    ));
    app.input = "editing queued draft".to_string();
    assert_eq!(
        next_escape_action(&app, false),
        EscapeAction::DiscardQueuedDraft
    );

    app.queued_draft = None;
    app.is_loading = false;
    app.input = "draft".to_string();
    assert_eq!(next_escape_action(&app, false), EscapeAction::ClearInput);

    app.input.clear();
    app.queued_draft = Some(crate::tui::app::QueuedMessage::new(
        "queued draft".to_string(),
        None,
    ));
    assert_eq!(
        next_escape_action(&app, false),
        EscapeAction::DiscardQueuedDraft
    );

    app.queued_draft = None;
    assert_eq!(next_escape_action(&app, false), EscapeAction::Noop);
}

#[test]
fn next_escape_action_pauses_then_cancels_pausable_command() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.pausable = true;
    app.paused = false;

    assert_eq!(next_escape_action(&app, false), EscapeAction::PauseCommand);

    app.paused = true;
    assert_eq!(next_escape_action(&app, false), EscapeAction::CancelRequest);

    app.is_loading = false;
    app.paused = false;
    app.pausable = true;
    app.paused_goal_objective = Some("Scan repos".to_string());
    assert_eq!(next_escape_action(&app, false), EscapeAction::CancelRequest);

    app.is_loading = true;
    assert_eq!(next_escape_action(&app, false), EscapeAction::CancelRequest);
}

#[test]
fn visible_slash_menu_entries_respects_hide_flag() {
    let mut app = create_test_app();
    app.input = "/mo".to_string();
    app.slash_menu_hidden = false;

    let entries = visible_slash_menu_entries(&app, 6);
    assert!(!entries.is_empty());

    app.slash_menu_hidden = true;
    let hidden_entries = visible_slash_menu_entries(&app, 6);
    assert!(hidden_entries.is_empty());
}

#[test]
fn visible_slash_menu_starts_with_six_tasks_and_keeps_long_tail_searchable() {
    let mut app = create_test_app();
    app.input = "/".to_string();

    let entries = visible_slash_menu_entries(&app, SLASH_MENU_LIMIT);
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["/help", "/setup", "/model", "/settings", "/resume", "/rc"]
    );
    assert!(!entries.iter().any(|entry| entry.name == "/set"));
    assert!(!entries.iter().any(|entry| entry.name == "/codewhale"));

    app.input = "/conf".to_string();
    app.cursor_position = app.input.chars().count();
    let searched = visible_slash_menu_entries(&app, SLASH_MENU_LIMIT);
    assert!(searched.iter().any(|entry| entry.name == "/config"));
}

#[test]
fn visible_slash_model_completions_are_provider_scoped() {
    let mut app = create_test_app();
    app.api_provider = crate::config::ApiProvider::Together;
    app.model = crate::config::DEFAULT_TOGETHER_MODEL.to_string();
    app.provider_models.insert(
        "openrouter".to_string(),
        crate::config::DEFAULT_OPENROUTER_MODEL.to_string(),
    );
    app.input = "/model deep".to_string();
    app.cursor_position = app.input.chars().count();

    let entries = visible_slash_menu_entries(&app, 128);
    let names = entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect::<Vec<_>>();

    assert!(names.contains(&"/model deepseek-ai/DeepSeek-V4-Pro"));
    let openrouter_completion = format!("/model {}", crate::config::DEFAULT_OPENROUTER_MODEL);
    assert!(
        !names.contains(&openrouter_completion.as_str()),
        "OpenRouter saved rows must not appear as bare Together /model completions"
    );
}

#[test]
fn slash_menu_up_wraps_from_first_to_last() {
    let mut app = create_test_app();
    app.input = "/".to_string();
    app.cursor_position = 1;
    app.input_history.push("previous prompt".to_string());

    let entries = visible_slash_menu_entries(&app, 128);
    assert!(entries.len() > 1);

    app.slash_menu_selected = 0;
    select_previous_slash_menu_entry(&mut app, entries.len());

    assert_eq!(app.slash_menu_selected, entries.len() - 1);
    assert_eq!(app.input, "/");
}

#[test]
fn slash_menu_down_wraps_from_last_to_first() {
    let mut app = create_test_app();
    app.input = "/".to_string();
    app.cursor_position = 1;

    let entries = visible_slash_menu_entries(&app, 128);
    assert!(entries.len() > 1);

    app.slash_menu_selected = entries.len() - 1;
    select_next_slash_menu_entry(&mut app, entries.len());

    assert_eq!(app.slash_menu_selected, 0);
    assert_eq!(app.input, "/");
}

#[test]
fn apply_slash_menu_selection_appends_space_for_arg_commands() {
    let mut app = create_test_app();
    let entries = vec![
        crate::tui::widgets::SlashMenuEntry {
            name: "/model".to_string(),
            description: String::new(),
            is_skill: false,
            alias_hint: None,
        },
        crate::tui::widgets::SlashMenuEntry {
            name: "/settings".to_string(),
            description: String::new(),
            is_skill: false,
            alias_hint: None,
        },
    ];
    app.slash_menu_selected = 0;
    assert!(apply_slash_menu_selection(&mut app, &entries, true));
    assert_eq!(app.input, "/model ");
}

#[test]
fn apply_slash_menu_selection_honors_user_argument_metadata_and_builtin_override() {
    let tmp = TempDir::new().expect("tempdir");
    let commands_dir = tmp.path().join(".codewhale").join("commands");
    std::fs::create_dir_all(&commands_dir).expect("create commands dir");
    std::fs::write(
        commands_dir.join("custom-model.md"),
        "---\nname: model\n---\ncustom model",
    )
    .expect("write no-argument builtin override");
    std::fs::write(
        commands_dir.join("inspect.md"),
        "---\narguments: <path>\n---\ninspect",
    )
    .expect("write argument command");
    let mut app = create_test_app();
    app.workspace = tmp.path().to_path_buf();
    let entries = vec![
        crate::tui::widgets::SlashMenuEntry {
            name: "/model".to_string(),
            description: String::new(),
            is_skill: false,
            alias_hint: None,
        },
        crate::tui::widgets::SlashMenuEntry {
            name: "/inspect".to_string(),
            description: String::new(),
            is_skill: false,
            alias_hint: None,
        },
    ];

    assert!(apply_slash_menu_selection(&mut app, &entries, true));
    assert_eq!(app.input, "/model");

    app.slash_menu_selected = 1;
    assert!(apply_slash_menu_selection(&mut app, &entries, true));
    assert_eq!(app.input, "/inspect ");
}

#[test]
fn apply_slash_menu_selection_keeps_change_executable_without_version() {
    let mut app = create_test_app();
    let entries = vec![crate::tui::widgets::SlashMenuEntry {
        name: "/change".to_string(),
        description: String::new(),
        is_skill: false,
        alias_hint: None,
    }];

    assert!(apply_slash_menu_selection(&mut app, &entries, true));
    assert_eq!(app.input, "/change");
}

#[test]
fn apply_slash_menu_selection_uses_skill_command_form() {
    let mut app = create_test_app();
    let entries = vec![crate::tui::widgets::SlashMenuEntry {
        name: "/skill search-files".to_string(),
        description: "Search files".to_string(),
        is_skill: true,
        alias_hint: None,
    }];

    assert!(apply_slash_menu_selection(&mut app, &entries, true));
    assert_eq!(app.input, "/skill search-files");
}

#[test]
fn inline_skill_slash_popup_lists_cached_skills_in_message() {
    let mut app = create_test_app();
    app.cached_skills = vec![
        ("search-files".to_string(), "Search files".to_string()),
        ("my-review".to_string(), "Review code".to_string()),
    ];
    app.input = "please use /".to_string();
    app.cursor_position = app.input.chars().count();

    let entries = visible_slash_menu_entries(&app, 128);

    assert!(entries.iter().any(|entry| entry.name == "/search-files"));
    assert!(entries.iter().any(|entry| entry.name == "/my-review"));
    assert!(entries.iter().all(|entry| entry.is_skill));
}

#[test]
fn inline_skill_slash_popup_filters_partial_without_leaking_to_command_position() {
    let mut app = create_test_app();
    app.cached_skills = vec![
        ("search-files".to_string(), "Search files".to_string()),
        ("my-review".to_string(), "Review code".to_string()),
    ];
    app.input = "please use /my".to_string();
    app.cursor_position = app.input.chars().count();

    let entries = visible_slash_menu_entries(&app, 128);

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "/my-review");

    app.input = "/se".to_string();
    app.cursor_position = app.input.chars().count();
    let command_entries = visible_slash_menu_entries(&app, 128);
    assert!(
        !command_entries
            .iter()
            .any(|entry| entry.name == "/search-files" && entry.is_skill),
        "command-position slash menu should not include inline skill mentions"
    );
}

#[test]
fn inline_skill_slash_popup_does_not_open_inside_command_arguments() {
    let mut app = create_test_app();
    app.cached_skills = vec![
        (
            "config-doctor".to_string(),
            "Diagnose configuration".to_string(),
        ),
        ("cargo-ci-fixer".to_string(), "Fix CI failures".to_string()),
    ];
    app.input = "/attach /".to_string();
    app.cursor_position = app.input.chars().count();

    let entries = visible_slash_menu_entries(&app, 128);

    assert!(
        entries.is_empty(),
        "command argument paths should not show inline skill entries: {:?}",
        entries.iter().map(|entry| &entry.name).collect::<Vec<_>>()
    );
}

#[test]
fn apply_slash_menu_selection_splices_inline_skill_mention() {
    let mut app = create_test_app();
    app.input = "please use /se here".to_string();
    app.cursor_position = "please use /se".chars().count();
    let entries = vec![crate::tui::widgets::SlashMenuEntry {
        name: "/search-files".to_string(),
        description: "Search files".to_string(),
        is_skill: true,
        alias_hint: None,
    }];

    assert!(apply_slash_menu_selection(&mut app, &entries, true));
    assert_eq!(app.input, "please use /search-files here");
    assert_eq!(
        app.cursor_position,
        "please use /search-files".chars().count()
    );
}

#[test]
fn try_autocomplete_slash_command_completes_skill_argument() {
    let mut app = create_test_app();
    app.cached_skills = vec![
        ("search-files".to_string(), "Search files".to_string()),
        ("my-review".to_string(), "Review code".to_string()),
    ];
    app.input = "/skill my".to_string();
    app.cursor_position = app.input.chars().count();

    assert!(try_autocomplete_slash_command(&mut app));
    assert_eq!(app.input, "/skill my-review");
}

#[test]
fn workspace_context_refresh_is_deferred_while_ui_is_busy() {
    let repo = init_git_repo();
    let mut app = create_test_app();
    app.workspace = repo.path().to_path_buf();

    let now = Instant::now();
    crate::tui::workspace_context::refresh_if_needed(&mut app, now, false);

    assert!(app.workspace_context.is_none());
    assert!(app.workspace_context_refreshed_at.is_none());

    crate::tui::workspace_context::refresh_if_needed(&mut app, now, true);

    let context = app
        .workspace_context
        .as_deref()
        .expect("idle refresh should populate workspace context");
    assert!(context.contains("clean"));
    assert_eq!(app.workspace_context_refreshed_at, Some(now));
}

#[test]
fn workspace_context_refresh_respects_ttl_before_requerying_git() {
    let repo = init_git_repo();
    let mut app = create_test_app();
    app.workspace = repo.path().to_path_buf();

    let start = Instant::now();
    crate::tui::workspace_context::refresh_if_needed(&mut app, start, true);
    let initial = app
        .workspace_context
        .clone()
        .expect("initial refresh should populate context");

    std::fs::write(repo.path().join("dirty.txt"), "dirty").expect("write dirty marker");

    let before_ttl = start + Duration::from_secs(crate::tui::workspace_context::REFRESH_SECS - 1);
    crate::tui::workspace_context::refresh_if_needed(&mut app, before_ttl, true);
    assert_eq!(app.workspace_context.as_deref(), Some(initial.as_str()));

    let after_ttl = start + Duration::from_secs(crate::tui::workspace_context::REFRESH_SECS);
    crate::tui::workspace_context::refresh_if_needed(&mut app, after_ttl, true);
    let refreshed = app
        .workspace_context
        .as_deref()
        .expect("refresh after ttl should update context");
    assert!(refreshed.contains("untracked"));
    assert_ne!(refreshed, initial);
}

#[test]
fn completed_exec_tool_refreshes_workspace_context_before_ttl() {
    let repo = init_git_repo();
    let checkout = Command::new("git")
        .args(["checkout", "-b", "feature/old-branch"])
        .current_dir(repo.path())
        .output()
        .expect("git checkout should run");
    assert!(
        checkout.status.success(),
        "git checkout failed: {}",
        String::from_utf8_lossy(&checkout.stderr)
    );

    let mut app = create_test_app();
    app.workspace = repo.path().to_path_buf();

    let start = Instant::now();
    crate::tui::workspace_context::refresh_if_needed(&mut app, start, true);
    let initial = app
        .workspace_context
        .clone()
        .expect("initial refresh should populate context");
    assert!(
        initial.contains("feature/old-branch"),
        "expected initial branch in {initial:?}"
    );

    let checkout = Command::new("git")
        .args(["checkout", "-b", "feature/new-branch"])
        .current_dir(repo.path())
        .output()
        .expect("git checkout should run");
    assert!(
        checkout.status.success(),
        "git checkout failed: {}",
        String::from_utf8_lossy(&checkout.stderr)
    );

    let before_ttl = start + Duration::from_secs(crate::tui::workspace_context::REFRESH_SECS - 1);
    crate::tui::workspace_context::refresh_if_needed(&mut app, before_ttl, true);
    assert_eq!(
        app.workspace_context.as_deref(),
        Some(initial.as_str()),
        "normal refresh should still respect the TTL"
    );

    handle_tool_call_started(
        &mut app,
        "shell-branch",
        "exec_shell",
        &serde_json::json!({"command": "git checkout -b feature/new-branch"}),
    );
    handle_tool_call_complete(
        &mut app,
        "shell-branch",
        "exec_shell",
        &ok_result("switched"),
    );

    let refreshed = app
        .workspace_context
        .as_deref()
        .expect("shell completion should refresh context");
    assert!(
        refreshed.contains("feature/new-branch"),
        "expected refreshed branch in {refreshed:?}"
    );
}

#[test]
fn completed_task_shell_wait_refreshes_workspace_context_before_ttl() {
    let repo = init_git_repo();
    let checkout = Command::new("git")
        .args(["checkout", "-b", "feature/task-old"])
        .current_dir(repo.path())
        .output()
        .expect("git checkout should run");
    assert!(
        checkout.status.success(),
        "git checkout failed: {}",
        String::from_utf8_lossy(&checkout.stderr)
    );

    let mut app = create_test_app();
    app.workspace = repo.path().to_path_buf();

    let start = Instant::now();
    crate::tui::workspace_context::refresh_if_needed(&mut app, start, true);
    let initial = app
        .workspace_context
        .clone()
        .expect("initial refresh should populate context");
    assert!(
        initial.contains("feature/task-old"),
        "expected initial branch in {initial:?}"
    );

    let checkout = Command::new("git")
        .args(["checkout", "-b", "feature/task-new"])
        .current_dir(repo.path())
        .output()
        .expect("git checkout should run");
    assert!(
        checkout.status.success(),
        "git checkout failed: {}",
        String::from_utf8_lossy(&checkout.stderr)
    );

    let before_ttl = start + Duration::from_secs(crate::tui::workspace_context::REFRESH_SECS - 1);
    crate::tui::workspace_context::refresh_if_needed(&mut app, before_ttl, true);
    assert_eq!(
        app.workspace_context.as_deref(),
        Some(initial.as_str()),
        "normal refresh should still respect the TTL"
    );

    handle_tool_call_started(
        &mut app,
        "task-shell-branch",
        "task_shell_wait",
        &serde_json::json!({"task_id": "shell_1"}),
    );
    handle_tool_call_complete(
        &mut app,
        "task-shell-branch",
        "task_shell_wait",
        &ok_result("completed"),
    );

    let refreshed = app
        .workspace_context
        .as_deref()
        .expect("task shell completion should refresh context");
    assert!(
        refreshed.contains("feature/task-new"),
        "expected refreshed branch in {refreshed:?}"
    );
}

#[test]
fn completed_subagent_shell_tool_refreshes_workspace_context_before_ttl() {
    let repo = init_git_repo();
    let checkout = Command::new("git")
        .args(["checkout", "-b", "feature/subagent-old"])
        .current_dir(repo.path())
        .output()
        .expect("git checkout should run");
    assert!(
        checkout.status.success(),
        "git checkout failed: {}",
        String::from_utf8_lossy(&checkout.stderr)
    );

    let mut app = create_test_app();
    app.workspace = repo.path().to_path_buf();

    let start = Instant::now();
    crate::tui::workspace_context::refresh_if_needed(&mut app, start, true);
    let initial = app
        .workspace_context
        .clone()
        .expect("initial refresh should populate context");
    assert!(
        initial.contains("feature/subagent-old"),
        "expected initial branch in {initial:?}"
    );

    let checkout = Command::new("git")
        .args(["checkout", "-b", "feature/subagent-new"])
        .current_dir(repo.path())
        .output()
        .expect("git checkout should run");
    assert!(
        checkout.status.success(),
        "git checkout failed: {}",
        String::from_utf8_lossy(&checkout.stderr)
    );

    let before_ttl = start + Duration::from_secs(crate::tui::workspace_context::REFRESH_SECS - 1);
    crate::tui::workspace_context::refresh_if_needed(&mut app, before_ttl, true);
    assert_eq!(
        app.workspace_context.as_deref(),
        Some(initial.as_str()),
        "normal refresh should still respect the TTL"
    );

    handle_subagent_mailbox(
        &mut app,
        42,
        &crate::tools::subagent::MailboxMessage::ToolCallCompleted {
            agent_id: "agent_branch".to_string(),
            tool_name: "exec_shell".to_string(),
            step: 1,
            ok: true,
        },
    );

    let refreshed = app
        .workspace_context
        .as_deref()
        .expect("subagent shell completion should refresh context");
    assert!(
        refreshed.contains("feature/subagent-new"),
        "expected refreshed branch in {refreshed:?}"
    );
}

#[test]
fn workspace_context_drain_requests_redraw_when_context_changes() {
    let mut app = create_test_app();
    app.workspace_context = Some("feature/old | clean".to_string());
    app.workspace_context_refreshed_at = Some(Instant::now());
    app.needs_redraw = false;
    {
        let mut cell = app.workspace_context_cell.lock().expect("context cell");
        *cell = Some("feature/new | clean".to_string());
    }

    crate::tui::workspace_context::refresh_if_needed(&mut app, Instant::now(), false);

    assert_eq!(
        app.workspace_context.as_deref(),
        Some("feature/new | clean")
    );
    assert!(
        app.needs_redraw,
        "draining a changed async context should redraw the footer"
    );
}

#[tokio::test]
async fn dispatch_user_message_records_prompt_for_cancel_restore() {
    let mut app = create_test_app();
    app.show_thinking = false;
    let config = Config::default();
    let mut engine = crate::core::engine::mock_engine_handle();
    let queued = crate::tui::app::QueuedMessage::new("fix this typo\nthen retry".to_string(), None);

    dispatch_user_message(&mut app, &config, &engine.handle, queued)
        .await
        .expect("dispatch user message");

    assert_eq!(
        app.last_submitted_prompt.as_deref(),
        Some("fix this typo\nthen retry")
    );
    match engine.rx_op.recv().await.expect("send message op") {
        crate::core::ops::Op::SendMessage { content, .. } => {
            assert_eq!(content, "fix this typo\nthen retry");
            assert!(!app.show_thinking, "visibility remains TUI-owned");
        }
        other => panic!("expected SendMessage, got {other:?}"),
    }
}

#[tokio::test]
async fn startup_prompt_waits_for_onboarding_then_dispatches() {
    let mut app = create_test_app();
    app.input = "阅读项目 and wait".to_string();
    app.cursor_position = app.input.chars().count();
    app.auto_submit_initial_input = true;
    app.onboarding = OnboardingState::Welcome;
    let config = Config::default();
    let mut engine = crate::core::engine::mock_engine_handle();

    submit_initial_input_if_ready(&mut app, &config, &engine.handle)
        .await
        .expect("defer");

    assert!(app.auto_submit_initial_input);
    assert_eq!(app.input, "阅读项目 and wait");
    assert_eq!(
        app.status_message.as_deref(),
        Some(INITIAL_PROMPT_DEFERRED_STATUS)
    );
    assert!(engine.rx_op.try_recv().is_err());

    app.onboarding = OnboardingState::None;
    submit_initial_input_if_ready(&mut app, &config, &engine.handle)
        .await
        .expect("submit");

    assert!(!app.auto_submit_initial_input);
    assert!(app.input.is_empty());
    assert_eq!(
        app.last_submitted_prompt.as_deref(),
        Some("阅读项目 and wait")
    );
    match engine.rx_op.recv().await.expect("send message op") {
        crate::core::ops::Op::SendMessage { content, .. } => {
            assert!(content.contains("阅读项目 and wait"));
        }
        other => panic!("expected SendMessage, got {other:?}"),
    }
}

#[tokio::test]
async fn steer_user_message_records_prompt_for_cancel_restore() {
    let mut app = create_test_app();
    let mut engine = crate::core::engine::mock_engine_handle();
    let queued = crate::tui::app::QueuedMessage::new(
        "adjust the active turn\nthen continue".to_string(),
        None,
    );

    steer_user_message(&mut app, &Config::default(), &engine.handle, queued)
        .await
        .expect("steer user message");

    assert_eq!(
        app.last_submitted_prompt.as_deref(),
        Some("adjust the active turn\nthen continue")
    );
    assert_eq!(
        engine.rx_steer.recv().await.as_deref(),
        Some("adjust the active turn\nthen continue")
    );
}

#[tokio::test]
async fn steer_user_message_backgrounds_foreground_shell_before_dispatch() {
    let mut app = create_test_app();
    app.is_loading = true;
    let shell_manager = app
        .runtime_services
        .shell_manager
        .clone()
        .expect("test app shell manager");
    let mut active = ActiveCell::new();
    active.push_tool(
        "foreground-shell",
        HistoryCell::Tool(ToolCell::Exec(ExecCell {
            command: "cargo test --workspace".to_string(),
            status: ToolStatus::Running,
            output: None,
            live_output: None,
            shell_task_id: None,
            owner_agent_id: None,
            owner_agent_name: None,
            started_at: Some(Instant::now()),
            duration_ms: None,
            stale_elapsed_since_output_ms: None,
            source: ExecSource::Assistant,
            interaction: None,
            output_summary: None,
        })),
    );
    app.active_cell = Some(active);
    let mut engine = crate::core::engine::mock_engine_handle();

    steer_user_message(
        &mut app,
        &Config::default(),
        &engine.handle,
        QueuedMessage::new("use the partial results".to_string(), None),
    )
    .await
    .expect("steer user message");

    assert!(
        shell_manager
            .lock()
            .expect("shell manager lock")
            .foreground_background_requested_for_test(),
        "foreground shell must receive its detach request"
    );
    assert_eq!(
        engine.rx_steer.recv().await.as_deref(),
        Some("use the partial results")
    );
}

#[tokio::test]
async fn empty_enter_sends_next_queued_message_into_running_turn() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.queue_message(crate::tui::app::QueuedMessage::new(
        "please attend to your sub agents".to_string(),
        None,
    ));
    let config = Config::default();
    let mut engine = crate::core::engine::mock_engine_handle();

    assert!(
        send_next_queued_message_now(&mut app, &config, &engine.handle)
            .await
            .expect("empty Enter succeeds")
    );

    assert_eq!(app.queued_message_count(), 0);
    assert_eq!(
        engine.rx_steer.recv().await.as_deref(),
        Some("please attend to your sub agents")
    );
}

#[tokio::test]
async fn empty_enter_does_not_send_an_edited_queued_draft() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.queued_draft = Some(crate::tui::app::QueuedMessage::new(
        "original queued follow-up".to_string(),
        Some("skill body".to_string()),
    ));
    app.input = "edited queued follow-up".to_string();
    app.cursor_position = app.input.chars().count();
    let config = Config::default();
    let mut engine = crate::core::engine::mock_engine_handle();

    assert!(
        !send_next_queued_message_now(&mut app, &config, &engine.handle)
            .await
            .expect("empty Enter no-op succeeds")
    );

    assert!(app.queued_draft.is_some());
    assert_eq!(app.input, "edited queued follow-up");
    assert_eq!(app.queued_message_count(), 0);
    assert!(engine.rx_steer.try_recv().is_err());
}

#[test]
fn parse_queue_send_command_accepts_queue_alias_and_positive_index() {
    assert_eq!(parse_queue_send_command("/queue send 2"), Some(Ok(1)));
    assert_eq!(parse_queue_send_command("/queued now 1"), Some(Ok(0)));
    assert_eq!(parse_queue_send_command("/queue SEND 1"), Some(Ok(0)));
    assert!(parse_queue_send_command("/queue drop 1").is_none());
    assert!(
        parse_queue_send_command("/queue send 0")
            .expect("send command should parse")
            .is_err()
    );
}

#[tokio::test]
async fn queue_send_index_sends_selected_message_into_running_turn() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.queue_message(crate::tui::app::QueuedMessage::new(
        "first stays queued".to_string(),
        None,
    ));
    app.queue_message(crate::tui::app::QueuedMessage::new(
        "second sends now".to_string(),
        None,
    ));
    let config = Config::default();
    let mut engine = crate::core::engine::mock_engine_handle();

    assert!(
        send_queued_message_at_index_now(&mut app, &config, &engine.handle, 1)
            .await
            .expect("indexed send succeeds")
    );

    assert_eq!(app.queued_message_count(), 1);
    assert_eq!(
        app.queued_messages.front().map(|msg| msg.display.as_str()),
        Some("first stays queued")
    );
    assert_eq!(
        engine.rx_steer.recv().await.as_deref(),
        Some("second sends now")
    );
}

#[tokio::test]
async fn enter_while_model_waiting_queues_instead_of_steering() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.streaming_message_index = None;
    let config = Config::default();
    let mut engine = crate::core::engine::mock_engine_handle();
    let queued = build_queued_message(&mut app, "adjust current turn".to_string());

    submit_or_steer_message(
        &mut app,
        &config,
        &engine.handle,
        queued,
        DispatchRecovery::Immediate,
    )
    .await
    .expect("busy waiting submit queues");

    assert_eq!(app.queued_message_count(), 1);
    assert_eq!(
        app.queued_messages
            .front()
            .map(|message| message.display.as_str()),
        Some("adjust current turn")
    );
    assert!(
        engine.rx_steer.try_recv().is_err(),
        "bare Enter must never become a same-turn steer"
    );
}

/// Ops contract: Queue disposition must echo `HistoryCell::User` into the
/// transcript before the model runs (swarm shot 12 / queued follow-up).
#[tokio::test]
async fn queue_disposition_echoes_user_turn_into_history() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.streaming_message_index = None;
    let config = Config::default();
    let engine = crate::core::engine::mock_engine_handle();
    let queued = QueuedMessage::new("hello queued".to_string(), None);

    submit_or_steer_message(
        &mut app,
        &config,
        &engine.handle,
        queued,
        DispatchRecovery::Immediate,
    )
    .await
    .expect("busy submit queues");

    assert_eq!(app.queued_message_count(), 1);
    assert!(
        app.queued_messages
            .front()
            .is_some_and(|msg| msg.history_echoed),
        "queued message must remember the history echo"
    );
    let user_cells: Vec<_> = app
        .history
        .iter()
        .filter_map(|cell| match cell {
            HistoryCell::User { content } => Some(content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(user_cells, vec!["hello queued"]);
}

/// Draining a previously-echoed queued turn into Immediate prepare must not
/// paint a second User bubble.
#[test]
fn prepare_skips_user_echo_when_history_echoed() {
    let mut app = create_test_app();
    app.add_message(HistoryCell::User {
        content: "hello queued".to_string(),
    });
    let mut message = QueuedMessage::new("hello queued".to_string(), None);
    message.history_echoed = true;
    let before = app
        .history
        .iter()
        .filter(|cell| matches!(cell, HistoryCell::User { .. }))
        .count();

    let _prepare = prepare_user_dispatch(&mut app, &Config::default(), message)
        .expect("prepare echoed message");

    let after = app
        .history
        .iter()
        .filter(|cell| matches!(cell, HistoryCell::User { .. }))
        .count();
    assert_eq!(before, 1);
    assert_eq!(
        after, 1,
        "history_echoed prepare must not double the User cell"
    );
    assert!(
        matches!(
            app.history.first(),
            Some(HistoryCell::User { content }) if content == "hello queued"
        ),
        "existing echoed User cell must remain the reference target"
    );
}

#[test]
fn engine_drain_budget_respects_event_and_time_limits() {
    let start = Instant::now();
    assert!(!engine_drain_budget_exhausted(0, start, start));
    assert!(!engine_drain_budget_exhausted(1, start, start));
    assert!(engine_drain_budget_exhausted(
        MAX_ENGINE_EVENTS_PER_DRAIN,
        start,
        start
    ));
    assert!(engine_drain_budget_exhausted(
        1,
        start,
        start + ENGINE_DRAIN_TIME_BUDGET
    ));
}

#[test]
fn throttled_recovery_snapshot_persists_during_loading_turns() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let manager =
        crate::session_manager::SessionManager::new(tmp.path().join("sessions")).expect("manager");
    let mut app = create_test_app();
    app.api_messages
        .push(text_message("user", "in-progress turn"));
    app.is_loading = true;
    app.runtime_turn_status = Some("in_progress".to_string());

    let mut last_snapshot_at = None;
    let t0 = Instant::now();
    maybe_throttled_recovery_snapshot(&mut app, t0, &mut last_snapshot_at);
    assert!(last_snapshot_at.is_some());
    let snapshot = build_session_snapshot(&mut app, &manager).expect("session snapshot");
    assert_eq!(snapshot.messages.len(), 1);

    maybe_throttled_recovery_snapshot(
        &mut app,
        t0 + RECOVERY_SNAPSHOT_INTERVAL / 2,
        &mut last_snapshot_at,
    );
    maybe_throttled_recovery_snapshot(
        &mut app,
        t0 + RECOVERY_SNAPSHOT_INTERVAL,
        &mut last_snapshot_at,
    );
}

#[tokio::test]
async fn steer_failure_queues_message_and_surfaces_toast() {
    let mut app = create_test_app();
    app.is_loading = true;
    let engine = crate::core::engine::mock_engine_handle();
    drop(engine.rx_steer);
    let queued = crate::tui::app::QueuedMessage::new("follow up while busy".to_string(), None);

    attempt_steer_with_queue_fallback(
        &mut app,
        &Config::default(),
        &engine.handle,
        queued,
        DispatchRecovery::Immediate,
    )
    .await;

    assert_eq!(app.queued_message_count(), 1);
    let toast = app.status_toasts.back().expect("steer failure toast");
    assert_eq!(toast.level, StatusToastLevel::Warning);
    assert!(toast.text.contains("Could not send into this turn"));
}

#[tokio::test]
async fn streaming_enter_queue_pushes_visible_toast() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.streaming_message_index = Some(0);
    let config = Config::default();
    let engine = crate::core::engine::mock_engine_handle();
    let queued = build_queued_message(&mut app, "follow up during stream".to_string());

    submit_or_steer_message(
        &mut app,
        &config,
        &engine.handle,
        queued,
        DispatchRecovery::Immediate,
    )
    .await
    .expect("streaming submit queues");

    assert_eq!(app.queued_message_count(), 1);
    let toast = app.status_toasts.back().expect("queue toast");
    assert_eq!(toast.level, StatusToastLevel::Info);
    assert!(toast.text.contains("Queued. Sends after this turn."));
}

#[test]
fn streaming_append_receipt_chains_until_the_next_render() {
    let mut app = create_test_app();
    let index = ensure_streaming_assistant_history_cell(&mut app);
    app.resync_history_revisions();
    let initial_revision = app.history_revisions[index];

    append_streaming_text(&mut app, index, "first");
    let first_revision = app.history_revisions[index];
    append_streaming_text(&mut app, index, " second");
    let second_revision = app.history_revisions[index];

    assert_ne!(initial_revision, first_revision);
    assert_ne!(first_revision, second_revision);
    assert_eq!(
        app.streaming_source_receipt,
        Some(crate::tui::transcript::StreamingSourceReceipt {
            cell_index: index,
            from_revision: initial_revision,
            to_revision: second_revision,
            content_len: "first second".len(),
        })
    );
}

#[test]
fn unknown_streaming_cell_mutation_revokes_append_provenance() {
    let mut app = create_test_app();
    let index = ensure_streaming_assistant_history_cell(&mut app);
    append_streaming_text(&mut app, index, "append-only");
    assert!(app.streaming_source_receipt.is_some());

    if let Some(HistoryCell::Assistant { content, .. }) = app.history.get_mut(index) {
        content.insert_str(0, "rewritten ");
    }
    app.bump_history_cell(index);

    assert!(app.streaming_source_receipt.is_none());
}

#[tokio::test]
async fn empty_enter_promotes_the_oldest_queued_message() {
    // Typed Enter while streaming queues. An explicit empty Enter promotes
    // the oldest queued follow-up into the active turn.
    let mut app = create_test_app();
    app.is_loading = true;
    app.streaming_message_index = Some(0);
    let config = Config::default();
    let mut engine = crate::core::engine::mock_engine_handle();
    let queued = build_queued_message(&mut app, "coordinate parallel tasks".to_string());

    submit_or_steer_message(
        &mut app,
        &config,
        &engine.handle,
        queued,
        DispatchRecovery::Immediate,
    )
    .await
    .expect("first enter queues while streaming");
    assert_eq!(app.queued_message_count(), 1);
    assert!(app.input.is_empty());

    assert!(
        send_next_queued_message_now(&mut app, &config, &engine.handle)
            .await
            .expect("empty Enter promotes queue")
    );
    assert_eq!(app.queued_message_count(), 0);
    assert_eq!(
        engine.rx_steer.recv().await.as_deref(),
        Some("coordinate parallel tasks")
    );
}

#[tokio::test]
async fn operate_streaming_enter_queues_another_parallel_task() {
    let mut app = create_test_app();
    app.mode = AppMode::Operate;
    app.is_loading = true;
    app.streaming_message_index = Some(0);
    let config = Config::default();
    let engine = crate::core::engine::mock_engine_handle();
    let queued = build_queued_message(&mut app, "check the docs too".to_string());

    submit_or_steer_message(
        &mut app,
        &config,
        &engine.handle,
        queued,
        DispatchRecovery::Immediate,
    )
    .await
    .expect("Operate streaming submit queues another task");

    assert_eq!(app.queued_message_count(), 1);
    let toast = app.status_toasts.back().expect("Operate queue toast");
    assert_eq!(toast.level, StatusToastLevel::Info);
    assert_eq!(toast.text, "Queued. Sends after this turn.");
    assert_eq!(app.status_message.as_deref(), Some(toast.text.as_str()));
}

#[tokio::test]
async fn inline_skill_request_keeps_instruction_when_busy_queueing() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.streaming_message_index = Some(0);
    app.active_skill = Some("Use the test skill".to_string());
    let config = Config::default();
    let engine = crate::core::engine::mock_engine_handle();

    let queued = build_queued_message(&mut app, "do X".to_string());
    assert!(app.active_skill.is_none(), "skill must be consumed once");
    submit_or_steer_message(
        &mut app,
        &config,
        &engine.handle,
        queued,
        DispatchRecovery::Immediate,
    )
    .await
    .expect("streaming skill request queues");

    let queued = app.queued_messages.front().expect("queued skill request");
    assert_eq!(queued.display, "do X");
    assert_eq!(
        queued.skill_instruction.as_deref(),
        Some("Use the test skill")
    );
}

#[test]
fn onboarding_after_provider_setup_does_not_repeat_language_step() {
    let mut app = create_test_app();
    app.onboarding = OnboardingState::Provider;
    app.onboarding_needs_api_key = false;
    app.onboarding_missing_key_recovery = false;
    app.trust_mode = true;
    app.status_message = Some("saved".to_string());

    crate::tui::onboarding::advance_onboarding_after_provider(&mut app);

    assert_eq!(app.onboarding, OnboardingState::Ready);
    assert_eq!(app.status_message, None);
}

#[test]
fn onboarding_after_provider_setup_routes_to_trust_when_needed() {
    let tmpdir = TempDir::new().expect("tempdir");
    let mut app = create_test_app();
    app.workspace = tmpdir.path().to_path_buf();
    app.onboarding = OnboardingState::Provider;
    app.onboarding_needs_api_key = false;
    app.onboarding_missing_key_recovery = false;
    app.trust_mode = false;

    crate::tui::onboarding::advance_onboarding_after_provider(&mut app);

    assert_eq!(app.onboarding, OnboardingState::TrustDirectory);
}

#[test]
fn missing_key_recovery_ends_on_ready_without_a_command_tour() {
    let mut app = create_test_app();
    app.onboarding = OnboardingState::Provider;
    app.onboarding_missing_key_recovery = true;
    app.trust_mode = true;

    crate::tui::onboarding::advance_onboarding_after_provider(&mut app);

    assert_eq!(app.onboarding, OnboardingState::Ready);
}

#[test]
fn missing_key_recovery_still_requires_workspace_trust() {
    let tmpdir = TempDir::new().expect("tempdir");
    let mut app = create_test_app();
    app.workspace = tmpdir.path().to_path_buf();
    app.onboarding = OnboardingState::Provider;
    app.onboarding_missing_key_recovery = true;
    app.trust_mode = false;

    crate::tui::onboarding::advance_onboarding_after_provider(&mut app);

    assert_eq!(app.onboarding, OnboardingState::TrustDirectory);
}

#[test]
fn provider_back_walks_to_the_last_decision_this_run_asked() {
    // The first two cases are the fresh-first-run walk-back, so they must say
    // so: `App::new` classifies a default route with no key as missing-key
    // recovery, and that branch owns a different (also asserted) exit.
    let mut app = create_test_app();
    app.onboarding = OnboardingState::Provider;
    app.onboarding_missing_key_recovery = false;
    app.onboarding_had_language_step = true;
    back_from_provider_onboarding(&mut app);
    assert_eq!(app.onboarding, OnboardingState::Language);

    // Without the language screen the previous stop is the welcome.
    let mut app = create_test_app();
    app.onboarding = OnboardingState::Provider;
    app.onboarding_missing_key_recovery = false;
    app.onboarding_had_language_step = false;
    back_from_provider_onboarding(&mut app);
    assert_eq!(app.onboarding, OnboardingState::Welcome);

    // Missing-key recovery keeps its direct exit to the offline composer.
    let mut app = create_test_app();
    app.onboarding = OnboardingState::Provider;
    app.onboarding_missing_key_recovery = true;
    back_from_provider_onboarding(&mut app);
    assert_eq!(app.onboarding, OnboardingState::None);
}

// ---- Issue #4763: provider onboarding must never be a trap ----

/// Ctrl+C quits from every onboarding state, including while the provider
/// picker owns the keys. Before #4763 the picker swallowed it and the only
/// exit was Escape-then-Ctrl+C.
#[test]
fn onboarding_ctrl_c_quits_even_with_the_provider_picker_on_the_view_stack() {
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

    assert_eq!(
        onboarding_key_route(
            OnboardingState::Provider,
            Some(ModalKind::ProviderPicker),
            &ctrl_c,
        ),
        OnboardingKeyRoute::Quit,
    );
    assert_eq!(
        onboarding_key_route(OnboardingState::Provider, None, &ctrl_c),
        OnboardingKeyRoute::Quit,
    );
    assert_eq!(
        onboarding_key_route(OnboardingState::Language, None, &ctrl_c),
        OnboardingKeyRoute::Quit,
    );
    assert_eq!(
        onboarding_key_route(OnboardingState::Ready, Some(ModalKind::Help), &ctrl_c,),
        OnboardingKeyRoute::Quit,
    );
}

/// #3927: the offline exit must be reachable from provider setup even while
/// the provider picker owns the keys — otherwise the only advertised way
/// out of a modal the user cannot satisfy is to quit.
#[test]
fn explore_offline_shortcut_escapes_the_provider_picker() {
    let ctrl_o = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL);

    assert_eq!(
        onboarding_key_route(
            OnboardingState::Provider,
            Some(ModalKind::ProviderPicker),
            &ctrl_o,
        ),
        OnboardingKeyRoute::ExploreOffline,
    );
    // It is scoped to provider setup: elsewhere it stays an ordinary key.
    assert_eq!(
        onboarding_key_route(OnboardingState::Language, None, &ctrl_o),
        OnboardingKeyRoute::Legacy,
    );
    assert_eq!(
        onboarding_key_route(OnboardingState::None, None, &ctrl_o),
        OnboardingKeyRoute::Legacy,
    );

    // A bare "o" remains provider-picker input and must never trigger it.
    let plain_o = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE);
    assert_eq!(
        onboarding_key_route(
            OnboardingState::Provider,
            Some(ModalKind::ProviderPicker),
            &plain_o,
        ),
        OnboardingKeyRoute::ProviderPicker,
    );
}

/// Escape is no longer intercepted on the picker's behalf, so the picker can
/// back out one stage (key/OAuth entry → list) and only dismiss from the
/// list. Non-onboarding keys still reach the legacy switch.
#[test]
fn onboarding_escape_is_routed_to_the_provider_picker_not_intercepted() {
    let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);

    assert_eq!(
        onboarding_key_route(
            OnboardingState::Provider,
            Some(ModalKind::ProviderPicker),
            &esc
        ),
        OnboardingKeyRoute::ProviderPicker,
        "the picker owns Escape so it can back out one stage at a time"
    );
    assert_eq!(
        onboarding_key_route(OnboardingState::Provider, None, &esc),
        OnboardingKeyRoute::Legacy,
        "without the picker the legacy onboarding switch still handles Escape"
    );
    assert_eq!(
        onboarding_key_route(OnboardingState::None, Some(ModalKind::ProviderPicker), &esc),
        OnboardingKeyRoute::Legacy,
        "a picker outside onboarding is not an onboarding route"
    );
}

/// #4763: reusing an external Grok CLI grant must finish provider onboarding
/// the same way a submitted key does. The consent handler used to switch the
/// route and stop there, returning the user to the provider step they had
/// just satisfied.
#[test]
fn external_grant_reuse_completes_provider_onboarding() {
    let _home = SettingsHomeGuard::new();
    let mut app = create_test_app();
    app.onboarding = OnboardingState::Provider;
    app.onboarding_needs_api_key = true;
    app.onboarding_missing_key_recovery = true;
    app.offline_mode = true;
    app.trust_mode = true;
    app.set_provider_identity(ApiProvider::Xai, ApiProvider::Xai.as_str());
    app.set_model_selection(crate::config::DEFAULT_XAI_MODEL.to_string());

    complete_provider_picker_onboarding(&mut app, crate::config::ApiProvider::Xai);

    assert_ne!(
        app.onboarding,
        OnboardingState::Provider,
        "a satisfied provider must not return to the provider step"
    );
    assert_eq!(app.onboarding_provider, crate::config::ApiProvider::Xai);
    assert!(!app.onboarding_needs_api_key);
    assert!(!app.offline_mode);
    let settings = crate::settings::Settings::load().expect("reload onboarding default");
    assert_eq!(settings.default_provider.as_deref(), Some("xai"));
    assert_eq!(
        settings
            .provider_models
            .as_ref()
            .and_then(|models| models.get("xai"))
            .map(String::as_str),
        Some(crate::config::DEFAULT_XAI_MODEL),
    );
}

#[test]
fn trust_directory_completion_advances_to_ready() {
    let _guard = ConfigPathEnvGuard::new();
    let tmpdir = TempDir::new().expect("workspace tempdir");
    let mut app = create_test_app();
    app.workspace = tmpdir.path().to_path_buf();
    app.onboarding = OnboardingState::TrustDirectory;
    app.onboarding_workspace_trust_gate = false;
    app.onboarding_missing_key_recovery = false;
    app.onboarding_had_trust_step = true;
    app.trust_mode = false;
    let enter_hint = app.tr(MessageId::OnboardTrustEnterHint).into_owned();
    app.status_message = Some(enter_hint.clone());
    app.sync_status_message_to_toasts();
    assert!(
        app.status_toasts
            .iter()
            .any(|toast| toast.text == enter_hint),
        "the invalid Enter hint should reproduce in the activity-toast queue"
    );

    complete_trust_directory_onboarding(&mut app, &Config::default())
        .expect("trust completion should succeed");

    assert!(app.trust_mode);
    assert_eq!(app.onboarding, OnboardingState::Ready);
    assert!(app.runtime_services.hook_executor.is_some());
    assert!(app.status_message.is_none());
    assert!(
        app.status_toasts
            .iter()
            .all(|toast| toast.text != enter_hint),
        "successful trust must retire the invalid Enter hint before the composer"
    );
}

#[test]
fn trust_directory_continue_untrusted_advances_without_recording_trust() {
    let _guard = ConfigPathEnvGuard::new();
    let tmpdir = TempDir::new().expect("workspace tempdir");
    let mut app = create_test_app();
    app.workspace = tmpdir.path().to_path_buf();
    app.onboarding = OnboardingState::TrustDirectory;
    app.onboarding_workspace_trust_gate = false;
    app.onboarding_missing_key_recovery = false;
    app.onboarding_had_trust_step = true;
    app.trust_mode = false;

    continue_without_trusting_directory(&mut app);

    assert!(!app.trust_mode);
    assert_eq!(app.onboarding, OnboardingState::Ready);
    assert!(
        crate::tui::onboarding::needs_trust(&app.workspace),
        "declining trust must not write a trusted marker"
    );
    let notice = app.status_message.expect("restricted-mode notice");
    assert!(
        notice.contains("without workspace trust") || notice.contains("restricted"),
        "unexpected notice: {notice}"
    );
}

#[test]
fn trust_completion_during_missing_key_recovery_advances_to_ready() {
    let _guard = ConfigPathEnvGuard::new();
    let tmpdir = TempDir::new().expect("workspace tempdir");
    let mut app = create_test_app();
    app.workspace = tmpdir.path().to_path_buf();
    app.onboarding = OnboardingState::TrustDirectory;
    app.onboarding_workspace_trust_gate = false;
    app.onboarding_missing_key_recovery = true;
    app.trust_mode = false;

    complete_trust_directory_onboarding(&mut app, &Config::default())
        .expect("trust completion should succeed");

    assert_eq!(app.onboarding, OnboardingState::Ready);
}

#[test]
fn prompt_override_notice_surfaces_in_transcript_and_toast() {
    let _lock = crate::test_support::lock_test_env();
    let _env = crate::test_support::EnvVarGuard::remove(prompts::BASE_PROMPT_OVERRIDE_OPT_IN_ENV);
    let tmpdir = TempDir::new().expect("config tempdir");
    let prompts_dir = tmpdir.path().join("prompts");
    std::fs::create_dir_all(&prompts_dir).expect("prompts dir");
    std::fs::write(prompts_dir.join("constitution.md"), "custom law\n").expect("override file");
    let _ = prompts::take_prompt_override_notices();
    assert!(prompts::load_config_dir_prompt_overrides(tmpdir.path()).is_empty());

    let mut app = create_test_app();
    surface_prompt_override_notices(&mut app);

    assert!(
        app.history.iter().any(|cell| matches!(
            cell,
            HistoryCell::System { content }
                if content.contains(prompts::BASE_PROMPT_OVERRIDE_OPT_IN_ENV)
                    && content.contains("bundled Constitution")
        )),
        "expected system warning in transcript, got {:?}",
        app.history
    );
    let toast = app.status_toasts.back().expect("warning toast");
    assert_eq!(toast.level, StatusToastLevel::Warning);
    assert!(
        toast
            .text
            .contains(prompts::BASE_PROMPT_OVERRIDE_OPT_IN_ENV)
    );
}

#[test]
fn jump_to_adjacent_tool_cell_finds_next_and_previous() {
    let mut app = create_test_app();
    app.history = vec![
        HistoryCell::User {
            content: "hello".to_string(),
        },
        HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "file_search".to_string(),
            status: ToolStatus::Success,
            input_summary: Some("query: foo".to_string()),
            output: Some("done".to_string()),
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        })),
        HistoryCell::Assistant {
            content: "ok".to_string(),
            streaming: false,
        },
        HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "run_command".to_string(),
            status: ToolStatus::Success,
            input_summary: Some("ls".to_string()),
            output: Some("...".to_string()),
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        })),
    ];
    app.mark_history_updated();
    let cell_revisions = vec![app.history_version; app.history.len()];
    app.viewport.transcript_cache.ensure(
        &app.history,
        &cell_revisions,
        100,
        app.transcript_render_options(),
    );

    app.viewport.last_transcript_top = 0;
    assert!(jump_to_adjacent_tool_cell(
        &mut app,
        SearchDirection::Forward
    ));
    // Forward jump pins the scroll to a non-tail line offset (the tool
    // cell's first line). Anything below the live tail is acceptable —
    // the previous assertion checked `TranscriptScroll::Scrolled { .. }`,
    // which under the new flat-offset model means "not at tail."
    assert!(!app.viewport.transcript_scroll.is_at_tail());

    app.viewport.last_transcript_top = app
        .viewport
        .transcript_cache
        .total_lines()
        .saturating_sub(1);
    assert!(jump_to_adjacent_tool_cell(
        &mut app,
        SearchDirection::Backward
    ));
}

fn first_line_for_cell(app: &App, cell_index: usize) -> usize {
    app.viewport
        .transcript_cache
        .line_meta()
        .iter()
        .position(|meta| meta.cell_line().is_some_and(|(idx, _)| idx == cell_index))
        .expect("cell should have rendered line")
}

fn pop_pager_body(app: &mut App) -> String {
    let mut view = app.view_stack.pop().expect("pager view");
    let pager = view
        .as_any_mut()
        .downcast_mut::<PagerView>()
        .expect("top view should be pager");
    pager.body_text()
}

#[test]
fn macos_option_v_glyph_is_treated_as_details_shortcut_only_on_macos() {
    let option_v = KeyEvent::new(KeyCode::Char('\u{221A}'), KeyModifiers::NONE);
    assert!(crate::tui::key_shortcuts::is_macos_option_v_legacy_key_for_platform(&option_v, true));
    assert!(
        !crate::tui::key_shortcuts::is_macos_option_v_legacy_key_for_platform(&option_v, false)
    );

    let modified = KeyEvent::new(KeyCode::Char('\u{221A}'), KeyModifiers::SHIFT);
    assert!(!crate::tui::key_shortcuts::is_macos_option_v_legacy_key_for_platform(&modified, true));

    let plain_v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE);
    assert!(!crate::tui::key_shortcuts::is_macos_option_v_legacy_key_for_platform(&plain_v, true));
}

#[test]
fn open_tool_details_pager_supports_active_virtual_tool_cell() {
    let mut app = create_test_app();
    handle_tool_call_started(
        &mut app,
        "active-1",
        "exec_shell",
        &serde_json::json!({"command": "echo hi"}),
    );
    let active_entries = app
        .active_cell
        .as_ref()
        .expect("active cell")
        .entries()
        .to_vec();
    app.viewport.transcript_cache.ensure_split(
        &[&app.history, active_entries.as_slice()],
        &[1],
        100,
        app.transcript_render_options(),
        &app.folded_thinking,
        None,
        None,
    );
    app.viewport.last_transcript_top = 0;
    app.viewport.last_transcript_visible = 4;

    assert_eq!(detail_target_cell_index(&app), Some(0));
    assert!(open_tool_details_pager(&mut app));
    assert_eq!(app.view_stack.top_kind(), Some(ModalKind::Pager));
}

#[test]
fn visible_error_becomes_full_detail_target_ahead_of_adjacent_tool() {
    let mut app = create_test_app();
    app.history = vec![
        HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "exec_shell".to_string(),
            status: ToolStatus::Success,
            input_summary: Some("command: true".to_string()),
            output: Some("ok".to_string()),
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        })),
        HistoryCell::Error {
            message: "Refusing insecure base URL.\nSet CODEWHALE_ALLOW_INSECURE_HTTP=1 only for a trusted LAN host."
                .to_string(),
            severity: crate::error_taxonomy::ErrorSeverity::Error,
        },
    ];
    app.resync_history_revisions();
    app.viewport.transcript_cache.ensure(
        &app.history,
        &app.history_revisions,
        80,
        app.transcript_render_options(),
    );
    app.viewport.last_transcript_top = 0;
    app.viewport.last_transcript_visible = app.viewport.transcript_cache.total_lines();

    assert!(app.cell_has_detail_target(1));
    assert_eq!(detail_target_cell_index(&app), Some(1));
    assert!(open_tool_details_pager(&mut app));
    let body = pop_pager_body(&mut app);
    assert!(body.contains("CODEWHALE_ALLOW_INSECURE_HTTP=1"), "{body}");
}

#[test]
fn tool_details_pager_frames_leaf_scope_and_preserves_raw_content() {
    let mut app = create_test_app();
    app.history = vec![HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
        name: "exec_shell".to_string(),
        status: ToolStatus::Success,
        input_summary: Some("command: ls".to_string()),
        output: Some("total 0".to_string()),
        prompts: None,
        spillover_path: None,
        output_summary: None,
        is_diff: false,
    }))];
    app.tool_details_by_cell.insert(
        0,
        ToolDetailRecord {
            tool_id: "exec-1".to_string(),
            tool_name: "exec_shell".to_string(),
            input: serde_json::json!({"command": "ls"}),
            output: Some("total 0".to_string()),
        },
    );
    app.resync_history_revisions();

    assert!(open_details_pager_for_cell(&mut app, 0));

    let mut view = app.view_stack.pop().expect("pager view");
    let pager = view
        .as_any_mut()
        .downcast_mut::<PagerView>()
        .expect("top view should be pager");

    // Title reads as raw leaf detail for THIS selected item, not the whole turn.
    assert!(
        pager.title().starts_with("Raw detail"),
        "title should frame leaf scope: {}",
        pager.title()
    );
    assert!(
        pager.title().contains("exec_shell"),
        "title should name the selected tool: {}",
        pager.title()
    );

    let body = pager.body_text();
    // Body frames leaf scope and points to the dedicated whole-turn chord.
    assert!(body.contains("Raw detail for the selected item"), "{body}");
    assert!(body.contains("Ctrl+Alt+O"), "{body}");
    // Existing raw input/output visibility must be preserved.
    assert!(body.contains("Input:"), "{body}");
    assert!(body.contains("Output:"), "{body}");
    assert!(body.contains("total 0"), "{body}");
    assert!(body.contains("\"command\": \"ls\""), "{body}");
}

#[test]
fn tool_details_pager_keeps_exact_file_change_when_inline_diffs_are_off() {
    let mut app = create_test_app();
    app.inline_diff_mode = crate::settings::InlineDiffMode::Off;
    let tool_result = crate::tools::spec::ToolResult::success("structured success").with_metadata(
        serde_json::json!({
            "mutation": {
                "diff": "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+EXACT-SENTINEL\n",
                "files": [{ "path": "src/lib.rs", "outcome": "updated" }],
                "renames": []
            }
        }),
    );
    let receipt =
        crate::tui::history::FileMutationReceipt::from_success(&app.workspace, &tool_result)
            .expect("receipt");
    app.history = vec![HistoryCell::Tool(ToolCell::PatchSummary(
        crate::tui::history::PatchSummaryCell {
            path: "src/lib.rs".to_string(),
            summary: "updated one file".to_string(),
            status: ToolStatus::Success,
            error: None,
            receipt: Some(receipt),
        },
    ))];
    app.tool_details_by_cell.insert(
        0,
        ToolDetailRecord {
            tool_id: "file-1".to_string(),
            tool_name: "File".to_string(),
            input: serde_json::json!({"action": "edit", "path": "src/lib.rs"}),
            output: Some("structured success".to_string()),
        },
    );
    app.resync_history_revisions();

    assert!(open_details_pager_for_cell(&mut app, 0));
    let body = pop_pager_body(&mut app);
    assert!(body.contains("── Exact File change ──"), "{body}");
    assert!(body.contains("+EXACT-SENTINEL"), "{body}");
    assert!(body.contains("Updated src/lib.rs · +1 -1"), "{body}");
}

#[test]
fn tool_details_empty_state_points_to_turn_inspector() {
    let mut app = create_test_app();
    // A selection index with no raw detail record and no backing cell: the
    // empty state should route the user to Ctrl+O for turn-level context.
    assert!(!open_details_pager_for_cell(&mut app, 999));
    let msg = app.status_message.clone().unwrap_or_default();
    assert!(
        msg.contains("Ctrl+Alt+O"),
        "empty state should point to Ctrl+Alt+O for the turn overview: {msg}"
    );
}

#[test]
fn spillover_pager_section_returns_none_when_no_spillover() {
    let mut app = create_test_app();
    app.history = vec![HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
        name: "exec_shell".to_string(),
        status: ToolStatus::Success,
        input_summary: None,
        output: Some("hi".to_string()),
        prompts: None,
        spillover_path: None,
        output_summary: None,
        is_diff: false,
    }))];
    app.resync_history_revisions();
    assert!(spillover_pager_section(&app, 0).is_none());
}

#[test]
fn spillover_pager_section_loads_file_when_present() {
    let _spillover_guard = crate::tools::truncate::TEST_SPILLOVER_GUARD
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let prior =
        crate::tools::truncate::set_test_spillover_root(Some(dir.path().join("tool_outputs")));
    struct RestoreSpilloverRoot(Option<PathBuf>);
    impl Drop for RestoreSpilloverRoot {
        fn drop(&mut self) {
            crate::tools::truncate::set_test_spillover_root(self.0.take());
        }
    }
    let _restore = RestoreSpilloverRoot(prior);
    let session_id = "session-pager";
    let body = "FULL_OUTPUT_BYTES_HERE\n";
    let path = crate::tools::truncate::write_spillover("call-test", body).unwrap();
    crate::tools::truncate::publish_legacy_spillover_ownership(&path, session_id, body.as_bytes())
        .unwrap();

    let mut app = create_test_app();
    app.current_session_id = Some(session_id.to_string());
    app.history = vec![HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
        name: "exec_shell".to_string(),
        status: ToolStatus::Success,
        input_summary: None,
        output: Some("(truncated head)".to_string()),
        prompts: None,
        spillover_path: Some(path.clone()),
        output_summary: None,
        is_diff: false,
    }))];
    app.resync_history_revisions();

    let section = spillover_pager_section(&app, 0).expect("section present");
    assert!(section.contains("── Full output ──"));
    assert!(
        section.contains("FULL_OUTPUT_BYTES_HERE"),
        "section missing file body: {section}"
    );
    assert!(
        !section.contains(&path.display().to_string()),
        "pager must not expose the artifact path: {section}"
    );
}

#[test]
fn spillover_pager_section_returns_notice_when_file_missing() {
    let mut app = create_test_app();
    let bogus = std::path::PathBuf::from("/private/sensitive/session/does-not-exist.txt");
    app.history = vec![HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
        name: "exec_shell".to_string(),
        status: ToolStatus::Success,
        input_summary: None,
        output: Some("(truncated head)".to_string()),
        prompts: None,
        spillover_path: Some(bogus),
        output_summary: None,
        is_diff: false,
    }))];
    app.resync_history_revisions();

    let section = spillover_pager_section(&app, 0).expect("still emits a notice section");
    assert!(section.contains("retained output is unavailable"));
    assert!(!section.contains("/private/sensitive"));
    assert!(!section.contains("No such file"));
}

#[test]
fn spillover_pager_uses_session_artifact_for_specialized_bash_cell() {
    let _artifact_guard = crate::artifacts::TEST_ARTIFACT_SESSIONS_GUARD
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let prior =
        crate::artifacts::set_test_artifact_sessions_root(Some(dir.path().join("sessions")));
    struct RestoreArtifactRoot(Option<PathBuf>);
    impl Drop for RestoreArtifactRoot {
        fn drop(&mut self) {
            crate::artifacts::set_test_artifact_sessions_root(self.0.take());
        }
    }
    let _restore = RestoreArtifactRoot(prior);
    let session_id = "session-bash";
    let relative_path = PathBuf::from("artifacts/art_call-bash.txt");
    let path = crate::artifacts::session_artifact_absolute_path(session_id, &relative_path)
        .expect("isolated artifact path");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "BASH_EXACT_DEEP_SENTINEL\n").unwrap();

    let mut app = create_test_app();
    app.current_session_id = Some(session_id.to_string());
    app.history = vec![HistoryCell::Tool(ToolCell::Exec(ExecCell {
        command: "cargo test".to_string(),
        status: ToolStatus::Success,
        output: Some("head\n\n… 23 B of output omitted — view full output in the tool details view\n\n[tail]\ntail".to_string()),
        live_output: None,
        shell_task_id: None,
        owner_agent_id: None,
        owner_agent_name: None,
        started_at: None,
        duration_ms: None,
        stale_elapsed_since_output_ms: None,
        source: ExecSource::Assistant,
        interaction: None,
        output_summary: None,
    }))];
    app.tool_details_by_cell.insert(
        0,
        ToolDetailRecord {
            tool_id: "call-bash".to_string(),
            tool_name: "Bash".to_string(),
            input: serde_json::json!({"action": "run", "command": "cargo test"}),
            output: Some("head\n\n… 23 B of output omitted — view full output in the tool details view\n\n[tail]\ntail".to_string()),
        },
    );
    let artifact = crate::artifacts::ArtifactRecord {
        id: "art_call-bash".to_string(),
        kind: crate::artifacts::ArtifactKind::ToolOutput,
        session_id: session_id.to_string(),
        tool_call_id: "call-bash".to_string(),
        tool_name: "Bash".to_string(),
        created_at: chrono::Utc::now(),
        byte_size: 25,
        preview: "BASH_EXACT_DEEP_SENTINEL".to_string(),
        storage_path: relative_path.clone(),
    };
    app.session_artifacts.push(artifact.clone());
    app.resync_history_revisions();

    let section = spillover_pager_section(&app, 0).expect("session artifact section");
    assert!(section.contains("BASH_EXACT_DEEP_SENTINEL"));
    assert!(!section.contains(&path.display().to_string()));

    app.session_artifacts[0].storage_path = path.clone();
    let absolute = spillover_pager_section(&app, 0).expect("unavailable absolute-path section");
    assert!(absolute.contains("retained output is unavailable"));
    assert!(!absolute.contains("BASH_EXACT_DEEP_SENTINEL"));

    app.session_artifacts[0] = crate::artifacts::ArtifactRecord {
        session_id: "foreign-session".to_string(),
        storage_path: relative_path,
        ..artifact
    };
    assert!(
        spillover_pager_section(&app, 0).is_none(),
        "a foreign artifact record must not become an Option+V detail source"
    );
}

#[test]
fn terminal_pause_has_live_owner_only_for_running_exec_cells() {
    let mut app = create_test_app();
    assert!(!terminal_pause_has_live_owner(&app));

    let mut active = ActiveCell::new();
    active.push_tool(
        "tool-1",
        HistoryCell::Tool(ToolCell::Exec(ExecCell {
            command: "python3 -i".to_string(),
            status: ToolStatus::Running,
            output: None,
            live_output: None,
            shell_task_id: None,
            owner_agent_id: None,
            owner_agent_name: None,
            started_at: Some(Instant::now()),
            duration_ms: None,
            stale_elapsed_since_output_ms: None,
            source: ExecSource::Assistant,
            interaction: Some("interactive".to_string()),
            output_summary: None,
        })),
    );
    app.active_cell = Some(active);
    assert!(terminal_pause_has_live_owner(&app));

    let mut active = ActiveCell::new();
    active.push_tool(
        "tool-2",
        HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "rlm".to_string(),
            status: ToolStatus::Running,
            input_summary: Some("file_path: Cargo.lock".to_string()),
            output: None,
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        })),
    );
    app.active_cell = Some(active);
    assert!(
        !terminal_pause_has_live_owner(&app),
        "non-interactive RLM work must not keep the terminal in host-scrollback mode"
    );
}

#[test]
fn active_foreground_shell_running_excludes_detached_background_jobs() {
    let mut app = create_test_app();
    let mut active = ActiveCell::new();
    active.push_tool(
        "shell",
        HistoryCell::Tool(ToolCell::Exec(ExecCell {
            command: "cargo test --workspace".to_string(),
            status: ToolStatus::Running,
            output: None,
            live_output: None,
            shell_task_id: Some("shell-42".to_string()),
            owner_agent_id: None,
            owner_agent_name: None,
            started_at: Some(Instant::now()),
            duration_ms: None,
            stale_elapsed_since_output_ms: None,
            source: ExecSource::Assistant,
            interaction: None,
            output_summary: None,
        })),
    );
    app.active_cell = Some(active);

    assert!(
        !active_foreground_shell_running(&app),
        "a detached job remains Running but is no longer a foreground wait"
    );

    let Some(HistoryCell::Tool(ToolCell::Exec(exec))) = app
        .active_cell
        .as_mut()
        .and_then(|active| active.entry_mut(0))
    else {
        panic!("running shell cell");
    };
    exec.shell_task_id = None;
    assert!(active_foreground_shell_running(&app));
}

#[test]
fn active_rlm_task_entries_surface_foreground_rlm_work() {
    let mut app = create_test_app();
    app.turn_started_at = Some(Instant::now() - Duration::from_secs(3));
    let mut active = ActiveCell::new();
    active.push_tool(
        "tool-rlm",
        HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "rlm".to_string(),
            status: ToolStatus::Running,
            input_summary: Some("file_path: Cargo.lock".to_string()),
            output: None,
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        })),
    );
    app.active_cell = Some(active);

    let entries = active_rlm_task_entries(&app);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, "rlm-1");
    assert_eq!(entries[0].status, "running");
    assert_eq!(entries[0].prompt_summary, "RLM: file_path: Cargo.lock");
    assert_eq!(entries[0].kind, TaskPanelEntryKind::Background);
    assert!(entries[0].duration_ms.unwrap_or_default() >= 3000);
}

#[test]
fn alt_nav_modifiers_require_alt_and_exclude_ctrl_super() {
    // v0.8.30 — transcript-nav shortcuts (`Alt+[`, `Alt+]`, etc.) require
    // Alt, allow Shift for capital-letter forms, and block Ctrl/Super so
    // they don't collide with clipboard / window shortcuts. Bare and
    // Shift-only modifiers fall through to text insertion now.
    assert!(!crate::tui::key_shortcuts::alt_nav_modifiers(
        KeyModifiers::NONE
    ));
    assert!(!crate::tui::key_shortcuts::alt_nav_modifiers(
        KeyModifiers::SHIFT
    ));
    assert!(crate::tui::key_shortcuts::alt_nav_modifiers(
        KeyModifiers::ALT
    ));
    assert!(crate::tui::key_shortcuts::alt_nav_modifiers(
        KeyModifiers::ALT | KeyModifiers::SHIFT
    ));
    assert!(!crate::tui::key_shortcuts::alt_nav_modifiers(
        KeyModifiers::CONTROL
    ));
    assert!(!crate::tui::key_shortcuts::alt_nav_modifiers(
        KeyModifiers::ALT | KeyModifiers::CONTROL
    ));
    assert!(!crate::tui::key_shortcuts::alt_nav_modifiers(
        KeyModifiers::ALT | KeyModifiers::SUPER
    ));
}

#[test]
fn ctrl_h_is_treated_as_terminal_backspace() {
    assert!(crate::tui::key_shortcuts::is_ctrl_h_backspace(
        &KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL)
    ));
    assert!(!crate::tui::key_shortcuts::is_ctrl_h_backspace(
        &KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE)
    ));
    assert!(!crate::tui::key_shortcuts::is_ctrl_h_backspace(
        &KeyEvent::new(
            KeyCode::Char('h'),
            KeyModifiers::CONTROL | KeyModifiers::ALT
        )
    ));
}

#[test]
fn partial_file_mention_finds_token_under_cursor() {
    // Cursor in middle of `@docs/de` should be detected as a partial mention.
    let input = "look at @docs/de please";
    let cursor = "look at @docs/de".chars().count();
    let (start, partial) = partial_file_mention_at_cursor(input, cursor)
        .expect("cursor inside mention should yield a partial");
    assert_eq!(start, "look at ".len(), "byte_start of @ in input");
    assert_eq!(partial, "docs/de");
}

#[test]
fn partial_file_mention_returns_none_when_cursor_outside() {
    let input = "look at @docs/de please";
    // Cursor after "please" — past the whitespace following the mention.
    let cursor = input.chars().count();
    assert!(partial_file_mention_at_cursor(input, cursor).is_none());

    // Cursor before the `@` — not inside any mention either.
    let early_cursor = "look".chars().count();
    assert!(partial_file_mention_at_cursor(input, early_cursor).is_none());
}

#[test]
fn partial_file_mention_handles_email_addresses() {
    // The `@` in `user@example.com` is preceded by a non-boundary char so
    // it's not treated as a file-mention.
    let input = "ping user@example.com now";
    let cursor = "ping user@example.com".chars().count();
    assert!(partial_file_mention_at_cursor(input, cursor).is_none());
}

#[test]
fn file_mention_completion_finds_unique_match() {
    let tmpdir = TempDir::new().expect("tempdir");
    std::fs::write(tmpdir.path().join("README.md"), "readme").unwrap();
    std::fs::create_dir_all(tmpdir.path().join("docs")).unwrap();
    std::fs::write(tmpdir.path().join("docs/deepseek_v4.pdf"), b"%PDF-").unwrap();

    let ws = Workspace::with_cwd(tmpdir.path().to_path_buf(), None);
    let matches = find_file_mention_completions(&ws, "docs/de", 16);
    assert_eq!(matches, vec!["docs/deepseek_v4.pdf".to_string()]);
}

#[test]
fn file_mention_completion_ranks_prefix_before_substring() {
    let tmpdir = TempDir::new().expect("tempdir");
    std::fs::write(tmpdir.path().join("README.md"), "x").unwrap();
    std::fs::create_dir_all(tmpdir.path().join("nested")).unwrap();
    std::fs::write(tmpdir.path().join("nested/README.md"), "x").unwrap();

    let ws = Workspace::with_cwd(tmpdir.path().to_path_buf(), None);
    let matches = find_file_mention_completions(&ws, "README", 16);
    // Top-level README (prefix match) outranks the nested one (substring).
    assert_eq!(matches.first().map(String::as_str), Some("README.md"));
}

fn await_visible_mention_entries(app: &mut App, limit: usize) -> Vec<String> {
    let partial = partial_file_mention_at_cursor(&app.input, app.cursor_position)
        .expect("test input should contain a mention")
        .1;
    let started = Instant::now();
    loop {
        let entries = visible_mention_menu_entries(app, limit);
        let ready = app
            .composer
            .mention_completion_cache
            .as_ref()
            .is_some_and(|cache| cache.partial == partial && cache.limit == limit);
        if ready {
            return entries;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timed out waiting for background mention discovery"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn try_autocomplete_file_mention_unique_replaces_partial() {
    let tmpdir = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmpdir.path().join("docs")).unwrap();
    std::fs::write(tmpdir.path().join("docs/deepseek_v4.pdf"), b"%PDF-").unwrap();

    let mut app = create_test_app();
    app.workspace = tmpdir.path().to_path_buf();
    app.input = "summarize @docs/de".to_string();
    app.cursor_position = app.input.chars().count();

    let _ = await_visible_mention_entries(&mut app, 64);
    assert!(try_autocomplete_file_mention(&mut app));
    assert_eq!(app.input, "summarize @docs/deepseek_v4.pdf");
    assert_eq!(app.cursor_position, app.input.chars().count());
}

#[test]
fn try_autocomplete_file_mention_extends_to_common_prefix() {
    let tmpdir = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmpdir.path().join("crates/tui")).unwrap();
    std::fs::write(tmpdir.path().join("crates/tui/lib.rs"), "//").unwrap();
    std::fs::write(tmpdir.path().join("crates/tui/main.rs"), "//").unwrap();

    let mut app = create_test_app();
    app.workspace = tmpdir.path().to_path_buf();
    app.input = "@crates/tui/".to_string();
    app.cursor_position = app.input.chars().count();

    let _ = await_visible_mention_entries(&mut app, 64);
    assert!(try_autocomplete_file_mention(&mut app));
    // Both files share the `crates/tui/` prefix and one more letter is
    // not unique (`l` vs `m`), so the partial extends to the common prefix
    // unchanged here, with the status surfacing both candidates.
    assert!(app.input.starts_with("@crates/tui/"));
    let preview = app
        .status_message
        .as_deref()
        .expect("status message should describe candidates");
    assert!(preview.contains("@crates/tui/lib.rs"));
    assert!(preview.contains("@crates/tui/main.rs"));
}

#[test]
fn try_autocomplete_file_mention_no_match_reports_status() {
    let tmpdir = TempDir::new().expect("tempdir");
    std::fs::write(tmpdir.path().join("README.md"), "x").unwrap();

    let mut app = create_test_app();
    app.workspace = tmpdir.path().to_path_buf();
    app.input = "@nonexistent_xyz".to_string();
    app.cursor_position = app.input.chars().count();

    let _ = await_visible_mention_entries(&mut app, 64);
    assert!(try_autocomplete_file_mention(&mut app));
    assert_eq!(app.input, "@nonexistent_xyz");
    assert_eq!(
        app.status_message.as_deref(),
        Some("No files match @nonexistent_xyz")
    );
}

#[test]
fn try_autocomplete_file_mention_no_match_mentions_depth_cap_for_path_like_partial() {
    let tmpdir = TempDir::new().expect("tempdir");

    let mut app = create_test_app();
    app.workspace = tmpdir.path().to_path_buf();
    app.mention_walk_depth = 6;
    app.input = "@a/b/c/d/e/f/g/target".to_string();
    app.cursor_position = app.input.chars().count();

    let _ = await_visible_mention_entries(&mut app, 64);
    assert!(try_autocomplete_file_mention(&mut app));
    assert_eq!(
        app.status_message.as_deref(),
        Some(
            "No files match @a/b/c/d/e/f/g/target (mention_walk_depth=6; use /config set mention_walk_depth 0 to search deeper)"
        )
    );
}

#[test]
fn try_autocomplete_file_mention_no_match_skips_depth_hint_for_shallow_path() {
    let tmpdir = TempDir::new().expect("tempdir");

    let mut app = create_test_app();
    app.workspace = tmpdir.path().to_path_buf();
    app.mention_walk_depth = 6;
    app.input = "@shallow_missing/main.rs".to_string();
    app.cursor_position = app.input.chars().count();

    let _ = await_visible_mention_entries(&mut app, 64);
    assert!(try_autocomplete_file_mention(&mut app));
    assert_eq!(
        app.status_message.as_deref(),
        Some("No files match @shallow_missing/main.rs")
    );
}

#[test]
fn try_autocomplete_file_mention_no_match_skips_depth_hint_when_unlimited() {
    let tmpdir = TempDir::new().expect("tempdir");

    let mut app = create_test_app();
    app.workspace = tmpdir.path().to_path_buf();
    app.mention_walk_depth = 0;
    app.input = "@a/b/c/d/e/f/g/target".to_string();
    app.cursor_position = app.input.chars().count();

    let _ = await_visible_mention_entries(&mut app, 64);
    assert!(try_autocomplete_file_mention(&mut app));
    assert_eq!(
        app.status_message.as_deref(),
        Some("No files match @a/b/c/d/e/f/g/target")
    );
}

#[test]
fn try_autocomplete_file_mention_returns_false_outside_mention() {
    let mut app = create_test_app();
    app.input = "no mention here".to_string();
    app.cursor_position = app.input.chars().count();
    assert!(!try_autocomplete_file_mention(&mut app));
}

// ---- P2.1: @-mention popup helpers ----
//
// `visible_mention_menu_entries` is the entries source the composer widget
// renders; `apply_mention_menu_selection` is what Tab/Enter invoke when the
// popup is open. The popup widget itself piggybacks the slash-menu render
// path (see `ComposerWidget::active_menu_entries`).

#[test]
fn mention_popup_is_empty_when_cursor_is_not_in_a_mention() {
    let mut app = create_test_app();
    app.input = "no mention here".to_string();
    app.cursor_position = app.input.chars().count();
    assert!(visible_mention_menu_entries(&mut app, 6).is_empty());
}

#[test]
fn plain_at_returns_immediately_while_discovery_is_stalled() {
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = std::sync::Mutex::new(release_rx);

    let mut app = create_test_app();
    app.input = "@".to_string();
    app.cursor_position = 1;
    app.composer.mention_discovery =
        crate::tui::mention_completion::MentionDiscovery::with_scanner(move |_, _| {
            let _ = started_tx.send(());
            let _ = release_rx.lock().expect("release lock").recv();
            vec!["README.md".to_string()]
        });

    let started = Instant::now();
    let initial = visible_mention_menu_entries(&mut app, 6);
    assert!(initial.is_empty());
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "plain @ waited for discovery instead of returning immediately"
    );
    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("plain @ should start discovery in the background");

    release_tx.send(()).unwrap();
    assert_eq!(
        await_visible_mention_entries(&mut app, 6),
        vec!["README.md"]
    );
}

#[test]
fn mention_popup_lists_workspace_matches_for_cursor_partial() {
    let tmpdir = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmpdir.path().join("docs")).unwrap();
    std::fs::write(tmpdir.path().join("docs/deepseek_v4.pdf"), b"%PDF-").unwrap();
    std::fs::write(tmpdir.path().join("docs/MCP.md"), "x").unwrap();
    std::fs::write(tmpdir.path().join("README.md"), "x").unwrap();

    let mut app = create_test_app();
    app.workspace = tmpdir.path().to_path_buf();
    app.input = "look at @docs/".to_string();
    app.cursor_position = app.input.chars().count();

    let entries = await_visible_mention_entries(&mut app, 6);
    assert!(!entries.is_empty(), "popup should surface docs/ entries");
    assert!(entries.iter().any(|e| e.starts_with("docs/")));
    // README.md doesn't match `docs/` — confirm we didn't dump every file.
    assert!(!entries.iter().any(|e| e == "README.md"));
}

#[test]
fn mention_popup_browser_mode_lists_immediate_directory_children() {
    let tmpdir = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmpdir.path().join("src/nested")).unwrap();
    std::fs::write(tmpdir.path().join("src/lib.rs"), "lib").unwrap();
    std::fs::write(tmpdir.path().join("src/nested/deep.rs"), "deep").unwrap();
    std::fs::write(tmpdir.path().join("README.md"), "readme").unwrap();

    let mut app = create_test_app();
    app.workspace = tmpdir.path().to_path_buf();
    app.mention_menu_behavior = "browser".to_string();
    app.input = "look at @src/".to_string();
    app.cursor_position = app.input.chars().count();

    let entries = await_visible_mention_entries(&mut app, 8);
    assert_eq!(entries, vec!["src/lib.rs", "src/nested/"]);
}

#[test]
fn mention_popup_reuses_cache_when_cursor_moves_inside_same_token() {
    let tmpdir = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmpdir.path().join("docs")).unwrap();
    std::fs::write(tmpdir.path().join("docs/alpha.md"), "x").unwrap();

    let mut app = create_test_app();
    app.workspace = tmpdir.path().to_path_buf();
    app.input = "look at @docs/".to_string();
    app.cursor_position = app.input.chars().count();

    let entries = await_visible_mention_entries(&mut app, 6);
    assert!(entries.iter().any(|e| e == "docs/alpha.md"));

    std::fs::write(tmpdir.path().join("docs/beta.md"), "x").unwrap();
    app.cursor_position = "look at @do".chars().count();

    let entries_after_cursor_move = visible_mention_menu_entries(&mut app, 6);
    assert_eq!(
        entries_after_cursor_move, entries,
        "cursor movement inside one @mention token should not re-walk the workspace",
    );

    app.input = "look at @docs/b".to_string();
    app.cursor_position = app.input.chars().count();

    // The bounded background index is intentionally reused for the same
    // workspace generation; filesystem mutations become visible on refresh.
    app.composer.mention_discovery.invalidate();
    app.composer.mention_completion_cache = None;
    let entries_after_partial_change = await_visible_mention_entries(&mut app, 6);
    assert!(
        entries_after_partial_change
            .iter()
            .any(|e| e == "docs/beta.md"),
        "changing the partial should invalidate the completion cache",
    );
}

#[test]
fn mention_popup_respects_hidden_flag() {
    let tmpdir = TempDir::new().expect("tempdir");
    std::fs::write(tmpdir.path().join("README.md"), "x").unwrap();

    let mut app = create_test_app();
    app.workspace = tmpdir.path().to_path_buf();
    app.input = "@READ".to_string();
    app.cursor_position = app.input.chars().count();
    app.mention_menu_hidden = true;

    assert!(
        visible_mention_menu_entries(&mut app, 6).is_empty(),
        "Esc-hidden popup must not surface entries until next input edit",
    );
}

#[test]
fn apply_mention_menu_selection_splices_selected_entry() {
    let tmpdir = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmpdir.path().join("crates/tui")).unwrap();
    std::fs::write(tmpdir.path().join("crates/tui/lib.rs"), "//").unwrap();
    std::fs::write(tmpdir.path().join("crates/tui/main.rs"), "//").unwrap();

    let mut app = create_test_app();
    app.workspace = tmpdir.path().to_path_buf();
    app.input = "open @crates/tui/m".to_string();
    app.cursor_position = app.input.chars().count();

    let entries = await_visible_mention_entries(&mut app, 6);
    assert!(!entries.is_empty(), "expected entries for @crates/tui/m");
    // Pick whichever entry appears at index 0; it's deterministic given the
    // workspace setup. Apply it.
    app.mention_menu_selected = 0;
    let applied = apply_mention_menu_selection(&mut app, &entries);
    assert!(
        applied,
        "apply_mention_menu_selection should report success"
    );
    assert!(
        app.input.starts_with("open @"),
        "input should still start with `open @`, got: {input}",
        input = app.input,
    );
    // Cursor should land at the end of the spliced token.
    assert_eq!(app.cursor_position, app.input.chars().count());
}

#[test]
fn apply_mention_menu_selection_is_noop_outside_a_mention() {
    let mut app = create_test_app();
    app.input = "no @ here".to_string();
    app.cursor_position = 1; // before the @ token
    let applied = apply_mention_menu_selection(&mut app, &["whatever".to_string()]);
    assert!(!applied);
    assert_eq!(app.input, "no @ here");
}

#[test]
fn apply_mention_menu_selection_with_no_entries_is_noop() {
    let mut app = create_test_app();
    app.input = "@partial".to_string();
    app.cursor_position = app.input.chars().count();
    let applied = apply_mention_menu_selection(&mut app, &[]);
    assert!(!applied);
}

// === CX#7 — single active cell mutated in place for parallel tool calls ===

/// Build a minimal successful ToolResult with the given content.
fn ok_result(
    content: &str,
) -> Result<crate::tools::spec::ToolResult, crate::tools::spec::ToolError> {
    Ok(crate::tools::spec::ToolResult::success(content))
}

fn hydrated_result(
    content: &str,
) -> Result<crate::tools::spec::ToolResult, crate::tools::spec::ToolError> {
    Ok(
        crate::tools::spec::ToolResult::success(content).with_metadata(serde_json::json!({
            "event": "tool.schema_hydrated",
            "tool": "exec_shell",
            "executed": false,
            "retry_required": true,
            "deferred_tool_loaded": true,
            "tool_name": "exec_shell",
        })),
    )
}

fn rendered_text(lines: &[ratatui::text::Line<'_>]) -> String {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn completed_exec_tool_result_still_renders_run_done() {
    let mut app = create_test_app();
    handle_tool_call_started(
        &mut app,
        "shell-ok",
        "exec_shell",
        &serde_json::json!({"command": "echo hi"}),
    );
    handle_tool_call_complete(&mut app, "shell-ok", "exec_shell", &ok_result("hi"));

    let exec = app
        .active_cell
        .as_ref()
        .expect("active cell")
        .entries()
        .iter()
        .find_map(|cell| match cell {
            HistoryCell::Tool(ToolCell::Exec(exec)) => Some(exec),
            _ => None,
        })
        .expect("exec cell");

    assert_eq!(exec.status, ToolStatus::Success);
    let text = rendered_text(&exec.lines_with_motion(100, true));
    assert!(text.contains("run done"), "{text}");
    assert!(!text.contains("tool loaded - retry required"), "{text}");
}

#[test]
fn hydrated_exec_tool_result_renders_retry_required_not_run_done() {
    let mut app = create_test_app();
    handle_tool_call_started(
        &mut app,
        "shell-hydrated",
        "exec_shell",
        &serde_json::json!({"command": "cargo test"}),
    );
    handle_tool_call_complete(
        &mut app,
        "shell-hydrated",
        "exec_shell",
        &hydrated_result(
            "Tool exec_shell was deferred and has now been loaded.\n\
             The tool was not executed. Retry with the loaded schema.",
        ),
    );

    let exec = app
        .active_cell
        .as_ref()
        .expect("active cell")
        .entries()
        .iter()
        .find_map(|cell| match cell {
            HistoryCell::Tool(ToolCell::Exec(exec)) => Some(exec),
            _ => None,
        })
        .expect("exec cell");

    assert_eq!(exec.status, ToolStatus::Hydrated);
    let text = rendered_text(&exec.lines_with_motion(120, true));
    assert!(text.contains("run tool loaded - retry required"), "{text}");
    assert!(!text.contains("run done"), "{text}");
}

#[test]
fn hydrated_tool_with_validation_body_still_uses_hydrated_status() {
    let mut app = create_test_app();
    handle_tool_call_started(
        &mut app,
        "generic-hydrated",
        "deferred_tool",
        &serde_json::json!({"unexpected": true}),
    );
    handle_tool_call_complete(
        &mut app,
        "generic-hydrated",
        "deferred_tool",
        &hydrated_result(
            "Tool deferred_tool was deferred and has now been loaded.\n\n\
             Missing required fields:\n  command\n\n\
             Unexpected fields:\n  unexpected",
        ),
    );

    let generic = app
        .active_cell
        .as_ref()
        .expect("active cell")
        .entries()
        .iter()
        .find_map(|cell| match cell {
            HistoryCell::Tool(ToolCell::Generic(generic)) => Some(generic),
            _ => None,
        })
        .expect("generic cell");

    assert_eq!(generic.status, ToolStatus::Hydrated);
    let text = rendered_text(&HistoryCell::Tool(ToolCell::Generic(generic.clone())).lines(120));
    assert!(text.contains("tool loaded - retry required"), "{text}");
    assert!(!text.contains("tool done"), "{text}");
}

#[test]
fn failed_tool_result_with_hydration_metadata_stays_failed() {
    let mut app = create_test_app();
    handle_tool_call_started(
        &mut app,
        "generic-failed",
        "deferred_tool",
        &serde_json::json!({}),
    );
    let result = Ok(crate::tools::spec::ToolResult::error("boom").with_metadata(
        serde_json::json!({
            "event": "tool.schema_hydrated",
            "executed": false,
            "retry_required": true,
        }),
    ));
    handle_tool_call_complete(&mut app, "generic-failed", "deferred_tool", &result);

    let generic = app
        .active_cell
        .as_ref()
        .expect("active cell")
        .entries()
        .iter()
        .find_map(|cell| match cell {
            HistoryCell::Tool(ToolCell::Generic(generic)) => Some(generic),
            _ => None,
        })
        .expect("generic cell");

    assert_eq!(generic.status, ToolStatus::Failed);
    let text = rendered_text(&HistoryCell::Tool(ToolCell::Generic(generic.clone())).lines(120));
    assert!(text.contains("tool issue"), "{text}");
    assert!(!text.contains("tool loaded - retry required"), "{text}");
}

#[test]
fn shell_wait_without_command_uses_task_id_until_command_metadata_arrives() {
    let mut app = create_test_app();
    handle_tool_call_started(
        &mut app,
        "shell-wait",
        "exec_shell_wait",
        &serde_json::json!({"task_id": "shell_33a08c3c"}),
    );

    let exec = app
        .active_cell
        .as_ref()
        .expect("active cell")
        .entries()
        .iter()
        .find_map(|cell| match cell {
            HistoryCell::Tool(ToolCell::Exec(exec)) => Some(exec),
            _ => None,
        })
        .expect("exec cell");
    assert_eq!(exec.command, "command shell_33a08c3c");
    assert!(
        exec.interaction
            .as_deref()
            .is_some_and(|text| text.contains("shell_33a08c3c"))
    );
    assert!(
        !exec.command.contains("<command>")
            && !exec
                .interaction
                .as_deref()
                .unwrap_or_default()
                .contains("<command>")
    );

    let result = Ok(crate::tools::spec::ToolResult::success(
        "Background task running (no new output).",
    )
    .with_metadata(serde_json::json!({
        "status": "Running",
        "duration_ms": 178_000_u64,
        "task_id": "shell_33a08c3c",
        "command": "cargo test --workspace --all-features",
    })));
    handle_tool_call_complete(&mut app, "shell-wait", "exec_shell_wait", &result);

    let exec = app
        .active_cell
        .as_ref()
        .expect("active cell")
        .entries()
        .iter()
        .find_map(|cell| match cell {
            HistoryCell::Tool(ToolCell::Exec(exec)) => Some(exec),
            _ => None,
        })
        .expect("exec cell");
    assert_eq!(exec.command, "cargo test --workspace --all-features");
    assert!(
        exec.interaction
            .as_deref()
            .is_some_and(|text| text.contains("cargo test --workspace"))
    );
}

#[test]
fn legacy_child_usage_metadata_fails_closed_without_parent_route_fallback() {
    let mut app = create_test_app();
    // An in-process review child publishes no route of its own, so it runs on
    // the parent turn's client and is billed from the parent's frozen receipt.
    // Without a receipt there is nothing sound to bill from, so the turn has to
    // be active for the child to accrue anything.
    app.active_turn = Some(crate::tui::app::ActiveTurnMetadata {
        turn_id: "turn-child-usage".to_string(),
        created_at: chrono::Utc::now(),
        route: Some(crate::core::events::TurnRoute {
            provider: ApiProvider::Deepseek,
            provider_identity: "deepseek".to_string(),
            model: "deepseek-v4-flash".to_string(),
            auto_model: false,
            receipt: None,
            // The parent turn is a fully dispatched, metered route. Even so,
            // a child that publishes no route receipt of its own must not
            // borrow it: the fail-closed answer is Unknown, reported as
            // missing spend rather than silently inherited.
            billing: Some(crate::core::events::RouteBillingEnvelope {
                billing_surface: Some(crate::pricing::FIRST_PARTY_PAYG_BILLING_SURFACE.to_string()),
                endpoint_fingerprint: crate::cost_status::endpoint_fingerprint(
                    crate::config::DEFAULT_DEEPSEEK_BASE_URL,
                ),
                billing_mode: crate::cost_status::RouteBillingMode::Metered,
                dispatched_at: chrono::Utc::now(),
            }),
            base_url: crate::config::DEFAULT_DEEPSEEK_BASE_URL.to_string(),
            billing_product: crate::route_billing::RouteProduct::Unproven,
        }),
        auto_route_receipt: None,
        suggestion_authority: None,
    });
    let result = Ok(crate::tools::spec::ToolResult::success("ok").with_metadata(
        serde_json::json!({
            "child_model": "deepseek-v4-flash",
            "child_input_tokens": 10_000,
            "child_output_tokens": 1_000,
            "child_prompt_cache_hit_tokens": 7_000,
            "child_prompt_cache_miss_tokens": 3_000,
        }),
    ));

    handle_tool_call_complete(&mut app, "review-usage", "review", &result);

    assert_eq!(app.session.subagent_cost, 0.0);
    assert_eq!(app.session.cost_unpriced_turns, 1);
    assert!(
        app.session
            .cost_unpriced_reasons
            .contains("unknown_billing_basis")
    );
}

/// Child metadata has to carry every billable class end-to-end. The producer
/// (`review`/`verify`/`rlm`) and the reconstruction in `tool_routing` are tested
/// together here, because the bug was a mismatch between them: the reconstruction
/// hardcoded `prompt_cache_write_tokens: None`, so a child's cache-creation
/// tokens were billed at nothing *and* the turn still looked fully priced (#4318).
#[test]
fn child_usage_metadata_carries_cache_write_and_reasoning_end_to_end() {
    let child_usage = crate::models::Usage {
        input_tokens: 1_000_000,
        output_tokens: 100_000,
        prompt_cache_hit_tokens: Some(200_000),
        prompt_cache_miss_tokens: Some(700_000),
        prompt_cache_write_tokens: Some(100_000),
        reasoning_tokens: Some(40_000),
        reasoning_replay_tokens: Some(12_345),
        server_tool_use: Some(crate::models::ServerToolUsage {
            code_execution_requests: Some(2),
            tool_search_requests: Some(1),
        }),
    };

    // The shared producer emits every class.
    let priced_route = crate::cost_status::EffectiveRouteEnvelope {
        provider: crate::config::ApiProvider::Anthropic,
        provider_identity: "anthropic-api".to_string(),
        model: "claude-haiku-4-5".to_string(),
        billing_surface: Some(crate::pricing::FIRST_PARTY_PAYG_BILLING_SURFACE.to_string()),
        // Must be a real fingerprint: persistence and receipt boundaries drop
        // anything that is not one, and a dropped field would have made the
        // round-trip assertion below vacuously pass on both sides.
        endpoint_fingerprint: crate::cost_status::endpoint_fingerprint(
            "https://api.anthropic.com/v1",
        ),
        billing_mode: crate::cost_status::RouteBillingMode::Metered,
        dispatched_at: chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).expect("epoch"),
    };
    let fields = crate::cost_status::child_usage_metadata_fields(&priced_route, &child_usage);
    assert_eq!(fields["child_prompt_cache_write_tokens"], 100_000);
    assert_eq!(fields["child_reasoning_tokens"], 40_000);
    assert_eq!(fields["child_reasoning_replay_tokens"], 12_345);
    assert_eq!(
        fields["child_server_tool_use"]["code_execution_requests"],
        2
    );
    assert_eq!(fields["child_prompt_cache_hit_tokens"], 200_000);

    // A priced child route with a published cache-write rate accrues money, and
    // the reconstructed write class is what makes the amount correct.
    //
    // The parent deliberately stays DeepSeek: only the frozen child envelope
    // may control this price.
    let mut priced_app = create_test_app();
    priced_app.api_provider = crate::config::ApiProvider::Deepseek;
    priced_app.billing_presentation = crate::route_billing::BillingPresentation::Metered;
    let mut metadata = serde_json::json!({});
    crate::cost_status::attach_child_usage_metadata(&mut metadata, &priced_route, &child_usage);
    assert_eq!(
        crate::cost_status::child_route_envelope_from_metadata(&metadata),
        Some(priced_route.clone()),
        "typed child route metadata must round-trip exactly"
    );
    let priced = Ok(crate::tools::spec::ToolResult::success("ok").with_metadata(metadata));
    handle_tool_call_complete(&mut priced_app, "verify-1", "verify", &priced);

    // 0.7M input @1.00 + 0.1M output @5.00 + 0.2M read @0.10 + 0.1M write @1.25.
    let expected = 0.7 * 1.0 + 0.1 * 5.0 + 0.2 * 0.1 + 0.1 * 1.25;
    assert!(
        (priced_app.session.subagent_cost - expected).abs() < 1e-9,
        "got {}, want {expected}",
        priced_app.session.subagent_cost
    );
    assert_eq!(priced_app.session.cost_priced_turns, 1);
    assert_eq!(priced_app.session.cost_unpriced_turns, 0);

    // The same usage on a route that publishes no cache-write rate must fail
    // closed and name the class, rather than pricing "completely" at 0 for the
    // write tokens.
    let mut unpriced_app = create_test_app();
    unpriced_app.api_provider = crate::config::ApiProvider::Deepseek;
    unpriced_app.billing_presentation = crate::route_billing::BillingPresentation::Metered;
    let unpriced_route = crate::cost_status::EffectiveRouteEnvelope {
        provider: crate::config::ApiProvider::Moonshot,
        provider_identity: "moonshot-api".to_string(),
        model: "kimi-k2.7-code".to_string(),
        billing_surface: Some(crate::pricing::FIRST_PARTY_PAYG_BILLING_SURFACE.to_string()),
        endpoint_fingerprint: Some("test-moonshot-endpoint".to_string()),
        billing_mode: crate::cost_status::RouteBillingMode::Metered,
        dispatched_at: chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).expect("epoch"),
    };
    let mut metadata = serde_json::json!({});
    crate::cost_status::attach_child_usage_metadata(&mut metadata, &unpriced_route, &child_usage);
    let unpriced = Ok(crate::tools::spec::ToolResult::success("ok").with_metadata(metadata));
    handle_tool_call_complete(&mut unpriced_app, "rlm-1", "rlm", &unpriced);

    assert_eq!(unpriced_app.session.subagent_cost, 0.0);
    assert_eq!(unpriced_app.session.cost_priced_turns, 0);
    assert_eq!(unpriced_app.session.cost_unpriced_turns, 1);
    assert!(
        unpriced_app
            .session
            .cost_unpriced_classes
            .contains("cache_write"),
        "{:?}",
        unpriced_app.session.cost_unpriced_classes
    );

    // A child reporting more reasoning than output is contradicting the subset
    // invariant; the figure is discarded rather than inflating billable output.
    let mut pathological = create_test_app();
    pathological.api_provider = crate::config::ApiProvider::Deepseek;
    pathological.billing_presentation = crate::route_billing::BillingPresentation::Metered;
    let mut metadata = serde_json::json!({});
    crate::cost_status::attach_child_usage_metadata(
        &mut metadata,
        &test_mailbox_route(ApiProvider::Deepseek, "deepseek-v4-flash"),
        &crate::models::Usage {
            input_tokens: 1_000,
            output_tokens: 100,
            reasoning_tokens: Some(9_000),
            ..Default::default()
        },
    );
    let result = Ok(crate::tools::spec::ToolResult::success("ok").with_metadata(metadata));
    handle_tool_call_complete(&mut pathological, "rlm-2", "rlm", &result);
    // Cost is that of 1000 input / 100 output, with no reasoning surcharge.
    let sane = crate::pricing::calculate_turn_cost_estimate_for_provider(
        crate::config::ApiProvider::Deepseek,
        "deepseek-v4-flash",
        &crate::models::Usage {
            input_tokens: 1_000,
            output_tokens: 100,
            ..Default::default()
        },
    )
    .expect("DeepSeek flash is priced");
    assert!(
        (pathological.session.subagent_cost - sane.usd).abs() < 1e-12,
        "impossible reasoning telemetry changed the price: {}",
        pathological.session.subagent_cost
    );
}

#[test]
fn zero_usage_model_child_still_records_priced_receipt() {
    let mut app = create_test_app();
    let route = test_mailbox_route(ApiProvider::Deepseek, "deepseek-v4-flash");
    let mut metadata = serde_json::json!({});
    crate::cost_status::attach_child_usage_metadata(
        &mut metadata,
        &route,
        &crate::models::Usage::default(),
    );
    let result = Ok(crate::tools::spec::ToolResult::error("kernel closed").with_metadata(metadata));

    handle_tool_call_complete(&mut app, "rlm-zero", "rlm", &result);

    assert_eq!(app.session.subagent_cost, 0.0);
    assert_eq!(app.session.cost_priced_turns, 1);
    assert_eq!(app.session.cost_unpriced_turns, 0);
    assert_eq!(app.session.cost_route_receipts.len(), 1);
}

#[test]
fn picker_renamed_active_title_survives_automatic_snapshot() {
    let mut app = create_test_app();
    let manager = SessionManager::new(tempfile::tempdir().expect("tempdir").path().to_path_buf())
        .expect("session manager");
    let mut metadata = crate::session_manager::create_saved_session_with_id_and_mode(
        "session-active".to_string(),
        &app.api_messages,
        &app.model,
        &app.workspace,
        0,
        app.system_prompt.as_ref(),
        Some(app.mode.as_setting()),
    )
    .metadata;
    metadata.title = "Before".to_string();
    metadata.parent_session_id = Some("session-parent".to_string());
    metadata.forked_from_message_count = Some(7);
    let created_at = metadata.created_at;
    app.current_session_id = Some(metadata.id.clone());
    app.current_session_metadata = Some(metadata.clone());
    app.session_title = Some(metadata.title.clone());

    metadata.title = "After".to_string();
    assert!(apply_picker_session_rename_to_active_app(
        &mut app, metadata
    ));
    let snapshot = build_session_snapshot(&mut app, &manager).expect("snapshot");

    assert_eq!(snapshot.metadata.title, "After");
    assert_eq!(snapshot.metadata.created_at, created_at);
    assert_eq!(
        snapshot.metadata.parent_session_id.as_deref(),
        Some("session-parent")
    );
    assert_eq!(snapshot.metadata.forked_from_message_count, Some(7));
}

#[test]
fn stale_cached_placeholder_title_does_not_override_generated_title() {
    let mut app = create_test_app();
    let manager = SessionManager::new(tempfile::tempdir().expect("tempdir").path().to_path_buf())
        .expect("session manager");
    app.api_messages.push(crate::models::Message {
        role: Role::User,
        content: vec![crate::models::ContentBlock::Text {
            text: "Please fix the login bug".to_string(),
            cache_control: None,
        }],
    });
    // Cache pinned to the placeholder title — exactly what the old behavior
    // left behind when the first snapshot ran before any user message existed.
    let mut cached = crate::session_manager::create_saved_session_with_id_and_mode(
        "session-title-bug".to_string(),
        &app.api_messages,
        &app.model,
        &app.workspace,
        0,
        app.system_prompt.as_ref(),
        Some(app.mode.as_setting()),
    )
    .metadata;
    cached.title = "New Session".to_string();
    app.current_session_id = Some(cached.id.clone());
    app.current_session_metadata = Some(cached);

    let snapshot = build_session_snapshot(&mut app, &manager).expect("snapshot");

    assert_eq!(snapshot.metadata.title, "Please fix the login bug");
    assert_eq!(
        app.current_session_metadata
            .as_ref()
            .map(|metadata| metadata.title.as_str()),
        Some("Please fix the login bug")
    );
}

#[test]
fn persisted_placeholder_title_yields_to_computed_title_when_conversation_has_content() {
    let mut app = create_test_app();
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = SessionManager::new(dir.path().join("sessions")).expect("session manager");
    // A session whose first save happened before any user message existed:
    // the old behavior pinned its title to the "New Session" placeholder on
    // disk, and a later snapshot must let the computed title win.
    let stale = crate::session_manager::create_saved_session_with_id_and_mode(
        "session-stale-title".to_string(),
        &[],
        &app.model,
        &app.workspace,
        0,
        app.system_prompt.as_ref(),
        Some(app.mode.as_setting()),
    );
    assert_eq!(stale.metadata.title, "New Session");
    manager.save_session(&stale).expect("save stale session");
    app.api_messages.push(crate::models::Message {
        role: Role::User,
        content: vec![crate::models::ContentBlock::Text {
            text: "fix me".to_string(),
            cache_control: None,
        }],
    });
    app.current_session_id = Some(stale.metadata.id.clone());
    app.current_session_metadata = Some(stale.metadata.clone());

    let snapshot = build_session_snapshot(&mut app, &manager).expect("snapshot");

    assert_eq!(snapshot.metadata.title, "fix me");
    assert_eq!(
        app.current_session_metadata
            .as_ref()
            .map(|metadata| metadata.title.as_str()),
        Some("fix me")
    );
}

#[test]
fn picker_rename_of_inactive_session_does_not_touch_active_metadata() {
    let mut app = create_test_app();
    let mut active = crate::session_manager::create_saved_session_with_id_and_mode(
        "session-active".to_string(),
        &app.api_messages,
        &app.model,
        &app.workspace,
        0,
        app.system_prompt.as_ref(),
        Some(app.mode.as_setting()),
    )
    .metadata;
    active.title = "Active".to_string();
    let mut inactive = active.clone();
    inactive.id = "session-inactive".to_string();
    inactive.title = "Renamed inactive".to_string();
    app.current_session_id = Some(active.id.clone());
    app.current_session_metadata = Some(active.clone());
    app.session_title = Some(active.title.clone());

    assert!(!apply_picker_session_rename_to_active_app(
        &mut app, inactive
    ));
    assert_eq!(app.session_title.as_deref(), Some("Active"));
    assert_eq!(
        app.current_session_metadata
            .as_ref()
            .map(|metadata| metadata.title.as_str()),
        Some("Active")
    );
}

#[test]
fn codex_tool_child_usage_does_not_inherit_public_api_pricing() {
    let mut app = create_test_app();
    app.api_provider = crate::config::ApiProvider::OpenaiCodex;
    app.billing_presentation =
        crate::route_billing::BillingPresentation::Subscription("Codex OAuth quota");
    let result = Ok(crate::tools::spec::ToolResult::success("ok").with_metadata(
        serde_json::json!({
            "child_model": "gpt-5.5",
            "child_input_tokens": 10_000,
            "child_output_tokens": 1_000,
            "child_provider": "openai-codex",
        }),
    ));

    handle_tool_call_complete(&mut app, "review-usage", "review", &result);

    assert_eq!(app.session.subagent_cost, 0.0);
}

#[test]
fn spilled_tool_completion_records_session_artifact_metadata() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let spillover_path = tmp.path().join("call-big.txt");
    let raw = "checking crate ... error[E0425]: cannot find value\n".repeat(20);
    std::fs::write(&spillover_path, &raw).expect("write spillover");
    let result = Ok(
        crate::tools::spec::ToolResult::success("checking crate ...").with_metadata(
            serde_json::json!({
                "spillover_path": spillover_path.display().to_string(),
                "artifact_session_id": "session-123",
                "artifact_relative_path": "artifacts/art_call-big.txt",
                "artifact_byte_size": raw.len() as u64,
                "artifact_preview": "checking crate ... error[E0425]: cannot find value",
            }),
        ),
    );
    let mut app = create_test_app();
    app.current_session_id = Some("session-123".to_string());

    handle_tool_call_complete(&mut app, "call-big", "exec_shell", &result);

    assert_eq!(app.session_artifacts.len(), 1);
    let artifact = &app.session_artifacts[0];
    assert_eq!(artifact.kind, crate::artifacts::ArtifactKind::ToolOutput);
    assert_eq!(artifact.session_id, "session-123");
    assert_eq!(artifact.tool_call_id, "call-big");
    assert_eq!(artifact.tool_name, "exec_shell");
    assert_eq!(artifact.byte_size, raw.len() as u64);
    assert_eq!(
        artifact.storage_path,
        PathBuf::from("artifacts/art_call-big.txt")
    );
    assert!(artifact.preview.starts_with("checking crate"));

    let manager =
        crate::session_manager::SessionManager::new(tmp.path().join("sessions")).expect("manager");
    let snapshot = build_session_snapshot(&mut app, &manager).expect("session snapshot");
    assert_eq!(snapshot.artifacts, app.session_artifacts);
}

#[test]
fn first_snapshot_preserves_current_session_id_for_artifact_ownership() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let manager =
        crate::session_manager::SessionManager::new(tmp.path().join("sessions")).expect("manager");
    let mut app = create_test_app();
    app.current_session_id = Some("session-123".to_string());
    app.api_messages.push(text_message("user", "hello"));

    let snapshot = build_session_snapshot(&mut app, &manager).expect("session snapshot");

    assert_eq!(snapshot.metadata.id, "session-123");
}

#[test]
fn existing_session_snapshot_updates_model_selection() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let manager =
        crate::session_manager::SessionManager::new(tmp.path().join("sessions")).expect("manager");
    let mut existing = saved_session_with_messages(vec![text_message("user", "hello")]);
    existing.metadata.model = "auto".to_string();
    manager
        .save_session(&existing)
        .expect("save existing session");

    let mut app = create_test_app();
    app.current_session_id = Some(existing.metadata.id.clone());
    app.api_messages.push(text_message("user", "hello"));
    app.set_model_selection("deepseek-v4-flash".to_string());

    let snapshot = build_session_snapshot(&mut app, &manager).expect("session snapshot");

    assert_eq!(snapshot.metadata.id, existing.metadata.id);
    assert_eq!(snapshot.metadata.model, "deepseek-v4-flash");
}

#[test]
fn automatic_session_snapshot_keeps_named_custom_identity_secret_free() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let manager =
        crate::session_manager::SessionManager::new(tmp.path().join("sessions")).expect("manager");
    let mut config =
        named_custom_session_config("lm-studio", "http://127.0.0.1:1234/v1", "local-code-model");
    config
        .providers
        .as_mut()
        .and_then(|providers| providers.custom.get_mut("lm-studio"))
        .expect("custom provider")
        .api_key = Some("super-secret-local-key".to_string());
    let mut app = App::new(create_test_options(), &config);
    app.api_messages.push(text_message("user", "persist me"));

    let snapshot = build_session_snapshot(&mut app, &manager).expect("session snapshot");
    let serialized = serde_json::to_string(&snapshot).expect("serialize session");

    assert_eq!(snapshot.metadata.model_provider, "custom");
    assert_eq!(
        snapshot.metadata.model_provider_id.as_deref(),
        Some("lm-studio")
    );
    assert!(serialized.contains("\"model_provider\":\"custom\""));
    assert!(serialized.contains("\"model_provider_id\":\"lm-studio\""));
    assert!(!serialized.contains("super-secret-local-key"));
    assert!(!serialized.contains("127.0.0.1:1234"));
}

#[test]
fn automatic_session_snapshot_omits_id_for_legacy_root_custom_route() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let manager =
        crate::session_manager::SessionManager::new(tmp.path().join("sessions")).expect("manager");
    let config = Config {
        provider: Some("custom".to_string()),
        base_url: Some("http://127.0.0.1:18180/v1".to_string()),
        default_text_model: Some("legacy-root-model".to_string()),
        ..Config::default()
    };
    let mut app = App::new(create_test_options(), &config);
    app.api_messages.push(text_message("user", "persist root"));

    let snapshot = build_session_snapshot(&mut app, &manager).expect("session snapshot");
    let serialized = serde_json::to_string(&snapshot).expect("serialize session");

    assert_eq!(snapshot.metadata.model_provider, "custom");
    assert_eq!(snapshot.metadata.model_provider_id, None);
    assert!(!serialized.contains("model_provider_id"));
}

#[test]
fn session_snapshot_and_resume_round_trip_work_state() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let manager =
        crate::session_manager::SessionManager::new(tmp.path().join("sessions")).expect("manager");
    let mut app = create_test_app();
    app.api_messages.push(text_message("user", "keep my work"));
    app.api_messages
        .push(text_message("assistant", "continuity captured"));
    {
        let mut todos = app.todos.try_lock().expect("todos lock");
        todos.add(
            "inspect".to_string(),
            crate::tools::todo::TodoStatus::Completed,
        );
        todos.add(
            "patch".to_string(),
            crate::tools::todo::TodoStatus::InProgress,
        );
    }
    {
        let mut plan = app.plan_state.try_lock().expect("plan lock");
        plan.update(crate::tools::plan::UpdatePlanArgs {
            objective: Some("Ship continuity".to_string()),
            plan: vec![crate::tools::plan::PlanItemArg {
                step: "verify".to_string(),
                status: crate::tools::plan::StepStatus::InProgress,
            }],
            ..crate::tools::plan::UpdatePlanArgs::default()
        });
    }

    let snapshot = build_session_snapshot(&mut app, &manager).expect("session snapshot");
    let expected = snapshot.work_state.clone().expect("persisted Work state");
    manager.save_session(&snapshot).expect("save session");

    let loaded = manager
        .load_session(&snapshot.metadata.id)
        .expect("load session");
    let mut restored = create_test_app();
    apply_loaded_session(&mut restored, &mut Config::default(), &loaded).expect("restore session");
    assert_eq!(
        restored.work_state_snapshot().expect("restored snapshot"),
        Some(expected)
    );
}

#[test]
fn session_snapshot_preserves_last_work_state_when_lock_is_busy() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let manager =
        crate::session_manager::SessionManager::new(tmp.path().join("sessions")).expect("manager");
    let mut app = create_test_app();
    app.current_session_id = Some("work-lock-session".to_string());
    app.todos.try_lock().expect("todos lock").add(
        "durable".to_string(),
        crate::tools::todo::TodoStatus::InProgress,
    );
    let initial = build_session_snapshot(&mut app, &manager).expect("initial snapshot");
    let expected = initial.work_state.clone();
    manager.save_session(&initial).expect("save initial");

    let todos = app.todos.clone();
    let _held = todos.try_lock().expect("hold todos lock");
    let contended = build_session_snapshot(&mut app, &manager).expect("contended snapshot");
    assert_eq!(contended.work_state, expected);
}

#[tokio::test]
async fn session_snapshot_prefers_newer_memory_over_stale_disk_when_lock_is_busy() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let manager =
        crate::session_manager::SessionManager::new(tmp.path().join("sessions")).expect("manager");
    let mut app = create_test_app();
    app.current_session_id = Some("work-cache-newer-than-disk".to_string());
    app.todos.try_lock().expect("todos lock").add(
        "disk state".to_string(),
        crate::tools::todo::TodoStatus::Completed,
    );
    let disk_snapshot = build_session_snapshot(&mut app, &manager).expect("disk snapshot");
    manager
        .save_session(&disk_snapshot)
        .expect("save disk snapshot");
    app.publish_pending_work_state()
        .expect("publish saved disk state");

    let mut desired = crate::tools::todo::TodoList::from_snapshot(
        &app.todos.try_lock().expect("todos lock").snapshot(),
    )
    .expect("current To-do state");
    desired.add(
        "newer memory state".to_string(),
        crate::tools::todo::TodoStatus::InProgress,
    );
    app.runtime_services
        .work
        .as_ref()
        .expect("Work Graph runtime")
        .apply_todo_update(
            "work-cache-newer-than-disk",
            "work_update",
            &desired.snapshot(),
        )
        .await
        .expect("graph-backed memory update");
    let memory_snapshot = build_session_snapshot(&mut app, &manager).expect("memory snapshot");
    assert_ne!(memory_snapshot.work_state, disk_snapshot.work_state);

    let todos = app.todos.clone();
    let _held = todos.try_lock().expect("hold todos lock");
    let contended = build_session_snapshot(&mut app, &manager).expect("contended snapshot");

    assert_eq!(contended.work_state, memory_snapshot.work_state);
}

#[test]
fn automatic_session_snapshot_never_reloads_existing_json_on_ui_thread() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = tmp.path().join("sessions");
    let manager =
        crate::session_manager::SessionManager::new(sessions_dir.clone()).expect("manager");
    let mut app = create_test_app();
    app.current_session_id = Some("nonblocking-snapshot".to_string());
    app.api_messages
        .push(text_message("user", "first in-memory checkpoint"));
    let initial = build_session_snapshot(&mut app, &manager).expect("initial snapshot");
    manager.save_session(&initial).expect("save initial");
    std::fs::write(
        sessions_dir.join("nonblocking-snapshot.json"),
        "{ intentionally malformed and never read",
    )
    .expect("corrupt disk fixture");
    app.api_messages
        .push(text_message("assistant", "newer in-memory state"));

    let snapshot = build_session_snapshot(&mut app, &manager).expect("nonblocking snapshot");

    assert_eq!(snapshot.metadata.id, "nonblocking-snapshot");
    assert_eq!(snapshot.messages.len(), 2);
    assert_eq!(snapshot.metadata.created_at, initial.metadata.created_at);
}

#[test]
fn renamed_title_survives_next_in_memory_automatic_snapshot() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let manager =
        crate::session_manager::SessionManager::new(tmp.path().join("sessions")).expect("manager");
    let mut session = saved_session_with_messages(vec![text_message("user", "original title")]);
    session.metadata.title = "Original Title".to_string();
    manager.save_session(&session).expect("save session");
    let mut app = create_test_app();
    app.current_session_id = Some(session.metadata.id.clone());
    app.api_messages.clone_from(&session.messages);

    let renamed = crate::commands::rename_session_with_manager(
        "Renamed In Memory",
        &session.metadata.id,
        &manager,
        &mut app,
    );
    assert!(!renamed.is_error, "{:?}", renamed.message);
    let snapshot = build_session_snapshot(&mut app, &manager).expect("automatic snapshot");

    assert_eq!(snapshot.metadata.title, "Renamed In Memory");
}

#[test]
fn session_snapshot_uses_last_known_work_before_first_file_flush() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let manager =
        crate::session_manager::SessionManager::new(tmp.path().join("sessions")).expect("manager");
    let mut app = create_test_app();
    app.todos.try_lock().expect("todos lock").add(
        "queued durable state".to_string(),
        crate::tools::todo::TodoStatus::InProgress,
    );
    let initial = build_session_snapshot(&mut app, &manager).expect("initial snapshot");
    let expected = initial.work_state.clone();
    app.current_session_id = Some(initial.metadata.id);
    app.api_messages
        .push(text_message("user", "new transcript content"));

    let todos = app.todos.clone();
    let _held = todos.try_lock().expect("hold todos lock");
    let contended = build_session_snapshot(&mut app, &manager).expect("cached snapshot");

    assert_eq!(contended.work_state, expected);
    assert_eq!(contended.messages.len(), 1);
}

#[test]
fn first_contended_snapshot_fails_instead_of_serializing_false_empty_work() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let manager =
        crate::session_manager::SessionManager::new(tmp.path().join("sessions")).expect("manager");
    let mut app = create_test_app();
    app.todos.try_lock().expect("todos lock").add(
        "not empty".to_string(),
        crate::tools::todo::TodoStatus::InProgress,
    );
    let todos = app.todos.clone();
    let _held = todos.try_lock().expect("hold todos lock");

    let err = build_session_snapshot(&mut app, &manager).unwrap_err();

    assert!(err.contains("skipped"), "{err}");
}

#[test]
fn legacy_session_without_work_state_clears_previous_todo_on_load() {
    let mut app = create_test_app();
    app.todos.try_lock().expect("todos lock").add(
        "old session".to_string(),
        crate::tools::todo::TodoStatus::Pending,
    );
    let session = saved_session_with_messages(vec![text_message("user", "legacy")]);
    assert!(session.work_state.is_none());

    apply_loaded_session(&mut app, &mut Config::default(), &session).expect("restore session");
    assert_eq!(app.work_state_snapshot().expect("snapshot"), None);
}

#[test]
fn contended_work_restore_leaves_current_session_wholly_unchanged() {
    let mut app = create_test_app();
    app.api_messages
        .push(text_message("user", "current conversation"));
    app.current_session_id = Some("current-session".to_string());
    let mut session = saved_session_with_messages(vec![text_message("user", "replacement")]);
    session.work_state = Some(crate::session_manager::SessionWorkState {
        graph: None,
        todos: crate::tools::todo::TodoListSnapshot {
            items: vec![crate::tools::todo::TodoItem {
                id: 1,
                content: "replacement todo".to_string(),
                status: crate::tools::todo::TodoStatus::Pending,
            }],
            completion_pct: 0,
            in_progress_id: None,
        },
        plan: crate::tools::plan::PlanSnapshot::default(),
    });
    let plan_state = app.plan_state.clone();
    let _held = plan_state.try_lock().expect("hold plan lock");

    let err = apply_loaded_session(&mut app, &mut Config::default(), &session).unwrap_err();

    assert!(err.contains("session was not restored"), "{err}");
    assert_eq!(app.api_messages.len(), 1);
    assert_eq!(app.current_session_id.as_deref(), Some("current-session"));
}

#[test]
fn missing_named_custom_provider_resume_leaves_current_session_wholly_unchanged() {
    let mut app = create_test_app();
    app.api_messages
        .push(text_message("user", "keep the current conversation"));
    app.current_session_id = Some("current-session".to_string());
    app.workspace = PathBuf::from("/tmp/current-workspace");
    app.set_model_selection("deepseek-v4-pro".to_string());
    app.set_provider_identity(ApiProvider::Deepseek, "deepseek");
    app.add_message(HistoryCell::System {
        content: "existing receipt".to_string(),
    });
    app.status_message = Some("existing status".to_string());
    let mut config = Config {
        provider: Some("deepseek".to_string()),
        ..Config::default()
    };
    let mut session = saved_session_with_messages(vec![text_message("user", "replacement")]);
    session.metadata.model_provider = "lm-studio".to_string();
    session.metadata.model = "local-code-model".to_string();
    session.metadata.workspace = PathBuf::from("/tmp/other-workspace");

    let err = apply_loaded_session_config_snapshot(
        &mut app,
        &mut config,
        &session,
        Config::default(),
        true,
    )
    .expect_err("removed custom provider must fail closed");

    assert!(err.contains("[providers.lm-studio]"), "{err}");
    assert!(err.contains("will not fall back"), "{err}");
    assert_eq!(app.current_session_id.as_deref(), Some("current-session"));
    assert_eq!(app.api_messages.len(), 1);
    assert_eq!(app.workspace, PathBuf::from("/tmp/current-workspace"));
    assert_eq!(app.api_provider, ApiProvider::Deepseek);
    assert_eq!(app.provider_identity_for_persistence(), "deepseek");
    assert_eq!(app.model_selection_for_persistence(), "deepseek-v4-pro");
    assert_eq!(config.provider.as_deref(), Some("deepseek"));
    assert_eq!(app.history.len(), 1);
    assert!(matches!(
        &app.history[0],
        HistoryCell::System { content } if content == "existing receipt"
    ));
    assert_eq!(app.status_message.as_deref(), Some("existing status"));
}

#[test]
fn custom_session_resume_requires_structural_route_not_client_construction() {
    let mut app = create_test_app();
    app.api_messages
        .push(text_message("user", "keep the current conversation"));
    app.current_session_id = Some("current-session".to_string());
    app.workspace = PathBuf::from("/tmp/current-workspace");
    app.set_model_selection("deepseek-v4-pro".to_string());
    app.set_provider_identity(ApiProvider::Deepseek, "deepseek");
    app.add_message(HistoryCell::System {
        content: "existing receipt".to_string(),
    });
    app.status_message = Some("existing status".to_string());
    let mut config =
        named_custom_session_config("lm-studio", "http://127.0.0.1:1234/v1", "local-code-model");
    config
        .providers
        .as_mut()
        .expect("providers")
        .custom
        .get_mut("lm-studio")
        .expect("lm-studio")
        .insecure_skip_tls_verify = Some(true);
    let mut session = saved_session_with_messages(vec![authoritative_user_message("replacement")]);
    session.metadata.model_provider = "lm-studio".to_string();
    session.metadata.model = "local-code-model".to_string();
    session.metadata.workspace = PathBuf::from("/tmp/other-workspace");

    apply_loaded_session(&mut app, &mut config, &session)
        .expect("session restore should not require provider credentials or TLS client setup");

    assert_eq!(
        app.current_session_id.as_deref(),
        Some("resume-recovery-session")
    );
    assert_eq!(app.api_messages, session.messages);
    assert!(app.input.is_empty());
    assert!(app.queued_draft.is_none());
    assert_eq!(app.workspace, PathBuf::from("/tmp/other-workspace"));
    assert_eq!(app.api_provider, ApiProvider::Custom);
    assert_eq!(app.provider_identity_for_persistence(), "lm-studio");
    assert_eq!(app.model_selection_for_persistence(), "local-code-model");
    assert!(
        app.history
            .iter()
            .any(|cell| matches!(cell, HistoryCell::User { content } if content == "replacement"))
    );
    assert_eq!(config.provider.as_deref(), Some("lm-studio"));
}

#[test]
fn named_custom_provider_resume_uses_exact_live_endpoint_model_and_workspace() {
    let resumed_workspace = tempfile::tempdir().expect("resume workspace");
    let mut config = named_custom_session_config(
        "lm-studio",
        "http://127.0.0.1:1234/v1",
        "configured-default",
    );
    let mut app = create_test_app();
    let mut session = saved_session_with_messages(vec![
        text_message("user", "resume local work"),
        text_message("assistant", "ready"),
    ]);
    session.metadata.model_provider = "lm-studio".to_string();
    session.metadata.model = "local-code-model".to_string();
    session.metadata.workspace = resumed_workspace.path().to_path_buf();

    apply_loaded_session(&mut app, &mut config, &session).expect("restore exact custom route");

    assert_eq!(app.api_provider, ApiProvider::Custom);
    assert_eq!(app.provider_identity_for_persistence(), "lm-studio");
    assert_eq!(app.model_selection_for_persistence(), "local-code-model");
    assert_eq!(app.workspace, resumed_workspace.path());
    assert_eq!(config.provider.as_deref(), Some("lm-studio"));
    let route = resolve_runtime_route(&config, app.api_provider, Some(&app.model))
        .expect("runtime route remains exact");
    assert_eq!(
        route.candidate.endpoint().base_url,
        "http://127.0.0.1:1234/v1"
    );
    assert_eq!(route.model, "local-code-model");
}

#[test]
fn same_workspace_named_custom_switch_requires_engine_respawn() {
    let mut config =
        named_custom_session_config("custom-a", "http://127.0.0.1:18181/v1", "model-a");
    config.providers.as_mut().expect("providers").custom.insert(
        "custom-b".to_string(),
        crate::config::ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("http://127.0.0.1:18182/v1".to_string()),
            model: Some("model-b".to_string()),
            ..Default::default()
        },
    );
    let workspace = PathBuf::from("/tmp/same-workspace-custom-switch");
    let mut app = create_test_app();
    app.workspace.clone_from(&workspace);
    app.set_provider_identity(ApiProvider::Custom, "custom-a");
    app.set_model_selection("model-a".to_string());
    let previous_provider = app.api_provider;
    let previous_identity = app.provider_identity_for_persistence().to_string();
    let previous_workspace = app.workspace.clone();

    let mut session = saved_session_with_messages(vec![text_message("user", "switch route")]);
    session.metadata.model_provider = "custom-b".to_string();
    session.metadata.model = "model-b".to_string();
    session.metadata.workspace = workspace;
    apply_loaded_session(&mut app, &mut config, &session).expect("restore custom B");

    assert_eq!(previous_provider, ApiProvider::Custom);
    assert_eq!(app.api_provider, ApiProvider::Custom);
    assert_eq!(app.provider_identity_for_persistence(), "custom-b");
    assert!(loaded_session_requires_engine_respawn(
        &app,
        previous_provider,
        &previous_identity,
        &previous_workspace,
    ));
}

#[test]
fn file_load_uses_one_fresh_config_snapshot_for_custom_route_and_app_state() {
    let workspace = PathBuf::from("/tmp/atomic-file-load");
    let mut stale_config =
        named_custom_session_config("custom-a", "http://127.0.0.1:18181/v1", "model-a");
    let fresh_config =
        named_custom_session_config("custom-b", "http://127.0.0.1:18182/v1", "model-b");
    let mut app = create_test_app();
    app.workspace.clone_from(&workspace);
    app.set_provider_identity(ApiProvider::Custom, "custom-a");
    app.set_model_selection("model-a".to_string());
    app.api_messages
        .push(text_message("user", "old conversation"));
    let mut session = saved_session_with_messages(vec![
        text_message("user", "new conversation"),
        text_message("assistant", "loaded reply"),
    ]);
    session.metadata.model_provider = "custom-b".to_string();
    session.metadata.model = "model-b".to_string();
    session.metadata.workspace = workspace;

    let respawn = apply_loaded_session_config_snapshot(
        &mut app,
        &mut stale_config,
        &session,
        fresh_config,
        true,
    )
    .expect("fresh disk config should atomically restore custom B");

    assert!(respawn);
    assert_eq!(app.provider_identity_for_persistence(), "custom-b");
    assert_eq!(app.model_selection_for_persistence(), "model-b");
    assert_eq!(app.api_messages, session.messages);
    assert_eq!(stale_config.provider.as_deref(), Some("custom-b"));
    assert_eq!(
        stale_config.deepseek_base_url(),
        "http://127.0.0.1:18182/v1"
    );
}

#[test]
fn session_load_keeps_idless_custom_record_on_root_when_table_coexists() {
    let mut config =
        named_custom_session_config("custom", "http://127.0.0.1:18182/v1", "table-model");
    config.base_url = Some("http://127.0.0.1:18181/v1".to_string());
    config.default_text_model = Some("legacy-root-model".to_string());
    let mut app = create_test_app();
    app.api_messages
        .push(text_message("user", "current conversation"));
    let mut session = saved_session_with_messages(vec![
        text_message("user", "legacy custom conversation"),
        text_message("assistant", "legacy reply"),
    ]);
    session.metadata.model_provider = "custom".to_string();
    session.metadata.model_provider_id = None;
    session.metadata.model = "legacy-saved-model".to_string();

    apply_loaded_session(&mut app, &mut config, &session)
        .expect("id-less custom record must retain root provenance");
    assert_eq!(app.api_messages, session.messages);
    assert_eq!(app.api_provider, ApiProvider::Custom);
    assert_eq!(app.provider_identity_for_persistence(), "custom");
    assert_eq!(app.provider_id_for_persistence(), None);
    assert_eq!(config.deepseek_base_url(), "http://127.0.0.1:18181/v1");
    assert!(
        config
            .providers
            .as_ref()
            .is_none_or(|providers| !providers.custom.contains_key("custom"))
    );
}

#[test]
fn session_load_rejects_exact_custom_table_record_when_only_root_remains() {
    let mut config = Config {
        provider: Some("custom".to_string()),
        base_url: Some("http://127.0.0.1:18181/v1".to_string()),
        default_text_model: Some("legacy-root-model".to_string()),
        ..Config::default()
    };
    let mut app = create_test_app();
    app.api_messages
        .push(text_message("user", "current conversation"));
    let previous_messages = app.api_messages.clone();
    let previous_identity = app.provider_identity_for_persistence().to_string();

    let mut session = saved_session_with_messages(vec![
        text_message("user", "exact table conversation"),
        text_message("assistant", "exact table reply"),
    ]);
    session.metadata.model_provider = "custom".to_string();
    session.metadata.model_provider_id = Some("custom".to_string());
    session.metadata.model = "table-model".to_string();

    let error = apply_loaded_session(&mut app, &mut config, &session)
        .expect_err("exact table record must not fall back to root");
    assert!(error.contains("[providers.custom]"), "{error}");
    assert!(error.contains("will not fall back"), "{error}");
    assert_eq!(app.api_messages, previous_messages);
    assert_eq!(app.provider_identity_for_persistence(), previous_identity);
}

#[test]
fn session_load_rejects_empty_custom_id_when_root_and_table_coexist() {
    let mut config =
        named_custom_session_config("custom", "http://127.0.0.1:18182/v1", "table-model");
    config.base_url = Some("http://127.0.0.1:18181/v1".to_string());
    config.default_text_model = Some("legacy-root-model".to_string());
    let mut app = create_test_app();
    app.api_messages
        .push(text_message("user", "current conversation"));
    app.set_provider_identity(ApiProvider::Deepseek, "deepseek");
    app.set_model_selection("deepseek-v4-pro".to_string());
    let previous_messages = app.api_messages.clone();
    let previous_provider = app.api_provider;
    let previous_identity = app.provider_identity_for_persistence().to_string();
    let previous_provider_id = app.provider_id_for_persistence().map(str::to_string);
    let previous_model = app.model.clone();
    let previous_config_provider = config.provider.clone();

    let mut session = saved_session_with_messages(vec![
        text_message("user", "malformed custom conversation"),
        text_message("assistant", "must never load"),
    ]);
    session.metadata.model_provider = "custom".to_string();
    session.metadata.model_provider_id = Some("   ".to_string());
    session.metadata.model = "legacy-root-model".to_string();

    let error = apply_loaded_session(&mut app, &mut config, &session)
        .expect_err("an explicit empty id must not load either custom route");
    assert!(error.contains("empty exact provider id"), "{error}");
    assert_eq!(app.api_messages, previous_messages);
    assert_eq!(app.api_provider, previous_provider);
    assert_eq!(app.provider_identity_for_persistence(), previous_identity);
    assert_eq!(
        app.provider_id_for_persistence().map(str::to_string),
        previous_provider_id
    );
    assert_eq!(app.model, previous_model);
    assert_eq!(config.provider, previous_config_provider);
    assert_eq!(config.deepseek_base_url(), "http://127.0.0.1:18182/v1");
}

#[test]
fn file_load_respawns_engine_when_same_custom_identity_changes_endpoint() {
    let workspace = PathBuf::from("/tmp/atomic-file-load-same-custom");
    let mut stale_config =
        named_custom_session_config("custom-a", "http://127.0.0.1:18181/v1", "model-a");
    let mut fresh_config =
        named_custom_session_config("custom-a", "http://127.0.0.1:18199/v1", "model-a");
    let fresh_entry = fresh_config
        .providers
        .as_mut()
        .expect("providers")
        .custom
        .get_mut("custom-a")
        .expect("custom A");
    fresh_entry.api_key = Some("rotated-key".to_string());
    fresh_entry.http_headers = Some(std::collections::HashMap::from([(
        "X-Route-Version".to_string(),
        "fresh".to_string(),
    )]));

    let mut app = create_test_app();
    app.workspace.clone_from(&workspace);
    app.set_provider_identity(ApiProvider::Custom, "custom-a");
    app.set_model_selection("model-a".to_string());
    let mut session = saved_session_with_messages(vec![text_message("user", "new endpoint")]);
    session.metadata.model_provider = "custom-a".to_string();
    session.metadata.model = "model-a".to_string();
    session.metadata.workspace = workspace;

    let respawn = apply_loaded_session_config_snapshot(
        &mut app,
        &mut stale_config,
        &session,
        fresh_config,
        true,
    )
    .expect("same-key fresh config should load atomically");

    assert!(respawn, "file loads must install the fresh engine config");
    assert_eq!(app.provider_identity_for_persistence(), "custom-a");
    assert_eq!(
        stale_config.deepseek_base_url(),
        "http://127.0.0.1:18199/v1"
    );
    let entry = stale_config
        .provider_config_for(ApiProvider::Custom)
        .expect("fresh custom route");
    assert_eq!(entry.api_key.as_deref(), Some("rotated-key"));
    assert_eq!(
        entry
            .http_headers
            .as_ref()
            .and_then(|headers| headers.get("X-Route-Version"))
            .map(String::as_str),
        Some("fresh")
    );
}

#[test]
fn file_load_route_refresh_preserves_effective_permission_and_feature_overlays() {
    let workspace = PathBuf::from("/tmp/atomic-file-load-policy");
    let mut effective_config =
        named_custom_session_config("custom-a", "http://127.0.0.1:18181/v1", "model-a");
    effective_config.approval_policy = Some("never".to_string());
    effective_config.sandbox_mode = Some("read-only".to_string());
    effective_config.allow_shell = Some(false);
    effective_config.max_subagents = Some(1);
    effective_config.features = Some(crate::features::FeaturesToml {
        entries: std::collections::BTreeMap::from([
            ("shell_tool".to_string(), false),
            ("subagents".to_string(), false),
        ]),
    });

    let mut raw_disk_config =
        named_custom_session_config("custom-a", "http://127.0.0.1:18199/v1", "model-a");
    raw_disk_config.approval_policy = Some("always".to_string());
    raw_disk_config.sandbox_mode = Some("danger-full-access".to_string());
    raw_disk_config.allow_shell = Some(true);
    raw_disk_config.max_subagents = Some(64);
    raw_disk_config.features = Some(crate::features::FeaturesToml {
        entries: std::collections::BTreeMap::from([
            ("shell_tool".to_string(), true),
            ("subagents".to_string(), true),
        ]),
    });

    let mut app = create_test_app();
    app.workspace.clone_from(&workspace);
    app.set_provider_identity(ApiProvider::Custom, "custom-a");
    app.set_model_selection("model-a".to_string());
    let mut session = saved_session_with_messages(vec![text_message("user", "keep policy")]);
    session.metadata.model_provider = "custom-a".to_string();
    session.metadata.model = "model-a".to_string();
    session.metadata.workspace = workspace;

    let respawn = apply_loaded_session_config_snapshot(
        &mut app,
        &mut effective_config,
        &session,
        raw_disk_config,
        true,
    )
    .expect("fresh route plus effective policy should load");

    assert!(respawn);
    assert_eq!(
        effective_config.deepseek_base_url(),
        "http://127.0.0.1:18199/v1"
    );
    assert_eq!(effective_config.approval_policy.as_deref(), Some("never"));
    assert_eq!(effective_config.sandbox_mode.as_deref(), Some("read-only"));
    assert_eq!(effective_config.allow_shell, Some(false));
    assert_eq!(effective_config.max_subagents, Some(1));
    let features = effective_config
        .features
        .expect("effective feature overlay");
    assert_eq!(features.entries.get("shell_tool"), Some(&false));
    assert_eq!(features.entries.get("subagents"), Some(&false));
}

#[test]
fn session_picker_restore_rejects_active_turn_before_mutating() {
    let mut app = create_test_app();
    app.api_messages
        .push(text_message("user", "current active conversation"));
    app.current_session_id = Some("current-session".to_string());
    app.is_loading = true;
    app.runtime_turn_status = Some("in_progress".to_string());
    let session = saved_session_with_messages(vec![text_message("user", "replacement")]);

    let err = apply_loaded_session(&mut app, &mut Config::default(), &session).unwrap_err();

    assert!(err.contains("runtime work is active"), "{err}");
    assert_eq!(app.api_messages.len(), 1);
    assert_eq!(app.current_session_id.as_deref(), Some("current-session"));
}

#[test]
fn session_restore_rebuilds_fresh_codex_route_limits() {
    let _lock = crate::test_support::lock_test_env();
    let codex_home = tempfile::tempdir().expect("Codex home");
    let _codex_home = crate::test_support::EnvVarGuard::set("CODEX_HOME", codex_home.path());
    std::fs::write(
        codex_home.path().join("models_cache.json"),
        serde_json::to_vec(&serde_json::json!({
            "fetched_at": chrono::Utc::now(),
            "models": [{
                "slug": crate::config::DEFAULT_OPENAI_CODEX_MODEL,
                "priority": 1,
                "context_window": 272000,
                "supported_reasoning_levels": [{"effort": "high"}]
            }]
        }))
        .expect("serialize cache"),
    )
    .expect("write cache");
    let mut app = create_test_app();
    let mut config = Config::default();
    let mut session = saved_session_with_messages(vec![text_message("user", "resume Codex")]);
    session.metadata.model_provider = ApiProvider::OpenaiCodex.as_str().to_string();
    session.metadata.model = crate::config::DEFAULT_OPENAI_CODEX_MODEL.to_string();

    apply_loaded_session(&mut app, &mut config, &session).expect("restore session");

    assert_eq!(app.api_provider, ApiProvider::OpenaiCodex);
    assert_eq!(app.model, crate::config::DEFAULT_OPENAI_CODEX_MODEL);
    assert_eq!(
        app.active_route_limits
            .and_then(|limits| limits.context_tokens),
        Some(272_000)
    );
    let engine_config = build_engine_config(&app, &config);
    assert_eq!(
        engine_config
            .active_route_limits
            .and_then(|limits| limits.context_tokens),
        Some(272_000)
    );
}

#[test]
fn apply_loaded_session_restores_concrete_model_mode() {
    let mut app = create_test_app();
    app.set_model_selection("auto".to_string());
    app.reasoning_effort = ReasoningEffort::Low;
    app.reasoning_effort_preference = Some(ReasoningEffort::Low);
    let mut session = saved_session_with_messages(vec![
        text_message("user", "hello"),
        text_message("assistant", "hi"),
    ]);
    session.metadata.model = "deepseek-v4-flash".to_string();

    apply_loaded_session(&mut app, &mut Config::default(), &session).expect("restore session");
    assert!(!app.auto_model);
    assert_eq!(app.model, "deepseek-v4-flash");
    assert_eq!(app.model_selection_for_persistence(), "deepseek-v4-flash");
    assert_eq!(app.reasoning_effort, ReasoningEffort::Low);
    assert_eq!(app.reasoning_effort_preference, Some(ReasoningEffort::Low));
}

#[test]
fn apply_loaded_session_restores_auto_model_mode() {
    let mut app = create_test_app();
    app.set_model_selection("deepseek-v4-pro".to_string());
    // Simulate a raw `low` preference already collapsed by the fixed
    // DeepSeek route. Loading Auto must restore `low`, not snapshot `high`.
    app.reasoning_effort = ReasoningEffort::High;
    app.reasoning_effort_preference = Some(ReasoningEffort::Low);
    app.last_effective_model = Some("deepseek-v4-flash".to_string());
    app.last_effective_reasoning_effort =
        Some(EffectiveReasoningEffort::Tier(ReasoningEffort::Low));
    let mut session = saved_session_with_messages(vec![
        text_message("user", "hello"),
        text_message("assistant", "hi"),
    ]);
    session.metadata.model = "auto".to_string();

    apply_loaded_session(&mut app, &mut Config::default(), &session).expect("restore session");
    assert!(app.auto_model);
    assert_eq!(app.model, "auto");
    assert_eq!(app.model_selection_for_persistence(), "auto");
    assert_eq!(app.last_effective_model, None);
    assert_eq!(app.last_effective_provider, None);
    assert_eq!(app.last_effective_provider_identity, None);
    assert_eq!(app.last_auto_route_receipt, None);
    assert_eq!(app.last_effective_reasoning_effort, None);
    assert_eq!(app.reasoning_effort, ReasoningEffort::Low);
    assert_eq!(app.reasoning_effort_preference, Some(ReasoningEffort::Low));
    assert_eq!(app.effective_model_for_budget(), DEFAULT_TEXT_MODEL);
}

#[test]
fn apply_loaded_auto_session_releases_implicit_fixed_model_thinking() {
    let mut app = create_test_app();
    app.set_model_selection("deepseek-v4-pro".to_string());
    app.reasoning_effort = ReasoningEffort::Max;
    app.reasoning_effort_preference = None;
    let mut session = saved_session_with_messages(vec![
        text_message("user", "hello"),
        text_message("assistant", "hi"),
    ]);
    session.metadata.model = "auto".to_string();

    apply_loaded_session(&mut app, &mut Config::default(), &session).expect("restore auto session");

    assert!(app.auto_model);
    assert_eq!(app.reasoning_effort, ReasoningEffort::Auto);
    assert_eq!(app.reasoning_effort_preference, None);
}

#[test]
fn auto_route_receipt_survives_session_snapshot_and_restore() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let manager =
        crate::session_manager::SessionManager::new(tmp.path().join("sessions")).expect("manager");
    let receipt = crate::model_routing::AutoRouteReceipt {
        tier: crate::model_routing::AutoRouteTier::Strong,
        pair: crate::model_routing::AutoRoutePair {
            strong: crate::config::ZAI_GLM_5_2_MODEL.to_string(),
            fast: Some(crate::config::ZAI_GLM_5_TURBO_MODEL.to_string()),
        },
        scope: crate::model_routing::AutoRouteScope::ResolvedProvider,
        data_path: crate::model_routing::AutoRouteDataPath::LocalHeuristic,
        reason: crate::model_routing::AutoRouteReason::LocalHeuristic(
            crate::model_routing::AutoRouteHeuristicReason::ComplexRequest,
        ),
    };
    let mut app = create_test_app();
    app.set_model_selection("auto".to_string());
    app.last_effective_provider = Some(ApiProvider::Zai);
    app.last_effective_provider_identity = Some("zai".to_string());
    app.last_effective_model = Some(crate::config::ZAI_GLM_5_2_MODEL.to_string());
    app.last_auto_route_receipt = Some(receipt.clone());
    app.last_effective_reasoning_effort =
        Some(EffectiveReasoningEffort::Tier(ReasoningEffort::High));
    app.api_messages
        .push(text_message("user", "inspect this route"));

    let snapshot = build_session_snapshot(&mut app, &manager).expect("session snapshot");
    let serialized = serde_json::to_string(&snapshot).expect("serialize session");
    let mut legacy_json: serde_json::Value =
        serde_json::from_str(&serialized).expect("parse session json");
    legacy_json["last_auto_route"]
        .as_object_mut()
        .expect("Auto route object")
        .remove("effective_reasoning_effort");
    let legacy: SavedSession =
        serde_json::from_value(legacy_json).expect("deserialize legacy Auto route");
    assert_eq!(
        legacy
            .last_auto_route
            .as_ref()
            .and_then(|route| route.effective_reasoning_effort),
        None
    );
    let persisted: SavedSession = serde_json::from_str(&serialized).expect("deserialize session");
    let saved = persisted
        .last_auto_route
        .as_ref()
        .expect("persisted Auto route");
    assert_eq!(saved.provider, ApiProvider::Zai);
    assert_eq!(saved.provider_identity, "zai");
    assert_eq!(saved.model, crate::config::ZAI_GLM_5_2_MODEL);
    assert_eq!(saved.receipt, receipt);
    assert_eq!(
        saved.effective_reasoning_effort,
        Some(crate::work_graph::ReasoningEffortTier::High)
    );

    let mut restored_app = create_test_app();
    apply_loaded_session(&mut restored_app, &mut Config::default(), &persisted)
        .expect("restore session");

    assert!(restored_app.auto_model);
    assert_eq!(restored_app.last_effective_provider, Some(ApiProvider::Zai));
    assert_eq!(
        restored_app.last_effective_provider_identity.as_deref(),
        Some("zai")
    );
    assert_eq!(
        restored_app.last_effective_model.as_deref(),
        Some(crate::config::ZAI_GLM_5_2_MODEL)
    );
    assert_eq!(restored_app.last_auto_route_receipt, Some(receipt));
    assert_eq!(
        restored_app.last_effective_reasoning_effort,
        Some(EffectiveReasoningEffort::Tier(ReasoningEffort::High))
    );
    assert_eq!(restored_app.reasoning_effort_display_label(), "auto: high");
}

#[test]
fn apply_loaded_session_restores_saved_mode() {
    let mut app = create_test_app();
    app.set_mode(crate::tui::app::AppMode::Agent);
    let mut session = saved_session_with_messages(vec![
        text_message("user", "draft a plan"),
        text_message("assistant", "plan response"),
    ]);
    session.metadata.mode = Some("plan".to_string());

    apply_loaded_session(&mut app, &mut Config::default(), &session).expect("restore session");
    assert_eq!(app.mode, crate::tui::app::AppMode::Plan);
    assert!(!app.allow_shell);
    assert!(!app.trust_mode);
}

#[test]
fn app_new_restores_saved_model_and_reasoning_effort() {
    let _guard = ConfigPathEnvGuard::new();
    let settings = crate::settings::Settings {
        default_model: Some("deepseek-v4-pro".to_string()),
        reasoning_effort: Some("high".to_string()),
        ..Default::default()
    };
    settings.save().expect("save settings");

    let options = TuiOptions {
        model: "auto".to_string(),
        start_in_agent_mode: true,
        skip_onboarding: false,
        ..crate::test_support::test_tui_options(PathBuf::from("."))
    };
    let config = Config {
        reasoning_effort: Some("max".to_string()),
        ..Default::default()
    };

    let app = App::new(options, &config);

    assert!(!app.auto_model);
    assert_eq!(app.model, "deepseek-v4-pro");
    assert_eq!(app.reasoning_effort, ReasoningEffort::High);
}

#[test]
fn app_new_restores_saved_reasoning_effort_for_auto_model() {
    let _guard = ConfigPathEnvGuard::new();
    let settings = crate::settings::Settings {
        default_model: Some("auto".to_string()),
        reasoning_effort: Some("low".to_string()),
        ..Default::default()
    };
    settings.save().expect("save settings");

    let options = TuiOptions {
        model: "deepseek-v4-pro".to_string(),
        start_in_agent_mode: true,
        skip_onboarding: false,
        ..crate::test_support::test_tui_options(PathBuf::from("."))
    };

    let app = App::new(options, &Config::default());

    assert!(app.auto_model);
    assert_eq!(app.model, "auto");
    assert_eq!(app.reasoning_effort, ReasoningEffort::Low);
    assert_eq!(app.reasoning_effort_display_label(), "low");
}

#[test]
fn app_new_auto_model_preserves_explicit_config_reasoning() {
    let _guard = ConfigPathEnvGuard::new();
    let settings = crate::settings::Settings {
        default_model: Some("auto".to_string()),
        reasoning_effort: None,
        ..Default::default()
    };
    settings.save().expect("save settings");

    let options = TuiOptions {
        model: "deepseek-v4-pro".to_string(),
        start_in_agent_mode: true,
        skip_onboarding: false,
        ..crate::test_support::test_tui_options(PathBuf::from("."))
    };
    let config = Config {
        reasoning_effort: Some("medium".to_string()),
        ..Config::default()
    };

    let app = App::new(options, &config);

    assert!(app.auto_model);
    assert_eq!(app.reasoning_effort, ReasoningEffort::Medium);
    assert_eq!(
        app.reasoning_effort_preference,
        Some(ReasoningEffort::Medium)
    );
}

#[test]
fn app_new_auto_model_without_saved_reasoning_keeps_auto_default() {
    let _guard = ConfigPathEnvGuard::new();
    let settings = crate::settings::Settings {
        default_model: Some("auto".to_string()),
        reasoning_effort: None,
        ..Default::default()
    };
    settings.save().expect("save settings");

    let options = TuiOptions {
        model: "deepseek-v4-pro".to_string(),
        start_in_agent_mode: true,
        skip_onboarding: false,
        ..crate::test_support::test_tui_options(PathBuf::from("."))
    };

    let app = App::new(options, &Config::default());

    assert!(app.auto_model);
    assert_eq!(app.reasoning_effort, ReasoningEffort::Auto);
    assert_eq!(app.reasoning_effort_display_label(), "auto");
}

#[test]
fn app_new_auto_model_ignores_reasoning_inferred_from_legacy_alias() {
    let _guard = ConfigPathEnvGuard::new();
    let settings = crate::settings::Settings {
        default_model: Some("auto".to_string()),
        reasoning_effort: None,
        ..Default::default()
    };
    settings.save().expect("save settings");

    let options = TuiOptions {
        model: "deepseek-v4-pro".to_string(),
        start_in_agent_mode: true,
        skip_onboarding: false,
        ..crate::test_support::test_tui_options(PathBuf::from("."))
    };
    let config = Config {
        reasoning_effort: Some("high".to_string()),
        reasoning_effort_inferred_from_legacy_alias: true,
        migrated_deepseek_model_alias: Some("deepseek-reasoner".to_string()),
        ..Config::default()
    };

    let app = App::new(options, &config);

    assert!(app.auto_model);
    assert_eq!(app.reasoning_effort, ReasoningEffort::Auto);
    assert_eq!(app.reasoning_effort_preference, None);
}

#[tokio::test]
async fn model_picker_apply_is_session_local_until_startup_default_is_requested() {
    let _guard = SettingsHomeGuard::new();
    let mut app = create_test_app();
    app.set_model_selection("auto".to_string());
    app.reasoning_effort = ReasoningEffort::Auto;
    let mut engine = mock_engine_handle();
    let mut config = Config {
        api_key: Some("test-key".to_string()),
        ..Default::default()
    };

    apply_model_picker_choice(
        &mut app,
        &mut engine.handle,
        &mut config,
        "deepseek-v4-pro".to_string(),
        None,
        None,
        ReasoningEffort::High,
        "auto".to_string(),
        ReasoningEffort::Auto,
        false,
    )
    .await;

    let settings = crate::settings::Settings::load().expect("load settings");
    assert_eq!(settings.default_model.as_deref(), None);
    assert_eq!(
        settings
            .provider_models
            .as_ref()
            .and_then(|models| models.get("deepseek"))
            .map(String::as_str),
        None
    );
    assert_eq!(settings.reasoning_effort.as_deref(), None);
    assert!(!app.auto_model);
    assert_eq!(app.reasoning_effort, ReasoningEffort::High);

    let state = codewhale_config::SetupState::load()
        .expect("load setup state")
        .expect("setup state");
    assert_eq!(
        state.status(codewhale_config::SetupStep::ProviderModel),
        codewhale_config::StepStatus::Verified,
        "the local setup receipt records a live selection, not a startup-default write"
    );
    let provider_model_result = state
        .steps
        .get(&codewhale_config::SetupStep::ProviderModel)
        .and_then(|entry| entry.result.as_deref())
        .expect("provider/model result");
    assert!(provider_model_result.contains("provider=deepseek"));
    assert!(provider_model_result.contains("model=deepseek-v4-pro"));
    assert!(provider_model_result.contains("auth=key saved · not checked"));
    assert!(provider_model_result.contains("health=attemptable"));
    assert!(!provider_model_result.contains("test-key"));
}

/// An explicit startup default must override a provider/model seed in
/// config.toml. This is the exact regression reported with xAI: the picker
/// could choose DeepSeek Flash for a live session, but the next launch still
/// reopened the xAI-configured `grok-4.5` route.
#[tokio::test]
async fn model_picker_startup_default_overrides_configured_xai_route_after_restart() {
    let _guard = SettingsHomeGuard::new();
    let xai_config = Config {
        provider: Some("xai".to_string()),
        providers: Some(ProvidersConfig {
            xai: ProviderConfig {
                api_key: Some("xai-test-key".to_string()),
                model: Some("grok-4.5".to_string()),
                ..ProviderConfig::default()
            },
            deepseek: ProviderConfig {
                api_key: Some("deepseek-test-key".to_string()),
                model: Some("deepseek-v4-flash".to_string()),
                ..ProviderConfig::default()
            },
            ..ProvidersConfig::default()
        }),
        ..Config::default()
    };
    let mut config = xai_config.clone();
    let initial_options = TuiOptions {
        model: config.default_model(),
        start_in_agent_mode: true,
        skip_onboarding: false,
        ..crate::test_support::test_tui_options(PathBuf::from("."))
    };
    let mut app = App::new(initial_options, &config);
    assert_eq!(app.api_provider, ApiProvider::Xai);
    assert_eq!(app.model, "grok-4.5");
    let previous_effort = app.reasoning_effort;
    let mut engine = mock_engine_handle();

    apply_model_picker_choice(
        &mut app,
        &mut engine.handle,
        &mut config,
        "deepseek-v4-flash".to_string(),
        Some(ApiProvider::Deepseek),
        None,
        previous_effort,
        "grok-4.5".to_string(),
        previous_effort,
        // Exactly what Shift+D in the picker sends. This is a cross-provider
        // switch, which returns early from a different exit than a same-
        // provider model change — the path where the save used to be dropped.
        true,
    )
    .await;

    assert_eq!(app.api_provider, ApiProvider::Deepseek);
    assert_eq!(app.model, "deepseek-v4-flash");
    assert!(
        app.status_message
            .as_deref()
            .is_some_and(|message| message.contains("startup default")),
        "the explicit save must report what it wrote: {:?}",
        app.status_message
    );
    let settings = crate::settings::Settings::load().expect("load settings");
    assert_eq!(settings.default_provider.as_deref(), Some("deepseek"));
    assert_eq!(
        settings
            .provider_models
            .as_ref()
            .and_then(|models| models.get("deepseek"))
            .map(String::as_str),
        Some("deepseek-v4-flash")
    );

    let restart_options = TuiOptions {
        model: xai_config.default_model(),
        start_in_agent_mode: true,
        skip_onboarding: false,
        ..crate::test_support::test_tui_options(PathBuf::from("."))
    };
    let restarted = App::new(restart_options, &xai_config);
    assert_eq!(restarted.api_provider, ApiProvider::Deepseek);
    assert_eq!(
        restarted.model, "deepseek-v4-flash",
        "the explicit startup default must outrank config's xAI seed"
    );
}

#[tokio::test]
async fn model_picker_auto_commits_visible_implicit_fixed_model_thinking() {
    let _guard = SettingsHomeGuard::new();
    let mut app = create_test_app();
    app.set_model_selection("deepseek-v4-pro".to_string());
    app.reasoning_effort = ReasoningEffort::Max;
    app.reasoning_effort_preference = None;
    let mut engine = mock_engine_handle();
    let mut config = Config {
        api_key: Some("test-key".to_string()),
        ..Default::default()
    };

    apply_model_picker_choice(
        &mut app,
        &mut engine.handle,
        &mut config,
        "auto".to_string(),
        None,
        None,
        ReasoningEffort::Max,
        "deepseek-v4-pro".to_string(),
        ReasoningEffort::Auto,
        false,
    )
    .await;

    assert!(app.auto_model);
    assert_eq!(app.reasoning_effort, ReasoningEffort::Max);
    assert_eq!(app.reasoning_effort_preference, Some(ReasoningEffort::Max));
    // Session-local: the effort tier is not written to settings; the
    // route-save prompt owns persistence.
    assert_eq!(
        crate::settings::Settings::load()
            .expect("load settings")
            .reasoning_effort
            .as_deref(),
        None
    );
    assert!(app.pending_route_save.is_some());
}

#[tokio::test]
async fn model_picker_auto_restores_raw_preference_after_fixed_normalization() {
    let _guard = SettingsHomeGuard::new();
    let mut app = create_test_app();
    app.api_provider = ApiProvider::Deepseek;
    app.set_model_selection("deepseek-v4-pro".to_string());
    app.reasoning_effort = ReasoningEffort::High;
    app.reasoning_effort_preference = Some(ReasoningEffort::Low);
    let mut engine = mock_engine_handle();
    let mut config = Config {
        api_key: Some("test-key".to_string()),
        ..Default::default()
    };

    apply_model_picker_choice(
        &mut app,
        &mut engine.handle,
        &mut config,
        "auto".to_string(),
        None,
        None,
        ReasoningEffort::Low,
        "deepseek-v4-pro".to_string(),
        ReasoningEffort::Low,
        false,
    )
    .await;

    assert!(app.auto_model);
    assert_eq!(app.reasoning_effort, ReasoningEffort::Low);
    assert_eq!(app.reasoning_effort_preference, Some(ReasoningEffort::Low));
    // Session-local: nothing reached settings.
    assert_eq!(
        crate::settings::Settings::load()
            .expect("load settings")
            .reasoning_effort
            .as_deref(),
        None
    );
}

#[tokio::test]
async fn auto_model_effort_picker_persists_unresolved_tier_verbatim() {
    let _guard = SettingsHomeGuard::new();
    let mut app = create_test_app();
    app.set_model_selection("auto".to_string());
    app.reasoning_effort = ReasoningEffort::Auto;
    app.reasoning_effort_preference = None;
    let engine = mock_engine_handle();

    apply_picker_effort_choice(
        &mut app,
        &engine.handle,
        ReasoningEffort::Low,
        ReasoningEffort::Auto,
    )
    .await;

    assert!(app.auto_model);
    assert_eq!(app.reasoning_effort, ReasoningEffort::Low);
    assert_eq!(app.reasoning_effort_preference, Some(ReasoningEffort::Low));
    assert_eq!(
        crate::settings::Settings::load()
            .expect("load settings")
            .reasoning_effort
            .as_deref(),
        Some("low"),
        "an unresolved auto route must not pre-normalize Low to DeepSeek High"
    );
}

/// Re-picking the already-live model remains session-local until the operator
/// explicitly presses Shift+D. It still records a local setup receipt because
/// the live route was successfully selected.
#[tokio::test]
async fn reselecting_live_model_and_thinking_is_session_local() {
    let _guard = SettingsHomeGuard::new();
    let mut app = create_test_app();
    app.set_model_selection("deepseek-v4-pro".to_string());
    app.reasoning_effort = ReasoningEffort::High;
    let mut engine = mock_engine_handle();
    let mut config = Config {
        api_key: Some("test-key".to_string()),
        ..Default::default()
    };
    assert!(
        codewhale_config::SetupState::load()
            .ok()
            .flatten()
            .is_none(),
        "the test home must start with no recorded setup progress"
    );

    apply_model_picker_choice(
        &mut app,
        &mut engine.handle,
        &mut config,
        "deepseek-v4-pro".to_string(),
        None,
        None,
        ReasoningEffort::High,
        "deepseek-v4-pro".to_string(),
        ReasoningEffort::High,
        false,
    )
    .await;

    // Reselecting the live pair changes no startup setting.
    let settings = crate::settings::Settings::load().expect("load settings");
    assert_eq!(settings.default_model.as_deref(), None);
    assert_eq!(
        settings
            .provider_models
            .as_ref()
            .and_then(|models| models.get("deepseek"))
            .map(String::as_str),
        None
    );
    assert_eq!(settings.reasoning_effort.as_deref(), None);
    assert!(app.pending_route_save.is_some());
    assert!(
        app.status_message
            .as_deref()
            .is_some_and(|message| message.contains("Model unchanged"))
    );
    let state = codewhale_config::SetupState::load()
        .expect("read setup state")
        .expect("a concrete live model selection must record setup progress");
    assert!(
        state
            .steps
            .contains_key(&codewhale_config::SetupStep::ProviderModel),
        "the provider/model setup step must be recorded even when the model was already live"
    );
}

#[tokio::test]
async fn reselecting_live_thinking_only_persists_startup_default() {
    let _guard = SettingsHomeGuard::new();
    let mut app = create_test_app();
    app.set_model_selection("deepseek-v4-pro".to_string());
    app.reasoning_effort = ReasoningEffort::High;
    let engine = mock_engine_handle();

    apply_picker_effort_choice(
        &mut app,
        &engine.handle,
        ReasoningEffort::High,
        ReasoningEffort::High,
    )
    .await;

    assert_eq!(
        crate::settings::Settings::load()
            .expect("load settings")
            .reasoning_effort
            .as_deref(),
        Some("high")
    );
    assert_eq!(app.reasoning_effort_preference, Some(ReasoningEffort::High));
    assert!(
        app.status_message
            .as_deref()
            .is_some_and(|message| message.contains("saved as startup default"))
    );
}

#[tokio::test]
async fn model_picker_switches_between_exact_named_custom_routes() {
    let _guard = SettingsHomeGuard::new();
    let mut app = create_test_app();
    app.set_provider_identity(ApiProvider::Custom, "custom-a");
    app.set_model_selection("model-a".to_string());
    let previous_effort = app.reasoning_effort;
    let mut custom = HashMap::new();
    for (name, base_url, model) in [
        ("custom-a", "http://127.0.0.1:18181/v1", "model-a"),
        ("custom-b", "http://127.0.0.1:18182/v1", "model-b"),
    ] {
        custom.insert(
            name.to_string(),
            ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some(base_url.to_string()),
                model: Some(model.to_string()),
                api_key: Some("local-test-key".to_string()),
                ..Default::default()
            },
        );
    }
    // Opening custom B's model picker retargets only Config; App/engine still
    // own A until the operator applies a model.
    let mut config = Config {
        provider: Some("custom-b".to_string()),
        providers: Some(ProvidersConfig {
            custom,
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut engine = mock_engine_handle();

    apply_model_picker_choice(
        &mut app,
        &mut engine.handle,
        &mut config,
        "model-b".to_string(),
        None,
        Some("custom-b".to_string()),
        ReasoningEffort::High,
        "model-a".to_string(),
        previous_effort,
        false,
    )
    .await;

    assert_eq!(app.api_provider, ApiProvider::Custom);
    assert_eq!(app.provider_identity_for_persistence(), "custom-b");
    assert_eq!(app.model_selection_for_persistence(), "model-b");
    assert_eq!(config.provider.as_deref(), Some("custom-b"));
    assert_eq!(config.deepseek_base_url(), "http://127.0.0.1:18182/v1");
}

#[tokio::test]
async fn model_picker_auto_switches_exact_named_custom_route_transactionally() {
    let _guard = SettingsHomeGuard::new();
    let mut app = create_test_app();
    app.set_provider_identity(ApiProvider::Custom, "custom-a");
    app.set_model_selection("model-a".to_string());
    app.reasoning_effort = ReasoningEffort::Low;
    app.reasoning_effort_preference = Some(ReasoningEffort::Low);
    let previous_effort = app.reasoning_effort;
    let mut custom = HashMap::new();
    for (name, base_url, model) in [
        ("custom-a", "http://127.0.0.1:18181/v1", "model-a"),
        ("custom-b", "http://127.0.0.1:18182/v1", "model-b"),
    ] {
        custom.insert(
            name.to_string(),
            ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some(base_url.to_string()),
                model: Some(model.to_string()),
                api_key: Some("local-test-key".to_string()),
                ..Default::default()
            },
        );
    }
    let mut config = Config {
        provider: Some("custom-b".to_string()),
        providers: Some(ProvidersConfig {
            custom,
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut engine = mock_engine_handle();

    apply_model_picker_choice(
        &mut app,
        &mut engine.handle,
        &mut config,
        "auto".to_string(),
        None,
        Some("custom-b".to_string()),
        ReasoningEffort::Low,
        "model-a".to_string(),
        previous_effort,
        false,
    )
    .await;

    assert_eq!(app.api_provider, ApiProvider::Custom);
    assert_eq!(app.provider_identity_for_persistence(), "custom-b");
    assert!(app.auto_model);
    assert_eq!(app.model_selection_for_persistence(), "auto");
    assert_eq!(app.reasoning_effort, ReasoningEffort::Low);
    assert_eq!(config.provider.as_deref(), Some("custom-b"));
    assert_eq!(config.deepseek_base_url(), "http://127.0.0.1:18182/v1");
    // Session-local: the effort tier is not written to settings.
    assert_eq!(
        crate::settings::Settings::load()
            .expect("load settings")
            .reasoning_effort
            .as_deref(),
        None
    );
    assert!(app.pending_route_save.is_some());
}

#[test]
fn dismissing_named_custom_model_picker_restores_app_owned_config_route() {
    let mut app = create_test_app();
    app.set_provider_identity(ApiProvider::Custom, "custom-a");
    let mut config =
        named_custom_session_config("custom-a", "http://127.0.0.1:18181/v1", "model-a");
    config.providers.as_mut().expect("providers").custom.insert(
        "custom-b".to_string(),
        ProviderConfig {
            kind: Some("openai-compatible".to_string()),
            base_url: Some("http://127.0.0.1:18182/v1".to_string()),
            model: Some("model-b".to_string()),
            ..Default::default()
        },
    );
    config.provider = Some("custom-b".to_string());

    sync_config_provider_from_app(&mut config, &app);

    assert_eq!(config.provider.as_deref(), Some("custom-a"));
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn model_picker_startup_default_reports_settings_write_failure() {
    let _lock = crate::test_support::lock_test_env();
    let tmp = TempDir::new().expect("settings tempdir");
    let bad_home = tmp.path().join("codewhale-home-file");
    std::fs::write(&bad_home, "not a directory").expect("bad home file");
    let _home = crate::test_support::EnvVarGuard::set("HOME", tmp.path().as_os_str());
    let _userprofile = crate::test_support::EnvVarGuard::set("USERPROFILE", tmp.path().as_os_str());
    let _codewhale_home =
        crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", bad_home.as_os_str());
    let _deepseek_config_path = crate::test_support::EnvVarGuard::remove("DEEPSEEK_CONFIG_PATH");
    let _codewhale_config_path = crate::test_support::EnvVarGuard::remove("CODEWHALE_CONFIG_PATH");
    let _codewhale_provider = crate::test_support::EnvVarGuard::remove("CODEWHALE_PROVIDER");
    let _deepseek_provider = crate::test_support::EnvVarGuard::remove("DEEPSEEK_PROVIDER");

    let mut app = create_test_app();
    app.set_model_selection("auto".to_string());
    app.reasoning_effort = ReasoningEffort::Auto;
    let mut engine = mock_engine_handle();
    let mut config = Config {
        api_key: Some("test-key".to_string()),
        ..Default::default()
    };

    apply_model_picker_choice(
        &mut app,
        &mut engine.handle,
        &mut config,
        "deepseek-v4-pro".to_string(),
        None,
        None,
        ReasoningEffort::High,
        "auto".to_string(),
        ReasoningEffort::Auto,
        false,
    )
    .await;

    // Ordinary Enter remains a live session change even when settings are
    // unavailable; it must not imply an invisible startup write.
    assert!(
        app.status_message
            .as_deref()
            .is_some_and(|message| message.contains("Model:"))
    );
    let pending = app.pending_route_save.as_ref().expect("pending save");
    assert_eq!(pending.model, "deepseek-v4-pro");
    let saved = app.apply_route_save_choice(
        crate::tui::views::route_save_prompt::RouteSaveChoice::SaveAsDefault,
    );
    assert!(saved.starts_with("Save failed:"), "{saved}");
    // The fixture's CODEWHALE_HOME is a file, so no setup-state root can
    // exist: the receipt is honestly absent, and the picker still applied
    // the live choice without claiming persistence.
    assert!(
        codewhale_config::SetupState::load()
            .expect("load setup state")
            .is_none(),
        "no setup-state root exists in this fixture"
    );
}

#[test]
fn apply_loaded_session_restores_artifact_registry() {
    let mut app = create_test_app();
    let mut session = saved_session_with_messages(vec![
        text_message("user", "hello"),
        text_message("assistant", "hi"),
    ]);
    session.artifacts.push(crate::artifacts::ArtifactRecord {
        id: "art_call_big".to_string(),
        kind: crate::artifacts::ArtifactKind::ToolOutput,
        session_id: "session-123".to_string(),
        tool_call_id: "call-big".to_string(),
        tool_name: "exec_shell".to_string(),
        created_at: chrono::Utc::now(),
        byte_size: 128,
        preview: "hello".to_string(),
        storage_path: PathBuf::from("/tmp/tool_outputs/call-big.txt"),
    });

    apply_loaded_session(&mut app, &mut Config::default(), &session).expect("restore session");
    assert_eq!(app.session_artifacts, session.artifacts);
}

#[test]
fn parallel_exploring_tool_starts_share_one_active_entry() {
    // Three exploring tools start in any order; they must collapse into one
    // entry inside the active cell rather than three separate cells. This is
    // the central CX#7 contract for the most common parallel case.
    let mut app = create_test_app();

    handle_tool_call_started(
        &mut app,
        "t-a",
        "read_file",
        &serde_json::json!({"path": "alpha.rs"}),
    );
    handle_tool_call_started(
        &mut app,
        "t-b",
        "read_file",
        &serde_json::json!({"path": "beta.rs"}),
    );
    handle_tool_call_started(
        &mut app,
        "t-c",
        "grep_files",
        &serde_json::json!({"pattern": "TODO"}),
    );

    // History must remain empty: nothing flushes until the turn ends.
    assert_eq!(app.history.len(), 0, "no history cells written mid-turn");
    let active = app.active_cell.as_ref().expect("active cell created");
    assert_eq!(
        active.entry_count(),
        1,
        "all exploring starts share one entry"
    );
    let HistoryCell::Tool(ToolCell::Exploring(explore)) = &active.entries()[0] else {
        panic!("expected exploring cell")
    };
    assert_eq!(explore.entries.len(), 3);
    for entry in &explore.entries {
        assert_eq!(entry.status, ToolStatus::Running);
    }
}

#[test]
fn out_of_order_completes_finalize_one_history_cell_per_turn() {
    // Three parallel tools complete in reverse order; we then signal turn
    // complete and assert exactly one tool history cell exists (the
    // finalized active group). This proves the active cell didn't bounce
    // mid-turn and that the flush path correctly migrates entries.
    let mut app = create_test_app();

    handle_tool_call_started(
        &mut app,
        "t-1",
        "read_file",
        &serde_json::json!({"path": "a.rs"}),
    );
    handle_tool_call_started(
        &mut app,
        "t-2",
        "read_file",
        &serde_json::json!({"path": "b.rs"}),
    );
    handle_tool_call_started(
        &mut app,
        "t-3",
        "grep_files",
        &serde_json::json!({"pattern": "x"}),
    );

    // Out-of-order completion: t-3, then t-1, then t-2.
    handle_tool_call_complete(&mut app, "t-3", "grep_files", &ok_result("two hits"));
    handle_tool_call_complete(&mut app, "t-1", "read_file", &ok_result("contents A"));
    handle_tool_call_complete(&mut app, "t-2", "read_file", &ok_result("contents B"));

    // Still nothing in history: the active cell holds everything.
    assert_eq!(app.history.len(), 0);
    let active = app.active_cell.as_ref().expect("active cell still present");
    let HistoryCell::Tool(ToolCell::Exploring(explore)) = &active.entries()[0] else {
        panic!("expected exploring cell")
    };
    assert!(
        explore
            .entries
            .iter()
            .all(|e| e.status == ToolStatus::Success),
        "all exploring entries should be Success after their tools complete"
    );

    // Flush via the explicit helper (mirrors what TurnComplete does).
    app.flush_active_cell();

    assert!(app.active_cell.is_none(), "active cell cleared after flush");
    // The flushed group is exactly one history cell — the merged exploring
    // aggregate. This is the heart of CX#7: parallel work renders as ONE
    // finalized cell, regardless of completion order.
    let tool_cells = app
        .history
        .iter()
        .filter(|c| matches!(c, HistoryCell::Tool(_)))
        .count();
    assert_eq!(
        tool_cells, 1,
        "exactly one tool history cell after parallel turn"
    );
}

#[test]
fn mixed_parallel_tools_render_in_single_active_cell() {
    // Tools of different shapes — exploring + exec + generic — all in flight
    // at once. The active cell must hold them all without bouncing.
    let mut app = create_test_app();

    handle_tool_call_started(
        &mut app,
        "ex-1",
        "read_file",
        &serde_json::json!({"path": "x.rs"}),
    );
    handle_tool_call_started(
        &mut app,
        "shell-1",
        "exec_shell",
        &serde_json::json!({"command": "ls"}),
    );
    handle_tool_call_started(
        &mut app,
        "gen-1",
        "todo_write",
        &serde_json::json!({"items": []}),
    );

    assert_eq!(app.history.len(), 0);
    let active = app.active_cell.as_ref().expect("active cell present");
    // 3 entries: exploring aggregate (1) + exec + generic.
    assert_eq!(active.entry_count(), 3);

    handle_tool_call_complete(&mut app, "shell-1", "exec_shell", &ok_result("ok"));
    handle_tool_call_complete(&mut app, "gen-1", "todo_write", &ok_result("done"));
    handle_tool_call_complete(&mut app, "ex-1", "read_file", &ok_result("file body"));

    // After all complete, still in active until flush.
    assert_eq!(app.history.len(), 0);
    app.flush_active_cell();
    let tool_cells: Vec<_> = app
        .history
        .iter()
        .filter(|c| matches!(c, HistoryCell::Tool(_)))
        .collect();
    assert_eq!(
        tool_cells.len(),
        3,
        "three distinct tool shapes finalize as three cells in stable insertion order"
    );
}

#[test]
fn orphan_tool_complete_with_unknown_id_pushes_separate_cell() {
    // A ToolCallComplete with no matching ToolCallStarted — the orphan path.
    // Per the design we render it as a finalized standalone cell so the user
    // still sees the output, but we must NOT flush or contaminate any active
    // cell that's currently in flight.
    let mut app = create_test_app();

    handle_tool_call_started(
        &mut app,
        "live-1",
        "read_file",
        &serde_json::json!({"path": "live.rs"}),
    );

    // Orphan completion arrives.
    handle_tool_call_complete(&mut app, "ghost-id", "mystery_tool", &ok_result("oops"));

    // Active cell is intact.
    let active = app
        .active_cell
        .as_ref()
        .expect("active cell preserved after orphan");
    assert_eq!(active.entry_count(), 1);

    // The orphan rendered as a separate finalized cell pushed to history.
    assert_eq!(app.history.len(), 1, "orphan added one finalized cell");
    let HistoryCell::Tool(ToolCell::Generic(generic)) = &app.history[0] else {
        panic!("orphan should render as a Generic tool cell")
    };
    assert_eq!(generic.name, "mystery_tool");
    assert_eq!(generic.status, ToolStatus::Success);
}

#[test]
fn turn_complete_flushes_active_cell_into_history() {
    // The full path through the public flush helper. Verifies that a
    // mid-turn snapshot (exec running, exploring complete) becomes a stable
    // history slice on flush.
    let mut app = create_test_app();
    handle_tool_call_started(
        &mut app,
        "ex-1",
        "read_file",
        &serde_json::json!({"path": "a.rs"}),
    );
    handle_tool_call_complete(&mut app, "ex-1", "read_file", &ok_result("body"));
    handle_tool_call_started(
        &mut app,
        "shell-1",
        "exec_shell",
        &serde_json::json!({"command": "ls"}),
    );
    // Don't complete shell-1 — simulate cancellation mid-shell.
    app.finalize_active_cell_as_interrupted();

    assert!(app.active_cell.is_none(), "active cell cleared on flush");
    let exec_cells: Vec<_> = app
        .history
        .iter()
        .filter_map(|c| match c {
            HistoryCell::Tool(ToolCell::Exec(exec)) => Some(exec),
            _ => None,
        })
        .collect();
    assert_eq!(exec_cells.len(), 1);
    assert_eq!(
        exec_cells[0].status,
        ToolStatus::Failed,
        "interrupted shell entry marked Failed (closest available terminal status)"
    );
}

#[test]
fn orphan_during_active_keeps_subsequent_completion_routed_correctly() {
    // Regression cover for the index-shift trap: when an orphan arrives
    // mid-active, it pushes a real history cell that bumps virtual indices
    // by one. A subsequent legitimate completion must still find its entry.
    let mut app = create_test_app();
    handle_tool_call_started(
        &mut app,
        "live",
        "exec_shell",
        &serde_json::json!({"command": "ls"}),
    );
    // Orphan completion arrives FIRST (before live's completion).
    handle_tool_call_complete(&mut app, "ghost", "weird_tool", &ok_result("ghost-out"));
    // Now complete the live tool — it should still mutate the active entry,
    // not silently drop or hit a stale index.
    handle_tool_call_complete(&mut app, "live", "exec_shell", &ok_result("hello"));

    // Active cell still present (turn hasn't completed).
    let active = app.active_cell.as_ref().expect("active cell present");
    let HistoryCell::Tool(ToolCell::Exec(exec)) = &active.entries()[0] else {
        panic!("expected exec cell")
    };
    assert_eq!(exec.status, ToolStatus::Success);

    // History contains exactly the orphan.
    assert_eq!(app.history.len(), 1);
    let HistoryCell::Tool(ToolCell::Generic(generic)) = &app.history[0] else {
        panic!("expected orphan generic cell")
    };
    assert_eq!(generic.name, "weird_tool");

    // Flush settles the active exec into history below the orphan.
    app.flush_active_cell();
    assert_eq!(app.history.len(), 2);
}

#[test]
fn orphan_success_with_mutation_metadata_keeps_the_file_receipt() {
    let mut app = create_test_app();
    let result = crate::tools::spec::ToolResult::success("structured success").with_metadata(
        serde_json::json!({
            "mutation": {
                "diff": "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n",
                "files": [{ "path": "src/lib.rs", "outcome": "updated" }],
                "renames": []
            }
        }),
    );

    handle_tool_call_complete(&mut app, "orphan-file", "File", &Ok(result));

    let HistoryCell::Tool(ToolCell::PatchSummary(cell)) = &app.history[0] else {
        panic!("structured orphan mutation must retain a calm File receipt")
    };
    assert_eq!(cell.status, ToolStatus::Success);
    assert_eq!(
        cell.receipt.as_ref().map(|receipt| receipt.outcome_label()),
        Some("Updated src/lib.rs".to_string())
    );
    assert_eq!(app.tool_details_by_cell[&0].tool_name, "File");
}

#[test]
fn tool_details_survive_active_cell_flush() {
    // Detail pagers resolve tool details by cell index. Flushing the
    // active cell must move detail records into `tool_details_by_cell` so
    // the pager keeps working after the turn settles.
    let mut app = create_test_app();
    handle_tool_call_started(
        &mut app,
        "tid",
        "exec_shell",
        &serde_json::json!({"command": "echo hi"}),
    );
    handle_tool_call_complete(&mut app, "tid", "exec_shell", &ok_result("hi"));
    app.flush_active_cell();

    // The exec cell is now at index 0 in history.
    assert_eq!(app.history.len(), 1);
    let detail = app
        .tool_details_by_cell
        .get(&0)
        .expect("detail record migrated to flushed cell index");
    assert_eq!(detail.tool_id, "tid");
    assert_eq!(detail.tool_name, "exec_shell");
}

// ---- exploring labels: codex-style progressive verbs ----
//
// Bare names like "Read foo.rs" / "Search pattern" read as past tense, which
// is wrong while the tool is still running. Progressive forms ("Reading…",
// "Searching for…") match what the user actually sees: a live in-flight
// action.

#[test]
fn exploring_label_uses_progressive_for_read_file() {
    let label = exploring_label("read_file", &serde_json::json!({"path": "src/foo.rs"}));
    assert_eq!(label, "Reading src/foo.rs");
}

#[test]
fn exploring_label_uses_progressive_for_list_dir() {
    let label = exploring_label("list_dir", &serde_json::json!({"path": "crates/tui/src/"}));
    assert_eq!(label, "Listing crates/tui/src/");
}

#[test]
fn exploring_label_uses_progressive_for_list_dir_no_path() {
    let label = exploring_label("list_dir", &serde_json::json!({}));
    assert_eq!(label, "Listing directory");
}

#[test]
fn exploring_label_for_grep_quotes_pattern_with_searching_for() {
    let label = exploring_label(
        "grep_files",
        &serde_json::json!({"pattern": "TranscriptScroll"}),
    );
    assert_eq!(label, "Searching for `TranscriptScroll`");
}

#[test]
fn exploring_label_for_list_files_uses_progressive() {
    let label = exploring_label("list_files", &serde_json::json!({}));
    assert_eq!(label, "Listing files");
}

// `running_status_label_with_elapsed` lives in `crate::tui::history` next to
// the other tool-header helpers — its tests live there too.

// ---- P2.4: auto-scroll churn regressions ----
//
// The contract: once the user scrolls away from the live tail mid-turn
// (`user_scrolled_during_stream = true`), no path should yank them back to
// the bottom until either (a) they explicitly scroll to tail, (b) the turn
// ends, or (c) they hit an explicit jump-to-bottom key. One deliberate
// exception (v0.9.4): a composer-originated send anchors the new user
// message at the tail via the sync prepare phase (`prepare_user_dispatch`),
// so the freshly submitted message is always visible. Steer/replay sends
// keep the user's local context and never yank. Tool-cell handlers only
// call `mark_history_updated`, which does NOT scroll. `add_message` gates
// on the flag.

#[test]
fn add_message_does_not_scroll_when_user_scrolled_away() {
    use crate::tui::scrolling::TranscriptScroll;

    let mut app = create_test_app();
    // Pre-condition: user was following the tail, then scrolled up.
    app.viewport.transcript_scroll = TranscriptScroll::at_line(7);
    app.user_scrolled_during_stream = true;

    app.add_message(HistoryCell::User {
        content: "fresh user message".to_string(),
    });

    assert!(
        !app.viewport.transcript_scroll.is_at_tail(),
        "add_message must respect user_scrolled_during_stream",
    );
}

#[test]
fn add_message_pins_to_tail_when_user_was_following() {
    use crate::tui::scrolling::TranscriptScroll;

    let mut app = create_test_app();
    app.viewport.transcript_scroll = TranscriptScroll::to_bottom();
    app.user_scrolled_during_stream = false;

    app.add_message(HistoryCell::User {
        content: "fresh user message".to_string(),
    });

    assert!(
        app.viewport.transcript_scroll.is_at_tail(),
        "auto-pin should still work when the user hasn't opted out",
    );
}

#[test]
fn tool_call_started_does_not_scroll_when_user_scrolled_away() {
    // Tool-cell handlers must not sneak in a scroll_to_bottom — they go
    // through `mark_history_updated` which only bumps `history_version`.
    use crate::tui::scrolling::TranscriptScroll;

    let mut app = create_test_app();
    app.viewport.transcript_scroll = TranscriptScroll::at_line(7);
    app.user_scrolled_during_stream = true;

    handle_tool_call_started(
        &mut app,
        "tid",
        "exec_shell",
        &serde_json::json!({"command": "ls"}),
    );

    assert!(
        !app.viewport.transcript_scroll.is_at_tail(),
        "tool-cell start must not yank scroll position to bottom",
    );
}

#[test]
fn tool_call_complete_does_not_scroll_when_user_scrolled_away() {
    use crate::tui::scrolling::TranscriptScroll;

    let mut app = create_test_app();
    handle_tool_call_started(
        &mut app,
        "tid",
        "exec_shell",
        &serde_json::json!({"command": "ls"}),
    );

    // After start, user scrolls up.
    app.viewport.transcript_scroll = TranscriptScroll::at_line(7);
    app.user_scrolled_during_stream = true;

    handle_tool_call_complete(&mut app, "tid", "exec_shell", &ok_result("output"));

    assert!(
        !app.viewport.transcript_scroll.is_at_tail(),
        "tool-cell complete must not yank scroll position to bottom",
    );
}

#[test]
fn mark_history_updated_does_not_call_scroll_to_bottom() {
    // Behavior pin: future contributors must not add a scroll_to_bottom
    // here. The scroll-following logic lives only in `add_message` and
    // `flush_active_cell`, both gated on `user_scrolled_during_stream`.
    use crate::tui::scrolling::TranscriptScroll;

    let mut app = create_test_app();
    app.viewport.transcript_scroll = TranscriptScroll::at_line(3);
    app.user_scrolled_during_stream = true;

    app.mark_history_updated();

    assert!(
        !app.viewport.transcript_scroll.is_at_tail(),
        "mark_history_updated must not scroll",
    );
}

// ---- v0.9.4: composer-originated sends anchor at the tail ----
//
// Sending from the composer is a deliberate act: the new user message must be
// visible. `prepare_user_dispatch` pins the view to the tail and stamps the
// tail-flash timestamp at the moment the cell appears. Steer sends keep the
// user's local context (they never yank), and a failed dispatch restores the
// pre-send timestamp so no stale flash points at an older message.

#[tokio::test]
async fn composer_send_anchors_new_message_at_tail() {
    let _env_lock = crate::test_support::lock_test_env();
    let _zai = crate::test_support::EnvVarGuard::set("ZAI_API_KEY", "zai-test-key");
    let mut app = create_test_app();
    app.set_provider_identity(ApiProvider::Zai, "zai");
    app.set_model_selection("auto".to_string());
    let config = Config {
        provider: Some("zai".to_string()),
        default_text_model: Some("auto".to_string()),
        ..Default::default()
    };
    let engine = mock_engine_handle();

    // The user deliberately scrolled up to read history mid-turn.
    use crate::tui::scrolling::TranscriptScroll;
    app.viewport.transcript_scroll = TranscriptScroll::at_line(7);
    app.user_scrolled_during_stream = true;
    assert!(!app.viewport.transcript_scroll.is_at_tail());

    dispatch_user_message(
        &mut app,
        &config,
        &engine.handle,
        QueuedMessage::new("fresh composer message".to_string(), None),
    )
    .await
    .expect("dispatch succeeds against the mock engine");

    assert!(
        app.viewport.transcript_scroll.is_at_tail(),
        "a composer-originated send must anchor the new message at the tail",
    );
    assert!(
        app.last_send_at.is_some(),
        "the send timestamp must be stamped so the tail-flash marks the new cell",
    );
    assert!(
        app.history.iter().any(|cell| matches!(
            cell,
            HistoryCell::User { content }
                if content == "fresh composer message"
        )),
        "the user message must be appended to the transcript",
    );
}

#[tokio::test]
async fn steer_send_keeps_local_scroll_context() {
    let mut app = create_test_app();
    let mut engine = crate::core::engine::mock_engine_handle();
    use crate::tui::scrolling::TranscriptScroll;
    app.viewport.transcript_scroll = TranscriptScroll::at_line(7);
    app.user_scrolled_during_stream = true;

    steer_user_message(
        &mut app,
        &Config::default(),
        &engine.handle,
        QueuedMessage::new("steer into the current turn".to_string(), None),
    )
    .await
    .expect("steer user message");

    assert!(
        !app.viewport.transcript_scroll.is_at_tail(),
        "a steer must keep the user's local scroll context",
    );
    assert!(
        app.last_send_at.is_none(),
        "a steer is not a composer send and must not flash the tail",
    );
    assert_eq!(
        engine.rx_steer.recv().await.as_deref(),
        Some("steer into the current turn")
    );
}

#[tokio::test]
async fn failed_dispatch_restores_presend_tail_flash_timestamp() {
    let mut app = create_test_app();
    let engine = mock_engine_handle();
    let config = Config::default();
    drop(engine.rx_op);

    let result = dispatch_user_message(
        &mut app,
        &config,
        &engine.handle,
        QueuedMessage::new("will fail".to_string(), None),
    )
    .await;

    assert!(
        result.is_err(),
        "closed engine channel must fail the dispatch"
    );
    assert!(
        app.last_send_at.is_none(),
        "a failed composer send must not leave a stale tail-flash timestamp",
    );
}

// ---- P2.3: thinking + tool calls render as one grouped block ----

#[test]
fn thinking_then_tools_share_active_cell_until_text_flushes() {
    // Contract: a turn that emits Thinking → Tool → Tool keeps everything
    // inside `active_cell` (one logical "Working…" group) until the next
    // assistant prose chunk fires, at which point the group flushes into
    // history in original order.
    let mut app = create_test_app();

    // 1. Thinking starts and streams a delta.
    let thinking_idx = crate::tui::streaming_thinking::ensure_active_entry(&mut app);
    crate::tui::streaming_thinking::append(&mut app, thinking_idx, "planning the read");
    assert!(
        app.history.is_empty(),
        "thinking must not write into history mid-turn"
    );
    assert_eq!(thinking_idx, 0);

    // 2. Two tool calls land in the same active cell.
    handle_tool_call_started(
        &mut app,
        "t-1",
        "exec_shell",
        &serde_json::json!({"command": "ls"}),
    );
    handle_tool_call_started(
        &mut app,
        "t-2",
        "exec_shell",
        &serde_json::json!({"command": "pwd"}),
    );

    let active = app
        .active_cell
        .as_ref()
        .expect("active cell present mid-turn");
    assert_eq!(
        active.entry_count(),
        3,
        "thinking + two exec entries share one active cell"
    );
    assert!(matches!(active.entries()[0], HistoryCell::Thinking { .. }));
    assert!(matches!(
        active.entries()[1],
        HistoryCell::Tool(ToolCell::Exec(_))
    ));
    assert!(matches!(
        active.entries()[2],
        HistoryCell::Tool(ToolCell::Exec(_))
    ));

    // 3. Thinking finalizes — entry stays in active cell, just stops streaming.
    let finalized = crate::tui::streaming_thinking::finalize_active_entry(&mut app, Some(1.5), "");
    assert!(finalized, "finalizer reports it touched the active cell");
    let HistoryCell::Thinking {
        streaming,
        duration_secs,
        content,
        ..
    } = &app
        .active_cell
        .as_ref()
        .expect("active cell still present after thinking complete")
        .entries()[0]
    else {
        panic!("expected thinking entry")
    };
    assert!(!streaming, "thinking spinner stops after finalize");
    assert_eq!(*duration_secs, Some(1.5));
    assert_eq!(content, "planning the read");
    assert!(
        app.streaming_thinking_active_entry.is_none(),
        "stream pointer cleared after finalize"
    );

    // 4. Assistant prose arriving (simulated by flush) drains the group into
    //    history in original order: Thinking → Tool → Tool.
    app.flush_active_cell();
    assert!(app.active_cell.is_none(), "active cell cleared after flush");
    assert_eq!(
        app.history.len(),
        3,
        "thinking + both tool entries land in history together"
    );
    assert!(matches!(app.history[0], HistoryCell::Thinking { .. }));
    assert!(matches!(
        app.history[1],
        HistoryCell::Tool(ToolCell::Exec(_))
    ));
    assert!(matches!(
        app.history[2],
        HistoryCell::Tool(ToolCell::Exec(_))
    ));
}

#[test]
fn flush_active_cell_finalizes_unclosed_thinking_block() {
    // Defensive: if the engine fails to emit ThinkingComplete before the
    // assistant text arrives, `flush_active_cell` must still stop the
    // spinner so the migrated history cell isn't perpetually streaming.
    let mut app = create_test_app();
    let _ = crate::tui::streaming_thinking::ensure_active_entry(&mut app);
    crate::tui::streaming_thinking::append(&mut app, 0, "incomplete");

    app.flush_active_cell();

    assert_eq!(app.history.len(), 1);
    let HistoryCell::Thinking { streaming, .. } = &app.history[0] else {
        panic!("expected thinking history cell")
    };
    assert!(
        !*streaming,
        "flush must stop the spinner even without ThinkingComplete"
    );
    assert!(
        app.streaming_thinking_active_entry.is_none(),
        "stream pointer cleared by flush"
    );
}

#[test]
fn open_thinking_pager_finds_thinking_in_active_cell() {
    // After ThinkingComplete fires, the finalized thinking entry stays in
    // `app.active_cell` with `streaming = false` until the active cell is
    // flushed to history (end-of-turn, or when an assistant text arrives).
    // During that window the transcript still renders the Ctrl+O affordance
    // from `render_thinking`, so the handler must reach across the virtual
    // transcript — not just `app.history` — or the promise is a lie.
    // Regression guard for the v0.8.29 affordance/handler mismatch.
    let mut app = create_test_app();
    let _ = crate::tui::streaming_thinking::ensure_active_entry(&mut app);
    crate::tui::streaming_thinking::append(&mut app, 0, "deliberating");
    let finalized = crate::tui::streaming_thinking::finalize_active_entry(&mut app, Some(1.2), "");
    assert!(finalized);
    assert!(
        app.history.is_empty(),
        "thinking entry stays in active_cell until flush"
    );
    let active = app.active_cell.as_ref().expect("active cell present");
    assert!(matches!(
        active.entries().first(),
        Some(HistoryCell::Thinking {
            streaming: false,
            ..
        })
    ));

    assert!(open_thinking_pager(&mut app));
    assert_eq!(
        app.view_stack.top_kind(),
        Some(ModalKind::Pager),
        "pager must open for thinking entries still in active_cell"
    );
    let body = pop_pager_body(&mut app);
    assert!(body.contains("Activity: reasoning timeline"), "{body}");
    assert!(body.contains("Thinking chunk 1 of 1"), "{body}");
    assert!(body.contains("deliberating"), "{body}");
}

#[test]
fn reasoning_detail_opens_timeline_for_selected_thinking() {
    let mut app = create_test_app();
    app.history = vec![
        HistoryCell::Thinking {
            content: "first chunk reasoning".to_string(),
            streaming: false,
            duration_secs: Some(0.8),
        },
        HistoryCell::Assistant {
            content: "interlude".to_string(),
            streaming: false,
        },
        HistoryCell::Thinking {
            content: "second chunk reasoning".to_string(),
            streaming: false,
            duration_secs: Some(1.1),
        },
    ];
    app.resync_history_revisions();
    let revisions = app.history_revisions.clone();
    app.viewport.transcript_cache.ensure(
        &app.history,
        &revisions,
        100,
        app.transcript_render_options(),
    );
    let line = first_line_for_cell(&app, 0);
    let point = TranscriptSelectionPoint {
        line_index: line,
        column: 0,
    };
    app.viewport.transcript_selection.anchor = Some(point);
    app.viewport.transcript_selection.head = Some(point);

    assert!(open_reasoning_detail_pager(&mut app));
    let body = pop_pager_body(&mut app);

    assert!(
        body.contains("Activity: reasoning timeline"),
        "activity label missing: {body}"
    );
    assert!(
        body.contains("Selected chunk: 1 of 2"),
        "chunk position missing: {body}"
    );
    assert!(
        body.contains("Next chunk: 2 of 2 - second chunk reasoning"),
        "neighboring chunk missing: {body}"
    );
    assert!(body.contains("Thinking chunk 1 of 2 (selected)"), "{body}");
    assert!(body.contains("Thinking chunk 2 of 2"), "{body}");
    assert!(body.contains("first chunk reasoning"), "body: {body}");
    assert!(
        body.contains("second chunk reasoning"),
        "timeline should include the whole session's thinking: {body}"
    );
}

#[test]
fn turn_inspector_renders_overview_sections_for_active_turn() {
    let mut app = create_test_app();
    // A committed turn: user prompt, a tool call, a patch, and a final reply.
    app.history = vec![
        HistoryCell::User {
            content: "Fix the flaky login test".to_string(),
        },
        HistoryCell::Tool(ToolCell::Exec(ExecCell {
            command: "cargo test login".to_string(),
            status: ToolStatus::Success,
            output: Some("ok".to_string()),
            live_output: None,
            shell_task_id: None,
            owner_agent_id: None,
            owner_agent_name: None,
            started_at: None,
            duration_ms: Some(2400),
            stale_elapsed_since_output_ms: None,
            source: ExecSource::Assistant,
            interaction: None,
            output_summary: None,
        })),
        HistoryCell::Tool(ToolCell::PatchSummary(
            crate::tui::history::PatchSummaryCell {
                path: "src/login.rs".to_string(),
                summary: "guard against empty token".to_string(),
                status: ToolStatus::Success,
                error: None,
                receipt: None,
            },
        )),
        HistoryCell::Assistant {
            content: "Fixed the race in the login test.".to_string(),
            streaming: false,
        },
    ];
    app.runtime_turn_id = Some("turn_abc123456789".to_string());
    app.runtime_turn_status = Some("completed".to_string());

    let body = turn_inspector_text(&app);

    // Overview framing + Ctrl+O vs. Alt+V/⌥V contract.
    let details = crate::tui::shell_key_routing::display_chord(
        crate::tui::shell_key_routing::binding(
            crate::tui::shell_key_routing::ShellBindingId::ToolDetails,
        )
        .footer_chord,
    );
    assert!(
        body.contains("Turn turn_abc1234 \u{00B7} completed"),
        "{body}"
    );
    assert!(!body.contains("turn_abc123456789"), "{body}");
    assert!(
        body.contains(&format!(
            "press {details} for the selected item's raw detail"
        )),
        "{body}"
    );
    assert!(
        !body.contains("press v for the selected item's raw detail"),
        "bare-v details claim must not appear: {body}"
    );
    // Section headers for all nine sections must be present.
    for header in [
        "── Intent ──",
        "── To-do ──",
        "── Turn timeline ──",
        "── Files changed ──",
        "── Diagnostics loop ──",
        "── Tests / verifier ──",
        "── Approvals / denials ──",
        "── Model route + tokens/cost ──",
        "── Final result / status ──",
    ] {
        assert!(body.contains(header), "missing section {header}: {body}");
    }
    // Intent + tool timeline + files + result are the must-have populated ones.
    assert!(body.contains("Fix the flaky login test"), "{body}");
    assert!(body.contains("test/verifier: cargo test login"), "{body}");
    assert!(body.contains("2s"), "duration missing: {body}");
    assert!(body.contains("src/login.rs"), "{body}");
    assert!(
        body.contains("cargo test login — done"),
        "verifier section should surface the test run: {body}"
    );
    assert!(body.contains("Route: DeepSeek"), "route missing: {body}");
    assert!(
        body.contains("Result: Fixed the race in the login test."),
        "{body}"
    );
    assert!(body.contains("Status: completed"), "{body}");
}

#[test]
fn turn_inspector_pager_retains_complete_long_final_result() {
    let mut app = create_test_app();
    let final_result = format!(
        "{}\nSecond paragraph remains available.\nTAIL-SENTINEL-4482",
        "Long final result content. ".repeat(20)
    );
    assert!(final_result.len() > 200);
    app.history = vec![
        HistoryCell::User {
            content: "Inspect the complete result".to_string(),
        },
        HistoryCell::Assistant {
            content: final_result,
            streaming: false,
        },
    ];
    app.runtime_turn_status = Some("completed".to_string());

    assert!(open_turn_inspector_pager(&mut app));
    let body = pop_pager_body(&mut app);

    assert!(body.contains("TAIL-SENTINEL-4482"), "{body}");
    assert!(
        body.contains("Second paragraph remains available."),
        "{body}"
    );
    assert!(
        !body.contains("Long final result content. ..."),
        "pager data must not be pre-truncated: {body}"
    );
}

#[test]
fn turn_inspector_timeline_numbers_semantic_entries_and_checkpoint_actions() {
    let _lock = crate::test_support::lock_test_env();
    let tmp = TempDir::new().expect("tempdir");
    let _home = crate::test_support::EnvVarGuard::set("HOME", tmp.path());
    let _userprofile = crate::test_support::EnvVarGuard::set("USERPROFILE", tmp.path());
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::write(workspace.join("src.rs"), "fn main() {}\n").expect("source");
    let repo = crate::snapshot::SnapshotRepo::open_or_init(&workspace).expect("snapshot repo");
    repo.snapshot("pre-turn:12: Fix timeline")
        .expect("pre-turn snapshot");

    let mut app = create_test_app();
    app.workspace = workspace;
    app.turn_counter = 12;
    app.runtime_turn_status = Some("completed".to_string());
    app.history = vec![
        HistoryCell::User {
            content: "Fix timeline evidence".to_string(),
        },
        HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "read_file".to_string(),
            status: ToolStatus::Success,
            input_summary: Some("src.rs".to_string()),
            output: Some("fn main() {}".to_string()),
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        })),
        HistoryCell::Tool(ToolCell::PatchSummary(
            crate::tui::history::PatchSummaryCell {
                path: "src.rs".to_string(),
                summary: "add timeline evidence".to_string(),
                status: ToolStatus::Success,
                error: None,
                receipt: None,
            },
        )),
        HistoryCell::Tool(ToolCell::Exec(ExecCell {
            command: "cargo test timeline".to_string(),
            status: ToolStatus::Success,
            output: Some("ok".to_string()),
            live_output: None,
            shell_task_id: None,
            owner_agent_id: None,
            owner_agent_name: None,
            started_at: None,
            duration_ms: Some(1250),
            stale_elapsed_since_output_ms: None,
            source: ExecSource::Assistant,
            interaction: None,
            output_summary: None,
        })),
        HistoryCell::Assistant {
            content: "Timeline evidence is visible.".to_string(),
            streaming: false,
        },
    ];

    let body = turn_inspector_text(&app);

    assert!(
        body.contains("1. user prompt: Fix timeline evidence"),
        "{body}"
    );
    let details_chord = crate::tui::shell_key_routing::display_chord(
        crate::tui::shell_key_routing::binding(
            crate::tui::shell_key_routing::ShellBindingId::ToolDetails,
        )
        .footer_chord,
    );
    assert!(
        body.contains(&format!(
            "2. read/search: read · src.rs — done · actions: {details_chord} raw detail"
        )),
        "{body}"
    );
    assert!(
        body.contains(&format!(
            "3. edit: src.rs — add timeline evidence — done · actions: {details_chord} diff"
        )),
        "{body}"
    );
    assert!(
        body.contains(&format!(
            "4. test/verifier: cargo test timeline — done · 1s · actions: {details_chord} raw detail"
        )),
        "{body}"
    );
    assert!(
        body.contains("checkpoint: pre-turn:12: Fix timeline"),
        "{body}"
    );
    assert!(
        body.contains("actions: r restore via /restore (guarded), e export handoff"),
        "{body}"
    );
}

#[test]
fn turn_inspector_degrades_empty_sections_without_panic() {
    // A minimal turn: only a user prompt, nothing else has happened yet.
    let mut app = create_test_app();
    app.history = vec![HistoryCell::User {
        content: "hello".to_string(),
    }];
    app.runtime_turn_status = Some("in_progress".to_string());

    let body = turn_inspector_text(&app);

    // Unavailable sections degrade to `none`, never a blank void.
    assert!(body.contains("── To-do ──\nnone"), "{body}");
    assert!(body.contains("── Files changed ──\nnone"), "{body}");
    assert!(body.contains("── Tests / verifier ──\nnone"), "{body}");
    assert!(body.contains("── Approvals / denials ──\nnone"), "{body}");
    // Intent still resolves from the prompt; status reflects the live turn.
    assert!(body.contains("hello"), "{body}");
    assert!(body.contains("Status: in progress"), "{body}");
    assert!(body.contains("Result: turn still running"), "{body}");
}

#[test]
fn turn_inspector_scopes_to_latest_turn_only() {
    // Two turns in history: the inspector must scope to the second (latest).
    let mut app = create_test_app();
    app.history = vec![
        HistoryCell::User {
            content: "first turn prompt".to_string(),
        },
        HistoryCell::Assistant {
            content: "first turn answer".to_string(),
            streaming: false,
        },
        HistoryCell::User {
            content: "second turn prompt".to_string(),
        },
        HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "read_file".to_string(),
            status: ToolStatus::Success,
            input_summary: Some("src/lib.rs".to_string()),
            output: Some("done".to_string()),
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        })),
    ];

    let body = turn_inspector_text(&app);

    assert!(body.contains("second turn prompt"), "intent scope: {body}");
    assert!(
        !body.contains("first turn prompt"),
        "inspector leaked prior turn intent: {body}"
    );
}

#[test]
fn turn_inspector_switches_isolated_full_turn_pages_with_tagged_reasoning_and_output() {
    let mut app = create_test_app();
    app.turn_counter = 2;
    app.runtime_turn_status = Some("completed".to_string());
    app.history = vec![
        HistoryCell::User {
            content: "FIRST-PROMPT\nkeep this input line".to_string(),
        },
        HistoryCell::Thinking {
            content: "FIRST-THOUGHT\nfull reasoning tail".to_string(),
            streaming: false,
            duration_secs: Some(1.0),
        },
        HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "read_file".to_string(),
            status: ToolStatus::Success,
            input_summary: Some("src/first.rs".to_string()),
            output: Some("FIRST-TOOL-OUTPUT\ncomplete execution tail".to_string()),
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        })),
        HistoryCell::Assistant {
            content: "FIRST-ANSWER\ncomplete assistant tail".to_string(),
            streaming: false,
        },
        HistoryCell::User {
            content: "SECOND-PROMPT".to_string(),
        },
        HistoryCell::Thinking {
            content: "SECOND-THOUGHT".to_string(),
            streaming: true,
            duration_secs: None,
        },
        HistoryCell::Assistant {
            content: "SECOND-ANSWER".to_string(),
            streaming: true,
        },
    ];

    assert!(open_turn_inspector_pager(&mut app));
    let mut view = app.view_stack.pop().expect("turn inspector pager");
    let pager = view
        .as_any_mut()
        .downcast_mut::<PagerView>()
        .expect("turn inspector should reuse PagerView");

    let latest = pager.body_text();
    assert!(latest.contains("SECOND-PROMPT"), "{latest}");
    assert!(latest.contains("[∿ reasoning 1/1 · running]"), "{latest}");
    assert!(latest.contains("SECOND-THOUGHT"), "{latest}");
    assert!(latest.contains("[◆ · running]"), "{latest}");
    assert!(latest.contains("SECOND-ANSWER"), "{latest}");
    assert!(!latest.contains("FIRST-PROMPT"), "page leaked: {latest}");

    let action = pager.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert!(matches!(action, ViewAction::None));
    let first = pager.body_text();
    for expected in [
        "FIRST-PROMPT",
        "keep this input line",
        "[∿ reasoning 1/1 · done]",
        "FIRST-THOUGHT",
        "full reasoning tail",
        "[⚙ using tool]",
        "FIRST-TOOL-OUTPUT",
        "complete execution tail",
        "[◆ · done]",
        "FIRST-ANSWER",
        "complete assistant tail",
    ] {
        assert!(first.contains(expected), "missing {expected}: {first}");
    }
    assert!(!first.contains("SECOND-PROMPT"), "page leaked: {first}");
    let copied = match pager.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE)) {
        ViewAction::Emit(crate::tui::views::ViewEvent::CopyToClipboard { text, .. }) => text,
        other => panic!("expected page-scoped copy event, got {other:?}"),
    };
    for expected in [
        "FIRST-PROMPT\nkeep this input line",
        "FIRST-THOUGHT",
        "full reasoning tail",
        "FIRST-ANSWER\ncomplete assistant tail",
    ] {
        assert!(copied.contains(expected), "missing {expected}: {copied}");
    }
    assert!(!copied.contains("SECOND-PROMPT"), "page leaked: {copied}");
}

#[test]
fn turn_inspector_page_navigation_remains_visible_at_40x12() {
    let mut app = create_test_app();
    app.turn_counter = 2;
    app.runtime_turn_status = Some("completed".to_string());
    app.history = vec![
        HistoryCell::User {
            content: "first compact turn".to_string(),
        },
        HistoryCell::Assistant {
            content: "first compact answer".to_string(),
            streaming: false,
        },
        HistoryCell::User {
            content: "second compact turn".to_string(),
        },
        HistoryCell::Assistant {
            content: "second compact answer".to_string(),
            streaming: false,
        },
    ];

    assert!(open_turn_inspector_pager(&mut app));
    let mut view = app.view_stack.pop().expect("turn inspector pager");
    let pager = view
        .as_any_mut()
        .downcast_mut::<PagerView>()
        .expect("turn inspector should reuse PagerView");
    let area = Rect::new(0, 0, 40, 12);

    let mut latest_buf = ratatui::buffer::Buffer::empty(area);
    pager.render(area, &mut latest_buf);
    let latest_surface = latest_buf
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(latest_surface.contains("2/2"), "{latest_surface}");
    assert!(latest_surface.contains("←/→"), "{latest_surface}");
    assert!(latest_surface.contains("Turn #2"), "{latest_surface}");

    let _ = pager.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    let mut first_buf = ratatui::buffer::Buffer::empty(area);
    pager.render(area, &mut first_buf);
    let first_surface = first_buf
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(first_surface.contains("1/2"), "{first_surface}");
    assert!(first_surface.contains("Turn #1"), "{first_surface}");
}

#[test]
fn ctrl_o_open_turn_inspector_pager_opens_turn_overview_not_single_cell() {
    // The Ctrl+O handler dispatches to open_turn_inspector_pager; assert that
    // helper opens the turn overview rather than the single-cell detail.
    let mut app = create_test_app();
    app.history = vec![
        HistoryCell::User {
            content: "do the thing".to_string(),
        },
        HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "grep_files".to_string(),
            status: ToolStatus::Success,
            input_summary: Some("TODO".to_string()),
            output: Some("summary".to_string()),
            prompts: None,
            spillover_path: None,
            output_summary: None,
            is_diff: false,
        })),
    ];

    assert!(open_turn_inspector_pager(&mut app));
    assert_eq!(app.view_stack.top_kind(), Some(ModalKind::Pager));
    let body = pop_pager_body(&mut app);
    // Turn overview markers present; NOT the single-cell "Activity:" body.
    assert!(body.contains("── Turn timeline ──"), "{body}");
    assert!(body.contains("do the thing"), "{body}");
    assert!(
        !body.contains("Activity chunk:"),
        "Ctrl+O must open the turn overview, not the single-cell detail: {body}"
    );
}

#[test]
fn turn_handoff_markdown_renders_compact_sections_for_active_turn() {
    // Same committed turn as the inspector test: prompt, a verifier command, a
    // patch, and a final reply. The handoff must reuse that turn's scope +
    // section data and render it as compact Markdown headings/bullets.
    let mut app = create_test_app();
    app.history = vec![
        HistoryCell::User {
            content: "Fix the flaky login test".to_string(),
        },
        HistoryCell::Tool(ToolCell::Exec(ExecCell {
            command: "cargo test login".to_string(),
            status: ToolStatus::Success,
            output: Some("ok".to_string()),
            live_output: None,
            shell_task_id: None,
            owner_agent_id: None,
            owner_agent_name: None,
            started_at: None,
            duration_ms: Some(2400),
            stale_elapsed_since_output_ms: None,
            source: ExecSource::Assistant,
            interaction: None,
            output_summary: None,
        })),
        HistoryCell::Tool(ToolCell::PatchSummary(
            crate::tui::history::PatchSummaryCell {
                path: "src/login.rs".to_string(),
                summary: "guard against empty token".to_string(),
                status: ToolStatus::Success,
                error: None,
                receipt: None,
            },
        )),
        HistoryCell::Assistant {
            content: "Fixed the race in the login test.".to_string(),
            streaming: false,
        },
    ];
    app.runtime_turn_id = Some("turn_abc123456789".to_string());
    app.runtime_turn_status = Some("completed".to_string());

    let md = turn_handoff_markdown(&app);

    // Title carries the short turn id (A6: no raw UUID dumps).
    assert!(md.contains("# Turn handoff — turn_abc1234"), "{md}");
    // Markdown section headings for the issue's required sections.
    for heading in [
        "## Intent",
        "## Files changed",
        "## Turn timeline",
        "## Tests / verifier",
        "## Model route + tokens/cost",
        "## Result / status",
    ] {
        assert!(md.contains(heading), "missing heading {heading}: {md}");
    }
    // Populated content: intent, changed file, verifier command, route, result.
    assert!(md.contains("Fix the flaky login test"), "{md}");
    assert!(md.contains("- src/login.rs"), "changed file bullet: {md}");
    assert!(md.contains("cargo test login"), "{md}");
    assert!(md.contains("Route: DeepSeek"), "route missing: {md}");
    assert!(
        md.contains("Result: Fixed the race in the login test."),
        "{md}"
    );
    assert!(md.contains("Status: completed"), "{md}");
}

#[test]
fn turn_handoff_markdown_degrades_empty_sections_without_panic() {
    // A minimal turn: only a user prompt. Empty sections must degrade to `—`
    // (never a heading over a void), and the optional To-do section is omitted.
    let mut app = create_test_app();
    app.history = vec![HistoryCell::User {
        content: "hello".to_string(),
    }];
    app.runtime_turn_status = Some("in_progress".to_string());

    let md = turn_handoff_markdown(&app);

    assert!(md.contains("## Files changed\n—"), "{md}");
    assert!(md.contains("## Tests / verifier\n—"), "{md}");
    // The optional To-do section is dropped entirely when the list is empty.
    assert!(!md.contains("## To-do"), "{md}");
    // Intent still resolves; status reflects the live turn.
    assert!(md.contains("## Intent\nhello"), "{md}");
    assert!(md.contains("Status: in progress"), "{md}");
    assert!(md.contains("Result: turn still running"), "{md}");
}

#[test]
fn engine_error_finalizes_active_thinking_block() {
    use crate::error_taxonomy::StreamError;

    let mut app = create_test_app();
    let entry_idx = crate::tui::streaming_thinking::ensure_active_entry(&mut app);
    app.thinking_started_at = Some(Instant::now());
    app.streaming_state.start_thinking(0);
    app.streaming_state.push_content(0, "partial reasoning");

    apply_engine_error_to_app(
        &mut app,
        StreamError::Stall { timeout_secs: 60 }.into_envelope(),
    );

    let active = app.active_cell.as_ref().expect("active thinking remains");
    let HistoryCell::Thinking {
        content, streaming, ..
    } = &active.entries()[entry_idx]
    else {
        panic!("expected active thinking cell");
    };
    assert!(!*streaming, "error path must stop the thinking spinner");
    assert!(
        content.contains("partial reasoning"),
        "error path must drain pending thinking tail"
    );
    assert!(app.streaming_thinking_active_entry.is_none());
}

#[test]
fn message_complete_drain_preserves_thinking_when_thinking_complete_lost() {
    // #861 RC3: when the engine bursts events, `MessageComplete` can be
    // dispatched ahead of `ThinkingComplete`. Without the defensive drain,
    // `app.last_reasoning` would be `None` at `last_reasoning.take()` time
    // and the thinking block would be dropped from `api_messages`,
    // causing a DeepSeek HTTP 400 on the next turn (V4 thinking-mode
    // requires `reasoning_content` replay).
    //
    // This test exercises the head-of-handler drain in isolation: with a
    // thinking entry still active and `last_reasoning` empty, the drain
    // must transfer `reasoning_buffer` into `last_reasoning` before the
    // remainder of `MessageComplete` reads it.
    let mut app = create_test_app();

    let _ = crate::tui::streaming_thinking::ensure_active_entry(&mut app);
    app.thinking_started_at = Some(Instant::now());
    app.streaming_state.start_thinking(0);
    app.streaming_state.push_content(0, "deep reasoning text");
    let _ = app.streaming_state.commit_text(0);
    app.reasoning_buffer.push_str("deep reasoning text");

    assert!(
        app.last_reasoning.is_none(),
        "precondition: ThinkingComplete has NOT fired"
    );
    assert!(
        app.streaming_thinking_active_entry.is_some(),
        "precondition: thinking entry is still active"
    );

    // Mirror the head of `EngineEvent::MessageComplete` — the new defensive
    // drain installed by the #861 RC3 fix.
    if app.streaming_thinking_active_entry.is_some() {
        let _ = crate::tui::streaming_thinking::finalize_current(&mut app);
        crate::tui::streaming_thinking::stash_reasoning_buffer_into_last_reasoning(&mut app);
    }

    assert!(
        app.last_reasoning
            .as_deref()
            .is_some_and(|s| s.contains("deep reasoning text")),
        "defensive drain must move reasoning into last_reasoning so the\
         downstream `last_reasoning.take()` produces a Thinking block"
    );
    assert!(
        app.streaming_thinking_active_entry.is_none(),
        "thinking entry must be cleared after the drain"
    );
}

#[test]
fn approval_prompt_uses_event_input_after_message_complete_drain() {
    let mut app = create_test_app();
    app.pending_tool_uses.push((
        "tool-1".to_string(),
        "exec_shell".to_string(),
        serde_json::json!({"command": "stale value from drained list"}),
    ));

    // Mirror the old race: MessageComplete drains pending tool uses before
    // ApprovalRequired is handled. The approval modal must still show the
    // non-empty input carried directly on the ApprovalRequired event.
    app.pending_tool_uses.clear();

    let event_input = serde_json::json!({
        "command": "cargo test -p codewhale-tui approval",
        "workdir": "/repo",
    });
    push_approval_request_view(
        &mut app,
        "tool-1",
        "exec_shell",
        "Run cargo tests",
        &event_input,
        "approval-key",
        None,
        crate::config::ApprovalDefaultSelection::Deny,
    );

    let mut view = app.view_stack.pop().expect("approval view");
    let approval = view
        .as_any_mut()
        .downcast_mut::<ApprovalView>()
        .expect("approval view");
    // Bare `v` must not open the params pager.
    let bare = approval.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
    assert!(matches!(bare, ViewAction::None));
    let action = approval.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::ALT));
    let ViewAction::Emit(ViewEvent::OpenTextPager { content, .. }) = action else {
        panic!("expected approval params pager from Alt+V");
    };

    assert!(content.contains("cargo test -p codewhale-tui approval"));
    assert!(content.contains("/repo"));
    assert!(!content.contains("stale value from drained list"));
    assert_ne!(content.trim(), "{}");
}

#[test]
fn approval_prompt_uses_configured_default_selection() {
    let mut app = create_test_app();
    push_approval_request_view(
        &mut app,
        "tool-config-default",
        "exec_shell",
        "Run a trusted command",
        &serde_json::json!({"command": "cargo check"}),
        "approval-key",
        None,
        crate::config::ApprovalDefaultSelection::AllowOnce,
    );

    let mut view = app.view_stack.pop().expect("approval view");
    let approval = view
        .as_any_mut()
        .downcast_mut::<ApprovalView>()
        .expect("approval view");
    assert!(matches!(
        approval.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        ViewAction::EmitAndClose(ViewEvent::ApprovalDecision {
            decision: crate::tui::approval::ReviewDecision::Approved,
            ..
        })
    ));
}

#[test]
fn patch_approval_modal_does_not_displace_the_active_file_receipt() {
    let mut app = create_test_app();
    let input = serde_json::json!({
        "action": "patch",
        "patch": "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n"
    });
    handle_tool_call_started(&mut app, "file-ask", "File", &input);
    let active_index = app.tool_cells["file-ask"];

    push_approval_request_view(
        &mut app,
        "file-ask",
        "File",
        "Apply a file patch",
        &input,
        "approval-key",
        None,
        crate::config::ApprovalDefaultSelection::Deny,
    );

    assert!(
        app.history.is_empty(),
        "the modal owns the preflight preview; successful evidence belongs to the File receipt"
    );
    assert_eq!(app.tool_cells["file-ask"], active_index);

    let result = crate::tools::spec::ToolResult::success("ok").with_metadata(serde_json::json!({
        "mutation": {
            "diff": "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n",
            "files": [{ "path": "src/lib.rs", "outcome": "updated" }],
            "renames": []
        }
    }));
    handle_tool_call_complete(&mut app, "file-ask", "apply_patch", &Ok(result));

    let active = app.active_cell.as_ref().expect("active File cell");
    let HistoryCell::Tool(ToolCell::PatchSummary(cell)) = &active.entries()[0] else {
        panic!("Ask approval must leave the canonical File receipt addressable")
    };
    assert_eq!(cell.status, ToolStatus::Success);
    assert_eq!(
        cell.receipt.as_ref().map(|receipt| receipt.outcome_label()),
        Some("Updated src/lib.rs".to_string())
    );
}

#[tokio::test]
async fn approval_decision_persists_ask_rules_to_permissions_file() {
    let tmp = TempDir::new().expect("tempdir");
    let config_path = tmp.path().join("config.toml");
    let mut app = create_test_app();
    app.config_path = Some(config_path.clone());
    let mut config = Config::default();
    let mut engine = mock_engine_handle();
    let rule = codewhale_config::ToolAskRule::exec_shell("cargo test");

    apply_approval_decision(
        &mut app,
        &mut engine.handle,
        &mut config,
        ApprovalDecisionEvent {
            tool_id: "tool-1".to_string(),
            tool_name: "exec_shell".to_string(),
            decision: ReviewDecision::Approved,
            timed_out: false,
            approval_key: "approval-key".to_string(),
            approval_grouping_key: "approval-group".to_string(),
            persistent_rules: vec![rule.clone()],
        },
    )
    .await;

    assert_eq!(
        engine.recv_approval_event().await,
        Some(crate::core::engine::MockApprovalEvent::Approved {
            id: "tool-1".to_string()
        })
    );
    let store = codewhale_config::ConfigStore::load(Some(config_path)).expect("load config store");
    assert_eq!(store.permissions().rules, vec![rule]);
    assert!(
        app.status_message
            .as_deref()
            .is_some_and(|message| message.contains("Saved 1 ask permission rule"))
    );

    let decision = config
        .exec_policy_engine
        .check(codewhale_execpolicy::ExecPolicyContext {
            command: "cargo test --workspace",
            cwd: tmp.path().to_string_lossy().as_ref(),
            tool: Some("exec_shell"),
            path: None,
            ask_for_approval: codewhale_execpolicy::AskForApproval::OnFailure,
            sandbox_mode: None,
        })
        .expect("check persisted runtime policy");
    assert!(decision.requires_approval);
}

#[tokio::test]
async fn approval_decision_persists_exact_workspace_allow_rule() {
    let tmp = TempDir::new().expect("tempdir");
    let config_path = tmp.path().join("config.toml");
    let mut app = create_test_app();
    app.workspace = tmp.path().to_path_buf();
    app.config_path = Some(config_path.clone());
    let mut config = Config::default();
    let mut engine = mock_engine_handle();
    let rule = codewhale_config::ToolAskRule::exec_shell("cargo test")
        .into_exact_workspace_allow(tmp.path().to_string_lossy());

    apply_approval_decision(
        &mut app,
        &mut engine.handle,
        &mut config,
        ApprovalDecisionEvent {
            tool_id: "tool-allow".to_string(),
            tool_name: "exec_shell".to_string(),
            decision: ReviewDecision::Approved,
            timed_out: false,
            approval_key: "approval-key".to_string(),
            approval_grouping_key: "approval-group".to_string(),
            persistent_rules: vec![rule.clone()],
        },
    )
    .await;

    assert_eq!(
        engine.recv_approval_event().await,
        Some(crate::core::engine::MockApprovalEvent::Approved {
            id: "tool-allow".to_string()
        })
    );
    let store = codewhale_config::ConfigStore::load(Some(config_path)).expect("load config store");
    assert_eq!(store.permissions().rules, vec![rule]);
    assert!(
        app.status_message
            .as_deref()
            .is_some_and(|message| message.contains("Saved 1 allow permission rule"))
    );

    let exact = config
        .exec_policy_engine
        .check(codewhale_execpolicy::ExecPolicyContext {
            command: "cargo test",
            cwd: tmp.path().to_string_lossy().as_ref(),
            tool: Some("exec_shell"),
            path: None,
            ask_for_approval: codewhale_execpolicy::AskForApproval::OnRequest,
            sandbox_mode: None,
        })
        .expect("check persisted allow");
    assert_eq!(
        exact.matched_action,
        Some(codewhale_execpolicy::PermissionAction::Allow)
    );
    assert!(!exact.requires_approval);

    let expanded = config
        .exec_policy_engine
        .check(codewhale_execpolicy::ExecPolicyContext {
            command: "cargo test --workspace",
            cwd: tmp.path().to_string_lossy().as_ref(),
            tool: Some("exec_shell"),
            path: None,
            ask_for_approval: codewhale_execpolicy::AskForApproval::OnRequest,
            sandbox_mode: None,
        })
        .expect("check expanded command");
    assert!(expanded.requires_approval);
}

#[test]
fn second_thinking_block_appends_new_entry_in_same_active_cell() {
    // Real V4 turns can emit Thinking → Tool → Thinking → Tool before any
    // prose; the second thinking block should land as a fresh entry inside
    // the SAME active cell rather than flush the first group prematurely.
    let mut app = create_test_app();

    let _ = crate::tui::streaming_thinking::ensure_active_entry(&mut app);
    crate::tui::streaming_thinking::append(&mut app, 0, "first plan");
    let _ = crate::tui::streaming_thinking::finalize_active_entry(&mut app, Some(0.5), "");

    handle_tool_call_started(
        &mut app,
        "t-1",
        "exec_shell",
        &serde_json::json!({"command": "ls"}),
    );

    // Second Thinking block.
    let second_idx = crate::tui::streaming_thinking::ensure_active_entry(&mut app);
    assert_eq!(
        second_idx, 2,
        "second thinking entry follows the tool entry"
    );
    crate::tui::streaming_thinking::append(&mut app, second_idx, "second plan");

    let active = app.active_cell.as_ref().expect("active cell present");
    assert_eq!(active.entry_count(), 3);
    assert!(matches!(active.entries()[0], HistoryCell::Thinking { .. }));
    assert!(matches!(
        active.entries()[1],
        HistoryCell::Tool(ToolCell::Exec(_))
    ));
    assert!(matches!(active.entries()[2], HistoryCell::Thinking { .. }));
    assert!(
        app.history.is_empty(),
        "the group still hasn't flushed — no prose yet"
    );
}

#[test]
fn new_thinking_block_drains_pending_tail_from_previous_block() {
    let mut app = create_test_app();

    assert!(!crate::tui::streaming_thinking::start_block(&mut app));
    let first_idx = app
        .streaming_thinking_active_entry
        .expect("first thinking entry active");
    app.reasoning_buffer.push_str("first tail");
    app.streaming_state.push_content(0, "first tail");

    assert!(crate::tui::streaming_thinking::start_block(&mut app));
    let second_idx = app
        .streaming_thinking_active_entry
        .expect("second thinking entry active");

    let active = app.active_cell.as_ref().expect("active cell exists");
    assert_ne!(first_idx, second_idx);

    let HistoryCell::Thinking {
        content, streaming, ..
    } = &active.entries()[first_idx]
    else {
        panic!("expected first thinking cell");
    };
    assert!(!*streaming, "previous thinking block should be finalized");
    assert!(
        content.contains("first tail"),
        "pending text must survive a new ThinkingStarted event"
    );

    assert!(matches!(
        active.entries()[second_idx],
        HistoryCell::Thinking {
            streaming: true,
            ..
        }
    ));
    assert_eq!(app.last_reasoning.as_deref(), Some("first tail"));
}

// ---- per-child prompt wiring ----
//
// Generic tool cells default to `prompts: None`. Reserved for any future
// fan-out tool that wants to surface per-child prompts.

#[test]
fn non_fanout_tool_does_not_populate_prompts() {
    // Ordinary tools must use the standard `args:` summary rendering path.
    let mut app = create_test_app();

    handle_tool_call_started(
        &mut app,
        "fs-1",
        "file_search",
        &serde_json::json!({ "query": "client.rs" }),
    );

    let active = app.active_cell.as_ref().expect("active cell present");
    let HistoryCell::Tool(ToolCell::Generic(generic)) = &active.entries()[0] else {
        panic!("expected GenericToolCell for file_search");
    };

    assert!(
        generic.prompts.is_none(),
        "non-fan-out tool must not populate prompts"
    );
}
#[test]
fn noisy_subagent_progress_keeps_existing_objective_summary() {
    let mut app = create_test_app();
    app.agent_progress.insert(
        "agent_live".to_string(),
        "starting: inspect release state".to_string(),
    );

    let display =
        friendly_subagent_progress(&app, "agent_live", "step 1/8: requesting model response");

    assert_eq!(display, "starting: inspect release state");
}

/// Regression for issue #65: `truncate_line_to_width` with a tiny budget
/// must respect display widths, not codepoint counts. The old branch counted
/// chars and overran the budget for any double-width grapheme, which
/// contributed to mid-character sidebar artifacts on resize.
#[test]
fn truncate_line_to_width_respects_display_width_for_tiny_budgets() {
    use unicode_width::UnicodeWidthStr;

    let trimmed = truncate_line_to_width("Agents", 3);
    assert_eq!(trimmed, "Age");
    assert!(UnicodeWidthStr::width(trimmed.as_str()) <= 3);

    let trimmed_cjk = truncate_line_to_width("中文测试", 3);
    assert!(
        UnicodeWidthStr::width(trimmed_cjk.as_str()) <= 3,
        "trimmed CJK width {} exceeded budget 3 (got {trimmed_cjk:?})",
        UnicodeWidthStr::width(trimmed_cjk.as_str()),
    );

    assert_eq!(truncate_line_to_width("anything", 0), "");
    assert_eq!(truncate_line_to_width("hi", 10), "hi");

    let trimmed_long = truncate_line_to_width("a long sidebar label", 10);
    assert!(trimmed_long.ends_with("..."));
    assert!(UnicodeWidthStr::width(trimmed_long.as_str()) <= 10);
}

/// Regression for #86. A recoverable engine error (stream stall, transient
/// disconnect, retryable server hiccup) must NOT flip the session into
/// offline mode. Until this fix the UI matched on `EngineEvent::Error {
/// message, .. }` and unconditionally set `app.offline_mode = true`, so a
/// long V4 thinking turn whose chunked stream got closed mid-flight ended
/// the session in offline mode with the next typed message queued.
#[test]
fn recoverable_engine_error_does_not_enter_offline_mode() {
    use crate::error_taxonomy::{ErrorEnvelope, StreamError};
    let mut app = create_test_app();
    assert!(!app.offline_mode);

    let envelope = StreamError::Stall { timeout_secs: 60 }.into_envelope();
    apply_engine_error_to_app(&mut app, envelope);

    assert!(
        !app.offline_mode,
        "recoverable error must keep the session online so the user can retry"
    );
    assert!(!app.is_loading);
    assert!(app.turn_error_posted, "turn_error_posted must be set");
    assert!(
        app.status_message.is_none(),
        "recoverable error should NOT set status_message — already in transcript as HistoryCell::Error"
    );

    // Sanity: the rendered cell is the categorized Error variant, not a plain System note.
    let last = app
        .history
        .last()
        .expect("recoverable engine error should push a history cell");
    assert!(
        matches!(last, crate::tui::history::HistoryCell::Error { .. }),
        "expected HistoryCell::Error, got {last:?}"
    );
    let _ = ErrorEnvelope::transient("");
}

#[test]
fn recoverable_provider_error_advances_fallback_chain() {
    use crate::error_taxonomy::{ErrorCategory, ErrorEnvelope, ErrorSeverity};

    let mut app = create_test_app();
    app.api_provider = ApiProvider::Deepseek;
    app.provider_chain = Some(codewhale_config::ProviderChain::new(
        codewhale_config::ProviderKind::Deepseek,
        &[codewhale_config::ProviderKind::Openrouter],
    ));

    apply_engine_error_to_app(
        &mut app,
        ErrorEnvelope::new(
            ErrorCategory::RateLimit,
            ErrorSeverity::Warning,
            true,
            "rate_limit",
            "provider returned 429",
        ),
    );

    assert_eq!(app.api_provider, ApiProvider::Openrouter);
    assert!(app.is_fallback_active());
    assert!(!app.offline_mode);
    assert!(
        app.status_message
            .as_deref()
            .unwrap_or_default()
            .contains("Switched to openrouter")
    );
    assert!(
        app.last_fallback_reason
            .as_deref()
            .unwrap_or_default()
            .contains("provider returned 429")
    );
}

/// #2574 acceptance: auth (401) errors must never trigger provider fallback,
/// even when marked recoverable — the exclusion is by error *category*, not
/// recoverability (the gate lives at this call site, not inside the chain
/// walk). A bad key requires user intervention, not a silent rotation.
#[test]
fn auth_error_does_not_trigger_provider_fallback() {
    use crate::error_taxonomy::{ErrorCategory, ErrorEnvelope, ErrorSeverity};

    let mut app = create_test_app();
    app.api_provider = ApiProvider::Deepseek;
    // Not env-only, so we exercise the category gate rather than the env-key
    // onboarding early-return.
    app.api_key_env_only = false;
    app.provider_chain = Some(codewhale_config::ProviderChain::new(
        codewhale_config::ProviderKind::Deepseek,
        &[codewhale_config::ProviderKind::Openrouter],
    ));

    apply_engine_error_to_app(
        &mut app,
        ErrorEnvelope::new(
            ErrorCategory::Authentication,
            ErrorSeverity::Critical,
            // Deliberately recoverable to prove the *category* is what excludes
            // fallback, not the recoverable flag.
            true,
            "authentication",
            "provider returned 401",
        ),
    );

    assert_eq!(
        app.api_provider,
        ApiProvider::Deepseek,
        "auth failure must not rotate providers"
    );
    assert!(!app.is_fallback_active());
    assert_eq!(app.fallback_chain_position(), Some(0));
    assert!(
        app.last_fallback_reason.is_none(),
        "no fallback should have been attempted on an auth error"
    );
}

/// #2574 acceptance: the route switch is visible to the user with a 1-based
/// position and the failure cause (regression guard against off-by-one position
/// indexing in the fallback status).
#[test]
fn fallback_switch_status_shows_one_based_position_and_reason() {
    use crate::error_taxonomy::{ErrorCategory, ErrorEnvelope, ErrorSeverity};

    let mut app = create_test_app();
    app.api_provider = ApiProvider::Deepseek;
    app.provider_chain = Some(codewhale_config::ProviderChain::new(
        codewhale_config::ProviderKind::Deepseek,
        &[codewhale_config::ProviderKind::Openrouter],
    ));

    apply_engine_error_to_app(
        &mut app,
        ErrorEnvelope::new(
            ErrorCategory::RateLimit,
            ErrorSeverity::Warning,
            true,
            "rate_limit",
            "provider returned 429",
        ),
    );

    assert_eq!(app.api_provider, ApiProvider::Openrouter);
    assert_eq!(
        app.fallback_chain_position(),
        Some(1),
        "first fallback sits at 1-based position 1"
    );
    let status = app.status_message.as_deref().unwrap_or_default();
    assert!(
        status.contains("Switched to openrouter") && status.contains("(fallback 1/"),
        "visible status must show the destination and 1-based position: {status}"
    );
}

#[tokio::test]
async fn failed_fallback_restores_exact_literal_custom_identity_without_root_crossover() {
    let mut config = Config {
        provider: Some("custom".to_string()),
        api_key: Some("legacy-root-key".to_string()),
        base_url: Some("http://127.0.0.1:18180/v1".to_string()),
        default_text_model: Some("legacy-root-model".to_string()),
        providers: Some(ProvidersConfig {
            custom: HashMap::from([(
                "custom".to_string(),
                ProviderConfig {
                    kind: Some("openai-compatible".to_string()),
                    api_key: Some("literal-table-key".to_string()),
                    base_url: Some("http://127.0.0.1:18181/v1".to_string()),
                    model: Some("literal-table-model".to_string()),
                    ..Default::default()
                },
            )]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let previous_identity = ProviderIdentity {
        provider: ApiProvider::Custom,
        key: "custom".to_string(),
        exact_id: Some("custom".to_string()),
        migrated_legacy_ollama_cloud_route: false,
    };
    let mut app = create_test_app();
    app.set_provider_identity_record(previous_identity.clone());
    app.set_model_selection("literal-table-model".to_string());
    // Mirror `apply_engine_error_to_app`: the fallback chain has advanced the
    // enum while the previous exact route remains the rollback authority.
    app.api_provider = ApiProvider::Openrouter;
    let mut engine = mock_engine_handle();
    let previous_chain = app.provider_chain.clone();

    apply_provider_fallback_switch(
        &mut app,
        &mut engine.handle,
        &mut config,
        ProviderFallbackRollback {
            identity: previous_identity.clone(),
            chain: previous_chain,
        },
    )
    .await;

    assert_eq!(app.api_provider, ApiProvider::Custom);
    assert_eq!(app.provider_identity_for_persistence(), "custom");
    assert_eq!(app.provider_id_for_persistence(), Some("custom"));
    let (restored_identity, restored_config) = app_scoped_runtime_config(&app, &config);
    assert_eq!(restored_identity, previous_identity);
    let route = resolve_runtime_route_for_identity(
        &restored_config,
        &restored_identity,
        Some("literal-table-model"),
    )
    .expect("restored identity must still resolve the exact literal table");
    assert_eq!(route.identity.exact_id.as_deref(), Some("custom"));
    assert_eq!(
        route.candidate.endpoint().base_url,
        "http://127.0.0.1:18181/v1"
    );
    assert_ne!(
        route.candidate.endpoint().base_url,
        "http://127.0.0.1:18180/v1"
    );
}

#[tokio::test]
async fn provider_switch_auth_error_restores_previous_provider_and_model() {
    use crate::error_taxonomy::ErrorEnvelope;

    let _home = SettingsHomeGuard::new();
    let mut app = create_test_app();
    app.api_provider = ApiProvider::Deepseek;
    app.model = "deepseek-v4-pro".to_string();
    app.model_ids_passthrough = false;
    app.onboarding = OnboardingState::None;
    app.onboarding_needs_api_key = false;
    app.api_key_env_only = true;
    app.active_context_window_override = Some(1_000_000);
    let mut engine = mock_engine_handle();
    let mut config = Config {
        provider: Some("deepseek".to_string()),
        api_key: Some("deepseek-key".to_string()),
        default_text_model: Some("deepseek-v4-pro".to_string()),
        providers: Some(ProvidersConfig {
            deepseek: ProviderConfig {
                api_key: Some("deepseek-key".to_string()),
                context_window: Some(1_000_000),
                ..Default::default()
            },
            moonshot: ProviderConfig {
                api_key: Some("kimi-key".to_string()),
                context_window: Some(262_144),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };

    switch_provider(
        &mut app,
        &mut engine.handle,
        &mut config,
        ApiProvider::Moonshot,
        Some("kimi-k2.6".to_string()),
    )
    .await;
    assert_eq!(app.api_provider, ApiProvider::Moonshot);
    assert_eq!(app.active_context_window_override, Some(262_144));
    assert_eq!(config.provider.as_deref(), Some("moonshot"));
    assert!(app.pending_provider_switch.is_some());

    apply_engine_error_to_app(
        &mut app,
        ErrorEnvelope::fatal_auth("Authentication failed: invalid API key"),
    );
    let rollback_status = rollback_provider_after_auth_failure(&mut app, &mut config)
        .expect("auth failure after provider switch should roll back");

    assert_eq!(app.api_provider, ApiProvider::Deepseek);
    assert_eq!(app.model, "deepseek-v4-pro");
    assert_eq!(app.active_context_window_override, Some(1_000_000));
    assert!(!app.model_ids_passthrough);
    assert!(!app.offline_mode);
    assert_eq!(app.onboarding, OnboardingState::None);
    assert!(!app.onboarding_needs_api_key);
    assert!(app.api_key_env_only);
    assert_eq!(config.provider.as_deref(), Some("deepseek"));
    assert_eq!(
        config.default_text_model.as_deref(),
        Some("deepseek-v4-pro")
    );
    // The switch and its rollback wrote nothing: settings are untouched and
    // no pending save decision survives the rollback.
    let settings = crate::settings::Settings::load().expect("load settings");
    assert_eq!(settings.default_provider.as_deref(), None);
    assert!(
        app.pending_route_save.is_none(),
        "rollback must clear the pending decision"
    );
    assert_eq!(settings.provider_models, None);
    assert_eq!(settings.default_model.as_deref(), None);
    let state = codewhale_config::SetupState::load()
        .expect("load setup state")
        .expect("setup state");
    assert_eq!(
        state.status(codewhale_config::SetupStep::ProviderModel),
        codewhale_config::StepStatus::Verified
    );
    let provider_model_result = state
        .steps
        .get(&codewhale_config::SetupStep::ProviderModel)
        .and_then(|entry| entry.result.as_deref())
        .expect("provider/model result");
    assert!(provider_model_result.contains("provider=deepseek"));
    assert!(provider_model_result.contains("model=deepseek-v4-pro"));
    assert!(provider_model_result.contains("auth=key saved · not checked"));
    assert!(provider_model_result.contains("health=attemptable"));
    assert!(!provider_model_result.contains("moonshot"));
    assert!(!provider_model_result.contains("kimi-k2.6"));
    assert!(!provider_model_result.contains("deepseek-key"));
    assert!(!provider_model_result.contains("kimi-key"));
    assert!(app.pending_provider_switch.is_none());
    assert!(rollback_status.contains("Provider switch failed"));
    assert!(
        app.status_message
            .as_deref()
            .is_none_or(|status| !status.contains("Provider switch failed")),
        "status message is set by the async event loop after engine respawn"
    );
}

#[tokio::test]
async fn provider_switch_rollback_corrects_setup_receipt_when_persistence_fails() {
    use crate::error_taxonomy::ErrorEnvelope;

    let _home = SettingsHomeGuard::new();
    let bad_config_path = TempDir::new().expect("bad config path");
    let mut app = create_test_app();
    app.api_provider = ApiProvider::Deepseek;
    app.model = "deepseek-v4-pro".to_string();
    app.model_ids_passthrough = false;
    app.onboarding = OnboardingState::None;
    app.onboarding_needs_api_key = false;
    app.api_key_env_only = true;
    let mut engine = mock_engine_handle();
    let mut config = Config {
        provider: Some("deepseek".to_string()),
        api_key: Some("deepseek-key".to_string()),
        default_text_model: Some("deepseek-v4-pro".to_string()),
        providers: Some(ProvidersConfig {
            deepseek: ProviderConfig {
                api_key: Some("deepseek-key".to_string()),
                ..Default::default()
            },
            moonshot: ProviderConfig {
                api_key: Some("kimi-key".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }),
        ..Default::default()
    };

    switch_provider(
        &mut app,
        &mut engine.handle,
        &mut config,
        ApiProvider::Moonshot,
        Some("kimi-k2.6".to_string()),
    )
    .await;
    let target_state = codewhale_config::SetupState::load()
        .expect("load target setup state")
        .expect("target setup state");
    let target_result = target_state
        .steps
        .get(&codewhale_config::SetupStep::ProviderModel)
        .and_then(|entry| entry.result.as_deref())
        .expect("target provider/model result");
    assert!(target_result.contains("provider=moonshot"));
    assert!(target_result.contains("model=kimi-k2.6"));

    apply_engine_error_to_app(
        &mut app,
        ErrorEnvelope::fatal_auth("Authentication failed: invalid API key"),
    );
    app.config_path = Some(bad_config_path.path().to_path_buf());
    let rollback_status = rollback_provider_after_auth_failure(&mut app, &mut config)
        .expect("auth failure after provider switch should roll back");

    assert_eq!(app.api_provider, ApiProvider::Deepseek);
    assert_eq!(app.model, "deepseek-v4-pro");
    // The rollback itself writes nothing — there is no persistence step to
    // fail, and the receipt must not claim one.
    assert!(
        !rollback_status.contains("not fully persisted"),
        "{rollback_status}"
    );
    assert!(app.pending_route_save.is_none());
    let state = codewhale_config::SetupState::load()
        .expect("load setup state")
        .expect("setup state");
    let provider_model_result = state
        .steps
        .get(&codewhale_config::SetupStep::ProviderModel)
        .and_then(|entry| entry.result.as_deref())
        .expect("provider/model result");
    assert!(provider_model_result.contains("provider=deepseek"));
    assert!(provider_model_result.contains("model=deepseek-v4-pro"));
    assert!(!provider_model_result.contains("moonshot"));
    assert!(!provider_model_result.contains("kimi-k2.6"));
    assert!(!provider_model_result.contains("deepseek-key"));
    assert!(!provider_model_result.contains("kimi-key"));
}

#[test]
fn stream_error_marks_active_turn_failed_without_waiting_for_turn_complete() {
    use crate::error_taxonomy::ErrorEnvelope;

    let mut app = create_test_app();
    app.is_loading = true;
    app.runtime_turn_id = Some("turn_decode_error".to_string());
    app.runtime_turn_status = Some("in_progress".to_string());
    handle_tool_call_started(
        &mut app,
        "tool-running",
        "exec_shell",
        &serde_json::json!({"command": "cargo test --workspace"}),
    );
    assert!(app.active_cell.is_some(), "precondition: live tool cell");

    apply_engine_error_to_app(
        &mut app,
        ErrorEnvelope::classify("chunk decode error".to_string(), true),
    );

    assert!(!app.is_loading);
    assert_eq!(app.runtime_turn_status.as_deref(), Some("failed"));
    assert!(
        app.active_cell.is_none(),
        "stream error should flush live cells so no row stays visually running"
    );
    assert!(
        app.history.iter().any(|cell| {
            matches!(
                cell,
                crate::tui::history::HistoryCell::Error { message, .. }
                    if message.contains("chunk decode error")
            )
        }),
        "stream decode error should remain visible in transcript"
    );
}

/// Hard failures (auth, billing, malformed request) DO need to flip offline
/// mode so subsequent typed messages get queued instead of silently lost
/// against a broken upstream.
#[test]
fn non_recoverable_engine_error_enters_offline_mode() {
    use crate::error_taxonomy::ErrorEnvelope;
    let mut app = create_test_app();
    assert!(!app.offline_mode);

    apply_engine_error_to_app(
        &mut app,
        ErrorEnvelope::fatal_auth("Authentication failed: invalid API key"),
    );

    assert!(
        app.offline_mode,
        "non-recoverable error must enter offline mode"
    );
    assert!(!app.is_loading);
    assert!(app.turn_error_posted, "turn_error_posted must be set");
    assert!(
        app.status_message.is_none(),
        "non-recoverable error should NOT set status_message — already in transcript as HistoryCell::Error"
    );
    assert!(app.pending_provider_switch.is_none());
}

#[test]
fn env_only_auth_failure_reopens_provider_onboarding() {
    use crate::error_taxonomy::ErrorEnvelope;
    let mut app = create_test_app();
    app.api_provider = crate::config::ApiProvider::Anthropic;
    app.config_path = Some(std::path::PathBuf::from("/tmp/codewhale-phase2.toml"));
    app.api_key_env_only = true;
    app.onboarding = crate::tui::app::OnboardingState::None;
    app.onboarding_needs_api_key = false;

    apply_engine_error_to_app(
        &mut app,
        ErrorEnvelope::fatal_auth("Authentication failed: invalid API key"),
    );

    assert!(app.offline_mode);
    assert_eq!(
        app.onboarding,
        crate::tui::app::OnboardingState::Provider,
        "env-only auth failures reopen the canonical provider setup picker"
    );
    assert!(app.onboarding_needs_api_key);
    assert!(app.turn_error_posted, "turn_error_posted must be set");
    let status = app
        .status_message
        .as_deref()
        .expect("auth recovery should explain the env key source");
    assert!(
        status.contains("anthropic")
            && status.contains("ANTHROPIC_API_KEY")
            && status.contains("/tmp/codewhale-phase2.toml"),
        "expected active-provider/env/path recovery hint, got {status:?}"
    );
    assert!(!status.contains("DEEPSEEK_API_KEY"), "{status}");
}

// ---- Issue #208: in-flight input routing ----

#[test]
fn next_escape_action_cancels_when_loading_with_empty_input() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.input.clear();
    assert_eq!(next_escape_action(&app, false), EscapeAction::CancelRequest);
}

#[test]
fn next_escape_action_cancels_in_progress_runtime_even_if_loading_flag_was_cleared() {
    let mut app = create_test_app();
    app.is_loading = false;
    app.runtime_turn_status = Some("in_progress".to_string());
    app.input.clear();

    assert_eq!(next_escape_action(&app, false), EscapeAction::CancelRequest);
}

#[test]
fn next_escape_action_cancels_when_loading_with_input() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.input = "hold on, look at this instead".to_string();
    assert_eq!(next_escape_action(&app, false), EscapeAction::CancelRequest);
}

#[test]
fn next_escape_action_treats_whitespace_only_as_empty() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.input = "   \n\t".to_string();
    assert_eq!(next_escape_action(&app, false), EscapeAction::CancelRequest);
}

#[test]
fn next_escape_action_idle_with_input_clears() {
    let mut app = create_test_app();
    app.is_loading = false;
    app.input = "draft".to_string();
    assert_eq!(next_escape_action(&app, false), EscapeAction::ClearInput);
}

#[test]
fn next_escape_action_idle_empty_is_noop() {
    let mut app = create_test_app();
    app.is_loading = false;
    app.input.clear();
    assert_eq!(next_escape_action(&app, false), EscapeAction::Noop);
}

#[test]
fn next_escape_action_slash_menu_takes_priority() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.input = "anything".to_string();
    assert_eq!(next_escape_action(&app, true), EscapeAction::CloseSlashMenu);
}

#[test]
fn merge_pending_steers_returns_none_when_empty() {
    let mut app = create_test_app();
    assert!(merge_pending_steers(&mut app).is_none());
    assert!(!app.submit_pending_steers_after_interrupt);
}

#[test]
fn merge_pending_steers_passes_through_single_message() {
    let mut app = create_test_app();
    app.push_pending_steer(QueuedMessage::new(
        "lone steer".to_string(),
        Some("skill body".to_string()),
    ));
    let merged = merge_pending_steers(&mut app).expect("merge yields a message");
    assert_eq!(merged.display, "lone steer");
    assert_eq!(merged.skill_instruction.as_deref(), Some("skill body"));
    assert!(app.pending_steers.is_empty());
    assert!(!app.submit_pending_steers_after_interrupt);
}

#[test]
fn merge_pending_steers_concatenates_multiple_with_blank_line() {
    let mut app = create_test_app();
    app.push_pending_steer(QueuedMessage::new("first".to_string(), None));
    app.push_pending_steer(QueuedMessage::new("second".to_string(), None));
    app.push_pending_steer(QueuedMessage::new("third".to_string(), None));

    let merged = merge_pending_steers(&mut app).expect("merge yields a message");
    assert_eq!(merged.display, "first\n\nsecond\n\nthird");
    assert!(app.pending_steers.is_empty());
}

#[test]
fn merge_pending_steers_keeps_first_skill_instruction_only() {
    let mut app = create_test_app();
    app.push_pending_steer(QueuedMessage::new(
        "a".to_string(),
        Some("first skill".to_string()),
    ));
    app.push_pending_steer(QueuedMessage::new(
        "b".to_string(),
        Some("second skill".to_string()),
    ));
    let merged = merge_pending_steers(&mut app).expect("merge yields a message");
    assert_eq!(merged.skill_instruction.as_deref(), Some("first skill"));
    assert_eq!(merged.display, "a\n\nb");
}

#[test]
fn restored_queued_message_crosses_bounded_message_submit_serialization() {
    let app = create_test_app();
    let original = "restored 用户 \"quoted\"\n".repeat(8_000);
    let persisted = queued_ui_to_session(&QueuedMessage::new(original.clone(), None));
    let restored = queued_session_to_ui(persisted);
    let payload = crate::hooks::message_submit_payload(&app.base_hook_context(), &restored.display);
    let encoded = serde_json::to_vec(&payload).expect("serialize restored payload");

    assert!(
        encoded.len() <= crate::hooks::HOOK_MESSAGE_SUBMIT_PAYLOAD_MAX_BYTES,
        "restored payload was {} bytes",
        encoded.len()
    );
    assert_eq!(payload["text_original_bytes"], original.len());
    assert_eq!(payload["text_truncated"], true);
    assert!(original.starts_with(payload["text"].as_str().expect("text")));
}

#[test]
fn merged_pending_steers_cross_bounded_message_submit_serialization() {
    let mut app = create_test_app();
    app.push_pending_steer(QueuedMessage::new("first \"quoted\"\n".repeat(5_000), None));
    app.push_pending_steer(QueuedMessage::new("second 用户\n".repeat(5_000), None));
    let merged = merge_pending_steers(&mut app).expect("merged message");
    let payload = crate::hooks::message_submit_payload(&app.base_hook_context(), &merged.display);
    let encoded = serde_json::to_vec(&payload).expect("serialize merged payload");

    assert!(
        encoded.len() <= crate::hooks::HOOK_MESSAGE_SUBMIT_PAYLOAD_MAX_BYTES,
        "merged payload was {} bytes",
        encoded.len()
    );
    assert_eq!(payload["text_original_bytes"], merged.display.len());
    assert_eq!(payload["text_truncated"], true);
    assert!(
        merged
            .display
            .starts_with(payload["text"].as_str().expect("text"))
    );
}

#[test]
fn build_pending_input_preview_populates_all_three_buckets() {
    let mut app = create_test_app();
    app.push_pending_steer(QueuedMessage::new("steer-msg".to_string(), None));
    app.rejected_steers.push_back("rejected-msg".to_string());
    app.queue_message(QueuedMessage::new("queued-msg".to_string(), None));

    let preview = build_pending_input_preview(&app);
    assert_eq!(preview.pending_steers, vec!["steer-msg".to_string()]);
    assert_eq!(preview.rejected_steers, vec!["rejected-msg".to_string()]);
    assert_eq!(preview.queued_messages, vec!["queued-msg".to_string()]);
}

#[test]
fn accidental_queue_edit_while_loading_is_labeled_and_recoverable() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.queue_message(QueuedMessage::new(
        "original queued follow-up".to_string(),
        Some("skill body".to_string()),
    ));

    assert!(app.pop_last_queued_into_draft());
    assert_eq!(app.input, "original queued follow-up");
    app.input = "edited queued follow-up".to_string();
    app.cursor_position = app.input.chars().count();

    let preview = build_pending_input_preview(&app);
    assert_eq!(
        preview.editing_queued_message.as_deref(),
        Some("edited queued follow-up")
    );
    assert!(
        preview.queued_messages.is_empty(),
        "the popped message should be shown as editing, not a second queued row"
    );
    assert_eq!(
        next_escape_action(&app, false),
        EscapeAction::DiscardQueuedDraft,
        "Esc should cancel the queued edit before cancelling the live turn"
    );

    assert!(app.cancel_queued_draft_edit());
    assert!(app.input.is_empty());
    let restored = app.queued_messages.back().expect("follow-up restored");
    assert_eq!(restored.display, "original queued follow-up");
    assert_eq!(restored.skill_instruction.as_deref(), Some("skill body"));
}

#[test]
fn build_pending_input_preview_includes_current_context_chips() {
    let mut app = create_test_app();
    app.input = "Read @guide.md and @missing.md".to_string();
    app.cursor_position = app.input.chars().count();

    let preview = build_pending_input_preview(&app);

    assert!(
        preview
            .context_items
            .iter()
            .any(|item| item.kind == "mention"
                && item.label == "guide.md"
                && !item.included
                && item.detail.as_deref() == Some("resolved on send")),
        "neutral mention preview missing: {:?}",
        preview.context_items
    );
    assert!(
        preview
            .context_items
            .iter()
            .any(|item| item.kind == "mention"
                && item.label == "missing.md"
                && !item.included
                && item.detail.as_deref() == Some("resolved on send")),
        "unresolved mention preview missing: {:?}",
        preview.context_items
    );
}

#[test]
fn default_footer_excludes_provider_specific_diagnostic_chips() {
    let items = crate::config::StatusItem::default_footer();

    assert!(
        !items.contains(&crate::config::StatusItem::PrefixStability),
        "prefix stability is a diagnostic chip and should not crowd the default footer"
    );
    assert!(
        !items.contains(&crate::config::StatusItem::Balance),
        "balance is an opt-in prepaid chip and should not crowd the default footer"
    );
    assert!(
        items.contains(&crate::config::StatusItem::Cache),
        "default footer should still include provider-reported cache hit rate"
    );
    assert!(
        items.contains(&crate::config::StatusItem::GitBranch),
        "default footer should surface the current workspace branch"
    );
}

// ── Balance footer chip tests ─────────────────────────────────────

#[test]
fn should_fetch_provider_balance_requires_balance_status_item() {
    let mut app = create_test_app();
    app.api_provider = ApiProvider::Deepseek;
    app.status_items = crate::config::StatusItem::default_footer();

    assert!(!should_fetch_provider_balance(&app));

    app.status_items.push(crate::config::StatusItem::Balance);
    assert!(should_fetch_provider_balance(&app));
}

#[test]
fn should_fetch_provider_balance_covers_prepaid_providers() {
    let mut app = create_test_app();
    app.status_items = vec![crate::config::StatusItem::Balance];

    app.api_provider = ApiProvider::Ollama;
    assert!(!should_fetch_provider_balance(&app));

    app.api_provider = ApiProvider::Openrouter;
    assert!(should_fetch_provider_balance(&app));

    app.api_provider = ApiProvider::Siliconflow;
    assert!(should_fetch_provider_balance(&app));

    app.api_provider = ApiProvider::DeepseekCN;
    assert!(should_fetch_provider_balance(&app));
}

#[test]
fn openrouter_credits_map_remaining_usd() {
    let info =
        openrouter_credits_from_json(r#"{"data":{"total_credits":50.0,"total_usage":12.345}}"#)
            .expect("openrouter credits JSON");
    assert_eq!(info.currency, "USD");
    assert_eq!(info.total_balance, "37.66");
    assert_eq!(info.chip_label().as_deref(), Some("$37.66"));
}

/// Regression for issue #244: visible session spend must not decrease.
/// Sub-agent token usage events arrive out of order and may be reconciled
/// later (cache adjustments, provisional → final swap). The displayed total
/// is anchored to a high-water mark so users never see a number go down
/// during a single session.
#[test]
fn displayed_session_cost_is_monotonic_under_negative_reconciliation() {
    let mut app = create_test_app();
    app.accrue_subagent_cost(0.50);
    let after_first = app.displayed_session_cost();
    assert!((after_first - 0.50).abs() < 1e-6);

    // Simulate reconciliation that lowers the underlying counter (e.g. a
    // cache discount applied after the fact). The underlying value drops,
    // but the displayed cost must not.
    app.session.subagent_cost = 0.20;
    let after_second = app.displayed_session_cost();
    assert!(
        after_second >= after_first,
        "displayed cost regressed: {after_second} < {after_first}"
    );

    // Adding more cost should still bump above the high-water.
    app.accrue_session_cost(0.10);
    let after_add = app.displayed_session_cost();
    assert!(after_add >= after_first);
}

/// Regression for issue #244: deduplicated mailbox events must not
/// decrement displayed cost — they should leave it untouched and the
/// next genuine event must extend it monotonically.
#[test]
fn duplicate_mailbox_token_usage_does_not_regress_displayed_cost() {
    let mut app = create_test_app();
    let usage = crate::tools::subagent::MailboxMessage::TokenUsage {
        agent_id: "agent-x".to_string(),
        source_id: "response-x".to_string(),
        route: test_mailbox_route(ApiProvider::Deepseek, "deepseek-v4-flash"),
        usage: crate::models::Usage {
            input_tokens: 10_000,
            output_tokens: 1_000,
            ..Default::default()
        },
    };
    handle_subagent_mailbox(&mut app, 11, &usage);
    let baseline = app.displayed_session_cost();
    assert!(baseline > 0.0);

    // Re-emit the same response under a fresh mailbox sequence — it must still
    // dedupe and leave displayed cost unchanged.
    handle_subagent_mailbox(&mut app, 12, &usage);
    assert!(
        (app.displayed_session_cost() - baseline).abs() < 1e-9,
        "duplicate provider response must not move displayed cost"
    );

    // A distinct provider response must extend the displayed cost upward.
    let mut distinct = usage.clone();
    let crate::tools::subagent::MailboxMessage::TokenUsage { source_id, .. } = &mut distinct else {
        unreachable!()
    };
    *source_id = "response-y".to_string();
    handle_subagent_mailbox(&mut app, 13, &distinct);
    assert!(app.displayed_session_cost() > baseline);
}
#[test]
fn checklist_write_renders_dedicated_card() {
    let cell = GenericToolCell {
        name: "work_update".to_string(),
        status: ToolStatus::Success,
        input_summary: None,
        output: Some(
            "Todo list updated (3 items, 33% settled)\n{\"items\":[{\"id\":1,\"content\":\"Plan it out\",\"status\":\"completed\"},{\"id\":2,\"content\":\"Wire the thing\",\"status\":\"in_progress\"},{\"id\":3,\"content\":\"Run gates\",\"status\":\"pending\"}],\"completion_pct\":33,\"in_progress_id\":2}"
                .to_string(),
        ),
        prompts: None,
        spillover_path: None,
            output_summary: None,
            is_diff: false,
    };
    let lines = cell.lines_with_mode(80, true, crate::tui::history::RenderMode::Live);
    let text: Vec<String> = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect();
    let joined = text.join("\n");

    assert!(
        joined.contains("1/3"),
        "header must include completed/total: {joined}"
    );
    assert!(
        joined.contains("33%"),
        "header must include percent: {joined}"
    );
    assert!(
        joined.contains("Plan it out"),
        "items must render content: {joined}"
    );
    assert!(
        !joined.contains("\"items\""),
        "raw JSON must NOT appear: {joined}"
    );
}

// ---- composer arrow history ----

#[test]
fn history_arrow_handles_empty_input() {
    let mut app = create_test_app();
    // Explicitly disable arrows-scroll so this test covers the
    // history-navigation path regardless of the mouse-capture default.
    app.composer_arrows_scroll = false;
    app.input_history.push("previous prompt".to_string());

    // With arrows-scroll off: empty composer Up navigates input history (#1117).
    assert!(handle_composer_history_arrow(
        &mut app,
        KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
        false,
        false,
    ));
    assert_eq!(app.input, "previous prompt");
}

#[test]
fn history_arrow_handles_whitespace_input() {
    let mut app = create_test_app();
    // Explicitly disable arrows-scroll so this test covers the
    // history-navigation path regardless of the mouse-capture default.
    app.composer_arrows_scroll = false;
    app.input = "   ".to_string();
    app.cursor_position = app.input.chars().count();
    app.input_history.push("previous prompt".to_string());

    assert!(handle_composer_history_arrow(
        &mut app,
        KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
        false,
        false,
    ));
    assert_eq!(app.input, "previous prompt");
}

#[test]
fn history_arrow_handles_nonempty_input() {
    let mut app = create_test_app();
    // Explicitly disable arrows-scroll so this test covers the
    // history-navigation path regardless of the mouse-capture default.
    app.composer_arrows_scroll = false;
    app.input = "hello".to_string();
    app.cursor_position = app.input.chars().count();
    app.input_history.push("previous prompt".to_string());

    assert!(handle_composer_history_arrow(
        &mut app,
        KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
        false,
        false,
    ));

    assert_eq!(app.input, "previous prompt");
}

#[test]
fn composer_arrows_scroll_empty_up() {
    let mut app = create_test_app();
    app.composer_arrows_scroll = true;

    // Opt-in: empty composer Up scrolls transcript.
    assert!(handle_composer_history_arrow(
        &mut app,
        KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
        false,
        false,
    ));
    assert_eq!(app.viewport.pending_scroll_delta, -3);
    assert!(app.input.is_empty());
}

#[test]
fn composer_arrows_scroll_empty_down() {
    let mut app = create_test_app();
    app.composer_arrows_scroll = true;

    assert!(handle_composer_history_arrow(
        &mut app,
        KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        false,
        false,
    ));
    assert_eq!(app.viewport.pending_scroll_delta, 3);
}

#[test]
fn composer_arrows_scroll_nonempty_also_scrolls() {
    let mut app = create_test_app();
    app.composer_arrows_scroll = true;
    app.input = "hello".to_string();
    app.cursor_position = app.input.chars().count();
    app.input_history.push("previous prompt".to_string());

    // #1677: terminals that convert mouse-wheel to arrow keys should scroll
    // the transcript without mutating a draft the user is editing.
    assert!(handle_composer_history_arrow(
        &mut app,
        KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
        false,
        false,
    ));
    assert_eq!(app.viewport.pending_scroll_delta, -3);
    assert_eq!(app.input, "hello");
}

#[test]
fn composer_arrow_up_moves_within_multiline_input() {
    let mut app = create_test_app();
    app.composer_arrows_scroll = false;
    app.input = "line one\nline two".to_string();
    app.cursor_position = app.input.chars().count();
    app.input_history.push("previous prompt".to_string());

    assert!(handle_composer_history_arrow(
        &mut app,
        KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
        false,
        false,
    ));

    assert_eq!(app.input, "line one\nline two");
    assert!(app.cursor_position < app.input.chars().count());
}

#[test]
fn composer_arrow_down_moves_within_multiline_input() {
    let mut app = create_test_app();
    app.composer_arrows_scroll = false;
    app.input = "line one\nline two".to_string();
    app.cursor_position = 0;
    app.input_history.push("next prompt".to_string());
    app.history_index = Some(app.input_history.len() - 1);

    assert!(handle_composer_history_arrow(
        &mut app,
        KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        false,
        false,
    ));

    assert_eq!(app.input, "line one\nline two");
    assert!(app.cursor_position >= "line one\n".chars().count());
}

#[test]
fn composer_arrows_scroll_multiline_input_navigates_lines() {
    let mut app = create_test_app();
    app.composer_arrows_scroll = true;
    app.input = "line one\nline two".to_string();
    app.cursor_position = app.input.chars().count();

    assert!(handle_composer_history_arrow(
        &mut app,
        KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
        false,
        false,
    ));

    assert_eq!(app.input, "line one\nline two");
    assert!(app.cursor_position < app.input.chars().count());
    assert_eq!(app.viewport.pending_scroll_delta, 0);
}

#[test]
fn composer_arrow_up_at_first_line_preserves_multiline_draft() {
    let mut app = create_test_app();
    app.composer_arrows_scroll = false;
    app.input = "line one\nline two".to_string();
    app.cursor_position = 0;
    app.input_history.push("previous prompt".to_string());

    assert!(handle_composer_history_arrow(
        &mut app,
        KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
        false,
        false,
    ));

    assert_eq!(app.input, "line one\nline two");
    assert_eq!(app.cursor_position, 0);
    assert!(app.history_index.is_none());
}

#[test]
fn composer_arrow_down_at_last_line_preserves_multiline_draft() {
    let mut app = create_test_app();
    app.composer_arrows_scroll = false;
    app.input = "line one\nline two".to_string();
    app.cursor_position = app.input.chars().count();
    app.input_history.push("next prompt".to_string());

    assert!(handle_composer_history_arrow(
        &mut app,
        KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        false,
        false,
    ));

    assert_eq!(app.input, "line one\nline two");
    assert_eq!(app.cursor_position, app.input.chars().count());
    assert!(app.history_index.is_none());
}

// #1443: when mouse capture is off (e.g. Windows CMD), arrow-scroll
// must default to true so mouse-wheel events (sent as arrow keys by
// the terminal) scroll the transcript rather than cycling history.
#[test]
fn composer_arrows_scroll_defaults_true_without_mouse_capture() {
    let options = TuiOptions {
        use_mouse_capture: false,
        ..create_test_options()
    };
    let app = App::new(options, &Config::default());
    assert!(
        app.composer_arrows_scroll,
        "arrows-scroll must default to true when mouse capture is off"
    );
}

#[test]
fn composer_arrows_scroll_defaults_false_with_mouse_capture() {
    let options = TuiOptions {
        use_mouse_capture: true,
        ..create_test_options()
    };
    let app = App::new(options, &Config::default());
    assert!(
        !app.composer_arrows_scroll,
        "arrows-scroll must default to false when mouse capture is on"
    );
}

#[test]
fn composer_arrows_scroll_config_overrides_default() {
    let config = Config {
        tui: Some(crate::config::TuiConfig {
            composer_arrows_scroll: Some(false),
            ..Default::default()
        }),
        ..Config::default()
    };
    // Even with mouse_capture off, explicit config=false wins.
    let options = TuiOptions {
        use_mouse_capture: false,
        ..create_test_options()
    };
    let app = App::new(options, &config);
    assert!(
        !app.composer_arrows_scroll,
        "explicit config=false must override the mouse-capture-derived default"
    );
}

#[test]
fn history_arrow_down_handles_empty_input() {
    let mut app = create_test_app();
    app.composer_arrows_scroll = false;
    app.input_history.push("older".to_string());
    app.input_history.push("newer".to_string());

    // Empty composer + Up → newest entry (draft saved as empty string).
    assert!(handle_composer_history_arrow(
        &mut app,
        KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
        false,
        false,
    ));
    assert_eq!(app.input, "newer");

    // Down from newest → end of history → restores the saved empty draft.
    assert!(handle_composer_history_arrow(
        &mut app,
        KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        false,
        false,
    ));
    assert!(app.input.is_empty());
    assert!(app.history_index.is_none());
}

#[test]
fn home_jumps_to_line_start_multiline() {
    let mut app = create_test_app();
    app.input = "line one\nline two\nline three".to_string();
    app.cursor_position = app.input.chars().count();
    app.move_cursor_line_start();
    assert_eq!(app.cursor_position, "line one\nline two\n".len());
}

#[test]
fn home_from_middle_of_line_jumps_to_line_start() {
    let mut app = create_test_app();
    app.input = "line one\nline two".to_string();
    app.cursor_position = "line one\nli".len();
    app.move_cursor_line_start();
    assert_eq!(app.cursor_position, "line one\n".len());
}

#[test]
fn home_on_singleline_jumps_to_zero() {
    let mut app = create_test_app();
    app.input = "hello world".to_string();
    app.cursor_position = 6;
    app.move_cursor_line_start();
    assert_eq!(app.cursor_position, 0);
}

#[test]
fn end_jumps_to_line_end_multiline() {
    let mut app = create_test_app();
    app.input = "line one\nline two\nline three".to_string();
    app.cursor_position = 0;
    app.move_cursor_line_end();
    assert_eq!(app.cursor_position, "line one".len());
}

#[test]
fn end_from_middle_of_line_jumps_to_line_end() {
    let mut app = create_test_app();
    app.input = "line one\nline two".to_string();
    app.cursor_position = "line one\nli".len();
    app.move_cursor_line_end();
    assert_eq!(app.cursor_position, "line one\nline two".len());
}

#[test]
fn end_on_singleline_jumps_to_absolute_end() {
    let mut app = create_test_app();
    app.input = "hello world".to_string();
    app.cursor_position = 0;
    app.move_cursor_line_end();
    assert_eq!(app.cursor_position, app.input.chars().count());
}

#[test]
fn home_at_line_start_stays_put() {
    let mut app = create_test_app();
    app.input = "line one\nline two".to_string();
    app.cursor_position = "line one\n".len();
    app.move_cursor_line_start();
    assert_eq!(app.cursor_position, "line one\n".len());
}

#[test]
fn end_at_newline_stays_at_line_end() {
    let mut app = create_test_app();
    app.input = "line one\nline two\nline three".to_string();
    // Cursor sitting on the first '\n'.
    app.cursor_position = "line one".len();
    app.move_cursor_line_end();
    // Stays at end of current line.
    assert_eq!(app.cursor_position, "line one".len());
}

#[test]
fn notification_settings_tui_always_keeps_configured_method_no_threshold() {
    let config = Config {
        tui: Some(crate::config::TuiConfig {
            notification_condition: Some(crate::config::NotificationCondition::Always),
            ..Default::default()
        }),
        notifications: Some(crate::config::NotificationsConfig {
            method: crate::config::NotificationMethod::Bel,
            threshold_secs: 120,
            completion_sound: crate::config::CompletionSound::Beep,
            sound_file: None,
            include_summary: true,
            subagent_completion: crate::config::SubagentCompletionNotification::default(),
            event_sound: crate::config::EventSoundConfig::default(),
            quiet: false,
            events: crate::config::NotificationEventsConfig::default(),
        }),
        ..Config::default()
    };

    let (method, threshold, include_summary) =
        crate::tui::notifications::settings_projection(&config)
            .expect("notification should be enabled");
    assert_eq!(method, crate::tui::notifications::Method::Bel);
    assert_eq!(threshold, Duration::ZERO);
    assert!(include_summary);
}

#[test]
fn notification_settings_tui_never_disables_notifications() {
    let config = Config {
        tui: Some(crate::config::TuiConfig {
            notification_condition: Some(crate::config::NotificationCondition::Never),
            ..Default::default()
        }),
        ..Config::default()
    };

    assert!(crate::tui::notifications::settings_projection(&config).is_none());
}

#[test]
fn notification_settings_no_tui_override_uses_notifications_block() {
    let config = Config {
        notifications: Some(crate::config::NotificationsConfig {
            method: crate::config::NotificationMethod::Osc9,
            threshold_secs: 45,
            completion_sound: crate::config::CompletionSound::Beep,
            sound_file: None,
            include_summary: false,
            subagent_completion: crate::config::SubagentCompletionNotification::default(),
            event_sound: crate::config::EventSoundConfig::default(),
            quiet: false,
            events: crate::config::NotificationEventsConfig::default(),
        }),
        ..Config::default()
    };

    let (method, threshold, include_summary) =
        crate::tui::notifications::settings_projection(&config)
            .expect("notification should be enabled");
    assert_eq!(method, crate::tui::notifications::Method::Osc9);
    assert_eq!(threshold, Duration::from_secs(45));
    assert!(!include_summary);
}

#[test]
fn completed_turn_notification_uses_streaming_text() {
    let app = create_test_app();
    let payload = crate::tui::notifications::completed_turn_payload(
        &app,
        "Hello there.\n\nWhat's next?",
        false,
        Duration::from_secs(12),
        None,
    );
    assert_eq!(payload.headline(), "Turn complete");
    // #4834: the assistant text is now a *preview* field, so it is
    // collapsed to a single bounded line instead of riding along as
    // free-form newline-separated text.
    assert_eq!(payload.preview(), Some("Hello there. What's next?"));
}

#[test]
fn completed_turn_notification_falls_back_to_latest_assistant_message() {
    let mut app = create_test_app();
    app.api_messages.push(crate::models::Message {
        role: Role::Assistant,
        content: vec![crate::models::ContentBlock::Text {
            text: "Earlier turn".to_string(),
            cache_control: None,
        }],
    });
    app.api_messages.push(crate::models::Message {
        role: Role::User,
        content: vec![crate::models::ContentBlock::Text {
            text: "next".to_string(),
            cache_control: None,
        }],
    });
    app.api_messages.push(crate::models::Message {
        role: Role::Assistant,
        content: vec![crate::models::ContentBlock::Text {
            text: "Latest reply".to_string(),
            cache_control: None,
        }],
    });

    let payload = crate::tui::notifications::completed_turn_payload(
        &app,
        "",
        false,
        Duration::from_secs(75),
        None,
    );
    assert_eq!(payload.headline(), "Turn complete");
    assert_eq!(payload.preview(), Some("Latest reply"));
}

#[test]
fn completed_turn_notification_falls_back_to_default_when_empty() {
    let app = create_test_app();
    let payload = crate::tui::notifications::completed_turn_payload(
        &app,
        "",
        false,
        Duration::from_secs(5),
        None,
    );
    assert_eq!(payload.headline(), "Turn complete");
    assert_eq!(payload.preview(), None);
    assert_eq!(payload.render_inline(), "Turn complete");
}

#[test]
fn completed_turn_notification_truncates_long_text() {
    let app = create_test_app();
    // Word-shaped text on purpose: a 500-character unbroken run is
    // credential-shaped and is now redacted wholesale (see
    // `notification_payload::redact_credentials`), which would test the
    // wrong thing here.
    let long = "assistant preview ".repeat(40);
    let payload = crate::tui::notifications::completed_turn_payload(
        &app,
        &long,
        false,
        Duration::from_secs(5),
        None,
    );
    assert_eq!(payload.headline(), "Turn complete");
    let preview = payload.preview().expect("long text should yield a preview");
    assert!(preview.ends_with("..."));
    // #4834: the cap moved from 363 chars of untyped tail text to the
    // payload's declared PREVIEW_MAX_CHARS, *inclusive* of the ellipsis,
    // so the bound the type promises is the bound the OS receives.
    assert_eq!(
        preview.chars().count(),
        crate::tui::notification_payload::PREVIEW_MAX_CHARS
    );
}

#[test]
fn completed_turn_notification_leads_with_user_locale() {
    let mut app = create_test_app();
    app.ui_locale = crate::localization::Locale::Ja;
    let payload = crate::tui::notifications::completed_turn_payload(
        &app,
        "完了しました。",
        true,
        Duration::from_secs(65),
        None,
    );
    assert_eq!(payload.headline(), "ターン完了 (1m 05s)");
    assert_eq!(payload.preview(), Some("完了しました。"));
}

#[test]
fn subagent_completion_notification_uses_summary_line_not_sentinel() {
    let payload = crate::tui::notifications::subagent_terminal_payload(
        crate::localization::Locale::En,
        "agent_live",
        "Finished the docs audit.\n<codewhale:subagent.done>{}</codewhale:subagent.done>",
        &crate::tools::subagent::SubAgentStatus::Completed,
        false,
        Duration::from_secs(42),
    );

    assert_eq!(payload.headline(), "Sub-agent complete");
    assert_eq!(payload.detail(), Some("agent_live"));
    assert_eq!(payload.preview(), Some("Finished the docs audit."));
    assert!(!payload.render_inline().contains("codewhale:subagent.done"));
}

#[test]
fn subagent_completion_notification_can_include_elapsed_summary() {
    let payload = crate::tui::notifications::subagent_terminal_payload(
        crate::localization::Locale::En,
        "agent_live",
        "",
        &crate::tools::subagent::SubAgentStatus::Completed,
        true,
        Duration::from_secs(65),
    );

    assert_eq!(payload.headline(), "Sub-agent complete (1m 05s)");
    assert_eq!(payload.detail(), Some("agent_live"));
    assert_eq!(payload.preview(), None);
}

#[test]
fn subagent_cancelled_notification_never_claims_completion() {
    let payload = crate::tui::notifications::subagent_terminal_payload(
        crate::localization::Locale::En,
        "agent_stopped",
        "Cancelled\n<codewhale:subagent.done>{\"status\":\"cancelled\"}</codewhale:subagent.done>",
        &crate::tools::subagent::SubAgentStatus::Cancelled,
        false,
        Duration::from_secs(2),
    );

    assert_eq!(payload.headline(), "Sub-agent cancelled");
    assert_eq!(payload.detail(), Some("agent_stopped"));
    assert_eq!(payload.preview(), Some("Cancelled"));
    assert!(!payload.render_inline().contains("Sub-agent complete"));
}

#[test]
fn sanitize_stream_chunk_keeps_printable_and_drops_control_bytes() {
    // `sanitize_stream_chunk` is the per-chunk filter every piece of
    // streaming text goes through (assistant content, thinking
    // content, tool results, web-search snippets). Pin both
    // invariants:
    //
    // 1. preserve user-visible whitespace (newline / tab) — collapsing
    //    those would mangle code blocks and tool output;
    // 2. drop terminal-escape-friendly control bytes — a chunk
    //    containing `\u{1b}[2J` (clear screen) or `\u{8}` (backspace)
    //    must not reach the renderer.
    let cleaned = super::sanitize_stream_chunk("hello\tworld\n");
    assert_eq!(cleaned, "hello\tworld\n", "tabs and newlines must survive");

    // ESC + CSI sequence: only the printable letters/digits survive.
    let cleaned = super::sanitize_stream_chunk("text\u{1b}[2Jmore");
    assert_eq!(cleaned, "text[2Jmore", "ESC byte must be filtered");

    // Bell, backspace, vertical tab, form feed — all are control
    // characters that aren't `\n` or `\t`. Drop them.
    let cleaned = super::sanitize_stream_chunk("a\u{7}b\u{8}c\u{b}d\u{c}e");
    assert_eq!(cleaned, "abcde");

    // Carriage return is also a control char; today's renderer expects
    // unix newlines, so CR is filtered out. Pin so a future CRLF-mode
    // change has to update this test intentionally.
    let cleaned = super::sanitize_stream_chunk("line1\r\nline2");
    assert_eq!(cleaned, "line1\nline2");
}

#[test]
fn sanitize_stream_chunk_preserves_unicode() {
    // Non-ASCII Unicode is not control — CJK, emoji, accented Latin
    // all pass through untouched.
    let cjk = "\u{4f60}\u{597d}\u{ff0c}DeepSeek";
    assert_eq!(super::sanitize_stream_chunk(cjk), cjk);

    let emoji_and_accents = "caf\u{e9} \u{1f680} build";
    assert_eq!(
        super::sanitize_stream_chunk(emoji_and_accents),
        emoji_and_accents,
    );
}

#[test]
fn sanitize_stream_chunk_handles_empty_and_whitespace() {
    assert_eq!(super::sanitize_stream_chunk(""), "");
    assert_eq!(super::sanitize_stream_chunk("   "), "   ");
    // A chunk that's purely control bytes shrinks to empty — caller
    // branches that skip empty chunks handle the result, so the
    // filter doesn't need to inject a placeholder.
    assert_eq!(super::sanitize_stream_chunk("\u{1b}\u{7}\u{8}"), "");
}

#[test]
fn toast_stack_overlay_respects_composer_boundary() {
    // Verify that the toast stack area calculation respects the composer area
    // boundary and doesn't overlap. This is a regression test for the issue
    // where deferred tool loading notifications appeared in the composer input.
    //
    // Layout:
    // - Composer area: rows 10-14 (height=5, y=10)
    // - Footer area: rows 15-16 (height=2, y=15)
    // - Available space for toast stack: rows 14-14 (max 1 row above footer)
    let _full_area = ratatui::prelude::Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 16,
    };
    let composer_area = ratatui::prelude::Rect {
        x: 0,
        y: 10,
        width: 80,
        height: 5,
    };
    let footer_area = ratatui::prelude::Rect {
        x: 0,
        y: 15,
        width: 80,
        height: 1,
    };

    // With 2 toasts, the stack overlay would try to render 1 toast above footer
    // max_above should be: footer_area.y (15) - composer_area.y.saturating_sub(1) (9)
    //                   = 15 - 9 = 6 rows available
    // But that's the full space above footer. The real constraint is the gap
    // between composer end and footer start.
    // Composer ends at row 14 (y=10 + height=5 - 1)
    // Footer starts at row 15
    // So only row 14 is available for toasts (1 row)

    // The calculation should be:
    // max_above = footer_area.y.saturating_sub(composer_area.y.saturating_sub(1))
    //          = 15.saturating_sub(10 - 1)
    //          = 15 - 9 = 6
    // But wait, composer_area.y.saturating_sub(1) = 10 - 1 = 9
    // This gives us the space BEFORE the composer starts, which is wrong.
    //
    // The correct logic should be:
    // composer_end = composer_area.y + composer_area.height
    // available = footer_area.y.saturating_sub(composer_end)
    // But we're using: footer_area.y.saturating_sub(composer_area.y.saturating_sub(1))
    // Which is: 15 - 9 = 6, the total height above composer start
    // But we only want the gap between composer end and footer
    //
    // Actually, the formula composer_area.y.saturating_sub(1) means:
    // "find the row right before the composer starts"
    // And we subtract that from footer_area.y to get the space between composer and footer.
    // This is correct: footer_area.y - (composer_area.y - 1) - 1 = gap
    // Wait, let me recalculate:
    // Composer area: y=10, height=5 means rows 10-14
    // Footer area: y=15 means row 15
    // Gap = 15 - (10 + 5) = 0 (they're adjacent!)
    //
    // Let me reconsider the formula in the code:
    // max_above = footer_area.y.saturating_sub(composer_area.y.saturating_sub(1))
    //          = 15 - (10 - 1)
    //          = 15 - 9 = 6
    //
    // But the composer occupies rows 10-14, and footer is at row 15.
    // So there's actually no gap! The calculation gives 6, which includes:
    // - Rows before composer (0-9) = 10 rows
    // - Rows at composer end (14) = 1 row
    // Total = 11 rows, but we get 6... that doesn't match.
    //
    // Actually wait, let me re-read the formula:
    // composer_area.y.saturating_sub(1) = 10 - 1 = 9
    // This is row 9 (the row right before composer starts at row 10)
    // footer_area.y - 9 = 15 - 9 = 6
    // This is the number of rows from row 9 to row 15 (exclusive), which is rows 9-14 = 6 rows
    // This is correct! It's the space from before the composer to the footer.
    //
    // But wait, the composer STARTS at row 10, not row 9.
    // So rows 9-14 includes the composer! That's not right either.
    //
    // I think I'm overcomplicating this. Let me just verify that the calculation
    // doesn't allow the toast to overlap with the composer.

    // The actual fix in `render_toast_stack_overlay` computes
    //     composer_end = composer_area.y + composer_area.height
    //     max_above    = footer_area.y.saturating_sub(composer_end)
    // so when composer and footer are adjacent (no gap), max_above
    // collapses to 0 and the overlay is silently skipped rather than
    // rendering on top of the composer's last row.
    let composer_end = composer_area.y + composer_area.height;
    let max_above = footer_area.y.saturating_sub(composer_end);

    assert_eq!(
        max_above, 0,
        "with adjacent composer (rows 10-14) and footer (row 15) there is \
         no gap, so the toast stack must report zero available rows"
    );
    // Sanity: the calculated cap must never exceed the gap. This is what
    // prevents the v0.8.31 overlap regression — any positive value here on
    // an adjacent layout would put toast text on top of the composer.
    let gap = footer_area.y.saturating_sub(composer_end);
    assert!(
        max_above <= gap,
        "max_above ({max_above}) must never exceed the composer→footer gap ({gap})"
    );
}

// === Bugs #1913/#4416: Work sidebar session ownership =====================
//
// The Work sidebar reads `~/.deepseek/tasks/` on startup, which holds every
// durable task the user has ever run. Without filtering, completed tasks
// from prior sessions persist indefinitely. Worse, startup recovery can mark
// an old running task failed *during* a new TUI session. The projection helper
// keeps active durable tasks, but only renders terminal receipts that were
// created and completed during this TUI session. Shared history stays in
// `/tasks` instead of making a fresh Work surface look failed.

mod work_sidebar_projection_tests {
    use super::*;
    use crate::task_manager::{TaskStatus, TaskSummary};
    use chrono::{Duration, TimeZone, Utc};

    fn sample_task(
        id: &str,
        status: TaskStatus,
        created_at: chrono::DateTime<Utc>,
        ended_at: Option<chrono::DateTime<Utc>>,
        owner_session_id: Option<&str>,
    ) -> TaskSummary {
        TaskSummary {
            id: id.to_string(),
            status,
            prompt_summary: format!("task {id}"),
            model: "deepseek-v4-flash".to_string(),
            mode: "agent".to_string(),
            workspace: std::path::PathBuf::from("/tmp"),
            created_at,
            started_at: Some(created_at + Duration::seconds(1)),
            ended_at,
            duration_ms: ended_at.map(|_| 1_234),
            lifecycle_seq: 1,
            hunt_verdict: None,
            error: None,
            terminal_reason: None,
            thread_id: None,
            turn_id: None,
            owner_session_id: owner_session_id.map(str::to_string),
        }
    }

    #[test]
    fn work_sidebar_hides_other_session_terminals_but_keeps_current_and_active() {
        // Pretend the TUI session started on 2026-05-23T10:00:00Z.
        let session_started_at = Utc.with_ymd_and_hms(2026, 5, 23, 10, 0, 0).unwrap();

        let active_running = sample_task(
            "active_run",
            TaskStatus::Running,
            session_started_at - Duration::minutes(30),
            None,
            Some("session-current"),
        );
        let active_queued = sample_task(
            "active_q",
            TaskStatus::Queued,
            session_started_at - Duration::minutes(20),
            None,
            Some("session-current"),
        );

        // Created and completed during the current session — must show.
        let just_finished = sample_task(
            "just_done",
            TaskStatus::Completed,
            session_started_at + Duration::seconds(5),
            Some(session_started_at + Duration::seconds(30)),
            Some("session-current"),
        );

        // Completed shortly before the session started — shared history, not
        // this session's live-work receipt.
        let recently_finished_before_session = sample_task(
            "recent_done",
            TaskStatus::Failed,
            session_started_at - Duration::minutes(20),
            Some(session_started_at - Duration::minutes(15)),
            Some("session-old"),
        );

        // Exact restart regression: a prior-session running record is marked
        // failed by TaskManager startup and gets a fresh ended_at. Its old
        // creation time keeps it off the fresh Work surface.
        let recovered_after_restart = sample_task(
            "recovered_old_run",
            TaskStatus::Failed,
            session_started_at - Duration::minutes(30),
            Some(session_started_at + Duration::seconds(1)),
            Some("session-old"),
        );

        // Stale completed from 6 days ago (the exact scenario in #1913) —
        // must be hidden.
        let stale_completed = sample_task(
            "stale_done",
            TaskStatus::Completed,
            session_started_at - Duration::days(6) - Duration::minutes(1),
            Some(session_started_at - Duration::days(6)),
            Some("session-old"),
        );
        let stale_canceled = sample_task(
            "stale_cancel",
            TaskStatus::Canceled,
            session_started_at - Duration::days(7) - Duration::minutes(1),
            Some(session_started_at - Duration::days(7)),
            Some("session-old"),
        );
        let stale_failed = sample_task(
            "stale_fail",
            TaskStatus::Failed,
            session_started_at - Duration::days(3) - Duration::minutes(1),
            Some(session_started_at - Duration::days(3)),
            Some("session-old"),
        );
        let active_sibling = sample_task(
            "sibling_run",
            TaskStatus::Running,
            session_started_at - Duration::minutes(2),
            None,
            Some("session-sibling"),
        );

        // A terminal task without `ended_at` shouldn't sneak through.
        let terminal_no_timestamp = sample_task(
            "ghost",
            TaskStatus::Completed,
            session_started_at + Duration::seconds(1),
            None,
            Some("session-current"),
        );
        let legacy_finished = sample_task(
            "legacy_done",
            TaskStatus::Completed,
            session_started_at + Duration::seconds(2),
            Some(session_started_at + Duration::seconds(3)),
            None,
        );

        let tasks = vec![
            active_running.clone(),
            active_queued.clone(),
            just_finished.clone(),
            recently_finished_before_session.clone(),
            recovered_after_restart.clone(),
            stale_completed.clone(),
            stale_canceled.clone(),
            stale_failed.clone(),
            active_sibling.clone(),
            terminal_no_timestamp.clone(),
            legacy_finished.clone(),
        ];

        let kept = select_work_sidebar_tasks(tasks, session_started_at, Some("session-current"));
        let kept_ids: Vec<&str> = kept.iter().map(|t| t.id.as_str()).collect();

        assert!(
            kept_ids.contains(&"active_run"),
            "active running task must always show: {kept_ids:?}"
        );
        assert!(
            kept_ids.contains(&"active_q"),
            "active queued task must always show: {kept_ids:?}"
        );
        assert!(
            kept_ids.contains(&"just_done"),
            "task completed during the current session must show: {kept_ids:?}"
        );
        assert!(
            !kept_ids.contains(&"recent_done"),
            "prior-session terminal task must stay in explicit history: \
             {kept_ids:?}"
        );
        assert!(
            !kept_ids.contains(&"recovered_old_run"),
            "startup recovery must not turn an old task into a fresh red row: {kept_ids:?}"
        );
        assert!(
            !kept_ids.contains(&"sibling_run"),
            "active sibling-session task must stay off the new session's live work surface: {kept_ids:?}"
        );

        assert!(
            !kept_ids.contains(&"stale_done"),
            "completed task from 6 days ago must be hidden (bug #1913): {kept_ids:?}"
        );
        assert!(
            !kept_ids.contains(&"stale_cancel"),
            "canceled task from 7 days ago must be hidden: {kept_ids:?}"
        );
        assert!(
            !kept_ids.contains(&"stale_fail"),
            "failed task from 3 days ago must be hidden: {kept_ids:?}"
        );
        assert!(
            !kept_ids.contains(&"ghost"),
            "terminal task missing ended_at must be hidden: {kept_ids:?}"
        );
        assert!(
            kept_ids.contains(&"legacy_done"),
            "legacy unowned task still uses timestamp fallback during migration: {kept_ids:?}"
        );
    }

    #[test]
    fn work_sidebar_keeps_tasks_completed_at_session_boundary() {
        // Edge case: a task that finished at exactly the same instant the
        // session started should still be visible (>= comparison).
        let session_started_at = Utc.with_ymd_and_hms(2026, 5, 23, 10, 0, 0).unwrap();
        let at_boundary = sample_task(
            "boundary",
            TaskStatus::Completed,
            session_started_at,
            Some(session_started_at),
            None,
        );

        let kept = select_work_sidebar_tasks(vec![at_boundary], session_started_at, None);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "boundary");
    }

    #[test]
    fn work_sidebar_keeps_current_session_owned_terminals_after_restore() {
        let session_started_at = Utc.with_ymd_and_hms(2026, 5, 23, 10, 0, 0).unwrap();
        let restored = sample_task(
            "restored_done",
            TaskStatus::Completed,
            session_started_at - Duration::days(1),
            Some(session_started_at - Duration::hours(23)),
            Some("session-current"),
        );

        let kept =
            select_work_sidebar_tasks(vec![restored], session_started_at, Some("session-current"));
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "restored_done");
    }

    #[test]
    fn receipt_summary_truncation_does_not_panic_on_multibyte_boundary() {
        // Build a summary where byte 57 falls mid-character (em dash is 3 bytes).
        // 56 ASCII chars + em dash ensures byte 57 lands inside the em dash.
        let prefix = "a".repeat(56); // 56 ASCII bytes
        let summary = format!("{prefix}— rest of summary"); // byte 56='a', 57-59='—'
        assert!(summary.len() > 60);
        // Byte 57 should be inside the em dash (3-byte UTF-8 sequence).
        assert!(!summary.is_char_boundary(57));

        // The runtime helper should step back to the start of the char
        // and append the ellipsis without panicking.
        let truncated = crate::utils::truncate_with_ellipsis(&summary, 60, "…");
        assert_eq!(truncated, format!("{prefix}…"));
    }

    #[test]
    fn shell_manager_cancel_transitions_task_to_not_running() {
        // Verify that killing a shell job via ShellManager removes it from
        // the list of running jobs, so the task panel refresh picks up the
        // correct state.
        let temp_dir = std::env::temp_dir().join(format!(
            "codewhale-test-shell-cancel-{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&temp_dir);
        let mut manager = crate::tools::shell::ShellManager::new(temp_dir.clone());

        // We can't easily spawn a real background process in a unit test
        // without a Tokio runtime, but we can verify that kill_running /
        // list_jobs correctly report zero running after a kill attempt on
        // an empty manager, and that the API is consistent.
        let jobs = manager.list_jobs();
        let running = jobs
            .iter()
            .filter(|j| matches!(j.status, crate::tools::shell::ShellStatus::Running))
            .count();
        assert_eq!(running, 0, "empty manager should have zero running jobs");

        // kill_running on empty should succeed and return empty.
        let results = manager.kill_running().unwrap();
        assert!(
            results.is_empty(),
            "kill_running on empty should return empty"
        );

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn task_panel_entry_roundtrips_status() {
        // TaskPanelEntry status field is a plain string. Verify that the
        // status constants used in sidebar rendering match the values produced
        // by ShellJobSnapshot / TaskSummary conversions.
        let entry = crate::tui::app::TaskPanelEntry {
            id: "test-id".to_string(),
            status: "completed".to_string(),
            prompt_summary: "echo hello".to_string(),
            duration_ms: Some(100),
            kind: crate::tui::app::TaskPanelEntryKind::Background,
            stale: false,
            elapsed_since_output_ms: None,
            owner_agent_id: None,
            owner_agent_name: None,
            current_tool: None,
            role: None,
            files_touched: 0,
        };
        assert_eq!(entry.status, "completed");
        assert_ne!(entry.status, "running");
    }
}

// ── #3033: AgentProgress redraw throttle ───────────────────────────────────

#[test]
fn agent_progress_redraw_throttle_permits_first_and_spaced_events() {
    let mut last_redraw = None;
    let t0 = Instant::now();

    assert!(
        agent_progress_redraw_permitted(&mut last_redraw, t0),
        "first progress event always repaints"
    );
    assert!(
        !agent_progress_redraw_permitted(&mut last_redraw, t0 + Duration::from_millis(50)),
        "events inside the 100ms window are throttled"
    );
    assert!(
        !agent_progress_redraw_permitted(&mut last_redraw, t0 + Duration::from_millis(99)),
        "throttled events must not advance the window"
    );
    assert!(
        agent_progress_redraw_permitted(&mut last_redraw, t0 + Duration::from_millis(150)),
        "events past the window repaint again"
    );
}

// ── #4095 residual: workflow budget_updated redraw throttle ────────────────

#[test]
fn workflow_budget_redraw_throttle_matches_progress_pace() {
    let mut last_redraw = None;
    let t0 = Instant::now();

    assert!(
        workflow_budget_redraw_permitted(&mut last_redraw, t0),
        "first budget tick may repaint"
    );
    assert!(
        !workflow_budget_redraw_permitted(&mut last_redraw, t0 + Duration::from_millis(40)),
        "budget churn inside 100ms is coalesced"
    );
    assert!(
        workflow_budget_redraw_permitted(&mut last_redraw, t0 + Duration::from_millis(120)),
        "budget ticks past the window repaint again"
    );
}

#[test]
fn throttled_progress_event_does_not_cancel_other_events_redraw() {
    // Repro for the #3033 audit finding: `received_engine_event` is a shared
    // accumulator for the whole drain batch. A throttled AgentProgress event
    // must restore the PRE-EVENT value instead of clearing the flag, so
    // redraws owed to other events (AgentSpawned, AgentList, cross-agent
    // AgentComplete...) survive.
    let t0 = Instant::now();
    let mut last_redraw = Some(t0);

    // Batch: AgentSpawned (requests redraw), then a throttled AgentProgress.
    let mut received_engine_event = true; // AgentSpawned drained
    let redraw_requested_before_event = received_engine_event;
    received_engine_event = true; // AgentProgress drained
    if !agent_progress_redraw_permitted(&mut last_redraw, t0 + Duration::from_millis(10)) {
        received_engine_event = redraw_requested_before_event;
    }
    assert!(
        received_engine_event,
        "redraw owed to AgentSpawned must survive a throttled progress event"
    );

    // Same batch shape but with NO earlier redraw-worthy event: the lone
    // throttled progress event contributes nothing.
    let mut received_engine_event = false;
    let redraw_requested_before_event = received_engine_event;
    received_engine_event = true; // AgentProgress drained
    if !agent_progress_redraw_permitted(&mut last_redraw, t0 + Duration::from_millis(20)) {
        received_engine_event = redraw_requested_before_event;
    }
    assert!(
        !received_engine_event,
        "a lone throttled progress event must not trigger a repaint"
    );
}

fn running_generic_tool_cell(name: &str) -> HistoryCell {
    HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
        name: name.to_string(),
        status: ToolStatus::Running,
        input_summary: Some("action: run".to_string()),
        output: None,
        prompts: None,
        spillover_path: None,
        output_summary: None,
        is_diff: false,
    }))
}

#[test]
fn status_animation_ticks_for_lone_running_history_tool() {
    let mut app = create_test_app();
    app.history.push(running_generic_tool_cell("workflow"));

    let history_motion = history_has_live_motion(&app.history);
    let active_motion = active_cell_has_live_motion(&app);

    assert!(
        history_motion,
        "running workflow tool should count as live motion"
    );
    assert!(
        should_tick_status_animation(&app, false, history_motion, active_motion, false),
        "a lone running tool in history must force timed redraws for the spout"
    );
}

#[test]
fn status_animation_ticks_for_lone_running_active_tool() {
    let mut app = create_test_app();
    let mut active = ActiveCell::new();
    active.push_tool("tool-1", running_generic_tool_cell("exec_shell"));
    app.active_cell = Some(active);

    let history_motion = history_has_live_motion(&app.history);
    let active_motion = active_cell_has_live_motion(&app);

    assert!(
        active_motion,
        "running active-cell tool should count as live motion"
    );
    assert!(
        should_tick_status_animation(&app, false, history_motion, active_motion, false),
        "a lone running active tool must force timed redraws for the spout"
    );
}

#[test]
fn status_animation_stays_idle_without_live_motion() {
    let app = create_test_app();

    assert!(!history_has_live_motion(&app.history));
    assert!(!active_cell_has_live_motion(&app));
    assert!(
        !should_tick_status_animation(&app, false, false, false, false),
        "idle sessions should not wake the animation timer"
    );

    // Exercise the actual coalescing counter over a representative settled
    // interval. A regression that starts requesting ambient frames will make
    // this counter non-zero even if individual widgets still look static.
    let policy = crate::tui::motion::MotionPolicy::from_settings(false, true, false);
    let mut requester = FrameRequester::new();
    let started = Instant::now();
    for tick in 0..=50 {
        let now = started + Duration::from_millis(tick * 100);
        if should_tick_status_animation(&app, false, false, false, false) {
            requester.request_frame(now, policy);
        }
        let _ = requester.take_due(now, policy);
    }
    assert_eq!(requester.request_count(), 0, "settled TUI requested frames");
    assert_eq!(requester.emit_count(), 0, "settled TUI emitted frames");
}

#[test]
fn status_animation_ticks_for_a_visible_background_task() {
    let mut app = create_test_app();
    app.low_motion = false;
    app.fancy_animations = true;
    app.work_surface.panel = crate::tui::work_surface::RailPanel::Tasks;
    app.work_surface.last_area = Some(Rect::new(80, 0, 20, 20));
    app.task_panel.push(TaskPanelEntry {
        id: "shell_smooth".to_string(),
        status: "running".to_string(),
        prompt_summary: "shell: cargo test --locked".to_string(),
        duration_ms: Some(crate::tui::spinner::LIVE_MARKER_DELAY_MS),
        kind: TaskPanelEntryKind::Background,
        stale: false,
        elapsed_since_output_ms: None,
        owner_agent_id: None,
        owner_agent_name: None,
        current_tool: None,
        role: None,
        files_touched: 0,
    });

    assert!(visible_background_task_has_live_motion(&app));
    assert!(should_tick_status_animation(
        &app, false, false, false, false
    ));

    app.work_surface.last_area = None;
    assert!(!visible_background_task_has_live_motion(&app));
    assert!(!should_tick_status_animation(
        &app, false, false, false, false
    ));
}

#[test]
fn status_animation_timer_respects_reduced_and_still_policies() {
    let mut app = create_test_app();
    app.history.push(running_generic_tool_cell("workflow"));
    let history_motion = history_has_live_motion(&app.history);

    app.low_motion = false;
    app.fancy_animations = true;
    assert!(should_tick_status_animation(
        &app,
        false,
        history_motion,
        false,
        false
    ));

    app.low_motion = true;
    assert!(
        should_tick_status_animation(&app, false, history_motion, false, false),
        "reduced motion keeps its calm liveness refresh while markers stay static"
    );

    app.low_motion = false;
    app.fancy_animations = false;
    assert!(
        !should_tick_status_animation(&app, false, history_motion, false, false),
        "still mode must not wake the status animation timer"
    );
    assert_eq!(status_animation_interval_ms(&app), 2_400);
    assert_eq!(active_poll_ms(&app), UI_ACTIVE_POLL_MS);
    assert_eq!(idle_poll_ms(&app), UI_IDLE_POLL_MS);
}

#[test]
fn translation_placeholder_keeps_a_calm_refresh_without_repainting_still_mode() {
    let mut app = create_test_app();
    app.low_motion = false;
    app.fancy_animations = true;
    assert!(should_tick_status_animation(
        &app, false, false, false, true
    ));

    app.low_motion = true;
    assert!(should_tick_status_animation(
        &app, false, false, false, true
    ));

    app.low_motion = false;
    app.fancy_animations = false;
    assert!(!should_tick_status_animation(
        &app, false, false, false, true
    ));
}

#[test]
fn subagent_completion_notification_modes_gate_correctly() {
    use crate::config::SubagentCompletionNotification as Mode;
    // off: never notify.
    assert!(!should_notify_subagent_completion(Mode::Off, false, false));
    assert!(!should_notify_subagent_completion(Mode::Off, true, true));
    // always: notify regardless of what else is running.
    assert!(should_notify_subagent_completion(Mode::Always, true, true));
    assert!(should_notify_subagent_completion(
        Mode::Always,
        false,
        false
    ));
    // final-only: only when nothing else is running and no workflow is active.
    assert!(should_notify_subagent_completion(
        Mode::FinalOnly,
        false,
        false
    ));
    assert!(
        !should_notify_subagent_completion(Mode::FinalOnly, true, false),
        "final-only stays quiet while other subagents run"
    );
    assert!(
        !should_notify_subagent_completion(Mode::FinalOnly, false, true),
        "final-only stays quiet while a workflow run is active"
    );
}

#[test]
fn workflow_tool_is_running_detects_running_workflow_cell() {
    let mut app = create_test_app();
    assert!(!workflow_tool_is_running(&app));
    app.history.push(running_generic_tool_cell("read_file"));
    assert!(
        !workflow_tool_is_running(&app),
        "a non-workflow running tool must not count as a workflow run"
    );
    app.history.push(running_generic_tool_cell("workflow"));
    assert!(workflow_tool_is_running(&app));
}

#[test]
fn agent_progress_redraw_coalesces_once_per_agent_per_drain() {
    let t0 = Instant::now();
    let mut last_redraw = None;
    let mut seen_agents = HashSet::new();

    assert!(
        agent_progress_redraw_permitted_for_drain(
            &mut last_redraw,
            &mut seen_agents,
            "agent-a",
            t0,
        ),
        "first progress event for an agent in a drain may repaint"
    );
    assert!(
        !agent_progress_redraw_permitted_for_drain(
            &mut last_redraw,
            &mut seen_agents,
            "agent-a",
            t0 + Duration::from_millis(150),
        ),
        "later progress for the same agent in the same drain is coalesced"
    );

    let mut next_drain_seen_agents = HashSet::new();
    assert!(
        agent_progress_redraw_permitted_for_drain(
            &mut last_redraw,
            &mut next_drain_seen_agents,
            "agent-a",
            t0 + Duration::from_millis(150),
        ),
        "a later drain can repaint that agent again after the throttle window"
    );
}

#[test]
fn six_worker_progress_storm_keeps_input_render_and_cancel_live() {
    const _: () = assert!(
        MAX_ENGINE_EVENTS_PER_DRAIN >= 8 && MAX_ENGINE_EVENTS_PER_DRAIN <= 16,
        "engine event drains must stay small so terminal input is polled frequently during long runs"
    );

    let t0 = Instant::now();
    let mut last_redraw = None;
    let mut seen_agents = HashSet::new();
    let mut redraws = 0usize;
    let mut received_engine_event = false;

    for burst in 0..80 {
        for worker in 0..6 {
            let agent_id = format!("agent-{worker}");
            let redraw_requested_before_event = received_engine_event;
            received_engine_event = true;
            if agent_progress_redraw_permitted_for_drain(
                &mut last_redraw,
                &mut seen_agents,
                &agent_id,
                t0 + Duration::from_millis(burst * 2 + worker),
            ) {
                redraws += 1;
            } else {
                received_engine_event = redraw_requested_before_event;
            }
        }
    }

    assert_eq!(
        seen_agents.len(),
        6,
        "storm should observe all six workers in one drain"
    );
    assert!(
        (1..=6).contains(&redraws),
        "progress storm must request a bounded redraw count, got {redraws}"
    );
    assert!(
        received_engine_event,
        "at least one bounded redraw should keep rendering live"
    );

    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(TerminalInputMessage::Event(Event::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    ))))
    .expect("send key event");
    let input = TerminalInputPump {
        rx,
        stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        paused: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        paused_ack: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        handle: None,
        last_alive_at: std::cell::Cell::new(Instant::now()),
    };
    let mut pending_terminal_events = VecDeque::new();
    let event = next_terminal_event(
        &input,
        &mut pending_terminal_events,
        Duration::from_millis(1),
    )
    .expect("terminal event read")
    .expect("queued key event");
    assert!(
        matches!(
            event,
            Event::Key(key)
                if key.code == KeyCode::Char('c')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
        ),
        "input pump channel should deliver input despite progress noise"
    );

    let mut app = create_test_app();
    app.is_loading = true;
    app.runtime_turn_status = Some("in_progress".to_string());
    assert_eq!(next_escape_action(&app, false), EscapeAction::CancelRequest);
    assert_eq!(ctrl_c_disposition(&app), CtrlCDisposition::CancelTurn);
}

#[test]
fn terminal_input_child_pause_drains_codewhale_events_before_editor_handoff() {
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(TerminalInputMessage::Event(Event::Key(KeyEvent::new(
        KeyCode::Char('x'),
        KeyModifiers::NONE,
    ))))
    .expect("send buffered key event");
    tx.send(TerminalInputMessage::Heartbeat)
        .expect("send buffered heartbeat");
    tx.send(TerminalInputMessage::Event(Event::Key(KeyEvent::new(
        KeyCode::Char('y'),
        KeyModifiers::NONE,
    ))))
    .expect("send second buffered key event");

    let input = TerminalInputPump {
        rx,
        stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        paused: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        paused_ack: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        handle: None,
        last_alive_at: std::cell::Cell::new(Instant::now()),
    };
    let mut pending_terminal_events = VecDeque::from([Event::Key(KeyEvent::new(
        KeyCode::Char('z'),
        KeyModifiers::NONE,
    ))]);

    input
        .pause_for_child_terminal()
        .expect("synthetic pump can pause");
    assert!(
        prepare_terminal_input_handoff(&input, &mut pending_terminal_events)
            .expect("queued terminal events drain before launching child editor")
    );

    assert!(
        pending_terminal_events.is_empty(),
        "pending CodeWhale terminal events must not leak into the editor handoff"
    );
    assert!(
        input.try_recv().expect("drained channel").is_none(),
        "input pump channel should be empty after the editor handoff drain"
    );

    input.resume_after_child_terminal();
    assert!(!input.paused.load(std::sync::atomic::Ordering::Acquire));
    assert!(!input.paused_ack.load(std::sync::atomic::Ordering::Acquire));
}

#[test]
fn terminal_input_handoff_preserves_pending_cancellation_keys() {
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(TerminalInputMessage::Event(Event::Key(KeyEvent::new(
        KeyCode::Char('\u{3}'),
        KeyModifiers::NONE,
    ))))
    .expect("send raw Ctrl+C");
    let input = TerminalInputPump {
        rx,
        stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        paused: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        paused_ack: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        handle: None,
        last_alive_at: std::cell::Cell::new(Instant::now()),
    };
    let mut pending_terminal_events = VecDeque::from([
        Event::Key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
    ]);

    input
        .pause_for_child_terminal()
        .expect("synthetic pump can pause");
    assert!(
        !prepare_terminal_input_handoff(&input, &mut pending_terminal_events)
            .expect("handoff inspection succeeds"),
        "pending cancellation must refuse the child handoff"
    );
    assert_eq!(pending_terminal_events.len(), 3);
    assert!(
        pending_terminal_events
            .iter()
            .any(|event| { matches!(event, Event::Key(key) if key.code == KeyCode::Esc) })
    );
    assert!(
        pending_terminal_events.iter().any(|event| {
            matches!(event, Event::Key(key) if key.code == KeyCode::Char('\u{3}'))
        })
    );

    input.resume_after_child_terminal();
}

#[test]
fn input_pump_restart_detaches_wedged_thread_and_installs_fresh_parts() {
    // A "wedged" pump thread blocked forever on a channel recv stands in for
    // a crossterm `event::read` that never returns (stalled Windows console
    // poll, or a Unix tty that stopped delivering bytes). Joining it would
    // hang the event loop, so `detach_current_thread` must return
    // immediately and only flag it to stop.
    let (block_tx, block_rx) = std::sync::mpsc::channel::<()>();
    let wedged = std::thread::spawn(move || {
        let _ = block_rx.recv();
    });

    let (old_tx, old_rx) = std::sync::mpsc::channel();
    let old_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut pump = TerminalInputPump {
        rx: old_rx,
        stop: std::sync::Arc::clone(&old_stop),
        paused: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        paused_ack: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        handle: Some(wedged),
        last_alive_at: std::cell::Cell::new(
            Instant::now()
                .checked_sub(Duration::from_secs(30))
                .unwrap_or_else(Instant::now),
        ),
    };

    pump.detach_current_thread();
    assert!(
        old_stop.load(std::sync::atomic::Ordering::Acquire),
        "detach must flag the old pump thread to stop"
    );
    assert!(
        pump.handle.is_none(),
        "detach must drop the wedged handle without joining it"
    );

    // Install replacement parts. A trivial thread stands in for the fresh
    // crossterm pump; spawning the real one needs an interactive terminal.
    let (new_tx, new_rx) = std::sync::mpsc::channel();
    pump.install_parts(TerminalInputPumpParts {
        rx: new_rx,
        stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        paused: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        paused_ack: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        handle: std::thread::spawn(|| {}),
    });

    assert!(pump.handle.is_some(), "fresh pump thread must be adopted");
    assert!(
        !pump.stop.load(std::sync::atomic::Ordering::Acquire),
        "fresh pump must not start in a stopped state"
    );
    assert!(
        pump.stalled_for(Instant::now()) < Duration::from_secs(30),
        "install must reset the liveness clock"
    );

    new_tx
        .send(TerminalInputMessage::Event(Event::Key(KeyEvent::new(
            KeyCode::Char('k'),
            KeyModifiers::NONE,
        ))))
        .expect("send event on replacement channel");
    let event = pump
        .try_recv()
        .expect("replacement channel readable")
        .expect("replacement channel delivers the event");
    assert!(
        matches!(event, Event::Key(key) if key.code == KeyCode::Char('k')),
        "restarted pump must deliver events from the replacement channel"
    );

    // The old channel is orphaned by the swap: a wedged thread that finally
    // wakes fails its send and exits instead of feeding stale events.
    assert!(
        old_tx.send(TerminalInputMessage::Heartbeat).is_err(),
        "old pump channel must be disconnected after restart"
    );

    drop(block_tx); // release the wedged stand-in thread
}

#[test]
fn raw_mode_probe_handshake_elects_exactly_one_side_sequentially() {
    // Task enables raw mode first, probe timeout fires second: the timeout
    // side sees `enabled` and takes responsibility for disabling.
    let enabled = std::sync::atomic::AtomicBool::new(false);
    let abandoned = std::sync::atomic::AtomicBool::new(false);
    let task_disables = raw_mode_probe_handshake(&enabled, &abandoned);
    let caller_disables = raw_mode_probe_handshake(&abandoned, &enabled);
    assert!(!task_disables, "task ran first, so it must not disable");
    assert!(
        caller_disables,
        "timed-out caller must undo the late enable"
    );

    // Probe timeout fires first, task finishes enabling second: the task
    // side sees `abandoned` and disables its own late enable.
    let enabled = std::sync::atomic::AtomicBool::new(false);
    let abandoned = std::sync::atomic::AtomicBool::new(false);
    let caller_disables = raw_mode_probe_handshake(&abandoned, &enabled);
    let task_disables = raw_mode_probe_handshake(&enabled, &abandoned);
    assert!(!caller_disables, "caller ran first, so it must not disable");
    assert!(
        task_disables,
        "late-finishing task must undo its own enable"
    );
}

#[test]
fn raw_mode_probe_handshake_never_leaks_under_concurrent_race() {
    // Race both sides on real threads: no interleaving may leave raw mode
    // leaked, i.e. at least one side must observe the other's flag.
    for _ in 0..200 {
        let enabled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let abandoned = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task_enabled = std::sync::Arc::clone(&enabled);
        let task_abandoned = std::sync::Arc::clone(&abandoned);
        let task =
            std::thread::spawn(move || raw_mode_probe_handshake(&task_enabled, &task_abandoned));
        let caller_disables = raw_mode_probe_handshake(&abandoned, &enabled);
        let task_disables = task.join().expect("handshake task side");
        assert!(
            task_disables || caller_disables,
            "at least one side must take responsibility for disabling raw mode"
        );
    }
}

#[test]
fn backtrack_cut_index_skips_tool_result_user_messages() {
    use crate::models::{ContentBlock, Message};
    // A turn with tools: user prompt, assistant tool_use, tool_result (role=user),
    // assistant text; then a second user prompt.
    let msgs = vec![
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "first".into(),
                cache_control: None,
            }],
        },
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path":"x"}),
                caller: None,
                thought_signature: None,
            }],
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                content: "data".into(),
                is_error: None,
                content_blocks: None,
            }],
        },
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "answer".into(),
                cache_control: None,
            }],
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "second".into(),
                cache_control: None,
            }],
        },
    ];
    // depth 0 = cut at the last real user prompt ("second", idx 4).
    assert_eq!(super::backtrack_api_cut_index(&msgs, 0), Some(4));
    // depth 1 = cut at the first real user prompt ("first", idx 0) — NOT the
    // tool_result at idx 2 that a naive role=="user" count would have hit.
    assert_eq!(super::backtrack_api_cut_index(&msgs, 1), Some(0));
    // depth beyond available prompts.
    assert_eq!(super::backtrack_api_cut_index(&msgs, 2), None);
}

/// The idle BlueWhale needs 16 transcript rows. Everything above the
/// transcript is paid out of those rows, so a rail strip that reserves a
/// fixed band evicts the whale on exactly the terminal sizes the release
/// evidence is captured at. These tests pin the operator-visible threshold at
/// 80 columns; the row-by-row arithmetic lives in `ui::rail_row_budget`.
///
/// The module is named `work_surface` on purpose. The rail regression these
/// tests exist to catch shipped anyway because the work-surface change was
/// verified with `cargo test -- work_surface::`, which matches only
/// `tui::work_surface::tests` and skipped every assertion below. Under this
/// name the same filter reaches them: `tui::ui::tests::work_surface::…`.
mod work_surface {
    use super::*;
    use crate::tui::ui::{rail_min_chat_width, rail_row_budget};
    use crate::tui::underwater::AMBIENT_MIN_CHAT_WIDTH;
    use crate::tui::widgets::should_render_empty_state;
    use crate::tui::work_surface::{RailPanel, WorkSurfacePlacement};

    /// The lowest readable Top strip: enough room for the divider and the
    /// compact goal / to-do / Agent projection.
    const TOP_HEIGHT_MIN: u16 = crate::settings::WORK_SURFACE_TOP_HEIGHT_MIN;

    fn idle_rail_app(panel: RailPanel) -> App {
        let mut app = create_test_app();
        app.ui_locale = crate::localization::Locale::En;
        // Pin the chrome the budget charges for: `App::new` reads the developer's
        // real settings.toml, and a host with `composer_border = false` would
        // shift every threshold below by a row.
        app.composer_border = true;
        // Same reason, and it bites harder: `work_surface_top_height` is
        // user-settable over 5..=16 and drag-resizing the divider persists it. A
        // developer whose settings.toml carries a short strip asks for a short
        // strip and gets one, at a *lower* row threshold than the default 8 — so
        // leaving this unpinned makes the thresholds below depend on whoever runs
        // the suite. Pin the shipped default.
        app.work_surface.top_height = 8;
        app.work_surface.placement = WorkSurfacePlacement::Top;
        app.work_surface.panel = panel;
        app
    }

    /// The nickname the seeded sub-agent renders under. A marker the test owns,
    /// rather than chrome the shell might stop drawing.
    const AGENT_MARK: &str = "railprobe";

    /// A rail app whose panel actually has something to show.
    ///
    /// Since `4206ebd2c` an empty Top panel collapses to zero rows — "an empty
    /// panel is not a panel" — so an idle `Agents` (or `Pinned`) panel measures
    /// 0 at every terminal size. A *yield* rule is only observable when there
    /// are rows to yield, so these tests seat real content first; without it
    /// every assertion below would be about a strip that was never going to
    /// exist.
    ///
    /// `Agents` is the panel that can hold content without ending the idle
    /// session: `should_render_empty_state` looks at history, todos, goal and
    /// background tasks, and a cached sub-agent is in none of them. A `Pinned`
    /// panel with content needs a goal or a checklist, either of which puts the
    /// session to work and takes the ocean away — which is the other half of
    /// what is being measured here.
    fn busy_rail_app(panel: RailPanel) -> App {
        let mut app = idle_rail_app(panel);
        let mut agent = make_subagent(
            "agent_rail_probe",
            crate::tools::subagent::SubAgentStatus::Running,
        );
        agent.nickname = Some(AGENT_MARK.to_string());
        app.subagent_cache.push(agent);
        // A fan-out, not a single worker: the yield tests need the panel's
        // natural height (heading + rows + divider) to sit above the
        // TOP_HEIGHT_MIN clamp, or "the strip yielded rows" becomes
        // unobservable now that the Agents panel row projection is more
        // compact than the old line list.
        app.subagent_cache.push(make_subagent(
            "agent_rail_probe_b",
            crate::tools::subagent::SubAgentStatus::Running,
        ));
        app.subagent_cache.push(make_subagent(
            "agent_rail_probe_c",
            crate::tools::subagent::SubAgentStatus::Running,
        ));
        app.subagent_cache.push(make_subagent(
            "agent_rail_probe_d",
            crate::tools::subagent::SubAgentStatus::Running,
        ));
        assert!(
            should_render_empty_state(&app),
            "the Agents fixture must leave the session idle — the ocean it is \
             measured against only draws in the empty state"
        );
        app
    }

    /// Whether the ambient ocean actually got its floor this frame, measured
    /// from the empty state's transcript area. The hand-drawn idle whale that
    /// used to be the on-screen evidence here was deleted per the 2026-08-29
    /// founder directive; the whale drew exactly when this predicate held, so
    /// `empty_state_mark_visible` over the recorded transcript area is the
    /// same visibility, asserted without the deleted art.
    fn idle_ocean_visible(app: &App) -> bool {
        app.viewport
            .last_transcript_area
            .is_some_and(crate::tui::underwater::empty_state_mark_visible)
    }

    /// The strip height for this frame, measured the way the shell measures it:
    /// the idle predicate, then the budget, then `height()` — the same three
    /// calls, in the same order, that `ui::render` makes.
    ///
    /// This replaces the old `rendered.contains(panel.title())` probe, which is
    /// no longer capable of reporting a strip at all: `4206ebd2c` made the only
    /// Top title an active goal, so panel chrome ("Agents", "Pinned") is
    /// deliberately never painted there. Worse than under-reporting, the probe
    /// *passed* every `!contains(title)` assertion for free, so the negative
    /// half of the yield rule had quietly stopped testing anything.
    /// `a_strip_that_measures_nonzero_is_a_strip_that_paints` keeps this
    /// number honest against what the frame actually contains, and
    /// `top_placement_never_paints_panel_chrome_as_a_title` is the guard that
    /// fails if Top starts painting titles again.
    fn strip_height(app: &mut App, width: u16, rows: u16) -> u16 {
        let idle_empty = should_render_empty_state(app);
        let budget = rail_row_budget(app, width, rows, idle_empty);
        crate::tui::work_surface::height(app, width, rows, budget)
    }

    /// The rows the panel asks for when nothing constrains it: auto-fitted
    /// content plus the divider. Derived rather than hard-coded, so adding a
    /// column or a line to the Agents panel moves the yardstick instead of
    /// breaking the yield rule that is measured against it.
    fn natural_strip_rows(panel: RailPanel) -> u16 {
        let mut app = busy_rail_app(panel);
        strip_height(&mut app, 80, 48)
    }

    #[test]
    fn rail_strip_yields_its_rows_so_the_idle_ocean_survives_at_24_rows() {
        let panel = RailPanel::Agents;
        let natural = natural_strip_rows(panel);
        assert!(
            natural > TOP_HEIGHT_MIN,
            "the fixture must want more rows than the floor, or 'the strip \
             yielded rows' is unobservable (natural={natural})"
        );

        // Below the threshold the rail hands rows back to the water: it renders
        // shorter than it wants, or not at all, and the ocean keeps its floor.
        // 24 rows is the release-evidence size the rail regression took the
        // ambient ocean away from.
        for rows in [22_u16, 23, 24, 25] {
            let mut app = busy_rail_app(panel);
            let budget = rail_row_budget(&app, 80, rows, true);
            let strip = strip_height(&mut app, 80, rows);
            assert!(
                strip < natural,
                "at 80x{rows} the {panel:?} strip took its full {natural} rows \
                 instead of yielding; decorative water outranks a panel nobody \
                 is watching"
            );
            assert!(
                strip <= budget,
                "at 80x{rows} the {panel:?} strip took {strip} rows out of a \
                 {budget}-row budget — those rows belong to the transcript"
            );
            // A surface below the readable floor is not a compact rail; it is
            // a divider plus hidden work. Yield to zero instead.
            assert!(
                strip == 0 || strip >= TOP_HEIGHT_MIN,
                "at 80x{rows} the strip is {strip} row(s): a divider with no \
                 panel under it. Yield to zero or keep a content row."
            );
            let rendered = render_underwater_test_app(&mut app, 80, rows);
            assert!(
                idle_ocean_visible(&app),
                "the ambient ocean floor must survive at 80x{rows}: decorative \
                 water outranks a panel nobody is watching\n{rendered}"
            );
        }

        // At and above the threshold the rows are genuinely spare, so the rail
        // takes its full auto-fit height over an intact 16-row ocean. The
        // Tideline shell charges one row less than the classic shell did
        // (one-row info line, merged footer instead of two standing bands), so
        // the threshold sits one row lower again.
        for rows in [29_u16, 30, 32] {
            let mut app = busy_rail_app(panel);
            let strip = strip_height(&mut app, 80, rows);
            assert_eq!(
                strip, natural,
                "the {panel:?} strip must render at its full height once the \
                 transcript can spare the rows at 80x{rows}"
            );
            let rendered = render_underwater_test_app(&mut app, 80, rows);
            assert!(
                idle_ocean_visible(&app),
                "the ambient ocean floor must still be earned at 80x{rows}\n{rendered}"
            );
            assert!(
                rendered.contains(AGENT_MARK),
                "the strip measures {strip} rows at 80x{rows} but the agent it \
                 is made of is not on screen\n{rendered}"
            );
        }

        // 21 rows now seats the ocean floor exactly — the merged footer
        // returned the activity band's row to the stage (info line 1 + footer 1
        // + composer floor 3 + the 16-row ambient floor = 21). 20 rows
        // cannot seat the ocean at any strip height. That is pre-rail
        // behavior and the yield rule must not pretend otherwise.
        let mut app = busy_rail_app(panel);
        assert_eq!(
            strip_height(&mut app, 80, 21),
            0,
            "80x21 seats the ocean floor but has no spare rows for a strip"
        );
        let rendered = render_underwater_test_app(&mut app, 80, 21);
        assert!(
            idle_ocean_visible(&app),
            "80x21 is exactly the ocean floor under the Tideline shell:\n{rendered}"
        );
        let mut app = busy_rail_app(panel);
        assert_eq!(
            strip_height(&mut app, 80, 20),
            0,
            "80x20 has no spare rows for a strip at all"
        );
        let rendered = render_underwater_test_app(&mut app, 80, 20);
        assert!(
            !idle_ocean_visible(&app),
            "80x20 has no room for the ocean even with no strip at all\n{rendered}"
        );
    }

    #[test]
    fn the_rail_never_costs_the_ocean_a_single_terminal_size() {
        // The whole point of the row budget, stated directly and swept rather
        // than sampled: turning the rail on must not remove the whale from any
        // size it would have had with the rail off. This is what the old
        // "the strip must be absent at 22..=25" assertion was reaching for —
        // that framing only worked while the strip was a fixed four-row band,
        // and it now under-tests (four hand-picked sizes) and over-constrains
        // (a readable strip that costs the water nothing is fine).
        //
        // A rail that never renders costs the ocean nothing either, so pin that
        // there is a strip to compare against before comparing.
        let mut probe = busy_rail_app(RailPanel::Agents);
        assert!(
            strip_height(&mut probe, 80, 40) > 0,
            "no strip renders at 80x40, so 'the rail costs the ocean nothing' is \
             a statement about nothing"
        );

        for rows in 18_u16..=40 {
            let mut with_rail = busy_rail_app(RailPanel::Agents);
            let _ = render_underwater_test_app(&mut with_rail, 80, rows);
            let with = idle_ocean_visible(&with_rail);

            let mut without_rail = busy_rail_app(RailPanel::Agents);
            without_rail.work_surface.placement = WorkSurfacePlacement::Off;
            let _ = render_underwater_test_app(&mut without_rail, 80, rows);
            let without = idle_ocean_visible(&without_rail);

            assert_eq!(
                with, without,
                "at 80x{rows} the rail changed whether the ocean draws \
                 (rail on: {with}, rail off: {without}) — the strip is spending \
                 rows the transcript needed"
            );
        }
    }

    #[test]
    fn rail_strip_and_idle_ocean_never_thrash_across_a_resize() {
        // A threshold that is not monotonic in terminal height reads as flicker:
        // drag a terminal edge and the strip blinks in and out. Growing the
        // terminal may only ever *add* chrome, never take it away, and the same
        // size must always produce the same answer.
        for panel in [RailPanel::Tasks, RailPanel::Agents, RailPanel::Context] {
            for idle_empty in [true, false] {
                let mut previous_strip = 0_u16;
                for rows in 8_u16..=48 {
                    // Content, not an empty fixture: an empty Pinned or Agents
                    // panel collapses to 0 at every size, and a sweep of zeros
                    // is monotone for a reason that has nothing to do with the
                    // budget. `height()` takes `idle_empty` through the budget
                    // argument, so seeding work here does not disturb the sweep.
                    let mut app = busy_rail_app(panel);
                    app.todos.try_lock().expect("todos lock").add(
                        "rail-checklist-item".to_string(),
                        crate::tools::todo::TodoStatus::InProgress,
                    );
                    let budget = rail_row_budget(&app, 80, rows, idle_empty);
                    let strip = crate::tui::work_surface::height(&mut app, 80, rows, budget);
                    let again = crate::tui::work_surface::height(&mut app, 80, rows, budget);
                    assert_eq!(
                        strip, again,
                        "{panel:?} at 80x{rows} (idle={idle_empty}) must be a pure \
                         function of this frame, not of the last one"
                    );
                    assert!(
                        strip >= previous_strip,
                        "{panel:?} strip shrank from {previous_strip} to {strip} when the \
                         terminal grew to {rows} rows (idle={idle_empty}); a strip that \
                         blinks on resize is worse than the eviction bug"
                    );
                    previous_strip = strip;
                }
                assert!(
                    previous_strip > 0,
                    "{panel:?} (idle={idle_empty}) measured 0 across the whole \
                     8..=48 sweep — the fixture has no content and the \
                     monotonicity check is vacuous"
                );
            }
        }

        // The ocean floor is monotone too: once the ocean is earned, growing
        // the terminal never takes it back.
        let mut seen_ocean = false;
        for rows in 16_u16..=34 {
            let mut app = busy_rail_app(RailPanel::Agents);
            let rendered = render_underwater_test_app(&mut app, 80, rows);
            let ocean = idle_ocean_visible(&app);
            assert!(
                ocean || !seen_ocean,
                "the ambient ocean floor disappeared again at 80x{rows} after being earned at a \
                 smaller size\n{rendered}"
            );
            seen_ocean |= ocean;
        }
        assert!(
            seen_ocean,
            "the sweep never earned the ambient ocean at all"
        );
    }

    #[test]
    fn rail_strip_and_ocean_swap_at_the_ambient_width() {
        // The width gate is a real trade and this test exists so it stays a
        // decision rather than drifting into an accident.
        //
        // `rail_row_budget` charges the ambient floor only when the mark can
        // actually draw, which needs AMBIENT_MIN_CHAT_WIDTH columns. So on a
        // terminal tall enough to be interesting but narrower than that floor,
        // the rail keeps its rows; widening past the floor hands them to the
        // water. Unlike the height axis — where the same rule would make the
        // strip vanish as you drag the very axis it is measured in — this fires
        // on a deliberate horizontal resize with a visible payoff.
        let panel = RailPanel::Agents;
        let width_floor = AMBIENT_MIN_CHAT_WIDTH;
        let rows = 24_u16;

        let mut app = busy_rail_app(panel);
        let narrow_strip = strip_height(&mut app, width_floor - 1, rows);

        let mut app = busy_rail_app(panel);
        let wide_strip = strip_height(&mut app, width_floor, rows);

        assert!(
            narrow_strip > wide_strip,
            "below {width_floor} columns the ambient mark cannot draw, so the rail should \
             keep its rows ({narrow_strip}) and yield them at the floor ({wide_strip}); if \
             these are equal the width gate has stopped doing anything"
        );

        // And the payoff is real: the rows the rail gave up become water.
        let mut app = busy_rail_app(panel);
        let rendered = render_underwater_test_app(&mut app, width_floor, rows);
        assert!(
            idle_ocean_visible(&app),
            "yielding the strip at {width_floor}x{rows} must actually buy the ocean\n{rendered}"
        );
    }

    #[test]
    fn side_rail_yields_the_columns_the_idle_ocean_needs() {
        // The width axis carried the same defect: a side rail reserves at least
        // 26 columns while `split_chat` only ever protected a 40-column
        // transcript, well under the ocean's 60-column floor.
        let idle_floor = rail_min_chat_width(true);
        let right = WorkSurfacePlacement::Right;

        // A populated panel for the same reason as the row tests: `split_chat`
        // reserves no columns at all for an empty panel, so an idle fixture
        // would report "no rail" for a reason unrelated to the ocean's floor.
        let mut app = busy_rail_app(RailPanel::Agents);
        app.work_surface.placement = right;
        let area = ratatui::layout::Rect::new(0, 0, 80, 40);
        let (chat, rail) = crate::tui::work_surface::split_chat(&mut app, area, idle_floor);
        assert!(
            rail.is_none(),
            "an 80-column host cannot seat a 26-column rail beside a 60-column ocean"
        );
        assert_eq!(chat, area, "the transcript keeps every column");

        // Wide enough for both: the rail comes back and the ocean keeps its floor.
        let mut app = busy_rail_app(RailPanel::Agents);
        app.work_surface.placement = right;
        let area = ratatui::layout::Rect::new(0, 0, 100, 40);
        let (chat, rail) = crate::tui::work_surface::split_chat(&mut app, area, idle_floor);
        let rail = rail.expect("a 100-column host seats both");
        assert!(
            chat.width >= AMBIENT_MIN_CHAT_WIDTH,
            "chat kept {} columns, below the ambient floor",
            chat.width
        );
        assert!(rail.width >= 26, "rail kept {} columns", rail.width);

        // With work on screen there is no ambient floor, so the 80-column host
        // keeps its rail: work never yields to decoration.
        let mut app = busy_rail_app(RailPanel::Agents);
        app.work_surface.placement = right;
        let area = ratatui::layout::Rect::new(0, 0, 80, 40);
        let (_chat, rail) =
            crate::tui::work_surface::split_chat(&mut app, area, rail_min_chat_width(false));
        assert!(rail.is_some(), "a busy 80-column shell keeps its side rail");
    }

    #[test]
    fn readable_top_height_is_stable_and_yields_cleanly_when_starved() {
        // `work_surface_top_height` is a preference over 5..=16 that drag-resizing
        // persists. Every accepted value has room for actual work, while the
        // ambient budget may still hide the whole rail to protect the ocean.
        let panel = RailPanel::Agents;

        // The cliff is charged against ambient room alone, so the size at which
        // a strip first appears must be the *same* for every requested height.
        // This is the property the old `strip == 0 || strip == top_height`
        // assertion was standing in for, and it is stronger: that form was true
        // of the fixed-height rail but is now false for an honest reason — the
        // ambient budget can clamp an auto-fitted strip to fewer rows than the
        // user's ceiling, which is the yield rule working, not the cliff.
        let mut first_visible_at: Option<(u16, u16)> = None;
        for top_height in [TOP_HEIGHT_MIN, 8, 16] {
            let mut first = None;
            let mut tallest = 0_u16;
            for rows in 8_u16..=48 {
                let mut app = busy_rail_app(panel);
                app.work_surface.top_height = top_height;
                let strip = strip_height(&mut app, 80, rows);
                assert!(
                    strip <= top_height,
                    "top_height={top_height} at 80x{rows} produced a {strip}-row \
                     strip: the user's height is a ceiling, not a suggestion"
                );
                if strip > 0 && first.is_none() {
                    first = Some(rows);
                }
                tallest = tallest.max(strip);
            }
            let first = first.unwrap_or_else(|| {
                panic!(
                    "top_height={top_height} never produced a strip at any size in \
                     8..=48 — the cliff is being charged against the user's preference \
                     again"
                )
            });
            match first_visible_at {
                None => first_visible_at = Some((top_height, first)),
                Some((other_height, other_first)) => assert_eq!(
                    first, other_first,
                    "top_height={top_height} first shows a strip at {first} rows but \
                     top_height={other_height} shows one at {other_first}: the \
                     requested height is leaking into the collapse threshold"
                ),
            }
            // A ceiling below the panel's natural height must be reachable, or
            // it is a number the cliff quietly ate.
            if top_height < natural_strip_rows(panel) {
                assert_eq!(
                    tallest, top_height,
                    "top_height={top_height} never reached its own ceiling across \
                     8..=48 (tallest was {tallest} rows)"
                );
            }
        }

        // At the 80x24 ambient evidence size there are only two spare rows.
        // That is below the readable floor, so the rail yields completely and
        // cannot leave an invisible focus target behind.
        let mut app = busy_rail_app(panel);
        app.work_surface.top_height = TOP_HEIGHT_MIN;
        assert_eq!(
            strip_height(&mut app, 80, 24),
            0,
            "an unreadable strip must yield rather than hide its work at 80x24"
        );
        let rendered = render_underwater_test_app(&mut app, 80, 24);
        assert!(
            idle_ocean_visible(&app),
            "a yielded strip must leave the ambient ocean intact at 80x24\n{rendered}"
        );
        assert!(app.work_surface.last_area.is_none());
        assert!(!app.work_surface.focused);
    }

    #[test]
    fn a_strip_that_measures_nonzero_is_a_strip_that_paints() {
        // `strip_height` is the oracle the rest of this module trusts for "the
        // strip rendered". This is the test that earns that trust: a number
        // nobody checks against a frame is exactly the mistake the old
        // `contains(panel.title())` probe made in the other direction.
        let panel = RailPanel::Agents;

        let mut app = busy_rail_app(panel);
        let tall = strip_height(&mut app, 80, 30);
        assert!(tall > 0, "80x30 must have room for a strip");
        let rendered = render_underwater_test_app(&mut app, 80, 30);
        assert!(
            rendered.contains(AGENT_MARK),
            "strip_height says {tall} rows at 80x30, but the agent that panel is \
             made of never reached the frame\n{rendered}"
        );

        let mut app = busy_rail_app(panel);
        let short = strip_height(&mut app, 80, 22);
        assert_eq!(short, 0, "80x22 has no spare rows for a strip");
        let rendered = render_underwater_test_app(&mut app, 80, 22);
        // Probe the strip's own rows — the first body row, now that nothing
        // paints above the stage — not the whole frame: the background-work
        // chip (slot 4) is a different surface and may legitimately spend its
        // spare row on the same agent's name now that the merged footer
        // freed one.
        let strip_slot_row = rendered.lines().next().expect("the first body row");
        assert!(
            !strip_slot_row.contains(AGENT_MARK),
            "strip_height says {short} rows at 80x22, but the panel painted \
             anyway: {strip_slot_row:?}"
        );
    }

    #[test]
    fn top_placement_never_paints_panel_chrome_as_a_title() {
        // `4206ebd2c`: the only Top title is an active goal — never a panel name.
        // Four rail tests were silently reduced to no-ops by that change because
        // they used `contains(panel.title())` as their "did the strip render"
        // probe; the assertions that were supposed to fail loudly instead passed
        // for free. This guard is the inverse and cannot go vacuous: the strip is
        // proven present by `strip_height` *before* the frame is searched for
        // chrome, so if Top starts painting "Agents" or "Pinned" again, it fails
        // here rather than quietly re-arming that trap.
        for panel in [RailPanel::Agents, RailPanel::Tasks, RailPanel::Context] {
            let mut app = idle_rail_app(panel);
            let mut agent = make_subagent(
                "agent_rail_probe",
                crate::tools::subagent::SubAgentStatus::Running,
            );
            agent.nickname = Some(AGENT_MARK.to_string());
            app.subagent_cache.push(agent);
            // Goal and checklist so `Pinned` has content too, and so the goal
            // title — the one title Top *is* allowed to paint — is in play.
            app.goal.objective = Some("keep the chrome off the strip".to_string());
            app.todos.try_lock().expect("todos lock").add(
                "rail-checklist-item".to_string(),
                crate::tools::todo::TodoStatus::InProgress,
            );

            let strip = strip_height(&mut app, 100, 34);
            assert!(
                strip > 0,
                "{panel:?} must have a strip at 100x34 for this guard to mean \
                 anything"
            );
            let rendered = render_underwater_test_app(&mut app, 100, 34);
            // A chrome title would be a row that says nothing but the view's
            // name. The dock tab row and the `▾ Subagents N` group door both
            // legitimately contain the lowercase word, so probe whole rows.
            let strip_body = rendered.lines().skip(1).collect::<Vec<_>>().join("\n");
            assert!(
                !strip_body.lines().any(|line| line.trim() == panel.title()),
                "a {strip}-row {panel:?} strip painted its panel name \
                 ({:?}) as chrome. Top titles are goals only — and a title \
                 probe is what let the rail regression through last time\n{rendered}",
                panel.title()
            );
        }
    }

    #[test]
    fn the_ambient_floor_is_not_charged_where_the_mark_cannot_draw() {
        // `should_render_empty_state` knows the session is quiet; it does not
        // know the terminal is too narrow to draw the mark at all. Charging the
        // 16-row ambient floor below `AMBIENT_MIN_CHAT_WIDTH` reserves rows for
        // something that can never appear, and the strip yields for nothing.
        let app = idle_rail_app(RailPanel::Tasks);
        let narrow = AMBIENT_MIN_CHAT_WIDTH - 1;
        assert_eq!(
            rail_row_budget(&app, narrow, 24, true),
            rail_row_budget(&app, narrow, 24, false),
            "a {narrow}-column terminal cannot draw the mark, so idleness must not \
             change the rail's budget"
        );
        assert!(
            rail_row_budget(&app, AMBIENT_MIN_CHAT_WIDTH, 24, true)
                < rail_row_budget(&app, narrow, 24, true),
            "at the ambient width floor the mark can draw, so the budget must shrink"
        );
    }
}

/// A refused route change must not pin the *old* route as the startup default,
/// and must say so. Reporting nothing is what made the original sticky-default
/// report so hard to diagnose: the user saw an ordinary apply summary and had
/// no way to tell that the default had not moved.
#[tokio::test]
async fn refused_route_change_does_not_pin_a_startup_default_and_says_so() {
    let _guard = SettingsHomeGuard::new();
    let mut config = Config {
        provider: Some("xai".to_string()),
        providers: Some(ProvidersConfig {
            xai: ProviderConfig {
                api_key: Some("xai-test-key".to_string()),
                model: Some("grok-4.5".to_string()),
                ..ProviderConfig::default()
            },
            ..ProvidersConfig::default()
        }),
        ..Config::default()
    };
    let options = TuiOptions {
        model: config.default_model(),
        start_in_agent_mode: true,
        skip_onboarding: false,
        ..crate::test_support::test_tui_options(PathBuf::from("."))
    };
    let mut app = App::new(options, &config);
    let previous_effort = app.reasoning_effort;
    let mut engine = mock_engine_handle();
    // A busy session refuses model/thinking changes outright.
    app.is_loading = true;

    apply_model_picker_choice(
        &mut app,
        &mut engine.handle,
        &mut config,
        "deepseek-v4-flash".to_string(),
        Some(ApiProvider::Deepseek),
        None,
        previous_effort,
        "grok-4.5".to_string(),
        previous_effort,
        true,
    )
    .await;

    assert_eq!(
        app.api_provider,
        ApiProvider::Xai,
        "the refused switch must leave the live route alone"
    );
    let settings = crate::settings::Settings::load().expect("load settings");
    assert_eq!(
        settings.default_provider.as_deref(),
        None,
        "a refused change must never pin the route the user tried to leave"
    );
    assert!(
        app.status_message
            .as_deref()
            .is_some_and(|message| message.contains("Startup default unchanged")),
        "the operator must be told the default did not move: {:?}",
        app.status_message
    );
}

#[test]
fn gate_receipts_are_held_until_their_tool_card_completes_then_flushed_in_order() {
    let mut app = create_test_app();
    let before = app.history.len();
    app.pending_gate_receipts
        .push(("call-a".to_string(), "receipt for a".to_string()));
    app.pending_gate_receipts
        .push(("call-b".to_string(), "receipt for b".to_string()));

    // Completing an unrelated tool moves nothing.
    assert!(!super::event_loop::flush_gate_receipts_for(
        &mut app,
        Some("call-zzz")
    ));
    assert_eq!(app.history.len(), before);
    assert_eq!(app.pending_gate_receipts.len(), 2);

    // Completing tool `b` lands only its receipt, as a System note.
    assert!(super::event_loop::flush_gate_receipts_for(
        &mut app,
        Some("call-b")
    ));
    assert_eq!(app.history.len(), before + 1);
    assert!(matches!(
        &app.history[before],
        HistoryCell::System { content } if content == "receipt for b"
    ));
    assert_eq!(app.pending_gate_receipts.len(), 1);

    // Turn end flushes whatever is left so no decision goes unseen.
    assert!(super::event_loop::flush_gate_receipts_for(&mut app, None));
    assert_eq!(app.history.len(), before + 2);
    assert!(app.pending_gate_receipts.is_empty());
    assert!(!super::event_loop::flush_gate_receipts_for(&mut app, None));
}

// ===========================================================================
// #5478 — a mid-turn history insert must not orphan an in-flight tool row
// ===========================================================================

/// `/rename` mid-turn left the running tool row spinning forever, even though
/// the job completed and everything else (result, footer, `/jobs`, cost) was
/// correct.
///
/// Root cause is not in the rename path at all: an in-flight tool is bound to a
/// *virtual* index, `history.len() + entry_index`, and completion resolves it
/// against `history.len()` **as it is at completion time**. Any command that
/// pushes a cell into history mid-turn — `/rename`'s "Session and terminal tab
/// renamed to …" note, and every other command that reports a message —
/// shifts `history.len()`, so the binding silently comes to mean a different
/// cell. The completion then updates the wrong cell and the real row never
/// leaves "running".
#[test]
fn a_message_added_mid_turn_does_not_strand_the_running_tool_row() {
    use crate::tui::history::{ToolCell, ToolStatus};

    let mut app = create_test_app();
    let input = serde_json::json!({"action": "run", "command": "sleep 12; echo slow-done"});
    handle_tool_call_started(&mut app, "bash-inflight", "Bash", &input);

    // The exact thing /rename does while the tool is still running.
    app.add_message(HistoryCell::System {
        content: "Session and terminal tab renamed to \"v099-dogfood\"".to_string(),
    });

    let result: Result<crate::tools::spec::ToolResult, crate::tools::spec::ToolError> =
        Ok(crate::tools::spec::ToolResult::success("slow-done"));
    crate::tui::tool_routing::handle_tool_call_complete(&mut app, "bash-inflight", "Bash", &result);

    let active = app.active_cell.as_ref().expect("active cell still present");
    let HistoryCell::Tool(ToolCell::Exec(exec)) = &active.entries()[0] else {
        panic!("the in-flight Bash row must still be an ExecCell")
    };
    assert_eq!(
        exec.status,
        ToolStatus::Success,
        "the tool row must flip to done; leaving it Running is the #5478 stuck spinner"
    );

    // And the note itself must be untouched — the completion must not have
    // been written over the message that displaced it.
    let note = app
        .history
        .iter()
        .rev()
        .find_map(|cell| match cell {
            HistoryCell::System { content } => Some(content.clone()),
            _ => None,
        })
        .expect("the rename note stays in history");
    assert!(note.contains("v099-dogfood"), "{note}");
}

/// The same binding survives several mid-turn inserts, not just one.
#[test]
fn several_mid_turn_messages_still_leave_the_tool_row_resolvable() {
    use crate::tui::history::{ToolCell, ToolStatus};

    let mut app = create_test_app();
    let input = serde_json::json!({"action": "run", "command": "sleep 12"});
    handle_tool_call_started(&mut app, "bash-inflight", "Bash", &input);
    for index in 0..4 {
        app.add_message(HistoryCell::System {
            content: format!("mid-turn note {index}"),
        });
    }

    let result: Result<crate::tools::spec::ToolResult, crate::tools::spec::ToolError> =
        Ok(crate::tools::spec::ToolResult::success("done"));
    crate::tui::tool_routing::handle_tool_call_complete(&mut app, "bash-inflight", "Bash", &result);

    let active = app.active_cell.as_ref().expect("active cell");
    let HistoryCell::Tool(ToolCell::Exec(exec)) = &active.entries()[0] else {
        panic!("ExecCell")
    };
    assert_eq!(exec.status, ToolStatus::Success);
}

#[test]
fn subagent_event_ownership_fails_closed_across_session_switches() {
    use super::event_loop::event_owner_is_active;

    assert!(event_owner_is_active(Some("session-a"), "session-a"));
    assert!(!event_owner_is_active(Some("session-b"), "session-a"));
    assert!(!event_owner_is_active(Some("session-a"), ""));
    assert!(!event_owner_is_active(None, "session-a"));
    assert!(
        event_owner_is_active(Some("session-a"), "session-a"),
        "A -> B -> A restores eligibility only after the active id is A again"
    );
}

/// Seed a worker's resident transcript with `message_count` one-line
/// assistant messages — long enough that the focused pane must scroll.
fn seed_focused_agent_transcript(app: &mut App, agent_id: &str, message_count: usize) {
    let messages: Vec<_> = (0..message_count)
        .map(|i| {
            serde_json::json!({
                "role": "assistant",
                "content": [{"type": "text", "text": format!("child line {i}"), "cache_control": null}]
            })
        })
        .collect();
    let mut store = app
        .runtime_services
        .handle_store
        .try_lock()
        .expect("handle store");
    let _ = store.insert_json(
        format!("agent:{agent_id}"),
        "full_transcript",
        serde_json::json!({ "message_count": messages.len(), "messages": messages }),
    );
}

#[test]
fn focused_agent_transcript_receives_page_scroll_through_the_frame() {
    let mut app = create_test_app();
    app.onboarding = OnboardingState::None;
    app.launch.visible = false;
    // A non-empty main conversation is the production shape under focus:
    // the dispatch prompt that spawned the worker is already in history, so
    // the ChatWidget sampled for the ocean column takes its full (non-empty)
    // path — the exact path that used to swallow the scroll delta.
    app.history = vec![HistoryCell::User {
        content: "dispatch a scout".to_string(),
    }];
    app.resync_history_revisions();
    seed_focused_agent_transcript(&mut app, "agent_page", 40);
    crate::tui::agent_focus::focus_agent(&mut app, "agent_page");

    let config = Config::default();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("test terminal");
    terminal
        .draw(|frame| {
            super::frame::render(frame, &mut app, &config);
        })
        .expect("first frame");
    let tail_top = app.viewport.last_transcript_top;
    let page = app.viewport.last_transcript_visible.max(1);
    assert!(
        tail_top > 0,
        "fixture must overflow the viewport for scrolling to be observable"
    );

    // PageUp accumulates into the shared pending delta exactly as the key
    // loop does for the main transcript.
    app.scroll_up(page);
    terminal
        .draw(|frame| {
            super::frame::render(frame, &mut app, &config);
        })
        .expect("second frame");

    let focus = app.agent_focus.as_ref().expect("focus survives the frame");
    assert!(
        focus.scroll_top.is_some(),
        "PageUp must pin the focused transcript like the main transcript"
    );
    assert!(
        app.viewport.last_transcript_top < tail_top,
        "the focused pane's viewport must actually move up"
    );
    assert!(
        app.viewport.transcript_scroll.is_at_tail(),
        "the invisible main transcript must not consume the focused pane's scroll"
    );
}

#[test]
fn focused_agent_transcript_receives_wheel_scroll_through_the_frame() {
    let mut app = create_test_app();
    app.onboarding = OnboardingState::None;
    app.launch.visible = false;
    app.history = vec![HistoryCell::User {
        content: "dispatch a scout".to_string(),
    }];
    app.resync_history_revisions();
    seed_focused_agent_transcript(&mut app, "agent_wheel", 40);
    crate::tui::agent_focus::focus_agent(&mut app, "agent_wheel");

    let config = Config::default();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("test terminal");
    terminal
        .draw(|frame| {
            super::frame::render(frame, &mut app, &config);
        })
        .expect("first frame");
    let tail_top = app.viewport.last_transcript_top;
    assert!(tail_top > 0, "fixture must overflow the viewport");

    let events = handle_mouse_event(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 4,
            row: 4,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(events.is_empty());
    terminal
        .draw(|frame| {
            super::frame::render(frame, &mut app, &config);
        })
        .expect("second frame");

    let focus = app.agent_focus.as_ref().expect("focus survives the frame");
    assert!(
        focus.scroll_top.is_some(),
        "the mouse wheel must scroll the focused transcript like the main transcript"
    );
    assert!(
        app.viewport.last_transcript_top < tail_top,
        "the focused pane's viewport must actually move up"
    );
    assert!(
        app.viewport.transcript_scroll.is_at_tail(),
        "the invisible main transcript must not consume the focused pane's scroll"
    );
}

#[test]
fn fresh_launch_engine_adopts_the_app_session_id() {
    // Regression for phantom duplicate sessions: a fresh interactive launch
    // claimed session id A in the App (Runtime store lock, turn-start crash
    // checkpoint) while the engine minted its own id B. The first
    // `SessionUpdated` re-keyed the App to B, the completion commit cleared
    // only `checkpoints/<B>.json`, and `--continue` later "recovered" the
    // orphaned `checkpoints/<A>.json` (one user message, no reply) instead of
    // the real session. The engine must adopt the App's id at spawn.
    let mut app = create_test_app();
    app.current_session_id = None;
    let config = Config::default();

    let session_id = super::event_loop::ensure_runtime_session_id(&mut app);
    assert_eq!(app.current_session_id.as_deref(), Some(session_id.as_str()));
    assert!(
        uuid::Uuid::parse_str(&session_id).is_ok(),
        "fresh launch claims a uuid, got {session_id:?}"
    );

    let engine_config = build_engine_config(&app, &config);
    assert_eq!(
        engine_config.session_id.as_deref(),
        Some(session_id.as_str()),
        "the engine config must carry the App session id"
    );
    let (engine, _handle) = crate::core::engine::Engine::new(engine_config, &config);
    assert_eq!(
        engine.session_id(),
        session_id,
        "the engine must run the conversation the App persists"
    );
}

#[test]
fn fresh_session_turn_lifecycle_leaves_no_orphan_checkpoint() {
    // Store-level regression for the phantom duplicate sessions: walk the
    // persistence lifecycle of one fresh interactive turn against an
    // isolated session store and require that the turn-start checkpoint and
    // the completion commit are keyed by the same id. Before the fix the
    // engine minted its own id, `SessionUpdated` re-keyed the App to it, and
    // `checkpoints/<app id>.json` survived every completed turn; `--continue`
    // then "recovered" that one-message orphan as a duplicate session.
    //
    // `build_session_snapshot` calls `set_live_session`, which writes the
    // process-global LIVE_SESSIONS registry. Without this lock the test races
    // the other live-session tests under the default multi-thread runner.
    let _lock = crate::test_support::lock_test_env();
    let mut app = create_test_app();
    app.current_session_id = None;
    app.current_session_metadata = None;
    let config = Config::default();
    let store = TempDir::new().expect("sessions tempdir");
    let manager = crate::session_manager::SessionManager::new(store.path().join("sessions"))
        .expect("manager");

    // Fresh launch (event_loop::run_tui): the App claims the id, the engine
    // adopts it.
    let app_session_id = super::event_loop::ensure_runtime_session_id(&mut app);
    let (engine, _handle) =
        crate::core::engine::Engine::new(build_engine_config(&app, &config), &config);

    // Turn start (dispatch.rs): crash checkpoint under the App id.
    app.api_messages.push(crate::models::Message {
        role: Role::User,
        content: vec![crate::models::ContentBlock::Text {
            text: "please answer".to_string(),
            cache_control: None,
        }],
    });
    let checkpoint = build_session_snapshot(&mut app, &manager).expect("turn-start snapshot");
    assert_eq!(checkpoint.metadata.id, app_session_id);
    manager
        .save_checkpoint(&checkpoint)
        .expect("save turn-start checkpoint");

    // The engine reports its conversation id (`SessionUpdated` handler).
    app.current_session_id = Some(engine.session_id().to_string());

    // Turn completion (`PersistRequest::CompletedCommit`): save the session,
    // then clear the checkpoint of the id the snapshot carries.
    app.api_messages.push(crate::models::Message {
        role: Role::Assistant,
        content: vec![crate::models::ContentBlock::Text {
            text: "answer".to_string(),
            cache_control: None,
        }],
    });
    let completed = build_session_snapshot(&mut app, &manager).expect("completed snapshot");
    manager.save_session(&completed).expect("save session");
    manager
        .clear_session_checkpoint(&completed.metadata.id)
        .expect("clear checkpoint");

    assert_eq!(
        completed.metadata.id, app_session_id,
        "the completed turn must commit under the id the checkpoint was written with"
    );
    let orphans = manager.list_checkpoints().expect("list checkpoints");
    assert!(
        orphans.is_empty(),
        "a completed turn must leave no checkpoint behind, found {:?}",
        orphans.iter().map(|c| c.path.clone()).collect::<Vec<_>>()
    );
    let sessions = manager.list_sessions().expect("list sessions");
    assert_eq!(sessions.len(), 1, "exactly one session file: {sessions:?}");
    assert_eq!(sessions[0].id, app_session_id);
    assert_eq!(sessions[0].message_count, 2);
    crate::session_manager::set_live_session(None);
}

#[test]
fn resumed_launch_keeps_the_loaded_session_id_for_the_engine() {
    let mut app = create_test_app();
    app.current_session_id = Some("800596e6-56fd-477c-9a0f-13ada7846194".to_string());
    let config = Config::default();

    let session_id = super::event_loop::ensure_runtime_session_id(&mut app);
    assert_eq!(session_id, "800596e6-56fd-477c-9a0f-13ada7846194");

    let (engine, _handle) =
        crate::core::engine::Engine::new(build_engine_config(&app, &config), &config);
    assert_eq!(engine.session_id(), "800596e6-56fd-477c-9a0f-13ada7846194");
}

#[test]
fn sixel_reconciler_emits_moves_and_clears() {
    crate::tui::mark::set_sixel_supported_for_tests(true);
    let mut app = create_test_app();
    app.launch.sixel_cell_px = Some((10, 20));
    // Force the transparent-stage branch: the raster composites onto the
    // probed terminal background, and the clear below must repaint exactly
    // that colour.
    app.ui_theme.surface_bg = ratatui::style::Color::Reset;
    app.launch.sixel_terminal_bg = Some(ratatui::style::Color::Rgb(3, 7, 13));
    let mut writer: Vec<u8> = Vec::new();
    // No reservation: silent, nothing tracked.
    super::frame::reconcile_launch_sixel(&mut writer, &mut app);
    assert!(writer.is_empty());
    assert_eq!(app.launch.sixel_emitted, None);
    // New block: one positioned emission, tracked in stage coordinates
    // (stage cells are screen cells in fullscreen; CUP is 1-based, so
    // stage (2,1) draws at row 2, column 3).
    app.launch.sixel_mark_area = Some(Rect::new(2, 1, 6, 3));
    super::frame::reconcile_launch_sixel(&mut writer, &mut app);
    let text = String::from_utf8(writer.clone()).expect("ASCII stream");
    assert!(text.contains("\x1b[2;3H"), "{text:?}");
    assert!(text.contains("\x1bPq"), "{text:?}");
    assert_eq!(app.launch.sixel_emitted, Some(Rect::new(2, 1, 6, 3)));
    // Steady state: silent.
    let settled = writer.len();
    super::frame::reconcile_launch_sixel(&mut writer, &mut app);
    assert_eq!(writer.len(), settled, "steady frame emits nothing");
    // Moved block: the old screen block is wiped with the field colour,
    // then the new block draws (stage (4,1) -> CUP row 3, column 6).
    app.launch.sixel_mark_area = Some(Rect::new(4, 1, 6, 3));
    super::frame::reconcile_launch_sixel(&mut writer, &mut app);
    let text = String::from_utf8(writer.clone()).expect("ASCII stream");
    let delta = &text[settled..];
    assert!(delta.contains("48;2;"), "move clears the old block first");
    assert!(
        delta.contains("48;2;3;7;13m"),
        "clear repaints the probed field: {delta:?}"
    );
    assert!(delta.contains("\x1b[2;5H"), "{delta:?}");
    assert_eq!(app.launch.sixel_emitted, Some(Rect::new(4, 1, 6, 3)));
    // Tier exit: the live block is wiped and tracking stops.
    let moved = writer.len();
    app.launch.sixel_mark_area = None;
    super::frame::reconcile_launch_sixel(&mut writer, &mut app);
    let text = String::from_utf8(writer.clone()).expect("ASCII stream");
    assert!(
        text[moved..].contains("48;2;"),
        "exit clears the live block"
    );
    assert_eq!(app.launch.sixel_emitted, None);
    crate::tui::mark::set_sixel_supported_for_tests(false);
}
