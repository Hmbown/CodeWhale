//! Diagnostic prompt source map for context pressure reports.
//!
//! The report is intentionally approximate for v0.8.59. It uses the same
//! conservative token heuristic as compaction and describes the runtime sources
//! CodeWhale already tracks, without claiming provider-tokenizer parity.

use std::fmt::Write as _;
use std::path::Path;

use chrono::{SecondsFormat, Utc};
use serde::Serialize;

use crate::compaction::{estimate_input_tokens_conservative, estimate_text_tokens_conservative};
use crate::config::Config;
use crate::context_budget::PressureLevel;
use crate::models::{CacheControl, ContentBlock, Message, SystemPrompt, Tool};
use crate::prompts::{CORE_EXECUTION_PROFILE_PROMPT, Personality};
use crate::route_budget::route_context_window_tokens;
use crate::tui::app::App;

#[derive(Debug, Clone, Serialize)]
pub struct PromptSourceMap {
    pub entries: Vec<SourceEntry>,
    pub total_estimated_tokens: usize,
    pub active_context_estimated_tokens: usize,
    pub context_window_tokens: Option<u32>,
    /// Non-secret receipt for the effective context-window value.
    pub context_window_source: Option<String>,
    pub budget_used_percent: Option<f64>,
    pub generated_at: String,
    pub note: String,
}

/// Inspectable request-prefix context for the current session.
///
/// `PromptSourceMap` explains provenance and estimated pressure. This sibling
/// type exposes the current assembled system-prompt sections and most recently
/// sent model tool catalog so users can audit the prompt plumbing as JSON.
#[derive(Debug, Clone, Serialize)]
pub struct PromptContext {
    pub schema_version: u8,
    pub provider: String,
    pub model: String,
    pub system_prompt_state: &'static str,
    pub tool_catalog_state: &'static str,
    pub sections: Vec<PromptContextSection>,
    pub tools: Vec<Tool>,
    pub source_map: PromptSourceMap,
}

#[derive(Debug, Clone, Serialize)]
pub struct PromptContextSection {
    pub index: usize,
    pub block_type: String,
    pub cache_control: Option<CacheControl>,
    pub estimated_tokens: usize,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceEntry {
    pub source_kind: SourceKind,
    pub label: String,
    pub source_path: Option<String>,
    pub activation_reason: ActivationReason,
    pub estimated_tokens: usize,
    pub counting_confidence: CountingConfidence,
    pub authority_tier: Option<u8>,
    pub truncation_reason: Option<String>,
}

impl SourceEntry {
    fn text(
        source_kind: SourceKind,
        label: impl Into<String>,
        source_path: Option<String>,
        activation_reason: ActivationReason,
        text: &str,
        counting_confidence: CountingConfidence,
        authority_tier: Option<u8>,
    ) -> Self {
        Self::estimate(
            source_kind,
            label,
            source_path,
            activation_reason,
            estimate_text_tokens_conservative(text),
            counting_confidence,
            authority_tier,
        )
    }

    fn estimate(
        source_kind: SourceKind,
        label: impl Into<String>,
        source_path: Option<String>,
        activation_reason: ActivationReason,
        estimated_tokens: usize,
        counting_confidence: CountingConfidence,
        authority_tier: Option<u8>,
    ) -> Self {
        Self {
            source_kind,
            label: label.into(),
            source_path,
            activation_reason,
            estimated_tokens,
            counting_confidence,
            authority_tier,
            truncation_reason: None,
        }
    }

    fn omitted(
        source_kind: SourceKind,
        label: impl Into<String>,
        source_path: Option<String>,
        authority_tier: Option<u8>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            source_kind,
            label: label.into(),
            source_path,
            activation_reason: ActivationReason::Omitted,
            estimated_tokens: 0,
            counting_confidence: CountingConfidence::High,
            authority_tier,
            truncation_reason: Some(reason.into()),
        }
    }

    fn diagnostic(
        source_kind: SourceKind,
        label: impl Into<String>,
        source_path: Option<String>,
        activation_reason: ActivationReason,
        detail: impl Into<String>,
        estimated_tokens: usize,
        authority_tier: Option<u8>,
    ) -> Self {
        Self {
            source_kind,
            label: label.into(),
            source_path,
            activation_reason,
            estimated_tokens,
            counting_confidence: CountingConfidence::High,
            authority_tier,
            truncation_reason: Some(detail.into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Constitution,
    UserConstitution,
    RepoConstitution,
    ProjectContext,
    ProjectContextWarning,
    ProjectContextPack,
    SkillsBlock,
    ContextManagement,
    CompactionRelayTemplate,
    RuntimePolicy,
    AuthorityRecap,
    EnvironmentBlock,
    UserMemory,
    SessionGoal,
    HandoffRelay,
    ToolSchemas,
    UserRequest,
    ConversationHistory,
    ToolResult,
    ModelProviderFact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationReason {
    AlwaysOn,
    FilePresent,
    ConfigEnabled,
    RuntimeState,
    PerRequest,
    Omitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CountingConfidence {
    High,
    Approximate,
}

struct ReportBuilder {
    entries: Vec<SourceEntry>,
}

impl ReportBuilder {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    fn push(&mut self, entry: SourceEntry) {
        self.entries.push(entry);
    }

    /// The window arrives as one resolution rather than a number plus a
    /// separately chosen label, so the report can never attribute one rung's
    /// tokens to another rung.
    fn finish(
        self,
        context_window: crate::route_runtime::ContextWindowResolution,
        active_context_estimated_tokens: usize,
        note: impl Into<String>,
    ) -> PromptSourceMap {
        let total_estimated_tokens = self
            .entries
            .iter()
            .map(|entry| entry.estimated_tokens)
            .sum();
        let budget_used_percent =
            ((active_context_estimated_tokens as f64 / f64::from(context_window.tokens)) * 100.0)
                .clamp(0.0, 100.0);
        PromptSourceMap {
            entries: self.entries,
            total_estimated_tokens,
            active_context_estimated_tokens,
            context_window_tokens: Some(context_window.tokens),
            context_window_source: Some(context_window.source.label().to_string()),
            budget_used_percent: Some(budget_used_percent),
            generated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            note: note.into(),
        }
    }
}

pub fn build_context_report(app: &App) -> PromptSourceMap {
    // The host still stores the rung apart from the number; pair them against
    // the same route limits the pressure meter reads.
    let context_window = crate::route_runtime::ContextWindowResolution {
        tokens: route_context_window_tokens(app.api_provider, &app.model, app.active_route_limits),
        source: app.active_context_window_source,
    };
    let mut builder = base_source_entries(
        &app.model,
        &app.workspace,
        Some(&app.skills_dir),
        app.project_context_pack_enabled,
        app.skills_scan_codewhale_only,
        app.ui_locale.tag(),
        app.mode,
        Some(app.plugin_registry.as_ref()),
        Some(context_window.tokens),
    );
    add_app_runtime_entries(&mut builder, app);
    let active_context_estimated_tokens =
        estimate_input_tokens_conservative(&app.api_messages, app.system_prompt.as_ref());
    builder.finish(
        context_window,
        active_context_estimated_tokens,
        "Diagnostic source map. Token counts are conservative estimates and may differ from provider billing.",
    )
}

#[must_use]
pub fn build_prompt_context(app: &App) -> PromptContext {
    let tool_catalog_state = if app.session.last_tool_catalog.is_some() {
        "last_sent"
    } else {
        "not_yet_sent"
    };
    let sections = match app.system_prompt.as_ref() {
        Some(SystemPrompt::Text(text)) => vec![PromptContextSection {
            index: 0,
            block_type: "text".to_string(),
            cache_control: None,
            estimated_tokens: estimate_text_tokens_conservative(text),
            text: text.clone(),
        }],
        Some(SystemPrompt::Blocks(blocks)) => blocks
            .iter()
            .enumerate()
            .map(|(index, block)| PromptContextSection {
                index,
                block_type: block.block_type.clone(),
                cache_control: block.cache_control.clone(),
                estimated_tokens: estimate_text_tokens_conservative(&block.text),
                text: block.text.clone(),
            })
            .collect(),
        None => Vec::new(),
    };
    PromptContext {
        schema_version: 1,
        provider: app.api_provider.as_str().to_string(),
        model: app.model.clone(),
        system_prompt_state: "current_session",
        tool_catalog_state,
        sections,
        tools: app.session.last_tool_catalog.clone().unwrap_or_default(),
        source_map: build_context_report(app),
    }
}

pub fn build_headless_context_report(config: &Config, workspace: &Path) -> PromptSourceMap {
    let model = config.default_model();
    let provider = config.api_provider();
    let provider_identity = config.provider_identity_for(provider);
    let route = crate::route_runtime::resolve_runtime_route(config, provider, Some(&model)).ok();
    // A route we could not resolve does not erase an operator-configured
    // window: doctor must report the same number the session would use.
    let context_window = route.as_ref().map_or_else(
        || {
            crate::route_runtime::resolve_context_window(
                provider,
                &model,
                None,
                config.context_window_for_provider_config(provider),
            )
        },
        |route| route.context_window,
    );
    let global_skills_dir = config.skills_dir();
    let selected_skills_dir =
        crate::tui::app::resolve_skills_dir(workspace, &global_skills_dir, config);
    let mut builder = base_source_entries(
        &model,
        workspace,
        Some(&selected_skills_dir),
        config.project_context_pack_enabled(),
        config.skills_config().scan_codewhale_only(),
        "en",
        crate::tui::app::AppMode::Agent,
        None,
        Some(context_window.tokens),
    );
    let memory_path = config.memory_path();
    let memory_enabled = config.memory_enabled();

    if let Some(memory_block) =
        crate::native_memory::native_prompt_block(memory_enabled, &memory_path, workspace)
    {
        builder.push(SourceEntry::text(
            SourceKind::UserMemory,
            "User memory",
            Some(memory_path.display().to_string()),
            ActivationReason::ConfigEnabled,
            &memory_block,
            CountingConfidence::High,
            Some(6),
        ));
    } else {
        builder.push(SourceEntry::omitted(
            SourceKind::UserMemory,
            "User memory",
            Some(memory_path.display().to_string()),
            Some(6),
            "disabled, missing, or empty",
        ));
    }

    builder.push(SourceEntry::text(
        SourceKind::ModelProviderFact,
        format!("Provider facts ({provider_identity})"),
        None,
        ActivationReason::RuntimeState,
        &format!(
            "provider: {}\nmodel: {}\ncontext_window: {}\ncontext_window_source: {}",
            provider_identity,
            model,
            context_window.tokens,
            context_window.source.label()
        ),
        CountingConfidence::Approximate,
        None,
    ));

    let active_context_estimated_tokens = builder
        .entries
        .iter()
        .map(|entry| entry.estimated_tokens)
        .sum();
    builder.finish(
        context_window,
        active_context_estimated_tokens,
        "Headless diagnostic source map. Conversation, tool results, and live TUI state are unavailable in doctor mode.",
    )
}

#[allow(clippy::too_many_arguments)]
fn base_source_entries(
    model: &str,
    workspace: &Path,
    skills_dir: Option<&Path>,
    project_pack_enabled: bool,
    skills_scan_codewhale_only: bool,
    locale_tag: &str,
    mode: crate::tui::app::AppMode,
    plugin_registry: Option<&crate::plugins::PluginRegistry>,
    context_window_tokens: Option<u32>,
) -> ReportBuilder {
    let mut builder = ReportBuilder::new();

    let constitution = crate::prompts::compose_default_static_layers(Personality::Calm, model);
    builder.push(SourceEntry::text(
        SourceKind::Constitution,
        "Bundled constitution, language policy, and output policy",
        Some(crate::prompts::base_prompt_origin().label().to_string()),
        ActivationReason::AlwaysOn,
        &constitution,
        CountingConfidence::High,
        Some(1),
    ));

    if let Some(block) = crate::prompts::load_user_constitution_block() {
        builder.push(SourceEntry::text(
            SourceKind::UserConstitution,
            "User-global constitution",
            codewhale_config::UserConstitution::path()
                .ok()
                .map(|path| path.display().to_string()),
            ActivationReason::FilePresent,
            &block,
            CountingConfidence::High,
            Some(2),
        ));
    }

    let project_context = crate::project_context::load_project_context_with_parents(workspace);
    if let Some(block) = project_context.constitution_block.as_deref() {
        builder.push(SourceEntry::text(
            SourceKind::RepoConstitution,
            "Repository constitution",
            project_context
                .constitution_source_path
                .as_ref()
                .map(|path| path.display().to_string()),
            ActivationReason::FilePresent,
            block,
            CountingConfidence::High,
            Some(4),
        ));
    }

    if let Some(content) = project_context.instructions.as_deref() {
        let source = project_context
            .source_path
            .as_ref()
            .map_or_else(|| "project".to_string(), |p| p.display().to_string());
        let mut block = format!(
            "<project_instructions source=\"{source}\">\n{content}\n</project_instructions>"
        );
        // Include rules in the report when present
        if let Some(rules) = &project_context.rules_block {
            block.push('\n');
            block.push_str(rules);
        }
        builder.push(SourceEntry::text(
            SourceKind::ProjectContext,
            "Project instructions",
            project_context
                .source_path
                .as_ref()
                .map(|path| path.display().to_string()),
            ActivationReason::FilePresent,
            &block,
            CountingConfidence::High,
            Some(5),
        ));
    } else if let Some(rules) = &project_context.rules_block {
        // Rules exist without main instructions
        builder.push(SourceEntry::text(
            SourceKind::ProjectContext,
            "Project rules",
            None::<String>,
            ActivationReason::FilePresent,
            rules,
            CountingConfidence::High,
            Some(5),
        ));
    }

    if project_context.constitution_block.is_none() && project_context.instructions.is_none() {
        builder.push(SourceEntry::omitted(
            SourceKind::ProjectContext,
            "Project context and repository instructions",
            Some(workspace.display().to_string()),
            Some(5),
            "no project context block available",
        ));
    }
    if !project_context.warnings.is_empty() {
        let warnings = project_context.warnings.join("\n");
        let estimated_tokens = estimate_text_tokens_conservative(&warnings);
        builder.push(SourceEntry::diagnostic(
            SourceKind::ProjectContextWarning,
            "Project context warnings",
            Some(workspace.display().to_string()),
            ActivationReason::RuntimeState,
            warnings,
            estimated_tokens,
            Some(4),
        ));
    }

    if project_pack_enabled {
        if let Some(pack) = crate::project_context::generate_project_context_pack(workspace) {
            builder.push(SourceEntry::text(
                SourceKind::ProjectContextPack,
                "Project context pack",
                Some(workspace.display().to_string()),
                ActivationReason::ConfigEnabled,
                &pack,
                CountingConfidence::Approximate,
                Some(5),
            ));
        }
    } else {
        builder.push(SourceEntry::omitted(
            SourceKind::ProjectContextPack,
            "Project context pack",
            Some(workspace.display().to_string()),
            Some(5),
            "disabled; project_map provides this information on demand",
        ));
    }

    let skill_discovery_mode =
        crate::skills::SkillDiscoveryMode::from_codewhale_only(skills_scan_codewhale_only);
    let skills_budget = crate::skills::skills_prompt_budget_chars(context_window_tokens);
    let skills_block = match skills_dir {
        Some(dir) => crate::skills::render_available_skills_context_for_workspace_and_dir_with_mode_and_plugins(
            workspace,
            dir,
            skill_discovery_mode,
            locale_tag,
            plugin_registry,
            skills_budget,
        ),
        None => crate::skills::render_available_skills_context_for_workspace_with_mode_and_plugins(
            workspace,
            skill_discovery_mode,
            locale_tag,
            plugin_registry,
            skills_budget,
        ),
    };
    if let Some(block) = skills_block {
        builder.push(SourceEntry::text(
            SourceKind::SkillsBlock,
            "Available skills",
            skills_dir.map(|path| path.display().to_string()),
            ActivationReason::FilePresent,
            &block,
            CountingConfidence::High,
            Some(5),
        ));
    } else {
        builder.push(SourceEntry::omitted(
            SourceKind::SkillsBlock,
            "Available skills",
            skills_dir.map(|path| path.display().to_string()),
            Some(5),
            "no skills discovered",
        ));
    }

    builder.push(SourceEntry::omitted(
        SourceKind::ContextManagement,
        format!("{} runtime mode", mode.label()),
        None,
        Some(3),
        "mode enforced by runtime policy and the live tool catalog; no prompt doctrine",
    ));
    builder.push(SourceEntry::omitted(
        SourceKind::CompactionRelayTemplate,
        "Session relay template",
        Some("bundled in this codewhale-tui build (COMPACT_TEMPLATE, compiled in)".to_string()),
        Some(3),
        "loaded only when /relay is requested; automatic compaction owns its successor brief",
    ));
    builder.push(SourceEntry::text(
        SourceKind::RuntimePolicy,
        "Core execution discipline",
        None,
        ActivationReason::AlwaysOn,
        CORE_EXECUTION_PROFILE_PROMPT,
        CountingConfidence::High,
        Some(3),
    ));
    builder.push(SourceEntry::text(
        SourceKind::AuthorityRecap,
        "Authority recap",
        None,
        ActivationReason::AlwaysOn,
        crate::prompts::effective_authority_recap(),
        CountingConfidence::High,
        Some(1),
    ));
    builder.push(SourceEntry::text(
        SourceKind::EnvironmentBlock,
        "Runtime environment",
        Some(workspace.display().to_string()),
        ActivationReason::AlwaysOn,
        &crate::prompts::render_environment_block(workspace, locale_tag),
        CountingConfidence::High,
        Some(4),
    ));

    add_handoff_entry(&mut builder, workspace);
    builder
}

fn add_app_runtime_entries(builder: &mut ReportBuilder, app: &App) {
    if let Some(memory_block) =
        crate::native_memory::native_prompt_block(app.use_memory, &app.memory_path, &app.workspace)
    {
        builder.push(SourceEntry::text(
            SourceKind::UserMemory,
            "User memory",
            Some(app.memory_path.display().to_string()),
            ActivationReason::ConfigEnabled,
            &memory_block,
            CountingConfidence::High,
            Some(6),
        ));
    } else {
        builder.push(SourceEntry::omitted(
            SourceKind::UserMemory,
            "User memory",
            Some(app.memory_path.display().to_string()),
            Some(6),
            "disabled, missing, or empty",
        ));
    }

    if let Some(goal) = app
        .goal
        .objective
        .as_deref()
        .filter(|goal| !goal.trim().is_empty())
    {
        builder.push(SourceEntry::text(
            SourceKind::SessionGoal,
            "Session goal",
            None,
            ActivationReason::RuntimeState,
            goal,
            CountingConfidence::High,
            Some(6),
        ));
    } else {
        builder.push(SourceEntry::omitted(
            SourceKind::SessionGoal,
            "Session goal",
            None,
            Some(6),
            "no active /goal objective",
        ));
    }

    if let Some(tools) = app.session.last_tool_catalog.as_ref() {
        let rendered = serde_json::to_string(tools).unwrap_or_default();
        builder.push(SourceEntry::text(
            SourceKind::ToolSchemas,
            format!("Tool schemas ({} tools)", tools.len()),
            None,
            ActivationReason::PerRequest,
            &rendered,
            CountingConfidence::Approximate,
            Some(3),
        ));
    } else {
        builder.push(SourceEntry::omitted(
            SourceKind::ToolSchemas,
            "Tool schemas",
            None,
            Some(3),
            "no tool catalog has been sent yet",
        ));
    }

    add_message_entries(builder, &app.api_messages);
}

fn add_handoff_entry(builder: &mut ReportBuilder, workspace: &Path) {
    let primary = workspace.join(crate::prompts::HANDOFF_RELATIVE_PATH);
    let legacy = workspace.join(".deepseek/handoff.md");
    let path = if primary.exists() { primary } else { legacy };
    let Some(raw) = std::fs::read_to_string(&path)
        .ok()
        .filter(|raw| !raw.trim().is_empty())
    else {
        builder.push(SourceEntry::omitted(
            SourceKind::HandoffRelay,
            "Previous session relay",
            Some(
                workspace
                    .join(crate::prompts::HANDOFF_RELATIVE_PATH)
                    .display()
                    .to_string(),
            ),
            Some(6),
            "no relay artifact found",
        ));
        return;
    };

    builder.push(SourceEntry::text(
        SourceKind::HandoffRelay,
        "Previous session relay",
        Some(path.display().to_string()),
        ActivationReason::FilePresent,
        &raw,
        CountingConfidence::High,
        Some(6),
    ));
}

fn add_message_entries(builder: &mut ReportBuilder, messages: &[Message]) {
    if messages.is_empty() {
        builder.push(SourceEntry::omitted(
            SourceKind::ConversationHistory,
            "Conversation history",
            None,
            None,
            "no API messages yet",
        ));
        return;
    }

    let latest_user = messages.iter().rposition(|message| message.role == "user");
    let mut latest_user_tokens = 0usize;
    let mut conversation_tokens = 0usize;
    let mut tool_result_tokens = 0usize;
    let mut tool_result_count = 0usize;

    for (index, message) in messages.iter().enumerate() {
        for block in &message.content {
            let tokens = estimate_text_tokens_conservative(&content_block_text(block));
            match block {
                ContentBlock::ToolResult { .. }
                | ContentBlock::ToolSearchToolResult { .. }
                | ContentBlock::CodeExecutionToolResult { .. } => {
                    tool_result_tokens += tokens;
                    tool_result_count += 1;
                }
                ContentBlock::Text { .. } if Some(index) == latest_user => {
                    latest_user_tokens += tokens;
                }
                _ => {
                    conversation_tokens += tokens;
                }
            }
        }
    }

    if latest_user_tokens > 0 {
        builder.push(SourceEntry::estimate(
            SourceKind::UserRequest,
            "Latest user request",
            None,
            ActivationReason::PerRequest,
            latest_user_tokens,
            CountingConfidence::High,
            Some(7),
        ));
    }
    if conversation_tokens > 0 {
        builder.push(SourceEntry::estimate(
            SourceKind::ConversationHistory,
            "Conversation history",
            None,
            ActivationReason::RuntimeState,
            conversation_tokens,
            CountingConfidence::High,
            None,
        ));
    }
    if tool_result_count > 0 {
        builder.push(SourceEntry::estimate(
            SourceKind::ToolResult,
            format!("Tool results ({tool_result_count})"),
            None,
            ActivationReason::RuntimeState,
            tool_result_tokens,
            CountingConfidence::High,
            None,
        ));
    }
}

fn content_block_text(block: &ContentBlock) -> String {
    match block {
        ContentBlock::Text { text, .. } => text.clone(),
        ContentBlock::Thinking { thinking, .. } => thinking.clone(),
        ContentBlock::ToolResult { content, .. } => content.clone(),
        ContentBlock::ToolSearchToolResult { content, .. }
        | ContentBlock::CodeExecutionToolResult { content, .. } => content.to_string(),
        ContentBlock::ToolUse { input, .. } | ContentBlock::ServerToolUse { input, .. } => {
            input.to_string()
        }
        ContentBlock::ImageUrl { image_url } => image_url.url.clone(),
    }
}

fn pressure_label(percent: Option<f64>) -> &'static str {
    // Delegate to the unified pressure thresholds so this diagnostic label can't
    // drift from `context_budget::PressureLevel`. `None` (unknown window) keeps
    // its own sentinel since a level requires a usage percentage.
    match percent {
        Some(value) => PressureLevel::from_usage_percent(value).label(),
        None => "unknown",
    }
}

pub fn format_context_report(report: &PromptSourceMap) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Context Source Map");
    let _ = writeln!(
        out,
        "Estimated active context: {} tokens",
        report.active_context_estimated_tokens
    );
    match (report.context_window_tokens, report.budget_used_percent) {
        (Some(window), Some(percent)) => {
            let source = report
                .context_window_source
                .as_deref()
                .unwrap_or_else(|| crate::route_runtime::ContextWindowSource::Fallback.label());
            // An unverified rung is a guess about the window printed on this
            // same line; it must not claim a fixed 128K default the capability
            // matrix may not hold. A label from no known rung is no evidence
            // either, so it reads the same way.
            let source_label = if crate::route_runtime::ContextWindowSource::from_label(source)
                .is_some_and(crate::route_runtime::ContextWindowSource::is_verified)
            {
                source.to_string()
            } else {
                format!(
                    "{source} (unverified — nothing describes this model, so this window is a guess)"
                )
            };
            let _ = writeln!(
                out,
                "Window: {window} tokens ({percent:.1}% used, {}; source: {})",
                pressure_label(Some(percent)),
                source_label
            );
        }
        _ => {
            let _ = writeln!(out, "Window: unknown");
        }
    }
    // #5134: the source label says where the window came from but not how to
    // change it. Name the key here so the report answers the question it
    // provokes.
    let _ = writeln!(
        out,
        "Change the window: set `context_window` on the active `[providers.<name>]` table in config.toml (docs/CONFIGURATION.md, \"Context length\")."
    );
    let _ = writeln!(
        out,
        "Source-entry total: {} tokens",
        report.total_estimated_tokens
    );
    let _ = writeln!(
        out,
        "Manage standing law: /constitution (status/preview), /constitution repo (repo-local law), /setup report (readiness)."
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "Sources:");
    for entry in &report.entries {
        let path = entry
            .source_path
            .as_deref()
            .map(|path| format!(" [{path}]"))
            .unwrap_or_default();
        let tier = entry
            .authority_tier
            .map(|tier| format!(", tier {tier}"))
            .unwrap_or_default();
        let omitted = entry
            .truncation_reason
            .as_deref()
            .map(|reason| format!(" - {reason}"))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "- {:?}: {}{} - {} tokens ({:?}{}){}",
            entry.source_kind,
            entry.label,
            path,
            entry.estimated_tokens,
            entry.counting_confidence,
            tier,
            omitted
        );
    }
    let _ = writeln!(out);
    let _ = write!(out, "{}", report.note);
    out
}

pub fn format_context_summary(report: &PromptSourceMap) -> String {
    let mut entries = report.entries.clone();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.estimated_tokens));
    let top = entries
        .iter()
        .take(5)
        .map(|entry| format!("{} ({})", entry.label, entry.estimated_tokens))
        .collect::<Vec<_>>()
        .join(", ");

    let mut out = String::new();
    let _ = writeln!(out, "Context Summary");
    let _ = writeln!(
        out,
        "Pressure: {}",
        pressure_label(report.budget_used_percent)
    );
    let _ = writeln!(
        out,
        "Estimated active context: {} tokens",
        report.active_context_estimated_tokens
    );
    if let Some(percent) = report.budget_used_percent {
        let _ = writeln!(out, "Budget used: {percent:.1}%");
    }
    let _ = write!(out, "Top sources: {top}");
    out
}

pub fn context_report_json(report: &PromptSourceMap) -> String {
    serde_json::to_string_pretty(report).unwrap_or_else(|err| {
        format!("{{\"error\":\"failed to serialize context report: {err}\"}}")
    })
}

#[must_use]
pub fn prompt_context_json(context: &PromptContext) -> String {
    serde_json::to_string_pretty(context).unwrap_or_else(|error| {
        format!(r#"{{"error":"failed to serialize prompt context: {error}"}}"#)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ApiProvider, Config};
    use crate::models::Role;
    use crate::models::Tool;
    use crate::route_runtime::{ContextWindowResolution, ContextWindowSource};
    use codewhale_config::route::RouteLimits;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn context_report_json_contains_sources_and_tool_results() {
        let messages = vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "read src/lib.rs".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "call_1".to_string(),
                    content: "large tool output".repeat(40),
                    is_error: None,
                    content_blocks: None,
                }],
            },
        ];
        let mut builder = ReportBuilder::new();
        builder.push(SourceEntry::text(
            SourceKind::Constitution,
            "Test static",
            None,
            ActivationReason::AlwaysOn,
            "static",
            CountingConfidence::High,
            Some(1),
        ));
        add_message_entries(&mut builder, &messages);
        let report = builder.finish(
            ContextWindowResolution {
                tokens: 128_000,
                source: ContextWindowSource::Fallback,
            },
            123,
            "test",
        );
        let json = context_report_json(&report);

        assert!(json.contains("\"source_kind\": \"tool_result\""));
        assert!(json.contains("\"active_context_estimated_tokens\": 123"));
    }

    #[test]
    fn context_report_surfaces_repo_constitution_source_and_warnings() {
        let tmp = tempdir().expect("tempdir");
        fs::create_dir(tmp.path().join(".git")).expect("mkdir .git");
        fs::create_dir(tmp.path().join(".codewhale")).expect("mkdir .codewhale");
        fs::write(
            tmp.path().join(".codewhale").join("constitution.json"),
            r#"{
                "schema_version": 1,
                "authority": ["current user request"],
                "branch_policy": "v0.8.53 work targets the codex/v0.8.53 integration branch, not main"
            }"#,
        )
        .expect("write constitution");

        let report = build_headless_context_report(&Config::default(), tmp.path());
        assert!(
            report.entries.iter().any(|entry| {
                entry.source_kind == SourceKind::RepoConstitution
                    && entry.source_path.as_deref().is_some_and(|path| {
                        path.replace('\\', "/")
                            .ends_with(".codewhale/constitution.json")
                    })
            }),
            "repo constitution source should be an explicit source-map entry: {:?}",
            report.entries
        );
        assert!(
            report.entries.iter().any(|entry| {
                entry.source_kind == SourceKind::ProjectContextWarning
                    && entry
                        .truncation_reason
                        .as_deref()
                        .is_some_and(|reason| reason.contains("branch_policy appears stale"))
                    && entry.estimated_tokens > 0
            }),
            "repo constitution warnings should be explicit source-map entries: {:?}",
            report.entries
        );

        let formatted = format_context_report(&report);
        assert!(formatted.contains("Repository constitution"));
        assert!(formatted.contains("Project context warnings"));
        assert!(formatted.contains("/constitution"));
        assert!(formatted.contains("/setup report"));
        let json = context_report_json(&report);
        assert!(json.contains("\"repo_constitution\""));
        assert!(json.contains("branch_policy appears stale"));
    }

    #[test]
    fn headless_context_report_uses_kimi_code_k3_route_context() {
        let tmp = tempdir().expect("workspace");
        let config = Config {
            provider: Some("moonshot".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                moonshot: crate::config::ProviderConfig {
                    api_key: Some("test-kimi-key".to_string()),
                    base_url: Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL.to_string()),
                    model: Some(crate::config::KIMI_CODE_K3_MODEL.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        let report = build_headless_context_report(&config, tmp.path());

        assert_eq!(report.context_window_tokens, Some(262_144));
        assert_eq!(
            report.context_window_source.as_deref(),
            Some("static Kimi Code safe floor")
        );
        assert!(context_report_json(&report).contains("\"context_window_tokens\": 262144"));
    }

    #[test]
    fn headless_context_report_honors_kimi_code_k3_context_override() {
        let tmp = tempdir().expect("workspace");
        let config = Config {
            provider: Some("moonshot".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                moonshot: crate::config::ProviderConfig {
                    api_key: Some("test-kimi-key".to_string()),
                    base_url: Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL.to_string()),
                    model: Some(crate::config::KIMI_CODE_K3_MODEL.to_string()),
                    context_window: Some(1_048_576),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        let report = build_headless_context_report(&config, tmp.path());

        assert_eq!(report.context_window_tokens, Some(1_048_576));
        assert_eq!(report.context_window_source.as_deref(), Some("configured"));
    }

    fn private_deployment_config(context_window: Option<u32>) -> Config {
        Config {
            provider: Some("custom".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                custom: std::collections::HashMap::from([(
                    "custom".to_string(),
                    crate::config::ProviderConfig {
                        api_key: Some("test-private-key".to_string()),
                        base_url: Some("https://private.test/v1".to_string()),
                        model: Some("private-1m-deployment-v9".to_string()),
                        context_window,
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// #5239: a privately deployed id nobody catalogs, with an operator
    /// override, is a 1M route in the report — no route-resolution outcome may
    /// silently substitute the legacy window.
    #[test]
    fn headless_context_report_honors_a_private_model_context_override() {
        let tmp = tempdir().expect("workspace");

        let report =
            build_headless_context_report(&private_deployment_config(Some(1_048_576)), tmp.path());

        assert_eq!(report.context_window_tokens, Some(1_048_576));
        assert_eq!(report.context_window_source.as_deref(), Some("configured"));
    }

    /// The same id without an override is a guess, and the report must say so
    /// against the window it actually used.
    #[test]
    fn headless_context_report_marks_an_unknown_private_model_unverified() {
        let tmp = tempdir().expect("workspace");

        let report = build_headless_context_report(&private_deployment_config(None), tmp.path());

        assert_eq!(report.context_window_source.as_deref(), Some("fallback"));
        let formatted = format_context_report(&report);
        assert!(formatted.contains("this window is a guess"), "{formatted}");
        assert!(
            !formatted.contains("128K"),
            "the fallback rung must not assert a window it did not read: {formatted}"
        );
    }

    #[test]
    fn context_report_marks_whale_md_ignored_without_loading_body() {
        let tmp = tempdir().expect("tempdir");
        fs::write(tmp.path().join("WHALE.md"), "SECRET_LEGACY_WHALE_BODY").expect("write whale");

        let report = build_headless_context_report(&Config::default(), tmp.path());
        assert!(
            report.entries.iter().any(|entry| {
                entry.source_kind == SourceKind::ProjectContextWarning
                    && entry
                        .truncation_reason
                        .as_deref()
                        .is_some_and(|reason| reason.contains("WHALE.md is ignored"))
            }),
            "ignored WHALE.md should be visible as a migration warning: {:?}",
            report.entries
        );
        assert!(
            !context_report_json(&report).contains("SECRET_LEGACY_WHALE_BODY"),
            "ignored WHALE.md body must not enter context report"
        );
    }

    #[test]
    fn app_context_report_omits_legacy_plain_file_memory() {
        // The legacy single-file memory path (`~/.deepseek/memory.md` and
        // friends) was deleted for v0.9.4: only the native
        // `memory/global/MEMORY.md` store injects.
        let tmp = tempdir().expect("tempdir");
        let memory_path = tmp.path().join("memory.md");
        fs::write(&memory_path, "private legacy memory").expect("write memory");
        let config: Config = toml::from_str(
            r#"
            [memory]
            enabled = true
            "#,
        )
        .expect("parse config");
        let app = App::new(
            crate::tui::app::TuiOptions {
                screen_mode: crate::tui::app::ScreenMode::Inline,
                use_bracketed_paste: false,
                memory_path: memory_path.clone(),
                notes_path: tmp.path().join("notes.txt"),
                mcp_config_path: tmp.path().join("mcp.json"),
                use_memory: true,
                start_in_agent_mode: true,
                ..crate::test_support::test_tui_options(tmp.path())
            },
            &config,
        );

        let report = build_context_report(&app);
        let memory_entry = report
            .entries
            .iter()
            .find(|entry| entry.source_kind == SourceKind::UserMemory)
            .expect("user memory source entry");

        assert_eq!(memory_entry.activation_reason, ActivationReason::Omitted);
        assert!(!context_report_json(&report).contains("private legacy memory"));
    }

    #[test]
    fn headless_report_counts_project_pack_only_when_configured() {
        let tmp = tempdir().expect("tempdir");
        fs::create_dir(tmp.path().join(".git")).expect("mkdir .git");
        fs::create_dir(tmp.path().join("src")).expect("mkdir src");
        fs::write(tmp.path().join("src/lib.rs"), "pub fn fixture() {}\n").expect("write fixture");

        let default_report = build_headless_context_report(&Config::default(), tmp.path());
        let default_pack = default_report
            .entries
            .iter()
            .find(|entry| entry.source_kind == SourceKind::ProjectContextPack)
            .expect("project pack entry");
        assert_eq!(default_pack.activation_reason, ActivationReason::Omitted);
        assert_eq!(default_pack.estimated_tokens, 0);
        assert_eq!(
            default_pack.truncation_reason.as_deref(),
            Some("disabled; project_map provides this information on demand")
        );

        let relay = default_report
            .entries
            .iter()
            .find(|entry| entry.source_kind == SourceKind::CompactionRelayTemplate)
            .expect("relay template entry");
        assert_eq!(relay.activation_reason, ActivationReason::Omitted);
        assert_eq!(relay.estimated_tokens, 0);

        let mut configured = Config::default();
        configured.context.project_pack = Some(true);
        let configured_report = build_headless_context_report(&configured, tmp.path());
        let configured_pack = configured_report
            .entries
            .iter()
            .find(|entry| entry.source_kind == SourceKind::ProjectContextPack)
            .expect("configured project pack entry");
        assert_eq!(
            configured_pack.activation_reason,
            ActivationReason::ConfigEnabled
        );
        assert!(
            configured_pack.estimated_tokens > 0,
            "configured project pack must be counted"
        );

        let environment = configured_report
            .entries
            .iter()
            .find(|entry| entry.source_kind == SourceKind::EnvironmentBlock)
            .expect("runtime environment entry");
        assert_eq!(environment.activation_reason, ActivationReason::AlwaysOn);
    }

    #[test]
    fn app_context_report_counts_configured_project_pack_before_first_turn() {
        let tmp = tempdir().expect("tempdir");
        fs::create_dir(tmp.path().join(".git")).expect("mkdir .git");
        fs::create_dir(tmp.path().join("src")).expect("mkdir src");
        fs::write(tmp.path().join("src/lib.rs"), "pub fn fixture() {}\n").expect("write fixture");
        let mut config = Config::default();
        config.context.project_pack = Some(true);
        let app = App::new(
            crate::tui::app::TuiOptions {
                screen_mode: crate::tui::app::ScreenMode::Inline,
                use_bracketed_paste: false,
                notes_path: tmp.path().join("notes.txt"),
                mcp_config_path: tmp.path().join("mcp.json"),
                start_in_agent_mode: true,
                ..crate::test_support::test_tui_options(tmp.path())
            },
            &config,
        );

        assert!(
            app.system_prompt.is_none(),
            "fixture must be pre-first-turn"
        );
        let report = build_context_report(&app);
        let project_pack = report
            .entries
            .iter()
            .find(|entry| entry.source_kind == SourceKind::ProjectContextPack)
            .expect("project pack entry");
        assert_eq!(
            project_pack.activation_reason,
            ActivationReason::ConfigEnabled
        );
        assert!(project_pack.estimated_tokens > 0);
    }

    #[test]
    fn headless_context_report_omits_legacy_plain_file_memory() {
        let tmp = tempdir().expect("tempdir");
        let memory_path = tmp.path().join("memory.md");
        fs::write(&memory_path, "private legacy memory").expect("write memory");
        let mut config: Config = toml::from_str(
            r#"
            [memory]
            enabled = true
            "#,
        )
        .expect("parse config");
        config.memory_path = Some(memory_path.to_string_lossy().into_owned());

        let report = build_headless_context_report(&config, tmp.path());
        let memory_entry = report
            .entries
            .iter()
            .find(|entry| entry.source_kind == SourceKind::UserMemory)
            .expect("user memory source entry");

        assert_eq!(memory_entry.activation_reason, ActivationReason::Omitted);
        assert!(!context_report_json(&report).contains("private legacy memory"));
    }

    #[test]
    fn format_summary_lists_largest_sources() {
        let mut builder = ReportBuilder::new();
        builder.push(SourceEntry::estimate(
            SourceKind::ToolSchemas,
            "Tool schemas",
            None,
            ActivationReason::PerRequest,
            500,
            CountingConfidence::Approximate,
            Some(3),
        ));
        builder.push(SourceEntry::estimate(
            SourceKind::UserRequest,
            "Latest user request",
            None,
            ActivationReason::PerRequest,
            25,
            CountingConfidence::High,
            Some(7),
        ));
        let report = builder.finish(
            ContextWindowResolution {
                tokens: 128_000,
                source: ContextWindowSource::Fallback,
            },
            525,
            "test",
        );
        let summary = format_context_summary(&report);

        assert!(summary.contains("Context Summary"));
        assert!(summary.contains("Tool schemas (500)"));
    }

    #[test]
    fn finish_reflects_route_context_window_over_model_default() {
        // deepseek-v4-pro defaults to a 1M window; a resolved route advertising a
        // smaller window must win in the report's context_window_tokens.
        let route_window = 128_000u64;
        let model_default = crate::models::context_window_for_model("deepseek-v4-pro")
            .expect("model has a default window");
        assert_ne!(
            u64::from(model_default),
            route_window,
            "test fixture must differ from the model default to be meaningful"
        );

        let limits = RouteLimits {
            context_tokens: Some(route_window),
            input_tokens: None,
            output_tokens: None,
        };
        let resolved = crate::route_runtime::resolve_context_window(
            ApiProvider::Deepseek,
            "deepseek-v4-pro",
            Some(limits),
            None,
        );
        assert_eq!(resolved.source, ContextWindowSource::Catalog);

        let builder = ReportBuilder::new();
        let report = builder.finish(resolved, 10_000, "test");

        assert_eq!(report.context_window_tokens, Some(route_window as u32));
        assert_eq!(report.context_window_source.as_deref(), Some("catalog"));
        // Budget percent is computed against the route window, not the default.
        let expected = (10_000.0 / route_window as f64) * 100.0;
        let actual = report.budget_used_percent.expect("window known");
        assert!(
            (actual - expected).abs() < 1e-6,
            "got {actual}, want {expected}"
        );
    }

    #[test]
    fn pressure_label_matches_unified_pressure_levels() {
        // Boundaries mirror context_budget::PressureLevel.
        assert_eq!(pressure_label(None), "unknown");
        assert_eq!(pressure_label(Some(0.0)), "low");
        assert_eq!(pressure_label(Some(39.9)), "low");
        assert_eq!(pressure_label(Some(40.0)), "moderate");
        assert_eq!(pressure_label(Some(74.9)), "moderate");
        assert_eq!(pressure_label(Some(75.0)), "high");
        assert_eq!(pressure_label(Some(89.9)), "high");
        assert_eq!(pressure_label(Some(90.0)), "critical");
        assert_eq!(pressure_label(Some(100.0)), "critical");
    }

    #[test]
    fn tool_schema_entry_serializes_like_runtime_catalog() {
        let tool = Tool {
            tool_type: Some("function".to_string()),
            name: "read_file".to_string(),
            description: "read a file".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
            allowed_callers: None,
            defer_loading: None,
            input_examples: None,
            strict: Some(true),
            cache_control: None,
        };
        let rendered = serde_json::to_string(&vec![tool]).expect("serialize tool");

        assert!(rendered.contains("read_file"));
    }
}
