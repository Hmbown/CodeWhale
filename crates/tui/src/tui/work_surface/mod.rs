//! Ocean Work Graph surface ownership.
//!
//! Placement, scrolling, selection, and pager ownership remain local to this
//! component. Every visible work row derives from the active-session graph.

mod input;
mod interaction;
mod model;
mod panels;
mod render;

pub use input::{handle_key, handle_mouse};
pub(crate) use interaction::agent_details_closed;
pub use model::{RailPanel, WorkSurfacePlacement, WorkSurfaceState};
pub use render::{height, render, split_chat};

#[cfg(test)]
mod tests {
    use super::WorkSurfacePlacement;
    use std::path::PathBuf;

    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::{Terminal, backend::TestBackend};

    use crate::config::Config;
    use crate::tools::subagent::{
        AgentWorkerStatus, FleetRole, SubAgentAssignment, SubAgentResult, SubAgentStatus,
    };
    use crate::tools::todo::TodoStatus;
    use crate::tui::app::{
        AgentCurrentActivity, AgentCurrentActivityStatus, App, SidebarRowAction, ToolDetailRecord,
        TuiOptions,
    };
    use crate::tui::history::{
        FileMutationReceipt, GenericToolCell, HistoryCell, PatchSummaryCell, ToolCell, ToolStatus,
    };
    use crate::work_graph::{
        AcceptanceRequirement, ChangeCtx, EdgeKind, EvidenceKindTag, NodeKind, NodeState,
        OperationBinding, OperationOwnerSnapshot, OwnerState, Provenance, WorkEdge, WorkEdgeId,
        WorkGraph, WorkGraphChange, WorkNode, WorkNodeId,
    };

    const SESSION: &str = "work-surface-test";

    fn app() -> App {
        let options = TuiOptions {
            use_mouse_capture: true,
            max_subagents: 4,
            ..crate::test_support::test_tui_options(PathBuf::from("."))
        };
        let mut app = App::new(options, &Config::default());
        app.ui_locale = crate::localization::Locale::En;
        // Dogfood guard: App::new reads the developer's real settings.toml,
        // and the 0.9.4 migration maps a legacy sidebar_focus onto the rail
        // panel. These tests exercise the Tasks panel's row machinery, so
        // pin it rather than depend on the host file.
        app.work_surface.panel = super::RailPanel::Tasks;
        app
    }

    /// The row budget `ui::render` would hand the rail on a terminal of this
    /// height with real work on screen. Calls the production formula rather
    /// than restating it, so a change to the chrome accounting shows up here
    /// instead of silently diverging. The idle-empty budget (where the
    /// ambient floor bites) is covered end-to-end in `ui::tests`.
    fn working_budget(app: &App, terminal_height: u16) -> u16 {
        crate::tui::ui::rail_row_budget(app, 80, terminal_height, false)
    }

    /// A budget wide enough never to bind, for tests about something else.
    const AMPLE_BUDGET: u16 = u16::MAX;

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

    fn restore_saved_graph(app: &mut App, graph: &crate::work_graph::WorkGraphSnapshot) {
        app.current_session_id = Some(SESSION.to_string());
        let state = crate::session_manager::SessionWorkState {
            graph: Some(graph.clone()),
            todos: crate::work_graph::project_todos(graph),
            plan: crate::work_graph::project_plan(graph),
        };
        app.restore_work_state(SESSION, std::path::Path::new("."), Some(&state))
            .expect("restore saved graph");
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

        assert!(
            rows[0].label.starts_with("Work · Running:")
                || rows[0]
                    .label
                    .starts_with("Work · 1 active · 0 needs input · 3 ready"),
            "unexpected heading {}",
            rows[0].label
        );
        for index in 0..4 {
            assert!(
                rows.iter()
                    .any(|row| row.label == format!("work item {index}"))
            );
        }
        assert!(rows.iter().all(|row| !row.id.0.starts_with("todo:")));
    }

    #[test]
    fn coordination_projection_is_one_selectable_work_row_with_shared_details() {
        use crate::tools::subagent::CoordinationDetailProjection;
        use crate::tools::subagent::coord::{
            CoordinationDetailMetrics, DecisionRecord, DecisionStatus,
        };

        let mut app = app();
        app.coordination_detail = Some(CoordinationDetailProjection {
            schema_version: 1,
            sequence: 7,
            decisions: vec![DecisionRecord {
                decision_id: "decision-work".to_string(),
                subject: "coordination row".to_string(),
                status: DecisionStatus::Accepted,
                owner: "release-owner".to_string(),
                scope: Vec::new(),
                constraints: vec!["PRIVATE-TRANSCRIPT-MARKER".to_string()],
                evidence_handles: Vec::new(),
                version: 2,
                sequence: 7,
            }],
            write_claims: Vec::new(),
            reconciliations: Vec::new(),
            context_projections: Vec::new(),
            contentions: Vec::new(),
            metrics: CoordinationDetailMetrics {
                hottest_paths: Vec::new(),
                package_or_module_growth: None,
                route_or_cost: None,
                note: "No active claims".to_string(),
            },
            bounded: true,
            limit: 24,
            process_lock_held: true,
            process_lock_note: None,
        });

        let rows = super::model::project(&mut app);
        assert_eq!(
            rows[0].label,
            "Work · 0 active · 0 needs input · 0 ready · 1 recent"
        );
        let row = rows
            .iter()
            .find(|row| row.id.0 == "coordination")
            .expect("coordination Work row");
        assert_eq!(row.label, "Coordination Work");
        assert_eq!(row.detail, "1 decisions · 0 contentions · 0 reconciled");
        let Some(SidebarRowAction::InspectWork { title, body, .. }) = row.primary_action.as_ref()
        else {
            panic!("coordination row must open the shared Work inspector");
        };
        assert_eq!(title, "Coordination Work");
        assert!(body.contains("decision-work · coordination row"), "{body}");
        assert!(
            body.contains("status accepted · owner release-owner · version 2"),
            "{body}"
        );
        assert!(!body.contains("PRIVATE-TRANSCRIPT-MARKER"), "{body}");

        app.work_surface.placement = WorkSurfacePlacement::Right;
        app.work_surface.effective_placement = WorkSurfacePlacement::Right;
        let narrow = render_text(&mut app, 32, 4);
        assert!(narrow.contains("Coordination Work"), "{narrow}");
        let _ = super::handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::ALT),
        );
        let action = super::handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .expect("Work surface handled Enter")
            .expect("coordination inspector action");
        assert!(matches!(action, SidebarRowAction::InspectWork { .. }));
    }

    #[test]
    fn empty_coordination_projection_does_not_create_work_chrome() {
        use crate::tools::subagent::CoordinationDetailProjection;
        use crate::tools::subagent::coord::{ContextProjectionReceipt, CoordinationDetailMetrics};

        let mut app = app();
        app.coordination_detail = Some(CoordinationDetailProjection {
            schema_version: 1,
            sequence: 3,
            decisions: Vec::new(),
            write_claims: Vec::new(),
            reconciliations: Vec::new(),
            context_projections: ["agent-a", "agent-b", "agent-c"]
                .into_iter()
                .enumerate()
                .map(|(index, child_id)| ContextProjectionReceipt {
                    child_id: child_id.to_string(),
                    decision_ids: Vec::new(),
                    projected_bytes: 0,
                    deduplicated: 0,
                    omitted: 0,
                    sequence: u64::try_from(index + 1).expect("small fixture sequence"),
                })
                .collect(),
            contentions: Vec::new(),
            metrics: CoordinationDetailMetrics {
                hottest_paths: Vec::new(),
                package_or_module_growth: None,
                route_or_cost: None,
                note: "growth and route/cost stay null when the coordination ledger has no authoritative source".to_string(),
            },
            bounded: true,
            limit: 24,
            process_lock_held: true,
            process_lock_note: None,
        });

        let rows = super::model::project(&mut app);
        assert!(
            rows.is_empty(),
            "zero-byte, no-decision coordination receipts must not create Work chrome: {rows:?}"
        );
    }

    #[test]
    fn nonempty_context_projection_remains_inspectable_work() {
        use crate::tools::subagent::CoordinationDetailProjection;
        use crate::tools::subagent::coord::{ContextProjectionReceipt, CoordinationDetailMetrics};

        let mut app = app();
        app.coordination_detail = Some(CoordinationDetailProjection {
            schema_version: 1,
            sequence: 1,
            decisions: Vec::new(),
            write_claims: Vec::new(),
            reconciliations: Vec::new(),
            context_projections: vec![ContextProjectionReceipt {
                child_id: "agent-a".to_string(),
                decision_ids: vec!["decision-a".to_string()],
                projected_bytes: 32,
                deduplicated: 0,
                omitted: 0,
                sequence: 1,
            }],
            contentions: Vec::new(),
            metrics: CoordinationDetailMetrics {
                hottest_paths: Vec::new(),
                package_or_module_growth: None,
                route_or_cost: None,
                note: String::new(),
            },
            bounded: true,
            limit: 24,
            process_lock_held: true,
            process_lock_note: None,
        });

        let rows = super::model::project(&mut app);
        assert!(
            rows.iter().any(|row| row.id.0 == "coordination"),
            "non-empty context projection must remain inspectable: {rows:?}"
        );
    }

    #[test]
    fn current_blocked_contention_uses_attention_bucket_mark_and_tone() {
        use crate::tools::subagent::CoordinationDetailProjection;
        use crate::tools::subagent::coord::{
            CoordinationDetailMetrics, PersistedWriteClaim, WriteContentionDisposition,
            WriteContentionReceipt, WriteScopeClaim,
        };

        let mut app = app();
        app.coordination_detail = Some(CoordinationDetailProjection {
            schema_version: 1,
            sequence: 2,
            decisions: Vec::new(),
            write_claims: vec![PersistedWriteClaim {
                claim: WriteScopeClaim {
                    owner: "worker-a".to_string(),
                    roots: vec!["crates/tui".to_string()],
                    exact_files: Vec::new(),
                    contracts: vec!["ui-contract".to_string()],
                },
                sequence: 1,
                isolated_worktree: false,
            }],
            reconciliations: Vec::new(),
            context_projections: Vec::new(),
            contentions: vec![WriteContentionReceipt {
                claimant: "worker-b".to_string(),
                conflicting_owner: "worker-a".to_string(),
                roots: vec!["crates/tui".to_string()],
                exact_files: Vec::new(),
                contracts: vec!["ui-contract".to_string()],
                disposition: WriteContentionDisposition::BlockedPendingIsolationOrSerialization,
                resolution_sequence: None,
                sequence: 2,
            }],
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
        });

        let rows = super::model::project(&mut app);
        assert_eq!(
            rows[0].label,
            "Work · Needs input: Coordination Work · 1 blocked"
        );
        let row = rows
            .iter()
            .find(|row| row.id.0 == "coordination")
            .expect("blocked coordination Work row");
        assert_eq!(row.mark, crate::tui::glyphs::ATTENTION);
        assert_eq!(row.tone, super::model::WorkTone::Attention);
        assert_eq!(row.detail, "0 decisions · 1 contentions · 0 reconciled");
    }

    #[test]
    fn todos_share_one_canonical_work_projection_without_a_second_heading() {
        let mut app = app();
        {
            let mut todos = app.todos.try_lock().expect("todos");
            todos.add("finished".to_string(), TodoStatus::Completed);
            todos.add("current".to_string(), TodoStatus::InProgress);
            todos.add("next".to_string(), TodoStatus::Pending);
        }

        let rows = super::model::project(&mut app);

        assert!(
            rows[0].label.starts_with("Work · Running:")
                || rows[0].label.starts_with("Work · Ready:"),
            "expected actionable title heading, got {}",
            rows[0].label
        );
        assert_eq!(
            rows.iter()
                .skip(1)
                .map(|row| row.label.as_str())
                .collect::<Vec<_>>(),
            ["finished", "current", "next"]
        );
    }

    #[test]
    fn top_surface_pins_one_progress_receipt_and_numbers_canonical_rows() {
        let mut app = app();
        {
            let mut todos = app.todos.try_lock().expect("todos");
            todos.add("finished".to_string(), TodoStatus::Completed);
            todos.add("current".to_string(), TodoStatus::InProgress);
            todos.add("next".to_string(), TodoStatus::Pending);
        }

        let text = render_text(&mut app, 80, 6);
        let done = format!("1 · {} finished", crate::tui::glyphs::DONE);
        let current = format!("2 · {} current", crate::tui::glyphs::SELECTION);
        let next = format!("3 · {} next", crate::tui::glyphs::READY);

        assert!(text.contains("To-do · 1/3 · 2 left"), "{text:?}");
        assert_eq!(text.matches("To-do ·").count(), 1, "{text:?}");
        assert!(text.contains(&done), "{text:?}");
        assert!(text.contains(&current), "{text:?}");
        assert!(text.contains(&next), "{text:?}");
        assert!(
            text.find(&done) < text.find(&current) && text.find(&current) < text.find(&next),
            "canonical order drifted: {text:?}"
        );
        assert_eq!(app.work_surface.hitboxes.len(), 3);
        assert_eq!(app.work_surface.hitboxes[0].row_y, 1);
    }

    #[test]
    fn top_strip_auto_fits_step_count_up_to_caps() {
        // Two steps: divider + progress receipt + 2 rows = 4 lines, not a
        // fixed-height band of blank water.
        let mut two_steps = app();
        two_steps.work_surface.top_height = 8;
        add_todos(&mut two_steps, 2);
        let budget = working_budget(&two_steps, 40);
        assert_eq!(super::height(&mut two_steps, 100, 40, budget), 4);

        // Ten steps: content wants 12 lines, the default 8-line cap wins.
        let mut ten_steps = app();
        ten_steps.work_surface.top_height = 8;
        add_todos(&mut ten_steps, 10);
        let budget = working_budget(&ten_steps, 40);
        assert_eq!(super::height(&mut ten_steps, 100, 40, budget), 8);

        // Short terminal: the transcript's spare rows beat both content and
        // the configured cap. A 12-row terminal spends 1 on the header, 1 on
        // the phase strip and 3 on the bordered composer, and owes the
        // transcript its 3-row floor — so 4 rows are actually spare. (This
        // used to be 6, half the terminal, which left the transcript 2 rows.)
        let mut short_terminal = app();
        short_terminal.work_surface.top_height = 8;
        add_todos(&mut short_terminal, 10);
        let budget = working_budget(&short_terminal, 12);
        assert_eq!(super::height(&mut short_terminal, 100, 12, budget), 4);

        // Nothing to show: no strip at all.
        let mut empty = app();
        empty.work_surface.top_height = 8;
        assert_eq!(super::height(&mut empty, 100, 40, AMPLE_BUDGET), 0);
    }

    /// A strip that reports zero rows is not on screen, so the interaction
    /// state describing it must go with it. Stale hitboxes outlive the rows
    /// they described: the transcript rows that replaced the strip would keep
    /// routing clicks into a panel that is not there.
    #[test]
    fn a_yielded_strip_drops_its_interaction_state() {
        // Each case is a distinct zero-return inside `height`, and every one
        // of them has to tear down. `starve` turns a rendered strip into a
        // yielded one; the assertions are identical either way. The first two
        // are the returns this yield rule introduced — the ones that had no
        // teardown at all.
        type Starve = fn(&mut App) -> (u16, u16, u16);
        let cases: [(&str, Starve); 3] = [
            ("budget starves the Tasks strip", |_app| (100, 40, 0)),
            ("budget starves a switched-to panel", |app| {
                app.work_surface.panel = super::RailPanel::Pinned;
                (100, 40, 0)
            }),
            ("placement off", |app| {
                app.work_surface.placement = WorkSurfacePlacement::Off;
                (100, 40, AMPLE_BUDGET)
            }),
        ];

        for (label, starve) in cases {
            let mut app = app();
            app.work_surface.placement = WorkSurfacePlacement::Top;
            // `app()` reads the developer's real settings.toml. Pin the height
            // too, or the strip this test renders to earn its hitboxes depends
            // on whoever runs the suite.
            app.work_surface.top_height = 8;
            add_todos(&mut app, 4);

            // Earn a real strip, so the hitboxes under test are the ones the
            // renderer actually produces rather than a fixture's guess.
            render_text(&mut app, 100, 12);
            assert!(
                !app.work_surface.hitboxes.is_empty(),
                "{label}: setup never rendered a strip to tear down"
            );
            app.work_surface.focused = true;
            app.work_surface.resizing = true;
            app.work_surface.divider_hovered = true;

            let (width, height, budget) = starve(&mut app);
            assert_eq!(
                super::height(&mut app, width, height, budget),
                0,
                "{label}: expected the strip to yield"
            );
            assert!(
                app.work_surface.hitboxes.is_empty(),
                "{label}: left {} stale hitboxes behind",
                app.work_surface.hitboxes.len()
            );
            assert!(
                app.work_surface.last_area.is_none(),
                "{label}: stale last_area"
            );
            assert!(!app.work_surface.focused, "{label}: focus survived");
            assert!(!app.work_surface.resizing, "{label}: resize drag survived");
            assert!(
                !app.work_surface.divider_hovered,
                "{label}: divider hover survived"
            );
        }
    }

    /// The collapse cliff belongs to the terminal, not to the user. A short
    /// `top_height` is a request, and honouring it costs the transcript
    /// nothing: the budget is what protects the transcript's floor.
    #[test]
    fn a_short_top_height_is_honoured_rather_than_collapsed() {
        for top_height in [2_u16, 3] {
            let mut app = app();
            app.work_surface.placement = WorkSurfacePlacement::Top;
            app.work_surface.panel = super::RailPanel::Pinned;
            app.work_surface.top_height = top_height;
            app.composer_border = true;
            let budget = working_budget(&app, 40);
            assert_eq!(
                super::height(&mut app, 100, 40, budget),
                top_height,
                "a {top_height}-row strip was asked for and must be what renders"
            );
        }
    }

    #[test]
    fn minimum_top_surface_keeps_a_numbered_todo_selectable() {
        let mut app = app();
        add_todos(&mut app, 2);

        let text = render_text(&mut app, 40, 2);

        assert!(text.contains("1 ·"), "{text:?}");
        assert!(!text.contains("To-do · 0/"), "{text:?}");
        assert_eq!(app.work_surface.hitboxes.len(), 1);
        assert_eq!(app.work_surface.hitboxes[0].row_y, 0);
    }

    #[test]
    fn compact_progress_window_reveals_current_without_reordering() {
        let mut app = app();
        {
            let mut todos = app.todos.try_lock().expect("todos");
            todos.add("finished".to_string(), TodoStatus::Completed);
            todos.add("current".to_string(), TodoStatus::InProgress);
            todos.add("next".to_string(), TodoStatus::Pending);
        }

        // Three rows means one pinned progress receipt, one selectable row,
        // and the divider. The current item must win that compact window while
        // retaining its canonical ordinal.
        let text = render_text(&mut app, 80, 3);

        assert!(text.contains("To-do · 1/3 · 2 left"), "{text:?}");
        assert!(
            text.contains(&format!("2 · {} current", crate::tui::glyphs::SELECTION)),
            "{text:?}"
        );
        assert_eq!(app.work_surface.scroll_offset, 1);
        assert_eq!(app.work_surface.hitboxes[0].row_y, 1);
    }

    #[test]
    fn settled_file_tools_aggregate_once_and_keep_only_safe_targets() {
        let mut app = app();
        app.current_session_id = Some(SESSION.to_string());
        app.workspace = PathBuf::from("/workspace/project");
        for (id, name, input, status) in [
            (
                "read-1",
                "read_file",
                serde_json::json!({"path": "/workspace/project/src/lib.rs"}),
                ToolStatus::Success,
            ),
            (
                "search-1",
                "grep_files",
                serde_json::json!({"pattern": "WorkSurfaceState"}),
                ToolStatus::Success,
            ),
            (
                "write-1",
                "edit_file",
                serde_json::json!({"path": "src/lib.rs"}),
                ToolStatus::Success,
            ),
            (
                "read-external",
                "read_file",
                serde_json::json!({"path": "/Users/alice/private.txt"}),
                ToolStatus::Failed,
            ),
        ] {
            app.add_message(HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
                name: name.to_string(),
                status,
                input_summary: None,
                output: Some("done".to_string()),
                prompts: None,
                spillover_path: None,
                output_summary: None,
                is_diff: false,
            })));
            let index = app.history.len() - 1;
            app.tool_details_by_cell.insert(
                index,
                ToolDetailRecord {
                    tool_id: id.to_string(),
                    tool_name: name.to_string(),
                    input,
                    output: Some("done".to_string()),
                },
            );
        }

        let rows = super::model::project(&mut app);
        let activity = rows
            .iter()
            .find(|row| row.id.0 == "activity:aggregate")
            .expect("aggregated activity row");
        assert!(
            activity.label.contains("Read 1 files")
                && activity.label.contains("Searched 1 patterns")
                && activity.label.contains("Wrote 1 files"),
            "aggregated label: {}",
            activity.label
        );
        assert!(!activity.detail.contains("/Users/alice"));
        assert!(!activity.label.contains("WorkSurfaceState"));
    }

    #[test]
    fn agent_rows_show_role_assignment_and_open_real_agent_details() {
        let mut app = app();
        app.current_session_id = Some(SESSION.to_string());
        app.subagent_cache.push(SubAgentResult {
            name: "agent_worker".to_string(),
            agent_id: "agent_worker".to_string(),
            context_mode: "fresh".to_string(),
            fork_context: false,
            workspace: None,
            git_branch: None,
            agent_type: FleetRole::Builder,
            assignment: SubAgentAssignment {
                objective: "Wire settled file activity".to_string(),
                role: Some("worker".to_string()),
            },
            model: "test-model".to_string(),
            nickname: Some("Blue Whale".to_string()),
            status: SubAgentStatus::Running,
            worker_status: Some(AgentWorkerStatus::RunningTool),
            runtime_permissions: None,
            parent_run_id: None,
            spawn_depth: 1,
            result: None,
            steps_taken: 2,
            checkpoint: None,
            needs_input: None,
            duration_ms: 50,
            from_prior_session: false,
        });
        app.agent_progress_meta.insert(
            "agent_worker".to_string(),
            crate::tui::app::AgentProgressMeta {
                current_activity: Some(AgentCurrentActivity::bounded(
                    AgentCurrentActivityStatus::RunningTool,
                    None,
                    Some("File.apply_patch".to_string()),
                    Some(2),
                )),
                current_tool: Some("apply_patch".to_string()),
                files_touched: 2,
                ..crate::tui::app::AgentProgressMeta::default()
            },
        );

        let rows = super::model::project(&mut app);
        let row = rows
            .iter()
            .find(|row| row.id.0 == "worker:agent_worker")
            .expect("agent work row");
        // #36: number + fleet role + short name — never the raw agent id.
        assert_eq!(row.label, "1 worker · Blue Whale");
        assert!(row.detail.contains("Wire settled file activity"));
        assert!(row.detail.contains("using File.apply_patch"));
        assert!(row.detail.contains("step 2"));
        assert!(row.detail.contains("2 files changed"));
        assert_eq!(
            row.primary_action,
            Some(SidebarRowAction::OpenAgentDetail {
                agent_id: "agent_worker".to_string(),
            })
        );
    }

    fn cached_worker(
        id: &str,
        role: &str,
        nickname: Option<&str>,
        parent_run_id: Option<&str>,
        status: SubAgentStatus,
    ) -> SubAgentResult {
        SubAgentResult {
            // `name` is the raw session id in production snapshots — the
            // strip must never render it (#36).
            name: id.to_string(),
            agent_id: id.to_string(),
            context_mode: "fresh".to_string(),
            fork_context: false,
            workspace: None,
            git_branch: None,
            agent_type: FleetRole::Builder,
            assignment: SubAgentAssignment {
                objective: format!("objective for {id}"),
                role: Some(role.to_string()),
            },
            model: "test-model".to_string(),
            nickname: nickname.map(str::to_string),
            status,
            worker_status: None,
            runtime_permissions: None,
            parent_run_id: parent_run_id.map(str::to_string),
            spawn_depth: u32::from(parent_run_id.is_some()) + 1,
            result: None,
            steps_taken: 1,
            checkpoint: None,
            needs_input: None,
            duration_ms: 50,
            from_prior_session: false,
        }
    }

    #[test]
    fn agent_rows_number_by_fleet_role_and_never_leak_raw_ids() {
        // #36: the strip shows sequential number + fleet role; the raw agent
        // id hash is noise and must never render as the "name". Flat fan-outs
        // carry no nesting chrome.
        let mut app = app();
        app.current_session_id = Some(SESSION.to_string());
        app.subagent_cache.push(cached_worker(
            "agent_e0b2dcf1",
            "builder",
            None,
            None,
            SubAgentStatus::Running,
        ));
        app.subagent_cache.push(cached_worker(
            "agent_99aa77bb",
            "scout",
            None,
            None,
            SubAgentStatus::Running,
        ));

        let rows = super::model::project(&mut app);
        let first = rows
            .iter()
            .find(|row| row.id.0 == "worker:agent_e0b2dcf1")
            .expect("first agent row");
        let second = rows
            .iter()
            .find(|row| row.id.0 == "worker:agent_99aa77bb")
            .expect("second agent row");
        assert_eq!(first.label, "1 builder");
        assert_eq!(second.label, "2 scout");
        assert!(first.detail.starts_with("running"), "{}", first.detail);
        for row in rows.iter().filter(|row| row.id.0.starts_with("worker:")) {
            assert!(!row.label.contains("agent_e0b2dcf1"), "{}", row.label);
            assert!(!row.label.contains("agent_99aa77bb"), "{}", row.label);
            assert!(
                !row.label.contains('↳'),
                "flat fan-out must not show nesting chrome: {}",
                row.label
            );
        }
    }

    #[test]
    fn agent_rows_order_and_indent_nested_spawns_under_their_parent() {
        // #36: nesting is visible only when actually present — the child
        // renders directly under its parent with a `↳` indent.
        let mut app = app();
        app.current_session_id = Some(SESSION.to_string());
        app.subagent_cache.push(cached_worker(
            "agent_child",
            "scout",
            None,
            Some("agent_parent"),
            SubAgentStatus::Running,
        ));
        app.subagent_cache.push(cached_worker(
            "agent_parent",
            "builder",
            None,
            None,
            SubAgentStatus::Running,
        ));

        let rows = super::model::project(&mut app);
        let worker_labels = rows
            .iter()
            .filter(|row| row.id.0.starts_with("worker:"))
            .map(|row| row.label.as_str())
            .collect::<Vec<_>>();
        let parent_pos = worker_labels
            .iter()
            .position(|label| *label == "1 builder")
            .expect("parent row label");
        let child_pos = worker_labels
            .iter()
            .position(|label| *label == "↳ 2 scout")
            .expect("indented child row label");
        assert!(
            child_pos == parent_pos + 1,
            "child must render directly under its parent: {worker_labels:?}"
        );
    }

    #[test]
    fn agent_rows_completed_agents_render_quietly_without_spawn_metadata() {
        // #36: quiet completion — a finished agent keeps status + objective;
        // in-flight metadata (tool, step counters, file tallies) must not
        // linger as a receipt dump.
        let mut app = app();
        app.current_session_id = Some(SESSION.to_string());
        app.subagent_cache.push(cached_worker(
            "agent_done",
            "builder",
            None,
            None,
            SubAgentStatus::Completed,
        ));
        app.agent_progress_meta.insert(
            "agent_done".to_string(),
            crate::tui::app::AgentProgressMeta {
                current_activity: Some(AgentCurrentActivity::bounded(
                    AgentCurrentActivityStatus::Done,
                    Some("apply_patch finished".to_string()),
                    Some("File.apply_patch".to_string()),
                    Some(7),
                )),
                current_tool: Some("apply_patch".to_string()),
                files_touched: 4,
                ..crate::tui::app::AgentProgressMeta::default()
            },
        );

        let rows = super::model::project(&mut app);
        let row = rows
            .iter()
            .find(|row| row.id.0 == "worker:agent_done")
            .expect("completed agent row");
        assert!(row.detail.contains("completed"), "{}", row.detail);
        assert!(
            row.detail.contains("objective for agent_done"),
            "{}",
            row.detail
        );
        assert!(!row.detail.contains("using "), "{}", row.detail);
        assert!(!row.detail.contains("step 7"), "{}", row.detail);
        assert!(!row.detail.contains("files changed"), "{}", row.detail);
    }

    #[test]
    fn progress_only_work_rows_use_typed_activity_not_display_substrings() {
        let mut app = app();
        app.current_session_id = Some(SESSION.to_string());
        app.agent_progress.insert(
            "agent_progress_only".to_string(),
            "queued waiting failed completed".to_string(),
        );

        let rows = super::model::project(&mut app);
        let row = rows
            .iter()
            .find(|row| row.id.0 == "worker:agent_progress_only")
            .expect("progress-only work row");
        assert_eq!(row.detail, "running");

        app.agent_progress_meta.insert(
            "agent_progress_only".to_string(),
            crate::tui::app::AgentProgressMeta {
                current_activity: Some(AgentCurrentActivity::bounded(
                    AgentCurrentActivityStatus::Waiting,
                    Some("approval required".to_string()),
                    None,
                    Some(5),
                )),
                ..crate::tui::app::AgentProgressMeta::default()
            },
        );

        let rows = super::model::project(&mut app);
        let row = rows
            .iter()
            .find(|row| row.id.0 == "worker:agent_progress_only")
            .expect("typed progress-only work row");
        assert!(row.detail.contains("waiting for input"), "{}", row.detail);
        assert!(row.detail.contains("approval required"), "{}", row.detail);
        assert!(row.detail.contains("step 5"), "{}", row.detail);
    }

    #[test]
    fn agent_details_keyboard_mouse_and_return_selection_converge() {
        fn add_worker(app: &mut App) {
            app.current_session_id = Some(SESSION.to_string());
            app.subagent_cache.push(SubAgentResult {
                name: "agent_converge".to_string(),
                agent_id: "agent_converge".to_string(),
                context_mode: "fresh".to_string(),
                fork_context: false,
                workspace: None,
                git_branch: Some("codex/details".to_string()),
                agent_type: FleetRole::Builder,
                assignment: SubAgentAssignment {
                    objective: "Verify keyboard and mouse convergence".to_string(),
                    role: Some("worker".to_string()),
                },
                model: "test-model".to_string(),
                nickname: Some("Blue Whale".to_string()),
                status: SubAgentStatus::Running,
                worker_status: Some(AgentWorkerStatus::Running),
                runtime_permissions: None,
                parent_run_id: None,
                spawn_depth: 1,
                result: None,
                steps_taken: 1,
                checkpoint: None,
                needs_input: None,
                duration_ms: 100,
                from_prior_session: false,
            });
        }

        let mut keyboard = app();
        add_worker(&mut keyboard);
        let _ = render_text(&mut keyboard, 100, 6);
        let _ = super::handle_key(
            &mut keyboard,
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::ALT),
        );
        let keyboard_action = super::handle_key(
            &mut keyboard,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        )
        .expect("Work key handled")
        .expect("agent details action");
        let keyboard_selection = keyboard.work_surface.selected.clone();

        let mut mouse = app();
        add_worker(&mut mouse);
        let _ = render_text(&mut mouse, 100, 6);
        let row_y = mouse
            .work_surface
            .hitboxes
            .iter()
            .find(|hit| hit.id.0 == "worker:agent_converge")
            .expect("agent hitbox")
            .row_y;
        let mouse_action = super::handle_mouse(
            &mut mouse,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 2,
                row: row_y,
                modifiers: KeyModifiers::NONE,
            },
        )
        .action
        .expect("mouse agent details action");
        assert_eq!(mouse_action, keyboard_action);
        assert_eq!(mouse.work_surface.selected, keyboard_selection);

        crate::tui::mouse_ui::apply_sidebar_row_action(&mut mouse, mouse_action);
        let selected_before_close = mouse.work_surface.selected.clone();
        let events = mouse
            .view_stack
            .handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        let [crate::tui::views::ViewEvent::AgentDetailsClosed { agent_id }] = events.as_slice()
        else {
            panic!("Left should close Agent Details with a receipt: {events:?}");
        };
        super::interaction::agent_details_closed(&mut mouse, agent_id);
        assert_eq!(mouse.work_surface.selected, selected_before_close);
        assert!(mouse.work_surface.opened.is_none());
    }

    #[test]
    fn active_session_without_work_keeps_surface_invisible() {
        let mut app = app();
        app.current_session_id = Some(SESSION.to_string());

        let rows = super::model::project(&mut app);

        assert!(rows.is_empty());
        assert_eq!(super::height(&mut app, 120, 32, AMPLE_BUDGET), 0);
    }

    #[test]
    fn empty_work_stays_hidden_after_cached_session_state_is_cleared() {
        let mut app = app();
        app.current_session_id = Some(SESSION.to_string());
        app.work_surface.cached_graph = Some(operation_graph(NodeState::Active));

        let rows = super::model::project(&mut app);

        assert!(rows.is_empty());
        assert!(app.work_surface.cached_graph.is_none());
    }

    #[test]
    fn empty_work_reserves_no_side_rail() {
        for placement in [
            super::WorkSurfacePlacement::Left,
            super::WorkSurfacePlacement::Right,
        ] {
            let mut app = app();
            app.current_session_id = Some(SESSION.to_string());
            app.work_surface.placement = placement;
            let area = ratatui::layout::Rect::new(0, 0, 120, 32);

            assert_eq!(
                super::height(&mut app, area.width, area.height, AMPLE_BUDGET),
                0
            );
            assert_eq!(super::split_chat(&mut app, area, 0), (area, None));
        }
    }

    fn terminal_text(terminal: &Terminal<TestBackend>) -> String {
        let buf = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                text.push_str(buf[(x, y)].symbol());
            }
        }
        text
    }

    /// Render-level smoke coverage for the ported rail panels — reinstates
    /// the sidebar render smoke tests removed with the classic shell
    /// (739616787). Every non-Tasks panel must render its title in every
    /// placement the rail supports.
    #[test]
    fn rail_panels_render_in_all_placements() {
        for panel in [
            super::RailPanel::Agents,
            super::RailPanel::Context,
            super::RailPanel::Pinned,
        ] {
            for placement in [
                super::WorkSurfacePlacement::Top,
                super::WorkSurfacePlacement::Left,
                super::WorkSurfacePlacement::Right,
            ] {
                let mut app = app();
                app.work_surface.placement = placement;
                app.work_surface.panel = panel;
                let area = ratatui::layout::Rect::new(0, 0, 100, 24);

                // Render coverage, not yield coverage: a 24-row terminal with
                // work on screen has rows to spare, so the panel is expected
                // to draw. The idle-empty budget is exercised end-to-end in
                // `ui::tests::rail_strip_yields_the_ambient_floor_*`.
                let budget = working_budget(&app, area.height);
                let strip = super::height(&mut app, area.width, area.height, budget);
                let (_chat, rail) = super::split_chat(&mut app, area, 0);
                let backend = TestBackend::new(area.width, area.height);
                let mut terminal = Terminal::new(backend).expect("terminal");
                terminal
                    .draw(|frame| {
                        if strip > 0 {
                            super::render(
                                frame,
                                ratatui::layout::Rect::new(0, 0, area.width, strip),
                                &mut app,
                            );
                        } else if let Some(rail) = rail {
                            super::render(frame, rail, &mut app);
                        }
                    })
                    .expect("draw");
                let text = terminal_text(&terminal);
                assert!(
                    text.contains(panel.title()),
                    "{panel:?} in {placement:?} should render its title; got: {text}"
                );
            }
        }
    }

    #[test]
    fn off_placement_reserves_no_rail_in_any_panel() {
        for panel in [
            super::RailPanel::Tasks,
            super::RailPanel::Agents,
            super::RailPanel::Context,
            super::RailPanel::Pinned,
        ] {
            let mut app = app();
            add_todos(&mut app, 2);
            app.work_surface.placement = super::WorkSurfacePlacement::Off;
            app.work_surface.panel = panel;
            let area = ratatui::layout::Rect::new(0, 0, 120, 32);

            assert_eq!(
                super::height(&mut app, area.width, area.height, AMPLE_BUDGET),
                0
            );
            assert_eq!(super::split_chat(&mut app, area, 0), (area, None));
            assert_eq!(app.work_surface.last_area, None);
        }
    }

    #[test]
    fn context_panel_renders_session_facts_in_side_rail() {
        let mut app = app();
        app.work_surface.placement = super::WorkSurfacePlacement::Right;
        app.work_surface.panel = super::RailPanel::Context;
        let area = ratatui::layout::Rect::new(0, 0, 100, 24);

        let budget = working_budget(&app, area.height);
        let strip = super::height(&mut app, area.width, area.height, budget);
        assert_eq!(strip, 0, "side placements take no top strip");
        let (_chat, rail) = super::split_chat(&mut app, area, 0);
        let rail = rail.expect("context panel reserves a side rail");

        let backend = TestBackend::new(area.width, area.height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| super::render(frame, rail, &mut app))
            .expect("draw");
        let text = terminal_text(&terminal);
        assert!(text.contains("Context"), "panel title; got: {text}");
        assert!(text.contains("lsp:"), "session facts; got: {text}");
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
            rows[0].label.starts_with("Work · Needs input:")
                || rows[0]
                    .label
                    .starts_with("Work · 0 active · 1 needs input · 0 ready · 0 recent"),
            "{}",
            rows[0].label
        );
        assert!(
            rows[0].label.contains("blocked") || rows[0].label.contains("needs input"),
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
        assert!(
            rows[0].label.contains("Needs input") || rows[0].label.contains("1 needs input"),
            "{}",
            rows[0].label
        );
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

    /// A durable failed operation, as a fleet agent task from a crashed or
    /// sibling instance leaves behind in the persisted graph (#4416).
    fn durable_failed_operation_graph() -> crate::work_graph::WorkGraphSnapshot {
        let mut graph = WorkGraph::from_snapshot(operation_graph(NodeState::Failed));
        let operation = WorkNodeId::derive(SESSION, "operation");
        graph
            .apply(
                WorkGraphChange::BindOperation {
                    node: operation,
                    binding: OperationBinding {
                        external: "fleet:run_1/task_1".to_string(),
                        durable: true,
                        last_observation: None,
                    },
                },
                ChangeCtx {
                    session_id: SESSION.to_string(),
                    now: 6,
                    idempotency_key: None,
                },
            )
            .expect("durable binding");
        graph.into_snapshot()
    }

    // Regression for #4416: a persisted failed-agent record stamped by
    // another session instance (boot id) must not appear in the default
    // work listing of a fresh session in the same workspace.
    #[test]
    fn prior_instance_failed_rows_stay_out_of_the_default_listing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let manager =
            crate::session_manager::SessionManager::new(dir.path().to_path_buf()).expect("manager");
        manager
            .record_session_boot_owner(SESSION, "boot_other_instance")
            .expect("stamp other instance");

        let mut app = app();
        app.work_surface.session_owner_probe_dir = Some(dir.path().to_path_buf());
        let graph = durable_failed_operation_graph();
        restore_saved_graph(&mut app, &graph);

        let rows = super::model::project(&mut app);
        assert!(
            rows.iter()
                .all(|row| !row.label.contains("Verify installed build")),
            "prior-instance failed row leaked into the default listing: {rows:#?}"
        );
        assert!(
            rows.iter()
                .all(|row| !row.label.contains("needs input") && !row.label.contains("1 active")),
            "prior-instance residue must not count as live work: {rows:#?}"
        );
        // The record stays reachable through the explicit catalog, clearly
        // marked historical.
        let historical = app
            .work_surface
            .catalog_rows
            .iter()
            .find(|row| row.label.contains("Verify installed build"))
            .expect("historical row remains in the catalog");
        assert!(
            historical.detail.starts_with("prior session · "),
            "historical row must be labeled: {}",
            historical.detail
        );
    }

    // Ownership control for #4416: the same failed record owned by this
    // session instance still renders as actionable work.
    #[test]
    fn current_instance_failed_rows_still_render_in_the_default_listing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let manager =
            crate::session_manager::SessionManager::new(dir.path().to_path_buf()).expect("manager");
        manager
            .record_session_boot_owner(SESSION, crate::session_manager::current_session_boot_id())
            .expect("stamp current instance");

        let mut app = app();
        app.work_surface.session_owner_probe_dir = Some(dir.path().to_path_buf());
        let graph = durable_failed_operation_graph();
        restore_graph(&mut app, &graph);

        let rows = super::model::project(&mut app);
        assert!(
            rows.iter()
                .any(|row| row.label.contains("Verify installed build")),
            "this instance's own failed work must stay visible: {rows:#?}"
        );
    }

    // Regression for review of #5063: if a prior session persisted no graph,
    // the first graph captured later belongs to this process and must not be
    // mistaken for restored residue.
    #[test]
    fn first_live_graph_after_empty_prior_restore_stays_visible() {
        let dir = tempfile::tempdir().expect("tempdir");
        let manager =
            crate::session_manager::SessionManager::new(dir.path().to_path_buf()).expect("manager");
        manager
            .record_session_boot_owner(SESSION, "boot_other_instance")
            .expect("stamp other instance");

        let mut app = app();
        app.work_surface.session_owner_probe_dir = Some(dir.path().to_path_buf());
        app.current_session_id = Some(SESSION.to_string());
        app.restore_work_state(SESSION, std::path::Path::new("."), None)
            .expect("restore empty prior session");

        let graph = durable_failed_operation_graph();
        restore_graph(&mut app, &graph);
        let rows = super::model::project(&mut app);
        assert!(
            rows.iter()
                .any(|row| row.label.contains("Verify installed build")),
            "this instance's first live graph must stay visible: {rows:#?}"
        );
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
        assert!(
            rows[0].label.contains("Needs input") || rows[0].label.contains("1 needs input"),
            "{}",
            rows[0].label
        );
        let row = rows
            .iter()
            .find(|row| row.selectable)
            .expect("operation row");
        assert_eq!(row.mark, crate::tui::glyphs::ATTENTION);
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
        app.work_surface.placement = WorkSurfacePlacement::Right;
        app.work_surface.effective_placement = WorkSurfacePlacement::Right;
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
    fn narrow_file_activity_prioritizes_the_canonical_aggregate_label() {
        let mut app = app();
        app.workspace = PathBuf::from("/workspace/project");
        let result = crate::tools::spec::ToolResult::success("ok").with_metadata(
            serde_json::json!({
                "mutation": {
                    "diff": "--- a/update.rs\n+++ b/update.rs\n@@ -1 +1 @@\n-old\n+new\n--- /dev/null\n+++ b/create.rs\n@@ -0,0 +1 @@\n+created\n--- a/delete.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-deleted\n",
                    "files": [
                        { "path": "update.rs", "outcome": "updated" },
                        { "path": "create.rs", "outcome": "created" },
                        { "path": "delete.rs", "outcome": "deleted" }
                    ],
                    "renames": [{ "from": "old.rs", "to": "new.rs" }]
                }
            }),
        );
        let receipt = FileMutationReceipt::from_success(&app.workspace, &result).expect("receipt");
        app.add_message(HistoryCell::Tool(ToolCell::PatchSummary(
            PatchSummaryCell {
                path: "4 files".to_string(),
                summary: "ok".to_string(),
                status: ToolStatus::Success,
                error: None,
                receipt: Some(receipt),
            },
        )));
        app.tool_details_by_cell.insert(
            0,
            ToolDetailRecord {
                tool_id: "file-multi".to_string(),
                tool_name: "File".to_string(),
                input: serde_json::json!({"action": "patch"}),
                output: Some("ok".to_string()),
            },
        );

        app.work_surface.placement = WorkSurfacePlacement::Right;
        app.work_surface.effective_placement = WorkSurfacePlacement::Right;
        let text = render_text(&mut app, 80, 6);
        assert!(text.contains("Wrote 4 files"), "{text}");
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
            assert_eq!(super::height(&mut app, 100, 24, AMPLE_BUDGET), 0);
            let area = ratatui::layout::Rect::new(0, 0, 100, 12);
            let (chat, rail) = super::split_chat(&mut app, area, 0);
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
    fn divider_drag_resizes_top_left_and_right_surfaces() {
        let mut top = app();
        add_todos(&mut top, 3);
        let _ = render_text(&mut top, 80, 3);
        let down = super::handle_mouse(
            &mut top,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 20,
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
        );
        assert!(down.consumed);
        let _ = super::handle_mouse(
            &mut top,
            MouseEvent {
                kind: MouseEventKind::Drag(MouseButton::Left),
                column: 20,
                row: 7,
                modifiers: KeyModifiers::NONE,
            },
        );
        assert_eq!(top.work_surface.top_height, 8);

        for (placement, drag_column, expected_width) in [
            (WorkSurfacePlacement::Left, 39, 40),
            (WorkSurfacePlacement::Right, 10, 26),
        ] {
            let mut side = app();
            add_todos(&mut side, 2);
            side.work_surface.placement = placement;
            side.work_surface.effective_placement = placement;
            let _ = render_text(&mut side, 30, 8);
            let divider_column = if placement == WorkSurfacePlacement::Left {
                29
            } else {
                0
            };
            let _ = super::handle_mouse(
                &mut side,
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: divider_column,
                    row: 2,
                    modifiers: KeyModifiers::NONE,
                },
            );
            let _ = super::handle_mouse(
                &mut side,
                MouseEvent {
                    kind: MouseEventKind::Drag(MouseButton::Left),
                    column: drag_column,
                    row: 2,
                    modifiers: KeyModifiers::NONE,
                },
            );
            assert_eq!(
                side.work_surface.side_width, expected_width,
                "{placement:?}"
            );
        }
    }

    #[test]
    fn divider_hover_and_drag_render_a_discoverable_handle() {
        let mut app = app();
        add_todos(&mut app, 3);
        let resting = render_text(&mut app, 80, 3);
        assert!(resting.contains('─'), "{resting}");

        let hover = super::handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Moved,
                column: 20,
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
        );
        assert!(hover.consumed);
        assert!(app.work_surface.divider_hovered);
        let hovered = render_text(&mut app, 80, 3);
        assert!(hovered.contains('━'), "{hovered}");

        let _ = super::handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 20,
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
        );
        let dragging = render_text(&mut app, 80, 3);
        assert!(dragging.contains('━'), "{dragging}");
    }

    #[test]
    fn top_bar_excludes_generic_operations() {
        let mut operation_app = app();
        let graph = operation_graph(NodeState::Failed);
        restore_graph(&mut operation_app, &graph);

        assert_eq!(super::height(&mut operation_app, 100, 24, AMPLE_BUDGET), 0);
        assert!(operation_app.work_surface.latest_rows.is_empty());

        let mut todo_app = app();
        add_todos(&mut todo_app, 2);
        assert!(super::height(&mut todo_app, 100, 24, AMPLE_BUDGET) > 0);
        assert!(
            todo_app
                .work_surface
                .latest_rows
                .iter()
                .all(|row| row.id.0.starts_with("graph:") || row.id.0.starts_with("worker:"))
        );
        assert!(
            todo_app
                .work_surface
                .latest_rows
                .iter()
                .all(|row| !row.label.starts_with("Work ·"))
        );
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
