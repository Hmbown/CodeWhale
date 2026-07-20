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
use crate::models::{ContentBlock, Message};
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
    app.refresh_sensitive_user_input_projection();
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
    match app.clipboard.write_text(markdown) {
        Ok(()) if terminal_client => CommandResult::message(format!(
            "{label} sent to the terminal-client clipboard over SSH via tmux/OSC 52 ({} lines); terminal support and settings determine whether the client accepts it",
            markdown.lines().count()
        )),
        Ok(()) => CommandResult::message(format!(
            "{label} copied to the local clipboard ({} lines; a terminal clipboard fallback may have been used)",
            markdown.lines().count()
        )),
        Err(err) => CommandResult::error(format!(
            "Clipboard export failed: {err}. No file was written; use `/export file <path>` to choose an explicit destination"
        )),
    }
}

fn render_conversation(app: &App) -> String {
    let sensitive_values = app.sensitive_user_input_provenance.snapshot();
    let public_messages = crate::runtime_threads::redacted_durable_history_clone(
        &app.api_messages,
        &sensitive_values,
    );
    let message_count = if public_messages.is_empty() {
        app.history.len()
    } else {
        public_messages.len()
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

    if public_messages.is_empty() {
        render_history_fallback(&mut out, &app.history, &sensitive_values);
    } else {
        for (index, message) in public_messages.iter().enumerate() {
            render_message(&mut out, index + 1, message);
        }
    }
    out
}

fn render_message(out: &mut String, index: usize, message: &Message) {
    let role = inline_text(&message.role);
    let _ = writeln!(out, "## {index}. {role}\n");
    if is_internal_role(&message.role) {
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
                push_json(out, &Value::Array(blocks.clone()));
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

fn render_history_fallback(
    out: &mut String,
    history: &[HistoryCell],
    sensitive_values: &std::collections::HashSet<String>,
) {
    if history.is_empty() {
        out.push_str("## Conversation\n\n[empty conversation]\n");
        return;
    }
    out.push_str(
        "> Structured API messages were unavailable; the entries below are a sanitized visible-history fallback.\n\n",
    );
    let sanitize = |text: &str| {
        sanitize_text(&crate::runtime_threads::redacted_sensitive_user_input_text(
            text,
            sensitive_values,
        ))
    };
    for (index, cell) in history.iter().enumerate() {
        let (role, body) = match cell {
            HistoryCell::User { content } => ("user", sanitize(content)),
            HistoryCell::Assistant { content, .. } => ("assistant", sanitize(content)),
            HistoryCell::System { .. } => ("system", "[internal context omitted]".to_string()),
            HistoryCell::Error { message, severity } => {
                let role = match severity {
                    crate::error_taxonomy::ErrorSeverity::Info => "info",
                    crate::error_taxonomy::ErrorSeverity::Warning => "warning",
                    crate::error_taxonomy::ErrorSeverity::Error => "error",
                    crate::error_taxonomy::ErrorSeverity::Critical => "critical error",
                };
                (role, sanitize(message))
            }
            HistoryCell::Thinking { .. } => (
                "internal reasoning",
                "[internal reasoning omitted]".to_string(),
            ),
            HistoryCell::Tool(tool) => ("tool", sanitize(&render_lines(tool.lines(120)))),
            HistoryCell::SubAgent(subagent) => {
                ("sub-agent", sanitize(&render_lines(subagent.lines(120))))
            }
            HistoryCell::ArchivedContext {
                level,
                range,
                summary,
                ..
            } => (
                "archived context",
                sanitize(&format!("L{level} [{range}]: {summary}")),
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

fn redact_json(value: &mut Value, key: Option<&str>) {
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

fn is_sensitive_key(key: &str) -> bool {
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

fn sanitize_text(input: &str) -> String {
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

fn is_internal_role(role: &str) -> bool {
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
    let sensitive_values = app.sensitive_user_input_provenance.snapshot();
    let public_markdown = markdown
        .split('\n')
        .enumerate()
        .map(|(index, line)| project_turn_handoff_line(index, line, &sensitive_values))
        .collect::<Vec<_>>()
        .join("\n");
    let sanitized = sanitize_text(&public_markdown);
    let workspace = app.workspace.to_string_lossy();
    if workspace.is_empty() {
        sanitized
    } else {
        sanitized.replace(workspace.as_ref(), ".")
    }
}

/// Preserve renderer-owned Markdown structure when an answer happens to be a
/// short structural word (`Input`, `ID`, `Name`, `Result`, ...). Dynamic body
/// text is still projected, including the value side of generated key/value
/// bullets. This avoids applying taint replacement to one undifferentiated
/// Markdown blob after structure has already been rendered.
fn project_turn_handoff_line(
    index: usize,
    line: &str,
    sensitive_values: &std::collections::HashSet<String>,
) -> String {
    const SECTION_HEADINGS: &[&str] = &[
        "## Intent",
        "## Strategy / To-do",
        "## Files changed",
        "## Turn timeline",
        "## Tests / verifier",
        "## Model route + tokens/cost",
        "## Result / status",
    ];
    const FIXED_LINES: &[&str] = &["—"];

    if (index == 0 && line.starts_with("# Turn handoff"))
        || SECTION_HEADINGS.contains(&line)
        || FIXED_LINES.contains(&line)
        || (index == 1 && line.starts_with("_Status:") && line.ends_with('_'))
        || line.is_empty()
    {
        return line.to_string();
    }

    if let Some(body) = line.strip_prefix("- ") {
        // Only renderer-owned labels are structural. Treating every substring
        // before the first `: ` as a label lets user-authored plan/todo text
        // escape projection (for example `[ ] call 482915: then verify`).
        // Keep this allowlist exact and project the whole row otherwise.
        const FIXED_BULLET_PREFIXES: &[&str] = &[
            "Strategy: ",
            "Route steps: ",
            "To-do: ",
            "Route: ",
            "Auto decision: ",
            "Auto pair: ",
            "Auto scope: ",
            "Auto data: ",
            "Tokens (last turn): ",
            "Tokens (session): ",
            "Cost (session): ",
            "Usage plan: ",
            "Cost: ",
            "Status: ",
            "Result: ",
            "Error: ",
        ];
        if let Some(prefix) = FIXED_BULLET_PREFIXES
            .iter()
            .find(|prefix| body.starts_with(**prefix))
        {
            let value = &body[prefix.len()..];
            return format!(
                "- {prefix}{}",
                crate::runtime_threads::redacted_sensitive_user_input_text(value, sensitive_values,)
            );
        }
        return format!(
            "- {}",
            crate::runtime_threads::redacted_sensitive_user_input_text(body, sensitive_values)
        );
    }

    crate::runtime_threads::redacted_sensitive_user_input_text(line, sensitive_values)
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
            model: "deepseek-v4-pro".to_string(),
            workspace: tmpdir.path().to_path_buf(),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: tmpdir.path().join("skills"),
            memory_path: tmpdir.path().join("memory.md"),
            notes_path: tmpdir.path().join("notes.txt"),
            mcp_config_path: tmpdir.path().join("mcp.json"),
            use_memory: false,
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        App::new(options, &Config::default())
    }

    #[test]
    fn default_clipboard_export_preserves_structure_and_redacts_secrets() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        app.current_session_id = Some("session-123456789".to_string());
        app.api_messages = vec![
            Message {
                role: "system".to_string(),
                content: vec![ContentBlock::Text {
                    text: "hidden policy must never export".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Please inspect this\u{1b}[31m output\u{1b}[0m".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: "assistant".to_string(),
                content: vec![
                    ContentBlock::Thinking {
                        thinking: "private chain of thought".to_string(),
                        signature: Some("signature-secret".to_string()),
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
                    },
                ],
            },
            Message {
                role: "user".to_string(),
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
                role: "assistant".to_string(),
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
    fn export_projects_short_modal_values_but_preserves_tool_headings() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        app.sensitive_user_input_provenance.extend([
            "input".to_string(),
            "Input".to_string(),
            "ID".to_string(),
            "Name".to_string(),
            "Tool call".to_string(),
            "7".to_string(),
        ]);
        app.api_messages = vec![Message {
            role: "assistant".to_string(),
            content: vec![
                ContentBlock::Text {
                    text: "echo input, Input, ID, Name, Tool call, and 7".to_string(),
                    cache_control: None,
                },
                ContentBlock::ToolUse {
                    id: "call-input".to_string(),
                    name: "echo".to_string(),
                    input: serde_json::json!({"input": "7", "safe": "input"}),
                    caller: None,
                },
            ],
        }];

        let markdown = render_conversation(&app);
        assert!(markdown.contains("Tool call"));
        assert!(markdown.contains("- ID: call-input"));
        assert!(markdown.contains("- Name: echo"));
        assert!(markdown.contains("Input:"));
        assert!(!markdown.contains("echo input, Input, ID, Name, Tool call, and 7"));
        assert!(!markdown.contains("\"input\": \"7\""));
        assert!(!markdown.contains("\"safe\": \"input\""));

        app.api_messages.clear();
        app.history.push(HistoryCell::Assistant {
            content: "history echoed input and 7".to_string(),
            streaming: false,
        });
        let fallback = render_conversation(&app);
        assert!(!fallback.contains("history echoed input and 7"));
        assert!(fallback.contains("assistant"));
    }

    #[test]
    fn turn_handoff_projects_body_collisions_without_destroying_fixed_markdown() {
        let tmpdir = TempDir::new().expect("tempdir");
        let app = test_app(&tmpdir);
        app.sensitive_user_input_provenance.extend([
            "Input".to_string(),
            "ID".to_string(),
            "Name".to_string(),
            "Tool call".to_string(),
            "Intent".to_string(),
            "Result".to_string(),
            "Status".to_string(),
        ]);
        let raw = "# Turn handoff — turn_123\n\
_Status: completed · generated 2026-07-20 12:00:00_\n\
\n\
## Intent\n\
Input ID Name Tool call\n\
\n\
## Result / status\n\
- Result: Input ID Name Tool call\n\
- Status: completed\n\
Input:\n\
Structured result blocks:\n";

        let projected = sanitize_turn_handoff(&app, raw);

        for structural in [
            "# Turn handoff — turn_123",
            "_Status: completed · generated 2026-07-20 12:00:00_",
            "## Intent",
            "## Result / status",
            "- Result:",
            "- Status: completed",
        ] {
            assert!(
                projected.contains(structural),
                "missing {structural}: {projected}"
            );
        }
        assert!(
            !projected.contains("Input ID Name Tool call"),
            "{projected}"
        );
    }

    #[test]
    fn turn_handoff_does_not_treat_user_plan_text_before_colon_as_structure() {
        const SECRET: &str = "482915";
        let tmpdir = TempDir::new().expect("tempdir");
        let app = test_app(&tmpdir);
        app.sensitive_user_input_provenance
            .extend([SECRET.to_string()]);
        let raw = "# Turn handoff\n\
                   _Status: completed · generated 2026-07-20 12:00:00_\n\
                   \n\
                   ## Strategy / To-do\n\
                   - [ ] call 482915: then verify\n\
                   - Strategy: preserve fixed label while removing 482915\n";

        let projected = sanitize_turn_handoff(&app, raw);

        assert!(!projected.contains(SECRET), "{projected}");
        assert!(projected.contains("- [ ] call [redacted user input]: then verify"));
        assert!(projected.contains("- Strategy: preserve fixed label"));
    }

    #[test]
    fn clipboard_export_reports_ssh_terminal_client_and_failure_honestly() {
        let tmpdir = TempDir::new().expect("tempdir");
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

        app.clipboard = ClipboardHandler::unavailable_for_test(false);
        let failed = execute_export(&mut app, Some("clipboard"));
        assert!(failed.is_error);
        let message = failed.message.as_deref().unwrap_or_default();
        assert!(message.contains("No file was written"), "{message}");
        assert!(message.contains("/export file <path>"), "{message}");
        assert!(!tmpdir.path().join("chat_export.md").exists());
    }

    #[test]
    fn file_export_is_workspace_relative_private_and_no_overwrite_by_default() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir);
        app.api_messages.push(Message {
            role: "user".to_string(),
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
}
