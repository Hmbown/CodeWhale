//! `/export` command.
//!
//! The full-conversation export is a projection of the authoritative API
//! message stream. It deliberately omits hidden reasoning and signed-thinking
//! payloads, redacts secret-shaped values, and never mutates session or Work
//! state.

use std::fmt::Write as FmtWrite;
use std::fs::{self, OpenOptions};
use std::io::Write as IoWrite;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::models::{ContentBlock, Message, Role};
use crate::tui::app::App;
use crate::tui::history::HistoryCell;

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "export",
    aliases: &["daochu"],
    usage: "/export [clipboard|file [--force] <path>|turn [clipboard|file [--force] <path>]]",
    description_id: MessageId::CmdExportDescription,
};

pub(in crate::commands) struct ExportCmd;

impl RegisterCommand for ExportCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        execute_export(app, arg)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExportScope {
    Conversation,
    Turn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ExportDestination {
    Clipboard,
    File { path: String, force: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExportRequest {
    scope: ExportScope,
    destination: ExportDestination,
}

fn execute_export(app: &mut App, arg: Option<&str>) -> CommandResult {
    let request = match parse_request(arg) {
        Ok(request) => request,
        Err(err) => return CommandResult::error(err),
    };
    let label = match request.scope {
        ExportScope::Conversation => "Conversation",
        ExportScope::Turn => "Turn handoff",
    };
    let markdown = match request.scope {
        ExportScope::Conversation => render_conversation(app),
        ExportScope::Turn => {
            let rendered = crate::tui::ui::turn_handoff_markdown(app);
            sanitize_turn_handoff(app, &rendered)
        }
    };

    match request.destination {
        ExportDestination::Clipboard => copy_to_clipboard(app, label, &markdown),
        ExportDestination::File { path, force } => {
            let path = match resolve_export_path(&app.workspace, &path) {
                Ok(path) => path,
                Err(err) => return CommandResult::error(err),
            };
            match write_export_file(&path, markdown.as_bytes(), force) {
                Ok(()) => CommandResult::message(format!(
                    "{label} exported to {}{}",
                    path.display(),
                    if force {
                        " (overwrite explicitly allowed)"
                    } else {
                        ""
                    }
                )),
                Err(err) => CommandResult::error(format!(
                    "Failed to export {label} to {}: {err}",
                    path.display()
                )),
            }
        }
    }
}

fn parse_request(arg: Option<&str>) -> Result<ExportRequest, String> {
    let raw = arg.unwrap_or("").trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case("clipboard") {
        return Ok(ExportRequest {
            scope: ExportScope::Conversation,
            destination: ExportDestination::Clipboard,
        });
    }

    if raw.eq_ignore_ascii_case("turn") {
        return Ok(ExportRequest {
            scope: ExportScope::Turn,
            destination: ExportDestination::Clipboard,
        });
    }

    if let Some(rest) = strip_word(raw, "turn") {
        let rest = rest.trim();
        if rest.is_empty() || rest.eq_ignore_ascii_case("clipboard") {
            return Ok(ExportRequest {
                scope: ExportScope::Turn,
                destination: ExportDestination::Clipboard,
            });
        }
        let destination = if let Some(file_args) = strip_word(rest, "file") {
            parse_file_destination(file_args)?
        } else if rest.eq_ignore_ascii_case("file") {
            return Err(export_usage("missing file path"));
        } else if strip_word(rest, "clipboard").is_some() {
            return Err(export_usage("clipboard does not accept a path"));
        } else {
            // Backward compatibility: `/export turn <path>`.
            ExportDestination::File {
                path: rest.to_string(),
                force: false,
            }
        };
        return Ok(ExportRequest {
            scope: ExportScope::Turn,
            destination,
        });
    }

    if let Some(file_args) = strip_word(raw, "file") {
        return Ok(ExportRequest {
            scope: ExportScope::Conversation,
            destination: parse_file_destination(file_args)?,
        });
    }
    if raw.eq_ignore_ascii_case("file") {
        return Err(export_usage("missing file path"));
    }
    if strip_word(raw, "clipboard").is_some() {
        return Err(export_usage("clipboard does not accept a path"));
    }

    // Backward compatibility: `/export <path>`.
    Ok(ExportRequest {
        scope: ExportScope::Conversation,
        destination: ExportDestination::File {
            path: raw.to_string(),
            force: false,
        },
    })
}

fn parse_file_destination(raw: &str) -> Result<ExportDestination, String> {
    let trimmed = raw.trim();
    let (force, path) = if let Some(path) = strip_word(trimmed, "--force") {
        (true, path.trim())
    } else if trimmed.eq_ignore_ascii_case("--force") {
        (true, "")
    } else {
        (false, trimmed)
    };
    if path.is_empty() {
        return Err(export_usage("missing file path"));
    }
    Ok(ExportDestination::File {
        path: path.to_string(),
        force,
    })
}

fn strip_word<'a>(value: &'a str, word: &str) -> Option<&'a str> {
    let prefix = value.get(..word.len())?;
    if !prefix.eq_ignore_ascii_case(word) {
        return None;
    }
    let rest = value.get(word.len()..)?;
    rest.chars()
        .next()
        .is_some_and(char::is_whitespace)
        .then_some(rest)
}

fn export_usage(reason: &str) -> String {
    format!(
        "{reason}. Usage: /export [clipboard|file [--force] <path>|turn [clipboard|file [--force] <path>]]"
    )
}

fn copy_to_clipboard(app: &mut App, label: &str, markdown: &str) -> CommandResult {
    let terminal_client = app.clipboard.requires_terminal_paste();
    let last_copy = write_last_copy(markdown);
    let copy_hint = |path: Option<PathBuf>| match path {
        Some(path) => {
            format!("; a copy is at {}", path.display())
        }
        None => String::new(),
    };
    match app.clipboard.write_text(markdown) {
        Ok(()) if terminal_client => CommandResult::message(format!(
            "{label} sent to the terminal-client clipboard over SSH via tmux/OSC 52 ({} lines){}; terminal support and settings determine whether the client accepts it",
            markdown.lines().count(),
            copy_hint(last_copy)
        )),
        Ok(()) => CommandResult::message(format!(
            "{label} copied to the local clipboard ({} lines; a terminal clipboard fallback may have been used){}",
            markdown.lines().count(),
            copy_hint(last_copy)
        )),
        Err(err) => match last_copy {
            Some(path) => CommandResult::error(format!(
                "Clipboard export failed: {err}. The full export was written to {}; /export file <path> writes it where you choose",
                path.display()
            )),
            None => CommandResult::error(format!(
                "Clipboard export failed: {err}. No file was written; use `/export file <path>` to choose an explicit destination"
            )),
        },
    }
}

/// Write the export to a predictable last-copy file under the Codewhale home
/// (#5555): a clipboard-only export on SSH/headless must never dead-end the
/// user, so the same content lands at `<home>/exports/last-copy.md` and every
/// failure message names it. Returns the path when the write succeeded.
pub(crate) fn write_last_copy(markdown: &str) -> Option<PathBuf> {
    let home = codewhale_paths::codewhale_home().ok().flatten()?;
    let exports_dir = home.join("exports");
    std::fs::create_dir_all(&exports_dir).ok()?;
    let physical_home = std::fs::canonicalize(&home).ok()?;
    let physical_exports = std::fs::canonicalize(&exports_dir).ok()?;
    if !physical_exports.starts_with(&physical_home) {
        return None;
    }
    write_last_copy_to(&exports_dir, markdown).ok()
}

fn write_last_copy_to(exports_dir: &Path, markdown: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(exports_dir)?;
    let path = exports_dir.join("last-copy.md");
    // Reuse the private atomic writer: random same-directory temp names,
    // restrictive creation mode, symlink-safe replacement, and Windows
    // replace retries are all part of the existing persistence contract.
    crate::utils::write_atomic(&path, markdown.as_bytes())?;
    Ok(path)
}

fn render_conversation(app: &App) -> String {
    let message_count = if app.api_messages.is_empty() {
        app.history.len()
    } else {
        app.api_messages.len()
    };
    let mut out = String::new();
    out.push_str("# Codewhale conversation export\n\n");
    let _ = writeln!(
        out,
        "- Exported: {}",
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    );
    let session = app
        .current_session_id
        .as_deref()
        .map(crate::session_manager::truncate_id)
        .unwrap_or("unsaved");
    let _ = writeln!(out, "- Session: {}", inline_text(session));
    let _ = writeln!(
        out,
        "- Provider: {}",
        inline_text(app.provider_identity_for_persistence())
    );
    let _ = writeln!(out, "- Model: {}", inline_text(&app.model_display_label()));
    let _ = writeln!(out, "- Mode: {}", app.mode.display_name());
    let workspace_name = app
        .workspace
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("workspace");
    let _ = writeln!(out, "- Workspace: {}", inline_text(workspace_name));
    let _ = writeln!(out, "- Messages: {message_count}");
    out.push_str(
        "\n> Hidden instructions, internal reasoning, and reasoning signatures are omitted. Secret-like values and credential-bearing URLs are redacted as a defense in depth; review the export before sharing it.\n\n",
    );

    let restore_points = RestorePoints::read(&app.workspace);
    restore_points.render_summary(&mut out);

    if app.api_messages.is_empty() {
        render_history_fallback(&mut out, &app.history);
    } else {
        for (index, message) in app.api_messages.iter().enumerate() {
            render_message(&mut out, index + 1, message);
            restore_points.render_correlation(&mut out, message);
        }
    }
    out
}

/// Maximum restore points listed in the export summary. The side-git repo is
/// already capped, but the export is a document a person reads, so it gets its
/// own bound rather than inheriting whatever the repo happens to hold.
const RESTORE_POINT_SUMMARY_MAX: usize = 100;

/// Characters of the snapshot SHA shown as a restore-point id.
const RESTORE_POINT_ID_LEN: usize = 12;

/// Restore points (side-git workspace snapshots) recorded for this workspace,
/// read read-only so `/export` never creates a snapshot repo as a side effect.
enum RestorePoints {
    /// The repo exists and these are its most recent snapshots (newest first).
    Recorded(Vec<crate::snapshot::Snapshot>),
    /// No snapshot repo exists for this workspace.
    None,
    /// The repo exists but could not be read. The reason is reported rather
    /// than swallowed — a silent omission would read as "no restore points".
    Unreadable(String),
}

impl RestorePoints {
    fn read(workspace: &Path) -> Self {
        match crate::snapshot::SnapshotRepo::open_existing(workspace) {
            Ok(None) => Self::None,
            Err(err) => Self::Unreadable(err.to_string()),
            Ok(Some(repo)) => match repo.list(RESTORE_POINT_SUMMARY_MAX) {
                Ok(snapshots) => Self::Recorded(snapshots),
                Err(err) => Self::Unreadable(err.to_string()),
            },
        }
    }

    fn render_summary(&self, out: &mut String) {
        out.push_str("## Restore points\n\n");
        match self {
            Self::None => {
                out.push_str(
                    "No workspace restore points are recorded for this workspace, so nothing in this export can be correlated to a restorable workspace state. Snapshots may be disabled, or no turn has taken one yet.\n\n",
                );
            }
            Self::Unreadable(reason) => {
                let _ = writeln!(
                    out,
                    "Workspace restore points could not be read ({}). Treat the correlation below as unavailable rather than empty.\n",
                    inline_text(reason)
                );
            }
            Self::Recorded(snapshots) if snapshots.is_empty() => {
                out.push_str(
                    "A snapshot repository exists for this workspace but records no restore points yet.\n\n",
                );
            }
            Self::Recorded(snapshots) => {
                let _ = writeln!(
                    out,
                    "The {} most recent workspace restore points, newest first. `/restore <N>` restores by the index in this table and `/restore list` shows the live list.\n",
                    snapshots.len()
                );
                out.push_str(
                    "> The index is the position at export time. Every new turn records another restore point and shifts it, so re-check `/restore list` before restoring from an older export. The snapshot id does not shift.\n\n",
                );
                out.push_str("| N | Restore point | Recorded (UTC) | Label |\n");
                out.push_str("| --- | --- | --- | --- |\n");
                for (index, snapshot) in snapshots.iter().enumerate() {
                    let _ = writeln!(
                        out,
                        "| {} | `{}` | {} | {} |",
                        index + 1,
                        short_restore_id(snapshot.id.as_str()),
                        format_snapshot_time(snapshot.timestamp),
                        inline_text(&snapshot.label)
                    );
                }
                out.push('\n');
            }
        }
    }

    /// Append the restore points correlated to a single user message.
    ///
    /// Correlation is by the prompt snippet the snapshot label actually
    /// embeds, produced by the same function the snapshot writer uses. No
    /// message-index-to-turn-sequence mapping is invented: a turn sequence and
    /// an export message index are different counters, and asserting they line
    /// up would be a guess presented as provenance.
    fn render_correlation(&self, out: &mut String, message: &Message) {
        if message.role != Role::User {
            return;
        }
        let Self::Recorded(snapshots) = self else {
            return;
        };
        let Some(text) = first_text_block(message) else {
            return;
        };
        let Some(snippet) = crate::core::turn::snapshot_label_prompt_snippet(text) else {
            return;
        };

        let matches: Vec<(usize, &crate::snapshot::Snapshot)> = snapshots
            .iter()
            .enumerate()
            .filter(|(_, snapshot)| {
                let parsed = crate::core::turn::parse_snapshot_label(&snapshot.label);
                matches!(parsed.kind.as_str(), "pre-turn" | "post-turn")
                    && parsed.prompt_snippet.as_deref() == Some(snippet.as_str())
            })
            .collect();

        if matches.is_empty() {
            out.push_str(
                "- Restore points: none recorded for this message within the listed window.\n\n",
            );
            return;
        }

        let ambiguous = matches.len() > 1;
        let rendered: Vec<String> = matches
            .iter()
            .map(|(index, snapshot)| {
                let parsed = crate::core::turn::parse_snapshot_label(&snapshot.label);
                let seq = parsed
                    .seq
                    .map(|seq| format!(" turn {seq}"))
                    .unwrap_or_default();
                format!(
                    "N{} `{}` ({}{})",
                    index + 1,
                    short_restore_id(snapshot.id.as_str()),
                    parsed.kind,
                    seq
                )
            })
            .collect();
        let _ = writeln!(out, "- Restore points: {}", rendered.join(", "));
        if ambiguous {
            out.push_str(
                "  - More than one restore point carries this prompt snippet, so the match is ambiguous; compare the recorded times above before restoring.\n",
            );
        }
        out.push('\n');
    }
}

fn short_restore_id(id: &str) -> String {
    id.chars().take(RESTORE_POINT_ID_LEN).collect()
}

fn format_snapshot_time(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_else(|| "unknown".to_string())
}

fn first_text_block(message: &Message) -> Option<&str> {
    message.content.iter().find_map(|block| match block {
        ContentBlock::Text { text, .. } => Some(text.as_str()),
        _ => None,
    })
}

fn render_message(out: &mut String, index: usize, message: &Message) {
    let role = inline_text(message.role.as_str());
    let _ = writeln!(out, "## {index}. {role}\n");
    if is_internal_role(message.role.as_str()) {
        out.push_str("[internal context omitted]\n\n");
        return;
    }
    if message.content.is_empty() {
        out.push_str("[no content]\n\n");
        return;
    }
    for (block_index, block) in message.content.iter().enumerate() {
        render_content_block(out, block_index + 1, block);
    }
}

fn render_content_block(out: &mut String, index: usize, block: &ContentBlock) {
    match block {
        ContentBlock::Text { text, .. } => {
            let _ = writeln!(out, "### Content {index}: Text\n");
            push_sanitized_text(out, text);
        }
        ContentBlock::ImageUrl { image_url } => {
            let _ = writeln!(out, "### Content {index}: Image attachment\n");
            if image_url.url.starts_with("http://") || image_url.url.starts_with("https://") {
                let _ = writeln!(
                    out,
                    "- Reference: {}\n",
                    inline_text(&crate::client::redact_url_for_display(&image_url.url))
                );
            } else {
                out.push_str("- Reference omitted (inline or local image payload)\n\n");
            }
        }
        ContentBlock::Thinking { .. } => {
            let _ = writeln!(out, "### Content {index}: Internal reasoning\n");
            out.push_str("[internal reasoning and signature omitted]\n\n");
        }
        ContentBlock::ToolUse {
            id,
            name,
            input,
            caller,
            ..
        } => {
            let _ = writeln!(out, "### Content {index}: Tool call\n");
            let _ = writeln!(out, "- ID: {}", inline_text(id));
            let _ = writeln!(out, "- Name: {}", inline_text(name));
            if let Some(caller) = caller {
                let _ = writeln!(out, "- Caller type: {}", inline_text(&caller.caller_type));
                if let Some(tool_id) = caller.tool_id.as_deref() {
                    let _ = writeln!(out, "- Caller tool ID: {}", inline_text(tool_id));
                }
            }
            out.push_str("\nInput:\n\n");
            push_json(out, input);
        }
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
            content_blocks,
        } => {
            let _ = writeln!(out, "### Content {index}: Tool result\n");
            let _ = writeln!(out, "- Tool call ID: {}", inline_text(tool_use_id));
            let _ = writeln!(out, "- Error: {}\n", is_error.unwrap_or(false));
            out.push_str("Result:\n\n");
            push_sanitized_text(out, content);
            if let Some(blocks) = content_blocks {
                out.push_str("Structured result blocks:\n\n");
                push_json(
                    out,
                    &Value::Array(
                        crate::image_attach::safe_tool_result_content_blocks(Some(blocks))
                            .unwrap_or_default(),
                    ),
                );
            }
        }
        ContentBlock::ServerToolUse { id, name, input } => {
            let _ = writeln!(out, "### Content {index}: Server tool call\n");
            let _ = writeln!(out, "- ID: {}", inline_text(id));
            let _ = writeln!(out, "- Name: {}\n", inline_text(name));
            out.push_str("Input:\n\n");
            push_json(out, input);
        }
        ContentBlock::ToolSearchToolResult {
            tool_use_id,
            content,
        } => {
            let _ = writeln!(out, "### Content {index}: Tool-search result\n");
            let _ = writeln!(out, "- Tool call ID: {}\n", inline_text(tool_use_id));
            push_json(out, content);
        }
        ContentBlock::CodeExecutionToolResult {
            tool_use_id,
            content,
        } => {
            let _ = writeln!(out, "### Content {index}: Code-execution result\n");
            let _ = writeln!(out, "- Tool call ID: {}\n", inline_text(tool_use_id));
            push_json(out, content);
        }
    }
}

fn render_history_fallback(out: &mut String, history: &[HistoryCell]) {
    if history.is_empty() {
        out.push_str("## Conversation\n\n[empty conversation]\n");
        return;
    }
    out.push_str(
        "> Structured API messages were unavailable; the entries below are a sanitized visible-history fallback.\n\n",
    );
    for (index, cell) in history.iter().enumerate() {
        let (role, body) = match cell {
            HistoryCell::User { content } => ("user", sanitize_text(content)),
            HistoryCell::Assistant { content, .. } => ("assistant", sanitize_text(content)),
            HistoryCell::System { .. } => ("system", "[internal context omitted]".to_string()),
            HistoryCell::Error { message, severity } => {
                let role = match severity {
                    crate::error_taxonomy::ErrorSeverity::Info => "info",
                    crate::error_taxonomy::ErrorSeverity::Warning => "warning",
                    crate::error_taxonomy::ErrorSeverity::Error => "error",
                    crate::error_taxonomy::ErrorSeverity::Critical => "critical error",
                };
                (role, sanitize_text(message))
            }
            HistoryCell::Thinking { .. } => (
                "internal reasoning",
                "[internal reasoning omitted]".to_string(),
            ),
            HistoryCell::Tool(tool) => ("tool", sanitize_text(&render_lines(tool.lines(120)))),
            HistoryCell::SubAgent(subagent) => (
                "sub-agent",
                sanitize_text(&render_lines(subagent.lines(120))),
            ),
            HistoryCell::Automation(cell) => {
                ("automation", sanitize_text(&render_lines(cell.render(120))))
            }
            HistoryCell::ArchivedContext {
                level,
                range,
                summary,
                ..
            } => (
                "archived context",
                sanitize_text(&format!("L{level} [{range}]: {summary}")),
            ),
        };
        let _ = writeln!(out, "## {}. {}\n", index + 1, inline_text(role));
        push_pre_sanitized_text(out, &body);
    }
}

fn render_lines(lines: Vec<ratatui::text::Line<'static>>) -> String {
    lines
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn push_sanitized_text(out: &mut String, text: &str) {
    push_pre_sanitized_text(out, &sanitize_text(text));
}

fn push_pre_sanitized_text(out: &mut String, text: &str) {
    if text.trim().is_empty() {
        out.push_str("[empty text]\n\n");
    } else {
        out.push_str(text.trim_end());
        out.push_str("\n\n");
    }
}

fn push_json(out: &mut String, value: &Value) {
    let mut redacted = value.clone();
    redact_json(&mut redacted, None);
    let json = serde_json::to_string_pretty(&redacted)
        .unwrap_or_else(|_| "\"[structured content unavailable]\"".to_string());
    let fence = markdown_fence(&json);
    let _ = writeln!(out, "{fence}json\n{json}\n{fence}\n");
}

// Widened to `pub(super)` so `/structcopy` (#2033) reuses this exact seam
// instead of copying it.
pub(super) fn redact_json(value: &mut Value, key: Option<&str>) {
    if key.is_some_and(is_sensitive_key) {
        *value = Value::String("[redacted]".to_string());
        return;
    }
    match value {
        Value::String(text) => *text = sanitize_text(text),
        Value::Array(items) => {
            for item in items {
                redact_json(item, None);
            }
        }
        Value::Object(map) => {
            for (key, value) in map {
                redact_json(value, Some(key));
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

// Widened to `pub(super)` so `/structcopy` can classify a key again after
// removing control/ANSI obfuscation. Classification before and after
// normalization keeps the shared sensitive-key vocabulary authoritative.
pub(super) fn is_sensitive_key(key: &str) -> bool {
    let normalized = key
        .trim()
        .trim_matches(['\'', '"'])
        .replace(['-', '.', ' '], "_")
        .to_ascii_lowercase();
    [
        "api_key",
        "apikey",
        "secret",
        "token",
        "password",
        "passwd",
        "authorization",
        "access_key",
        "client_secret",
        "private_key",
        "cookie",
        "session_key",
    ]
    .iter()
    .any(|hint| normalized.contains(hint))
}

// Widened to `pub(super)` so `/structcopy` (#2033) reuses this exact seam
// instead of copying it.
pub(super) fn sanitize_text(input: &str) -> String {
    let mut visible = String::with_capacity(input.len());
    crate::tui::osc8::strip_ansi_into(input, &mut visible);
    let visible = visible.replace("\r\n", "\n").replace('\r', "\n");
    let visible: String = visible
        .chars()
        .filter(|ch| *ch == '\n' || *ch == '\t' || !ch.is_control())
        .collect();
    let private_keys = private_key_regex().replace_all(&visible, "[redacted private key]");
    let bearer = bearer_regex().replace_all(&private_keys, "Bearer [redacted]");
    let jwt = jwt_regex().replace_all(&bearer, "[redacted token]");
    let urls = url_regex().replace_all(&jwt, |captures: &regex::Captures<'_>| {
        redact_url_match(captures.get(0).map_or("", |value| value.as_str()))
    });
    codewhale_config::persistence::redact_secrets(&urls)
}

fn redact_url_match(raw: &str) -> String {
    let trimmed = raw.trim_end_matches(['.', ',', ';', '!']);
    let suffix = &raw[trimmed.len()..];
    format!(
        "{}{}",
        crate::client::redact_url_for_display(trimmed),
        suffix
    )
}

fn inline_text(input: &str) -> String {
    sanitize_text(input)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('`', "'")
}

// Widened to `pub(super)` so `/structcopy` (#2033) reuses this exact seam
// instead of copying it.
pub(super) fn is_internal_role(role: &str) -> bool {
    matches!(
        role.trim().to_ascii_lowercase().as_str(),
        "system" | "developer" | "internal"
    )
}

fn markdown_fence(content: &str) -> String {
    let longest = content
        .split(|ch| ch != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    "`".repeat(longest.saturating_add(1).max(3))
}

fn private_key_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?is)-----BEGIN [^-\r\n]*PRIVATE KEY-----.*?-----END [^-\r\n]*PRIVATE KEY-----",
        )
        .expect("private-key redaction regex")
    })
}

fn bearer_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\bbearer\s+[a-z0-9._~+/=-]{6,}").expect("bearer redaction regex")
    })
}

fn jwt_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\beyJ[a-zA-Z0-9_-]{5,}\.[a-zA-Z0-9_-]{5,}(?:\.[a-zA-Z0-9_-]{5,})?\b")
            .expect("JWT redaction regex")
    })
}

fn url_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"https?://[^\s<>\"'`\]\[\)\(\}\{]+"#).expect("URL redaction regex")
    })
}

fn sanitize_turn_handoff(app: &App, markdown: &str) -> String {
    let sanitized = sanitize_text(markdown);
    let workspace = app.workspace.to_string_lossy();
    if workspace.is_empty() {
        sanitized
    } else {
        sanitized.replace(workspace.as_ref(), ".")
    }
}

fn resolve_export_path(workspace: &Path, raw: &str) -> Result<PathBuf, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("export path is empty".to_string());
    }
    let requested = PathBuf::from(raw);
    if requested
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(
            "export paths may not contain `..`; use an explicit normalized absolute path instead"
                .to_string(),
        );
    }
    // Resolve the trusted workspace root once so platform aliases such as
    // macOS `/var -> /private/var` do not make every workspace-relative
    // export look like it traverses a user-controlled symlink. Requested
    // components beneath that root remain lexical and are checked below.
    let resolved_workspace =
        fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    let path = if requested.is_absolute() {
        if let Ok(relative) = requested.strip_prefix(workspace) {
            resolved_workspace.join(relative)
        } else if let Ok(relative) = requested.strip_prefix(&resolved_workspace) {
            resolved_workspace.join(relative)
        } else {
            requested
        }
    } else {
        resolved_workspace.join(requested)
    };
    if path.file_name().is_none() {
        return Err(format!("export path must name a file: {}", path.display()));
    }
    Ok(path)
}

fn write_export_file(path: &Path, contents: &[u8], force: bool) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| format!("path has no parent directory: {}", path.display()))?;
    let parent_metadata = fs::metadata(parent).map_err(|err| {
        format!(
            "parent directory {} is unavailable: {err}",
            parent.display()
        )
    })?;
    if !parent_metadata.is_dir() {
        return Err(format!("parent is not a directory: {}", parent.display()));
    }
    reject_symlink_components(path)?;

    match fs::symlink_metadata(path) {
        Ok(_) if !force => {
            return Err(format!(
                "destination already exists: {}. Re-run with `/export file --force <path>` to replace it",
                path.display()
            ));
        }
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(format!(
                "refusing to replace a non-regular file: {}",
                path.display()
            ));
        }
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(format!("could not inspect {}: {err}", path.display())),
    }

    if force {
        crate::utils::write_atomic(path, contents).map_err(|err| err.to_string())?;
        set_owner_only(path).map_err(|err| format!("could not secure file permissions: {err}"))?;
        return Ok(());
    }

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::AlreadyExists {
            format!(
                "destination already exists: {}. Re-run with `/export file --force <path>` to replace it",
                path.display()
            )
        } else {
            err.to_string()
        }
    })?;
    if let Err(err) = file.write_all(contents).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(err.to_string());
    }
    set_owner_only(path).map_err(|err| format!("could not secure file permissions: {err}"))
}

fn reject_symlink_components(path: &Path) -> Result<(), String> {
    for component_path in path.ancestors() {
        match fs::symlink_metadata(component_path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "refusing export through symlink component: {}",
                    component_path.display()
                ));
            }
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(format!(
                    "could not inspect path component {}: {err}",
                    component_path.display()
                ));
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_owner_only(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::models::{ImageUrlContent, ToolCaller};
    use crate::tui::app::{App, TuiOptions};
    use crate::tui::clipboard::ClipboardHandler;
    use tempfile::TempDir;

    fn test_app(tmpdir: &TempDir) -> App {
        let options = TuiOptions {
            skills_dir: tmpdir.path().join("skills"),
            memory_path: tmpdir.path().join("memory.md"),
            notes_path: tmpdir.path().join("notes.txt"),
            mcp_config_path: tmpdir.path().join("mcp.json"),
            ..crate::test_support::test_tui_options(tmpdir.path())
        };
        App::new(options, &Config::default())
    }

    #[test]
    fn last_copy_writes_the_export_without_leaving_a_temp_artifact() {
        let tmp = TempDir::new().expect("tempdir");
        let dir = tmp.path().join("exports");
        let path = write_last_copy_to(&dir, "# export\n\nhello\n").expect("write");
        assert_eq!(path, dir.join("last-copy.md"));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "# export\n\nhello\n"
        );
        // The next export overwrites the same predictable path.
        write_last_copy_to(&dir, "# second\n").expect("rewrite");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "# second\n");
        assert_eq!(
            std::fs::read_dir(&dir).expect("read exports").count(),
            1,
            "the atomic writer must not leave a temp artifact"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "recovery copy must remain private");
        }
    }

    #[test]
    fn last_copy_stays_inside_an_explicit_codewhale_home() {
        let ambient = TempDir::new().expect("ambient home");
        let isolated = TempDir::new().expect("isolated Codewhale home");
        let _env_lock = crate::test_support::lock_test_env();
        let _home = crate::test_support::EnvVarGuard::set("HOME", ambient.path());
        let _codewhale_home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", isolated.path());

        let path = write_last_copy("isolated response").expect("recovery copy");

        assert_eq!(path, isolated.path().join("exports/last-copy.md"));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read recovery copy"),
            "isolated response"
        );
        assert!(
            !ambient.path().join("exports/last-copy.md").exists(),
            "explicit CODEWHALE_HOME must prevent ambient-home writes"
        );
    }

    #[cfg(unix)]
    #[test]
    fn last_copy_refuses_an_exports_symlink_outside_codewhale_home() {
        use std::os::unix::fs::symlink;

        let ambient = TempDir::new().expect("ambient home");
        let isolated = TempDir::new().expect("isolated Codewhale home");
        let external = TempDir::new().expect("external dir");
        symlink(external.path(), isolated.path().join("exports")).expect("exports symlink");
        let _env_lock = crate::test_support::lock_test_env();
        let _home = crate::test_support::EnvVarGuard::set("HOME", ambient.path());
        let _codewhale_home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", isolated.path());

        assert_eq!(write_last_copy("must stay isolated"), None);
        assert!(
            !external.path().join("last-copy.md").exists(),
            "recovery content must not escape through a nested symlink"
        );
    }

    #[test]
    fn default_clipboard_export_preserves_structure_and_redacts_secrets() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        app.current_session_id = Some("session-123456789".to_string());
        app.api_messages = vec![
            Message {
                role: Role::System,
                content: vec![ContentBlock::Text {
                    text: "hidden policy must never export".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "Please inspect this\u{1b}[31m output\u{1b}[0m".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Thinking {
                        thinking: "private chain of thought".to_string(),
                        signature: Some("signature-secret".to_string()),
                        state: None,
                    },
                    ContentBlock::ToolUse {
                        id: "call-1".to_string(),
                        name: "fetch_url".to_string(),
                        input: serde_json::json!({
                            "url": "https://alice:password@example.com/path?token=very-secret&ok=1",
                            "api_key": "literal-api-secret",
                            "nested": {"authorization": "Bearer abcdefghijklmnop"},
                        }),
                        caller: Some(ToolCaller {
                            caller_type: "code_execution_20250825".to_string(),
                            tool_id: Some("server-tool-1".to_string()),
                        }),
                        thought_signature: None,
                    },
                ],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "call-1".to_string(),
                    content: "Authorization: Bearer another-secret-token\nresult ok".to_string(),
                    is_error: Some(false),
                    content_blocks: Some(vec![serde_json::json!({
                        "image": "https://example.com/a.png?api_key=hidden",
                        "session_token": "session-secret",
                    })]),
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ImageUrl {
                    image_url: ImageUrlContent {
                        url: "data:image/png;base64,very-secret-image-data".to_string(),
                    },
                }],
            },
        ];
        {
            let mut todos = app.todos.try_lock().expect("todos lock");
            todos.add(
                "export projection".to_string(),
                crate::tools::todo::TodoStatus::InProgress,
            );
        }
        app.cycle_effort();
        let work_before = app.work_state_snapshot().expect("Work snapshot");

        let result = execute_export(&mut app, None);

        assert!(!result.is_error, "{:?}", result.message);
        assert!(
            result
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("local clipboard")
        );
        let markdown = app
            .clipboard
            .last_written_text()
            .expect("clipboard payload");
        let system = markdown.find("## 1. system").expect("system role");
        let user = markdown.find("## 2. user").expect("user role");
        let assistant = markdown.find("## 3. assistant").expect("assistant role");
        let tool_result = markdown.find("## 4. user").expect("tool-result role");
        assert!(system < user && user < assistant && assistant < tool_result);
        assert!(markdown.contains("[internal context omitted]"));
        assert!(markdown.contains("call-1"));
        assert!(markdown.contains("fetch_url"));
        assert!(markdown.contains("server-tool-1"));
        assert!(markdown.contains("[internal reasoning and signature omitted]"));
        assert!(markdown.contains("[redacted]"));
        assert!(markdown.contains("https://***:***@example.com/path?token=***&ok=1"));
        assert!(markdown.contains("Reference omitted (inline or local image payload)"));
        let workspace_path = tmpdir.path().to_string_lossy().into_owned();
        for forbidden in [
            "hidden policy must never export",
            "private chain of thought",
            "signature-secret",
            "literal-api-secret",
            "very-secret",
            "another-secret-token",
            "session-secret",
            "very-secret-image-data",
            "\u{1b}[31m",
            workspace_path.as_str(),
        ] {
            assert!(
                !markdown.contains(forbidden),
                "leaked {forbidden:?}: {markdown}"
            );
        }
        assert_eq!(
            app.work_state_snapshot()
                .expect("Work snapshot after export"),
            work_before,
            "export must not mutate Work"
        );
    }

    #[test]
    fn clipboard_export_reports_ssh_terminal_client_and_failure_honestly() {
        let tmpdir = TempDir::new().expect("tempdir");
        // Seal both ambient and product homes so the backup cannot escape the
        // test sandbox even when the outer process has CODEWHALE_HOME set.
        let _env_lock = crate::test_support::lock_test_env();
        let test_home = tmpdir.path().join("home");
        std::fs::create_dir_all(&test_home).expect("home dir");
        let _home = crate::test_support::EnvVarGuard::set("HOME", &test_home);
        let _codewhale_home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &test_home);
        let mut app = test_app(&tmpdir);
        app.clipboard = ClipboardHandler::for_test(true, true);
        let ssh = execute_export(&mut app, Some("clipboard"));
        assert!(!ssh.is_error, "{:?}", ssh.message);
        assert!(
            ssh.message
                .as_deref()
                .unwrap_or_default()
                .contains("terminal-client clipboard over SSH")
        );
        assert!(
            ssh.message
                .as_deref()
                .unwrap_or_default()
                .contains("last-copy.md"),
            "success must name the backup copy: {:?}",
            ssh.message
        );

        app.clipboard = ClipboardHandler::unavailable_for_test(false);
        let failed = execute_export(&mut app, Some("clipboard"));
        assert!(failed.is_error);
        let message = failed.message.as_deref().unwrap_or_default();
        assert!(
            message.contains("The full export was written to"),
            "{message}"
        );
        assert!(message.contains("last-copy.md"), "{message}");
        assert!(message.contains("/export file <path>"), "{message}");
        assert!(!tmpdir.path().join("chat_export.md").exists());
        assert!(test_home.join("exports/last-copy.md").exists());
    }

    #[test]
    fn file_export_is_workspace_relative_private_and_no_overwrite_by_default() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        app.api_messages.push(Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "first export".to_string(),
                cache_control: None,
            }],
        });

        let first = execute_export(&mut app, Some("file transcript.md"));
        assert!(!first.is_error, "{:?}", first.message);
        let path = tmpdir.path().join("transcript.md");
        let original = fs::read_to_string(&path).expect("first export");
        assert!(original.contains("first export"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        app.api_messages[0].content = vec![ContentBlock::Text {
            text: "replacement export".to_string(),
            cache_control: None,
        }];
        let refused = execute_export(&mut app, Some("transcript.md"));
        assert!(refused.is_error);
        assert_eq!(fs::read_to_string(&path).unwrap(), original);

        let forced = execute_export(&mut app, Some("file --force transcript.md"));
        assert!(!forced.is_error, "{:?}", forced.message);
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("replacement export")
        );
    }

    #[test]
    fn file_export_rejects_traversal_missing_parent_and_invalid_usage() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        for arg in [
            "file ../outside.md",
            "file missing/export.md",
            "file",
            "file --force",
            "clipboard extra.md",
            "turn clipboard extra.md",
        ] {
            let result = execute_export(&mut app, Some(arg));
            assert!(result.is_error, "{arg}: {:?}", result.message);
        }
        assert!(!tmpdir.path().join("outside.md").exists());
    }

    #[cfg(unix)]
    #[test]
    fn file_export_rejects_symlink_leaf_and_ancestor() {
        use std::os::unix::fs::symlink;

        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        let real_file = tmpdir.path().join("real.md");
        fs::write(&real_file, "keep").expect("fixture file");
        let leaf = tmpdir.path().join("leaf.md");
        symlink(&real_file, &leaf).expect("leaf symlink");
        let leaf_result =
            execute_export(&mut app, Some(&format!("file --force {}", leaf.display())));
        assert!(leaf_result.is_error, "{:?}", leaf_result.message);
        assert_eq!(fs::read_to_string(&real_file).unwrap(), "keep");

        let real_dir = tmpdir.path().join("real-dir");
        fs::create_dir(&real_dir).expect("real dir");
        let linked_dir = tmpdir.path().join("linked-dir");
        symlink(&real_dir, &linked_dir).expect("dir symlink");
        let ancestor_result = execute_export(
            &mut app,
            Some(&format!("file {}", linked_dir.join("out.md").display())),
        );
        assert!(ancestor_result.is_error, "{:?}", ancestor_result.message);
        assert!(!real_dir.join("out.md").exists());
    }

    #[test]
    fn turn_export_supports_clipboard_and_safe_legacy_file_destination() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        app.history.push(HistoryCell::User {
            content: "Fix the flaky login test".to_string(),
        });
        app.history.push(HistoryCell::Assistant {
            content: "Fixed the login test.".to_string(),
            streaming: false,
        });
        app.runtime_turn_status = Some("completed".to_string());

        let clipboard = execute_export(&mut app, Some("turn"));
        assert!(!clipboard.is_error, "{:?}", clipboard.message);
        assert!(
            app.clipboard
                .last_written_text()
                .unwrap_or_default()
                .contains("# Turn handoff")
        );

        let path = tmpdir.path().join("handoff.md");
        let file = execute_export(&mut app, Some(&format!("turn {}", path.display())));
        assert!(!file.is_error, "{:?}", file.message);
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("Fix the flaky login test")
        );
        let refused = execute_export(&mut app, Some(&format!("turn {}", path.display())));
        assert!(refused.is_error);
    }

    #[test]
    fn parser_keeps_paths_with_spaces_and_legacy_forms() {
        assert_eq!(
            parse_request(Some("file --force reports/chat export.md")).unwrap(),
            ExportRequest {
                scope: ExportScope::Conversation,
                destination: ExportDestination::File {
                    path: "reports/chat export.md".to_string(),
                    force: true,
                },
            }
        );
        assert_eq!(
            parse_request(Some("legacy export.md")).unwrap(),
            ExportRequest {
                scope: ExportScope::Conversation,
                destination: ExportDestination::File {
                    path: "legacy export.md".to_string(),
                    force: false,
                },
            }
        );
    }

    fn snapshot(id: &str, label: &str, timestamp: i64) -> crate::snapshot::Snapshot {
        crate::snapshot::Snapshot {
            id: crate::snapshot::SnapshotId(id.to_string()),
            label: label.to_string(),
            timestamp,
            session_id: None,
        }
    }

    fn user_message(text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    #[test]
    fn restore_point_summary_lists_each_point_with_its_restore_index() {
        let points = RestorePoints::Recorded(vec![
            snapshot(
                "a".repeat(40).as_str(),
                "pre-turn:2: second prompt",
                1_700_000_100,
            ),
            snapshot(
                "b".repeat(40).as_str(),
                "pre-turn:1: first prompt",
                1_700_000_000,
            ),
        ]);
        let mut out = String::new();
        points.render_summary(&mut out);

        assert!(out.contains("## Restore points"), "{out}");
        assert!(
            out.contains(
                "| 1 | `aaaaaaaaaaaa` | 2023-11-14T22:15:00Z | pre-turn:2: second prompt |"
            ),
            "newest point must be restore index 1: {out}"
        );
        assert!(
            out.contains(
                "| 2 | `bbbbbbbbbbbb` | 2023-11-14T22:13:20Z | pre-turn:1: first prompt |"
            ),
            "{out}"
        );
        assert!(
            out.contains("`/restore <N>`"),
            "the export must name the command that consumes the index: {out}"
        );
        assert!(
            out.contains("position at export time"),
            "the index is only valid until the next snapshot; say so: {out}"
        );
    }

    #[test]
    fn user_message_correlates_to_its_own_restore_point() {
        let points = RestorePoints::Recorded(vec![
            snapshot(
                "c".repeat(40).as_str(),
                "pre-turn:4: rename the widget",
                1_700_000_200,
            ),
            snapshot(
                "d".repeat(40).as_str(),
                "pre-turn:3: unrelated prompt",
                1_700_000_100,
            ),
        ]);
        let mut out = String::new();
        points.render_correlation(&mut out, &user_message("rename the widget\ndetail line"));

        assert!(
            out.contains("- Restore points: N1 `cccccccccccc` (pre-turn turn 4)"),
            "{out}"
        );
        assert!(
            !out.contains("dddddddddddd"),
            "an unrelated prompt must not be correlated: {out}"
        );
    }

    #[test]
    fn repeated_prompts_are_reported_as_ambiguous_rather_than_guessed() {
        let points = RestorePoints::Recorded(vec![
            snapshot(
                "e".repeat(40).as_str(),
                "pre-turn:9: run the tests",
                1_700_000_300,
            ),
            snapshot(
                "f".repeat(40).as_str(),
                "pre-turn:5: run the tests",
                1_700_000_100,
            ),
        ]);
        let mut out = String::new();
        points.render_correlation(&mut out, &user_message("run the tests"));

        assert!(out.contains("N1 `eeeeeeeeeeee` (pre-turn turn 9)"), "{out}");
        assert!(out.contains("N2 `ffffffffffff` (pre-turn turn 5)"), "{out}");
        assert!(
            out.contains("ambiguous"),
            "two identical prompts must not be silently resolved to one: {out}"
        );
    }

    #[test]
    fn a_message_with_no_recorded_restore_point_says_none_rather_than_nothing() {
        let points = RestorePoints::Recorded(vec![snapshot(
            "1".repeat(40).as_str(),
            "pre-turn:1: something else",
            1_700_000_000,
        )]);
        let mut out = String::new();
        points.render_correlation(&mut out, &user_message("never snapshotted"));
        assert!(out.contains("none recorded for this message"), "{out}");
    }

    #[test]
    fn tool_snapshots_are_not_correlated_to_user_messages() {
        let points = RestorePoints::Recorded(vec![snapshot(
            "2".repeat(40).as_str(),
            "tool:call_abc: rename the widget",
            1_700_000_000,
        )]);
        let mut out = String::new();
        points.render_correlation(&mut out, &user_message("rename the widget"));
        assert!(
            out.contains("none recorded for this message"),
            "a per-tool snapshot is not the turn's restore point: {out}"
        );
    }

    #[test]
    fn assistant_messages_get_no_correlation_line() {
        let points = RestorePoints::Recorded(vec![snapshot(
            "3".repeat(40).as_str(),
            "pre-turn:1: hello",
            1_700_000_000,
        )]);
        let mut out = String::new();
        points.render_correlation(
            &mut out,
            &Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: "hello".to_string(),
                    cache_control: None,
                }],
            },
        );
        assert!(out.is_empty(), "{out}");
    }

    #[test]
    fn absent_snapshot_repo_is_reported_as_unavailable_not_as_an_empty_list() {
        let mut out = String::new();
        RestorePoints::None.render_summary(&mut out);
        assert!(
            out.contains("No workspace restore points are recorded"),
            "{out}"
        );
        assert!(
            out.contains("nothing in this export can be correlated"),
            "the export must not imply restorability it does not have: {out}"
        );
    }

    #[test]
    fn unreadable_snapshot_repo_reports_the_reason_instead_of_omitting_it() {
        let mut out = String::new();
        RestorePoints::Unreadable("permission denied".to_string()).render_summary(&mut out);
        assert!(out.contains("could not be read"), "{out}");
        assert!(out.contains("permission denied"), "{out}");
        assert!(
            out.contains("unavailable rather than empty"),
            "unknown must stay unknown: {out}"
        );
    }

    #[test]
    fn an_existing_repo_with_no_commits_is_distinguished_from_no_repo() {
        let mut out = String::new();
        RestorePoints::Recorded(Vec::new()).render_summary(&mut out);
        assert!(out.contains("records no restore points yet"), "{out}");
    }

    #[test]
    fn export_does_not_create_a_snapshot_repo_for_a_fresh_workspace() {
        let tmpdir = TempDir::new().expect("tempdir");
        let workspace = tmpdir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let before = crate::snapshot::snapshot_git_dir(&workspace);
        assert!(!before.exists(), "precondition: no side repo yet");

        assert!(matches!(
            RestorePoints::read(&workspace),
            RestorePoints::None
        ));

        assert!(
            !crate::snapshot::snapshot_git_dir(&workspace).exists(),
            "reading restore points must never create the side repo"
        );
    }
}
