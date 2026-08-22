//! `/goal` — codex-style thread goals: set, inspect, pause, resume, and close
//! a durable objective. The engine owns the goal: setting or resuming one
//! starts work through the runtime's continuation steering, never by echoing
//! the objective back as a user message.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tools::goal::GoalStatus;
use crate::tui::app::{App, AppAction};

use crate::commands::CommandResult;

/// Declare, show, pause, resume, or close a goal.
fn goal_command(app: &mut App, arg: Option<&str>) -> CommandResult {
    match arg {
        Some("clear") | Some("reset") => CommandResult::action(AppAction::SetGoalStatus {
            status: GoalStatus::Active,
            clear: true,
        }),
        Some("done") | Some("complete") => close_goal(app, GoalStatus::Complete),
        Some("pause") | Some("paused") => close_goal(app, GoalStatus::Paused),
        Some("resume") | Some("continue") => resume_goal(app),
        Some("help") | Some("?") | Some("usage") => CommandResult::message(goal_usage()),
        Some("status") | Some("show") => goal_status(app),
        Some("block") | Some("blocked") => close_goal(app, GoalStatus::Blocked),
        Some(text) if !text.is_empty() => {
            let (objective, budget) = parse_goal_budget(text);
            if objective.is_empty() || objective.chars().all(|c| c == '|') {
                return CommandResult::error(goal_usage());
            }
            // The command layer never mutates the visible projection. The UI
            // first persists and accepts this typed intent; only the engine's
            // authoritative GoalUpdated event may change what the user sees.
            CommandResult::action(AppAction::SetGoalObjective {
                objective,
                token_budget: budget,
            })
        }
        _ => {
            if !app.pending_goal_controls.is_empty() {
                CommandResult::message(app.tr(MessageId::GoalControlAccepted))
            } else if app.goal.objective.is_some() {
                goal_status(app)
            } else if app.api_messages.is_empty() {
                // Nothing has happened yet: there is no context to derive an
                // objective from, so answer with usage instead of spending a
                // model turn on a question we already know the answer to.
                CommandResult::message(goal_usage())
            } else {
                // Context-dependent bare /goal: with no active goal, the
                // invocation itself is the ask — derive the objective from
                // the conversation instead of demanding a restatement
                // (mirrors bare /workflow). The end-of-turn GoalUpdated
                // snapshot syncs the created goal into the sidebar.
                let message = "The user invoked /goal with no objective — declare a goal for the \
                     CURRENT work. Synthesize the objective from the conversation context (the \
                     task in flight, recent findings, open items) and set it by calling \
                     `create_goal` with the full objective (and a token_budget only if one was \
                     discussed). Then continue working toward it. Only if the conversation \
                     genuinely contains no work yet, ask the user what the goal should be."
                    .to_string();
                CommandResult::with_message_and_action(
                    "Declaring a goal from the current context...",
                    AppAction::SendMessage(message),
                )
            }
        }
    }
}

/// Plain status line: objective, state, elapsed, budget, continuations, and
/// — for an active goal that no turn is driving right now — how to continue.
fn goal_status(app: &App) -> CommandResult {
    let Some(obj) = app.goal.objective.as_deref() else {
        return CommandResult::message(goal_usage());
    };
    let elapsed = app
        .goal
        .time_used_seconds
        .gt(&0)
        .then(|| crate::elapsed::format_elapsed_secs(app.goal.time_used_seconds))
        .or_else(|| {
            app.goal
                .started_at
                .map(|t| crate::elapsed::format_elapsed_secs(t.elapsed().as_secs()))
        })
        .unwrap_or_else(|| "unknown".to_string());
    let budget_str = app
        .goal
        .token_budget
        .map(|b| {
            let used = if app.goal.tokens_used > 0 {
                app.goal.tokens_used
            } else {
                u64::from(app.session.total_conversation_tokens)
            };
            let pct = if b > 0 {
                (used as f64 / f64::from(b) * 100.0).min(100.0)
            } else {
                0.0
            };
            format!(" · tokens {used}/{b} ({pct:.0}%)")
        })
        .unwrap_or_default();
    let mut state = goal_status_label(app.goal.status).to_string();
    if let (GoalStatus::Paused, Some(reason)) = (app.goal.status, app.goal.pause_reason) {
        state = format!("{state} ({})", reason.label());
    }
    let mut line = format!(
        "Goal {state}: \"{obj}\" · elapsed {elapsed}{budget_str} · continuations {}",
        app.goal.continuation_count
    );
    if app.goal.status == GoalStatus::Active && !app.is_loading && !app.goal_continuation_waiting {
        line.push_str(" · ");
        line.push_str(&app.tr(MessageId::GoalStatusIdleHint));
    }
    CommandResult::message(line)
}

/// Close out the goal at `status`. Pure control plane: the engine stops (or
/// re-arms) the continuation loop from the `SetGoalStatus` op; no model turn
/// is dispatched.
fn close_goal(app: &mut App, status: GoalStatus) -> CommandResult {
    if effective_goal_objective(app).is_none_or(str::is_empty) {
        return CommandResult::error("No goal set. Use /goal <objective> [budget: N] first.");
    }
    if effective_goal_status(app) == status {
        if !app.pending_goal_controls.is_empty() {
            return CommandResult::message(app.tr(MessageId::GoalControlAccepted));
        }
        return goal_status(app);
    }

    CommandResult::action(AppAction::SetGoalStatus {
        status,
        clear: false,
    })
}

/// Resume a paused goal. The engine restarts the continuation loop itself
/// (`SetGoalStatus` → schedule kickoff); the objective is never re-sent as a
/// user message.
fn resume_goal(app: &mut App) -> CommandResult {
    if effective_goal_objective(app)
        .map(str::trim)
        .is_none_or(str::is_empty)
    {
        return CommandResult::error("No paused goal set. Use /goal <objective> first.");
    }

    // Resuming an already-active goal is a no-op: the continuation loop is
    // already running, and re-asserting Active could stack a second
    // autonomous turn. Report progress instead.
    if effective_goal_status(app) == GoalStatus::Active {
        if !app.pending_goal_controls.is_empty() {
            return CommandResult::message(app.tr(MessageId::GoalControlAccepted));
        }
        return goal_status(app);
    }

    CommandResult::action(AppAction::SetGoalStatus {
        status: GoalStatus::Active,
        clear: false,
    })
}

fn effective_goal_objective(app: &App) -> Option<&str> {
    if app.pending_goal_controls.is_empty() {
        app.goal.objective.as_deref()
    } else {
        app.last_known_goal_state
            .as_ref()
            .map(|goal| goal.objective.as_str())
    }
}

fn effective_goal_status(app: &App) -> GoalStatus {
    if app.pending_goal_controls.is_empty() {
        return app.goal.status;
    }
    match app.last_known_goal_state.as_ref().map(|goal| goal.status) {
        Some(crate::session_manager::SessionGoalStatus::Paused) => GoalStatus::Paused,
        Some(crate::session_manager::SessionGoalStatus::Complete) => GoalStatus::Complete,
        Some(crate::session_manager::SessionGoalStatus::Blocked) => GoalStatus::Blocked,
        Some(crate::session_manager::SessionGoalStatus::Active) | None => GoalStatus::Active,
    }
}

fn goal_usage() -> &'static str {
    "No goal set. /goal <objective> [budget: N] starts one; the agent works toward it \
     across turns until it is verified complete, blocked, or you stop it.\n\
     /goal — progress of the current goal\n\
     /goal pause — pause without continuing\n\
     /goal resume — resume and continue\n\
     /goal done — mark complete (skips the model's verification)\n\
     /goal blocked — mark blocked\n\
     /goal clear — remove the current goal."
}

fn goal_status_label(status: GoalStatus) -> &'static str {
    match status {
        GoalStatus::Active => "active",
        GoalStatus::Complete => "complete",
        GoalStatus::Paused => "paused",
        GoalStatus::Blocked => "blocked",
    }
}

/// Parse text like "Implement login | budget: 50000" into (objective, budget).
fn parse_goal_budget(text: &str) -> (String, Option<u32>) {
    // Only an explicit, well-formed budget suffix splits the objective.
    // `budget:` followed by something that is not a number is prose that
    // belongs to the objective — truncating it would silently rewrite what
    // the user asked for.
    for separator in [" | budget:", " budget:", "budget:"] {
        if let Some((objective, rest)) = text.split_once(separator) {
            let budget = rest
                .split_whitespace()
                .next()
                .and_then(|value| value.parse::<u32>().ok());
            if let Some(budget) = budget {
                return (objective.trim().to_string(), Some(budget));
            }
        }
    }
    (text.trim().to_string(), None)
}

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "goal",
    aliases: &[],
    usage: "/goal [objective|status|pause|resume|done|blocked|clear] [budget: N]",
    description_id: MessageId::CmdGoalDescription,
};

pub(in crate::commands) struct GoalCmd;

impl RegisterCommand for GoalCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        goal_command(app, arg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Role;

    fn create_test_app() -> App {
        let options = crate::tui::app::TuiOptions {
            skills_dir: std::path::PathBuf::from("/tmp/test-skills"),
            ..crate::test_support::test_tui_options(std::path::PathBuf::from("/tmp/test-workspace"))
        };
        let config = crate::config::Config::default();
        App::new(options, &config)
    }

    #[test]
    fn test_set_goal_dispatches_control_plane_not_user_echo() {
        let mut app = create_test_app();
        let result = goal_command(&mut app, Some("Fix the login bug"));
        assert!(result.message.is_none());
        assert_eq!(app.goal.objective, None);
        // The engine owns the kickoff: the objective must reach it as a
        // SetGoalObjective control op, never as a SendMessage user echo.
        assert!(matches!(
            result.action,
            Some(AppAction::SetGoalObjective { ref objective, token_budget: None })
                if objective == "Fix the login bug"
        ));
    }

    #[test]
    fn test_goal_budget_parsing_reaches_the_op() {
        let mut app = create_test_app();
        let result = goal_command(&mut app, Some("Ship 0.9.10 | budget: 5000"));
        assert_eq!(app.goal.objective, None);
        assert_eq!(app.goal.token_budget, None);
        assert!(matches!(
            result.action,
            Some(AppAction::SetGoalObjective { ref objective, token_budget: Some(5000) })
                if objective == "Ship 0.9.10"
        ));
    }

    #[test]
    fn pause_and_clear_are_control_ops_without_optimistic_state() {
        let mut app = create_test_app();
        app.goal.objective = Some("Keep the build green".to_string());
        app.goal.status = GoalStatus::Active;
        let paused = goal_command(&mut app, Some("pause"));
        assert!(paused.message.is_none());
        assert_eq!(app.goal.status, GoalStatus::Active);
        assert!(matches!(
            paused.action,
            Some(AppAction::SetGoalStatus {
                status: GoalStatus::Paused,
                clear: false
            })
        ));
        assert!(app.goal.finished_at.is_none());

        let cleared = goal_command(&mut app, Some("clear"));
        assert!(cleared.message.is_none());
        assert_eq!(app.goal.objective.as_deref(), Some("Keep the build green"));
        assert!(matches!(
            cleared.action,
            Some(AppAction::SetGoalStatus {
                status: GoalStatus::Active,
                clear: true
            })
        ));
    }

    #[test]
    fn test_goal_without_argument_synthesizes_goal_from_context() {
        // Bare /goal with no active goal is context-dependent: the model
        // derives the objective from the conversation and sets it via
        // create_goal — it must not error with a usage demand.
        let mut app = create_test_app();
        app.api_messages.push(crate::models::Message {
            role: Role::User,
            content: vec![crate::models::ContentBlock::Text {
                text: "make the tests pass".to_string(),
                cache_control: None,
            }],
        });
        let result = goal_command(&mut app, None);
        assert!(!result.is_error);
        let Some(AppAction::SendMessage(message)) = result.action else {
            panic!("expected SendMessage action");
        };
        assert!(message.contains("Synthesize the objective from the conversation"));
        assert!(message.contains("`create_goal`"));
    }

    #[test]
    fn bare_goal_on_an_empty_session_prints_usage_without_a_model_turn() {
        // No conversation yet: there is nothing to derive an objective from,
        // so the answer is usage — free, and not a question to the model.
        let mut app = create_test_app();
        let result = goal_command(&mut app, None);
        assert!(!result.is_error);
        assert!(result.action.is_none());
        assert!(result.message.unwrap().contains("/goal <objective>"));
    }

    #[test]
    fn goal_status_reports_objective_and_state() {
        let mut app = create_test_app();
        app.goal.objective = Some("Make the suite green".to_string());
        app.goal.token_budget = Some(100);
        app.goal.status = GoalStatus::Active;
        let result = goal_command(&mut app, Some("status"));
        let line = result.message.unwrap();
        assert!(line.contains("Make the suite green"));
        assert!(line.contains("active"));
        assert!(line.contains("100"));
    }

    #[test]
    fn resume_on_an_active_goal_is_a_no_op_report() {
        // Re-asserting Active while the loop is already running must not
        // schedule a second autonomous turn.
        let mut app = create_test_app();
        app.goal.objective = Some("Keep the build green".to_string());
        app.goal.status = GoalStatus::Active;
        let resumed = goal_command(&mut app, Some("resume"));
        assert!(!resumed.is_error);
        assert!(
            resumed.action.is_none(),
            "no control op on already-active goal"
        );
        assert!(resumed.message.unwrap().contains("Keep the build green"));
    }

    #[test]
    fn invalid_budget_suffix_stays_part_of_the_objective() {
        let mut app = create_test_app();
        let result = goal_command(&mut app, Some("Fix budget: handling in settings"));
        assert_eq!(app.goal.objective, None);
        assert_eq!(app.goal.token_budget, None);
        assert!(matches!(
            result.action,
            Some(AppAction::SetGoalObjective { ref objective, token_budget: None })
                if objective == "Fix budget: handling in settings"
        ));
    }

    #[test]
    fn completing_a_goal_without_one_is_an_error() {
        let mut app = create_test_app();
        let result = goal_command(&mut app, Some("done"));
        assert!(result.is_error);
    }
}
