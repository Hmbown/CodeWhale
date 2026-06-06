use crate::commands::CommandResult;
use crate::tui::app::{App, AppAction};

pub(crate) fn rlm(app: &mut App, arg: Option<&str>) -> CommandResult {
    let (max_depth, target) = match parse_depth_prefixed_arg(arg, 1) {
        Ok(parsed) => parsed,
        Err(message) => return CommandResult::error(message),
    };
    let target = match target {
        Some(path) if !path.trim().is_empty() => path.trim().to_string(),
        _ => {
            return CommandResult::error(
                "Usage: /rlm [N] <file_or_text>\n\n\
                 Opens a persistent RLM context with sub_rlm depth N (0-3, default 1).",
            );
        }
    };
    let source_arg = if resolves_to_existing_file(app, &target) {
        format!("file_path: \"{target}\"")
    } else {
        format!("content: {target:?}")
    };
    let message = format!(
        "Open and use a persistent RLM session. Call `rlm_open` with name `slash_rlm` and {source_arg}. Call `rlm_configure` with `sub_rlm_max_depth: {max_depth}`."
    );
    CommandResult::with_message_and_action(
        format!("Opening persistent RLM context at depth {max_depth}..."),
        AppAction::SendMessage(message),
    )
}

fn parse_depth_prefixed_arg(
    arg: Option<&str>,
    default_depth: u32,
) -> Result<(u32, Option<&str>), String> {
    let Some(raw) = arg.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok((default_depth, None));
    };
    let mut parts = raw.splitn(2, char::is_whitespace);
    let first = parts.next().unwrap_or_default();
    if first.chars().all(|ch| ch.is_ascii_digit()) {
        let depth: u32 = first
            .parse()
            .map_err(|_| "Depth must be an integer from 0 to 3".to_string())?;
        if depth > 3 {
            return Err("Depth must be between 0 and 3".to_string());
        }
        Ok((depth, parts.next().map(str::trim)))
    } else {
        Ok((default_depth, Some(raw)))
    }
}

fn resolves_to_existing_file(app: &App, input: &str) -> bool {
    let path = std::path::Path::new(input);
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        app.workspace.join(path)
    };
    candidate.is_file()
}
