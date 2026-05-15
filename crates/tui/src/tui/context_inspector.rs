//! Compact session context inspector.

use std::collections::HashSet;
use std::fmt::Write;

use crate::compaction::estimate_input_tokens_conservative;
use crate::models::{
    LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS, SystemPrompt, context_window_for_model,
};
use crate::session_manager::SessionContextReference;
use crate::tui::app::{App, ToolDetailRecord};
use crate::tui::file_mention::ContextReferenceSource;
use crate::utils::estimate_message_chars;

/// Marker used by per-turn working-set metadata. Replicated here so the
/// context inspector can distinguish stable prompt blocks from volatile
/// working-set context without importing engine internals.
const WORKING_SET_MARKER: &str = "## Repo Working Set";

const CONTEXT_WARNING_THRESHOLD_PERCENT: f64 = 85.0;
const CONTEXT_CRITICAL_THRESHOLD_PERCENT: f64 = 95.0;
const MAX_REFERENCE_ROWS: usize = 12;
const MAX_TOOL_ROWS: usize = 8;

const SYSTEM_LAYER_MARKERS: &[(&str, &str, PromptLayerKind)] = &[
    (
        "Project context",
        "<project_instructions",
        PromptLayerKind::Static,
    ),
    (
        "Project context pack",
        "## Project Context Pack",
        PromptLayerKind::Static,
    ),
    ("Environment", "## Environment", PromptLayerKind::Static),
    ("Skills", "## Skills", PromptLayerKind::Static),
    (
        "Context management",
        "## Context Management",
        PromptLayerKind::Static,
    ),
    ("Compact template", "## Compact", PromptLayerKind::Static),
    (
        "Configured instructions",
        "<instructions ",
        PromptLayerKind::Dynamic,
    ),
    ("User memory", "## User Memory", PromptLayerKind::Dynamic),
    (
        "Current session goal",
        "## Current Session Goal",
        PromptLayerKind::Dynamic,
    ),
    (
        "Previous session relay",
        "## Previous Session Relay",
        PromptLayerKind::Dynamic,
    ),
    (
        "Volatile working set",
        WORKING_SET_MARKER,
        PromptLayerKind::Dynamic,
    ),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PromptLayerKind {
    Static,
    Dynamic,
}

impl PromptLayerKind {
    fn label(self) -> &'static str {
        match self {
            Self::Static => "cache-friendly",
            Self::Dynamic => "changes by session/turn",
        }
    }
}

#[derive(Debug)]
struct PromptTextLayer<'a> {
    name: &'static str,
    kind: PromptLayerKind,
    body: &'a str,
}

#[must_use]
pub fn build_context_inspector_text(app: &App) -> String {
    let mut out = String::new();
    let usage = context_usage(app);
    let status = context_status(usage.2);

    let _ = writeln!(out, "Session Context");
    let _ = writeln!(out, "---------------");
    let _ = writeln!(out, "Model: {}", app.model);
    let _ = writeln!(
        out,
        "Workspace: {}",
        crate::utils::display_path(&app.workspace)
    );
    if let Some(session_id) = app.current_session_id.as_deref() {
        let _ = writeln!(out, "Session: {}", session_id);
    }
    let (used, max, percent) = usage;
    let _ = writeln!(
        out,
        "Context: {status} - ~{used}/{max} tokens ({percent:.1}%)"
    );
    let _ = writeln!(
        out,
        "Transcript: {} cells, {} API messages",
        app.history.len(),
        app.api_messages.len()
    );
    let _ = writeln!(
        out,
        "Workspace status: {}",
        app.workspace_context
            .as_deref()
            .unwrap_or("not sampled yet")
    );

    let _ = writeln!(out);
    push_system_prompt_structure(&mut out, app);
    let _ = writeln!(out);
    push_references(&mut out, &app.session_context_references);
    let _ = writeln!(out);
    push_tools(&mut out, app);

    out
}

fn context_usage(app: &App) -> (usize, u32, f64) {
    let max = context_window_for_model(&app.model).unwrap_or(LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS);
    let estimated =
        estimate_input_tokens_conservative(&app.api_messages, app.system_prompt.as_ref());
    let total_chars = estimate_message_chars(&app.api_messages);
    let used = estimated.max(total_chars / 4);
    let percent = ((used as f64 / f64::from(max)) * 100.0).clamp(0.0, 100.0);
    (used, max, percent)
}

fn context_status(percent: f64) -> &'static str {
    if percent >= CONTEXT_CRITICAL_THRESHOLD_PERCENT {
        "critical"
    } else if percent >= CONTEXT_WARNING_THRESHOLD_PERCENT {
        "high"
    } else {
        "ok"
    }
}

/// Inspect the system prompt structure, split into cache-friendly stable
/// prefix blocks and the volatile working-set tail block.
fn push_system_prompt_structure(out: &mut String, app: &App) {
    let _ = writeln!(out, "System Prompt Structure");
    let _ = writeln!(out, "-----------------------");

    // Conservative token estimate: ~3 chars per token (consistent with
    // compaction.rs internal helpers — replicated here to avoid depending
    // on a private function).
    let text_tokens = |text: &str| text.chars().count().div_ceil(3);

    let total_est = match &app.system_prompt {
        Some(SystemPrompt::Text(t)) => text_tokens(t),
        Some(SystemPrompt::Blocks(blocks)) => blocks.iter().map(|b| text_tokens(&b.text)).sum(),
        None => 0,
    };

    match &app.system_prompt {
        Some(SystemPrompt::Blocks(blocks)) => {
            let working_set_idx = blocks
                .iter()
                .position(|b| b.text.contains(WORKING_SET_MARKER));
            let (stable_count, working_block) = match working_set_idx {
                Some(idx) => (idx, Some(&blocks[idx])),
                None => (blocks.len(), None),
            };

            let stable_tokens: usize = blocks
                .iter()
                .take(stable_count)
                .map(|b| text_tokens(&b.text))
                .sum();
            let working_tokens = working_block.map(|b| text_tokens(&b.text)).unwrap_or(0);

            let _ = writeln!(
                out,
                "  Stable prefix: {stable_count} block(s), ~{stable_tokens} tokens  [cache-friendly]"
            );
            if let Some(block) = working_block {
                let _ = writeln!(
                    out,
                    "  Volatile working set: 1 block, ~{working_tokens} tokens  [changes every turn]"
                );
                let _ = writeln!(
                    out,
                    "    First line: {}",
                    block.text.lines().next().unwrap_or("(empty)")
                );
            } else {
                let _ = writeln!(out, "  Volatile working set: none");
            }
            let _ = writeln!(
                out,
                "  Total: {} block(s), ~{total_est} tokens",
                blocks.len()
            );
        }
        Some(SystemPrompt::Text(text)) => {
            let layers = split_text_prompt_layers(text);
            if layers.len() > 1
                || layers
                    .first()
                    .is_some_and(|layer| layer.name != "System prompt")
            {
                let _ = writeln!(
                    out,
                    "  Text prompt layers: {} layer(s), ~{total_est} tokens",
                    layers.len()
                );
                for layer in layers {
                    let tokens = text_tokens(layer.body);
                    let _ = writeln!(
                        out,
                        "  - {}: ~{} tokens [{}]",
                        layer.name,
                        tokens,
                        layer.kind.label()
                    );
                }
            } else {
                let _ = writeln!(
                    out,
                    "  Single text blob (~{total_est} tokens) [stable prefix only]"
                );
            }
        }
        None => {
            let _ = writeln!(out, "  No system prompt set.");
        }
    }

    // Cache-economics hint
    let _ = writeln!(
        out,
        "  Tip: Stable prefix blocks are DeepSeek V4 prefix-cache eligible. \
        Volatile working-set changes break the cache only for the tail."
    );
}

fn split_text_prompt_layers(text: &str) -> Vec<PromptTextLayer<'_>> {
    let mut starts = SYSTEM_LAYER_MARKERS
        .iter()
        .filter_map(|(name, marker, kind)| text.find(marker).map(|idx| (idx, *name, *kind)))
        .collect::<Vec<_>>();
    starts.sort_by_key(|(idx, _, _)| *idx);

    let Some((first_idx, _, _)) = starts.first().copied() else {
        return vec![PromptTextLayer {
            name: "System prompt",
            kind: PromptLayerKind::Static,
            body: text.trim(),
        }];
    };

    let mut layers = Vec::new();
    if first_idx > 0 {
        layers.push(PromptTextLayer {
            name: "Global system prefix",
            kind: PromptLayerKind::Static,
            body: text[..first_idx].trim(),
        });
    }

    for (i, (start, name, kind)) in starts.iter().enumerate() {
        let end = starts.get(i + 1).map_or(text.len(), |(idx, _, _)| *idx);
        layers.push(PromptTextLayer {
            name,
            kind: *kind,
            body: text[*start..end].trim(),
        });
    }

    layers
}

fn push_references(out: &mut String, references: &[SessionContextReference]) {
    let _ = writeln!(out, "References");
    let _ = writeln!(out, "----------");

    let mut seen = HashSet::new();
    let mut rendered = 0usize;
    for record in references {
        let reference = &record.reference;
        let key = format!(
            "{:?}:{:?}:{}:{}",
            reference.source, reference.kind, reference.target, reference.label
        );
        if !seen.insert(key) {
            continue;
        }
        if rendered >= MAX_REFERENCE_ROWS {
            let remaining = references.len().saturating_sub(rendered);
            if remaining > 0 {
                let _ = writeln!(out, "- ... {remaining} more reference(s)");
            }
            break;
        }

        let prefix = match reference.source {
            ContextReferenceSource::AtMention => "@",
            ContextReferenceSource::Attachment => "/attach ",
        };
        let state = if reference.included {
            if reference.expanded {
                "included"
            } else {
                "attached"
            }
        } else {
            "not included"
        };
        let detail = reference
            .detail
            .as_deref()
            .filter(|detail| !detail.trim().is_empty())
            .map(|detail| format!(" - {detail}"))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "- [{}] {prefix}{} -> {} ({state}{detail})",
            reference.badge, reference.label, reference.target
        );
        rendered += 1;
    }

    if rendered == 0 {
        let _ = writeln!(
            out,
            "- No file, directory, or media references recorded yet."
        );
    }
}

fn push_tools(out: &mut String, app: &App) {
    let _ = writeln!(out, "Recent Tools");
    let _ = writeln!(out, "------------");

    let mut rows: Vec<(usize, &ToolDetailRecord)> = app
        .tool_details_by_cell
        .iter()
        .map(|(idx, detail)| (*idx, detail))
        .collect();
    rows.sort_by_key(|(idx, _)| std::cmp::Reverse(*idx));

    let mut rendered = 0usize;
    for detail in app.active_tool_details.values() {
        push_tool_row(out, "active", detail);
        rendered += 1;
        if rendered >= MAX_TOOL_ROWS {
            return;
        }
    }
    for (cell_idx, detail) in rows
        .into_iter()
        .take(MAX_TOOL_ROWS.saturating_sub(rendered))
    {
        let location = format!("cell {cell_idx}");
        push_tool_row(out, &location, detail);
        rendered += 1;
    }

    if rendered == 0 {
        let _ = writeln!(out, "- No tool activity recorded yet.");
    } else {
        let _ = writeln!(
            out,
            "- Open the matching card and press Alt+V for full details."
        );
    }
}

fn push_tool_row(out: &mut String, location: &str, detail: &ToolDetailRecord) {
    let output_state = if detail.output.as_deref().is_some_and(|out| !out.is_empty()) {
        "output captured"
    } else {
        "no output yet"
    };
    let _ = writeln!(
        out,
        "- [{}] {} {} ({output_state})",
        location,
        detail.tool_name,
        short_tool_id(&detail.tool_id)
    );
}

fn short_tool_id(id: &str) -> String {
    if id.len() <= 8 {
        id.to_string()
    } else {
        format!("{}...", &id[..8])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::models::{ContentBlock, Message};
    use crate::session_manager::SessionContextReference;
    use crate::tui::app::TuiOptions;
    use crate::tui::file_mention::{
        ContextReference, ContextReferenceKind, ContextReferenceSource,
    };
    use crate::tui::history::HistoryCell;
    use std::path::PathBuf;

    fn test_app() -> App {
        App::new(
            TuiOptions {
                model: "unknown-model".to_string(),
                workspace: PathBuf::from("/tmp/project"),
                config_path: None,
                config_profile: None,
                allow_shell: false,
                use_alt_screen: true,
                use_mouse_capture: false,
                use_bracketed_paste: true,
                max_subagents: 1,
                skills_dir: PathBuf::from("/tmp/skills"),
                memory_path: PathBuf::from("memory.md"),
                notes_path: PathBuf::from("notes.md"),
                mcp_config_path: PathBuf::from("mcp.json"),
                use_memory: false,
                start_in_agent_mode: false,
                skip_onboarding: true,
                yolo: false,
                resume_session_id: None,
                initial_input: None,
            },
            &Config::default(),
        )
    }

    #[test]
    fn inspector_formats_empty_state() {
        let app = test_app();
        let text = build_context_inspector_text(&app);
        assert!(text.contains("Session Context"));
        assert!(text.contains("No file, directory, or media references recorded yet."));
        assert!(text.contains("No tool activity recorded yet."));
    }

    #[test]
    fn inspector_lists_context_references() {
        let mut app = test_app();
        app.history.push(HistoryCell::User {
            content: "read @src/main.rs".to_string(),
        });
        app.session_context_references
            .push(SessionContextReference {
                message_index: 0,
                reference: ContextReference {
                    kind: ContextReferenceKind::File,
                    source: ContextReferenceSource::AtMention,
                    badge: "file".to_string(),
                    label: "src/main.rs".to_string(),
                    target: "/tmp/project/src/main.rs".to_string(),
                    included: true,
                    expanded: true,
                    detail: Some("included".to_string()),
                },
            });

        let text = build_context_inspector_text(&app);
        assert!(text.contains("[file] @src/main.rs -> /tmp/project/src/main.rs"));
    }

    #[test]
    fn inspector_marks_high_context_pressure() {
        let mut app = test_app();
        app.api_messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "x".repeat(4_000_000),
                cache_control: None,
            }],
        });

        let text = build_context_inspector_text(&app);
        assert!(text.contains("Context: critical"), "{text}");
    }

    #[test]
    fn inspector_no_system_prompt_shows_section() {
        let app = test_app();
        let text = build_context_inspector_text(&app);
        assert!(text.contains("System Prompt Structure"));
        assert!(text.contains("No system prompt set."));
    }

    #[test]
    fn inspector_blocks_format_shows_stable_prefix_and_working_set() {
        let mut app = test_app();
        use crate::models::SystemBlock;
        app.system_prompt = Some(SystemPrompt::Blocks(vec![
            SystemBlock {
                block_type: "text".to_string(),
                text: "## Stable Base\n\nYou are DeepSeek TUI.".to_string(),
                cache_control: None,
            },
            SystemBlock {
                block_type: "text".to_string(),
                text: format!("{WORKING_SET_MARKER}\nsrc/main.rs changed"),
                cache_control: None,
            },
        ]));

        let text = build_context_inspector_text(&app);
        assert!(text.contains("System Prompt Structure"));
        assert!(
            text.contains("Stable prefix: 1 block"),
            "stable prefix count: {text}"
        );
        assert!(
            text.contains("Volatile working set: 1 block"),
            "working set section: {text}"
        );
        assert!(
            text.contains("[cache-friendly]"),
            "cache hint for stable: {text}"
        );
        assert!(
            text.contains("[changes every turn]"),
            "volatile marker: {text}"
        );
        assert!(
            text.contains("First line: ## Repo Working Set"),
            "first line of working set: {text}"
        );
    }

    #[test]
    fn inspector_blocks_without_working_set_shows_stable_only() {
        let mut app = test_app();
        use crate::models::SystemBlock;
        app.system_prompt = Some(SystemPrompt::Blocks(vec![
            SystemBlock {
                block_type: "text".to_string(),
                text: "## Stable Base".to_string(),
                cache_control: None,
            },
            SystemBlock {
                block_type: "text".to_string(),
                text: "## Personality\nCalm".to_string(),
                cache_control: None,
            },
        ]));

        let text = build_context_inspector_text(&app);
        assert!(text.contains("Stable prefix: 2 block(s)"));
        assert!(text.contains("Volatile working set: none"));
    }

    #[test]
    fn inspector_text_prompt_shows_layer_map() {
        let mut app = test_app();
        app.system_prompt = Some(SystemPrompt::Text(
            "You are DeepSeek TUI.\n\n<project_instructions source=\"AGENTS.md\">\nRules\n</project_instructions>\n\n## Project Context Pack\n{}\n\n## Environment\n- lang: en\n\n## Skills\n- rust\n\n## Context Management\nKeep compact\n\n## Compact\nTemplate\n\n## Repo Working Set\nsrc/".to_string(),
        ));

        let text = build_context_inspector_text(&app);
        assert!(text.contains("System Prompt Structure"));
        assert!(text.contains("Text prompt layers"));
        assert!(text.contains("Global system prefix"));
        assert!(text.contains("Project context"));
        assert!(text.contains("Project context pack"));
        assert!(text.contains("Environment"));
        assert!(text.contains("Skills"));
        assert!(text.contains("Context management"));
        assert!(text.contains("Compact template"));
        assert!(text.contains("Volatile working set"));
        assert!(text.contains("changes by session/turn"));
    }

    #[test]
    fn inspector_text_prompt_without_markers_shows_single_blob() {
        let mut app = test_app();
        app.system_prompt = Some(SystemPrompt::Text("You are DeepSeek TUI.".to_string()));

        let text = build_context_inspector_text(&app);
        assert!(text.contains("Single text blob"));
        assert!(text.contains("stable prefix only"));
    }
}
