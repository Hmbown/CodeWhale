//! Ocean Work Graph surface ownership.
//!
//! Placement, scrolling, selection, and pager ownership remain local to this
//! component. Durable work rows derive from the active-session graph; pending
//! engine decisions are projected here as ephemeral attention rows.

mod input;
mod interaction;
mod model;
mod render;

pub use input::{handle_key, handle_mouse};
pub use model::{WorkSurfacePlacement, WorkSurfaceState};
pub use render::{height, render, split_chat};

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::{Terminal, backend::TestBackend};

    use crate::config::Config;
    use crate::tools::todo::TodoStatus;
    use crate::tui::app::{App, SidebarRowAction, TuiOptions};
    use crate::work_graph::{
        AcceptanceRequirement, ChangeCtx, EdgeKind, EvidenceKindTag, NodeKind, NodeState,
        OperationBinding, OperationOwnerSnapshot, OwnerState, Provenance, WorkEdge, WorkEdgeId,
        WorkGraph, WorkGraphChange, WorkNode, WorkNodeId,
    };

    const SESSION: &str = "work-surface-test";

    fn app() -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: PathBuf::from("."),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: true,
            use_bracketed_paste: true,
            max_subagents: 4,
            skills_dir: PathBuf::from("."),
            memory_path: PathBuf::from("memory.md"),
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
            use_memory: false,
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        let mut app = App::new(options, &Config::default());
        app.ui_locale = crate::localization::Locale::En;
        app
    }

    fn add_todos(app: &mut App, count: usize) {
        let mut todos = app.todos.try_lock().expect("todos");
        for index in 0..count {
            todos.add(
                format!("work item {index}"),
                if index == 0 {
                    TodoStatus::InProgress
                } else {
                    TodoStatus::Pending
                },
            );
        }
    }

    fn operation_graph(state: NodeState) -> crate::work_graph::WorkGraphSnapshot {
        let objective = WorkNodeId::derive(SESSION, "objective");
        let operation = WorkNodeId::derive(SESSION, "operation");
        let ctx = |now| ChangeCtx {
            session_id: SESSION.to_string(),
            now,
            idempotency_key: None,
        };
        let node = |id: WorkNodeId, kind, title: &str, now| WorkNode {
            id,
            kind,
            title: title.to_string(),
            state: NodeState::Ready,
            acceptance: Vec::new(),
            binding: None,
            evidence: None,
            provenance: Provenance::RuntimeReconcile {
                source: "test-owner".to_string(),
                observed_at: now,
            },
            created_at: now,
            updated_at: now,
        };
        let mut graph = WorkGraph::new();
        graph
            .apply(
                WorkGraphChange::AddNode {
                    node: node(objective.clone(), NodeKind::Objective, "Ship v0.9.1", 1),
                },
                ctx(1),
            )
            .expect("objective");
        graph
            .apply(
                WorkGraphChange::AddNode {
                    node: node(
                        operation.clone(),
                        NodeKind::Operation,
                        "Verify installed build",
                        2,
                    ),
                },
                ctx(2),
            )
            .expect("operation");
        graph
            .apply(
                WorkGraphChange::AddEdge {
                    edge: WorkEdge {
                        id: WorkEdgeId::derive(SESSION, "contains"),
                        kind: EdgeKind::Contains,
                        from: objective,
                        to: operation.clone(),
                    },
                },
                ctx(3),
            )
            .expect("contains");
        graph
            .apply(
                WorkGraphChange::BindOperation {
                    node: operation.clone(),
                    binding: OperationBinding {
                        external: "shell:shell_1234abcd".to_string(),
                        durable: false,
                        last_observation: None,
                    },
                },
                ctx(4),
            )
            .expect("binding");
        if state != NodeState::Ready {
            graph
                .apply(
                    WorkGraphChange::UpdateNode {
                        id: operation,
                        patch: crate::work_graph::WorkNodePatch {
                            state: Some(state),
                            ..crate::work_graph::WorkNodePatch::default()
                        },
                    },
                    ctx(5),
                )
                .expect("state");
        }
        graph.into_snapshot()
    }

    fn restore_graph(app: &mut App, graph: &crate::work_graph::WorkGraphSnapshot) {
        app.current_session_id = Some(SESSION.to_string());
        app.runtime_services
            .work
            .as_ref()
            .expect("Work Graph runtime")
            .restore(
                SESSION,
                Some(graph),
                &crate::work_graph::project_todos(graph),
                &crate::work_graph::project_plan(graph),
            )
            .expect("restore graph");
    }

    fn render_text(app: &mut App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| super::render(frame, frame.area(), app))
            .expect("draw");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn projection_keeps_every_legacy_todo_as_a_graph_row() {
        let mut app = app();
        add_todos(&mut app, 4);

        let rows = super::model::project(&mut app);

        assert!(rows[0].label.starts_with("Work · 1 running · 3 ready"));
        assert_eq!(rows[1].label, "To-do 0/4 · 4 left");
        for index in 0..4 {
            assert!(
                rows.iter()
                    .any(|row| row.label == format!("work item {index}"))
            );
        }
        assert!(rows.iter().all(|row| !row.id.0.starts_with("todo:")));
    }

    #[test]
    fn todo_heading_reports_completed_total_and_remaining() {
        let mut app = app();
        {
            let mut todos = app.todos.try_lock().expect("todos");
            todos.add("finished".to_string(), TodoStatus::Completed);
            todos.add("current".to_string(), TodoStatus::InProgress);
            todos.add("next".to_string(), TodoStatus::Pending);
        }

        let rows = super::model::project(&mut app);

        assert_eq!(rows[1].label, "To-do 1/3 · 2 left");
    }

    #[test]
    fn active_session_without_work_renders_truthful_empty_state() {
        let mut app = app();
        app.current_session_id = Some(SESSION.to_string());

        let rows = super::model::project(&mut app);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "Work · empty");
        assert!(!rows[0].selectable);
    }

    #[test]
    fn auto_review_attention_replaces_empty_work_without_opening_a_modal() {
        let mut app = app();
        app.current_session_id = Some(SESSION.to_string());
        app.pending_work_approval = Some(crate::tui::approval::ApprovalRequest::new(
            "approval-1",
            "mcp_vendor_mutate_account",
            "Change a remote account",
            &serde_json::json!({"account": "example"}),
            "approval-key",
        ));

        let rows = super::model::project(&mut app);

        assert_eq!(rows[0].label, "Work · 1 needs input");
        assert_eq!(rows[1].mark, "?");
        assert_eq!(rows[1].label, "Review mcp_vendor_mutate_account");
        assert_eq!(rows[1].detail, "decision waiting");
        assert!(rows[1].selectable);
        assert!(matches!(
            rows[1].primary_action.as_ref(),
            Some(SidebarRowAction::ReviewApproval { tool_id }) if tool_id == "approval-1"
        ));
        assert!(
            app.view_stack.is_empty(),
            "projection must not throw a modal"
        );
    }

    #[test]
    fn genuine_question_is_selectable_from_work() {
        let mut app = app();
        app.current_session_id = Some(SESSION.to_string());
        app.pending_user_input_prompt = Some((
            "question-1".to_string(),
            crate::tools::user_input::UserInputRequest {
                questions: vec![crate::tools::user_input::UserInputQuestion {
                    header: "Choice".to_string(),
                    id: "choice".to_string(),
                    question: "Which path\nshould I\ttake?".to_string(),
                    options: vec![
                        crate::tools::user_input::UserInputOption {
                            label: "A".to_string(),
                            description: "Take path A".to_string(),
                        },
                        crate::tools::user_input::UserInputOption {
                            label: "B".to_string(),
                            description: "Take path B".to_string(),
                        },
                    ],
                    allow_free_text: false,
                    multi_select: false,
                }],
            },
        ));

        let rows = super::model::project(&mut app);

        assert_eq!(rows[0].label, "Work · 1 needs input");
        assert_eq!(rows[1].label, "Which path should I take?");
        assert_eq!(rows[1].detail, "decision waiting");
        assert!(matches!(
            rows[1].primary_action.as_ref(),
            Some(SidebarRowAction::AnswerUserInput { tool_id }) if tool_id == "question-1"
        ));
        assert!(
            app.view_stack.is_empty(),
            "projection must not throw a modal"
        );
    }

    #[test]
    fn missing_runtime_renders_disconnected_state() {
        let mut app = app();
        app.current_session_id = Some(SESSION.to_string());
        app.runtime_services.work = None;

        let rows = super::model::project(&mut app);

        assert_eq!(rows[0].label, "Work · disconnected");
    }

    #[test]
    fn busy_graph_authority_renders_truthful_error_without_leaking_it_into_header() {
        let mut app = app();
        app.current_session_id = Some(SESSION.to_string());
        let todos = app.todos.clone();
        let _guard = todos.try_lock().expect("hold To-do authority lock");

        let rows = super::model::project(&mut app);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "Work · error");
        assert!(rows[0].detail.contains("To-do state is busy"));
        assert!(!rows[0].label.contains("busy"));
    }

    #[test]
    fn graph_error_without_an_active_session_stays_suppressed() {
        let mut app = app();
        let todos = app.todos.clone();
        let _guard = todos.try_lock().expect("hold To-do authority lock");

        let rows = super::model::project(&mut app);

        assert!(rows.is_empty());
    }

    #[test]
    fn waiting_operation_is_not_counted_as_running() {
        let mut app = app();
        let graph = operation_graph(NodeState::Waiting);
        restore_graph(&mut app, &graph);
        app.runtime_services
            .work
            .as_ref()
            .expect("Work Graph runtime")
            .reconcile_operation(
                SESSION,
                OperationOwnerSnapshot::new("shell:shell_1234abcd", OwnerState::Waiting, 1, 6),
            )
            .expect("waiting shell owner");

        let rows = super::model::project(&mut app);

        assert!(
            rows[0]
                .label
                .starts_with("Work · 0 running · 1 waiting · 0 ready · 0 blocked"),
            "{}",
            rows[0].label
        );
    }

    #[test]
    fn stale_operation_is_blocked_attention_with_bounded_output_section() {
        let mut app = app();
        let graph = operation_graph(NodeState::Stale);
        restore_graph(&mut app, &graph);

        let rows = super::model::project(&mut app);
        assert!(rows[0].label.contains("1 blocked"), "{}", rows[0].label);
        let row = rows.iter().find(|row| row.selectable).expect("stale row");
        assert_eq!(row.mark, "?");
        assert!(row.detail.starts_with("stale · operation"));
        let Some(SidebarRowAction::InspectWork {
            body, stop_action, ..
        }) = row.primary_action.as_ref()
        else {
            panic!("stale row must open inspector");
        };
        assert!(
            stop_action.is_none(),
            "a stale owner cannot truthfully expose a stop action"
        );
        assert!(
            body.contains("Last bounded output\nNo output receipt"),
            "{body}"
        );
        assert!(body.contains("Owner cannot confirm liveness"), "{body}");
    }

    #[test]
    fn completed_operation_with_acceptance_is_not_rendered_done() {
        let mut graph = WorkGraph::from_snapshot(operation_graph(NodeState::Ready));
        let operation = WorkNodeId::derive(SESSION, "operation");
        graph
            .apply(
                WorkGraphChange::UpdateNode {
                    id: operation,
                    patch: crate::work_graph::WorkNodePatch {
                        state: Some(NodeState::Completed),
                        acceptance: Some(vec![AcceptanceRequirement::EvidenceOfKind {
                            kind: EvidenceKindTag::ToolRun,
                        }]),
                        ..crate::work_graph::WorkNodePatch::default()
                    },
                },
                ChangeCtx {
                    session_id: SESSION.to_string(),
                    now: 6,
                    idempotency_key: None,
                },
            )
            .expect("completed pending evidence");
        let graph = graph.into_snapshot();
        let mut app = app();
        restore_graph(&mut app, &graph);

        let rows = super::model::project(&mut app);
        assert!(rows[0].label.contains("1 blocked"), "{}", rows[0].label);
        let row = rows
            .iter()
            .find(|row| row.selectable)
            .expect("operation row");
        assert_eq!(row.mark, "!");
        assert!(row.detail.contains("completed · evidence pending"));
        assert_ne!(row.mark, "✓");
        let Some(SidebarRowAction::InspectWork { body, .. }) = row.primary_action.as_ref() else {
            panic!("completed operation must remain inspectable");
        };
        assert!(body.contains("evidence of kind tool run"), "{body}");
        assert!(
            body.contains("acceptance evidence is still missing"),
            "{body}"
        );
    }

    #[test]
    fn work_rows_open_graph_inspector_without_inline_controls() {
        let mut app = app();
        let graph = operation_graph(NodeState::Active);
        restore_graph(&mut app, &graph);
        app.runtime_services
            .work
            .as_ref()
            .expect("Work Graph runtime")
            .reconcile_operation(
                SESSION,
                OperationOwnerSnapshot::new("shell:shell_1234abcd", OwnerState::Running, 1, 6),
            )
            .expect("live shell owner");

        let text = render_text(&mut app, 100, 6);
        assert!(!text.contains("[open]"), "{text}");
        assert!(!text.contains("[stop]"), "{text}");
        let row_y = app
            .work_surface
            .hitboxes
            .iter()
            .find(|hit| hit.id.0.starts_with("graph:"))
            .expect("graph hitbox")
            .row_y;
        let outcome = super::handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 2,
                row: row_y,
                modifiers: KeyModifiers::NONE,
            },
        );
        let action = outcome.action.expect("inspector action");
        let SidebarRowAction::InspectWork {
            body, stop_action, ..
        } = &action
        else {
            panic!("expected Work inspector");
        };
        for section in [
            "Objective",
            "Prerequisites",
            "Downstream impact",
            "Binding + lifecycle owner",
            "Evidence vs acceptance",
            "Blockers / approvals",
            "Why next",
            "Provenance + last reconcile",
        ] {
            assert!(body.contains(section), "missing {section}: {body}");
        }
        assert!(matches!(
            stop_action.as_deref(),
            Some(SidebarRowAction::Command(command)) if command == "/jobs cancel shell_1234abcd"
        ));
        crate::tui::mouse_ui::apply_sidebar_row_action(&mut app, action);
        assert_eq!(
            app.view_stack.top_kind(),
            Some(crate::tui::views::ModalKind::Pager)
        );
    }

    #[test]
    fn narrow_render_hover_keeps_full_untruncated_row() {
        let mut app = app();
        app.todos.try_lock().expect("todos").add(
            "A deliberately long graph-owned work row".to_string(),
            TodoStatus::InProgress,
        );

        let _ = render_text(&mut app, 24, 4);
        let hover = app
            .sidebar_hover
            .sections
            .last()
            .and_then(|section| section.rows.first())
            .expect("hover row");
        assert!(hover.is_truncated);
        assert!(hover.full_text.contains("deliberately long graph-owned"));
        assert!(hover.stop_action.is_none());
    }

    #[test]
    fn overflow_scroll_and_selection_remain_panel_owned() {
        let mut app = app();
        add_todos(&mut app, 8);
        let _ = render_text(&mut app, 80, 5);
        assert!(app.work_surface.total_rows > app.work_surface.visible_rows);

        let transcript_delta = app.viewport.pending_scroll_delta;
        let outcome = super::handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 10,
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
        );
        assert!(outcome.consumed);
        assert_eq!(app.viewport.pending_scroll_delta, transcript_delta);
        assert!(app.work_surface.scroll_offset > 0);
    }

    #[test]
    fn mouse_wheel_reaches_last_todo_across_top_surface_heights() {
        for height in [3, 5, 6, 8] {
            let mut app = app();
            add_todos(&mut app, 10);
            let _ = render_text(&mut app, 80, height);
            assert!(app.work_surface.total_rows > app.work_surface.visible_rows);
            let transcript_delta = app.viewport.pending_scroll_delta;

            let mut text = String::new();
            for _ in 0..16 {
                let outcome = super::handle_mouse(
                    &mut app,
                    MouseEvent {
                        kind: MouseEventKind::ScrollDown,
                        column: 10,
                        row: 1,
                        modifiers: KeyModifiers::NONE,
                    },
                );
                assert!(outcome.consumed, "height {height}");
                text = render_text(&mut app, 80, height);
            }

            assert!(
                text.contains("work item 9"),
                "last To-do was unreachable at surface height {height}: {text:?}"
            );
            assert_eq!(
                app.work_surface.scroll_offset,
                app.work_surface
                    .total_rows
                    .saturating_sub(app.work_surface.visible_rows.max(1)),
                "wheel did not reach the legal tail at surface height {height}"
            );
            assert_eq!(app.viewport.pending_scroll_delta, transcript_delta);
        }
    }

    #[test]
    fn mouse_wheel_reaches_last_todo_in_side_rail_placements() {
        for placement in [
            super::WorkSurfacePlacement::Left,
            super::WorkSurfacePlacement::Right,
        ] {
            let mut app = app();
            add_todos(&mut app, 10);
            app.work_surface.placement = placement;
            app.work_surface.effective_placement = placement;
            let _ = render_text(&mut app, 30, 6);

            let mut text = String::new();
            for _ in 0..16 {
                let outcome = super::handle_mouse(
                    &mut app,
                    MouseEvent {
                        kind: MouseEventKind::ScrollDown,
                        column: 10,
                        row: 1,
                        modifiers: KeyModifiers::NONE,
                    },
                );
                assert!(outcome.consumed, "placement {placement:?}");
                text = render_text(&mut app, 30, 6);
            }

            assert!(
                text.contains("work item 9"),
                "last To-do was unreachable in {placement:?}: {text:?}"
            );
        }
    }

    #[test]
    fn keyboard_end_reveals_last_todo_after_redraw() {
        let mut app = app();
        add_todos(&mut app, 10);
        let _ = render_text(&mut app, 80, 5);
        let _ = super::handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::ALT),
        );
        let _ = super::handle_key(&mut app, KeyEvent::new(KeyCode::End, KeyModifiers::NONE));

        let text = render_text(&mut app, 80, 5);

        assert!(text.contains("work item 9"), "{text:?}");
        assert_eq!(
            app.work_surface.scroll_offset,
            app.work_surface
                .total_rows
                .saturating_sub(app.work_surface.visible_rows.max(1))
        );
    }

    #[test]
    fn keyboard_navigation_is_panel_local_when_focused() {
        let mut app = app();
        add_todos(&mut app, 3);
        app.work_surface.visible_rows = 2;
        assert!(
            super::handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('w'), KeyModifiers::ALT)
            )
            .is_some()
        );
        let first = app.work_surface.selected.clone();
        let _ = super::handle_key(&mut app, KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert_ne!(app.work_surface.selected, first);
        assert!(app.work_surface.focused);
    }

    #[test]
    fn printable_keys_release_panel_focus_for_composer() {
        let mut app = app();
        add_todos(&mut app, 1);
        let _ = super::handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::ALT),
        );

        let outcome = super::handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        );

        assert!(outcome.is_none());
        assert!(!app.work_surface.focused);
    }

    #[test]
    fn side_placements_reuse_the_same_graph_rows() {
        for (placement, expected_chat_x, expected_rail_x) in [
            (super::WorkSurfacePlacement::Left, 30, 0),
            (super::WorkSurfacePlacement::Right, 0, 70),
        ] {
            let mut app = app();
            add_todos(&mut app, 2);
            app.work_surface.placement = placement;
            assert_eq!(super::height(&mut app, 100, 24, false), 0);
            let area = ratatui::layout::Rect::new(0, 0, 100, 12);
            let (chat, rail) = super::split_chat(&mut app, area, false);
            let rail = rail.expect("side rail");
            assert_eq!(chat.x, expected_chat_x);
            assert_eq!(rail.x, expected_rail_x);
            assert_eq!(rail.width, 30);
            assert!(
                app.work_surface
                    .latest_rows
                    .iter()
                    .any(|row| row.label == "work item 1")
            );
        }
    }

    #[test]
    fn opened_row_toggles_closed_without_losing_selection() {
        let mut app = app();
        add_todos(&mut app, 1);
        let row = super::model::project(&mut app)
            .into_iter()
            .find(|row| row.selectable)
            .expect("work row");
        let open = row.primary_action.clone();

        assert!(super::interaction::activate_primary(&mut app, &row.id, open.clone()).is_some());
        assert!(super::interaction::activate_primary(&mut app, &row.id, open).is_none());
        assert!(app.work_surface.opened.is_none());
        assert_eq!(app.work_surface.selected.as_ref(), Some(&row.id));
    }
}
