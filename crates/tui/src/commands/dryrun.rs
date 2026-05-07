use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::{CommandResult, auto_model_heuristic, normalize_auto_route_effort};
use crate::client::{api_url, build_sanitized_chat_completion_body};
use crate::compaction::estimate_tokens;
use crate::config::{ApiProvider, Config};
use crate::features::{Feature, Features};
use crate::localization::{MessageId, tr};
use crate::models::{
    ContentBlock, Message, MessageRequest, SystemPrompt, Tool, context_window_for_model,
};
use crate::tools::{ToolContext, ToolRegistryBuilder};
use crate::tui::app::{App, AppMode, ReasoningEffort};
use crate::tui::approval::ApprovalMode;

const API_MAX_OUTPUT_TOKENS: u32 = 65_536;
const TOOL_RESULT_PREVIEW_LIMIT: usize = 4_096;
const CODE_EXECUTION_TOOL_NAME: &str = "code_execution";
const CODE_EXECUTION_TOOL_TYPE: &str = "code_execution_20250825";
const TOOL_SEARCH_REGEX_NAME: &str = "tool_search_tool_regex";
const TOOL_SEARCH_REGEX_TYPE: &str = "tool_search_tool_regex_20251119";
const TOOL_SEARCH_BM25_NAME: &str = "tool_search_tool_bm25";
const TOOL_SEARCH_BM25_TYPE: &str = "tool_search_tool_bm25_20251119";

#[derive(Debug, Default)]
struct RoleCounts {
    system: usize,
    user: usize,
    assistant: usize,
    tool: usize,
    other: usize,
}

impl RoleCounts {
    fn total(&self) -> usize {
        self.system + self.user + self.assistant + self.tool + self.other
    }
}

struct DryrunPreview {
    provider: ApiProvider,
    base_url: String,
    url: String,
    headers: BTreeMap<String, String>,
    request: MessageRequest,
    body: Value,
    role_counts: RoleCounts,
    system_chars: usize,
    system_tokens: usize,
    composer_chars: usize,
    composer_tokens: usize,
    chat_tokens: usize,
    tool_schema_tokens: usize,
    estimated_total_tokens: usize,
    reasoning_replay_messages: usize,
    config_warning: Option<String>,
}

/// Preview the next Chat Completions request without sending it.
pub fn dryrun(app: &mut App, arg: Option<&str>) -> CommandResult {
    let full = arg
        .map(str::trim)
        .is_some_and(|arg| matches!(arg, "--full" | "full" | "-f"));

    let preview = build_preview(app);
    if full {
        CommandResult::message(format_full_preview(&preview, app.ui_locale))
    } else {
        CommandResult::message(format_summary(&preview, app.ui_locale))
    }
}

fn build_preview(app: &App) -> DryrunPreview {
    let (mut config, config_warning) = load_config_for_app(app);
    config.provider = Some(app.api_provider.as_str().to_string());

    let provider = app.api_provider;
    let base_url = config.deepseek_base_url();
    let url = api_url(&base_url, "chat/completions");
    let draft = app.composer.input.as_str();
    let messages = messages_with_composer_draft(&app.api_messages, draft);
    let model = effective_model(app, draft);
    let reasoning_effort = effective_reasoning_effort(app, &messages);
    let mut tools = active_tools_for_preview(app, &config.features(), &model);
    let strict_tool_mode = config.strict_tool_mode.unwrap_or(false);
    if strict_tool_mode {
        crate::tools::schema_sanitize::prepare_tools_for_strict_mode(&mut tools);
    }
    let tool_count = tools.len();
    let tool_schema_tokens = estimate_json_tokens(&json!(tools));
    let tools = (!tools.is_empty()).then_some(tools);

    let request = MessageRequest {
        model: model.clone(),
        messages,
        max_tokens: effective_max_output_tokens(&model),
        system: app.system_prompt.clone(),
        tools,
        tool_choice: if tool_count > 0 {
            if strict_tool_mode {
                Some(json!("required"))
            } else {
                Some(json!({ "type": "auto" }))
            }
        } else {
            None
        },
        metadata: None,
        thinking: None,
        reasoning_effort: reasoning_effort.api_value().map(str::to_string),
        stream: Some(true),
        temperature: None,
        top_p: None,
    };

    let (body, _) = build_sanitized_chat_completion_body(&request, provider, &base_url, true);
    let role_counts = role_counts_from_body(&body);
    let system_text = system_prompt_text(request.system.as_ref());
    let system_chars = system_text.chars().count();
    let system_tokens = estimate_text_tokens(&system_text);
    let composer_chars = draft.trim().chars().count();
    let composer_tokens = estimate_text_tokens(draft.trim());
    let chat_tokens = estimate_tokens(&request.messages);
    let estimated_total_tokens = chat_tokens + system_tokens + tool_schema_tokens;
    let reasoning_replay_messages = body
        .get("messages")
        .and_then(Value::as_array)
        .map(|messages| {
            messages
                .iter()
                .filter(|message| {
                    message.get("role").and_then(Value::as_str) == Some("assistant")
                        && message.get("tool_calls").is_some()
                        && message.get("reasoning_content").is_some()
                })
                .count()
        })
        .unwrap_or(0);

    DryrunPreview {
        provider,
        base_url,
        url,
        headers: redacted_headers(&config),
        request,
        body,
        role_counts,
        system_chars,
        system_tokens,
        composer_chars,
        composer_tokens,
        chat_tokens,
        tool_schema_tokens,
        estimated_total_tokens,
        reasoning_replay_messages,
        config_warning,
    }
}

fn format_summary(preview: &DryrunPreview, locale: crate::localization::Locale) -> String {
    let mut lines = Vec::new();
    lines.push("Dry run: next Chat Completions request (not sent)".to_string());
    lines.push(format!(
        "Provider: {}  URL: {}",
        preview.provider.as_str(),
        preview.url
    ));
    lines.push(format!("Base URL: {}", preview.base_url));
    lines.push(format!(
        "Model: {}  reasoning_effort: {}  max_tokens: {}",
        preview.request.model,
        preview
            .request
            .reasoning_effort
            .as_deref()
            .unwrap_or("none"),
        preview.request.max_tokens
    ));
    lines.push(format!(
        "Messages: system={} user={} assistant={} tool={} other={} total={}",
        preview.role_counts.system,
        preview.role_counts.user,
        preview.role_counts.assistant,
        preview.role_counts.tool,
        preview.role_counts.other,
        preview.role_counts.total()
    ));
    if preview.reasoning_replay_messages > 0 {
        lines.push(format!(
            "Reasoning replay: {} assistant tool-call message(s) include reasoning_content",
            preview.reasoning_replay_messages
        ));
    } else {
        lines.push("Reasoning replay: no assistant tool-call replay in this preview".to_string());
    }
    lines.push(format!(
        "System prompt: {} chars / ~{} tokens",
        preview.system_chars, preview.system_tokens
    ));
    lines.push(format!(
        "Tools: {} active schema(s) / ~{} schema tokens",
        preview.request.tools.as_ref().map_or(0, std::vec::Vec::len),
        preview.tool_schema_tokens
    ));
    lines.push(format!(
        "Composer draft: {} chars / ~{} tokens",
        preview.composer_chars, preview.composer_tokens
    ));
    lines.push(format!(
        "Estimated input: ~{} tokens (chat ~{}, system ~{}, tools ~{})",
        preview.estimated_total_tokens,
        preview.chat_tokens,
        preview.system_tokens,
        preview.tool_schema_tokens
    ));
    if let Some(warning) = &preview.config_warning {
        lines.push(format!("Config warning: {warning}"));
    }
    lines.push(tr(locale, MessageId::DryrunFooterApprox).to_string());
    lines.join("\n")
}

fn format_full_preview(preview: &DryrunPreview, locale: crate::localization::Locale) -> String {
    let mut body = preview.body.clone();
    truncate_tool_result_bodies(&mut body, TOOL_RESULT_PREVIEW_LIMIT);
    let manifest = json!({
        "method": "POST",
        "url": preview.url,
        "headers": preview.headers,
        "body": body,
    });
    let json = serde_json::to_string_pretty(&manifest).unwrap_or_else(|_| manifest.to_string());
    format!(
        "Full request preview (not sent):\n```json\n{json}\n```\n{}",
        tr(locale, MessageId::DryrunFooterApprox)
    )
}

fn load_config_for_app(app: &App) -> (Config, Option<String>) {
    match Config::load(app.config_path.clone(), app.config_profile.as_deref()) {
        Ok(config) => (config, None),
        Err(err) => (
            Config::default(),
            Some(format!(
                "using defaults because config failed to load: {err}"
            )),
        ),
    }
}

fn effective_model(app: &App, draft: &str) -> String {
    if app.auto_model {
        return auto_model_heuristic(draft, &app.model);
    }
    app.model.clone()
}

fn effective_reasoning_effort(app: &App, messages: &[Message]) -> ReasoningEffort {
    if app.reasoning_effort != ReasoningEffort::Auto {
        return app.reasoning_effort;
    }
    let latest_user_text = messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(message_text)
        .unwrap_or_default();
    normalize_auto_route_effort(crate::auto_reasoning::select(false, &latest_user_text))
}

fn effective_max_output_tokens(model: &str) -> u32 {
    let window = context_window_for_model(model).unwrap_or(128_000);
    if window >= 500_000 {
        API_MAX_OUTPUT_TOKENS
    } else {
        (window / 2).min(API_MAX_OUTPUT_TOKENS)
    }
}

fn active_tools_for_preview(app: &App, features: &Features, model: &str) -> Vec<Tool> {
    let notes_path = app.workspace.join(".deepseek").join("notes.md");
    let mut context = ToolContext::with_auto_approve(
        app.workspace.clone(),
        app.trust_mode,
        notes_path,
        app.mcp_config_path.clone(),
        app.mode == AppMode::Yolo || matches!(app.approval_mode, ApprovalMode::Auto),
    )
    .with_runtime_services(app.runtime_services.clone())
    .with_features(features.clone())
    .with_state_namespace(
        app.current_session_id
            .clone()
            .unwrap_or_else(|| "workspace".to_string()),
    );
    if let Some(shell_manager) = app.runtime_services.shell_manager.clone() {
        context = context.with_shell_manager(shell_manager);
    }
    if app.use_memory {
        context.memory_path = Some(app.memory_path.clone());
    }

    let mut builder = if app.mode == AppMode::Plan {
        ToolRegistryBuilder::new()
            .with_read_only_file_tools()
            .with_search_tools()
            .with_git_tools()
            .with_git_history_tools()
            .with_diagnostics_tool()
            .with_skill_tools()
            .with_validation_tools()
            .with_runtime_task_tools()
            .with_todo_tool(app.todos.clone())
            .with_plan_tool(app.plan_state.clone())
    } else {
        ToolRegistryBuilder::new()
            .with_agent_tools(app.allow_shell)
            .with_todo_tool(app.todos.clone())
            .with_plan_tool(app.plan_state.clone())
    };

    builder = builder
        .with_review_tool(None, model.to_string())
        .with_rlm_tool(None, model.to_string())
        .with_fim_tool(None, model.to_string())
        .with_user_input_tool()
        .with_parallel_tool();
    if features.enabled(Feature::ApplyPatch) && app.mode != AppMode::Plan {
        builder = builder.with_patch_tools();
    }
    if features.enabled(Feature::WebSearch) {
        builder = builder.with_web_tools();
    }
    if features.enabled(Feature::ShellTool) && app.allow_shell {
        builder = builder.with_shell_tools();
    }
    if app.use_memory {
        builder = builder.with_remember_tool();
    }

    let registry = builder.build(context);
    let mut catalog = registry.to_api_tools_with_cache(true);
    apply_native_tool_deferral(&mut catalog, app.mode);
    catalog.sort_by(|a, b| a.name.cmp(&b.name));
    ensure_advanced_tooling(&mut catalog);
    active_tools_from_catalog(
        &catalog,
        should_force_update_plan_first(app.mode, app.composer.input.as_str()),
    )
}

fn should_default_defer_tool(name: &str, mode: AppMode) -> bool {
    if mode == AppMode::Yolo {
        return false;
    }

    let always_loaded_in_action_modes = matches!(mode, AppMode::Agent)
        && matches!(
            name,
            "exec_shell"
                | "exec_shell_wait"
                | "exec_shell_interact"
                | "exec_wait"
                | "exec_interact"
        );
    if always_loaded_in_action_modes {
        return false;
    }

    !matches!(
        name,
        "read_file"
            | "list_dir"
            | "grep_files"
            | "file_search"
            | "diagnostics"
            | "rlm"
            | "recall_archive"
            | "update_plan"
            | "checklist_write"
            | "todo_write"
            | "task_create"
            | "task_list"
            | "task_read"
            | "task_gate_run"
            | "task_shell_start"
            | "task_shell_wait"
            | "github_issue_context"
            | "github_pr_context"
            | "request_user_input"
    )
}

fn apply_native_tool_deferral(catalog: &mut [Tool], mode: AppMode) {
    for tool in catalog {
        tool.defer_loading = Some(should_default_defer_tool(&tool.name, mode));
    }
}

fn active_tools_from_catalog(catalog: &[Tool], force_update_plan: bool) -> Vec<Tool> {
    if force_update_plan {
        let forced: Vec<_> = catalog
            .iter()
            .filter(|tool| tool.name == "update_plan")
            .cloned()
            .collect();
        if !forced.is_empty() {
            return forced;
        }
    }

    let mut active = Vec::new();
    for tool in catalog {
        if !tool.defer_loading.unwrap_or(false) || is_tool_search_tool(&tool.name) {
            active.push(tool.clone());
        }
    }
    if active.is_empty()
        && !catalog.is_empty()
        && let Some(first) = catalog.first()
    {
        active.push(first.clone());
    }
    active
}

fn is_tool_search_tool(name: &str) -> bool {
    matches!(name, TOOL_SEARCH_REGEX_NAME | TOOL_SEARCH_BM25_NAME)
}

fn ensure_advanced_tooling(catalog: &mut Vec<Tool>) {
    if !catalog.iter().any(|t| t.name == CODE_EXECUTION_TOOL_NAME) {
        catalog.push(Tool {
            tool_type: Some(CODE_EXECUTION_TOOL_TYPE.to_string()),
            name: CODE_EXECUTION_TOOL_NAME.to_string(),
            description:
                "Execute Python code in a local sandboxed runtime and return stdout/stderr/return_code as JSON."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "code": { "type": "string", "description": "Python source code to execute." }
                },
                "required": ["code"]
            }),
            allowed_callers: Some(vec!["direct".to_string()]),
            defer_loading: Some(false),
            input_examples: None,
            strict: None,
            cache_control: None,
        });
    }

    if !catalog.iter().any(|t| t.name == TOOL_SEARCH_REGEX_NAME) {
        catalog.push(Tool {
            tool_type: Some(TOOL_SEARCH_REGEX_TYPE.to_string()),
            name: TOOL_SEARCH_REGEX_NAME.to_string(),
            description:
                "Search deferred tool definitions using a regex query and return matching tool references."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Regex pattern to search tool names/descriptions/schema." }
                },
                "required": ["query"]
            }),
            allowed_callers: Some(vec!["direct".to_string()]),
            defer_loading: Some(false),
            input_examples: None,
            strict: None,
            cache_control: None,
        });
    }

    if !catalog.iter().any(|t| t.name == TOOL_SEARCH_BM25_NAME) {
        catalog.push(Tool {
            tool_type: Some(TOOL_SEARCH_BM25_TYPE.to_string()),
            name: TOOL_SEARCH_BM25_NAME.to_string(),
            description:
                "Search deferred tool definitions using natural-language matching and return matching tool references."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Natural language query for tool discovery." }
                },
                "required": ["query"]
            }),
            allowed_callers: Some(vec!["direct".to_string()]),
            defer_loading: Some(false),
            input_examples: None,
            strict: None,
            cache_control: None,
        });
    }
}

fn should_force_update_plan_first(mode: AppMode, content: &str) -> bool {
    if mode != AppMode::Plan {
        return false;
    }

    let lower = content.to_ascii_lowercase();
    let asks_for_direct_plan = [
        "quick plan",
        "short plan",
        "simple plan",
        "3-step plan",
        "3 step plan",
        "three-step plan",
        "three step plan",
        "high-level plan",
        "high level plan",
        "give me a plan",
        "make a plan",
        "outline a plan",
        "draft a plan",
    ]
    .iter()
    .any(|needle| lower.contains(needle));

    if !asks_for_direct_plan {
        return false;
    }

    let asks_for_repo_exploration = [
        "inspect the repo",
        "inspect the code",
        "explore the repo",
        "search the repo",
        "read the code",
        "review the code",
        "analyze the code",
        "investigate",
        "look through",
        "understand the current",
        "ground it in the codebase",
        "based on the codebase",
    ]
    .iter()
    .any(|needle| lower.contains(needle));

    !asks_for_repo_exploration
}

fn role_counts_from_body(body: &Value) -> RoleCounts {
    let mut counts = RoleCounts::default();
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for message in messages {
            match message.get("role").and_then(Value::as_str) {
                Some("system") => counts.system += 1,
                Some("user") => counts.user += 1,
                Some("assistant") => counts.assistant += 1,
                Some("tool") => counts.tool += 1,
                _ => counts.other += 1,
            }
        }
    }
    counts
}

pub(crate) fn messages_with_composer_draft(messages: &[Message], draft: &str) -> Vec<Message> {
    let mut out = messages.to_vec();
    if !draft.trim().is_empty() {
        out.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: draft.to_string(),
                cache_control: None,
            }],
        });
    }
    out
}

fn message_text(message: &Message) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn system_prompt_text(system: Option<&SystemPrompt>) -> String {
    match system {
        Some(SystemPrompt::Text(text)) => text.clone(),
        Some(SystemPrompt::Blocks(blocks)) => blocks
            .iter()
            .map(|block| block.text.clone())
            .collect::<Vec<_>>()
            .join("\n\n---\n\n"),
        None => String::new(),
    }
}

fn estimate_text_tokens(text: &str) -> usize {
    let chars = text.chars().count();
    if chars == 0 {
        0
    } else {
        chars.div_ceil(4).max(1)
    }
}

fn estimate_json_tokens(value: &Value) -> usize {
    estimate_text_tokens(&value.to_string())
}

fn redacted_headers(config: &Config) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    headers.insert("content-type".to_string(), "application/json".to_string());
    if let Ok(api_key) = config.deepseek_api_key()
        && !api_key.trim().is_empty()
    {
        headers.insert(
            "authorization".to_string(),
            format!("Bearer {}", redact_secret_value(&api_key)),
        );
    }
    for (name, value) in config.http_headers() {
        let normalized = name.trim().to_ascii_lowercase();
        if normalized == "authorization" || normalized == "content-type" {
            continue;
        }
        headers.insert(name, redact_header_value(&normalized, &value));
    }
    headers
}

fn redact_header_value(normalized_name: &str, value: &str) -> String {
    if normalized_name.contains("authorization")
        || normalized_name.contains("api-key")
        || normalized_name.contains("apikey")
        || normalized_name.contains("token")
        || normalized_name == "key"
    {
        return redact_secret_value(value);
    }
    value.to_string()
}

pub(crate) fn redact_secret_value(value: &str) -> String {
    let trimmed = value.trim();
    if let Some(token) = trimmed.strip_prefix("Bearer ") {
        return format!("Bearer {}", redact_secret_value(token));
    }
    let last4: String = trimmed
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if trimmed.starts_with("sk-") {
        format!("sk-...{last4}")
    } else if last4.is_empty() {
        "...".to_string()
    } else {
        format!("...{last4}")
    }
}

pub(crate) fn truncate_tool_result_bodies(value: &mut Value, limit: usize) {
    let Some(messages) = value.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    for message in messages {
        if message.get("role").and_then(Value::as_str) != Some("tool") {
            continue;
        }
        let Some(content) = message.get_mut("content") else {
            continue;
        };
        if let Some(text) = content.as_str()
            && text.chars().count() > limit
        {
            *content = Value::String(truncate_text_with_marker(text, limit));
        }
    }
}

fn truncate_text_with_marker(text: &str, limit: usize) -> String {
    let kept: String = text.chars().take(limit).collect();
    let omitted = text.chars().count().saturating_sub(limit);
    format!("{kept}\n[truncated {omitted} chars]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::models::{ContentBlock, Message};
    use crate::tui::app::{App, TuiOptions};
    use std::path::PathBuf;

    fn create_test_app() -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: PathBuf::from("."),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: PathBuf::from("."),
            memory_path: PathBuf::from("memory.md"),
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
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
    fn dryrun_summary_appends_composer_draft_as_synthetic_user_turn() {
        let messages = vec![Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: "ready".to_string(),
                cache_control: None,
            }],
        }];

        let with_draft = messages_with_composer_draft(&messages, "next request");

        assert_eq!(with_draft.len(), 2);
        assert_eq!(with_draft[1].role, "user");
        assert!(matches!(
            &with_draft[1].content[0],
            ContentBlock::Text { text, .. } if text == "next request"
        ));
    }

    #[test]
    fn dryrun_full_redacts_api_key_to_last4() {
        assert_eq!(redact_secret_value("sk-1234567890abcdef"), "sk-...cdef");
    }

    #[test]
    fn dryrun_full_truncates_long_tool_result_bodies() {
        let mut value = serde_json::json!({
            "messages": [
                { "role": "tool", "content": "abcdef" }
            ]
        });

        truncate_tool_result_bodies(&mut value, 3);

        assert_eq!(value["messages"][0]["content"], "abc\n[truncated 3 chars]");
    }

    #[test]
    fn dryrun_summary_includes_model_provider_and_token_estimate() {
        let mut app = create_test_app();
        app.composer.input = "hello".to_string();

        let result = dryrun(&mut app, None);
        let message = result.message.expect("dryrun should render summary");

        assert!(message.contains("Provider: deepseek"));
        assert!(message.contains("Model: deepseek-v4-pro"));
        assert!(message.contains("Estimated input:"));
    }

    #[test]
    fn dryrun_summary_does_not_mutate_app_state() {
        let mut app = create_test_app();
        app.composer.input = "next request".to_string();
        app.api_messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "existing".to_string(),
                cache_control: None,
            }],
        });
        let before_messages = app.api_messages.clone();
        let before_input = app.composer.input.clone();
        let before_total = app.session.total_tokens;

        let _ = dryrun(&mut app, None);

        assert_eq!(app.api_messages, before_messages);
        assert_eq!(app.composer.input, before_input);
        assert_eq!(app.session.total_tokens, before_total);
    }

    #[test]
    fn dryrun_handles_empty_session_without_panic() {
        let mut app = create_test_app();

        let result = dryrun(&mut app, None);

        assert!(result.message.expect("summary").contains("Messages:"));
    }
}
