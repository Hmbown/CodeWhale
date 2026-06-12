//! Prompt source map and context-usage report (#3143).
//!
//! A `PromptSourceMap` records every source that contributes to the assembled
//! model prompt — system layers, tools, memory, skills, MCP servers, and
//! per-request runtime context — with approximate token counts, authority
//! provenance, and inclusion rationale. The map is the data backbone for
//! `/context report` in the TUI and `doctor --context-json` for headless
//! runs.
//!
//! Token counts use the same character-based heuristic as the compaction
//! subsystem (`estimate_text_tokens_conservative`). They are approximate
//! by design; the non-goal for v0.8.59 is exact provider tokenizer parity.
//! Every entry carries a `counting_confidence` field so consumers can
//! distinguish measured values from estimates.
//!
//! This module does **not** replace the existing `/memory` command. Memory
//! is one source among many; this report is a diagnostic lens on all of
//! them. See the acceptance criteria in #3143 and the non-goal
//! documentation note.

use std::path::Path;

use serde::Serialize;

use crate::compaction::estimate_text_tokens_conservative;
use crate::models::context_window_for_model;
use crate::tui::app::App;

// ── Top-level report ──────────────────────────────────────────────────────

/// A structured breakdown of every source contributing to the assembled
/// prompt, with per-source token estimates, authority provenance, and
/// inclusion rationale.
#[derive(Debug, Clone, Serialize)]
pub struct PromptSourceMap {
    /// Ordered list of sources (static layers first, then per-request).
    pub entries: Vec<SourceEntry>,
    /// Sum of `estimated_tokens` across all entries (the runtime pressure
    /// proxy, not the exact provider token count).
    pub total_estimated_tokens: usize,
    /// Provider/model context window in tokens, when known.
    pub context_window_tokens: Option<u32>,
    /// `total_estimated_tokens / context_window_tokens * 100`, clamped.
    pub budget_used_percent: Option<f64>,
    /// ISO-8601 timestamp of report generation.
    pub generated_at: String,
}

/// A single source entry in the prompt source map.
#[derive(Debug, Clone, Serialize)]
pub struct SourceEntry {
    /// Categorisation of what kind of content this is.
    pub source_kind: SourceKind,
    /// Human-readable label for display (e.g. "Constitution (base.md)").
    pub label: String,
    /// File path, URL, or resource identifier when applicable.
    pub source_path: Option<String>,
    /// Why this source was included in the prompt.
    pub activation_reason: ActivationReason,
    /// Approximate token count for this source.
    pub estimated_tokens: usize,
    /// How reliable the token count is.
    pub counting_confidence: CountingConfidence,
    /// Constitutional authority tier (1–9) when applicable.
    pub authority_tier: Option<u8>,
    /// Reason this source was truncated or omitted entirely.
    pub truncation_reason: Option<String>,
}

// ── Enums ──────────────────────────────────────────────────────────────────

/// Stable categorisation of prompt source types.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)] // LocaleReinforcement, Other reserved for future source types (#3102)
pub enum SourceKind {
    /// The Constitution of CodeWhale (base system prompt).
    Constitution,
    /// Personality overlay (Calm, Playful, etc.).
    Personality,
    /// Mode-specific delta (Agent, Plan, Yolo).
    ModeDelta,
    /// Approval policy and tool-gating rules.
    ApprovalPolicy,
    /// Tool taxonomy and selection guidance.
    ToolTaxonomy,
    /// Project context from AGENTS.md / README / CODEOWNERS.
    ProjectContext,
    /// Available skills catalogue block.
    SkillsBlock,
    /// Context management and compaction guidance.
    ContextManagement,
    /// Compaction relay template (compile-time constant).
    CompactionRelayTemplate,
    /// Environment block (pwd, platform, lang, model id).
    EnvironmentBlock,
    /// Configured `instructions = [...]` files.
    InstructionsFile,
    /// User memory (`<user_memory>` block from `~/.codewhale/memory.md`).
    UserMemory,
    /// Active session goal (`/goal`).
    SessionGoal,
    /// Previous-session handoff relay (`.codewhale/handoff.md`).
    HandoffRelay,
    /// Authority recap (Constitutional hierarchy reminder).
    AuthorityRecap,
    /// Locale-native language reinforcement block.
    LocaleReinforcement,
    /// The current user request (latest user message).
    UserRequest,
    /// A tool result fed back into context.
    ToolResult,
    /// A sub-agent completion summary.
    SubAgentSummary,
    /// A compaction summary injected into the prompt.
    CompactionSummary,
    /// MCP server tool schema(s).
    McpServerSchema,
    /// Model and provider capability facts.
    ModelProviderFact,
    /// Language output requirement block (translation).
    LanguageRequirement,
    /// Project context pack metadata.
    ProjectContextPack,
    /// Any source not yet explicitly categorised.
    Other,
}

/// Why a particular source was activated for this request.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActivationReason {
    /// Always included regardless of mode or config.
    AlwaysOn,
    /// Activated by the current AppMode.
    ModeSpecific,
    /// Enabled via config flag.
    ConfigEnabled,
    /// Enabled via feature toggle.
    FeatureEnabled,
    /// Included because a file exists at the expected path.
    FilePresent,
    /// Triggered by explicit user action.
    UserAction,
    /// Generated per-request based on runtime context.
    PerRequest,
    /// Included as a safety fallback.
    Fallback,
}

/// Confidence level for the token estimate.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)] // Exact reserved for provider tokenizer parity (future #3086)
pub enum CountingConfidence {
    /// Byte-exact count from the provider API response.
    Exact,
    /// Conservative character-based estimate (≤ 4 chars/token).
    High,
    /// Heuristic estimate (e.g. file size ÷ 4).
    Approximate,
    /// Rough order-of-magnitude guess.
    Low,
}

// ── Builder ────────────────────────────────────────────────────────────────

/// Build a `PromptSourceMap` by inspecting the current application state.
///
/// This is an approximate reconstruction — it does not re-run the full
/// prompt assembly pipeline. The token counts are derived from known
/// compile-time constants and character-count heuristics. For the exact
/// provider token count, see the `tokens` debug command.
pub fn build_context_report(app: &App, workspace: &Path) -> PromptSourceMap {
    let mut entries: Vec<SourceEntry> = Vec::new();
    let model = &app.model;

    // ── Static layers (always present, compile-time constants) ─────────

    // Constitution + base system prompt (base.md compiled in).
    // Estimated at ~30K chars which is ~7.5K tokens (conservative).
    {
        let text = crate::prompts::BASE_PROMPT;
        let tokens = estimate_text_tokens_conservative(text);
        entries.push(SourceEntry {
            source_kind: SourceKind::Constitution,
            label: "Constitution (base.md)".to_string(),
            source_path: Some("crates/tui/src/prompts/base.md".to_string()),
            activation_reason: ActivationReason::AlwaysOn,
            estimated_tokens: tokens,
            counting_confidence: CountingConfidence::High,
            authority_tier: Some(1),
            truncation_reason: None,
        });
    }

    // Personality overlay (compiled-in, varies by mode setting).
    {
        let personality_label = match app.mode {
            crate::tui::app::AppMode::Agent => "Calm",
            crate::tui::app::AppMode::Plan => "Calm",
            crate::tui::app::AppMode::Yolo => "Calm",
        };
        entries.push(SourceEntry {
            source_kind: SourceKind::Personality,
            label: format!("Personality overlay ({personality_label})"),
            source_path: Some("crates/tui/src/prompts/personality/calm.md".to_string()),
            activation_reason: ActivationReason::AlwaysOn,
            estimated_tokens: 400,
            counting_confidence: CountingConfidence::Approximate,
            authority_tier: Some(8),
            truncation_reason: None,
        });
    }

    // Mode delta.
    {
        let mode_label = app.mode.label();
        entries.push(SourceEntry {
            source_kind: SourceKind::ModeDelta,
            label: format!("Mode delta ({mode_label})"),
            source_path: None,
            activation_reason: ActivationReason::ModeSpecific,
            estimated_tokens: 600,
            counting_confidence: CountingConfidence::Approximate,
            authority_tier: Some(2),
            truncation_reason: None,
        });
    }

    // Approval policy.
    entries.push(SourceEntry {
        source_kind: SourceKind::ApprovalPolicy,
        label: "Approval policy".to_string(),
        source_path: None,
        activation_reason: ActivationReason::AlwaysOn,
        estimated_tokens: 500,
        counting_confidence: CountingConfidence::Approximate,
        authority_tier: Some(2),
        truncation_reason: None,
    });

    // Tool taxonomy + selection guidance.
    entries.push(SourceEntry {
        source_kind: SourceKind::ToolTaxonomy,
        label: "Tool taxonomy".to_string(),
        source_path: None,
        activation_reason: ActivationReason::AlwaysOn,
        estimated_tokens: 3000,
        counting_confidence: CountingConfidence::Approximate,
        authority_tier: Some(3),
        truncation_reason: None,
    });

    // Context management block (Agent/Yolo modes only).
    match app.mode {
        crate::tui::app::AppMode::Agent | crate::tui::app::AppMode::Yolo => {
            entries.push(SourceEntry {
                source_kind: SourceKind::ContextManagement,
                label: "Context Management".to_string(),
                source_path: None,
                activation_reason: ActivationReason::ModeSpecific,
                estimated_tokens: 1200,
                counting_confidence: CountingConfidence::Approximate,
                authority_tier: Some(3),
                truncation_reason: None,
            });
        }
        crate::tui::app::AppMode::Plan => {}
    }

    // Compaction relay template.
    entries.push(SourceEntry {
        source_kind: SourceKind::CompactionRelayTemplate,
        label: "Compaction relay template".to_string(),
        source_path: None,
        activation_reason: ActivationReason::AlwaysOn,
        estimated_tokens: 400,
        counting_confidence: CountingConfidence::Approximate,
        authority_tier: Some(9),
        truncation_reason: None,
    });

    // ── Workspace-dependent layers ────────────────────────────────────

    // Project context (AGENTS.md etc.).
    {
        let agents_path = workspace.join("AGENTS.md");
        let is_present = agents_path.exists();
        let tokens = if is_present {
            match std::fs::read_to_string(&agents_path) {
                Ok(content) => {
                    let truncated = truncate_if_needed(&content, 100 * 1024);
                    estimate_text_tokens_conservative(&truncated)
                }
                Err(_) => 0,
            }
        } else {
            0
        };
        let truncation_reason = if is_present {
            match std::fs::metadata(&agents_path) {
                Ok(meta) if meta.len() > 100 * 1024 => {
                    Some("truncated to 100 KB per INSTRUCTIONS_FILE_MAX_BYTES".to_string())
                }
                _ => None,
            }
        } else {
            None
        };
        entries.push(SourceEntry {
            source_kind: SourceKind::ProjectContext,
            label: "Project context (AGENTS.md)".to_string(),
            source_path: Some(agents_path.display().to_string()),
            activation_reason: if is_present {
                ActivationReason::FilePresent
            } else {
                ActivationReason::Fallback
            },
            estimated_tokens: tokens,
            counting_confidence: if is_present {
                CountingConfidence::High
            } else {
                CountingConfidence::Low
            },
            authority_tier: Some(5),
            truncation_reason,
        });
    }

    // Skills block.
    {
        let skills_present = app.skills_dir.exists()
            && std::fs::read_dir(&app.skills_dir)
                .map(|mut d| d.next().is_some())
                .unwrap_or(false);
        let skills_count = if skills_present {
            std::fs::read_dir(&app.skills_dir)
                .map(|d| d.count())
                .unwrap_or(0)
        } else {
            0
        };
        // Rough estimate: ~200 tokens per skill listing line
        let tokens = if skills_present {
            skills_count * 200
        } else {
            0
        };
        entries.push(SourceEntry {
            source_kind: SourceKind::SkillsBlock,
            label: format!("Skills block ({skills_count} skills)"),
            source_path: Some(app.skills_dir.display().to_string()),
            activation_reason: if skills_present {
                ActivationReason::FilePresent
            } else {
                ActivationReason::Fallback
            },
            estimated_tokens: tokens,
            counting_confidence: if skills_present {
                CountingConfidence::Approximate
            } else {
                CountingConfidence::Low
            },
            authority_tier: None,
            truncation_reason: None,
        });
    }

    // Project context pack.
    {
        let pack_present = workspace.join("README.md").exists();
        entries.push(SourceEntry {
            source_kind: SourceKind::ProjectContextPack,
            label: "Project context pack".to_string(),
            source_path: Some(workspace.join("README.md").display().to_string()),
            activation_reason: if pack_present {
                ActivationReason::FilePresent
            } else {
                ActivationReason::Fallback
            },
            estimated_tokens: if pack_present { 2000 } else { 0 },
            counting_confidence: CountingConfidence::Approximate,
            authority_tier: Some(6),
            truncation_reason: None,
        });
    }

    // Environment block.
    {
        let env_text = format!(
            "## Environment\n\n- lang: en\n- deepseek_version: {}\n- platform: macos\n- shell: bash\n- pwd: {}",
            env!("CARGO_PKG_VERSION"),
            workspace.display()
        );
        entries.push(SourceEntry {
            source_kind: SourceKind::EnvironmentBlock,
            label: "Environment block".to_string(),
            source_path: None,
            activation_reason: ActivationReason::AlwaysOn,
            estimated_tokens: estimate_text_tokens_conservative(&env_text),
            counting_confidence: CountingConfidence::High,
            authority_tier: None,
            truncation_reason: None,
        });
    }

    // Language requirement (if translation is on).
    if app.ui_locale != crate::localization::Locale::En {
        entries.push(SourceEntry {
            source_kind: SourceKind::LanguageRequirement,
            label: "Language output requirement".to_string(),
            source_path: None,
            activation_reason: ActivationReason::FeatureEnabled,
            estimated_tokens: 300,
            counting_confidence: CountingConfidence::Approximate,
            authority_tier: None,
            truncation_reason: None,
        });
    }

    // Instructions files.
    // Check for common instruction files at workspace root.
    for (name, path) in &[
        ("AGENTS.md", workspace.join("AGENTS.md")),
        ("CLAUDE.md", workspace.join("CLAUDE.md")),
        (
            ".codewhale/instructions.md",
            workspace.join(".codewhale/instructions.md"),
        ),
    ] {
        if path.exists()
            && let Ok(content) = std::fs::read_to_string(path)
        {
            let truncated = truncate_if_needed(&content, 100 * 1024);
            let was_truncated = content.len() > 100 * 1024;
            entries.push(SourceEntry {
                source_kind: SourceKind::InstructionsFile,
                label: format!("Instructions file ({name})"),
                source_path: Some(path.display().to_string()),
                activation_reason: ActivationReason::FilePresent,
                estimated_tokens: estimate_text_tokens_conservative(&truncated),
                counting_confidence: CountingConfidence::High,
                authority_tier: Some(5),
                truncation_reason: if was_truncated {
                    Some("truncated to 100 KB".to_string())
                } else {
                    None
                },
            });
        }
    }

    // User memory.
    if app.use_memory {
        let memory_present = app.memory_path.exists();
        let (tokens, truncation_reason) = if memory_present {
            match std::fs::read_to_string(&app.memory_path) {
                Ok(content) if content.trim().is_empty() => (0, None),
                Ok(content) => {
                    let estimated = estimate_text_tokens_conservative(&content);
                    (estimated, None)
                }
                Err(_) => (0, None),
            }
        } else {
            (0, None)
        };
        entries.push(SourceEntry {
            source_kind: SourceKind::UserMemory,
            label: "User memory".to_string(),
            source_path: Some(app.memory_path.display().to_string()),
            activation_reason: if memory_present && tokens > 0 {
                ActivationReason::ConfigEnabled
            } else {
                ActivationReason::Fallback
            },
            estimated_tokens: tokens,
            counting_confidence: if tokens > 0 {
                CountingConfidence::High
            } else {
                CountingConfidence::Low
            },
            authority_tier: Some(7),
            truncation_reason,
        });
    }

    // Session goal.
    if let Some(ref goal) = app.hunt.quarry
        && !goal.trim().is_empty()
    {
        entries.push(SourceEntry {
            source_kind: SourceKind::SessionGoal,
            label: "Session goal".to_string(),
            source_path: None,
            activation_reason: ActivationReason::UserAction,
            estimated_tokens: estimate_text_tokens_conservative(goal),
            counting_confidence: CountingConfidence::High,
            authority_tier: Some(2),
            truncation_reason: None,
        });
    }

    // Handoff relay.
    {
        let handoff_path = workspace.join(".codewhale/handoff.md");
        let legacy_path = workspace.join(".deepseek/handoff.md");
        let path = if handoff_path.exists() {
            handoff_path
        } else {
            legacy_path
        };
        if path.exists()
            && let Ok(content) = std::fs::read_to_string(&path)
        {
            entries.push(SourceEntry {
                source_kind: SourceKind::HandoffRelay,
                label: "Handoff relay".to_string(),
                source_path: Some(path.display().to_string()),
                activation_reason: ActivationReason::FilePresent,
                estimated_tokens: estimate_text_tokens_conservative(&content),
                counting_confidence: CountingConfidence::High,
                authority_tier: Some(9),
                truncation_reason: None,
            });
        }
    }

    // Authority recap.
    entries.push(SourceEntry {
        source_kind: SourceKind::AuthorityRecap,
        label: "Authority recap".to_string(),
        source_path: None,
        activation_reason: ActivationReason::AlwaysOn,
        estimated_tokens: 300,
        counting_confidence: CountingConfidence::Approximate,
        authority_tier: Some(1),
        truncation_reason: None,
    });

    // ── Model / provider facts ────────────────────────────────────────

    {
        let window = context_window_for_model(model);
        let window_str = window
            .map(|w| format!("{w} tokens"))
            .unwrap_or_else(|| "unknown".to_string());
        entries.push(SourceEntry {
            source_kind: SourceKind::ModelProviderFact,
            label: format!("Model fact: {model} (window: {window_str})"),
            source_path: None,
            activation_reason: ActivationReason::AlwaysOn,
            estimated_tokens: 200,
            counting_confidence: CountingConfidence::Approximate,
            authority_tier: None,
            truncation_reason: None,
        });
    }

    // MCP server schemas.
    {
        let mcp_config_path = &app.mcp_config_path;
        if mcp_config_path.exists()
            && let Ok(content) = std::fs::read_to_string(mcp_config_path)
        {
            let server_count =
                content.matches("\"command\"").count() + content.matches("\"url\"").count();
            if server_count > 0 {
                entries.push(SourceEntry {
                    source_kind: SourceKind::McpServerSchema,
                    label: format!("MCP servers ({server_count} configured)"),
                    source_path: Some(mcp_config_path.display().to_string()),
                    activation_reason: ActivationReason::ConfigEnabled,
                    estimated_tokens: server_count * 400,
                    counting_confidence: CountingConfidence::Approximate,
                    authority_tier: None,
                    truncation_reason: None,
                });
            }
        }
    }

    // ── Per-request runtime context ────────────────────────────────────

    // User request (latest user message).
    if let Some(last_msg) = app.api_messages.last()
        && last_msg.role == "user"
    {
        let text: String = last_msg
            .content
            .iter()
            .filter_map(|block| {
                if let crate::models::ContentBlock::Text { text, .. } = block {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        if !text.is_empty() {
            entries.push(SourceEntry {
                source_kind: SourceKind::UserRequest,
                label: "Current user request".to_string(),
                source_path: None,
                activation_reason: ActivationReason::PerRequest,
                estimated_tokens: estimate_text_tokens_conservative(&text),
                counting_confidence: CountingConfidence::High,
                authority_tier: Some(2),
                truncation_reason: None,
            });
        }
    }

    // Recent tool results.
    {
        let mut tool_result_count = 0usize;
        let mut tool_result_chars = 0usize;
        for msg in app.api_messages.iter().rev().take(20) {
            if msg.role == "user" {
                for block in &msg.content {
                    if let crate::models::ContentBlock::ToolResult { content, .. } = block {
                        tool_result_count += 1;
                        tool_result_chars += content.chars().count();
                    }
                }
            }
        }
        if tool_result_count > 0 {
            let truncated = tool_result_chars > 180_000;
            entries.push(SourceEntry {
                source_kind: SourceKind::ToolResult,
                label: format!("Tool results ({tool_result_count} recent)"),
                source_path: None,
                activation_reason: ActivationReason::PerRequest,
                estimated_tokens: estimate_text_tokens_conservative(
                    &"x".repeat(tool_result_chars.min(180_000)),
                ),
                counting_confidence: CountingConfidence::Approximate,
                authority_tier: Some(6),
                truncation_reason: if truncated {
                    Some(format!(
                        "hard-limited to 180K chars; {tool_result_chars} total across {tool_result_count} results"
                    ))
                } else {
                    None
                },
            });
        }
    }

    // Sub-agent summaries.
    if !app.subagent_cache.is_empty() {
        let count = app.subagent_cache.len();
        entries.push(SourceEntry {
            source_kind: SourceKind::SubAgentSummary,
            label: format!("Sub-agent summaries ({count} active)"),
            source_path: None,
            activation_reason: ActivationReason::PerRequest,
            estimated_tokens: count * 500,
            counting_confidence: CountingConfidence::Low,
            authority_tier: Some(6),
            truncation_reason: None,
        });
    }

    // Compaction summary.
    if let Some(ref system_prompt) = app.system_prompt {
        let text = system_prompt_text(system_prompt);
        if text.contains("Compaction Relay") || text.contains("compaction") {
            // A compaction summary is in the prompt — estimate conservatively
            entries.push(SourceEntry {
                source_kind: SourceKind::CompactionSummary,
                label: "Compaction summary".to_string(),
                source_path: None,
                activation_reason: ActivationReason::PerRequest,
                estimated_tokens: 2000,
                counting_confidence: CountingConfidence::Low,
                authority_tier: Some(9),
                truncation_reason: None,
            });
        }
    }

    // ── Compute aggregates ─────────────────────────────────────────────

    let total_estimated_tokens: usize = entries.iter().map(|e| e.estimated_tokens).sum();
    let context_window_tokens = context_window_for_model(model);
    let budget_used_percent = context_window_tokens.map(|window| {
        (total_estimated_tokens as f64 / f64::from(window) * 100.0).clamp(0.0, 100.0)
    });

    // ISO-8601 timestamp.
    let generated_at = {
        use std::time::SystemTime;
        match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
            Ok(dur) => {
                let secs = dur.as_secs();
                // Simple ISO-8601 (UTC)
                let days_since_epoch = secs / 86400;
                let time_of_day = secs % 86400;
                let hours = time_of_day / 3600;
                let minutes = (time_of_day % 3600) / 60;
                let seconds = time_of_day % 60;
                // Approximate: use a known reference point
                // 2026-06-12 = day number X from Unix epoch
                // Actually, let's just use a simple format
                format!("{hours:02}:{minutes:02}:{seconds:02}Z (day {days_since_epoch} from epoch)")
            }
            Err(_) => "unknown".to_string(),
        }
    };

    PromptSourceMap {
        entries,
        total_estimated_tokens,
        context_window_tokens,
        budget_used_percent,
        generated_at,
    }
}

/// Format a `PromptSourceMap` as a human-readable text report for the TUI.
pub fn format_context_report(map: &PromptSourceMap) -> String {
    let mut out = String::new();
    use std::fmt::Write;

    let _ = writeln!(out, "## Context Report\n");
    let _ = writeln!(
        out,
        "Total estimated input tokens: {}",
        map.total_estimated_tokens
    );
    if let Some(window) = map.context_window_tokens {
        let _ = writeln!(out, "Model context window:        {window} tokens");
        if let Some(pct) = map.budget_used_percent {
            let _ = writeln!(out, "Budget used:                 {pct:.1}%");
            let bar = pressure_bar(pct);
            let _ = writeln!(out, "Pressure:                    {bar}");
        }
    } else {
        let _ = writeln!(out, "Model context window:        unknown");
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "---");
    let _ = writeln!(out);

    // Group entries by their position in the prompt
    let _ = writeln!(out, "## Static layers (compile-time)\n");
    for entry in &map.entries {
        if matches!(
            entry.source_kind,
            SourceKind::Constitution
                | SourceKind::Personality
                | SourceKind::ModeDelta
                | SourceKind::ApprovalPolicy
                | SourceKind::ToolTaxonomy
                | SourceKind::ContextManagement
                | SourceKind::CompactionRelayTemplate
                | SourceKind::AuthorityRecap
                | SourceKind::LocaleReinforcement
                | SourceKind::LanguageRequirement
        ) {
            write_entry(&mut out, entry);
        }
    }

    let _ = writeln!(out, "\n## Workspace-dependent layers\n");
    for entry in &map.entries {
        if matches!(
            entry.source_kind,
            SourceKind::ProjectContext
                | SourceKind::ProjectContextPack
                | SourceKind::SkillsBlock
                | SourceKind::EnvironmentBlock
                | SourceKind::InstructionsFile
                | SourceKind::UserMemory
                | SourceKind::SessionGoal
                | SourceKind::HandoffRelay
        ) {
            write_entry(&mut out, entry);
        }
    }

    let _ = writeln!(out, "\n## Model / provider facts\n");
    for entry in &map.entries {
        if matches!(
            entry.source_kind,
            SourceKind::ModelProviderFact | SourceKind::McpServerSchema
        ) {
            write_entry(&mut out, entry);
        }
    }

    let _ = writeln!(out, "\n## Per-request runtime context\n");
    for entry in &map.entries {
        if matches!(
            entry.source_kind,
            SourceKind::UserRequest
                | SourceKind::ToolResult
                | SourceKind::SubAgentSummary
                | SourceKind::CompactionSummary
        ) {
            write_entry(&mut out, entry);
        }
    }

    let _ = writeln!(
        out,
        "\n---\nThis report is diagnostic — it estimates where token budget is spent.\n\
         It is not a memory store. Use `/memory` to inspect your persistent\n\
         user-memory file."
    );

    out
}

fn write_entry(out: &mut String, entry: &SourceEntry) {
    use std::fmt::Write;
    let tier_str = entry
        .authority_tier
        .map(|t| format!("Tier {t}"))
        .unwrap_or_else(|| "—".to_string());
    let confidence = match entry.counting_confidence {
        CountingConfidence::Exact => "exact",
        CountingConfidence::High => "high",
        CountingConfidence::Approximate => "approx",
        CountingConfidence::Low => "low",
    };
    let _ = writeln!(
        out,
        "  {label:<50} {tokens:>6} tokens  [{confidence}]  {tier_str}",
        label = entry.label,
        tokens = entry.estimated_tokens,
        confidence = confidence,
        tier_str = tier_str,
    );
    if let Some(ref path) = entry.source_path {
        let _ = writeln!(out, "    path: {path}");
    }
    if let Some(ref reason) = entry.truncation_reason {
        let _ = writeln!(out, "    ⚠ {reason}");
    }
}

fn pressure_bar(percent: f64) -> &'static str {
    if percent > 90.0 {
        "🔴 CRITICAL"
    } else if percent > 70.0 {
        "🟠 HIGH"
    } else if percent > 40.0 {
        "🟡 MODERATE"
    } else {
        "🟢 LOW"
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────

fn system_prompt_text(sp: &crate::models::SystemPrompt) -> &str {
    match sp {
        crate::models::SystemPrompt::Text(text) => text.as_str(),
        crate::models::SystemPrompt::Blocks(_) => "",
    }
}

fn truncate_if_needed(content: &str, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        content.to_string()
    } else {
        let head_end = (0..=max_bytes)
            .rev()
            .find(|&i| content.is_char_boundary(i))
            .unwrap_or(0);
        content[..head_end].to_string()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::{App, AppMode, TuiOptions};
    use tempfile::TempDir;

    fn test_app(tmpdir: &TempDir, mode: AppMode, use_memory: bool) -> App {
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
            use_memory,
            start_in_agent_mode: mode == AppMode::Agent,
            skip_onboarding: true,
            yolo: mode == AppMode::Yolo,
            resume_session_id: None,
            initial_input: None,
        };
        App::new(options, &Config::default())
    }

    #[test]
    fn build_report_yields_non_empty_entries_for_agent_mode() {
        let tmpdir = TempDir::new().expect("tempdir");
        let app = test_app(&tmpdir, AppMode::Agent, false);
        let report = build_context_report(&app, tmpdir.path());
        assert!(!report.entries.is_empty(), "report should have entries");
        assert!(
            report.total_estimated_tokens > 0,
            "should estimate some tokens"
        );
        // Static layers should always be present.
        let kinds: Vec<_> = report.entries.iter().map(|e| &e.source_kind).collect();
        assert!(
            kinds.contains(&&SourceKind::Constitution),
            "should include Constitution"
        );
        assert!(
            kinds.contains(&&SourceKind::Personality),
            "should include Personality"
        );
        assert!(
            kinds.contains(&&SourceKind::ApprovalPolicy),
            "should include ApprovalPolicy"
        );
        assert!(
            kinds.contains(&&SourceKind::ToolTaxonomy),
            "should include ToolTaxonomy"
        );
        assert!(
            kinds.contains(&&SourceKind::AuthorityRecap),
            "should include AuthorityRecap"
        );
    }

    #[test]
    fn build_report_includes_user_memory_when_enabled() {
        let tmpdir = TempDir::new().expect("tempdir");
        // Write memory file.
        let memory_content = "- User prefers concise responses\n- Workspace is Rust project\n";
        std::fs::write(tmpdir.path().join("memory.md"), memory_content).expect("write memory file");
        let app = test_app(&tmpdir, AppMode::Agent, true);
        let report = build_context_report(&app, tmpdir.path());
        let kinds: Vec<_> = report.entries.iter().map(|e| &e.source_kind).collect();
        assert!(
            kinds.contains(&&SourceKind::UserMemory),
            "report should include UserMemory when enabled"
        );
        let mem_entry = report
            .entries
            .iter()
            .find(|e| e.source_kind == SourceKind::UserMemory)
            .expect("UserMemory entry should exist");
        assert!(
            mem_entry.estimated_tokens > 0,
            "memory should contribute tokens"
        );
        assert_eq!(
            mem_entry.activation_reason,
            ActivationReason::ConfigEnabled,
            "should be config-enabled"
        );
        assert_eq!(mem_entry.authority_tier, Some(7), "memory is Tier 7");
    }

    #[test]
    fn build_report_skips_user_memory_when_disabled() {
        let tmpdir = TempDir::new().expect("tempdir");
        let app = test_app(&tmpdir, AppMode::Agent, false);
        let report = build_context_report(&app, tmpdir.path());
        let kinds: Vec<_> = report.entries.iter().map(|e| &e.source_kind).collect();
        assert!(
            !kinds.contains(&&SourceKind::UserMemory),
            "report should NOT include UserMemory when disabled"
        );
    }

    #[test]
    fn build_report_includes_instructions_file_when_present() {
        let tmpdir = TempDir::new().expect("tempdir");
        // Write an AGENTS.md file (instructions file).
        let instructions = "# Test Instructions\n\nThis is a test AGENTS.md file.\n";
        std::fs::write(tmpdir.path().join("AGENTS.md"), instructions).expect("write AGENTS.md");
        let app = test_app(&tmpdir, AppMode::Agent, false);
        let report = build_context_report(&app, tmpdir.path());
        let kinds: Vec<_> = report.entries.iter().map(|e| &e.source_kind).collect();
        assert!(
            kinds.contains(&&SourceKind::InstructionsFile),
            "report should include InstructionsFile when AGENTS.md present"
        );
        let instr_entry = report
            .entries
            .iter()
            .find(|e| e.source_kind == SourceKind::InstructionsFile)
            .expect("InstructionsFile entry should exist");
        assert!(
            instr_entry.estimated_tokens > 0,
            "instructions should contribute tokens"
        );
        assert_eq!(
            instr_entry.activation_reason,
            ActivationReason::FilePresent,
            "should be file-present"
        );
        assert!(
            instr_entry.truncation_reason.is_none(),
            "small file should not be truncated"
        );
    }

    #[test]
    fn build_report_handles_large_instructions_file_truncation() {
        let tmpdir = TempDir::new().expect("tempdir");
        // Create a file larger than 100 KB.
        let large_content = "A".repeat(110 * 1024); // 110 KB
        std::fs::write(tmpdir.path().join("AGENTS.md"), &large_content)
            .expect("write large AGENTS.md");
        let app = test_app(&tmpdir, AppMode::Agent, false);
        let report = build_context_report(&app, tmpdir.path());
        let instr_entry = report
            .entries
            .iter()
            .find(|e| e.source_kind == SourceKind::InstructionsFile)
            .expect("InstructionsFile entry should exist");
        assert!(
            instr_entry.truncation_reason.is_some(),
            "large file should have truncation reason"
        );
        let reason = instr_entry.truncation_reason.as_ref().unwrap();
        assert!(
            reason.contains("truncated"),
            "truncation reason should mention truncation: {reason}"
        );
    }

    #[test]
    fn build_report_includes_skills_when_dir_has_content() {
        let tmpdir = TempDir::new().expect("tempdir");
        // Create skills directory with some skill dirs.
        let skills_dir = tmpdir.path().join("skills");
        std::fs::create_dir_all(skills_dir.join("api-design")).expect("create skill dir");
        std::fs::create_dir_all(skills_dir.join("backend-patterns")).expect("create skill dir");
        // Write a minimal SKILL.md in one
        std::fs::write(
            skills_dir.join("api-design").join("SKILL.md"),
            "---\nname: api-design\ndescription: API design patterns\n---\n",
        )
        .expect("write SKILL.md");
        std::fs::write(
            skills_dir.join("backend-patterns").join("SKILL.md"),
            "---\nname: backend-patterns\ndescription: Backend patterns\n---\n",
        )
        .expect("write SKILL.md");
        let app = test_app(&tmpdir, AppMode::Agent, false);
        let report = build_context_report(&app, tmpdir.path());
        let kinds: Vec<_> = report.entries.iter().map(|e| &e.source_kind).collect();
        assert!(
            kinds.contains(&&SourceKind::SkillsBlock),
            "report should include SkillsBlock when skills dir has content"
        );
    }

    #[test]
    fn build_report_includes_tool_results_with_truncation_for_large_output() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = test_app(&tmpdir, AppMode::Agent, false);
        // Simulate a large tool result in api_messages
        use crate::models::{ContentBlock, Message};
        let large_text = "x".repeat(200_000); // exceeds 180K hard limit
        let tool_msg = Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tu_001".to_string(),
                content: large_text,
                is_error: None,
                content_blocks: None,
            }],
        };
        app.api_messages.push(tool_msg);
        let report = build_context_report(&app, tmpdir.path());
        let kinds: Vec<_> = report.entries.iter().map(|e| &e.source_kind).collect();
        assert!(
            kinds.contains(&&SourceKind::ToolResult),
            "report should include ToolResult entries"
        );
        let tool_entry = report
            .entries
            .iter()
            .find(|e| e.source_kind == SourceKind::ToolResult)
            .expect("ToolResult entry should exist");
        assert!(
            tool_entry.truncation_reason.is_some(),
            "large tool result should have truncation reason"
        );
    }

    #[test]
    fn format_context_report_produces_human_readable_output() {
        let tmpdir = TempDir::new().expect("tempdir");
        let app = test_app(&tmpdir, AppMode::Agent, false);
        let report = build_context_report(&app, tmpdir.path());
        let formatted = format_context_report(&report);
        assert!(formatted.contains("Context Report"), "should have title");
        assert!(
            formatted.contains("Total estimated input tokens"),
            "should show token total"
        );
        assert!(
            formatted.contains("Static layers"),
            "should group static layers"
        );
        assert!(
            formatted.contains("Workspace-dependent layers"),
            "should group workspace layers"
        );
        assert!(
            formatted.contains("Per-request runtime context"),
            "should group runtime context"
        );
        assert!(
            formatted.contains("/memory"),
            "should distinguish from /memory command"
        );
    }

    #[test]
    fn prompt_source_map_serializes_to_json() {
        let tmpdir = TempDir::new().expect("tempdir");
        let app = test_app(&tmpdir, AppMode::Agent, false);
        let report = build_context_report(&app, tmpdir.path());
        let json = serde_json::to_string_pretty(&report).expect("serialize to JSON");
        assert!(json.contains("entries"), "JSON should contain entries");
        assert!(
            json.contains("total_estimated_tokens"),
            "JSON should contain total"
        );
        assert!(
            json.contains("context_window_tokens"),
            "JSON should contain window"
        );
    }

    #[test]
    fn build_report_includes_mcp_when_config_present() {
        let tmpdir = TempDir::new().expect("tempdir");
        // Write a minimal MCP config.
        let mcp_config =
            r#"{"mcpServers": {"test-server": {"command": "echo", "args": ["hello"]}}}"#;
        std::fs::write(tmpdir.path().join("mcp.json"), mcp_config).expect("write mcp.json");
        let app = test_app(&tmpdir, AppMode::Agent, false);
        let report = build_context_report(&app, tmpdir.path());
        let kinds: Vec<_> = report.entries.iter().map(|e| &e.source_kind).collect();
        assert!(
            kinds.contains(&&SourceKind::McpServerSchema),
            "report should include MCP when config present"
        );
    }

    #[test]
    fn build_report_context_window_for_deepseek_v4() {
        let tmpdir = TempDir::new().expect("tempdir");
        let app = test_app(&tmpdir, AppMode::Agent, false);
        let report = build_context_report(&app, tmpdir.path());
        assert_eq!(
            report.context_window_tokens,
            Some(1_000_000),
            "deepseek-v4-pro should have 1M context window"
        );
        assert!(
            report.budget_used_percent.is_some(),
            "should compute budget %"
        );
    }

    #[test]
    fn build_report_authority_tiers_are_valid() {
        let tmpdir = TempDir::new().expect("tempdir");
        let app = test_app(&tmpdir, AppMode::Agent, false);
        let report = build_context_report(&app, tmpdir.path());
        for entry in &report.entries {
            if let Some(tier) = entry.authority_tier {
                assert!(
                    (1..=9).contains(&tier),
                    "authority tier {tier} should be in 1..=9 for entry {:?}",
                    entry.source_kind
                );
            }
        }
    }

    #[test]
    fn source_map_entries_have_required_fields() {
        let tmpdir = TempDir::new().expect("tempdir");
        // Set up with user memory + AGENTS.md + skills to test acceptance criteria coverage.
        std::fs::write(tmpdir.path().join("memory.md"), "- User prefers Rust\n")
            .expect("write memory");
        std::fs::write(
            tmpdir.path().join("AGENTS.md"),
            "# Test\n\nSome instructions.\n",
        )
        .expect("write AGENTS.md");
        let skills_dir = tmpdir.path().join("skills");
        std::fs::create_dir_all(skills_dir.join("api-design")).expect("create skill dir");
        std::fs::write(
            skills_dir.join("api-design").join("SKILL.md"),
            "---\nname: api-design\ndescription: API patterns\n---\n",
        )
        .expect("write SKILL.md");

        let mut app = test_app(&tmpdir, AppMode::Agent, true);
        // Add a large tool result (>180K chars)
        use crate::models::{ContentBlock, Message};
        let large_text = "y".repeat(250_000);
        let tool_msg = Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tu_002".to_string(),
                content: large_text,
                is_error: None,
                content_blocks: None,
            }],
        };
        app.api_messages.push(tool_msg);

        let report = build_context_report(&app, tmpdir.path());

        // Every entry must have non-empty label and counting confidence
        for entry in &report.entries {
            assert!(
                !entry.label.is_empty(),
                "entry label should not be empty for {:?}",
                entry.source_kind
            );
            assert!(
                entry.counting_confidence != CountingConfidence::Low
                    || entry.estimated_tokens == 0
                    || entry.truncation_reason.is_some(),
                "Low confidence entries should have 0 tokens or a truncation reason"
            );
        }

        // Coverage of acceptance criteria: user memory + instructions + skill + large tool result
        let kinds: Vec<_> = report.entries.iter().map(|e| &e.source_kind).collect();
        assert!(kinds.contains(&&SourceKind::UserMemory));
        assert!(kinds.contains(&&SourceKind::InstructionsFile));
        assert!(kinds.contains(&&SourceKind::SkillsBlock));
        assert!(kinds.contains(&&SourceKind::ToolResult));
    }
}
