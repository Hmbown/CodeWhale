//! Core command area: model/provider selection, help, navigation, and the
//! persistent RLM / sub-agent entry points.

mod anchor;
#[allow(clippy::module_inception)]
mod core;
mod feedback;
mod hf;
mod hooks;
mod provider;
mod queue;
mod stash;

pub(in crate::commands) use self::core::reset_conversation_state;

use crate::commands::CommandResult;
use crate::tui::app::{App, AppAction};

pub(in crate::commands) fn dispatch(
    app: &mut App,
    command: &str,
    arg: Option<&str>,
) -> Option<CommandResult> {
    let result = match command {
        "anchor" | "maodian" => anchor::anchor(app, arg),
        "help" | "?" | "bangzhu" | "帮助" => core::help(app, arg),
        "clear" | "qingping" => core::clear(app),
        "exit" | "quit" | "q" | "tuichu" => core::exit(),
        "model" | "moxing" => core::model(app, arg),
        "models" | "moxingliebiao" => core::models(app),
        "provider" => provider::provider(app, arg),
        "queue" | "queued" => queue::queue(app, arg),
        "stash" | "park" => stash::stash(app, arg),
        "hooks" | "hook" | "gouzi" => hooks::hooks(app, arg),
        "subagents" | "agents" | "zhinengti" => core::subagents(app),
        "agent" | "daili" => agent(app, arg),
        "links" | "dashboard" | "api" | "lianjie" => core::deepseek_links(app),
        "feedback" => feedback::feedback(app, arg),
        "hf" | "huggingface" => hf::hf(app, arg),
        "home" | "stats" | "overview" | "zhuye" | "shouye" => core::home_dashboard(app),
        "workspace" | "cwd" => core::workspace_switch(app, arg),
        "profile" | "dangan" => core::profile_switch(app, arg),
        "rlm" | "recursive" | "digui" => rlm(app, arg),
        "translate" | "translation" | "transale" => core::translate(app),
        _ => return None,
    };
    Some(result)
}

/// Execute a Recursive Language Model (RLM) turn — Algorithm 1 from
/// Zhang et al. (arXiv:2512.24601).
///
/// The user's prompt text is passed as the argument. It will be stored
/// in the REPL as the `PROMPT` variable. The root LLM will only see
/// metadata about the REPL state, never the prompt text directly.
pub fn rlm(app: &mut App, arg: Option<&str>) -> CommandResult {
    let (max_depth, target) = match parse_depth_prefixed_arg(arg, 1) {
        Ok(parsed) => parsed,
        Err(message) => return CommandResult::error(message),
    };
    let target = match target {
        Some(p) if !p.trim().is_empty() => p.trim().to_string(),
        _ => {
            return CommandResult::error(
                "Usage: /rlm [N] <file_or_text>\n\n\
                 Opens a persistent RLM context with sub_rlm depth N (0-3, default 1)."
                    .to_string(),
            );
        }
    };

    let source_arg = if resolves_to_existing_file(app, &target) {
        format!(r#"file_path: "{target}""#)
    } else {
        format!("content: {target:?}")
    };
    let message = format!(
        "Open and use a persistent RLM session for this request. Call `rlm_open` with name `slash_rlm` and {source_arg}. Then call `rlm_configure` with `sub_rlm_max_depth: {max_depth}`. Use `rlm_eval` to inspect the context through `peek`, `search`, and `chunk`, and call `finalize(...)` from the REPL when ready. If a `var_handle` is returned, use `handle_read` for bounded slices or projections before answering."
    );

    CommandResult::with_message_and_action(
        format!("Opening persistent RLM context at depth {max_depth}..."),
        AppAction::SendMessage(message),
    )
}

/// Open a persistent sub-agent session from a slash command.
pub fn agent(_app: &mut App, arg: Option<&str>) -> CommandResult {
    let (max_depth, task) = match parse_depth_prefixed_arg(arg, 1) {
        Ok(parsed) => parsed,
        Err(message) => return CommandResult::error(message),
    };
    let task = match task {
        Some(task) if !task.trim().is_empty() => task.trim().to_string(),
        _ => {
            return CommandResult::error(
                "Usage: /agent [N] <task>\n\n\
                 Opens a persistent sub-agent session with recursive agent depth N (0-3, default 1).",
            );
        }
    };
    let message = format!(
        "Open a persistent sub-agent session for this task. Call `agent_open` with name `slash_agent`, `prompt: {task:?}`, and `max_depth: {max_depth}`. Use `agent_eval` to wait for the next terminal/current projection and `handle_read` on the returned transcript_handle if you need more detail. Verify any claimed side effects before reporting success."
    );
    CommandResult::with_message_and_action(
        format!("Opening persistent sub-agent at depth {max_depth}..."),
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
