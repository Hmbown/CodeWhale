//! /goal command — set a session objective with token budget and progress tracking.

use crate::tui::app::App;

use super::CommandResult;

/// Set or show the current goal
pub fn goal(app: &mut App, arg: Option<&str>) -> CommandResult {
    match arg {
        Some("clear") | Some("reset") | Some("done") => {
            app.goal.goal_objective = None;
            app.goal.goal_token_budget = None;
            app.goal.goal_started_at = None;
            CommandResult::message("Goal cleared.")
        }
        Some(text) if !text.is_empty() => {
            // Parse optional budget: "/goal Implement login | budget: 50000"
            let (objective, budget) = parse_goal_budget(text);
            app.goal.goal_objective = Some(objective.clone());
            app.goal.goal_token_budget = budget;
            app.goal.goal_started_at = Some(std::time::Instant::now());
            let budget_str = budget
                .map(|b| format!(" (budget: {b} tokens)"))
                .unwrap_or_default();
            CommandResult::message(format!(
                "Goal set: \"{objective}\"{budget_str} — tracking progress."
            ))
        }
        _ => {
            // Show current goal
            if let Some(ref obj) = app.goal.goal_objective {
                // #447: render long elapsed times as `2d 3h` rather
                // than Rust's default Debug `Duration` (which produces
                // `188415.234s` or similar for multi-day goals).
                let elapsed = app
                    .goal
                    .goal_started_at
                    .map(|t| crate::tui::notifications::humanize_duration(t.elapsed()))
                    .unwrap_or_else(|| "unknown".to_string());
                let budget_str = app
                    .goal
                    .goal_token_budget
                    .map(|b| {
                        let used = app.session.total_conversation_tokens;
                        let pct = if b > 0 {
                            (used as f64 / b as f64 * 100.0).min(100.0)
                        } else {
                            0.0
                        };
                        format!(" | tokens: {used}/{b} ({pct:.0}%)")
                    })
                    .unwrap_or_default();
                CommandResult::message(format!("Goal: \"{obj}\" — elapsed: {elapsed}{budget_str}"))
            } else {
                CommandResult::message(
                    "No goal set. Use /goal <objective> [budget: N] to set one.\n\
                     /goal clear — remove the current goal.",
                )
            }
        }
    }
}

/// Parse optional token budget from goal text: "Implement login | budget: 50000"
fn parse_goal_budget(text: &str) -> (String, Option<u32>) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::{App, TuiOptions};
    use std::path::PathBuf;

    fn create_test_app() -> App {
        let options = TuiOptions {
            model: "deepseek-v4-flash".to_string(),
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
            start_in_agent_mode: true,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        App::new(options, &Config::default())
    }

    #[test]
    fn test_set_goal() {
        let mut app = create_test_app();
        let result = goal(&mut app, Some("Fix the login bug"));
        assert!(result.message.unwrap().contains("Goal set"));
        assert_eq!(
            app.goal.goal_objective.as_deref(),
            Some("Fix the login bug")
        );
    }

    #[test]
    fn test_set_goal_with_budget() {
        let mut app = create_test_app();
        let _ = goal(&mut app, Some("Refactor auth | budget: 50000"));
        assert_eq!(app.goal.goal_objective.as_deref(), Some("Refactor auth"));
        assert_eq!(app.goal.goal_token_budget, Some(50_000));
    }

    #[test]
    fn test_clear_goal() {
        let mut app = create_test_app();
        app.goal.goal_objective = Some("test".to_string());
        let _ = goal(&mut app, Some("clear"));
        assert!(app.goal.goal_objective.is_none());
        assert!(app.goal.goal_token_budget.is_none());
    }

    #[test]
    fn test_show_goal_when_none() {
        let mut app = create_test_app();
        let result = goal(&mut app, None);
        assert!(result.message.unwrap().contains("No goal set"));
    }

    #[test]
    fn test_parse_budget() {
        assert_eq!(
            parse_goal_budget("Do a thing | budget: 50000"),
            ("Do a thing".to_string(), Some(50_000))
        );
        assert_eq!(
            parse_goal_budget("Simple goal"),
            ("Simple goal".to_string(), None)
        );
        assert_eq!(
            parse_goal_budget("Goal budget:1000"),
            ("Goal".to_string(), Some(1000))
        );
    }
}
