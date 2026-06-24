//! Compact session context inspector.

use std::collections::HashSet;
use std::fmt::Write;

use crate::compaction::estimate_input_tokens_conservative;
use crate::localization::{Locale, MessageId, tr};
use crate::models::SystemPrompt;
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
    fn label(self, locale: Locale) -> &'static str {
        match self {
            Self::Static => tr(locale, MessageId::CtxInspCacheFriendly),
            Self::Dynamic => tr(locale, MessageId::CtxInspChangesByTurn),
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
pub fn build_context_inspector_text(app: &App, locale: Locale) -> String {
    let mut out = String::new();
    let usage = context_usage(app);
    let (used, max, percent) = usage;

    let _ = writeln!(out, "{}", tr(locale, MessageId::CtxInspSessionContext));
    let _ = writeln!(out, "---------------");
    let _ = writeln!(
        out,
        "{}: {}",
        tr(locale, MessageId::CtxInspModel),
        app.model
    );
    let _ = writeln!(
        out,
        "{}: {}",
        tr(locale, MessageId::CtxInspWorkspace),
        crate::utils::display_path(&app.workspace)
    );
    if let Some(session_id) = app.current_session_id.as_deref() {
        let _ = writeln!(
            out,
            "{}: {}",
            tr(locale, MessageId::CtxInspSession),
            crate::session_manager::truncate_id(session_id)
        );
    }
    let status_label = match context_status(percent) {
        ContextPressure::Critical => tr(locale, MessageId::CtxInspCritical),
        ContextPressure::High => tr(locale, MessageId::CtxInspHigh),
        ContextPressure::Ok => tr(locale, MessageId::CtxInspOk),
    };
    let tokens_unit = tr(locale, MessageId::CtxInspTokens);
    let _ = writeln!(
        out,
        "{ctx_label}: {status_label} - ~{used}/{max} {tokens_unit} ({percent:.1}%)",
        ctx_label = tr(locale, MessageId::CtxInspContext),
    );
    let cells = tr(locale, MessageId::CtxInspCells);
    let api_msgs = tr(locale, MessageId::CtxInspApiMessages);
    let _ = writeln!(
        out,
        "{label}: {} {cells}, {} {api_msgs}",
        app.history.len(),
        app.api_messages.len(),
        label = tr(locale, MessageId::CtxInspTranscript),
    );
    let _ = writeln!(
        out,
        "{}: {}",
        tr(locale, MessageId::CtxInspWorkspaceStatus),
        app.workspace_context
            .as_deref()
            .unwrap_or(tr(locale, MessageId::CtxInspNotSampledYet))
    );

    let _ = writeln!(out);
    push_system_prompt_structure(&mut out, app, locale);
    let _ = writeln!(out);
    push_references(&mut out, &app.session_context_references, locale);
    let _ = writeln!(out);
    push_tools(&mut out, app, locale);

    out
}

fn context_usage(app: &App) -> (usize, u32, f64) {
    let max = crate::route_budget::route_context_window_tokens(
        app.api_provider,
        app.effective_model_for_budget(),
        app.active_route_limits,
    );
    let estimated =
        estimate_input_tokens_conservative(&app.api_messages, app.system_prompt.as_ref());
    let total_chars = estimate_message_chars(&app.api_messages);
    let used = estimated.max(total_chars / 4);
    let percent = ((used as f64 / f64::from(max)) * 100.0).clamp(0.0, 100.0);
    (used, max, percent)
}

enum ContextPressure {
    Ok,
    High,
    Critical,
}

fn context_status(percent: f64) -> ContextPressure {
    if percent >= CONTEXT_CRITICAL_THRESHOLD_PERCENT {
        ContextPressure::Critical
    } else if percent >= CONTEXT_WARNING_THRESHOLD_PERCENT {
        ContextPressure::High
    } else {
        ContextPressure::Ok
    }
}

/// Inspect the system prompt structure, split into cache-friendly stable
/// prefix blocks and the volatile working-set tail block.
fn push_system_prompt_structure(out: &mut String, app: &App, locale: Locale) {
    let _ = writeln!(out, "{}", tr(locale, MessageId::CtxInspSystemPrompt));
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

    let stable_lbl = tr(locale, MessageId::CtxInspStablePrefix);
    let volatile_lbl = tr(locale, MessageId::CtxInspVolatileWorkingSet);
    let first_line_lbl = tr(locale, MessageId::CtxInspFirstLine);
    let total_lbl = tr(locale, MessageId::CtxInspTotal);
    let text_prompt_lbl = tr(locale, MessageId::CtxInspTextPromptLayers);
    let single_blob_lbl = tr(locale, MessageId::CtxInspSingleTextBlob);
    let blocks_unit = tr(locale, MessageId::CtxInspBlocks);
    let block_unit = tr(locale, MessageId::CtxInspBlock);
    let tokens_unit = tr(locale, MessageId::CtxInspTokens);
    let layers_unit = tr(locale, MessageId::CtxInspLayers);
    let none_lbl = tr(locale, MessageId::CtxInspNone);
    let empty_lbl = tr(locale, MessageId::CtxInspEmpty);
    let cache_friendly = tr(locale, MessageId::CtxInspCacheFriendly);
    let changes_by_turn = tr(locale, MessageId::CtxInspChangesByTurn);
    let stable_only = tr(locale, MessageId::CtxInspStablePrefixOnly);
    let no_system_prompt = tr(locale, MessageId::CtxInspNoSystemPrompt);
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
                "  {stable_lbl}: {stable_count} {blocks_unit}, ~{stable_tokens} {tokens_unit}  [{cache_friendly}]"
            );
            if let Some(block) = working_block {
                let _ = writeln!(
                    out,
                    "  {volatile_lbl}: 1 {block_unit}, ~{working_tokens} {tokens_unit}  [{changes_by_turn}]"
                );
                let _ = writeln!(
                    out,
                    "    {first_line_lbl}: {}",
                    block.text.lines().next().unwrap_or(empty_lbl)
                );
            } else {
                let _ = writeln!(out, "  {volatile_lbl}: {none_lbl}");
            }
            let _ = writeln!(
                out,
                "  {total_lbl}: {} {blocks_unit}, ~{total_est} {tokens_unit}",
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
                    "  {text_prompt_lbl}: {} {layers_unit}, ~{total_est} {tokens_unit}",
                    layers.len()
                );
                for layer in layers {
                    let tokens = text_tokens(layer.body);
                    let kind_lbl = layer.kind.label(locale);
                    let _ = writeln!(
                        out,
                        "  - {}: ~{tokens} {tokens_unit} [{kind_lbl}]",
                        layer.name,
                    );
                }
            } else {
                let _ = writeln!(
                    out,
                    "  {single_blob_lbl} (~{total_est} {tokens_unit}) [{stable_only}]"
                );
            }
        }
        None => {
            let _ = writeln!(out, "  {no_system_prompt}");
        }
    }

    // Cache-economics hint
    let _ = writeln!(out, "  {}", tr(locale, MessageId::CtxInspCacheTip));
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

fn push_references(out: &mut String, references: &[SessionContextReference], locale: Locale) {
    let _ = writeln!(out, "{}", tr(locale, MessageId::CtxInspReferences));
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
                let _ = writeln!(
                    out,
                    "- ... {remaining} {}",
                    tr(locale, MessageId::CtxInspMoreReferences)
                );
            }
            break;
        }

        let prefix = match reference.source {
            ContextReferenceSource::AtMention => "@",
            ContextReferenceSource::Attachment => "/attach ",
        };
        let state = if reference.included {
            if reference.expanded {
                tr(locale, MessageId::CtxInspIncluded)
            } else {
                tr(locale, MessageId::CtxInspAttached)
            }
        } else {
            tr(locale, MessageId::CtxInspNotIncluded)
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
        let _ = writeln!(out, "- {}", tr(locale, MessageId::CtxInspNoReferences));
    }
}

fn push_tools(out: &mut String, app: &App, locale: Locale) {
    let _ = writeln!(out, "{}", tr(locale, MessageId::CtxInspRecentTools));
    let _ = writeln!(out, "------------");

    let mut rows: Vec<(usize, &ToolDetailRecord)> = app
        .tool_details_by_cell
        .iter()
        .map(|(idx, detail)| (*idx, detail))
        .collect();
    rows.sort_by_key(|(idx, _)| std::cmp::Reverse(*idx));

    let mut rendered = 0usize;
    for detail in app.active_tool_details.values() {
        let location = tr(locale, MessageId::CtxInspActive);
        push_tool_row(out, locale, location, detail);
        rendered += 1;
        if rendered >= MAX_TOOL_ROWS {
            return;
        }
    }
    for (cell_idx, detail) in rows
        .into_iter()
        .take(MAX_TOOL_ROWS.saturating_sub(rendered))
    {
        let location = format!("{} {cell_idx}", tr(locale, MessageId::CtxInspCell));
        push_tool_row(out, locale, &location, detail);
        rendered += 1;
    }

    if rendered == 0 {
        let _ = writeln!(out, "- {}", tr(locale, MessageId::CtxInspNoToolActivity));
    } else {
        let _ = writeln!(out, "- {}", tr(locale, MessageId::CtxInspAltVHint));
    }
}

fn push_tool_row(out: &mut String, locale: Locale, location: &str, detail: &ToolDetailRecord) {
    let output_state = if detail.output.as_deref().is_some_and(|out| !out.is_empty()) {
        tr(locale, MessageId::CtxInspOutputCaptured)
    } else {
        tr(locale, MessageId::CtxInspNoOutputYet)
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

    use crate::localization::Locale;

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
        let text = build_context_inspector_text(&app, Locale::En);
        assert!(text.contains("Session Context"));
        assert!(text.contains("No file, directory, or media references recorded yet."));
        assert!(text.contains("No tool activity recorded yet."));
    }

    #[test]
    fn inspector_uses_compact_session_id() {
        let mut app = test_app();
        app.current_session_id = Some("1234567890abcdef".to_string());

        let text = build_context_inspector_text(&app, Locale::En);

        assert!(text.contains("Session: 12345678"), "{text}");
        assert!(!text.contains("1234567890abcdef"), "{text}");
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

        let text = build_context_inspector_text(&app, Locale::En);
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

        let text = build_context_inspector_text(&app, Locale::En);
        assert!(text.contains("Context: critical"), "{text}");
    }

    #[test]
    fn inspector_uses_effective_auto_model_context_window() {
        let mut app = test_app();
        app.model = "auto".to_string();
        app.auto_model = true;
        app.last_effective_model = Some("deepseek-v4-pro".to_string());

        let text = build_context_inspector_text(&app, Locale::En);
        assert!(text.contains("Model: auto"), "{text}");
        assert!(text.contains("/1000000 tokens"), "{text}");
    }

    #[test]
    fn inspector_no_system_prompt_shows_section() {
        let app = test_app();
        let text = build_context_inspector_text(&app, Locale::En);
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
                text: "## Stable Base\n\nYou are CodeWhale.".to_string(),
                cache_control: None,
            },
            SystemBlock {
                block_type: "text".to_string(),
                text: format!("{WORKING_SET_MARKER}\nsrc/main.rs changed"),
                cache_control: None,
            },
        ]));

        let text = build_context_inspector_text(&app, Locale::En);
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
            text.contains("[changes by session/turn]"),
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

        let text = build_context_inspector_text(&app, Locale::En);
        assert!(text.contains("Stable prefix: 2 block(s)"));
        assert!(text.contains("Volatile working set: none"));
    }

    #[test]
    fn inspector_text_prompt_shows_layer_map() {
        let mut app = test_app();
        app.system_prompt = Some(SystemPrompt::Text(
            "You are CodeWhale.\n\n<project_instructions source=\"AGENTS.md\">\nRules\n</project_instructions>\n\n## Project Context Pack\n{}\n\n## Environment\n- lang: en\n\n## Skills\n- rust\n\n## Context Management\nKeep compact\n\n## Compact\nTemplate\n\n## Repo Working Set\nsrc/".to_string(),
        ));

        let text = build_context_inspector_text(&app, Locale::En);
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
        app.system_prompt = Some(SystemPrompt::Text("You are CodeWhale.".to_string()));

        let text = build_context_inspector_text(&app, Locale::En);
        assert!(text.contains("Single text blob"));
        assert!(text.contains("stable prefix only"));
    }

    #[test]
    fn inspector_localizes_to_zh_hans() {
        use crate::models::SystemBlock;
        let mut app = test_app();
        app.system_prompt = Some(SystemPrompt::Blocks(vec![
            SystemBlock {
                block_type: "text".to_string(),
                text: "## Base\nYou are CodeWhale.".to_string(),
                cache_control: None,
            },
            SystemBlock {
                block_type: "text".to_string(),
                text: format!("{WORKING_SET_MARKER}\nsrc/main.rs changed"),
                cache_control: None,
            },
        ]));
        let text = build_context_inspector_text(&app, Locale::ZhHans);

        // Positive: key ZhHans labels present
        assert!(text.contains("会话上下文"), "session header: {text}");
        assert!(text.contains("模型"), "model label: {text}");
        assert!(text.contains("工作区"), "workspace: {text}");
        assert!(text.contains("系统提示结构"), "sysprompt section: {text}");
        assert!(text.contains("稳定前缀"), "stable prefix: {text}");
        assert!(text.contains("易变工作集"), "volatile ws: {text}");
        assert!(text.contains("第一行"), "first line: {text}");
        assert!(text.contains("总计"), "total line: {text}");
        assert!(text.contains("引用"), "references: {text}");
        assert!(text.contains("最近使用的工具"), "tools: {text}");
        assert!(text.contains("个区块"), "blocks unit: {text}");
        assert!(text.contains("个 token"), "tokens unit: {text}");
        assert!(text.contains("缓存友好"), "cache-friendly: {text}");
        assert!(text.contains("提示"), "cache tip: {text}");

        // Negative: no English labels leak
        assert!(!text.contains("Session Context"), "EN session leaked");
        assert!(!text.contains("Model:"), "EN model leaked");
        assert!(!text.contains("cells"), "EN cells leaked");
        assert!(!text.contains("API messages"), "EN API msgs leaked");
        assert!(!text.contains("Stable prefix"), "EN stable prefix leaked");
        assert!(
            !text.contains("Volatile working set"),
            "EN volatile ws leaked"
        );
        assert!(!text.contains("First line"), "EN first line leaked");
        assert!(!text.contains("Total:"), "EN total leaked");
        assert!(!text.contains("Text prompt layers"), "EN layers leaked");
        assert!(!text.contains("cache-friendly"), "EN cache-friendly leaked");
        assert!(!text.contains("more reference"), "EN more refs leaked");
        assert!(!text.contains("no output yet"), "EN no output leaked");
    }
}
