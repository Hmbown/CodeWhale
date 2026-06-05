//! /hunt command — declare a quarry with token budget and verdict tracking (#2092).

use std::io::Write;

use crate::tui::app::{App, AppAction, HuntVerdict};

use crate::commands::CommandResult;

/// Declare, show, or close a hunt
pub fn hunt(app: &mut App, arg: Option<&str>) -> CommandResult {
    match arg {
        Some("clear") | Some("reset") => {
            app.hunt.quarry = None;
            app.hunt.token_budget = None;
            app.hunt.started_at = None;
            app.hunt.verdict = HuntVerdict::default();
            CommandResult::message("Hunt cleared.")
        }
        Some("done") | Some("complete") | Some("hunted") => close_hunt(app, HuntVerdict::Hunted),
        Some("wound") | Some("wounded") => close_hunt(app, HuntVerdict::Wounded),
        Some("escape") | Some("escaped") => close_hunt(app, HuntVerdict::Escaped),
        Some(text) if !text.is_empty() => {
            let (objective, budget) = parse_hunt_budget(text);
            if objective.is_empty() || objective.chars().all(|c| c == '|') {
                return CommandResult::error("Usage: /hunt <quarry> [budget: N]");
            }
            app.hunt.quarry = Some(objective.clone());
            app.hunt.token_budget = budget;
            app.hunt.started_at = Some(std::time::Instant::now());
            app.hunt.verdict = HuntVerdict::Hunting;
            let budget_str = budget
                .map(|b| format!(" (budget: {b} tokens)"))
                .unwrap_or_default();
            CommandResult::with_message_and_action(
                format!("Hunt set: \"{objective}\"{budget_str} — tracking progress."),
                AppAction::SendMessage(objective),
            )
        }
        _ => {
            if let Some(ref obj) = app.hunt.quarry {
                let elapsed = app
                    .hunt
                    .started_at
                    .map(|t| crate::tui::notifications::humanize_duration(t.elapsed()))
                    .unwrap_or_else(|| "unknown".to_string());
                let budget_str = app
                    .hunt
                    .token_budget
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
                let verdict_label = match app.hunt.verdict {
                    HuntVerdict::Hunting => "[HUNTING]",
                    HuntVerdict::Hunted => "[HUNTED]",
                    HuntVerdict::Wounded => "[WOUNDED]",
                    HuntVerdict::Escaped => "[ESCAPED]",
                };
                CommandResult::message(format!(
                    "Hunt {verdict_label}: \"{obj}\" — elapsed: {elapsed}{budget_str}"
                ))
            } else {
                CommandResult::message(
                    "No hunt set. Use /hunt <quarry> [budget: N] to declare one.\n\
                     /hunt hunted — mark complete\n\
                     /hunt wounded — mark interrupted (resumable)\n\
                     /hunt escaped — mark abandoned\n\
                     /hunt clear — remove the current hunt.",
                )
            }
        }
    }
}

fn close_hunt(app: &mut App, verdict: HuntVerdict) -> CommandResult {
    if app.hunt.quarry.as_deref().is_none_or(str::is_empty) {
        return CommandResult::error("No hunt set. Use /hunt <quarry> [budget: N] first.");
    }

    let prev = app.hunt.verdict;
    let should_write_trophy = prev != verdict || !matches!(verdict, HuntVerdict::Hunted);
    if should_write_trophy && let Err(err) = write_trophy_card(app, verdict) {
        return CommandResult::error(err);
    }
    app.hunt.verdict = verdict;

    match verdict {
        HuntVerdict::Hunted => {
            let elapsed = app
                .hunt
                .started_at
                .map(|t| crate::tui::notifications::humanize_duration(t.elapsed()))
                .unwrap_or_else(|| "unknown".to_string());
            CommandResult::message(format!("Hunt complete! Elapsed: {elapsed}"))
        }
        HuntVerdict::Wounded => {
            CommandResult::message("Hunt wounded — progress saved, can be resumed.")
        }
        HuntVerdict::Escaped => CommandResult::message("Hunt escaped — quarry abandoned."),
        HuntVerdict::Hunting => CommandResult::message("Hunt resumed."),
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

/// Write a trophy card to `~/.codewhale/trophies/<date>-<time>-<slug>.md`
/// for the current hunt verdict (#2092).
fn write_trophy_card(app: &App, verdict: HuntVerdict) -> Result<std::path::PathBuf, String> {
    let quarry = app
        .hunt
        .quarry
        .as_deref()
        .ok_or_else(|| "No hunt set. Use /hunt <quarry> [budget: N] first.".to_string())?;
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
            "Cannot write trophy card: hunt quarry has no filename-safe characters.".into(),
        );
    }
    let now = chrono::Local::now();
    let time = now.format("%H%M%S");
    let date = now.format("%Y-%m-%d");
    let date_str = date.to_string();
    let now_str = now.to_string();
    let dir = codewhale_config::resolve_state_dir("trophies")
        .map_err(|err| format!("Could not resolve trophy directory: {err}"))?;
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("Could not create trophy directory {}: {err}", dir.display()))?;
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
        HuntVerdict::Hunting => "hunting",
        HuntVerdict::Hunted => "hunted",
        HuntVerdict::Wounded => "wounded",
        HuntVerdict::Escaped => "escaped",
    };
    let tokens = app.session.total_conversation_tokens;
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
    writeln!(f, "# Trophy: {}", card.quarry)?;
    writeln!(f)?;
    writeln!(f, "- **Verdict**: {}", card.verdict)?;
    writeln!(f, "- **Date**: {}", card.date)?;
    writeln!(f, "- **Elapsed**: {}", card.elapsed)?;
    writeln!(f, "- **Tokens used**: {}", card.tokens)?;
    writeln!(f, "- **Token budget**: {}", card.budget)?;
    writeln!(f)?;
    writeln!(f, "_Generated by CodeWhale `/hunt` — {}_", card.now)?;
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
        assert!(result.message.unwrap().contains("Hunt set"));
        assert_eq!(app.hunt.quarry.as_deref(), Some("Fix the login bug"));
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
        assert!(result.message.as_deref().unwrap().contains("No hunt set"));
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
                .contains("Usage: /hunt")
        );
        assert_eq!(app.hunt.quarry.as_deref(), Some("existing objective"));
        assert_eq!(app.hunt.token_budget, Some(10_000));
    }

    #[test]
    fn test_clear_hunt() {
        let mut app = create_test_app();
        app.hunt.quarry = Some("test".to_string());
        let _ = hunt(&mut app, Some("clear"));
        assert!(app.hunt.quarry.is_none());
        assert!(app.hunt.token_budget.is_none());
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
        assert!(result.message.unwrap().contains("No hunt set"));
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