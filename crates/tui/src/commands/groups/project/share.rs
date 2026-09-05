//! /share command — export the current session as a shareable web URL.
//!
//! Renders the current session transcript as a static HTML page, uploads it
//! to a GitHub Gist via the `gh` CLI, and displays the resulting URL.
//!
//! # Usage
//!
//! - `/share` — export the current session and print the Gist URL
//! - `/share help` — show usage

use std::io::Write;
use std::path::Path;

use codewhale_command_contract::facets::CommandProjectContext;
use codewhale_command_contract::handler::{CommandContexts, CommandHandler};
use codewhale_command_contract::metadata::{CommandInfo, RegisterCommand};

use crate::commands::CommandResult;
use crate::dependencies::ExternalTool;
use crate::tui::app::AppAction;

/// Share the current session as a web URL.
fn share(project: &dyn CommandProjectContext, arg: Option<&str>) -> CommandResult {
    let raw = arg.map(str::trim).unwrap_or("");

    match raw {
        "" => do_share(project),
        "help" | "--help" | "-h" => CommandResult::message(
            "/share — Export the current session as a shareable web URL.\n\
             \n\
             Usage:\n\
             /share         Export and upload the current session\n\
             \n\
             The session transcript is rendered as static HTML and uploaded\n\
             to a GitHub Gist using the `gh` CLI. The Gist URL is displayed\n\
             so you can paste it into Slack, GitHub, Twitter, etc."
                .to_string(),
        ),
        _ => CommandResult::error(format!(
            "Unknown /share argument `{raw}`. Use `/share` with no arguments or `/share help`."
        )),
    }
}

/// Export the session as HTML, upload to a Gist, and show the URL.
fn do_share(project: &dyn CommandProjectContext) -> CommandResult {
    let share = project.share_projection();

    // Check if there's any session content to share
    if share.history_is_empty {
        return CommandResult::error("Nothing to share. The current session is empty.");
    }

    // Use an AppAction to signal the engine to perform the async work.
    CommandResult::with_message_and_action(
        format!(
            "Exporting {} cell(s) from {} ({}) session...\n\n\
             The session will be rendered as static HTML and uploaded to a GitHub Gist.\n\
             This requires the `gh` CLI to be installed and authenticated.",
            share.history_len, share.model, share.mode_label
        ),
        AppAction::ShareSession {
            history_len: share.history_len,
            model: share.model,
            mode: share.mode_label,
        },
    )
}

/// Actually perform the share export.
///
/// This is called from the engine after receiving the `ShareSession` action.
/// It renders the session as HTML and uploads it via `gh gist create`.
pub async fn perform_share(history_json: &str, model: &str, mode: &str) -> Result<String, String> {
    // Build HTML from the session data
    let html = render_session_html(history_json, model, mode);

    // Write to a temp file
    let tmp = match write_temp_html(&html) {
        Ok(file) => file,
        Err(e) => return Err(format!("Failed to write temp file: {e}")),
    };

    // Upload via `gh gist create`
    let url = match upload_gist(tmp.path()).await {
        Ok(url) => url,
        Err(e) => return Err(format!("Failed to upload Gist: {e}")),
    };

    Ok(url)
}

/// Render the session as a standalone HTML page.
fn render_session_html(history_json: &str, model: &str, mode: &str) -> String {
    let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
    let escaped_model = html_escape(model);
    let escaped_mode = html_escape(mode);
    let escaped_body = html_escape(history_json);

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>codewhale Session Export</title>
<style>
  body {{
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
    max-width: 800px; margin: 2rem auto; padding: 0 1rem;
    background: #0d1117; color: #c9d1d9;
  }}
  h1 {{ color: #58a6ff; border-bottom: 1px solid #30363d; padding-bottom: 0.5rem; }}
  .meta {{ color: #8b949e; font-size: 0.9rem; margin-bottom: 2rem; }}
  .message {{ margin: 1rem 0; padding: 0.75rem; border-radius: 6px; }}
  .user {{ background: #1f2937; border-left: 3px solid #58a6ff; }}
  .assistant {{ background: #161b22; border-left: 3px solid #3fb950; }}
  .tool {{ background: #0d1117; border: 1px solid #30363d; font-family: monospace; font-size: 0.85rem; }}
  pre {{ white-space: pre-wrap; word-wrap: break-word; margin: 0; }}
  .footer {{ margin-top: 2rem; padding-top: 1rem; border-top: 1px solid #30363d; color: #8b949e; font-size: 0.8rem; }}
</style>
</head>
<body>
<h1>codewhale Session</h1>
<div class="meta">
  <strong>Model:</strong> {escaped_model} · <strong>Mode:</strong> {escaped_mode}<br>
  <strong>Exported:</strong> {timestamp}
</div>
<pre>{escaped_body}</pre>
<div class="footer">
  Generated by codewhale · https://github.com/Hmbown/CodeWhale
</div>
</body>
</html>"#,
    )
}

/// HTML-escape special characters.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Write HTML to a secure temp file and keep it alive for upload.
fn write_temp_html(html: &str) -> Result<tempfile::NamedTempFile, String> {
    let mut tmp = tempfile::Builder::new()
        .prefix("codewhale-share-")
        .suffix(".html")
        .tempfile()
        .map_err(|e| format!("{e}"))?;
    tmp.write_all(html.as_bytes()).map_err(|e| format!("{e}"))?;
    Ok(tmp)
}

/// Upload a file as a GitHub Gist using the `gh` CLI.
async fn upload_gist(path: &Path) -> Result<String, String> {
    let path_owned = path.to_path_buf();
    let output = tokio::task::spawn_blocking(move || {
        let mut cmd = crate::dependencies::Gh::command()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "gh not found"))?;
        cmd.args([
            "gist",
            "create",
            "--public",
            path_owned.to_string_lossy().as_ref(),
            "--filename",
            "session-export.html",
            "--desc",
            "codewhale Session Export",
        ])
        .output()
    })
    .await
    .map_err(|join_err| format!("gh gist create panicked: {join_err}"))?
    .map_err(|e| format!("Failed to run `gh gist create`: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("`gh gist create` failed: {stderr}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        return Err("`gh gist create` returned no output".to_string());
    }

    Ok(stdout)
}

pub(in crate::commands) const SHARE_INFO: CommandInfo = CommandInfo {
    name: "share",
    aliases: &[],
    usage: "/share",
    description_key: "cmd_share_description",
};

pub(in crate::commands) struct ShareCmd;

impl RegisterCommand<CommandResult> for ShareCmd {
    fn info() -> &'static CommandInfo {
        &SHARE_INFO
    }

    fn handler() -> CommandHandler<CommandResult> {
        CommandHandler::Contextual {
            capabilities: codewhale_command_contract::handler::CommandCapabilities::PROJECT,
            handler: share_contextual,
        }
    }
}

/// Contextual `/share` dispatch (FEAT-021 Phase 4).
///
/// Destructures the declared `PROJECT` facet with a safe missing-facet error.
fn share_contextual(contexts: CommandContexts<'_>, arg: Option<&str>) -> CommandResult {
    let mut parts = contexts.into_parts();
    let Some(project) = parts.project.as_deref_mut() else {
        return CommandResult::error("Command capability unavailable: project");
    };
    share(project, arg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codewhale_command_contract::facets::{
        ProjectGoalState, ProjectGoalStatus, ProjectShareProjection,
    };

    /// Deterministic fake project facet over portable values only.
    struct FakeProject {
        share: ProjectShareProjection,
    }

    impl CommandProjectContext for FakeProject {
        fn lsp_enabled(&self) -> bool {
            false
        }

        fn lsp_set(&mut self, _enabled: bool) -> Result<(), String> {
            Ok(())
        }

        fn share_projection(&self) -> ProjectShareProjection {
            self.share.clone()
        }

        fn goal_state(&self) -> ProjectGoalState {
            ProjectGoalState {
                objective: None,
                status: ProjectGoalStatus::Active,
                pause_reason: None,
                started_at_elapsed_seconds: None,
                time_used_seconds: 0,
                token_budget: None,
                tokens_used: 0,
                session_total_tokens: 0,
                continuation_count: 0,
                pending_controls: false,
                last_known_objective: None,
                last_known_status: None,
                conversation_present: false,
                is_loading: false,
                goal_continuation_waiting: false,
            }
        }
    }

    fn project_with_history() -> FakeProject {
        FakeProject {
            share: ProjectShareProjection {
                history_is_empty: false,
                history_len: 3,
                model: "deepseek-v4-pro".to_string(),
                mode_label: "ACT".to_string(),
            },
        }
    }

    fn project_empty() -> FakeProject {
        FakeProject {
            share: ProjectShareProjection {
                history_is_empty: true,
                history_len: 0,
                model: String::new(),
                mode_label: String::new(),
            },
        }
    }

    #[test]
    fn share_empty_session_errors() {
        let project = project_empty();
        let result = share(&project, Some(""));
        assert!(result.is_error);
        assert!(
            result.message.unwrap().contains("Nothing to share"),
            "empty share must error"
        );
    }

    #[test]
    fn share_populated_session_emits_exact_action_and_message() {
        let project = project_with_history();
        let result = share(&project, Some(""));
        assert!(!result.is_error);
        let msg = result.message.unwrap();
        assert!(
            msg.contains("Exporting 3 cell(s) from deepseek-v4-pro (ACT) session..."),
            "message was: {msg}"
        );
        assert!(
            matches!(
                result.action,
                Some(AppAction::ShareSession {
                    history_len: 3,
                    ref model,
                    ref mode,
                }) if model == "deepseek-v4-pro" && mode == "ACT"
            ),
            "action was: {:?}",
            result.action
        );
    }

    #[test]
    fn share_help_and_unknown_routes() {
        let project = project_with_history();
        for arg in ["help", "--help", "-h"] {
            let result = share(&project, Some(arg));
            assert!(!result.is_error);
            assert!(result.message.unwrap().contains("/share"));
        }
        let result = share(&project, Some("bogus"));
        assert!(result.is_error);
        assert!(
            result
                .message
                .unwrap()
                .contains("Unknown /share argument `bogus`")
        );
    }

    #[test]
    fn missing_project_facet_fails_safely() {
        let result = share_contextual(CommandContexts::empty(), Some(""));
        assert!(result.is_error);
        assert!(
            result
                .message
                .unwrap()
                .contains("Command capability unavailable: project")
        );
    }

    #[test]
    fn test_render_session_html_basic_structure() {
        let html = render_session_html("[{}]", "deepseek-v4-pro", "agent");
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("deepseek-v4-pro"));
        assert!(html.contains("agent"));
        assert!(html.contains("[{}]"));
        assert!(html.contains("codewhale"));
    }

    #[test]
    fn test_html_escape_handles_special_chars() {
        assert_eq!(html_escape("<script>"), "&lt;script&gt;");
        assert_eq!(html_escape("a&b"), "a&amp;b");
        assert_eq!(html_escape("\"quote\""), "&quot;quote&quot;");
    }

    #[test]
    fn test_write_temp_html_creates_file() {
        let file = write_temp_html("<html></html>").unwrap();
        assert!(file.path().exists());
        let content = std::fs::read_to_string(file.path()).unwrap();
        assert_eq!(content, "<html></html>");
    }

    #[test]
    fn test_render_session_html_metadata() {
        let html = render_session_html("test data", "deepseek-v4-flash", "plan");
        assert!(html.contains("deepseek-v4-flash"));
        assert!(html.contains("plan"));
        assert!(html.contains("test data"));
        assert!(html.contains("Exported:"));
        assert!(html.contains("https://github.com/Hmbown/CodeWhale"));
    }
}
