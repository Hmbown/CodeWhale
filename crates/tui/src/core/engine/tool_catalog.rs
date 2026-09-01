//! Deferred tool catalog and built-in advanced tool helpers.
//!
//! The streaming turn loop owns when tools are offered or executed. This module
//! owns the catalog-level policy around deferred loading, tool search, missing
//! tool suggestions, and the small set of built-in advanced tools that are not
//! registered by the normal runtime tool registry.

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

#[cfg(test)]
use crate::mcp::McpPool;
use crate::model_profile::ToolSurfaceBudget;
use crate::models::Tool;
use crate::tools::spec::{ToolError, ToolResult, optional_str, optional_u64, required_str};
use crate::tui::app::AppMode;

use crate::core::session::ToolActivationCache;
use crate::dependencies::ExternalTool;
use crate::regex_cache::compile_user_regex;

pub(super) const MULTI_TOOL_PARALLEL_NAME: &str = "multi_tool_use.parallel";
pub(super) const REQUEST_USER_INPUT_NAME: &str = "request_user_input";
pub(super) const CODE_EXECUTION_TOOL_NAME: &str = "code_execution";
const CODE_EXECUTION_TOOL_TYPE: &str = "code_execution_20250825";
const CODE_EXECUTION_DESCRIPTION: &str = "Execute Python code with the local Python interpreter in the workspace and return stdout/stderr/return_code as JSON.";
pub(super) use crate::tools::js_execution::JS_EXECUTION_TOOL_NAME;
pub(crate) const TOOL_SEARCH_NAME: &str = "tool_search";
const TOOL_RESULT_RETRIEVAL_NAME: &str = "retrieve_tool_result";
const TOOL_SEARCH_TYPE: &str = "tool_search_20251119";
const LEGACY_TOOL_SEARCH_REGEX_NAME: &str = "tool_search_tool_regex";
const LEGACY_TOOL_SEARCH_BM25_NAME: &str = "tool_search_tool_bm25";
const TOOL_SEARCH_DEFAULT_MAX_RESULTS: usize = 8;
const TOOL_SEARCH_MAX_RESULTS_LIMIT: usize = 8;

pub(crate) fn is_tool_search_tool(name: &str) -> bool {
    matches!(
        name,
        TOOL_SEARCH_NAME | LEGACY_TOOL_SEARCH_REGEX_NAME | LEGACY_TOOL_SEARCH_BM25_NAME
    )
}

// Crate-visible so the hook gate tests the real eager names instead of a copy.
#[rustfmt::skip]
pub(crate) const DEFAULT_ACTIVE_NATIVE_TOOLS: &[&str] = &[
    // Specialized native, MCP, plugin, and durable-work tools stay searchable.
    "read", "write", "edit", "bash", "agent", "todo_write",
];

const CORE_ACTION_TOOL_FALLBACKS: &[CoreActionToolFallback] = &[
    CoreActionToolFallback {
        name: "bash",
        description: "Run shell commands in the workspace.",
        unavailable_reason: "Not present in the current model-visible catalog. The session profile, feature availability, or a command tool allow/deny gate can remove shell access. Plan keeps the same primitive identity but centrally refuses execution.",
    },
    CoreActionToolFallback {
        name: "read",
        description: "Read workspace files.",
        unavailable_reason: "Not present in the current model-visible catalog. File reads are available in Plan and executable modes unless a command allow/deny gate removes them.",
    },
    CoreActionToolFallback {
        name: "write",
        description: "Create or replace workspace files.",
        unavailable_reason: "Not present in the current model-visible catalog. Plan mode has no file-mutation authority; switch to Work mode before writing.",
    },
    CoreActionToolFallback {
        name: "edit",
        description: "Apply exact replacements to workspace files.",
        unavailable_reason: "Not present in the current model-visible catalog. Plan mode has no file-mutation authority; switch to Work mode before editing.",
    },
];

#[derive(Debug, Clone, Copy)]
struct CoreActionToolFallback {
    name: &'static str,
    description: &'static str,
    unavailable_reason: &'static str,
}

/// Pre-computed lowercased haystack + name for each fallback; built once.
struct CachedFallback {
    fallback: CoreActionToolFallback,
    haystack: String,
    name_lower: String,
}

static CACHED_FALLBACKS: std::sync::OnceLock<Vec<CachedFallback>> = std::sync::OnceLock::new();

fn cached_fallbacks() -> &'static [CachedFallback] {
    CACHED_FALLBACKS.get_or_init(|| {
        CORE_ACTION_TOOL_FALLBACKS
            .iter()
            .map(|f| CachedFallback {
                fallback: *f,
                haystack: format!(
                    "{}\n{}\n{}",
                    f.name.to_lowercase(),
                    f.description.to_lowercase(),
                    f.unavailable_reason.to_lowercase(),
                ),
                name_lower: f.name.to_lowercase(),
            })
            .collect()
    })
}

/// Membership index over [`DEFAULT_ACTIVE_NATIVE_TOOLS`], built once for the
/// process lifetime. The array stays the source of truth for ordered
/// inspection; this set only accelerates the
/// hot membership check in [`should_default_defer_tool`], which runs once per
/// catalog tool on every catalog rebuild (i.e. per turn) — an O(n·m) linear
/// scan over the array collapses to O(1) hashed lookups.
static DEFAULT_ACTIVE_NATIVE_TOOLS_SET: std::sync::OnceLock<HashSet<&'static str>> =
    std::sync::OnceLock::new();

fn default_active_native_tools_set() -> &'static HashSet<&'static str> {
    DEFAULT_ACTIVE_NATIVE_TOOLS_SET
        .get_or_init(|| DEFAULT_ACTIVE_NATIVE_TOOLS.iter().copied().collect())
}

pub(super) fn should_default_defer_tool(name: &str, always_load: &HashSet<String>) -> bool {
    if always_load.contains(name) {
        return false;
    }

    if is_tool_search_tool(name) {
        return false;
    }

    // Membership-only test (no ordering dependency): the side set built from
    // DEFAULT_ACTIVE_NATIVE_TOOLS returns identical hit/miss results as the
    // former `.iter().any(...)` linear scan.
    !default_active_native_tools_set().contains(name)
}

pub(crate) fn apply_native_tool_deferral(catalog: &mut [Tool], always_load: &HashSet<String>) {
    for tool in catalog {
        tool.defer_loading = Some(should_default_defer_tool(&tool.name, always_load));
    }
}

pub(super) fn apply_mcp_tool_deferral(
    catalog: &mut [Tool],
    _mode: AppMode,
    always_load: &HashSet<String>,
) {
    for tool in catalog {
        if always_load.contains(&tool.name) {
            tool.defer_loading = Some(false);
            continue;
        }
        tool.defer_loading = Some(true);
    }
}

/// Build the model tool catalog from native and MCP tool lists.
///
/// **Catalog-head stability invariant.** The head of the catalog (all
/// non-deferred tools) must remain byte-identical across mode toggles
/// (Plan ↔ Agent ↔ YOLO) for tools that are common to both modes.
/// Deferred tool activations append to the tail and never reorder the
/// head. This invariant is critical for DeepSeek's KV prefix cache:
/// the tools array is part of the immutable prefix, and any byte-level
/// change in the head forces a full re-prefill on the next turn.
#[cfg(test)]
pub(super) fn build_model_tool_catalog(
    native_tools: Vec<Tool>,
    mcp_tools: Vec<Tool>,
    mode: AppMode,
    always_load: &HashSet<String>,
) -> Vec<Tool> {
    build_model_tool_catalog_with_surface(
        native_tools,
        mcp_tools,
        mode,
        always_load,
        ToolSurfaceBudget::Standard,
    )
}

pub(super) fn build_model_tool_catalog_with_surface(
    mut native_tools: Vec<Tool>,
    mut mcp_tools: Vec<Tool>,
    mode: AppMode,
    always_load: &HashSet<String>,
    surface_budget: ToolSurfaceBudget,
) -> Vec<Tool> {
    apply_native_tool_deferral(&mut native_tools, always_load);
    apply_mcp_tool_deferral(&mut mcp_tools, mode, always_load);
    apply_tool_surface_budget(&mut native_tools, surface_budget, always_load);
    apply_tool_surface_budget(&mut mcp_tools, surface_budget, always_load);
    // Sort each partition by name for prefix-cache stability (#263). The
    // upstream `to_api_tools()` already sorts the registry's HashMap output;
    // this catalog is built from caller-supplied Vecs which the test harness
    // and (future) caller refactors may not pre-sort. Built-ins stay as a
    // contiguous prefix ahead of MCP tools so adding/removing an MCP tool
    // never shifts a built-in's position.
    native_tools.sort_by(|a, b| a.name.cmp(&b.name));
    mcp_tools.sort_by(|a, b| a.name.cmp(&b.name));
    native_tools.extend(mcp_tools);
    native_tools
}

const REGISTRY_FIRST_SHELL_GUIDANCE: &str = "Before using this tool for a task whose core operation is a specialized capability (for example media or document conversion, data transformation, browser automation, database or service access, or a developer utility), call registry_sync with a query describing that capability; it returns at most eight scored matches from the host-side Registry snapshot. If a returned match plausibly covers the operation, call start_registry_mcp_server and inspect the connected tools before using a shell alternative. Use the shell directly for ordinary repo-native work and simple file operations, or after no match (or one refined query) is plausible or the matching server fails to start.";

/// Put the Registry-first decision at the point where the model considers its
/// strongest fallback. The discovery skill body is lazy-loaded, so relying on
/// it alone creates a loop: the model must already prefer discovery before it
/// can read the instruction that tells it to prefer discovery.
///
/// This is applied only while MCP is enabled. It changes no dispatch order and
/// performs no task matching in the host; the model still compares the user's
/// context against the Registry catalog itself.
pub(super) fn apply_registry_first_shell_guidance(catalog: &mut [Tool]) {
    // The small-contract-shaped lowercase bash schema stays small and direct. This legacy
    // compatibility hook is intentionally inert unless an old model-visible
    // exec_shell definition is present.
    let Some(shell) = catalog.iter_mut().find(|tool| tool.name == "exec_shell") else {
        return;
    };
    if shell.description.contains(REGISTRY_FIRST_SHELL_GUIDANCE) {
        return;
    }
    if !shell.description.ends_with(char::is_whitespace) {
        shell.description.push(' ');
    }
    shell.description.push_str(REGISTRY_FIRST_SHELL_GUIDANCE);
}

fn apply_tool_surface_budget(
    catalog: &mut [Tool],
    surface_budget: ToolSurfaceBudget,
    always_load: &HashSet<String>,
) {
    if !matches!(surface_budget, ToolSurfaceBudget::Compact) {
        return;
    }
    for tool in catalog {
        if always_load.contains(&tool.name) {
            continue;
        }
        if matches!(tool.name.as_str(), "Run" | "tasks" | "Web") {
            tool.defer_loading = Some(true);
        }
    }
}

/// Whether two tool-surface budgets currently produce the same catalog.
///
/// Runs [`apply_tool_surface_budget`] over `catalog` under both budgets and
/// compares the results. `/preview-request` publishes the Standard-vs-Full
/// answer as a derived field so the truthful "these are currently collapsed"
/// disclosure cannot drift from the code: the day the shaper narrows Standard
/// differently from Full, this starts returning `false` on its own.
pub(super) fn surface_budgets_produce_same_catalog(
    catalog: &[Tool],
    always_load: &HashSet<String>,
    left_budget: ToolSurfaceBudget,
    right_budget: ToolSurfaceBudget,
) -> bool {
    let mut left = catalog.to_vec();
    let mut right = catalog.to_vec();
    apply_tool_surface_budget(&mut left, left_budget, always_load);
    apply_tool_surface_budget(&mut right, right_budget, always_load);
    serde_json::to_string(&left).ok() == serde_json::to_string(&right).ok()
}

pub(crate) fn ensure_advanced_tooling(
    catalog: &mut Vec<Tool>,
    mode: AppMode,
    always_load: &HashSet<String>,
) {
    // code_execution depends on a locally-installed Python interpreter
    // (python3 / python / py -3). Before v0.8.31, the tool was always
    // advertised and would fail at execution time on Windows where
    // `python3` isn't on PATH — the model treated the tool as reliable
    // once it appeared in the catalog. We now probe at catalog-build
    // time and only advertise when an interpreter resolves. See
    // `crate::dependencies::resolve_python_interpreter` for the probe.
    if mode != AppMode::Plan
        && !catalog.iter().any(|t| t.name == CODE_EXECUTION_TOOL_NAME)
        && crate::dependencies::resolve_python_interpreter().is_some()
    {
        catalog.push(Tool {
            tool_type: Some(CODE_EXECUTION_TOOL_TYPE.to_string()),
            name: CODE_EXECUTION_TOOL_NAME.to_string(),
            description: CODE_EXECUTION_DESCRIPTION.to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "code": { "type": "string", "description": "Python source code to execute." }
                },
                "required": ["code"]
            }),
            allowed_callers: Some(vec!["direct".to_string()]),
            defer_loading: Some(should_default_defer_tool(
                CODE_EXECUTION_TOOL_NAME,
                always_load,
            )),
            input_examples: None,
            strict: None,
            cache_control: None,
        });
    }

    // js_execution mirrors code_execution: gate on Node.js being
    // present locally so the model never sees a runtime it can't
    // actually use. Plan mode hides shell/exec surfaces (including
    // both interpreter tools) by construction; Agent / YOLO advertise
    // the tool only when `resolve_node()` succeeds.
    if mode != AppMode::Plan
        && !catalog.iter().any(|t| t.name == JS_EXECUTION_TOOL_NAME)
        && crate::dependencies::resolve_node().is_some()
    {
        let mut tool = crate::tools::js_execution::js_execution_tool_definition();
        tool.defer_loading = Some(should_default_defer_tool(&tool.name, always_load));
        catalog.push(tool);
    }

    if !catalog.iter().any(|t| t.name == TOOL_SEARCH_NAME) {
        catalog.push(Tool {
            tool_type: Some(TOOL_SEARCH_TYPE.to_string()),
            name: TOOL_SEARCH_NAME.to_string(),
            description: "Search deferred tool definitions and return matching tool references.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query for tool discovery." },
                    "match": {
                        "type": "string",
                        "enum": ["bm25", "regex"],
                        "default": "bm25",
                        "description": "Matching algorithm: bm25 for natural-language matching, regex for a regular expression over tool names/descriptions/schema."
                    },
                    "max_results": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": TOOL_SEARCH_MAX_RESULTS_LIMIT,
                        "default": TOOL_SEARCH_DEFAULT_MAX_RESULTS,
                        "description": "Maximum number of matching tool references to return."
                    }
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

pub(crate) fn initial_active_tools(catalog: &[Tool]) -> HashSet<String> {
    let mut active = HashSet::new();
    for tool in catalog {
        if !tool.defer_loading.unwrap_or(false) || is_tool_search_tool(&tool.name) {
            active.insert(tool.name.clone());
        }
    }
    if active.is_empty()
        && !catalog.is_empty()
        && let Some(first) = catalog.first()
    {
        active.insert(first.name.clone());
    }
    active
}

/// Remove schemas evicted from the conversation cache without hiding tools
/// that the current catalog now exposes eagerly.
///
/// A cached tool can become eager after an explicit `tools_always_load`
/// change or another policy update. Cache revalidation correctly forgets the
/// old deferred entry, but the eager catalog entry must remain active.
pub(crate) fn remove_evicted_cache_activations(
    catalog: &[Tool],
    active: &mut HashSet<String>,
    evicted: impl IntoIterator<Item = String>,
) {
    for name in evicted {
        let is_eager_now = catalog
            .iter()
            .any(|tool| tool.name == name && !tool.defer_loading.unwrap_or(false));
        if !is_eager_now {
            active.remove(&name);
        }
    }
}

/// Promote a successfully executed deferred tool only when this conversation
/// had already activated it. Execution can update recency, never grant a name.
pub(crate) fn touch_cached_tool_after_execution(
    catalog: &[Tool],
    active: &mut HashSet<String>,
    cache: &mut ToolActivationCache,
    name: &str,
) -> bool {
    if !cache.names().any(|cached| cached == name) {
        return false;
    }
    let delta = cache.activate(catalog, &[name.to_string()]);
    remove_evicted_cache_activations(catalog, active, delta.evicted);
    active.extend(delta.admitted);
    true
}

/// Make the recovery schema visible on the next provider step when a tool
/// result actually publishes retrievable evidence. The initial toolbox stays
/// small, while a receipt never advertises a deferred route that merely asks
/// the model to repeat the same call.
pub(crate) fn activate_result_dependencies(
    catalog: &[Tool],
    active: &mut HashSet<String>,
    cache: &mut ToolActivationCache,
    result: &ToolResult,
) -> bool {
    let needs_retrieval = result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("evidence_available"))
        .and_then(Value::as_bool)
        == Some(true);
    if !needs_retrieval {
        return false;
    }
    let delta = cache.activate(catalog, &[TOOL_RESULT_RETRIEVAL_NAME.to_string()]);
    remove_evicted_cache_activations(catalog, active, delta.evicted.iter().cloned());
    active.extend(delta.admitted.iter().cloned());
    !delta.admitted.is_empty() || !delta.evicted.is_empty()
}

fn active_tool_list_from_catalog(catalog: &[Tool], active: &HashSet<String>) -> Vec<Tool> {
    // Two-pass for prefix-cache stability (#263). Always-loaded tools come
    // first in their stable catalog order; tools that started life deferred
    // and were activated mid-conversation by ToolSearch get appended at the
    // tail. Otherwise activating a deferred tool shifts every later tool's
    // byte offset and busts the cached prefix from that point onwards.
    let catalog_len = catalog.len();
    let mut head: Vec<Tool> = Vec::with_capacity(catalog_len);
    let mut tail: Vec<Tool> = Vec::with_capacity(catalog_len);
    for tool in catalog {
        if !active.contains(&tool.name) {
            continue;
        }
        if tool.defer_loading.unwrap_or(false) {
            tail.push(tool.clone());
        } else {
            head.push(tool.clone());
        }
    }
    head.extend(tail);
    head
}

pub(super) fn active_tools_for_step(catalog: &[Tool], active: &HashSet<String>) -> Vec<Tool> {
    active_tool_list_from_catalog(catalog, active)
}

/// One turn's executable and model-visible tool contract.
///
/// The catalog is also the prompt's only availability taxonomy: stable prompt
/// prose deliberately does not enumerate tool names. Keeping the concrete
/// registry, searchable catalog, initial request subset, and command gates in
/// one value prevents preview, dispatch, and execution from reconstructing
/// different surfaces from mutable engine configuration.
pub(super) struct ToolSurfacePolicy {
    /// Runtime registry that executes native and plugin tools.
    pub(super) registry: crate::tools::ToolRegistry,
    /// Full model-facing catalog, including deferred entries.
    pub(super) catalog: Vec<Tool>,
    /// Names active at the start of the turn.
    pub(super) active_names: HashSet<String>,
    /// Exact initial `tools` field. `None` means no field is sent.
    pub(super) active: Option<Vec<Tool>>,
    pub(super) mode: AppMode,
    pub(super) strict_tool_mode: bool,
    allowed_tools: Option<Vec<String>>,
    disallowed_tools: Option<Vec<String>>,
    /// Hard per-turn cap on admitted tool calls (#4415). The turn loop copies
    /// this limit into its own admission counter at turn start; `None` means
    /// unlimited (the default), which keeps the admission gate inert.
    pub(super) max_tool_calls: Option<u32>,
    questions_allowed: bool,
}

impl ToolSurfacePolicy {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        registry: crate::tools::ToolRegistry,
        tools: Option<Vec<Tool>>,
        mode: AppMode,
        always_load: &HashSet<String>,
        dynamic_active_tools: &[&'static str],
        strict_tool_mode: bool,
        allowed_tools: Option<Vec<String>>,
        disallowed_tools: Option<Vec<String>>,
        max_tool_calls: Option<u32>,
        approval_mode: crate::tui::approval::ApprovalMode,
    ) -> Self {
        let mut catalog = tools.unwrap_or_default();
        if !catalog.is_empty() {
            ensure_advanced_tooling(&mut catalog, mode, always_load);
        }

        // Synthetic tools are injected before narrowing. Doing this after the
        // retain would re-advertise tool_search/code execution despite an
        // explicit command gate.
        catalog.retain(|tool| {
            !tool_denied(disallowed_tools.as_deref(), &tool.name)
                && tool_allowed(allowed_tools.as_deref(), &tool.name)
        });
        let questions_allowed =
            super::super::authority::permission_posture_allows_questions(approval_mode);
        if !questions_allowed {
            catalog.retain(|tool| tool.name != REQUEST_USER_INPUT_NAME);
        }

        let mut active_names = initial_active_tools(&catalog);
        active_names.extend(dynamic_active_tools.iter().map(|name| (*name).to_string()));
        active_names.retain(|name| catalog.iter().any(|tool| tool.name == *name));
        let active = active_tools_for_request(&catalog, &active_names, strict_tool_mode);

        Self {
            registry,
            catalog,
            active_names,
            active,
            mode,
            strict_tool_mode,
            allowed_tools,
            disallowed_tools,
            max_tool_calls,
            questions_allowed,
        }
    }

    #[cfg(test)]
    pub(super) fn allows_tool(&self, name: &str) -> bool {
        !self.denies_tool(name) && self.passes_allow_list(name)
    }

    pub(super) fn passes_allow_list(&self, name: &str) -> bool {
        tool_allowed(self.allowed_tools.as_deref(), name)
    }

    pub(super) fn denies_tool(&self, name: &str) -> bool {
        tool_denied(self.disallowed_tools.as_deref(), name)
    }

    pub(super) fn allows_questions(&self) -> bool {
        self.questions_allowed
    }
}

pub(super) fn tool_allowed(allowed_tools: Option<&[String]>, tool_name: &str) -> bool {
    let Some(allowed_tools) = allowed_tools else {
        return true;
    };
    tool_matches_any_rule(allowed_tools, tool_name)
}

pub(super) fn tool_denied(disallowed_tools: Option<&[String]>, tool_name: &str) -> bool {
    disallowed_tools.is_some_and(|rules| tool_matches_any_rule(rules, tool_name))
}

pub(crate) fn tool_matches_any_rule(rules: &[String], tool_name: &str) -> bool {
    let tool_name = tool_name.to_ascii_lowercase();
    rules.iter().any(|rule| {
        let rule = rule.to_ascii_lowercase();
        let (rule_body, is_prefix) = rule
            .strip_suffix('*')
            .map_or((rule.as_str(), false), |prefix| (prefix, true));
        std::iter::once(tool_name.as_str())
            .chain(policy_tool_aliases(&tool_name).iter().copied())
            .any(|candidate| {
                if is_prefix {
                    candidate.starts_with(rule_body)
                } else {
                    candidate == rule_body
                }
            })
    })
}

/// Whether an explicit tool allowlist is provably limited to the native file
/// and shell primitives. Unknown names and wildcards remain conservative
/// because a configured MCP server may own them.
pub(crate) fn allowlist_is_native_file_and_shell_only(allowed_tools: Option<&[String]>) -> bool {
    let Some(rules) = allowed_tools else {
        return false;
    };
    const NATIVE_NAMES: &[&str] = &[
        "bash",
        "exec_shell",
        "read",
        "read_file",
        "write",
        "write_file",
        "edit",
        "edit_file",
        "file",
    ];
    rules.iter().all(|rule| {
        let rule = rule.trim();
        !rule.is_empty()
            && !rule.ends_with('*')
            && NATIVE_NAMES.contains(&rule.to_ascii_lowercase().as_str())
    })
}

fn policy_tool_aliases(name: &str) -> &'static [&'static str] {
    match name {
        "read" | "read_file" => &["read", "read_file", "file"],
        "write" | "write_file" => &["write", "write_file", "file"],
        "edit" | "edit_file" => &["edit", "edit_file", "file"],
        "file" => &[
            "file",
            "read",
            "read_file",
            "write",
            "write_file",
            "edit",
            "edit_file",
        ],
        "bash" | "exec_shell" => &["bash", "exec_shell"],
        _ => &[],
    }
}

/// The `tools` field of one outbound request, from a catalog and the set of
/// currently-active tool names.
///
/// Shared by [`ToolSurfacePolicy`] and the per-step rebuild inside the turn
/// loop, so activating a deferred tool mid-turn goes through one code path.
pub(crate) fn active_tools_for_request(
    catalog: &[Tool],
    active: &HashSet<String>,
    strict_tool_mode: bool,
) -> Option<Vec<Tool>> {
    if catalog.is_empty() {
        return None;
    }
    let mut tools = active_tools_for_step(catalog, active);
    if strict_tool_mode {
        crate::tools::schema_sanitize::prepare_tools_for_strict_mode(&mut tools);
    }
    Some(tools)
}

fn tool_search_haystack(tool: &Tool) -> String {
    format!(
        "{}\n{}\n{}",
        tool.name.to_lowercase(),
        tool.description.to_lowercase(),
        tool.input_schema.to_string().to_lowercase()
    )
}

fn catalog_contains_tool(catalog: &[Tool], name: &str) -> bool {
    catalog.iter().any(|tool| tool.name == name)
}

fn unavailable_core_action_tools_with_regex(
    catalog: &[Tool],
    query: &str,
    max_results: usize,
) -> Result<Vec<CoreActionToolFallback>, ToolError> {
    if max_results == 0 {
        return Ok(Vec::new());
    }
    let regex = compile_user_regex(query)
        .map_err(|err| ToolError::invalid_input(format!("Invalid regex query: {err}")))?;
    Ok(cached_fallbacks()
        .iter()
        .filter(|cf| !catalog_contains_tool(catalog, cf.fallback.name))
        .filter(|cf| regex.is_match(&cf.haystack))
        .take(max_results)
        .map(|cf| cf.fallback)
        .collect())
}

fn unavailable_core_action_tools_with_bm25_like(
    catalog: &[Tool],
    query: &str,
    max_results: usize,
) -> Vec<CoreActionToolFallback> {
    if max_results == 0 {
        return Vec::new();
    }
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|term| term.trim().to_lowercase())
        .filter(|term| !term.is_empty())
        .collect();
    if terms.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<(i64, CoreActionToolFallback)> = Vec::new();
    for cf in cached_fallbacks() {
        if catalog_contains_tool(catalog, cf.fallback.name) {
            continue;
        }
        let hay = &cf.haystack;
        let name = &cf.name_lower;
        let mut score = 0i64;
        for term in &terms {
            if hay.contains(term) {
                score += 1;
            }
            if name.contains(term) {
                score += 2;
            }
        }
        if score > 0 {
            scored.push((score, cf.fallback));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(b.1.name)));
    scored
        .into_iter()
        .take(max_results)
        .map(|(_, fallback)| fallback)
        .collect()
}

fn discover_tools_with_regex(
    catalog: &[Tool],
    query: &str,
    max_results: usize,
) -> Result<Vec<String>, ToolError> {
    let regex = compile_user_regex(query)
        .map_err(|err| ToolError::invalid_input(format!("Invalid regex query: {err}")))?;

    let mut matches = Vec::new();
    for tool in catalog {
        // tool_search loads definitions omitted from the current request. An
        // eager tool is already present, so returning it as a cache candidate
        // would misclassify it as rejected (the cache intentionally accepts
        // deferred definitions only).
        if !tool.defer_loading.unwrap_or(false) || is_tool_search_tool(&tool.name) {
            continue;
        }
        let hay = tool_search_haystack(tool);
        if regex.is_match(&hay) {
            matches.push(tool.name.clone());
        }
        if matches.len() >= max_results {
            break;
        }
    }
    Ok(matches)
}

fn discover_tools_with_bm25_like(catalog: &[Tool], query: &str, max_results: usize) -> Vec<String> {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|term| term.trim().to_lowercase())
        .filter(|term| !term.is_empty())
        .collect();
    if terms.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<(i64, String)> = Vec::new();
    for tool in catalog {
        if !tool.defer_loading.unwrap_or(false) || is_tool_search_tool(&tool.name) {
            continue;
        }
        let hay = tool_search_haystack(tool);
        let mut score = 0i64;
        for term in &terms {
            if hay.contains(term) {
                score += 1;
            }
            if tool.name.to_lowercase().contains(term) {
                score += 2;
            }
        }
        if score > 0 {
            scored.push((score, tool.name.clone()));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored
        .into_iter()
        .take(max_results)
        .map(|(_, name)| name)
        .collect()
}

fn edit_distance(a: &str, b: &str) -> usize {
    if a == b {
        return 0;
    }
    if a.is_empty() {
        return b.chars().count();
    }
    if b.is_empty() {
        return a.chars().count();
    }

    let b_chars: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b_chars.len()).collect();
    let mut curr = vec![0usize; b_chars.len() + 1];

    for (i, a_ch) in a.chars().enumerate() {
        curr[0] = i + 1;
        for (j, b_ch) in b_chars.iter().enumerate() {
            let cost = if a_ch == *b_ch { 0 } else { 1 };
            let delete = prev[j + 1] + 1;
            let insert = curr[j] + 1;
            let substitute = prev[j] + cost;
            curr[j + 1] = delete.min(insert).min(substitute);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b_chars.len()]
}

fn suggest_tool_names(catalog: &[Tool], requested: &str, limit: usize) -> Vec<String> {
    let requested = requested.trim().to_ascii_lowercase();
    if requested.is_empty() || limit == 0 {
        return Vec::new();
    }

    let mut candidates: Vec<(u8, usize, String)> = Vec::new();
    for tool in catalog {
        let candidate = tool.name.to_ascii_lowercase();
        let prefix_match = candidate.starts_with(&requested) || requested.starts_with(&candidate);
        let contains_match = candidate.contains(&requested) || requested.contains(&candidate);
        let distance = edit_distance(&candidate, &requested);
        let close_typo = distance <= 3;

        if !(prefix_match || contains_match || close_typo) {
            continue;
        }

        let rank = if prefix_match {
            0
        } else if contains_match {
            1
        } else {
            2
        };
        candidates.push((rank, distance, tool.name.clone()));
    }

    candidates.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    candidates.dedup_by(|a, b| a.2 == b.2);
    candidates
        .into_iter()
        .take(limit)
        .map(|(_, _, name)| name)
        .collect()
}

/// Catalog tools the engine injects itself rather than registering, plus the
/// legacy tool-search spellings. Exposed so the read-only request projection
/// can label their provenance as `synthetic` from the same source of truth as
/// `is_synthetic_catalog_tool` test coverage instead of guessing.
///
/// MCP-contributed names are deliberately *not* here: those resolve through the
/// real pool, and stay unknown when the pool did not resolve them.
/// [`MULTI_TOOL_PARALLEL_NAME`] is not here either — it is a call name the model
/// may emit, never a catalog entry, so it can never appear in a transmitted
/// tool array and has no catalog provenance to report.
pub(super) fn default_synthetic_catalog_tool_names() -> Vec<String> {
    let mut names: Vec<String> = vec![
        TOOL_SEARCH_NAME.to_string(),
        LEGACY_TOOL_SEARCH_REGEX_NAME.to_string(),
        LEGACY_TOOL_SEARCH_BM25_NAME.to_string(),
        CODE_EXECUTION_TOOL_NAME.to_string(),
        JS_EXECUTION_TOOL_NAME.to_string(),
    ];
    names.sort();
    names.dedup();
    names
}

#[cfg(test)]
fn is_synthetic_catalog_tool(name: &str) -> bool {
    is_tool_search_tool(name)
        || matches!(name, CODE_EXECUTION_TOOL_NAME | JS_EXECUTION_TOOL_NAME)
        || McpPool::is_mcp_tool(name)
}

#[cfg(test)]
pub(super) fn tool_catalog_consistency_issues(
    catalog: &[Tool],
    registry: &crate::tools::ToolRegistry,
) -> Vec<String> {
    let catalog_names = catalog
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<HashSet<_>>();
    let registry_api_tools = registry.to_api_tools();
    let registry_model_visible_names = registry_api_tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<HashSet<_>>();
    let mut issues = Vec::new();

    for tool in catalog {
        if is_synthetic_catalog_tool(&tool.name) {
            continue;
        }
        if !registry.contains(&tool.name) {
            issues.push(format!(
                "catalog advertises '{}' but no registered handler exists",
                tool.name
            ));
        }
    }

    for name in DEFAULT_ACTIVE_NATIVE_TOOLS {
        if registry_model_visible_names.contains(name) && !catalog_names.contains(name) {
            issues.push(format!(
                "registered core tool '{name}' is missing from the model/search catalog"
            ));
        }
    }

    issues.sort();
    issues
}

pub(super) fn missing_tool_error_message(tool_name: &str, catalog: &[Tool]) -> String {
    // Dogfood A5 (#4092): models mid-checklist sometimes emit each list entry
    // as its own tool call named `item`/`todo`/... . Fuzzy suggestions are
    // actively misleading there ("Did you mean: note, tts?"); name the actual
    // fix instead.
    if matches!(
        tool_name,
        "item" | "items" | "todo" | "todos" | "checklist" | "checklist_item" | "plan_item"
    ) {
        return format!(
            "Tool '{tool_name}' is not available in the current tool catalog. \
             Checklist entries are not separate tool calls — write the whole list \
             in one `todo_write` call with a `todos` array of \
             {{content, status}} objects."
        );
    }
    let suggestions = suggest_tool_names(catalog, tool_name, 3);
    let shell_hint = if is_shell_tool_name(tool_name) {
        Some(shell_tool_allow_shell_hint())
    } else {
        None
    };
    // #5123-class: `exec_shell` was replaced by lowercase `bash`. Name it first —
    // otherwise the error misdiagnoses a retired-name call as an allow_shell
    // permission problem and sends the model fixing the wrong thing.
    if tool_name == "exec_shell" {
        return format!(
            "Tool '{tool_name}' is not available in the current tool catalog. \
             `exec_shell` was replaced by `bash` — call `bash` with a `command` instead. \
             If `bash` is also absent: {shell_hint}.",
            shell_hint = shell_tool_allow_shell_hint()
        );
    }
    if matches!(
        tool_name,
        "exec_shell_wait" | "exec_shell_interact" | "exec_shell_cancel"
    ) {
        return format!(
            "Tool '{tool_name}' is not available in the current tool catalog. \
             Lowercase `bash` is foreground-only; use {TOOL_SEARCH_NAME} to discover shell session controls."
        );
    }
    if suggestions.is_empty() {
        if let Some(shell_hint) = shell_hint {
            return format!(
                "Tool '{tool_name}' is not available in the current tool catalog. \
                 {shell_hint}, or use {TOOL_SEARCH_NAME} with a short query."
            );
        }
        return format!(
            "Tool '{tool_name}' is not available in the current tool catalog. \
             Verify mode/feature flags, or use {TOOL_SEARCH_NAME} with a short query."
        );
    }

    let suggestion_text = format!("Did you mean: {}?", suggestions.join(", "));
    if let Some(shell_hint) = shell_hint {
        return format!(
            "Tool '{tool_name}' is not available in the current tool catalog. \
             {suggestion_text} {shell_hint}. \
             You can also use {TOOL_SEARCH_NAME} to discover tools."
        );
    }

    format!(
        "Tool '{tool_name}' is not available in the current tool catalog. \
         {suggestion_text} You can also use {TOOL_SEARCH_NAME} to discover tools."
    )
}

fn shell_tool_allow_shell_hint() -> &'static str {
    "Shell tools are absent because this session or profile disabled shell access, \
     commonly via top-level `allow_shell = false`. \
     Work mode exposes shell by default with approval gating unless disabled. \
     Run `/config allow_shell true` for this session or add `--save` for future sessions; \
     the next turn will expose shell again"
}

fn is_shell_tool_name(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "exec_shell"
            | "exec_shell_wait"
            | "exec_shell_interact"
            | "task_shell_start"
            | "task_shell_wait"
    )
}

pub(super) fn maybe_hydrate_requested_deferred_tool(
    tool_name: &str,
    tool_input: &Value,
    catalog: &[Tool],
    active_tools_at_batch_start: &HashSet<String>,
    hydrated_tools_this_batch: &mut HashSet<String>,
) -> Option<ToolResult> {
    let def = catalog.iter().find(|def| def.name == tool_name)?;

    if !def.defer_loading.unwrap_or(false) || active_tools_at_batch_start.contains(tool_name) {
        return None;
    }

    hydrated_tools_this_batch.insert(tool_name.to_string());
    Some(deferred_tool_schema_hydration_result(def, tool_input))
}

#[cfg(test)]
pub(super) fn preflight_requested_deferred_tool(
    tool_name: &str,
    tool_input: &Value,
    catalog: &[Tool],
    active_tools: &mut HashSet<String>,
) -> Option<ToolResult> {
    let active_tools_at_batch_start = active_tools.clone();
    let mut hydrated_tools_this_batch = HashSet::new();
    let result = maybe_hydrate_requested_deferred_tool(
        tool_name,
        tool_input,
        catalog,
        &active_tools_at_batch_start,
        &mut hydrated_tools_this_batch,
    );
    active_tools.extend(hydrated_tools_this_batch);
    result
}

fn deferred_tool_schema_hydration_result(tool: &Tool, tool_input: &Value) -> ToolResult {
    let expected = schema_fields(&tool.input_schema);
    let required = schema_required_fields(&tool.input_schema);
    let received = received_field_names(tool_input);
    let missing = required
        .iter()
        .filter(|field| !received.contains(field))
        .cloned()
        .collect::<Vec<_>>();
    let unexpected = received
        .iter()
        .filter(|field| !expected.iter().any(|expected| &expected.name == *field))
        .cloned()
        .collect::<Vec<_>>();
    let corrections = likely_field_corrections(&received, &expected, &tool.name);

    let mut lines = vec![
        format!("Tool `{}` was deferred and has now been loaded.", tool.name),
        String::new(),
        "The tool was not executed. Retry with the loaded schema.".to_string(),
        String::new(),
        "Expected fields:".to_string(),
    ];
    if expected.is_empty() {
        lines.push("  (none)".to_string());
    } else {
        for field in &expected {
            let required_marker = if required.contains(&field.name) {
                " required"
            } else {
                ""
            };
            lines.push(format!(
                "  {}: {}{}",
                field.name, field.kind, required_marker
            ));
        }
    }
    lines.push(String::new());
    lines.push("Received fields:".to_string());
    if received.is_empty() {
        lines.push("  (none)".to_string());
    } else {
        lines.push(format!("  {}", received.join(", ")));
    }
    if !missing.is_empty() {
        lines.push(String::new());
        lines.push("Missing required fields:".to_string());
        lines.push(format!("  {}", missing.join(", ")));
    }
    if !unexpected.is_empty() {
        lines.push(String::new());
        lines.push("Unexpected fields:".to_string());
        lines.push(format!("  {}", unexpected.join(", ")));
    }
    if !corrections.is_empty() {
        lines.push(String::new());
        lines.push("Likely corrections:".to_string());
        for correction in &corrections {
            lines.push(format!("  {correction}"));
        }
    }

    ToolResult::success(lines.join("\n")).with_metadata(json!({
        "event": "tool.schema_hydrated",
        "tool": tool.name,
        "executed": false,
        "retry_required": true,
        "reason": "deferred_tool_first_use",
        "deferred_tool_loaded": true,
        "tool_name": tool.name,
        "expected_fields": expected.iter().map(|field| field.name.clone()).collect::<Vec<_>>(),
        "received_fields": received,
        "missing_required_fields": missing,
        "unexpected_fields": unexpected,
        "likely_corrections": corrections,
    }))
}

#[derive(Debug, Clone)]
struct SchemaField {
    name: String,
    kind: String,
}

fn schema_fields(schema: &Value) -> Vec<SchemaField> {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut fields = properties
        .iter()
        .map(|(name, spec)| SchemaField {
            name: name.clone(),
            kind: schema_type_label(spec),
        })
        .collect::<Vec<_>>();
    fields.sort_by(|a, b| a.name.cmp(&b.name));
    fields
}

fn schema_required_fields(schema: &Value) -> Vec<String> {
    let mut required = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect::<Vec<_>>();
    required.sort();
    required
}

fn schema_type_label(spec: &Value) -> String {
    let Some(kind) = spec.get("type").and_then(Value::as_str) else {
        return "value".to_string();
    };
    if let Some(values) = spec.get("enum").and_then(Value::as_array) {
        let labels = values.iter().filter_map(Value::as_str).collect::<Vec<_>>();
        if !labels.is_empty() {
            return format!("{kind} ({})", labels.join(" | "));
        }
    }
    kind.to_string()
}

fn received_field_names(input: &Value) -> Vec<String> {
    let mut fields = input
        .as_object()
        .map(|object| object.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    fields.sort();
    fields
}

fn likely_field_corrections(
    received: &[String],
    expected: &[SchemaField],
    tool_name: &str,
) -> Vec<String> {
    let has_expected = |name: &str| expected.iter().any(|field| field.name == name);
    let has_received = |name: &str| received.iter().any(|field| field == name);
    let mut corrections = Vec::new();

    if has_received("old_string") && has_expected("search") {
        corrections.push("old_string -> search".to_string());
    } else if has_received("old_str") && has_expected("search") {
        corrections.push("old_str -> search".to_string());
    }
    if has_received("new_string") && has_expected("replace") {
        corrections.push("new_string -> replace".to_string());
    } else if has_received("new_str") && has_expected("replace") {
        corrections.push("new_str -> replace".to_string());
    } else if has_received("replacement") && has_expected("replace") {
        corrections.push("replacement -> replace".to_string());
    }
    if matches!(tool_name, "checklist_update" | "todo_update") && has_received("todos") {
        corrections.push(
            "Use todo_write to replace the full list, or retry checklist_update/todo_update with id and status."
                .to_string(),
        );
    }
    // RLM source fields are easy to misname (#2659). rlm_open takes exactly one
    // of file_path / content / url / session_object; nudge common wrong names
    // toward those. The unified `rlm` tool carries the same fields for
    // action=open, so it gets the same correction.
    if matches!(tool_name, "rlm_open" | "rlm") {
        for wrong in [
            "prompt",
            "resident_file",
            "text",
            "body",
            "path",
            "file",
            "source",
        ] {
            if has_received(wrong)
                && !has_received("file_path")
                && !has_received("content")
                && !has_received("url")
                && !has_received("session_object")
            {
                corrections.push(format!("{wrong} -> file_path (local file), content (inline text), url, or session_object"));
            }
        }
    }
    corrections
}

#[cfg(test)]
pub(super) fn execute_tool_search(
    tool_name: &str,
    input: &serde_json::Value,
    catalog: &[Tool],
    active_tools: &mut HashSet<String>,
) -> Result<ToolResult, ToolError> {
    execute_tool_search_inner(tool_name, input, catalog, active_tools, None)
}

/// Execute tool search while retaining activated schemas in the bounded
/// conversation cache. Kept separate from the test-only pure search helper so existing
/// pure catalog tests and compatibility callers do not need an engine session.
pub(crate) fn execute_tool_search_with_cache(
    tool_name: &str,
    input: &serde_json::Value,
    catalog: &[Tool],
    active_tools: &mut HashSet<String>,
    cache: &mut crate::core::session::ToolActivationCache,
) -> Result<ToolResult, ToolError> {
    execute_tool_search_inner(tool_name, input, catalog, active_tools, Some(cache))
}

fn execute_tool_search_inner(
    tool_name: &str,
    input: &serde_json::Value,
    catalog: &[Tool],
    active_tools: &mut HashSet<String>,
    cache: Option<&mut crate::core::session::ToolActivationCache>,
) -> Result<ToolResult, ToolError> {
    let query = required_str(input, "query")?;
    let match_kind = match tool_name {
        LEGACY_TOOL_SEARCH_REGEX_NAME => "regex",
        LEGACY_TOOL_SEARCH_BM25_NAME => "bm25",
        _ => optional_str(input, "match")?.unwrap_or("bm25"),
    };
    if !matches!(match_kind, "bm25" | "regex") {
        return Err(ToolError::invalid_input(format!(
            "Unsupported match algorithm '{match_kind}'. Expected one of: bm25, regex"
        )));
    }
    let max_results = usize::try_from(optional_u64(
        input,
        "max_results",
        TOOL_SEARCH_DEFAULT_MAX_RESULTS as u64,
    )?)
    .unwrap_or(TOOL_SEARCH_DEFAULT_MAX_RESULTS)
    .clamp(1, TOOL_SEARCH_MAX_RESULTS_LIMIT);
    let mut discovered = if match_kind == "regex" {
        discover_tools_with_regex(catalog, query, max_results)?
    } else {
        discover_tools_with_bm25_like(catalog, query, max_results)
    };
    let remaining_results = max_results.saturating_sub(discovered.len());
    let unavailable = if match_kind == "regex" {
        unavailable_core_action_tools_with_regex(catalog, query, remaining_results)?
    } else {
        unavailable_core_action_tools_with_bm25_like(catalog, query, remaining_results)
    };

    let mut cache_rejected = Vec::new();
    if let Some(cache) = cache {
        let delta = cache.activate(catalog, &discovered);
        remove_evicted_cache_activations(catalog, active_tools, delta.evicted);
        cache_rejected = delta.rejected;
        discovered = delta.admitted;
    }
    for name in &discovered {
        active_tools.insert(name.clone());
    }

    let references = discovered
        .iter()
        .map(|name| json!({"type": "tool_reference", "tool_name": name}))
        .collect::<Vec<_>>();
    let mut unavailable_references = unavailable
        .iter()
        .map(|fallback| {
            json!({
                "type": "unavailable_tool_reference",
                "tool_name": fallback.name,
                "reason": fallback.unavailable_reason,
            })
        })
        .collect::<Vec<_>>();
    unavailable_references.extend(cache_rejected.iter().map(|name| {
        json!({
            "type": "unavailable_tool_reference",
            "tool_name": name,
            "reason": "The tool schema exceeds the bounded conversation toolbox (8 cached tools, 16KiB of added serialized schemas). Narrow the search or use a smaller matching tool."
        })
    }));

    let payload = json!({
        "type": "tool_search_tool_search_result",
        "tool_references": references,
        "unavailable_tool_references": unavailable_references.clone(),
    });

    Ok(ToolResult {
        content: serde_json::to_string(&payload).unwrap_or_else(|_| payload.to_string()),
        success: true,
        metadata: Some(json!({
            "tool_references": discovered,
            "unavailable_tool_references": unavailable_references,
        })),
    })
}

pub(super) async fn execute_code_execution_tool(
    input: &serde_json::Value,
    workspace: &Path,
) -> Result<ToolResult, ToolError> {
    let code = required_str(input, "code")?;

    // Resolve the locally-installed Python interpreter we cached at
    // catalog-build time. If it's absent now (somehow registered but
    // disappeared between startup and this call — concurrent uninstall,
    // PATH change, etc.) the ExternalTool::tokio_command() will return
    // None and we fail fast with a clear message.
    //
    // Write the code to a temp file and execute it as a script rather
    // than passing it via `-c "<code>"`. Reasons:
    //   * `-c` has length limits (argv) on Windows.
    //   * Multiline code with quote nesting is brittle through `-c`.
    //   * Tracebacks reference a real filename instead of `<string>`,
    //     so the model can interpret line numbers correctly.
    // Tempfile lives only for the duration of this execution; Drop
    // removes it. We use `.py` so any shebang / encoding-sniffer
    // logic in the interpreter behaves normally.
    let temp_dir = tempfile::tempdir()
        .map_err(|e| ToolError::execution_failed(format!("tempdir failed: {e}")))?;
    let script_path = temp_dir.path().join("code_execution.py");
    tokio::fs::write(&script_path, code)
        .await
        .map_err(|e| ToolError::execution_failed(format!("tempfile write failed: {e}")))?;

    let mut cmd = crate::dependencies::Python::tokio_command().ok_or_else(|| {
        ToolError::execution_failed(
            "code_execution: Python interpreter became unavailable".to_string(),
        )
    })?;
    cmd.arg(&script_path).current_dir(workspace);

    let output = tokio::time::timeout(Duration::from_secs(120), cmd.output())
        .await
        .map_err(|_| ToolError::Timeout { seconds: 120 })
        .and_then(|res| res.map_err(|e| ToolError::execution_failed(e.to_string())))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let return_code = output.status.code().unwrap_or(-1);
    let success = output.status.success();
    let payload = json!({
        "type": "code_execution_result",
        "stdout": stdout,
        "stderr": stderr,
        "return_code": return_code,
        "content": [],
    });

    Ok(ToolResult {
        content: serde_json::to_string(&payload).unwrap_or_else(|_| payload.to_string()),
        success,
        metadata: Some(payload),
    })
}

#[cfg(test)]
#[path = "tool_catalog/tests.rs"]
mod synthetic_name_tests;
