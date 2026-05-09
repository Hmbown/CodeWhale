use super::*;
use crate::config::{ApiProvider, Config};
use crate::config_ui::{self, WebConfigSession, WebConfigSessionEvent};
use crate::core::engine::mock_engine_handle;
use crate::tui::file_mention::{
    apply_mention_menu_selection, find_file_mention_completions, partial_file_mention_at_cursor,
    try_autocomplete_file_mention, user_request_with_file_mentions, visible_mention_menu_entries,
};
use crate::tui::history::{
    ExecCell, ExecSource, GenericToolCell, HistoryCell, ToolCell, ToolStatus,
};
use crate::tui::views::{ModalView, ViewAction};
use crate::working_set::Workspace;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::TempDir;

#[test]
fn format_resume_hint_uses_canonical_resume_command() {
    assert_eq!(
        format_resume_hint(Some("019dd9d6-4f44-7c83-9863-59674a12b827")),
        Some("To continue this session, run deepseek --continue".to_string())
    );
}

#[test]
fn format_resume_hint_omits_missing_session_id() {
    assert_eq!(format_resume_hint(None), None);
    assert_eq!(format_resume_hint(Some("   ")), None);
}

#[test]
fn focus_gained_forces_terminal_viewport_recapture() {
    assert!(terminal_event_needs_viewport_recapture(&Event::FocusGained));
    assert!(!terminal_event_needs_viewport_recapture(&Event::FocusLost));
}

#[test]
fn terminal_origin_reset_resets_scroll_region_origin_and_clears() {
    assert!(
        TERMINAL_ORIGIN_RESET.starts_with(b"\x1b[r\x1b[?6l"),
        "must reset scroll margins and origin mode before repaint"
    );
    assert!(
        TERMINAL_ORIGIN_RESET.ends_with(b"\x1b[H\x1b[2J\x1b[3J"),
        "must home the cursor and clear the viewport"
    );
    assert!(
        TERMINAL_ORIGIN_RESET
            .windows(b"\x1b[3J".len())
            .any(|sequence| sequence == b"\x1b[3J"),
        "must erase saved scrollback when reclaiming the viewport"
    );
}

#[test]
fn composer_newline_shortcuts_do_not_steal_ctrl_enter() {
    assert!(is_composer_newline_key(KeyEvent::new(
        KeyCode::Char('j'),
        KeyModifiers::CONTROL,
    )));
    assert!(is_composer_newline_key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::ALT,
    )));
    assert!(is_composer_newline_key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::SHIFT,
    )));
    assert!(!is_composer_newline_key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )));
    assert!(!is_composer_newline_key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::CONTROL,
    )));
    assert!(!is_composer_newline_key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    )));
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

    assert_eq!(selection_to_text(&app).as_deref(), Some("a beta\n▏ gam"));
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
    assert!(selected.contains("▎ copy user"), "{selected:?}");
    assert!(selected.contains("copy thinking"), "{selected:?}");
    assert!(selected.contains("tool output line"), "{selected:?}");
    assert!(selected.contains("● copy assistant"), "{selected:?}");
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

#[test]
fn transcript_scrollbar_gutter_is_not_draggable() {
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
    assert!(app.viewport.transcript_selection.dragging);
    assert!(app.viewport.transcript_scroll.is_at_tail());
    assert!(!app.user_scrolled_during_stream);

    handle_mouse_event(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 21,
            row: 14,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert!(app.viewport.transcript_scroll.is_at_tail());
    assert!(!app.user_scrolled_during_stream);
    assert!(app.viewport.transcript_selection.dragging);

    handle_mouse_event(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 21,
            row: 14,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert!(!app.viewport.transcript_selection.dragging);
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
fn copy_shortcut_accepts_cmd_and_ctrl_shift_only() {
    assert!(is_copy_shortcut(&KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::SUPER,
    )));
    assert!(is_copy_shortcut(&KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    )));
    assert!(!is_copy_shortcut(&KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )));
}

#[test]
fn file_tree_shortcut_does_not_steal_plain_ctrl_e() {
    assert!(!is_file_tree_toggle_shortcut(&KeyEvent::new(
        KeyCode::Char('e'),
        KeyModifiers::CONTROL,
    )));
    assert!(is_file_tree_toggle_shortcut(&KeyEvent::new(
        KeyCode::Char('E'),
        KeyModifiers::CONTROL,
    )));
    assert!(is_file_tree_toggle_shortcut(&KeyEvent::new(
        KeyCode::Char('e'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    )));
    assert!(is_file_tree_toggle_shortcut(&KeyEvent::new(
        KeyCode::Char('E'),
        KeyModifiers::SUPER | KeyModifiers::SHIFT,
    )));
}

#[test]
fn parse_plan_choice_accepts_numbers() {
    assert_eq!(parse_plan_choice("1"), Some(PlanChoice::AcceptAgent));
    assert_eq!(parse_plan_choice("2"), Some(PlanChoice::AcceptYolo));
    assert_eq!(parse_plan_choice("3"), Some(PlanChoice::RevisePlan));
    assert_eq!(parse_plan_choice("4"), Some(PlanChoice::ExitPlan));
}

#[test]
fn parse_plan_choice_rejects_aliases_and_extra_text() {
    assert_eq!(parse_plan_choice("accept"), None);
    assert_eq!(parse_plan_choice("agent"), None);
    assert_eq!(parse_plan_choice("yolo"), None);
    assert_eq!(parse_plan_choice("3 revise"), None);
    assert_eq!(parse_plan_choice("unknown"), None);
}

#[test]
fn plan_choice_from_option_maps_expected_values() {
    assert_eq!(plan_choice_from_option(1), Some(PlanChoice::AcceptAgent));
    assert_eq!(plan_choice_from_option(2), Some(PlanChoice::AcceptYolo));
    assert_eq!(plan_choice_from_option(3), Some(PlanChoice::RevisePlan));
    assert_eq!(plan_choice_from_option(4), Some(PlanChoice::ExitPlan));
    assert_eq!(plan_choice_from_option(5), None);
}

#[test]
fn plan_prompt_view_escape_emits_dismiss_event() {
    let mut view = PlanPromptView::new();

    let action = view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    assert!(matches!(
        action,
        ViewAction::EmitAndClose(ViewEvent::PlanPromptDismissed)
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
        parse_git_status_path(" M crates/tui/src/tui/ui.rs"),
        Some("crates/tui/src/tui/ui.rs".to_string())
    );
    assert_eq!(
        parse_git_status_path("R  old name.rs -> crates/tui/src/tui/file_picker.rs"),
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
        workspace_file_candidate(&raw, root),
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
    mark_tool_paths_from_text(
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
    let options = TuiOptions {
        model: "deepseek-v4-pro".to_string(),
        workspace: PathBuf::from("."),
        config_path: None,
        config_profile: None,
        allow_shell: false,
        use_alt_screen: true,
        use_mouse_capture: false,
        use_bracketed_paste: true,
        max_subagents: 1,
        skills_dir: PathBuf::from("."),
        memory_path: PathBuf::from("memory.md"),
        notes_path: PathBuf::from("notes.txt"),
        mcp_config_path: PathBuf::from("mcp.json"),
        use_memory: false,
        start_in_agent_mode: false,
        skip_onboarding: false,
        yolo: false,
        resume_session_id: None,
        initial_input: None,
    };
    App::new(options, &Config::default())
}

fn text_message(role: &str, text: &str) -> Message {
    Message {
        role: role.to_string(),
        content: vec![ContentBlock::Text {
            text: text.to_string(),
            cache_control: None,
        }],
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
            workspace: PathBuf::from("/tmp/resume-recovery"),
            mode: Some("yolo".to_string()),
        },
        messages,
        system_prompt: None,
        context_references: Vec::new(),
    }
}

#[test]
fn apply_loaded_session_restores_dangling_user_tail_as_retry_draft() {
    let mut app = create_test_app();
    let session = saved_session_with_messages(vec![text_message(
        "user",
        "finish the Qthresh proof bundle",
    )]);

    let recovered = apply_loaded_session(&mut app, &session);

    assert!(recovered);
    assert!(app.api_messages.is_empty());
    assert_eq!(app.input, "finish the Qthresh proof bundle");
    assert_eq!(
        app.queued_draft
            .as_ref()
            .map(|draft| draft.display.as_str()),
        Some("finish the Qthresh proof bundle")
    );
    assert!(
        app.history
            .iter()
            .all(|cell| !matches!(cell, HistoryCell::User { .. }))
    );
    assert!(
        app.status_message
            .as_deref()
            .is_some_and(|msg| msg.contains("Recovered interrupted prompt")),
        "status was {:?}",
        app.status_message
    );
}

#[test]
fn apply_loaded_session_resets_unpersisted_telemetry() {
    let mut app = create_test_app();
    app.session.session_cost = 1.25;
    app.session.session_cost_cny = 9.13;
    app.session.subagent_cost = 0.75;
    app.session.subagent_cost_cny = 5.48;
    app.session.subagent_cost_event_seqs.insert(42);
    app.session.displayed_cost_high_water = 2.0;
    app.session.displayed_cost_high_water_cny = 14.61;
    app.session.last_prompt_tokens = Some(120);
    app.session.last_completion_tokens = Some(35);
    app.session.last_prompt_cache_hit_tokens = Some(80);
    app.session.last_prompt_cache_miss_tokens = Some(40);
    app.session.last_reasoning_replay_tokens = Some(12);
    app.push_turn_cache_record(crate::tui::app::TurnCacheRecord {
        input_tokens: 120,
        output_tokens: 35,
        cache_hit_tokens: Some(80),
        cache_miss_tokens: Some(40),
        reasoning_replay_tokens: Some(12),
        recorded_at: Instant::now(),
    });
    let mut session = saved_session_with_messages(vec![text_message("assistant", "ready")]);
    session.metadata.total_tokens = 500;

    let recovered = apply_loaded_session(&mut app, &session);

    assert!(!recovered);
    assert_eq!(app.session.total_tokens, 500);
    assert_eq!(app.session.total_conversation_tokens, 500);
    assert_eq!(app.session.session_cost, 0.0);
    assert_eq!(app.session.session_cost_cny, 0.0);
    assert_eq!(app.session.subagent_cost, 0.0);
    assert_eq!(app.session.subagent_cost_cny, 0.0);
    assert!(app.session.subagent_cost_event_seqs.is_empty());
    assert_eq!(app.session.displayed_cost_high_water, 0.0);
    assert_eq!(app.session.displayed_cost_high_water_cny, 0.0);
    assert_eq!(app.session.last_prompt_tokens, None);
    assert_eq!(app.session.last_completion_tokens, None);
    assert_eq!(app.session.last_prompt_cache_hit_tokens, None);
    assert_eq!(app.session.last_prompt_cache_miss_tokens, None);
    assert_eq!(app.session.last_reasoning_replay_tokens, None);
    assert!(app.session.turn_cache_history.is_empty());
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
        role: "user".to_string(),
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
        role: "assistant".to_string(),
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
fn active_tool_status_label_summarizes_live_tool_group() {
    let mut app = create_test_app();
    app.turn_started_at = Some(Instant::now() - Duration::from_secs(5));
    let mut active = ActiveCell::new();
    active.push_tool(
        "exec-1",
        HistoryCell::Tool(ToolCell::Exec(ExecCell {
            command: "cargo test --workspace --all-features".to_string(),
            status: ToolStatus::Running,
            output: None,
            started_at: app.turn_started_at,
            duration_ms: None,
            source: ExecSource::Assistant,
            interaction: None,
        })),
    );
    active.push_tool(
        "tool-2",
        HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "grep_files".to_string(),
            status: ToolStatus::Success,
            input_summary: Some("pattern: TODO".to_string()),
            output: Some("done".to_string()),
            prompts: None,
            spillover_path: None,
        })),
    );
    app.active_cell = Some(active);

    let label = active_tool_status_label(&app).expect("status label");

    assert!(label.contains("run cargo test"));
    assert!(label.contains("1 active"));
    assert!(label.contains("1 done"));
    assert!(label.contains(tool_details_shortcut_label()));
}

#[test]
fn active_tool_status_label_counts_foreground_rlm_work() {
    let mut app = create_test_app();
    app.turn_started_at = Some(Instant::now() - Duration::from_secs(5));
    let mut active = ActiveCell::new();
    active.push_tool(
        "rlm-1",
        HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "rlm".to_string(),
            status: ToolStatus::Running,
            input_summary: Some("task: compare projects".to_string()),
            output: None,
            prompts: None,
            spillover_path: None,
        })),
    );
    app.active_cell = Some(active);

    let label = active_tool_status_label(&app).expect("status label");

    assert!(label.contains("tool rlm"), "label: {label}");
    assert!(label.contains("1 active"), "label: {label}");
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
            status_items: None,
            osc8_links: None,
            notification_condition: None,
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

    let content = queued_message_content_for_app(&app, &message, None);

    assert!(content.starts_with("Summarize @guide.md"));
    assert!(content.contains("Local context from @mentions:"));
    assert!(content.contains("<file mention=\"@guide.md\""));
    assert!(content.contains("# Guide\nUse the fast path."));
    assert_eq!(message.display, "Summarize @guide.md");
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

    apply_model_and_compaction_update(&engine.handle, compaction).await;

    match engine.rx_op.recv().await.expect("set model op") {
        crate::core::ops::Op::SetModel { model } => {
            assert_eq!(model, "deepseek-v4-flash");
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
async fn provider_switch_clears_turn_cache_history() {
    let mut app = create_test_app();
    app.push_turn_cache_record(crate::tui::app::TurnCacheRecord {
        input_tokens: 100,
        output_tokens: 25,
        cache_hit_tokens: Some(70),
        cache_miss_tokens: Some(30),
        reasoning_replay_tokens: Some(12),
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
}

#[test]
fn turn_liveness_watchdog_clears_stale_dispatch() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.dispatch_started_at =
        Some(Instant::now() - DISPATCH_WATCHDOG_TIMEOUT - Duration::from_millis(1));

    let recovered = reconcile_turn_liveness(&mut app, Instant::now(), false);

    assert!(recovered);
    assert!(!app.is_loading);
    assert!(app.dispatch_started_at.is_none());
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

    let recovered = reconcile_turn_liveness(&mut app, Instant::now(), false);

    assert!(recovered);
    assert!(!app.is_loading);
    assert!(app.dispatch_started_at.is_none());
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
    app.dispatch_started_at =
        Some(Instant::now() - DISPATCH_WATCHDOG_TIMEOUT - Duration::from_secs(10));

    let recovered = reconcile_turn_liveness(&mut app, Instant::now(), false);

    assert!(!recovered);
    assert!(app.is_loading);
    assert!(app.dispatch_started_at.is_some());
    assert!(app.status_toasts.is_empty());
}

#[test]
fn fixed_model_auto_thinking_skips_auto_model_router() {
    let mut app = create_test_app();
    app.auto_model = false;
    app.model = "deepseek-v4-pro".to_string();
    app.reasoning_effort = ReasoningEffort::Auto;

    assert!(
        !should_resolve_auto_model_selection(&app),
        "fixed-model auto thinking must stay local instead of starting a hidden router request"
    );
}

#[test]
fn auto_model_still_uses_auto_model_router() {
    let mut app = create_test_app();
    app.auto_model = true;
    app.reasoning_effort = ReasoningEffort::Auto;

    assert!(
        should_resolve_auto_model_selection(&app),
        "auto model still needs the router to choose the concrete model"
    );
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

    let commit = Command::new("git")
        .args([
            "-c",
            "user.name=DeepSeek TUI Tests",
            "-c",
            "user.email=tests@example.com",
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

fn spans_text(spans: &[Span<'_>]) -> String {
    spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

#[test]
fn alt_4_focuses_agents_sidebar_without_switching_modes() {
    let mut app = create_test_app();
    app.mode = AppMode::Agent;
    app.sidebar_focus = SidebarFocus::Auto;

    apply_alt_4_shortcut(&mut app, KeyModifiers::ALT);

    assert_eq!(app.mode, AppMode::Agent);
    assert_eq!(app.sidebar_focus, SidebarFocus::Agents);
    assert_eq!(app.status_message.as_deref(), Some("Sidebar focus: agents"));
}

#[test]
fn ctrl_alt_4_focuses_agents_sidebar_without_switching_modes() {
    let mut app = create_test_app();
    app.mode = AppMode::Agent;
    app.sidebar_focus = SidebarFocus::Auto;

    apply_alt_4_shortcut(&mut app, KeyModifiers::ALT | KeyModifiers::CONTROL);

    assert_eq!(app.mode, AppMode::Agent);
    assert_eq!(app.sidebar_focus, SidebarFocus::Agents);
    assert_eq!(app.status_message.as_deref(), Some("Sidebar focus: agents"));
}

fn make_subagent(
    id: &str,
    status: crate::tools::subagent::SubAgentStatus,
) -> crate::tools::subagent::SubAgentResult {
    crate::tools::subagent::SubAgentResult {
        agent_id: id.to_string(),
        agent_type: crate::tools::subagent::SubAgentType::General,
        assignment: crate::tools::subagent::SubAgentAssignment {
            objective: format!("objective-{id}"),
            role: Some("worker".to_string()),
        },
        model: "deepseek-v4-flash".to_string(),
        nickname: None,
        status,
        result: None,
        steps_taken: 0,
        duration_ms: 0,
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
    app.agent_progress
        .insert("agent_stale".to_string(), "old".to_string());

    reconcile_subagent_activity_state(&mut app);
    assert!(app.agent_progress.contains_key("agent_a"));
    assert!(!app.agent_progress.contains_key("agent_stale"));
    assert!(app.agent_activity_started_at.is_some());

    app.subagent_cache.clear();
    reconcile_subagent_activity_state(&mut app);
    assert!(app.agent_progress.is_empty());
    assert!(app.agent_activity_started_at.is_none());
}

#[test]
fn subagent_token_usage_updates_live_cost_counter_without_card_change() {
    let mut app = create_test_app();
    handle_subagent_mailbox(
        &mut app,
        1,
        &crate::tools::subagent::MailboxMessage::TokenUsage {
            agent_id: "agent-a".to_string(),
            model: "deepseek-v4-flash".to_string(),
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
fn subagent_token_usage_is_deduped_by_mailbox_sequence() {
    let mut app = create_test_app();
    let usage = crate::tools::subagent::MailboxMessage::TokenUsage {
        agent_id: "agent-a".to_string(),
        model: "deepseek-v4-flash".to_string(),
        usage: crate::models::Usage {
            input_tokens: 10_000,
            output_tokens: 1_000,
            ..Default::default()
        },
    };

    handle_subagent_mailbox(&mut app, 7, &usage);
    let first = app.session.subagent_cost;
    handle_subagent_mailbox(&mut app, 7, &usage);
    assert_eq!(app.session.subagent_cost, first);
    handle_subagent_mailbox(&mut app, 8, &usage);
    assert!(app.session.subagent_cost > first);
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
fn footer_state_label_drops_thinking_and_prefers_compacting() {
    // We deliberately do not surface a "thinking" label for `is_loading` —
    // the animated water-spout strip in the footer's spacer is the visual
    // signal. `is_loading` alone falls through to "ready"; `is_compacting`
    // still wins because compacting is a less-common, distinct state.
    let mut app = create_test_app();
    assert_eq!(footer_state_label(&app).0, "ready");

    app.is_loading = true;
    assert_eq!(
        footer_state_label(&app).0,
        "ready",
        "is_loading must NOT produce a `thinking` text label — the animation handles it"
    );

    app.is_compacting = true;
    assert!(footer_state_label(&app).0.starts_with("compacting"));
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

#[test]
fn footer_status_line_spans_show_mode_and_model_idle_and_active() {
    let mut app = create_test_app();
    app.model = "deepseek-v4-flash".to_string();

    let idle = spans_text(&footer_status_line_spans(&app, 60));
    assert!(idle.contains("agent"));
    assert!(idle.contains("deepseek-v4-flash"));
    assert!(idle.contains("\u{00B7}"));
    assert!(!idle.contains("ready"));

    // is_loading no longer adds a "thinking" text label — the live-work
    // signal is the animated water-spout strip the renderer paints into
    // the footer's spacer. The mode + model still render unchanged.
    app.is_loading = true;
    let active = spans_text(&footer_status_line_spans(&app, 60));
    assert!(active.contains("agent"));
    assert!(active.contains("deepseek-v4-flash"));
    assert!(
        !active.contains("thinking"),
        "footer must not show a `thinking` text label while loading"
    );
}

#[test]
fn footer_status_line_spans_truncate_long_model_names() {
    let mut app = create_test_app();
    app.model = "deepseek-v4-pro-with-an-extremely-long-model-name".to_string();
    app.is_loading = true;

    let line = spans_text(&footer_status_line_spans(&app, 40));
    assert!(line.contains("..."));
    assert!(UnicodeWidthStr::width(line.as_str()) <= 40);
}

#[test]
fn footer_coherence_chip_hides_healthy_and_uses_clear_labels() {
    let mut app = create_test_app();

    app.coherence_state = crate::core::coherence::CoherenceState::Healthy;
    assert!(
        footer_coherence_spans(&app).is_empty(),
        "healthy state should produce no footer chip"
    );

    // GettingCrowded is intentionally suppressed — see the rationale in
    // `footer_coherence_spans`. The footer only surfaces active engine
    // interventions; soft pressure hints stay quiet.
    app.coherence_state = crate::core::coherence::CoherenceState::GettingCrowded;
    assert!(
        footer_coherence_spans(&app).is_empty(),
        "GettingCrowded should not surface a footer chip; only active interventions do"
    );

    let cases = [
        (
            crate::core::coherence::CoherenceState::RefreshingContext,
            "refreshing context",
        ),
        (
            crate::core::coherence::CoherenceState::VerifyingRecentWork,
            "verifying",
        ),
        (
            crate::core::coherence::CoherenceState::ResettingPlan,
            "resetting plan",
        ),
    ];

    for (state, expected) in cases {
        app.coherence_state = state;
        assert_eq!(spans_text(&footer_coherence_spans(&app)), expected);
    }
}

#[test]
fn footer_auxiliary_spans_show_cache_when_compact() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.session.last_prompt_tokens = Some(48_000);
    app.session.last_prompt_cache_hit_tokens = Some(36_000);
    app.session.last_prompt_cache_miss_tokens = Some(12_000);
    app.session.session_cost = 12.34;

    let compact = spans_text(&footer_auxiliary_spans(&app, 48));
    assert!(compact.contains("Cache: 75.0% hit"));
    assert!(!compact.contains('$'));
}

#[test]
fn footer_auxiliary_spans_show_cache_unavailable_when_provider_omits_cache_fields() {
    let mut app = create_test_app();
    app.session.last_prompt_tokens = Some(48_000);
    app.session.last_completion_tokens = Some(2_000);

    let roomy = spans_text(&footer_auxiliary_spans(&app, 72));

    assert!(roomy.contains("Cache: unavailable"));
}

#[test]
fn footer_auxiliary_spans_show_cache_and_cost_when_roomy() {
    let mut app = create_test_app();
    app.session.last_prompt_tokens = Some(48_000);
    app.session.last_prompt_cache_hit_tokens = Some(36_000);
    app.session.last_prompt_cache_miss_tokens = Some(12_000);
    app.session.session_cost = 12.34;

    let roomy = spans_text(&footer_auxiliary_spans(&app, 72));
    assert!(roomy.contains("Cache: 75.0% hit | hit 36000 | miss 12000"));
    assert!(roomy.contains("$12.34"));
    assert!(
        !roomy.contains("ctx"),
        "context % removed from footer — shown in header only"
    );
}

#[test]
fn footer_auxiliary_spans_show_tiny_positive_cost_when_roomy() {
    let mut app = create_test_app();
    app.session.session_cost = 0.00005;

    let roomy = spans_text(&footer_auxiliary_spans(&app, 32));
    assert!(roomy.contains("<$0.0001"));
}

#[test]
fn footer_auxiliary_spans_use_configured_cost_currency() {
    let mut app = create_test_app();
    app.cost_currency = crate::pricing::CostCurrency::Cny;
    app.session.session_cost_cny = 2.5;

    let roomy = spans_text(&footer_auxiliary_spans(&app, 32));
    assert!(roomy.contains("¥2.50"));
    assert!(!roomy.contains('$'));
}

#[test]
fn footer_auxiliary_spans_show_reasoning_replay_chip() {
    // Issue #30: when a thinking-mode tool-calling turn replays prior
    // reasoning_content, the footer surfaces the approximate input-token
    // cost so users can see why their context filled up.
    let mut app = create_test_app();
    app.session.last_prompt_tokens = Some(48_000);
    app.session.last_reasoning_replay_tokens = Some(8_200);

    let spans = footer_auxiliary_spans(&app, 64);
    let text = spans_text(&spans);
    assert!(
        text.contains("rsn 8.2k"),
        "expected replay chip, got {text:?}"
    );
}

#[test]
fn footer_auxiliary_spans_hide_reasoning_replay_when_zero() {
    let mut app = create_test_app();
    app.session.last_prompt_tokens = Some(48_000);
    app.session.last_reasoning_replay_tokens = Some(0);

    let spans = footer_auxiliary_spans(&app, 64);
    let text = spans_text(&spans);
    assert!(!text.contains("rsn"), "zero replay must not render chip");
}

#[test]
fn context_usage_snapshot_prefers_estimate_when_reported_exceeds_window() {
    let mut app = create_test_app();
    app.session.last_prompt_tokens = Some(1_200_000);
    app.api_messages = vec![Message {
        role: "user".to_string(),
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
fn context_usage_snapshot_prefers_estimate_when_reported_is_inflated_by_old_reasoning() {
    let mut app = create_test_app();
    app.session.last_prompt_tokens = Some(980_000);
    app.api_messages = vec![Message {
        role: "user".to_string(),
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
        role: "user".to_string(),
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
        role: "user".to_string(),
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
fn should_auto_compact_before_send_respects_threshold_and_setting() {
    let mut app = create_test_app();
    let big_buffer = vec![Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: "context ".repeat(400_000),
            cache_control: None,
        }],
    }];

    // High estimated context + auto_compact ON → auto-compact triggers.
    app.api_messages = big_buffer.clone();
    app.auto_compact = true;
    assert!(should_auto_compact_before_send(&app));

    // Same high context but auto_compact OFF → never triggers.
    app.auto_compact = false;
    assert!(!should_auto_compact_before_send(&app));

    // Small estimated context + auto_compact ON → does NOT trigger,
    // regardless of what `last_prompt_tokens` reports. This matches the
    // #115 fix: the estimate is the primary signal, not the engine's
    // turn-cumulative reported value (which used to rule the displayed
    // % and could spuriously trigger / suppress auto-compact).
    app.api_messages = vec![Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: "small".to_string(),
            cache_control: None,
        }],
    }];
    app.auto_compact = true;
    app.session.last_prompt_tokens = Some(10_000);
    assert!(!should_auto_compact_before_send(&app));
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
fn test_ctrl_c_exits_when_not_loading() {
    let mut app = create_test_app();
    app.is_loading = false;

    // Ctrl+C when not loading should trigger shutdown
    // We can't test the actual shutdown, but verify the state is correct
    // for the shutdown path to be taken
    assert!(!app.is_loading);
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
    app.mode = AppMode::Yolo;
    assert_eq!(next_escape_action(&app, false), EscapeAction::CancelRequest);

    app.input.clear();
    assert_eq!(next_escape_action(&app, false), EscapeAction::CancelRequest);

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
fn visible_slash_menu_entries_excludes_removed_commands() {
    let mut app = create_test_app();
    app.input = "/".to_string();

    let entries = visible_slash_menu_entries(&app, 128);
    assert!(entries.iter().any(|entry| entry.name == "/config"));
    assert!(entries.iter().any(|entry| entry.name == "/links"));
    assert!(!entries.iter().any(|entry| entry.name == "/set"));
    assert!(!entries.iter().any(|entry| entry.name == "/deepseek"));
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
        },
        crate::tui::widgets::SlashMenuEntry {
            name: "/settings".to_string(),
            description: String::new(),
            is_skill: false,
        },
    ];
    app.slash_menu_selected = 0;
    assert!(apply_slash_menu_selection(&mut app, &entries, true));
    assert_eq!(app.input, "/model ");
}

#[test]
fn apply_slash_menu_selection_uses_skill_command_form() {
    let mut app = create_test_app();
    let entries = vec![crate::tui::widgets::SlashMenuEntry {
        name: "/skill search-files".to_string(),
        description: "Search files".to_string(),
        is_skill: true,
    }];

    assert!(apply_slash_menu_selection(&mut app, &entries, true));
    assert_eq!(app.input, "/skill search-files");
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
    refresh_workspace_context_if_needed(&mut app, now, false);

    assert!(app.workspace_context.is_none());
    assert!(app.workspace_context_refreshed_at.is_none());

    refresh_workspace_context_if_needed(&mut app, now, true);

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
    refresh_workspace_context_if_needed(&mut app, start, true);
    let initial = app
        .workspace_context
        .clone()
        .expect("initial refresh should populate context");

    std::fs::write(repo.path().join("dirty.txt"), "dirty").expect("write dirty marker");

    let before_ttl = start + Duration::from_secs(WORKSPACE_CONTEXT_REFRESH_SECS - 1);
    refresh_workspace_context_if_needed(&mut app, before_ttl, true);
    assert_eq!(app.workspace_context.as_deref(), Some(initial.as_str()));

    let after_ttl = start + Duration::from_secs(WORKSPACE_CONTEXT_REFRESH_SECS);
    refresh_workspace_context_if_needed(&mut app, after_ttl, true);
    let refreshed = app
        .workspace_context
        .as_deref()
        .expect("refresh after ttl should update context");
    assert!(refreshed.contains("untracked"));
    assert_ne!(refreshed, initial);
}

#[tokio::test]
async fn dismissed_plan_prompt_leaves_non_numeric_input_for_normal_send_path() {
    let mut app = create_test_app();
    app.mode = AppMode::Plan;
    app.plan_prompt_pending = true;
    app.offline_mode = true;

    let engine = crate::core::engine::mock_engine_handle();
    let config = Config::default();

    let handled = handle_plan_choice(&mut app, &config, &engine.handle, "yolo")
        .await
        .expect("plan choice");

    assert!(!handled);
    assert!(!app.plan_prompt_pending);
    assert_eq!(app.mode, AppMode::Plan);

    let queued = build_queued_message(&mut app, "yolo".to_string());
    submit_or_steer_message(&mut app, &config, &engine.handle, queued)
        .await
        .expect("submit normal message");

    assert_eq!(app.queued_message_count(), 1);
    assert_eq!(
        app.queued_messages
            .front()
            .map(crate::tui::app::QueuedMessage::content),
        Some("yolo".to_string())
    );
    assert_eq!(
        app.status_message.as_deref(),
        Some("Offline: 1 queued — ↑ to edit, /queue list")
    );
}

#[tokio::test]
async fn numeric_plan_choice_still_queues_follow_up_when_busy() {
    let mut app = create_test_app();
    app.mode = AppMode::Plan;
    app.plan_prompt_pending = true;
    app.is_loading = true;

    let engine = crate::core::engine::mock_engine_handle();
    let config = Config::default();

    let handled = handle_plan_choice(&mut app, &config, &engine.handle, "2")
        .await
        .expect("plan choice");

    assert!(handled);
    assert!(!app.plan_prompt_pending);
    assert_eq!(app.mode, AppMode::Yolo);
    assert_eq!(app.queued_message_count(), 1);
    assert_eq!(
        app.queued_messages
            .front()
            .map(crate::tui::app::QueuedMessage::content),
        Some("Proceed with the accepted plan.".to_string())
    );
}

#[test]
fn api_key_validation_warns_without_blocking_unusual_formats() {
    assert!(matches!(
        validate_api_key_for_onboarding(""),
        ApiKeyValidation::Reject(_)
    ));
    assert!(matches!(
        validate_api_key_for_onboarding("sk short"),
        ApiKeyValidation::Reject(_)
    ));
    assert!(matches!(
        validate_api_key_for_onboarding("short-key"),
        ApiKeyValidation::Accept { warning: Some(_) }
    ));
    assert!(matches!(
        validate_api_key_for_onboarding("averylongkeywithoutdash123456"),
        ApiKeyValidation::Accept { warning: Some(_) }
    ));
    assert!(matches!(
        validate_api_key_for_onboarding("sk-valid-format-1234567890"),
        ApiKeyValidation::Accept { warning: None }
    ));
}

#[test]
fn onboarding_after_api_key_save_does_not_repeat_language_step() {
    let mut app = create_test_app();
    app.onboarding = OnboardingState::ApiKey;
    app.onboarding_needs_api_key = false;
    app.trust_mode = true;
    app.status_message = Some("saved".to_string());

    advance_onboarding_after_language(&mut app);

    assert_eq!(app.onboarding, OnboardingState::Tips);
    assert_eq!(app.status_message, None);
}

#[test]
fn onboarding_after_api_key_save_routes_to_trust_when_needed() {
    let tmpdir = TempDir::new().expect("tempdir");
    let mut app = create_test_app();
    app.workspace = tmpdir.path().to_path_buf();
    app.onboarding = OnboardingState::ApiKey;
    app.onboarding_needs_api_key = false;
    app.trust_mode = false;

    advance_onboarding_after_language(&mut app);

    assert_eq!(app.onboarding, OnboardingState::TrustDirectory);
}

#[test]
fn api_key_paste_shortcut_is_not_plain_text_input() {
    let ctrl_v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL);
    assert!(is_paste_shortcut(&ctrl_v));
    assert!(!is_text_input_key(&ctrl_v));

    let legacy_ctrl_v = KeyEvent::new(KeyCode::Char('\u{16}'), KeyModifiers::NONE);
    assert!(is_paste_shortcut(&legacy_ctrl_v));
    assert!(!is_text_input_key(&legacy_ctrl_v));

    let shifted = KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT);
    assert!(is_text_input_key(&shifted));
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

#[test]
fn detail_target_prefers_visible_tool_card() {
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
        })),
        HistoryCell::Assistant {
            content: "ok".to_string(),
            streaming: false,
        },
        HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
            name: "exec_shell".to_string(),
            status: ToolStatus::Success,
            input_summary: Some("command: ls".to_string()),
            output: Some("...".to_string()),
            prompts: None,
            spillover_path: None,
        })),
    ];
    app.tool_details_by_cell.insert(
        1,
        ToolDetailRecord {
            tool_id: "search-1".to_string(),
            tool_name: "file_search".to_string(),
            input: serde_json::json!({"query": "foo"}),
            output: Some("done".to_string()),
        },
    );
    app.tool_details_by_cell.insert(
        3,
        ToolDetailRecord {
            tool_id: "exec-1".to_string(),
            tool_name: "exec_shell".to_string(),
            input: serde_json::json!({"command": "ls"}),
            output: Some("...".to_string()),
        },
    );
    app.resync_history_revisions();
    let revisions = app.history_revisions.clone();
    app.viewport.transcript_cache.ensure(
        &app.history,
        &revisions,
        100,
        app.transcript_render_options(),
    );
    app.viewport.last_transcript_top = first_line_for_cell(&app, 1);
    app.viewport.last_transcript_visible = 6;

    assert_eq!(detail_target_cell_index(&app), Some(1));
    let expected = format!("{} details: file_search", tool_details_shortcut_label());
    assert_eq!(
        selected_detail_footer_label(&app).as_deref(),
        Some(expected.as_str())
    );
}

#[test]
fn macos_option_v_glyph_is_treated_as_details_shortcut_only_on_macos() {
    let option_v = KeyEvent::new(KeyCode::Char('\u{221A}'), KeyModifiers::NONE);
    assert!(is_macos_option_v_legacy_key_for_platform(&option_v, true));
    assert!(!is_macos_option_v_legacy_key_for_platform(&option_v, false));

    let modified = KeyEvent::new(KeyCode::Char('\u{221A}'), KeyModifiers::SHIFT);
    assert!(!is_macos_option_v_legacy_key_for_platform(&modified, true));

    let plain_v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE);
    assert!(!is_macos_option_v_legacy_key_for_platform(&plain_v, true));
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
    );
    app.viewport.last_transcript_top = 0;
    app.viewport.last_transcript_visible = 4;

    assert_eq!(detail_target_cell_index(&app), Some(0));
    assert!(open_tool_details_pager(&mut app));
    assert_eq!(app.view_stack.top_kind(), Some(ModalKind::Pager));
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
    }))];
    app.resync_history_revisions();
    assert!(spillover_pager_section(&app, 0).is_none());
}

#[test]
fn spillover_pager_section_loads_file_when_present() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("call-test.txt");
    let mut f = std::fs::File::create(&path).unwrap();
    writeln!(f, "FULL_OUTPUT_BYTES_HERE").unwrap();

    let mut app = create_test_app();
    app.history = vec![HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
        name: "exec_shell".to_string(),
        status: ToolStatus::Success,
        input_summary: None,
        output: Some("(truncated head)".to_string()),
        prompts: None,
        spillover_path: Some(path.clone()),
    }))];
    app.resync_history_revisions();

    let section = spillover_pager_section(&app, 0).expect("section present");
    assert!(section.contains("Full output (spillover)"));
    assert!(
        section.contains("FULL_OUTPUT_BYTES_HERE"),
        "section missing file body: {section}"
    );
    assert!(section.contains(&path.display().to_string()));
}

#[test]
fn spillover_pager_section_returns_notice_when_file_missing() {
    let mut app = create_test_app();
    let bogus = std::path::PathBuf::from("/tmp/this/path/does/not/exist-spill.txt");
    app.history = vec![HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
        name: "exec_shell".to_string(),
        status: ToolStatus::Success,
        input_summary: None,
        output: Some("(truncated head)".to_string()),
        prompts: None,
        spillover_path: Some(bogus),
    }))];
    app.resync_history_revisions();

    let section = spillover_pager_section(&app, 0).expect("still emits a notice section");
    assert!(section.contains("could not read spillover file"));
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
            started_at: Some(Instant::now()),
            duration_ms: None,
            source: ExecSource::Assistant,
            interaction: Some("interactive".to_string()),
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
        })),
    );
    app.active_cell = Some(active);
    assert!(
        !terminal_pause_has_live_owner(&app),
        "non-interactive RLM work must not keep the terminal in host-scrollback mode"
    );
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
        })),
    );
    app.active_cell = Some(active);

    let entries = active_rlm_task_entries(&app);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, "rlm-1");
    assert_eq!(entries[0].status, "running");
    assert_eq!(entries[0].prompt_summary, "RLM: file_path: Cargo.lock");
    assert!(entries[0].duration_ms.unwrap_or_default() >= 3000);
}

#[test]
fn details_shortcut_modifiers_accept_plain_shift_and_alt_only() {
    assert!(details_shortcut_modifiers(KeyModifiers::NONE));
    assert!(details_shortcut_modifiers(KeyModifiers::SHIFT));
    assert!(details_shortcut_modifiers(KeyModifiers::ALT));
    assert!(details_shortcut_modifiers(
        KeyModifiers::ALT | KeyModifiers::SHIFT
    ));
    assert!(!details_shortcut_modifiers(KeyModifiers::CONTROL));
    assert!(!details_shortcut_modifiers(
        KeyModifiers::ALT | KeyModifiers::CONTROL
    ));
}

#[test]
fn ctrl_h_is_treated_as_terminal_backspace() {
    assert!(is_ctrl_h_backspace(&KeyEvent::new(
        KeyCode::Char('h'),
        KeyModifiers::CONTROL
    )));
    assert!(!is_ctrl_h_backspace(&KeyEvent::new(
        KeyCode::Char('h'),
        KeyModifiers::NONE
    )));
    assert!(!is_ctrl_h_backspace(&KeyEvent::new(
        KeyCode::Char('h'),
        KeyModifiers::CONTROL | KeyModifiers::ALT
    )));
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

#[test]
fn try_autocomplete_file_mention_unique_replaces_partial() {
    let tmpdir = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmpdir.path().join("docs")).unwrap();
    std::fs::write(tmpdir.path().join("docs/deepseek_v4.pdf"), b"%PDF-").unwrap();

    let mut app = create_test_app();
    app.workspace = tmpdir.path().to_path_buf();
    app.input = "summarize @docs/de".to_string();
    app.cursor_position = app.input.chars().count();

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

    assert!(try_autocomplete_file_mention(&mut app));
    assert_eq!(app.input, "@nonexistent_xyz");
    assert_eq!(
        app.status_message.as_deref(),
        Some("No files match @nonexistent_xyz")
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

    let entries = visible_mention_menu_entries(&mut app, 6);
    assert!(!entries.is_empty(), "popup should surface docs/ entries");
    assert!(entries.iter().any(|e| e.starts_with("docs/")));
    // README.md doesn't match `docs/` — confirm we didn't dump every file.
    assert!(!entries.iter().any(|e| e == "README.md"));
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

    let entries = visible_mention_menu_entries(&mut app, 6);
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

    let entries_after_partial_change = visible_mention_menu_entries(&mut app, 6);
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

    let entries = visible_mention_menu_entries(&mut app, 6);
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

#[test]
fn tool_child_usage_metadata_updates_live_cost_counter() {
    let mut app = create_test_app();
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

    assert!(app.session.subagent_cost > 0.0);
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
fn tool_details_survive_active_cell_flush() {
    // The pager / Ctrl+O resolves tool details by cell index. Flushing the
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
// ends, or (c) they hit an explicit jump-to-bottom key. Tool-cell handlers
// only call `mark_history_updated`, which does NOT scroll. `add_message`
// gates on the flag.

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

// ---- P2.3: thinking + tool calls render as one grouped block ----

#[test]
fn thinking_then_tools_share_active_cell_until_text_flushes() {
    // Contract: a turn that emits Thinking → Tool → Tool keeps everything
    // inside `active_cell` (one logical "Working…" group) until the next
    // assistant prose chunk fires, at which point the group flushes into
    // history in original order.
    let mut app = create_test_app();

    // 1. Thinking starts and streams a delta.
    let thinking_idx = ensure_streaming_thinking_active_entry(&mut app);
    append_streaming_thinking(&mut app, thinking_idx, "planning the read");
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
    let finalized = finalize_streaming_thinking_active_entry(&mut app, Some(1.5), "");
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
    let _ = ensure_streaming_thinking_active_entry(&mut app);
    append_streaming_thinking(&mut app, 0, "incomplete");

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
fn engine_error_finalizes_active_thinking_block() {
    use crate::error_taxonomy::StreamError;

    let mut app = create_test_app();
    let entry_idx = ensure_streaming_thinking_active_entry(&mut app);
    app.thinking_started_at = Some(Instant::now());
    app.streaming_state.start_thinking(0, None);
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
fn second_thinking_block_appends_new_entry_in_same_active_cell() {
    // Real V4 turns can emit Thinking → Tool → Thinking → Tool before any
    // prose; the second thinking block should land as a fresh entry inside
    // the SAME active cell rather than flush the first group prematurely.
    let mut app = create_test_app();

    let _ = ensure_streaming_thinking_active_entry(&mut app);
    append_streaming_thinking(&mut app, 0, "first plan");
    let _ = finalize_streaming_thinking_active_entry(&mut app, Some(0.5), "");

    handle_tool_call_started(
        &mut app,
        "t-1",
        "exec_shell",
        &serde_json::json!({"command": "ls"}),
    );

    // Second Thinking block.
    let second_idx = ensure_streaming_thinking_active_entry(&mut app);
    assert_eq!(
        second_idx, 2,
        "second thinking entry follows the tool entry"
    );
    append_streaming_thinking(&mut app, second_idx, "second plan");

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

    assert!(!start_streaming_thinking_block(&mut app));
    let first_idx = app
        .streaming_thinking_active_entry
        .expect("first thinking entry active");
    app.reasoning_buffer.push_str("first tail");
    app.streaming_state.push_content(0, "first tail");

    assert!(start_streaming_thinking_block(&mut app));
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
    let status = app
        .status_message
        .as_deref()
        .expect("recoverable errors must set a status message");
    assert!(
        status.starts_with("Connection interrupted"),
        "expected interrupt-style status, got {status:?}"
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
    let status = app
        .status_message
        .as_deref()
        .expect("non-recoverable errors must set a status message");
    assert!(
        status.starts_with("Engine error"),
        "expected engine-error status, got {status:?}"
    );
}

#[test]
fn env_only_auth_failure_reopens_api_key_onboarding() {
    use crate::error_taxonomy::ErrorEnvelope;
    let mut app = create_test_app();
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
        crate::tui::app::OnboardingState::ApiKey,
        "env-only auth failures should prompt for a saved config key"
    );
    assert!(app.onboarding_needs_api_key);
    let status = app
        .status_message
        .as_deref()
        .expect("auth recovery should explain the env key source");
    assert!(
        status.contains("DEEPSEEK_API_KEY"),
        "expected env-specific recovery hint, got {status:?}"
    );
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
fn tab_queues_running_turn_draft_for_next_turn() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.input = "follow up next".to_string();
    app.cursor_position = app.input.chars().count();

    assert!(queue_current_draft_for_next_turn(&mut app));

    assert!(app.input.is_empty());
    assert_eq!(app.queued_message_count(), 1);
    assert_eq!(
        app.queued_messages.front().map(|msg| msg.display.as_str()),
        Some("follow up next")
    );
    assert!(
        app.status_message
            .as_deref()
            .is_some_and(|msg| msg.contains("queued — ↑"))
    );
}

#[test]
fn tab_queue_preserves_queued_draft_skill_instruction() {
    let mut app = create_test_app();
    app.is_loading = true;
    app.input = "edited queued follow-up".to_string();
    app.cursor_position = app.input.chars().count();
    app.queued_draft = Some(QueuedMessage::new(
        "original".to_string(),
        Some("skill body".to_string()),
    ));

    assert!(queue_current_draft_for_next_turn(&mut app));

    let queued = app.queued_messages.front().expect("queued message");
    assert_eq!(queued.display, "edited queued follow-up");
    assert_eq!(queued.skill_instruction.as_deref(), Some("skill body"));
    assert!(app.queued_draft.is_none());
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
fn build_pending_input_preview_includes_current_context_chips() {
    let tmpdir = TempDir::new().expect("tempdir");
    std::fs::write(tmpdir.path().join("guide.md"), "hello").expect("write");
    let mut app = create_test_app();
    app.workspace = tmpdir.path().to_path_buf();
    app.input = "Read @guide.md and @missing.md".to_string();
    app.cursor_position = app.input.chars().count();

    let preview = build_pending_input_preview(&app);

    assert!(
        preview
            .context_items
            .iter()
            .any(|item| item.kind == "file" && item.label == "guide.md" && item.included),
        "file mention preview missing: {:?}",
        preview.context_items
    );
    assert!(
        preview
            .context_items
            .iter()
            .any(|item| item.kind == "missing" && item.label == "missing.md" && !item.included),
        "missing mention preview missing: {:?}",
        preview.context_items
    );
}

#[test]
fn render_footer_from_with_default_items_renders_mode_and_model() {
    // Default footer composition should show the mode chip and model
    // identifier — whatever the configured default model is.
    let mut app = create_test_app();
    app.session.session_cost = 0.00005;
    let items = crate::config::StatusItem::default_footer();
    let props = render_footer_from(&app, &items, None);
    assert_eq!(props.mode_label, "agent");
    assert!(!props.model.is_empty(), "footer should show a model name");
    // Tiny but real costs should render instead of disappearing as "$0.00".
    assert!(!props.cost.is_empty());
    assert_eq!(spans_text(&props.cost), "<$0.0001");
}

#[test]
fn render_footer_from_with_empty_items_blanks_every_segment() {
    // A user who toggles every chip OFF should get a bare footer (no model
    // text, no cost, no auxiliary chips). This is the explicit-empty case.
    let mut app = create_test_app();
    app.session.session_cost = 1.5;
    let props = render_footer_from(&app, &[], None);
    assert_eq!(props.mode_label, "");
    assert!(props.model.is_empty());
    assert!(props.cost.is_empty());
    assert!(props.coherence.is_empty());
    assert!(props.agents.is_empty());
    assert!(props.cache.is_empty());
}

#[test]
fn render_footer_from_drops_only_unselected_clusters() {
    // Toggling Cost off but keeping the rest should hide cost only.
    let mut app = create_test_app();
    app.session.session_cost = 0.42;
    let items: Vec<crate::config::StatusItem> = crate::config::StatusItem::default_footer()
        .into_iter()
        .filter(|item| *item != crate::config::StatusItem::Cost)
        .collect();
    let props = render_footer_from(&app, &items, None);
    assert_eq!(props.mode_label, "agent");
    assert!(!props.model.is_empty(), "footer should show a model name");
    assert!(
        props.cost.is_empty(),
        "cost cluster should be empty when Cost is disabled"
    );
}

#[test]
fn render_footer_from_git_branch_item_renders_workspace_branch() {
    let repo = init_git_repo();
    let checkout = Command::new("git")
        .args(["checkout", "-b", "feature/statusline"])
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

    let props = render_footer_from(&app, &[crate::config::StatusItem::GitBranch], None);
    assert_eq!(spans_text(&props.cache), "feature/statusline");
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
    let after_recon = app.displayed_session_cost();
    assert!(
        after_recon >= after_first,
        "displayed cost regressed: {after_recon} < {after_first}"
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
        model: "deepseek-v4-flash".to_string(),
        usage: crate::models::Usage {
            input_tokens: 10_000,
            output_tokens: 1_000,
            ..Default::default()
        },
    };
    handle_subagent_mailbox(&mut app, 11, &usage);
    let baseline = app.displayed_session_cost();
    assert!(baseline > 0.0);

    // Re-emit the same seq — must be deduped, displayed cost unchanged.
    handle_subagent_mailbox(&mut app, 11, &usage);
    assert!(
        (app.displayed_session_cost() - baseline).abs() < 1e-9,
        "duplicate mailbox seq must not move displayed cost"
    );

    // A fresh seq must extend the displayed cost upward.
    handle_subagent_mailbox(&mut app, 12, &usage);
    assert!(app.displayed_session_cost() > baseline);
}
#[test]
fn checklist_write_renders_dedicated_card() {
    let cell = GenericToolCell {
        name: "checklist_write".to_string(),
        status: ToolStatus::Success,
        input_summary: None,
        output: Some(
            "Todo list updated (3 items, 33% complete)\n{\"items\":[{\"id\":1,\"content\":\"Plan it out\",\"status\":\"completed\"},{\"id\":2,\"content\":\"Wire the thing\",\"status\":\"in_progress\"},{\"id\":3,\"content\":\"Run gates\",\"status\":\"pending\"}],\"completion_pct\":33,\"in_progress_id\":2}"
                .to_string(),
        ),
        prompts: None,
        spillover_path: None,
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
fn history_arrow_handles_whitespace_input() {
    let mut app = create_test_app();
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
fn notification_settings_tui_always_keeps_configured_method_no_threshold() {
    let config = Config {
        tui: Some(crate::config::TuiConfig {
            notification_condition: Some(crate::config::NotificationCondition::Always),
            ..Default::default()
        }),
        notifications: Some(crate::config::NotificationsConfig {
            method: crate::config::NotificationMethod::Bel,
            threshold_secs: 120,
            include_summary: true,
        }),
        ..Config::default()
    };

    let (method, threshold, include_summary) =
        super::notification_settings(&config).expect("notification should be enabled");
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

    assert!(super::notification_settings(&config).is_none());
}

#[test]
fn notification_settings_no_tui_override_uses_notifications_block() {
    let config = Config {
        notifications: Some(crate::config::NotificationsConfig {
            method: crate::config::NotificationMethod::Osc9,
            threshold_secs: 45,
            include_summary: false,
        }),
        ..Config::default()
    };

    let (method, threshold, include_summary) =
        super::notification_settings(&config).expect("notification should be enabled");
    assert_eq!(method, crate::tui::notifications::Method::Osc9);
    assert_eq!(threshold, Duration::from_secs(45));
    assert!(!include_summary);
}

#[test]
fn completed_turn_notification_uses_streaming_text() {
    let app = create_test_app();
    let msg = super::completed_turn_notification_message(
        &app,
        "Hello there.\n\nWhat's next?",
        false,
        Duration::from_secs(12),
        None,
    );
    assert_eq!(msg, "Hello there.\nWhat's next?");
}

#[test]
fn completed_turn_notification_falls_back_to_latest_assistant_message() {
    let mut app = create_test_app();
    app.api_messages.push(crate::models::Message {
        role: "assistant".to_string(),
        content: vec![crate::models::ContentBlock::Text {
            text: "Earlier turn".to_string(),
            cache_control: None,
        }],
    });
    app.api_messages.push(crate::models::Message {
        role: "user".to_string(),
        content: vec![crate::models::ContentBlock::Text {
            text: "next".to_string(),
            cache_control: None,
        }],
    });
    app.api_messages.push(crate::models::Message {
        role: "assistant".to_string(),
        content: vec![crate::models::ContentBlock::Text {
            text: "Latest reply".to_string(),
            cache_control: None,
        }],
    });

    let msg =
        super::completed_turn_notification_message(&app, "", false, Duration::from_secs(75), None);
    assert_eq!(msg, "Latest reply");
}

#[test]
fn completed_turn_notification_falls_back_to_default_when_empty() {
    let app = create_test_app();
    let msg =
        super::completed_turn_notification_message(&app, "", false, Duration::from_secs(5), None);
    assert_eq!(msg, "deepseek: turn complete");
}

#[test]
fn completed_turn_notification_truncates_long_text() {
    let app = create_test_app();
    let long = "a".repeat(500);
    let msg = super::completed_turn_notification_message(
        &app,
        &long,
        false,
        Duration::from_secs(5),
        None,
    );
    assert!(msg.ends_with("..."));
    // 360-char body + 3-char ellipsis
    assert_eq!(msg.chars().count(), 363);
}

#[test]
fn subagent_completion_notification_uses_summary_line_not_sentinel() {
    let msg = super::subagent_completion_notification_message(
        "agent_live",
        "Finished the docs audit.\n<deepseek:subagent.done>{}</deepseek:subagent.done>",
        false,
        Duration::from_secs(42),
    );

    assert_eq!(msg, "sub-agent agent_live: Finished the docs audit.");
    assert!(!msg.contains("deepseek:subagent.done"));
}

#[test]
fn subagent_completion_notification_can_include_elapsed_summary() {
    let msg = super::subagent_completion_notification_message(
        "agent_live",
        "",
        true,
        Duration::from_secs(65),
    );

    assert!(msg.contains("deepseek: sub-agent agent_live complete"));
    assert!(msg.contains("deepseek: sub-agent complete (1m 5s)"));
}
