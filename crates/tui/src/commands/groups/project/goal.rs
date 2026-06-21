//! /goal command, with /hunt kept as a compatibility alias (#2092).

use std::io::Write;

use crate::tools::goal::GoalStatus;
use crate::tui::app::{App, AppAction, HuntVerdict};

use super::CommandResult;

/// Declare, show, pause, resume, or close a goal.
pub fn hunt(app: &mut App, arg: Option<&str>) -> CommandResult {
    match arg {
        Some("clear") | Some("reset") => {
            app.hunt.quarry = None;
            app.hunt.token_budget = None;
            app.hunt.tokens_used = 0;
            app.hunt.time_used_seconds = 0;
            app.hunt.continuation_count = 0;
            app.hunt.started_at = None;
            app.hunt.verdict = HuntVerdict::default();
            CommandResult::with_message_and_action(
                "Goal cleared.",
                AppAction::SetGoalStatus {
                    status: GoalStatus::Active,
                    clear: true,
                },
            )
        }
        Some("done") | Some("complete") | Some("hunted") => {
            close_hunt(app, HuntVerdict::Hunted, GoalStatus::Complete)
        }
        Some("pause") | Some("paused") | Some("wound") | Some("wounded") => {
            close_hunt(app, HuntVerdict::Wounded, GoalStatus::Paused)
        }
        Some("resume") | Some("continue") => resume_hunt(app),
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
            if let Some(ref obj) = app.hunt.quarry {
                let elapsed = app
                    .hunt
                    .time_used_seconds
                    .gt(&0)
                    .then(|| {
                        crate::tui::notifications::humanize_duration(
                            std::time::Duration::from_secs(app.hunt.time_used_seconds),
                        )
                    })
                    .or_else(|| {
                        app.hunt
                            .started_at
                            .map(|t| crate::tui::notifications::humanize_duration(t.elapsed()))
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
                        format!(" | tokens: {used}/{b} ({pct:.0}%)")
                    })
                    .unwrap_or_default();
                let verdict_label = match app.hunt.verdict {
                    HuntVerdict::Hunting => "[ACTIVE]",
                    HuntVerdict::Hunted => "[COMPLETE]",
                    HuntVerdict::Wounded => "[PAUSED]",
                    HuntVerdict::Escaped => "[BLOCKED]",
                };
                CommandResult::message(format!(
                    "Goal {verdict_label}: \"{obj}\" - elapsed: {elapsed}{budget_str} | continuations: {}",
                    app.hunt.continuation_count
                ))
            } else {
                CommandResult::message(goal_usage())
            }
        }
    }
}

fn close_hunt(app: &mut App, verdict: HuntVerdict, status: GoalStatus) -> CommandResult {
    if app.hunt.quarry.as_deref().is_none_or(str::is_empty) {
        return CommandResult::error("No goal set. Use /goal <objective> [budget: N] first.");
    }

    let prev = app.hunt.verdict;
    let should_write_trophy = match verdict {
        HuntVerdict::Hunted => prev != verdict,
        HuntVerdict::Escaped => true,
        HuntVerdict::Wounded | HuntVerdict::Hunting => false,
    };
    if should_write_trophy && let Err(err) = write_trophy_card(app, verdict) {
        return CommandResult::error(err);
    }
    app.hunt.verdict = verdict;

    // Push the new status to the engine's SharedGoalState so the cross-turn
    // continuation loop respects it: pause/blocked stops the loop, complete
    // ends it, resume restarts it.
    let action = AppAction::SetGoalStatus {
        status,
        clear: false,
    };

    match verdict {
        HuntVerdict::Hunted => {
            let elapsed = app
                .hunt
                .started_at
                .map(|t| crate::tui::notifications::humanize_duration(t.elapsed()))
                .unwrap_or_else(|| "unknown".to_string());
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
        HuntVerdict::Hunting => CommandResult::with_message_and_action("Goal resumed.", action),
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
    CommandResult::with_message_and_action("Goal resumed.", AppAction::SendMessage(objective))
}

fn goal_usage() -> &'static str {
    "No goal set. Use /goal <objective> [budget: N] to set one.\n\
     /goal complete - mark complete\n\
     /goal pause - pause without continuing\n\
     /goal resume - resume and continue\n\
     /goal blocked - mark blocked\n\
     /goal clear - remove the current goal."
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
        .map(|t| crate::tui::notifications::humanize_duration(t.elapsed()))
        .unwrap_or_else(|| "unknown".to_string());
    let verdict_str = match verdict {
        HuntVerdict::Hunting => "active",
        HuntVerdict::Hunted => "complete",
        HuntVerdict::Wounded => "paused",
        HuntVerdict::Escaped => "blocked",
    };
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
    writeln!(f, "_Generated by CodeWhale `/goal` - {}_", card.now)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_app() -> App {
        let options = crate::tui::app::TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: std::path::PathBuf::from("/tmp/test-workspace"),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: std::path::PathBuf::from("/tmp/test-skills"),
            memory_path: std::path::PathBuf::from("memory.md"),
            notes_path: std::path::PathBuf::from("notes.txt"),
            mcp_config_path: std::path::PathBuf::from("mcp.json"),
            use_memory: false,
            start_in_agent_mode: false,
            skip_onboarding: true,
            initial_input: None,
            resume_session_id: None,
            yolo: false,
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
    fn test_hunt_without_argument_shows_state() {
        let mut app = create_test_app();
        let result = hunt(&mut app, None);
        assert!(result.action.is_none());
        assert!(result.message.as_deref().unwrap().contains("No goal set"));
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
        let _ = hunt(&mut app, Some("clear"));
        assert!(app.hunt.quarry.is_none());
        assert!(app.hunt.token_budget.is_none());
        assert_eq!(app.hunt.tokens_used, 0);
        assert_eq!(app.hunt.time_used_seconds, 0);
        assert_eq!(app.hunt.continuation_count, 0);
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
    fn test_failed_trophy_write_does_not_mutate_verdict() {
        let mut app = create_test_app();
        app.hunt.quarry = Some("!!!".to_string());
        app.hunt.verdict = HuntVerdict::Hunting;

        let result = hunt(&mut app, Some("escaped"));

        assert!(result.is_error);
        assert_eq!(app.hunt.verdict, HuntVerdict::Hunting);
        assert_eq!(app.hunt.quarry.as_deref(), Some("!!!"));
    }

    #[test]
    fn test_show_hunt_when_none() {
        let mut app = create_test_app();
        let result = hunt(&mut app, None);
        assert!(result.message.unwrap().contains("No goal set"));
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
