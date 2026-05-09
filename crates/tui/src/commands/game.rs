//! Game Console commands: /play and /game.

use std::fmt::Write;
use std::path::PathBuf;

use crate::game::{GameLaunchOptions, GameSession};
use crate::tui::app::App;

use super::CommandResult;

/// Start or switch the active Game Console session.
pub fn play(app: &mut App, arg: Option<&str>) -> CommandResult {
    let launch = match parse_play_args(app, arg) {
        Ok(launch) => launch,
        Err(message) => return CommandResult::error(message),
    };

    let session = match crate::game::load_game_session(&app.workspace, launch) {
        Ok(session) => session,
        Err(err) => return CommandResult::error(format!("failed to load game session: {err}")),
    };
    let status = session.status_label();
    app.install_game_session(session);
    CommandResult::message(status)
}

/// Inspect or control the active Game Console session.
pub fn game(app: &mut App, arg: Option<&str>) -> CommandResult {
    let trimmed = arg.unwrap_or("").trim();
    if trimmed.is_empty() {
        return status(app);
    }

    let mut parts = trimmed.split_whitespace();
    let action = parts.next().unwrap_or_default().to_ascii_lowercase();
    match action.as_str() {
        "status" => status(app),
        "render" => render(app),
        "choices" | "playbook" => choices(app),
        "saves" => saves(app),
        "dev" => dev(app, parts.next()),
        "exit" | "close" => exit(app),
        _ => CommandResult::error(
            "Usage: /game [status|render|choices|saves|dev [on|off]|exit]".to_string(),
        ),
    }
}

fn parse_play_args(app: &App, arg: Option<&str>) -> Result<GameLaunchOptions, String> {
    let mut game_or_path = None;
    let mut save = None;
    let mut developer_mode = app
        .game_session
        .as_ref()
        .is_some_and(GameSession::developer_mode);

    let tokens: Vec<&str> = arg
        .unwrap_or("")
        .split_whitespace()
        .filter(|token| !token.trim().is_empty())
        .collect();
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--dev" | "-d" => {
                developer_mode = true;
                index += 1;
            }
            "--save" | "-s" => {
                let Some(value) = tokens.get(index + 1) else {
                    return Err("Usage: /play [game-or-path] [--save <id>] [--dev]".to_string());
                };
                save = Some((*value).to_string());
                index += 2;
            }
            token if token.starts_with('-') => {
                return Err(format!("unknown /play flag: {token}"));
            }
            token => {
                if game_or_path.is_some() {
                    return Err("Usage: /play [game-or-path] [--save <id>] [--dev]".to_string());
                }
                game_or_path = Some(PathBuf::from(token));
                index += 1;
            }
        }
    }

    Ok(GameLaunchOptions {
        game_or_path,
        save,
        developer_mode,
    })
}

fn status(app: &mut App) -> CommandResult {
    match app.game_session.as_ref() {
        Some(session) => CommandResult::message(session.status_report()),
        None => CommandResult::message("No active Game Console session. Use /play.".to_string()),
    }
}

fn render(app: &mut App) -> CommandResult {
    if app.render_game_session().is_some() {
        CommandResult::message("Rendered the active Game Console session.".to_string())
    } else {
        CommandResult::message("No active Game Console session. Use /play.".to_string())
    }
}

fn choices(app: &mut App) -> CommandResult {
    let Some(session) = app.game_session.as_ref() else {
        return CommandResult::message("No active Game Console session. Use /play.".to_string());
    };
    match session.choices_report() {
        Ok(report) => CommandResult::message(report),
        Err(err) => CommandResult::error(format!("failed to read game choices: {err}")),
    }
}

fn saves(app: &mut App) -> CommandResult {
    let Some(GameSession::Loaded(session)) = app.game_session.as_ref() else {
        return CommandResult::message("No loaded game saves. Use /play first.".to_string());
    };
    let entries = match std::fs::read_dir(&session.saves_root) {
        Ok(entries) => entries,
        Err(err) => {
            return CommandResult::error(format!(
                "failed to read saves from {}: {err}",
                session.saves_root.display()
            ));
        }
    };

    let mut saves = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            path.is_dir()
                .then(|| entry.file_name().to_string_lossy().to_string())
        })
        .collect::<Vec<_>>();
    saves.sort();

    if saves.is_empty() {
        return CommandResult::message(format!(
            "No saves found in {}",
            session.saves_root.display()
        ));
    }

    let mut message = format!("Game saves in {}:", session.saves_root.display());
    for save in saves {
        let marker = if save == session.save_id { " *" } else { "" };
        let _ = write!(message, "\n- {save}{marker}");
    }
    CommandResult::message(message)
}

fn dev(app: &mut App, arg: Option<&str>) -> CommandResult {
    let Some(current) = app
        .game_session
        .as_ref()
        .map(crate::game::GameSession::developer_mode)
    else {
        return CommandResult::message("No active Game Console session. Use /play.".to_string());
    };

    let enabled = match arg.map(str::trim).filter(|value| !value.is_empty()) {
        None => !current,
        Some(value) if value.eq_ignore_ascii_case("on") || value.eq_ignore_ascii_case("true") => {
            true
        }
        Some(value) if value.eq_ignore_ascii_case("off") || value.eq_ignore_ascii_case("false") => {
            false
        }
        Some(_) => return CommandResult::error("Usage: /game dev [on|off]"),
    };

    let message = app
        .set_game_developer_mode(enabled)
        .unwrap_or_else(|| "No active Game Console session. Use /play.".to_string());
    CommandResult::message(message)
}

fn exit(app: &mut App) -> CommandResult {
    if app.game_session.is_none() {
        return CommandResult::message("No active Game Console session.".to_string());
    }
    app.clear_game_session();
    CommandResult::message("Game Console session closed.".to_string())
}
