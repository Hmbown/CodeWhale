//! One owner per fact: the default shell paints each session fact in exactly
//! one chrome row (SHELL-DESIGN-20260901 §2.0 item 3, §2.2, §2.3, §2.3b).
//!
//! Under the composer: row 1 is the posture bar (permission, mode, live
//! counts, the one hint that applies now), row 2 is the metrics line (model,
//! ctx, cost, ttft, tok/s, output tokens); the roster and to-do rows follow
//! only when they have content. Every fact below is asserted to appear in
//! the composed frame exactly once.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::{Terminal, backend::TestBackend};

use crate::config::Config;
use crate::tui::app::App;
use crate::tui::history::HistoryCell;

fn frame_app() -> App {
    let mut app = crate::test_support::test_app_with_options(crate::tui::app::TuiOptions {
        model: "deepseek-v4-flash".to_string(),
        start_in_agent_mode: true,
        max_subagents: 4,
        ..crate::test_support::test_tui_options(PathBuf::from("."))
    });
    app.onboarding = crate::tui::app::OnboardingState::None;
    app.launch.visible = false;
    app.ui_locale = crate::localization::Locale::En;
    app
}

fn subagent(
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

/// A working turn with two running sub-agents and a session that has
/// already reported one turn's metrics.
fn working_app() -> App {
    let mut app = frame_app();
    app.history = vec![HistoryCell::User {
        content: "audit the shell".to_string(),
    }];
    app.resync_history_revisions();
    app.is_loading = true;
    app.turn_started_at = Some(Instant::now() - Duration::from_secs(75));
    app.subagent_cache = vec![
        subagent("agent_a", crate::tools::subagent::SubAgentStatus::Running),
        subagent("agent_b", crate::tools::subagent::SubAgentStatus::Running),
    ];
    app.session_metrics
        .record_model_call(1_200, 30_000, Some(400), None);
    app.session.last_output_throughput =
        crate::resource_telemetry::TokenThroughput::new(1_200, Duration::from_secs(30));
    app
}

fn draw(app: &mut App, width: u16, height: u16) -> Vec<String> {
    let config = Config::default();
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| {
            let _ = super::render(frame, app, &config);
        })
        .unwrap();
    let buf = terminal.backend().buffer();
    (0..height)
        .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect()
}

fn count_rows_containing(rows: &[String], needle: &str) -> usize {
    rows.iter().filter(|row| row.contains(needle)).count()
}

/// Every chrome fact paints in exactly one row of the composed default
/// frame: the context reading, the mode and permission chips, the model,
/// the cost, the agent count, and the help hint.
#[test]
fn composed_frame_paints_each_fact_in_exactly_one_row() {
    for (width, height) in [(80u16, 24u16), (120, 32)] {
        let mut app = working_app();
        let rows = draw(&mut app, width, height);
        let pct = super::info_context_percent(&app);
        let (_, model) = app.effective_route_identity_display();
        let (mode, permission) = crate::tui::underwater::posture_chips(&app);
        let mode = mode.expect("mode chip").0.into_owned();
        let permission = permission.expect("permission chip").0.into_owned();
        // The context reading stays silent below 50% fullness and paints
        // exactly once at or above it.
        let mut facts = vec![
            ("mode chip", format!("· {mode} (")),
            ("permission chip", format!("▶▶ {permission} (")),
            ("model", model),
            ("cost", super::session_cost_label(&app)),
            ("agent count", "2 agents".to_string()),
            (
                "help hint",
                crate::tui::shell_key_routing::info_help_hint(app.ui_locale),
            ),
            ("output rate", "40 tok/s".to_string()),
            ("ttft", "ttft 400ms".to_string()),
        ];
        if pct >= 50 {
            facts.push(("context reading", format!("ctx {pct}%")));
            facts.push(("context percent", format!("{pct}%")));
        } else {
            assert_eq!(
                count_rows_containing(&rows, "ctx "),
                0,
                "{width}x{height}: ctx stays silent below 50%:\n{}",
                rows.join("\n")
            );
        }
        for (name, needle) in facts {
            if needle.is_empty() {
                continue;
            }
            assert_eq!(
                count_rows_containing(&rows, &needle),
                1,
                "{width}x{height}: {name} {needle:?} must paint in exactly one row:\n{}",
                rows.join("\n")
            );
        }
        // Rows under the composer: posture bar, then metrics line, then the
        // roster — never the other way round.
        let posture = rows
            .iter()
            .position(|row| row.contains("▶▶"))
            .expect("posture bar");
        let metrics = rows
            .iter()
            .position(|row| row.contains("tok/s"))
            .expect("metrics line");
        let composer = app
            .viewport
            .last_composer_area
            .expect("composer area")
            .bottom();
        assert_eq!(
            posture,
            usize::from(composer),
            "posture bar is row 1 under the composer"
        );
        assert_eq!(metrics, posture + 1, "metrics line is row 2");
        // The bar carries no phase word and no elapsed; the metrics line no
        // repository, branch or provider.
        assert!(!rows[posture].contains("underway"), "{}", rows[posture]);
        assert!(!rows[posture].contains("1m 15s"), "{}", rows[posture]);
        assert!(!rows[metrics].contains('⑂'), "{}", rows[metrics]);
        // No dead key hints anywhere in the frame.
        for row in &rows {
            assert!(!row.contains("F1"), "F1 is not receivable: {row}");
            assert!(!row.contains("? help"), "bare ? is composer text: {row}");
        }
    }
}

/// Idle: the two rows are there, the roster is not, and the last turn's
/// metrics survive between turns.
#[test]
fn idle_frame_keeps_two_chrome_rows_and_last_turn_metrics() {
    let mut app = working_app();
    app.is_loading = false;
    app.turn_started_at = None;
    app.subagent_cache.clear();
    let rows = draw(&mut app, 100, 32);
    let composer = app.viewport.last_composer_area.unwrap().bottom() as usize;
    assert!(rows[composer].starts_with("▶▶"), "{}", rows[composer]);
    // The idle fixture sits at 0% context, so the reading stays silent.
    assert!(
        !rows[composer + 1].contains("ctx "),
        "{}",
        rows[composer + 1]
    );
    assert!(
        rows[composer + 1].contains("40 tok/s"),
        "{}",
        rows[composer + 1]
    );
    assert!(
        rows[composer + 1].contains("↓ 1.2K"),
        "{}",
        rows[composer + 1]
    );
    assert_eq!(
        composer + 2,
        rows.len(),
        "nothing under the metrics line when idle"
    );
    assert!(
        !rows[composer].contains("Esc to interrupt"),
        "{}",
        rows[composer]
    );
}

/// At the cap the posture bar's hint says what to do; the reading still
/// paints once, in the metrics line.
#[test]
fn context_cap_warns_once_in_the_posture_bar() {
    let mut app = working_app();
    app.active_route_limits = Some(codewhale_config::route::RouteLimits {
        context_tokens: Some(60),
        ..Default::default()
    });
    // 140 columns: on a backend-less platform (linux CI) the filesystem
    // scope notice paints `files: workspace (unenforced)` and the shed
    // ladder drops the hint first; the width keeps the hint inside the
    // budget with the notice present, so the warning is asserted on every
    // platform.
    let rows = draw(&mut app, 140, 32);
    let pct = super::info_context_percent(&app);
    assert!(pct >= 80, "fixture must sit at the cap: {pct}");
    assert_eq!(
        count_rows_containing(&rows, "surface soon — /compact"),
        1,
        "cap warning rows:\n{}",
        rows.join("\n")
    );
    assert_eq!(
        count_rows_containing(&rows, &format!("{pct}%")),
        1,
        "context reading rows:\n{}",
        rows.join("\n")
    );
}

/// The double-tap window advertises itself in the posture bar's hint slot.
#[test]
fn double_tap_window_shows_the_send_now_hint() {
    let mut app = working_app();
    app.arm_double_tap_window();
    let rows = draw(&mut app, 120, 32);
    let composer = app.viewport.last_composer_area.unwrap().bottom() as usize;
    assert!(
        rows[composer].contains("Enter again to send now · Ctrl+Enter steers"),
        "{}",
        rows[composer]
    );
    assert!(!rows[composer].contains("Esc to interrupt"));
}
