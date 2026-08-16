//! /goal command, with /hunt kept as a compatibility alias (#2092).

use std::io::Write;

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tools::goal::GoalStatus;
use crate::tui::app::{App, AppAction, HuntVerdict};
use serde_json::{Value, json};

use crate::commands::CommandResult;

/// Declare, show, pause, resume, or close a goal.
fn hunt(app: &mut App, arg: Option<&str>) -> CommandResult {
    match arg {
        Some("clear") | Some("reset") => {
            app.hunt.quarry = None;
            app.hunt.token_budget = None;
            app.hunt.tokens_used = 0;
            app.hunt.time_used_seconds = 0;
            app.hunt.continuation_count = 0;
            app.hunt.started_at = None;
            app.hunt.finished_at = None;
            app.hunt.verdict = HuntVerdict::default();
            CommandResult::with_message_and_action(
                "Goal cleared.",
                AppAction::SetGoalStatus {
                    status: GoalStatus::Active,
                    clear: true,
                },
            )
        }
        Some("declare-hunted")
        | Some("declare_hunted")
        | Some("force-complete")
        | Some("force_complete") => declare_hunted(app),
        Some("done") | Some("complete") | Some("hunted") => {
            close_hunt(app, HuntVerdict::Hunted, GoalStatus::Complete)
        }
        Some("pause") | Some("paused") | Some("wound") | Some("wounded") => {
            close_hunt(app, HuntVerdict::Wounded, GoalStatus::Paused)
        }
        Some("resume") | Some("continue") => resume_hunt(app),
        Some("help") | Some("?") | Some("usage") => CommandResult::message(goal_usage()),
        Some("status") | Some("show") => goal_status(app),
        Some("block") | Some("blocked") | Some("escape") | Some("escaped") => {
            close_hunt(app, HuntVerdict::Escaped, GoalStatus::Blocked)
        }
        Some(text) if !text.is_empty() => {
            let (objective, budget) = parse_hunt_budget(text);
            if objective.is_empty() || objective.chars().all(|c| c == '|') {
                return CommandResult::error(goal_usage());
            }
            app.hunt.quarry = Some(objective.clone());
            app.hunt.token_budget = budget;
            app.hunt.tokens_used = 0;
            app.hunt.time_used_seconds = 0;
            app.hunt.continuation_count = 0;
            app.hunt.started_at = Some(std::time::Instant::now());
            app.hunt.finished_at = None;
            app.hunt.verdict = HuntVerdict::Hunting;
            let budget_str = budget
                .map(|b| format!(" (budget: {b} tokens)"))
                .unwrap_or_default();
            CommandResult::with_message_and_action(
                format!("Goal set: \"{objective}\"{budget_str} - tracking progress."),
                AppAction::SendMessage(objective),
            )
        }
        _ => {
            if app.hunt.quarry.is_some() {
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
    let Some(obj) = app.hunt.quarry.as_deref() else {
        return CommandResult::message(goal_usage());
    };
    let elapsed = app
        .hunt
        .time_used_seconds
        .gt(&0)
        .then(|| crate::elapsed::format_elapsed_secs(app.hunt.time_used_seconds))
        .or_else(|| {
            app.hunt
                .started_at
                .map(|t| crate::elapsed::format_elapsed_secs(t.elapsed().as_secs()))
        })
        .unwrap_or_else(|| "unknown".to_string());
    let budget_str = app
        .hunt
        .token_budget
        .map(|b| {
            let used = if app.hunt.tokens_used > 0 {
                app.hunt.tokens_used
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
    let mut state = hunt_verdict_label(app.hunt.verdict).to_string();
    if let (HuntVerdict::Wounded, Some(reason)) = (app.hunt.verdict, app.hunt.pause_reason) {
        state = format!("{state} ({})", reason.label());
    }
    let mut line = format!(
        "Goal {state}: \"{obj}\" · elapsed {elapsed}{budget_str} · continuations {}",
        app.hunt.continuation_count
    );
    if app.hunt.verdict == HuntVerdict::Hunting && !app.is_loading {
        line.push_str(" · ");
        line.push_str(&app.tr(MessageId::GoalStatusIdleHint));
    }
    CommandResult::message(line)
}

fn declare_hunted(app: &mut App) -> CommandResult {
    let previous = app.hunt.verdict;
    let result = close_hunt(app, HuntVerdict::Hunted, GoalStatus::Complete);
    if !result.is_error {
        crate::audit::log_sensitive_event(
            "goal.declare_hunted",
            declare_hunted_audit_details(previous, app),
        );
    }
    result
}

fn declare_hunted_audit_details(previous: HuntVerdict, app: &App) -> Value {
    json!({
        "previous_verdict": hunt_verdict_name(previous),
        "current_verdict": hunt_verdict_name(app.hunt.verdict),
        "has_quarry": app.hunt.quarry.as_deref().is_some_and(|quarry| !quarry.is_empty()),
    })
}

fn close_hunt(app: &mut App, verdict: HuntVerdict, status: GoalStatus) -> CommandResult {
    if app.hunt.quarry.as_deref().is_none_or(str::is_empty) {
        return CommandResult::error("No goal set. Use /goal <objective> [budget: N] first.");
    }

    let prev = app.hunt.verdict;
    let should_write_trophy = matches!(verdict, HuntVerdict::Hunted) && prev != verdict;
    if should_write_trophy && let Err(err) = write_trophy_card(app, verdict) {
        return CommandResult::error(err);
    }
    app.hunt.verdict = verdict;
    // Freeze the sidebar timer at the moment of close-out so it stops ticking
    // for hunted/escaped goals. Wounded (paused) goals are not terminal — the
    // timer re-arms on resume — but we still record the pause instant so a
    // paused goal doesn't read as still-running in the sidebar.
    if app.hunt.finished_at.is_none() {
        app.hunt.finished_at = Some(std::time::Instant::now());
    }

    // Push the new status to the engine's SharedGoalState so the cross-turn
    // continuation loop respects it: pause/blocked stops the loop, complete
    // ends it, resume restarts it.
    let action = AppAction::SetGoalStatus {
        status,
        clear: false,
    };

    match verdict {
        HuntVerdict::Hunted => {
            let elapsed = goal_elapsed_at_close(&app.hunt);
            CommandResult::with_message_and_action(
                format!("Goal complete. Elapsed: {elapsed}"),
                action,
            )
        }
        HuntVerdict::Wounded => CommandResult::with_message_and_action(
            "Goal paused. Progress is saved; use /goal resume to continue.",
            action,
        ),
        HuntVerdict::Escaped => CommandResult::with_message_and_action("Goal blocked.", action),
        HuntVerdict::Hunting => CommandResult::with_message_and_action("Goal active.", action),
    }
}

fn resume_hunt(app: &mut App) -> CommandResult {
    let Some(objective) = app
        .hunt
        .quarry
        .as_deref()
        .map(str::trim)
        .filter(|objective| !objective.is_empty())
        .map(str::to_string)
    else {
        return CommandResult::error("No paused goal set. Use /goal <objective> first.");
    };

    app.hunt.verdict = HuntVerdict::Hunting;
    if app.hunt.started_at.is_none() {
        app.hunt.started_at = Some(std::time::Instant::now());
    }
    // Re-arm the elapsed timer: a resumed goal should keep ticking from where
    // it left off (started_at is preserved), not stay frozen at the pause.
    app.hunt.finished_at = None;
    CommandResult::with_message_and_action("Goal resumed.", AppAction::SendMessage(objective))
}

fn goal_usage() -> &'static str {
    "No goal set. /goal <objective> [budget: N] sets one; the agent works toward it \
     across turns until it is verified complete, blocked, or you stop it.\n\
     /goal - progress of the current goal\n\
     /goal pause - pause without continuing\n\
     /goal resume - resume and continue\n\
     /goal done - mark complete (declare-hunted skips verification)\n\
     /goal blocked - mark blocked\n\
     /goal clear - remove the current goal."
}

fn hunt_verdict_label(verdict: HuntVerdict) -> &'static str {
    match verdict {
        HuntVerdict::Hunting => "active",
        HuntVerdict::Hunted => "complete",
        HuntVerdict::Wounded => "paused",
        HuntVerdict::Escaped => "blocked",
    }
}

/// Humanized elapsed time for a closed goal, frozen at the finish instant so
/// the close-out message doesn't drift further each time it's read.
fn goal_elapsed_at_close(hunt: &crate::tui::app::HuntState) -> String {
    match (hunt.started_at, hunt.finished_at) {
        (Some(started), Some(finished)) => crate::elapsed::format_elapsed_secs(
            finished.saturating_duration_since(started).as_secs(),
        ),
        (Some(started), None) => crate::elapsed::format_elapsed_secs(started.elapsed().as_secs()),
        (None, _) => "unknown".to_string(),
    }
}

fn hunt_verdict_name(verdict: HuntVerdict) -> &'static str {
    match verdict {
        HuntVerdict::Hunting => "hunting",
        HuntVerdict::Hunted => "hunted",
        HuntVerdict::Wounded => "wounded",
        HuntVerdict::Escaped => "escaped",
    }
}

/// Parse text like "Implement login | budget: 50000" into (objective, budget).
fn parse_hunt_budget(text: &str) -> (String, Option<u32>) {
    if let Some((obj, rest)) = text.split_once(" | budget:") {
        let budget = rest
            .split_whitespace()
            .next()
            .and_then(|s| s.parse::<u32>().ok());
        (obj.trim().to_string(), budget)
    } else if let Some((obj, rest)) = text.split_once("budget:") {
        let budget = rest
            .split_whitespace()
            .next()
            .and_then(|s| s.parse::<u32>().ok());
        (obj.trim().to_string(), budget)
    } else {
        (text.trim().to_string(), None)
    }
}

/// Write a legacy trophy card to `~/.codewhale/trophies/<date>-<time>-<slug>.md`
/// for the current goal result (#2092).
fn write_trophy_card(app: &App, verdict: HuntVerdict) -> Result<std::path::PathBuf, String> {
    let quarry = app
        .hunt
        .quarry
        .as_deref()
        .ok_or_else(|| "No goal set. Use /goal <objective> [budget: N] first.".to_string())?;
    // Collapse consecutive non-alphanumeric chars into a single '-'
    let mut slug = String::new();
    let mut last_dash = false;
    for c in quarry.chars() {
        if c.is_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        return Err(
            "Cannot write trophy card: goal objective has no filename-safe characters.".into(),
        );
    }
    let now = chrono::Local::now();
    let time = now.format("%H%M%S");
    let date = now.format("%Y-%m-%d");
    let date_str = date.to_string();
    let now_str = now.to_string();
    let dir = codewhale_config::ensure_state_dir("trophies")
        .map_err(|err| format!("Could not resolve trophy directory: {err}"))?;
    // Include time in filename to avoid collisions on same-date hunts.
    let filename = format!("{date}-{time}-{slug}.md");
    let path = dir.join(&filename);

    let elapsed = app
        .hunt
        .started_at
        .as_ref()
        .map(|t| crate::elapsed::format_elapsed_secs(t.elapsed().as_secs()))
        .unwrap_or_else(|| "unknown".to_string());
    let verdict_str = hunt_verdict_name(verdict);
    let tokens = if app.hunt.tokens_used > 0 {
        u32::try_from(app.hunt.tokens_used).unwrap_or(u32::MAX)
    } else {
        app.session.total_conversation_tokens
    };
    let budget_str = app
        .hunt
        .token_budget
        .map(|b| format!("{b}"))
        .unwrap_or_else(|| "—".to_string());

    let mut f = std::fs::File::create(&path)
        .map_err(|err| format!("Could not create trophy card {}: {err}", path.display()))?;
    write_trophy_card_contents(
        &mut f,
        TrophyCard {
            quarry,
            verdict: verdict_str,
            date: &date_str,
            elapsed: &elapsed,
            tokens,
            budget: &budget_str,
            now: &now_str,
        },
    )
    .map_err(|err| format!("Could not write trophy card {}: {err}", path.display()))?;

    Ok(path)
}

struct TrophyCard<'a> {
    quarry: &'a str,
    verdict: &'a str,
    date: &'a str,
    elapsed: &'a str,
    tokens: u32,
    budget: &'a str,
    now: &'a str,
}

fn write_trophy_card_contents(mut f: impl Write, card: TrophyCard<'_>) -> std::io::Result<()> {
    writeln!(f, "# Goal result: {}", card.quarry)?;
    writeln!(f)?;
    writeln!(f, "- **Verdict**: {}", card.verdict)?;
    writeln!(f, "- **Date**: {}", card.date)?;
    writeln!(f, "- **Elapsed**: {}", card.elapsed)?;
    writeln!(f, "- **Tokens used**: {}", card.tokens)?;
    writeln!(f, "- **Token budget**: {}", card.budget)?;
    writeln!(f)?;
    writeln!(f, "_Generated by Codewhale `/goal` - {}_", card.now)?;
    Ok(())
}

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "goal",
    aliases: &["hunt", "mubiao", "狩猎"],
    usage: "/goal [objective|status|pause|resume|done|blocked|clear] [budget: N]",
    description_id: MessageId::CmdGoalDescription,
};

pub(in crate::commands) struct GoalCmd;

impl RegisterCommand for GoalCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        hunt(app, arg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_app() -> App {
        let options = crate::tui::app::TuiOptions {
            skills_dir: std::path::PathBuf::from("/tmp/test-skills"),
            ..crate::test_support::test_tui_options(std::path::PathBuf::from("/tmp/test-workspace"))
        };
        let config = crate::config::Config::default();
        App::new(options, &config)
    }

    #[test]
    fn test_set_hunt() {
        let mut app = create_test_app();
        let result = hunt(&mut app, Some("Fix the login bug"));
        assert!(result.message.unwrap().contains("Goal set"));
        assert_eq!(app.hunt.quarry.as_deref(), Some("Fix the login bug"));
        assert_eq!(
            app.hunt.verdict.goal_status(),
            crate::tools::goal::GoalStatus::Active
        );
        assert!(matches!(
            result.action,
            Some(AppAction::SendMessage(msg)) if msg == "Fix the login bug"
        ));
    }

    #[test]
    fn test_hunt_without_argument_synthesizes_goal_from_context() {
        // Bare /goal with no active goal is context-dependent: the model
        // derives the objective from the conversation and sets it via
        // create_goal — it must not error with a usage demand.
        let mut app = create_test_app();
        app.api_messages.push(crate::models::Message {
            role: "user".to_string(),
            content: vec![crate::models::ContentBlock::Text {
                text: "make the tests pass".to_string(),
                cache_control: None,
            }],
        });
        let result = hunt(&mut app, None);
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
        let result = hunt(&mut app, None);
        assert!(!result.is_error);
        assert!(result.action.is_none());
        assert!(result.message.unwrap().contains("/goal <objective>"));

        let result = hunt(&mut app, Some("help"));
        assert!(result.action.is_none());
        assert!(result.message.unwrap().contains("/goal resume"));
        assert!(
            app.hunt.quarry.is_none(),
            "help must not become an objective"
        );
    }

    #[test]
    fn goal_status_is_plain_and_says_how_to_continue_when_idle() {
        let mut app = create_test_app();
        let _ = hunt(&mut app, Some("ship the release notes"));
        app.is_loading = false;
        let result = hunt(&mut app, Some("status"));
        let text = result.message.unwrap();
        assert!(
            text.starts_with("Goal active: \"ship the release notes\""),
            "{text}"
        );
        assert!(text.contains("/goal resume"), "{text}");
        assert!(!text.contains('['), "no bracket tags: {text}");

        app.is_loading = true;
        let text = hunt(&mut app, None).message.unwrap();
        assert!(!text.contains("/goal resume"), "{text}");
    }

    #[test]
    fn test_hunt_without_argument_shows_state_when_goal_active() {
        // With an active goal, bare /goal stays a status readout.
        let mut app = create_test_app();
        let _ = hunt(&mut app, Some("Fix the login bug"));
        let result = hunt(&mut app, None);
        assert!(result.action.is_none());
        assert!(
            result
                .message
                .as_deref()
                .unwrap()
                .contains("Fix the login bug")
        );
    }

    #[test]
    fn test_command_usage_mentions_host_verbs() {
        assert!(COMMAND_INFO.usage.contains("status"));
        assert!(COMMAND_INFO.usage.contains("pause"));
        assert!(COMMAND_INFO.usage.contains("resume"));
        assert!(COMMAND_INFO.usage.contains("done"));
        assert!(COMMAND_INFO.usage.contains("blocked"));
        assert!(COMMAND_INFO.usage.contains("clear"));
    }

    #[test]
    fn test_set_hunt_with_budget() {
        let mut app = create_test_app();
        let _ = hunt(&mut app, Some("Refactor auth | budget: 50000"));
        assert_eq!(app.hunt.quarry.as_deref(), Some("Refactor auth"));
        assert_eq!(app.hunt.token_budget, Some(50_000));
        assert!(app.hunt.started_at.is_some());
    }

    #[test]
    fn test_set_hunt_rejects_budget_only_objective() {
        let mut app = create_test_app();
        app.hunt.quarry = Some("existing objective".to_string());
        app.hunt.token_budget = Some(10_000);

        let result = hunt(&mut app, Some("budget: 50000"));
        assert!(result.is_error);
        assert!(
            result
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("/goal <objective>")
        );
        assert_eq!(app.hunt.quarry.as_deref(), Some("existing objective"));
        assert_eq!(app.hunt.token_budget, Some(10_000));
    }

    #[test]
    fn test_clear_hunt() {
        let mut app = create_test_app();
        app.hunt.quarry = Some("test".to_string());
        app.hunt.token_budget = Some(100);
        app.hunt.tokens_used = 5;
        app.hunt.time_used_seconds = 3;
        app.hunt.continuation_count = 1;
        app.hunt.finished_at = Some(std::time::Instant::now());
        let _ = hunt(&mut app, Some("clear"));
        assert!(app.hunt.quarry.is_none());
        assert!(app.hunt.token_budget.is_none());
        assert_eq!(app.hunt.tokens_used, 0);
        assert_eq!(app.hunt.time_used_seconds, 0);
        assert_eq!(app.hunt.continuation_count, 0);
        assert!(app.hunt.finished_at.is_none());
        assert_eq!(
            app.hunt.verdict.goal_status(),
            crate::tools::goal::GoalStatus::Active
        );
    }

    #[test]
    fn test_verdict_requires_existing_hunt() {
        let mut app = create_test_app();

        let result = hunt(&mut app, Some("wounded"));

        assert!(result.is_error);
        assert_eq!(app.hunt.verdict, HuntVerdict::Hunting);
        assert!(app.hunt.quarry.is_none());
    }

    #[test]
    fn test_goal_pause_and_resume_update_status() {
        let mut app = create_test_app();
        let _ = hunt(&mut app, Some("Finish release prep"));

        let paused = hunt(&mut app, Some("pause"));
        // Pause now dispatches SetGoalStatus to push Paused into SharedGoalState.
        assert!(matches!(
            paused.action,
            Some(AppAction::SetGoalStatus {
                status: crate::tools::goal::GoalStatus::Paused,
                clear: false
            })
        ));
        assert_eq!(app.hunt.verdict, HuntVerdict::Wounded);
        assert_eq!(
            app.hunt.verdict.goal_status(),
            crate::tools::goal::GoalStatus::Paused
        );

        let resumed = hunt(&mut app, Some("resume"));
        assert_eq!(app.hunt.verdict, HuntVerdict::Hunting);
        assert_eq!(
            app.hunt.verdict.goal_status(),
            crate::tools::goal::GoalStatus::Active
        );
        assert!(matches!(
            resumed.action,
            Some(AppAction::SendMessage(msg)) if msg == "Finish release prep"
        ));
    }

    #[test]
    fn test_close_hunt_freezes_elapsed_timer() {
        // close_hunt writes a trophy card under CODEWHALE_HOME/trophies. Isolate
        // the home dir so sandbox/readonly HOME cannot turn a successful close
        // into a trophy path error (and so we never touch the real home).
        let _lock = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().expect("isolated CODEWHALE_HOME");
        let _codewhale_home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", temp.path());

        let mut app = create_test_app();
        let _ = hunt(&mut app, Some("Freeze the timer on close"));
        assert!(
            app.hunt.finished_at.is_none(),
            "an active goal must not have a frozen finish time"
        );

        // Closing the goal as hunted must set finished_at so the sidebar timer
        // stops ticking instead of reading "completed in {growing elapsed}".
        let result = hunt(&mut app, Some("done"));
        assert!(
            result
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("Goal complete. Elapsed:"),
            "close-out message should report a frozen elapsed; got {:?}",
            result.message
        );
        assert_eq!(app.hunt.verdict, HuntVerdict::Hunted);
        assert!(
            app.hunt.finished_at.is_some(),
            "hunted goal should freeze the elapsed timer"
        );

        // Resume must re-arm the timer so a resumed goal keeps ticking.
        let _ = hunt(&mut app, Some("resume"));
        assert_eq!(app.hunt.verdict, HuntVerdict::Hunting);
        assert!(
            app.hunt.finished_at.is_none(),
            "resume should clear the frozen timer"
        );
    }

    #[test]
    fn test_show_hunt_uses_hunt_verdict_label() {
        let mut app = create_test_app();
        app.hunt.quarry = Some("Review verifier claim".to_string());
        app.hunt.verdict = HuntVerdict::Escaped;

        let result = hunt(&mut app, None);

        let message = result.message.as_deref().unwrap_or_default();
        assert!(message.starts_with("Goal blocked:"), "{message}");
    }

    #[test]
    fn test_failed_trophy_write_does_not_mutate_verdict() {
        let mut app = create_test_app();
        app.hunt.quarry = Some("!!!".to_string());
        app.hunt.verdict = HuntVerdict::Hunting;

        let result = hunt(&mut app, Some("hunted"));

        assert!(result.is_error);
        assert_eq!(app.hunt.verdict, HuntVerdict::Hunting);
        assert_eq!(app.hunt.quarry.as_deref(), Some("!!!"));
    }

    #[test]
    fn test_escaped_verdict_does_not_write_trophy_card() {
        let mut app = create_test_app();
        app.hunt.quarry = Some("!!!".to_string());
        app.hunt.verdict = HuntVerdict::Hunting;

        let result = hunt(&mut app, Some("escaped"));

        assert!(!result.is_error);
        assert_eq!(app.hunt.verdict, HuntVerdict::Escaped);
        assert_eq!(app.hunt.quarry.as_deref(), Some("!!!"));
        assert!(matches!(
            result.action,
            Some(AppAction::SetGoalStatus {
                status: crate::tools::goal::GoalStatus::Blocked,
                clear: false
            })
        ));
    }

    #[test]
    fn test_declare_hunted_alias_uses_trophy_override_path() {
        let mut app = create_test_app();
        app.hunt.quarry = Some("!!!".to_string());
        app.hunt.verdict = HuntVerdict::Hunting;

        let result = hunt(&mut app, Some("declare-hunted"));

        assert!(result.is_error);
        assert_eq!(app.hunt.verdict, HuntVerdict::Hunting);
        assert_eq!(app.hunt.quarry.as_deref(), Some("!!!"));
        assert!(
            result
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("Cannot write trophy card")
        );
    }

    #[test]
    fn test_declare_hunted_audit_details_use_hunt_vocabulary() {
        let mut app = create_test_app();
        app.hunt.quarry = Some("Verify release gate".to_string());
        app.hunt.verdict = HuntVerdict::Hunted;

        let details = declare_hunted_audit_details(HuntVerdict::Wounded, &app);

        assert_eq!(details["previous_verdict"], "wounded");
        assert_eq!(details["current_verdict"], "hunted");
        assert_eq!(details["has_quarry"], true);
    }

    #[test]
    fn test_show_hunt_when_none() {
        // Bare /goal with no active goal but a live conversation declares one
        // from context instead of printing usage.
        let mut app = create_test_app();
        app.api_messages.push(crate::models::Message {
            role: "user".to_string(),
            content: vec![crate::models::ContentBlock::Text {
                text: "fix the flaky test".to_string(),
                cache_control: None,
            }],
        });
        let result = hunt(&mut app, None);
        assert!(
            result
                .message
                .unwrap()
                .contains("Declaring a goal from the current context")
        );
    }

    #[test]
    fn test_parse_budget() {
        assert_eq!(
            parse_hunt_budget("Do a thing | budget: 50000"),
            ("Do a thing".to_string(), Some(50_000))
        );
        assert_eq!(
            parse_hunt_budget("Simple goal"),
            ("Simple goal".to_string(), None)
        );
        assert_eq!(
            parse_hunt_budget("Goal budget:1000"),
            ("Goal".to_string(), Some(1000))
        );
    }
}
