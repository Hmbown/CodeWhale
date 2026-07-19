//! Application state for the `DeepSeek` TUI.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use ratatui::layout::Rect;
use ratatui::style::Color;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use codewhale_config::{ProviderChain, route::RouteLimits};

use crate::artifacts::ArtifactRecord;
use crate::client::{CacheWarmupKey, PromptInspection};
use crate::compaction::CompactionConfig;
use crate::config::{
    ApiProvider, ApprovalPolicyControl, Config, DEFAULT_TEXT_MODEL, SavedCredential, has_api_key,
    has_api_key_for, save_api_key, save_api_key_for,
};
use crate::config_ui::ConfigUiMode;
use crate::core::authority::{ModeSessionPrefs, base_policy_for_mode};
use crate::core::events::TurnRoute;
use crate::hooks::{HookContext, HookEvent, HookExecutor, HookResult};
use crate::localization::{Locale, MessageId, resolve_locale, tr};
use crate::models::{Message, SystemPrompt, Tool};
use crate::palette::{self, UiTheme};
use crate::pricing::{CostCurrency, CostEstimate};
use crate::resource_telemetry::TokenThroughput;
use crate::session_manager::{SessionContextReference, SessionMetadata, SessionWorkState};
use crate::settings::Settings;
use crate::tools::plan::{PlanState, SharedPlanState, new_shared_plan_state};
use crate::tools::shell::new_shared_shell_manager;
use crate::tools::spec::RuntimeToolServices;
use crate::tools::subagent::SubAgentResult;
use crate::tools::todo::{SharedTodoList, TodoList, new_shared_todo_list};
use crate::tui::active_cell::ActiveCell;
use crate::tui::approval::ApprovalMode;
use crate::tui::clipboard::{ClipboardContent, ClipboardHandler};
use crate::tui::file_mention::ContextReference;
use crate::tui::history::{HistoryCell, TranscriptRenderOptions};
use crate::tui::hotbar::HotbarActionRegistry;
use crate::tui::paste_burst::{FlushResult, PasteBurst};
use crate::tui::scrolling::{MouseScrollState, TranscriptLineMeta, TranscriptScroll};
use crate::tui::selection::{SelectionAutoscroll, TranscriptSelection};
use crate::tui::sidebar::SidebarWorkSummary;
use crate::tui::streaming::StreamingState;
use crate::tui::transcript::TranscriptViewCache;
use crate::tui::views::ViewStack;

// === Types ===

/// Lifecycle identity retained until the matching `TurnComplete` arrives.
///
/// This survives local cancellation clearing the visible runtime status, so
/// observer records still carry a stable id, start time, and effective route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveTurnMetadata {
    pub turn_id: String,
    pub created_at: DateTime<Utc>,
    pub route: Option<TurnRoute>,
    /// Auto decision metadata captured with this exact authoritative route.
    pub auto_route_receipt: Option<crate::model_routing::AutoRouteReceipt>,
}

/// State machine for onboarding new users.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingState {
    Welcome,
    /// Pick the UI locale before any other config decisions (#566).
    /// Defaults to auto-detection from `LC_ALL` / `LANG`; explicit picks
    /// land in the persisted settings.toml via `Settings::set("locale", …)`.
    Language,
    Provider,
    ApiKey,
    TrustDirectory,
    MentalModels,
    Tips,
    None,
}

pub(crate) fn resolve_skills_dir(
    workspace: &Path,
    global_skills_dir: &Path,
    config: &Config,
) -> PathBuf {
    if config.skills_config().scan_codewhale_only() {
        if config.skills_dir.is_some() {
            return global_skills_dir.to_path_buf();
        }
        if let Some(codewhale_skills_dir) = crate::skills::codewhale_workspace_skills_dir(workspace)
        {
            return codewhale_skills_dir;
        }
        return global_skills_dir.to_path_buf();
    }

    let agents_skills_dir = workspace.join(".agents").join("skills");
    if agents_skills_dir.exists() {
        return agents_skills_dir;
    }

    let local_skills_dir = workspace.join("skills");
    if local_skills_dir.exists() {
        return local_skills_dir;
    }

    if config.skills_dir.is_none()
        && let Some(global_agents) = crate::skills::agents_global_skills_dir()
        && global_agents.exists()
    {
        return global_agents;
    }

    global_skills_dir.to_path_buf()
}

pub(crate) fn looks_like_slash_command_input(input: &str) -> bool {
    let trimmed = input.trim_start();
    // `$skillname` at the start of input is treated like a slash command so the
    // skill-completion menu appears.
    let Some(rest) = trimmed
        .strip_prefix('/')
        .or_else(|| trimmed.strip_prefix('$'))
    else {
        return false;
    };
    if rest.chars().next().is_some_and(|ch| ch.is_whitespace()) {
        return false;
    }
    let Some(command) = rest.split_whitespace().next() else {
        return rest.is_empty();
    };

    !command.contains('/')
}

pub(crate) fn shell_command_from_bang_input(input: &str) -> Result<Option<&str>, &'static str> {
    let Some(rest) = input.trim_start().strip_prefix('!') else {
        return Ok(None);
    };
    let command = rest.trim();
    if command.is_empty() {
        return Err("Usage: ! <shell command>");
    }
    Ok(Some(command))
}

fn initial_onboarding_state(
    skip_onboarding: bool,
    was_onboarded: bool,
    needs_api_key: bool,
    needs_workspace_trust: bool,
) -> OnboardingState {
    if skip_onboarding || (was_onboarded && !needs_api_key && !needs_workspace_trust) {
        return OnboardingState::None;
    }

    if was_onboarded && needs_api_key {
        // Missing-key recovery uses the canonical provider picker so it can
        // preserve the configured provider, endpoint, and model route before
        // asking for a replacement secret.
        OnboardingState::Provider
    } else if was_onboarded && needs_workspace_trust {
        OnboardingState::TrustDirectory
    } else {
        OnboardingState::Welcome
    }
}

fn onboarding_is_workspace_trust_gate(
    skip_onboarding: bool,
    was_onboarded: bool,
    needs_api_key: bool,
    needs_workspace_trust: bool,
) -> bool {
    !skip_onboarding && was_onboarded && !needs_api_key && needs_workspace_trust
}

/// Supported application modes for the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    Agent,
    #[allow(dead_code)]
    Auto,
    /// Legacy compatibility alias; resolves to [`Self::Agent`] + bypass approvals.
    Yolo,
    Plan,
    Operate,
}

/// One row in the per-turn cache-telemetry ring (`/cache` debug surface, #263).
#[derive(Debug, Clone)]
pub struct TurnCacheRecord {
    /// API provider used for the turn. This is recorded so cache misses can be
    /// correlated with provider/model route changes.
    pub provider: Option<ApiProvider>,
    /// Exact non-secret configured route key. This distinguishes named custom
    /// providers which all share [`ApiProvider::Custom`].
    pub provider_identity: Option<String>,
    /// Concrete model used for the turn. For auto-model turns this is the
    /// routed model, not the literal `auto` setting.
    pub model: Option<String>,
    /// Whether the route came from the auto-model selector.
    pub auto_model: bool,
    /// Provider-reported total input tokens for the turn (cache-hit +
    ///   cache-miss + uncategorized). Useful for sanity-checking that hits +
    ///   misses sum back to roughly the prompt size.
    pub input_tokens: u32,
    /// Provider-reported output tokens.
    pub output_tokens: u32,
    /// `prompt_cache_hit_tokens` from DeepSeek's usage payload. `None` when
    ///   the model in use does not report cache telemetry (see
    ///   `Capabilities::cache_telemetry_supported`).
    pub cache_hit_tokens: Option<u32>,
    /// `prompt_cache_miss_tokens`. `None` when the provider did not report it
    ///   — in that case the `/cache` formatter infers the miss as
    ///   `input_tokens − cache_hit_tokens`.
    pub cache_miss_tokens: Option<u32>,
    /// Approximate tokens spent re-sending prior `reasoning_content` on
    ///   V4-thinking tool-calling turns (chars/3 heuristic). Helps separate
    ///   cache misses caused by reasoning-replay churn from misses caused by
    ///   real prefix instability.
    pub reasoning_replay_tokens: Option<u32>,
    /// Local timestamp the turn telemetry was recorded.
    pub recorded_at: Instant,
}

/// Reasoning-effort tier, mirrored across DeepSeek and Codex effort pickers.
///
/// The config file accepts all five string values for forward-compat with
/// providers that expose the full spectrum; DeepSeek currently collapses
/// `Low`/`Medium` → `high`. OpenAI Codex normalizes inherited DeepSeek-only
/// `Off` to `Low` and displays/sends `Max` as `xhigh` at the provider
/// boundary. The default keyboard cycler walks the three DeepSeek-distinct
/// tiers: `Off` → `High` → `Max` → `Off`; provider-aware callers should use
/// [`ReasoningEffort::cycle_next_for_provider`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningEffort {
    Off,
    Low,
    Medium,
    High,
    Auto,
    #[default]
    Max,
}

impl From<ReasoningEffort> for crate::work_graph::ReasoningEffortTier {
    fn from(value: ReasoningEffort) -> Self {
        match value {
            ReasoningEffort::Off => Self::Off,
            ReasoningEffort::Low => Self::Low,
            ReasoningEffort::Medium => Self::Medium,
            ReasoningEffort::High => Self::High,
            ReasoningEffort::Auto => Self::Auto,
            ReasoningEffort::Max => Self::Max,
        }
    }
}

impl ReasoningEffort {
    /// Parse an operator-supplied effort value.
    ///
    /// This is deliberately the one canonical spelling table for every
    /// human-facing route.  Callers that read an old persisted config may use
    /// [`Self::from_setting`] for its compatibility fallback, but a new CLI,
    /// settings, or tool input must reject an unknown value instead of quietly
    /// turning it into `max`.
    pub fn parse_strict(value: &str) -> Result<Self, String> {
        let trimmed = value.trim();
        match trimmed.to_ascii_lowercase().as_str() {
            "off" | "disabled" | "none" | "false" => Ok(Self::Off),
            "low" | "minimum" | "minimal" | "light" => Ok(Self::Low),
            "medium" | "mid" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "auto" | "automatic" => Ok(Self::Auto),
            "max" | "maximum" | "xhigh" | "ultra" | "ultracode" => Ok(Self::Max),
            _ => Err(format!(
                "Unrecognized reasoning effort {trimmed:?}. Expected: auto, off, low, medium, high, or max."
            )),
        }
    }

    /// Parse a persisted config-file string into an effort tier. Unknown
    /// legacy values fall back to the default (`Max`) so an old malformed
    /// settings file never prevents startup.  New user input should use
    /// [`Self::parse_strict`] instead.
    #[must_use]
    pub fn from_setting(value: &str) -> Self {
        Self::parse_strict(value).unwrap_or_default()
    }

    #[must_use]
    pub fn from_setting_for_provider(value: &str, provider: ApiProvider) -> Self {
        Self::from_setting(value).normalize_for_provider(provider)
    }

    /// Canonical lowercase label used for config storage and UI hints.
    #[must_use]
    pub fn as_setting(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Auto => "auto",
            Self::Max => "max",
        }
    }

    /// Short label for the header chip.
    #[must_use]
    pub fn short_label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "med",
            Self::High => "high",
            Self::Auto => "auto",
            Self::Max => "max",
        }
    }

    /// Provider-facing label for user-visible surfaces.
    #[must_use]
    pub fn display_label_for_provider(self, provider: ApiProvider) -> &'static str {
        match (provider, self.normalize_for_provider(provider)) {
            (ApiProvider::OpenaiCodex, Self::Low) => "low",
            (ApiProvider::OpenaiCodex, Self::Medium) => "medium",
            (ApiProvider::OpenaiCodex, Self::High) => "high",
            (ApiProvider::OpenaiCodex, Self::Max) => "xhigh",
            (_, effort) => effort.short_label(),
        }
    }

    /// Value forwarded to the engine/client. `None` means "provider default"
    /// (for `Off` we still emit `"off"` so the client can inject
    /// `thinking = {"type": "disabled"}`).
    #[must_use]
    pub fn api_value(self) -> Option<&'static str> {
        Some(self.as_setting())
    }

    #[must_use]
    pub fn normalize_for_provider(self, provider: ApiProvider) -> Self {
        if provider != ApiProvider::OpenaiCodex {
            return self;
        }
        match self {
            Self::Off => Self::Low,
            Self::Auto => Self::Medium,
            other => other,
        }
    }

    /// Resolve an effort against the exact provider route that will receive
    /// the request.  Low/medium remain distinct only for the official Kimi
    /// Code K3 endpoint; generic Moonshot and every other non-Codex route
    /// retain the historic high coercion.  This intentionally does not change
    /// [`Self::normalize_for_provider`], whose generic wire semantics are used
    /// by older callers that do not yet have a route receipt.
    #[must_use]
    pub fn normalize_for_route(
        self,
        provider: ApiProvider,
        base_url: &str,
        wire_model: &str,
    ) -> Self {
        let normalized = self.normalize_for_provider(provider);
        if crate::config::is_exact_kimi_code_k3_route(provider, base_url, wire_model) {
            return normalized;
        }
        if provider == ApiProvider::OpenaiCodex {
            return normalized;
        }
        match normalized {
            Self::Low | Self::Medium => Self::High,
            other => other,
        }
    }

    #[must_use]
    pub fn api_value_for_provider(self, provider: ApiProvider) -> Option<&'static str> {
        if provider != ApiProvider::OpenaiCodex {
            return self.api_value();
        }
        Some(match self.normalize_for_provider(provider) {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Max => "xhigh",
            Self::Off => "low",
            Self::Auto => "medium",
        })
    }

    /// Provider-facing value after exact-route normalization.
    #[must_use]
    pub fn api_value_for_route(
        self,
        provider: ApiProvider,
        base_url: &str,
        wire_model: &str,
    ) -> Option<&'static str> {
        self.normalize_for_route(provider, base_url, wire_model)
            .api_value_for_provider(provider)
    }

    #[must_use]
    pub fn as_setting_for_provider(self, provider: ApiProvider) -> &'static str {
        self.api_value_for_provider(provider)
            .unwrap_or_else(|| self.as_setting())
    }

    /// Persist the canonical setting after exact-route normalization.
    #[must_use]
    pub fn as_setting_for_route(
        self,
        provider: ApiProvider,
        base_url: &str,
        wire_model: &str,
    ) -> &'static str {
        self.normalize_for_route(provider, base_url, wire_model)
            .as_setting_for_provider(provider)
    }

    /// Cycle through the three behaviorally distinct tiers.
    #[must_use]
    pub fn cycle_next(self) -> Self {
        match self {
            Self::Off => Self::High,
            Self::Auto => Self::Off,
            Self::Low | Self::Medium | Self::High => Self::Max,
            Self::Max => Self::Off,
        }
    }

    #[must_use]
    pub fn cycle_next_for_provider(self, provider: ApiProvider) -> Self {
        if provider != ApiProvider::OpenaiCodex {
            return self.cycle_next();
        }
        match self.normalize_for_provider(provider) {
            Self::Low => Self::Medium,
            Self::Medium => Self::High,
            Self::High => Self::Max,
            Self::Max => Self::Low,
            Self::Off | Self::Auto => Self::Low,
        }
    }
}

/// Sidebar content focus mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarFocus {
    Auto,
    Pinned,
    Tasks,
    Agents,
    Context,
    Hidden,
}

/// Browsing context captured when the `/model` picker is dismissed (#4109).
/// Plain data so `App` does not depend on the picker's internal view enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPickerMemory {
    /// True when the user left the picker in the full-catalog view
    /// (`A` toggle), false for the configured-only default view.
    ///
    /// Kept for backward compatibility with older dismiss events; prefer
    /// [`Self::view`] when present (#4115).
    pub catalog_view: bool,
    /// Named catalog view left open (`configured` / `catalog` / `recent` /
    /// `coding` / `cheap` / `long_context`). When `None`, [`Self::catalog_view`]
    /// is the fallback.
    pub view: Option<String>,
    /// Model row id highlighted at dismissal, if it was a real row.
    pub selected_row_id: Option<String>,
}

/// Browsing context captured when the `/provider` picker is dismissed.
/// Mirrors [`ModelPickerMemory`] so reopen restores view + highlight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderPickerMemory {
    /// True when the user left the picker in the full-catalog view
    /// (`A` toggle), false for the configured-only default view.
    pub catalog_view: bool,
    /// Provider id highlighted at dismissal, if it was a real row.
    pub selected_provider_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentProgressMeta {
    pub parent_run_id: Option<String>,
    pub spawn_depth: u32,
}

/// Per-turn LSP repair-loop summary for the Turn Inspector (#4107).
/// Observable state only — no raw diagnostic text or prompt internals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspRepairState {
    pub diagnostics_found: usize,
    pub files_touched: usize,
    pub injected: bool,
    pub repair_attempted: bool,
    /// "resolved" | "still_failing" | "unknown" | "unavailable"
    pub latest: &'static str,
}

impl Default for LspRepairState {
    fn default() -> Self {
        Self {
            diagnostics_found: 0,
            files_touched: 0,
            injected: false,
            repair_attempted: false,
            latest: "unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposerDensity {
    Compact,
    Comfortable,
    Spacious,
}

impl ComposerDensity {
    #[must_use]
    pub fn from_setting(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "compact" | "tight" => Self::Compact,
            "spacious" | "loose" => Self::Spacious,
            _ => Self::Comfortable,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptSpacing {
    Compact,
    Comfortable,
    Spacious,
}

impl TranscriptSpacing {
    #[must_use]
    pub fn from_setting(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "compact" | "tight" => Self::Compact,
            "spacious" | "loose" => Self::Spacious,
            _ => Self::Comfortable,
        }
    }
}

impl SidebarFocus {
    #[must_use]
    pub fn from_setting(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "pinned" | "visible" | "show" | "on" | "work" | "plan" | "todos" => Self::Pinned,
            // Persist/compat key remains "tasks"; user-facing panel is Activity (#4147/#4135).
            "tasks" | "activity" | "live" | "running" => Self::Tasks,
            "agents" | "subagents" | "sub-agents" => Self::Agents,
            "context" | "session" => Self::Context,
            "hidden" | "hide" | "closed" | "off" | "none" => Self::Hidden,
            _ => Self::Auto,
        }
    }

    #[must_use]
    #[allow(dead_code)]
    pub fn as_setting(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Pinned => "pinned",
            Self::Tasks => "tasks",
            Self::Agents => "agents",
            Self::Context => "context",
            Self::Hidden => "hidden",
        }
    }
}

/// Controls how dense tool-call runs are collapsed in the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCollapseMode {
    /// Collapse qualifying tool runs by default.
    Compact,
    /// Never collapse tool runs automatically.
    Expanded,
    /// Collapse only when calm mode is active.
    Calm,
}

impl ToolCollapseMode {
    #[must_use]
    pub fn from_setting(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "expanded" | "off" | "none" => Self::Expanded,
            "calm" | "calm-mode" | "calm_only" | "calm-only" => Self::Calm,
            // `collapsed`/`collapse` are issue #3256's preferred names for the
            // default; treat them like the canonical `compact`.
            _ => Self::Compact,
        }
    }

    #[must_use]
    pub fn as_setting(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Expanded => "expanded",
            Self::Calm => "calm",
        }
    }

    #[must_use]
    pub fn is_active(self, calm_mode: bool) -> bool {
        match self {
            Self::Compact => true,
            Self::Expanded => false,
            Self::Calm => calm_mode,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusToastLevel {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct StatusToast {
    pub text: String,
    pub level: StatusToastLevel,
    pub created_at: Instant,
    pub ttl_ms: Option<u64>,
}

impl StatusToast {
    #[must_use]
    pub fn new(text: impl Into<String>, level: StatusToastLevel, ttl_ms: Option<u64>) -> Self {
        Self {
            text: text.into(),
            level,
            created_at: Instant::now(),
            ttl_ms,
        }
    }

    #[must_use]
    pub fn is_expired(&self, now: Instant) -> bool {
        self.ttl_ms
            .is_some_and(|ttl| now.duration_since(self.created_at).as_millis() >= u128::from(ttl))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerHistorySearch {
    pre_search_input: String,
    pre_search_cursor: usize,
    query: String,
    selected: usize,
}

impl ComposerHistorySearch {
    fn new(pre_search_input: String, pre_search_cursor: usize) -> Self {
        Self {
            pre_search_input,
            pre_search_cursor,
            query: String::new(),
            selected: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InputHistoryDraft {
    input: String,
    cursor: usize,
}

pub(crate) fn char_count(text: &str) -> usize {
    text.chars().count()
}

fn byte_index_at_char(text: &str, char_index: usize) -> usize {
    if char_index == 0 {
        return 0;
    }
    text.char_indices()
        .nth(char_index)
        .map(|(idx, _)| idx)
        .unwrap_or_else(|| text.len())
}

fn remove_char_at(text: &mut String, char_index: usize) -> bool {
    let start = byte_index_at_char(text, char_index);
    if start >= text.len() {
        return false;
    }
    let ch = text[start..].chars().next().unwrap();
    let end = start + ch.len_utf8();
    text.replace_range(start..end, "");
    true
}

fn normalize_paste_text(text: &str) -> String {
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text.to_string()
    }
}

fn sanitize_api_key_text(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

fn strip_raw_mouse_report_runs(input: &str, cursor: usize) -> Option<(String, usize)> {
    // First pass: strip the well-defined control-sequence fragment
    // shapes that crossterm sometimes hands us as `Char(c)` keystrokes
    // when its event reader is interrupted mid-sequence during dense
    // streaming output (#1915). This covers OSC 8 hyperlink fragments
    // (`]8;;URL`, including the closing `]8;;`) and Kitty keyboard
    // protocol fragments (`[?…u`, `[>…u`, `[?u`).
    let (after_fragments, after_fragments_cursor, fragments_changed) =
        strip_control_sequence_fragments(input, cursor);

    // Second pass: the existing run-based filter handles SGR mouse
    // reports (`[<35;44;18M`) and the multi-terminator burst shape
    // (`5;46;18M;48;18M`) introduced in e63a4ba4a. It operates on a
    // narrow char set so it can't be confused with user-typed text.
    let chars: Vec<char> = after_fragments.chars().collect();
    let mut output = String::with_capacity(after_fragments.len());
    let mut new_cursor = 0usize;
    let mut changed = fragments_changed;
    let mut index = 0usize;

    while index < chars.len() {
        if is_raw_mouse_report_run_char(chars[index]) {
            let start = index;
            while index < chars.len() && is_raw_mouse_report_run_char(chars[index]) {
                index += 1;
            }
            let run = &chars[start..index];
            if let Some(keep) = raw_mouse_report_keep_mask(run) {
                changed = true;
                for (offset, ch) in run.iter().copied().enumerate() {
                    if !keep[offset] {
                        continue;
                    }
                    if start + offset < cursor {
                        new_cursor += 1;
                    }
                    output.push(ch);
                }
                continue;
            }
            for (offset, ch) in run.iter().copied().enumerate() {
                if start + offset < after_fragments_cursor {
                    new_cursor += 1;
                }
                output.push(ch);
            }
            continue;
        }

        if index < after_fragments_cursor {
            new_cursor += 1;
        }
        output.push(chars[index]);
        index += 1;
    }

    changed.then(|| {
        let cursor = new_cursor.min(char_count(&output));
        (output, cursor)
    })
}

fn is_raw_mouse_report_run_char(ch: char) -> bool {
    matches!(ch, '\x1b' | '[' | '<' | ';' | ':' | 'M' | 'm') || ch.is_ascii_digit()
}

fn looks_like_raw_mouse_report_run(run: &[char]) -> bool {
    if run.len() < 5 {
        return false;
    }
    let has_separator = run.iter().any(|ch| matches!(ch, ';' | ':'));
    let terminators = run.iter().filter(|ch| matches!(ch, 'M' | 'm')).count();
    if !has_separator || terminators == 0 {
        return false;
    }
    has_sgr_mouse_marker(run) || terminators >= 2
}

fn has_sgr_mouse_marker(run: &[char]) -> bool {
    run.windows(2).any(|window| window == ['[', '<'])
}

fn raw_mouse_report_keep_mask(run: &[char]) -> Option<Vec<bool>> {
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut index = 0usize;

    while index < run.len() {
        let (start, body_start) = if run[index] == '\x1b'
            && run.get(index + 1) == Some(&'[')
            && run.get(index + 2) == Some(&'<')
        {
            (index, index + 3)
        } else if run[index] == '[' && run.get(index + 1) == Some(&'<') {
            (index, index + 2)
        } else {
            index += 1;
            continue;
        };

        let mut end = body_start;
        let mut has_digit = false;
        let mut has_separator = false;
        let mut matched = false;
        while end < run.len() {
            match run[end] {
                '0'..='9' => {
                    has_digit = true;
                    end += 1;
                }
                ';' | ':' => {
                    has_separator = true;
                    end += 1;
                }
                'M' | 'm' if has_digit && has_separator => {
                    ranges.push((start, end + 1));
                    index = end + 1;
                    matched = true;
                    break;
                }
                _ => break,
            }
        }
        if !matched {
            index = index.saturating_add(1);
        }
    }

    if ranges.is_empty() {
        if looks_like_raw_mouse_report_run(run) {
            return Some(vec![false; run.len()]);
        }
        return None;
    }

    ranges.sort_unstable_by_key(|(start, _)| *start);
    let first_start = ranges[0].0;
    let mut prefix_start = first_start;
    while prefix_start > 0 && is_raw_mouse_report_fragment_char(run[prefix_start - 1]) {
        prefix_start -= 1;
    }
    if prefix_start < first_start
        && looks_like_raw_mouse_report_fragment(&run[prefix_start..first_start])
    {
        ranges.push((prefix_start, first_start));
    }

    let last_end = ranges.iter().map(|(_, end)| *end).max().unwrap_or_default();
    if last_end < run.len() && looks_like_raw_mouse_report_fragment(&run[last_end..]) {
        ranges.push((last_end, run.len()));
    }

    ranges.sort_unstable_by_key(|(start, _)| *start);
    let mut keep = vec![true; run.len()];
    for (start, end) in ranges {
        for slot in keep.iter_mut().take(end.min(run.len())).skip(start) {
            *slot = false;
        }
    }
    Some(keep)
}

fn is_raw_mouse_report_fragment_char(ch: char) -> bool {
    matches!(ch, ';' | ':' | 'M' | 'm') || ch.is_ascii_digit()
}

fn looks_like_raw_mouse_report_fragment(run: &[char]) -> bool {
    if run.len() < 4 {
        return false;
    }
    run.iter().any(|ch| ch.is_ascii_digit())
        && run.iter().any(|ch| matches!(ch, ';' | ':'))
        && run.iter().any(|ch| matches!(ch, 'M' | 'm'))
}

/// Scan `input` for control-sequence fragment shapes (#1915) — OSC 8
/// hyperlinks and Kitty keyboard protocol responses — and excise each
/// match. Returns `(output, new_cursor, changed)`. Cursor positions
/// inside an excised fragment are moved to the fragment's start.
///
/// The match shapes are deliberately narrow so legitimate text like
/// `[is this ok?]` or a typed URL survives untouched:
///
/// - **OSC 8**: `(\x1b?)] 8 ; ...` consuming everything up to the
///   first BEL (`\x07`), `\x1b\\`, lone `\\`, or the next `\x1b]8;`
///   block — terminator characters are optional because crossterm may
///   have already consumed them.
/// - **Kitty CSI**: `(\x1b?) [ (? | > | < | =) ... u` — the
///   private-parameter prefix is what distinguishes a Kitty response
///   from a user-typed `[…u` (which is exceedingly rare and would
///   need an explicit private-parameter byte to be a real CSI).
fn strip_control_sequence_fragments(input: &str, cursor: usize) -> (String, usize, bool) {
    let chars: Vec<char> = input.chars().collect();
    let mut output = String::with_capacity(input.len());
    let mut new_cursor = 0usize;
    let mut changed = false;
    let mut index = 0usize;

    while index < chars.len() {
        if let Some(end) = match_osc8_fragment(&chars, index) {
            // The excised span contributes nothing to `output`, so
            // `new_cursor` simply doesn't tick for any of those
            // characters. A cursor that was inside the span ends up at
            // the fragment's start position in the rewritten input,
            // which matches the existing run-stripper's behavior.
            index = end;
            changed = true;
            continue;
        }

        if let Some(end) = match_kitty_csi_fragment(&chars, index) {
            index = end;
            changed = true;
            continue;
        }

        if index < cursor {
            new_cursor += 1;
        }
        output.push(chars[index]);
        index += 1;
    }

    let cursor = new_cursor.min(char_count(&output));
    (output, cursor, changed)
}

/// If an OSC 8 hyperlink fragment starts at `chars[start]`, return its
/// end index (exclusive). The leading `ESC` is optional because
/// crossterm's event parser often consumes it before reclassifying the
/// tail as keystrokes.
fn match_osc8_fragment(chars: &[char], start: usize) -> Option<usize> {
    let body_start = if chars.get(start) == Some(&'\x1b')
        && chars.get(start + 1) == Some(&']')
        && chars.get(start + 2) == Some(&'8')
        && chars.get(start + 3) == Some(&';')
    {
        start + 4
    } else if chars.get(start) == Some(&']')
        && chars.get(start + 1) == Some(&'8')
        && chars.get(start + 2) == Some(&';')
    {
        start + 3
    } else {
        return None;
    };

    // After `]8;` we expect the OSC 8 payload: an optional second `;`
    // (params separator), then the URL (or empty for the closing
    // wrapper), then a terminator. We deliberately stop at the first
    // ASCII whitespace so a typed `]8;` followed by real prose can't
    // swallow the user's words — real OSC 8 URLs don't contain spaces.
    let mut end = body_start;
    while end < chars.len() {
        let ch = chars[end];
        // BEL terminator.
        if ch == '\x07' {
            return Some(end + 1);
        }
        // `ESC \\` string terminator (ST).
        if ch == '\x1b' && chars.get(end + 1) == Some(&'\\') {
            return Some(end + 2);
        }
        // Lone `\\` — crossterm sometimes delivers ST with the leading
        // ESC already consumed, leaving just `\\` as a Char keystroke.
        if ch == '\\' {
            return Some(end + 1);
        }
        // Start of the next OSC 8 wrapper (closing `]8;;` glued to the
        // body) — close the current fragment here so the next iteration
        // matches that one separately.
        if ch == '\x1b' && chars.get(end + 1) == Some(&']') {
            return Some(end);
        }
        if ch == ']' && chars.get(end + 1) == Some(&'8') && chars.get(end + 2) == Some(&';') {
            return Some(end);
        }
        if ch.is_whitespace() {
            // We never crossed a terminator, so this isn't a real
            // fragment — give up rather than eat user prose.
            return None;
        }
        end += 1;
    }

    // Reached end of input without a terminator or whitespace. Treat as
    // a fragment in flight (its tail will arrive on a later keystroke
    // and get filtered then).
    Some(end)
}

/// If a private-parameter CSI fragment starts at `chars[start]`, return its
/// end index (exclusive). Shape: `(ESC)? [ (? | > | < | =) [0-9;:]* <final>`
/// where `<final>` is any ASCII letter. This covers the Kitty keyboard
/// protocol (`…u`) *and* the DEC private mode set/reset sequences a terminal
/// emits during a session — bracketed paste (`[?2004h`/`[?2004l`), mouse
/// capture (`[?1000h`), focus reporting (`[?1004h`), and synchronized output
/// (`[?2026h`). Those end in `h`/`l`, not `u`, so the old `u`-only terminator
/// let the leading `[` leak into the composer during dense streaming (#2592,
/// regression of #1915). The private-parameter byte (`?`, `>`, `<`, `=`) is
/// what keeps this distinct from text the user might plausibly type.
fn match_kitty_csi_fragment(chars: &[char], start: usize) -> Option<usize> {
    let after_csi = if chars.get(start) == Some(&'\x1b') && chars.get(start + 1) == Some(&'[') {
        start + 2
    } else if chars.get(start) == Some(&'[') {
        start + 1
    } else {
        return None;
    };

    let priv_byte = chars.get(after_csi)?;
    if !matches!(priv_byte, '?' | '>' | '<' | '=') {
        return None;
    }

    let mut end = after_csi + 1;
    let mut saw_param = false;
    while end < chars.len() {
        let ch = chars[end];
        if ch.is_ascii_digit() || ch == ';' || ch == ':' {
            saw_param = true;
            end += 1;
            continue;
        }
        // Final byte. The Kitty keyboard protocol ends in `u` and is valid
        // with no parameters (`[?u`). DEC private mode set/reset ends in
        // `h`/`l` and always carries a numeric mode — bracketed paste
        // (`[?2004h`/`l`), mouse capture (`[?1000h`), focus reporting
        // (`[?1004h`), synchronized output (`[?2026h`). Require a parameter
        // before `h`/`l` so ordinary text like `[?help]` is left untouched.
        return match ch {
            'u' => Some(end + 1),
            'h' | 'l' if saw_param => Some(end + 1),
            _ => None,
        };
    }
    None
}

const MAX_SUBMITTED_INPUT_CHARS: usize = 16_000;
/// Maximum characters displayed in the composer for oversized input.
/// Beyond this, the text is truncated for rendering but the full content
/// is preserved for model submission (#3263).
const MAX_COMPOSER_DISPLAY_CHARS: usize = 4_000;
const MAX_DRAFT_HISTORY: usize = 50;

impl AppMode {
    /// Productive keyboard cycle: Plan -> Act -> Operate -> Plan.
    ///
    /// `Auto` remains an internal variant while the real implementation is
    /// redesigned; do not expose it through user-facing mode selection (#3733).
    /// `Yolo` is kept for parse/back-compat only and is not in the Tab cycle.
    /// Operate joins the visible cycle because ordinary messages can now
    /// coordinate background workers without requiring a Workflow definition.
    pub const CYCLE: [Self; 3] = [Self::Plan, Self::Agent, Self::Operate];

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "agent" | "act" | "auto" | "1" => Some(Self::Agent),
            "plan" | "2" => Some(Self::Plan),
            "operate" | "operation" | "ops" | "3" => Some(Self::Operate),
            // Invisible one-way permission shorthand only — never a visible mode.
            "yolo" | "4" | "bypass" | "bypass-permissions" | "bypasspermissions" => {
                Some(Self::Yolo)
            }
            _ => None,
        }
    }

    #[must_use]
    pub fn from_setting(value: &str) -> Self {
        // Unreleased Multitask never shipped; normalize leftover settings to Operate.
        match value.trim().to_ascii_lowercase().as_str() {
            "multitask" | "multi" | "5" => Self::Operate,
            other => Self::parse(other).unwrap_or(Self::Agent),
        }
    }

    #[must_use]
    pub fn as_setting(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Auto => "agent",
            // Write current permission vocabulary, not the legacy YOLO label.
            Self::Yolo => "agent",
            Self::Plan => "plan",
            Self::Operate => "operate",
        }
    }

    /// Short label used in the UI footer.
    pub fn label(self) -> &'static str {
        match self {
            AppMode::Agent => "ACT",
            AppMode::Auto => "ACT",
            AppMode::Yolo => "ACT",
            AppMode::Plan => "PLAN",
            AppMode::Operate => "OPERATE",
        }
    }

    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            AppMode::Agent => "Act",
            AppMode::Auto => "Act",
            AppMode::Yolo => "Act",
            AppMode::Plan => "Plan",
            AppMode::Operate => "Operate",
        }
    }

    #[must_use]
    pub fn number(self) -> char {
        match self {
            AppMode::Agent | AppMode::Auto | AppMode::Yolo => '1',
            AppMode::Plan => '2',
            AppMode::Operate => '3',
        }
    }

    #[must_use]
    pub fn uses_agent_baseline(self) -> bool {
        matches!(self, Self::Agent | Self::Auto | Self::Operate)
    }

    /// Operate gets a higher parallel launch floor so background fan-out is
    /// not throttled to a single slot when config is low.
    #[must_use]
    pub fn mode_delegation_launch_floor(self) -> usize {
        match self {
            Self::Operate => 4,
            _ => 1,
        }
    }

    /// Localized short name for the mode picker (user-facing surface only).
    #[must_use]
    pub fn display_name_localized(self, locale: Locale) -> Cow<'static, str> {
        tr(
            locale,
            match self {
                AppMode::Agent | AppMode::Auto | AppMode::Yolo => MessageId::AppModeAgent,
                AppMode::Plan => MessageId::AppModePlan,
                AppMode::Operate => MessageId::AppModeOperate,
            },
        )
    }

    /// Localized one-line hint for the mode picker (user-facing surface only).
    #[must_use]
    pub fn picker_hint_localized(self, locale: Locale) -> Cow<'static, str> {
        tr(
            locale,
            match self {
                AppMode::Agent | AppMode::Auto | AppMode::Yolo => MessageId::AppModeAgentHint,
                AppMode::Plan => MessageId::AppModePlanHint,
                AppMode::Operate => MessageId::AppModeOperateHint,
            },
        )
    }

    #[allow(dead_code)]
    /// Description shown in help or onboarding text.
    pub fn description(self) -> &'static str {
        match self {
            AppMode::Agent | AppMode::Auto => {
                "Act mode - direct work in the current session with tools"
            }
            AppMode::Yolo => "Act mode with Full Access (legacy compatibility setting)",
            AppMode::Plan => "Plan mode - research and design before implementing",
            AppMode::Operate => "Operate mode - send tasks while Fleet workers run in parallel",
        }
    }

    #[must_use]
    pub fn next(self) -> Self {
        let Some(index) = Self::CYCLE.iter().position(|mode| *mode == self) else {
            return Self::Agent;
        };
        Self::CYCLE[(index + 1) % Self::CYCLE.len()]
    }

    #[must_use]
    pub fn previous(self) -> Self {
        let Some(index) = Self::CYCLE.iter().position(|mode| *mode == self) else {
            return Self::Agent;
        };
        Self::CYCLE[(index + Self::CYCLE.len() - 1) % Self::CYCLE.len()]
    }
}

/// Configuration required to bootstrap the TUI.
#[derive(Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct TuiOptions {
    pub model: String,
    pub workspace: PathBuf,
    pub config_path: Option<PathBuf>,
    pub config_profile: Option<String>,
    pub allow_shell: bool,
    /// Use the alternate screen buffer (fullscreen TUI).
    pub use_alt_screen: bool,
    /// Capture mouse input for internal scrolling/selection.
    pub use_mouse_capture: bool,
    /// Enable terminal bracketed-paste mode (OSC `?2004h` / `?2004l`). Defaults
    /// on; settable via `bracketed_paste = false` in `settings.toml` for the
    /// rare terminal that mishandles it.
    pub use_bracketed_paste: bool,
    /// Maximum number of concurrent sub-agents.
    pub max_subagents: usize,
    #[allow(dead_code)]
    pub skills_dir: PathBuf,
    #[allow(dead_code)]
    pub memory_path: PathBuf,
    #[allow(dead_code)]
    pub notes_path: PathBuf,
    #[allow(dead_code)]
    pub mcp_config_path: PathBuf,
    #[allow(dead_code)]
    pub use_memory: bool,
    /// Start in agent mode (defaults to agent; --yolo starts in YOLO)
    pub start_in_agent_mode: bool,
    /// Skip onboarding screens
    pub skip_onboarding: bool,
    /// Auto-approve tool executions (yolo mode)
    pub yolo: bool,
    /// Resume a previous session by ID
    pub resume_session_id: Option<String>,
    /// Pre-populate the composer with this text when the TUI starts.
    /// Used by `deepseek pr <N>` (#451) to drop the model into a
    /// session with the PR context already typed — the user can edit
    /// before sending or hit Enter to fire as-is.
    pub initial_input: Option<InitialInput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitialInput {
    /// Pre-populate the composer and wait for the user to press Enter.
    ///
    /// Used by `codewhale pr <N>` (#451) to drop the model into a session
    /// with the PR context already typed so the user can edit before sending.
    Prefill(String),
    /// Pre-populate the composer, submit it once startup is ready, then keep
    /// the interactive session open for follow-up messages (#2370).
    Submit(String),
}

/// Pre-session launch menu state for the underwater shell.
///
/// This is deliberately separate from onboarding and from the post-launch
/// empty session. It selects real session/worktree actions before the
/// transcript and composer become active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchState {
    pub visible: bool,
    pub selected: usize,
    pub worktree_input: Option<String>,
    pub status: Option<String>,
    pub workspace_session_count: usize,
    pub worktree_available: bool,
    /// Row hitboxes from the most recent launch render.
    pub row_areas: Vec<Rect>,
}

impl LaunchState {
    #[must_use]
    pub fn new(visible: bool, workspace: &std::path::Path) -> Self {
        let workspace_session_count = crate::session_manager::SessionManager::default_location()
            .and_then(|manager| manager.list_sessions())
            .map(|sessions| {
                sessions
                    .into_iter()
                    .filter(|session| {
                        crate::session_manager::workspace_scope_matches(
                            &session.workspace,
                            workspace,
                        )
                    })
                    .count()
            })
            .unwrap_or(0);
        let worktree_available = std::process::Command::new("git")
            .current_dir(workspace)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .is_ok_and(|output| output.status.success());
        Self {
            visible,
            selected: 0,
            worktree_input: None,
            status: None,
            workspace_session_count,
            worktree_available,
            row_areas: Vec::new(),
        }
    }
}

// === Sub-state structs for App field organization (#377) ===

/// Vim modal editing mode for the composer input area.
///
/// Enabled via `[composer] mode = "vim"` in `settings.toml`.  When the
/// composer vim mode is active the user starts in `Normal` mode and presses
/// `i`, `a`, or `o` to enter `Insert` mode.  `Esc` from `Insert` returns to
/// `Normal`.  Standard vim motions (`h`/`j`/`k`/`l`, `w`/`b`, `0`/`$`, `x`,
/// `dd`) work in `Normal` mode.  `Visual` is reserved for future selection
/// support and currently behaves like `Normal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VimMode {
    /// Normal / command mode — motions and operators, no text insertion.
    #[default]
    Normal,
    /// Insert mode — characters are appended at the cursor as typed.
    Insert,
    /// Visual mode — reserved for future selection support.
    Visual,
}

impl VimMode {
    /// Localized status-bar label shown in the composer border (user-facing).
    #[must_use]
    pub fn label_localized(self, locale: Locale) -> Cow<'static, str> {
        tr(
            locale,
            match self {
                Self::Normal => MessageId::VimModeNormal,
                Self::Insert => MessageId::VimModeInsert,
                Self::Visual => MessageId::VimModeVisual,
            },
        )
    }
}

/// Cached @-mention completion results to avoid re-walking the filesystem when
/// the cursor moves inside the same mention token.
#[derive(Debug, Clone)]
pub struct MentionCompletionCache {
    /// Workspace root used for this completion walk.
    pub workspace: PathBuf,
    /// Process cwd captured for cwd-relative completion entries.
    pub cwd: Option<PathBuf>,
    /// The partial text after `@` that triggered this completion.
    pub partial: String,
    /// Candidate limit used for this completion walk.
    pub limit: usize,
    /// Workspace depth limit used for this completion walk. Included so live
    /// config changes invalidate cached popup results.
    pub walk_depth: usize,
    /// Completion behavior used for this walk. Included so live config changes
    /// invalidate cached popup results.
    pub behavior: String,
    /// Whether symlink following was enabled for this completion walk.
    /// Included so live config changes invalidate cached popup results.
    pub follow_links: bool,
    /// Cached completion entries.
    pub entries: Vec<String>,
}

/// Composer input state — grouped fields for the text input area.
pub struct ComposerState {
    /// Current composer text content.
    pub input: String,
    /// Cursor position within `input` (in characters).
    pub cursor_position: usize,
    /// Single-entry kill buffer for emacs-style `Ctrl+K` cut / `Ctrl+Y` yank.
    pub kill_buffer: String,
    pub paste_burst: PasteBurst,
    /// When a large paste is consolidated at submit time, the file @mention
    /// is stored here so it can be appended to the submitted text without
    /// replacing the visible composer content (#3263).
    pub(crate) pending_paste_reference: Option<String>,
    /// When composer content is oversized, the full text is stored here
    /// while `self.input` shows a truncated preview. At submit time the
    /// full text is restored for model submission (#3263).
    pub(crate) oversized_paste_full_text: Option<String>,
    pub input_history: Vec<String>,
    pub draft_history: VecDeque<String>,
    pub clear_undo_buffer: Option<String>,
    pub history_index: Option<usize>,
    pub(crate) history_navigation_draft: Option<InputHistoryDraft>,
    pub composer_history_search: Option<ComposerHistorySearch>,
    pub selected_attachment_index: Option<usize>,
    pub slash_menu_selected: usize,
    pub slash_menu_hidden: bool,
    pub mention_menu_selected: usize,
    pub mention_menu_hidden: bool,
    /// Cached @-mention completions to avoid re-walking the filesystem when
    /// the cursor moves inside the same mention token.
    pub mention_completion_cache: Option<MentionCompletionCache>,
    /// Serialized background discovery and its bounded candidate cache. All
    /// filesystem traversal for composer completions lives behind this owner.
    pub(crate) mention_discovery: crate::tui::mention_completion::MentionDiscovery,
    /// Launch directory captured once so rendering a completion popup never
    /// needs to call `getcwd` on the UI thread.
    pub(crate) mention_cwd: Option<PathBuf>,
    /// Whether vim modal editing is enabled for this composer.
    /// Sourced from `Settings::composer_vim_mode` at startup.
    pub vim_enabled: bool,
    /// Current vim editing mode.  Only meaningful when `vim_enabled` is true.
    pub vim_mode: VimMode,
    /// Pending `d` prefix for the `dd` delete-line operator.  Set when the
    /// user presses `d` in Normal mode; cleared on the next key (either `d`
    /// to complete `dd`, or any other key to cancel).
    pub vim_pending_d: bool,
    /// When set, the cursor is the active end of a text selection and
    /// `selection_anchor` is the fixed end.  Both are char-indexed.
    /// `None` means no selection is active.
    pub selection_anchor: Option<usize>,
}

impl Default for ComposerState {
    fn default() -> Self {
        Self {
            input: String::new(),
            cursor_position: 0,
            kill_buffer: String::new(),
            paste_burst: PasteBurst::default(),
            pending_paste_reference: None,
            oversized_paste_full_text: None,
            input_history: Vec::new(),
            draft_history: VecDeque::new(),
            clear_undo_buffer: None,
            history_index: None,
            history_navigation_draft: None,
            composer_history_search: None,
            selected_attachment_index: None,
            slash_menu_selected: 0,
            slash_menu_hidden: false,
            mention_menu_selected: 0,
            mention_menu_hidden: false,
            mention_completion_cache: None,
            mention_discovery: crate::tui::mention_completion::MentionDiscovery::default(),
            mention_cwd: std::env::current_dir().ok(),
            vim_enabled: false,
            vim_mode: VimMode::Normal,
            vim_pending_d: false,
            selection_anchor: None,
        }
    }
}

/// Viewport/scroll state — fields related to transcript scrolling and caching.
pub struct ViewportState {
    pub transcript_scroll: TranscriptScroll,
    pub pending_scroll_delta: i32,
    pub mouse_scroll: MouseScrollState,
    pub transcript_cache: TranscriptViewCache,
    pub transcript_selection: TranscriptSelection,
    pub selection_autoscroll: Option<SelectionAutoscroll>,
    pub transcript_scrollbar_dragging: bool,
    pub last_transcript_area: Option<Rect>,
    pub last_composer_area: Option<Rect>,
    /// Painted band occupied by the active inline approval. Stored so wheel
    /// routing can prefer the visible card over side surfaces underneath it.
    pub last_approval_area: Option<Rect>,
    /// Outer rect of the right-hand sidebar (when visible), stored at render
    /// time so mouse hit-testing can keep scroll events over the sidebar from
    /// leaking into the transcript viewport.
    pub last_sidebar_area: Option<Rect>,
    /// WorkflowPanel rect above the composer (#4121), for mouse toggle/cancel.
    pub last_workflow_panel_area: Option<Rect>,
    pub last_workflow_cancel_area: Option<Rect>,
    pub last_transcript_top: usize,
    pub last_transcript_visible: usize,
    pub last_transcript_total: usize,
    pub last_transcript_padding_top: usize,
    pub jump_to_latest_button_area: Option<Rect>,
    /// Inner content rect of the composer (excluding border/padding),
    /// stored at render time for mouse coordinate mapping.
    pub last_composer_content: Option<Rect>,
    /// Number of rendered text lines scrolled off the top of the composer,
    /// stored at render time for mouse coordinate mapping.
    pub last_composer_scroll_offset: usize,
    /// Vertical padding above the first text line in the composer,
    /// stored at render time for mouse coordinate mapping.
    pub last_composer_top_padding: usize,
}

impl Default for ViewportState {
    fn default() -> Self {
        Self {
            transcript_scroll: TranscriptScroll::to_bottom(),
            pending_scroll_delta: 0,
            mouse_scroll: MouseScrollState::new(),
            transcript_cache: TranscriptViewCache::new(),
            transcript_selection: TranscriptSelection::default(),
            selection_autoscroll: None,
            transcript_scrollbar_dragging: false,
            last_transcript_area: None,
            last_composer_area: None,
            last_approval_area: None,
            last_sidebar_area: None,
            last_workflow_panel_area: None,
            last_workflow_cancel_area: None,
            last_transcript_top: 0,
            last_transcript_visible: 0,
            last_transcript_total: 0,
            last_transcript_padding_top: 0,
            jump_to_latest_button_area: None,
            last_composer_content: None,
            last_composer_scroll_offset: 0,
            last_composer_top_padding: 0,
        }
    }
}

/// Verdict for a hunt (#2092).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HuntVerdict {
    #[default]
    Hunting,
    Hunted,
    Wounded,
    Escaped,
}

impl HuntVerdict {
    #[must_use]
    pub fn goal_status(self) -> crate::tools::goal::GoalStatus {
        match self {
            Self::Hunting => crate::tools::goal::GoalStatus::Active,
            Self::Hunted => crate::tools::goal::GoalStatus::Complete,
            Self::Wounded => crate::tools::goal::GoalStatus::Paused,
            Self::Escaped => crate::tools::goal::GoalStatus::Blocked,
        }
    }

    #[must_use]
    pub fn from_goal_status(status: crate::tools::goal::GoalStatus) -> Self {
        match status {
            crate::tools::goal::GoalStatus::Active => Self::Hunting,
            crate::tools::goal::GoalStatus::Paused => Self::Wounded,
            crate::tools::goal::GoalStatus::Complete => Self::Hunted,
            crate::tools::goal::GoalStatus::Blocked => Self::Escaped,
        }
    }
}

/// Hunt tracking state (#2092 — was GoalState).
#[derive(Debug, Clone, Default)]
pub struct HuntState {
    pub quarry: Option<String>,
    pub token_budget: Option<u32>,
    pub tokens_used: u64,
    pub time_used_seconds: u64,
    pub continuation_count: u32,
    pub started_at: Option<Instant>,
    /// When the goal reached a terminal verdict (Hunted/Wounded/Escaped).
    /// While `None`, elapsed time keeps growing; once set, the sidebar freezes
    /// the timer at `finished_at - started_at` so completed goals stop ticking.
    pub finished_at: Option<Instant>,
    pub verdict: HuntVerdict,
}

/// Session cost and token telemetry state.
#[derive(Debug, Clone)]
pub struct SessionState {
    pub session_cost: f64,
    pub session_cost_cny: f64,
    pub subagent_cost: f64,
    pub subagent_cost_cny: f64,
    pub subagent_cost_event_seqs: HashSet<u64>,
    pub displayed_cost_high_water: f64,
    pub displayed_cost_high_water_cny: f64,
    pub last_prompt_tokens: Option<u32>,
    pub last_completion_tokens: Option<u32>,
    pub last_output_throughput: Option<TokenThroughput>,
    pub last_prompt_cache_hit_tokens: Option<u32>,
    pub last_prompt_cache_miss_tokens: Option<u32>,
    pub last_reasoning_replay_tokens: Option<u32>,
    pub total_tokens: u32,
    pub total_conversation_tokens: u32,
    /// Accumulated token breakdown for the session.
    pub total_input_tokens: u32,
    pub total_cache_hit_tokens: u32,
    pub total_cache_miss_tokens: u32,
    pub total_output_tokens: u32,
    pub turn_cache_history: VecDeque<TurnCacheRecord>,
    pub last_cache_inspection: Option<PromptInspection>,
    pub last_warmup_key: Option<CacheWarmupKey>,
    /// Tool catalog from the most recent model request.
    ///
    /// `/cache inspect` uses this to inspect the same tool schema bytes
    /// that were eligible for the provider's prefix cache.
    pub last_tool_catalog: Option<Vec<Tool>>,
    /// API base URL used by the most recent model request or cache warmup.
    pub last_base_url: Option<String>,
}

/// Sidebar hover state for mouse tooltip support.
#[derive(Debug, Clone, Default)]
pub struct SidebarHoverState {
    /// Rendered sections with their areas and full-text lines.
    pub sections: Vec<SidebarHoverSection>,
}

/// Per-row metadata for sidebar detail popovers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarRowAction {
    Command(String),
    /// Put a destructive command in the composer instead of executing it.
    /// The user confirms with Enter or cancels by editing/clearing the draft.
    #[allow(dead_code)] // destructive confirm path; mouse_ui already matches it (TUI-DOG-008)
    PrefillCommand(String),
    HotbarSlot(u8),
    ToggleAgentDetails {
        agent_id: String,
    },
    /// Drill into the child's transcript card (action tree, status, summary)
    /// in the detail pager — registered on the expanded dossier rows (#2889
    /// slice, dogfood A3).
    OpenAgentDetail {
        agent_id: String,
    },
    CancelAgent {
        agent_id: String,
    },
    /// Open the Work Graph inspector in the shared pager. Any lifecycle stop
    /// action is carried into that inspector instead of consuming row width.
    InspectWork {
        title: String,
        body: String,
        stop_action: Option<Box<SidebarRowAction>>,
    },
}

impl SidebarRowAction {
    #[must_use]
    pub fn as_command(&self) -> Option<&str> {
        match self {
            Self::Command(command) => Some(command.as_str()),
            Self::PrefillCommand(_)
            | Self::HotbarSlot(_)
            | Self::ToggleAgentDetails { .. }
            | Self::OpenAgentDetail { .. }
            | Self::CancelAgent { .. }
            | Self::InspectWork { .. } => None,
        }
    }

    #[must_use]
    pub fn is_cancel_action(&self) -> bool {
        match self {
            Self::Command(command) => command.contains(" cancel "),
            Self::PrefillCommand(command) => command.contains(" cancel "),
            Self::CancelAgent { .. } => true,
            Self::ToggleAgentDetails { .. }
            | Self::OpenAgentDetail { .. }
            | Self::InspectWork { .. }
            | Self::HotbarSlot(_) => false,
        }
    }
}

/// Per-row metadata for sidebar detail popovers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarHoverRow {
    /// Absolute row position in the terminal.
    pub row_y: u16,
    /// Text shown in the compact sidebar row.
    pub display_text: String,
    /// Full untruncated text for the popover.
    pub full_text: String,
    /// Optional additional detail line.
    pub detail: Option<String>,
    /// Whether the compact row lost information.
    pub is_truncated: bool,
    /// Slash command to execute when this row is clicked (#3028).
    /// `shell_*` job ids route through `/jobs` (e.g. `/jobs cancel
    /// shell_abc123`); task-manager ids route through `/task` (e.g.
    /// `/task show task_abc123`).
    pub click_action: Option<SidebarRowAction>,
    /// Optional narrower stop target for rows that show an inline `[x]`.
    pub stop_action: Option<SidebarRowAction>,
    pub stop_zone_start_col: Option<u16>,
    pub stop_zone_end_col: Option<u16>,
}

/// Per-section metadata for sidebar hover detection.
#[derive(Debug, Clone)]
pub struct SidebarHoverSection {
    /// Content area within the section (inside border + padding).
    pub content_area: Rect,
    /// Full original text for each content line rendered.
    pub lines: Vec<String>,
    /// Per-row metadata for rich hover popovers.
    pub rows: Vec<SidebarHoverRow>,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            session_cost: 0.0,
            session_cost_cny: 0.0,
            subagent_cost: 0.0,
            subagent_cost_cny: 0.0,
            subagent_cost_event_seqs: HashSet::new(),
            displayed_cost_high_water: 0.0,
            displayed_cost_high_water_cny: 0.0,
            last_prompt_tokens: None,
            last_completion_tokens: None,
            last_output_throughput: None,
            last_prompt_cache_hit_tokens: None,
            last_prompt_cache_miss_tokens: None,
            last_reasoning_replay_tokens: None,
            total_tokens: 0,
            total_conversation_tokens: 0,
            total_input_tokens: 0,
            total_cache_hit_tokens: 0,
            total_cache_miss_tokens: 0,
            total_output_tokens: 0,
            turn_cache_history: VecDeque::new(),
            last_cache_inspection: None,
            last_warmup_key: None,
            last_tool_catalog: None,
            last_base_url: None,
        }
    }
}

impl SessionState {
    /// Reset the accumulated token breakdown fields to zero.
    pub fn reset_token_breakdown(&mut self) {
        self.total_input_tokens = 0;
        self.total_cache_hit_tokens = 0;
        self.total_cache_miss_tokens = 0;
        self.total_output_tokens = 0;
        self.last_output_throughput = None;
    }
}

/// Evidence collected during a turn for the post-turn receipt.
#[derive(Debug, Clone)]
pub struct ToolEvidence {
    pub tool_name: String,
    pub summary: String,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingProviderSwitch {
    pub previous_provider: ApiProvider,
    pub previous_model: String,
    pub previous_model_ids_passthrough: bool,
    pub previous_route_limits: Option<RouteLimits>,
    pub previous_route_base_url: String,
    pub previous_context_window_source: crate::route_runtime::ContextWindowSource,
    pub previous_context_window_override: Option<u32>,
    pub previous_config: Config,
    pub previous_onboarding: OnboardingState,
    pub previous_onboarding_needs_api_key: bool,
    pub previous_api_key_env_only: bool,
}

/// Global UI state for the TUI.
#[allow(clippy::struct_excessive_bools)]
pub struct App {
    pub mode: AppMode,
    /// Registered hotbar actions available for future slot config/render layers.
    #[allow(dead_code)]
    pub hotbar_actions: HotbarActionRegistry,
    /// Composer sub-state (input, cursor, history, menus).
    pub composer: ComposerState,
    /// Viewport sub-state (scroll, cache, selection).
    pub viewport: ViewportState,
    /// Ocean work-surface state. Kept separate from transcript/sidebar state
    /// so the replacement shell can be removed or promoted as one unit.
    pub work_surface: crate::tui::work_surface::WorkSurfaceState,
    /// Goal sub-state.
    pub hunt: HuntState,
    /// Session sub-state (cost, tokens, telemetry).
    pub session: SessionState,
    /// Active tool restriction from custom slash command frontmatter.
    /// `None` means the current turn may use the normal tool set.
    pub active_allowed_tools: Option<Vec<String>>,
    /// True when the active custom slash command opted into pause/resume.
    pub pausable: bool,
    /// True after Esc paused a pausable command and before it is resumed or cancelled.
    pub paused: bool,
    /// Saved custom-command objective while the command is paused.
    pub paused_quarry: Option<String>,
    pub history: Vec<HistoryCell>,
    pub history_version: u64,
    /// Per-cell revision counter, kept in lockstep with `history`.
    pub history_revisions: Vec<u64>,
    /// Monotonic counter used to issue fresh per-cell revisions.
    pub next_history_revision: u64,
    pub api_messages: Vec<Message>,
    pub is_loading: bool,
    /// Timestamp of the most recent Enter while the engine was busy.
    /// Used by `enter_with_double_tap()` to detect a double-tap within 500 ms.
    pub last_enter_instant: Option<Instant>,
    /// Whether the once-per-turn provider-wait incident (#3095) has already
    /// been logged for the current turn.
    pub provider_wait_incident_logged: bool,
    /// Ghost-text follow-up suggestion shown in the composer when empty.
    /// Generated asynchronously after each completed turn; cleared on new input.
    pub prompt_suggestion: Option<String>,
    /// Monotonic turn counter for stale-suggestion protection. Incremented on
    /// each TurnStarted; background suggestion tasks capture the token and
    /// discard their result if the token no longer matches.
    pub prompt_suggestion_gen: std::sync::atomic::AtomicU64,
    /// Degraded connectivity mode; new user inputs are queued for later retry.
    pub offline_mode: bool,
    /// Whether an `EngineEvent::Error` has already been posted for the
    /// current turn. Suppresses the redundant "Turn failed:" status line
    /// that `TurnComplete { error: .. }` would otherwise emit on top of
    /// the in-transcript error cell.
    pub turn_error_posted: bool,
    /// Legacy status text sink retained for compatibility with existing call sites.
    pub status_message: Option<String>,
    /// Recent status toasts (ephemeral, newest at back).
    pub status_toasts: VecDeque<StatusToast>,
    /// Sticky status toast used for important warnings/errors.
    pub sticky_status: Option<StatusToast>,
    /// Last status text already promoted from `status_message` into toast state.
    pub last_status_message_seen: Option<String>,
    pub model: String,
    /// Persisted model selections by provider name. Loaded from settings so
    /// `/model` and the picker can surface saved provider-specific choices.
    pub provider_models: HashMap<String, String>,
    /// When true, the model is auto-selected based on request complexity
    /// rather than using a fixed model. The `/model auto` command sets this.
    /// `dispatch_user_message` calls `auto_model_heuristic` to resolve the
    /// effective model for each outbound message.
    pub auto_model: bool,
    /// Last concrete model chosen while `auto_model` is active.
    pub last_effective_model: Option<String>,
    /// Provider that actually served the latest auto-routed turn.
    pub last_effective_provider: Option<ApiProvider>,
    /// Exact non-secret identity for the provider that served the latest Auto
    /// turn. This matters for named custom providers, which all share the
    /// `ApiProvider::Custom` enum variant.
    pub(crate) last_effective_provider_identity: Option<String>,
    /// Auto decision metadata for the most recently resolved Auto turn.
    pub(crate) last_auto_route_receipt: Option<crate::model_routing::AutoRouteReceipt>,
    /// Route selected for the next turn, retained for in-flight UI details
    /// until the engine confirms the authoritative `TurnStarted` route.
    pub pending_turn_route: Option<(ApiProvider, String, bool)>,
    /// Auto decision metadata waiting to be paired with `pending_turn_route`.
    pub(crate) pending_auto_route_receipt: Option<crate::model_routing::AutoRouteReceipt>,
    /// Authoritative lifecycle metadata attached to the most recent
    /// `TurnStarted`. Kept separate from `pending_turn_route` so a preceding
    /// compaction completion cannot consume the next model turn's route.
    pub active_turn: Option<ActiveTurnMetadata>,
    /// Current API provider (mirrors `Config::api_provider`).
    /// Updated by `/provider` switches so the UI/commands can read the
    /// active backend without re-deriving it from the live config.
    pub api_provider: ApiProvider,
    /// Exact configured provider key for persistence and route restoration.
    /// Built-ins use their canonical slug; named custom providers retain the
    /// user-owned key instead of collapsing to `custom`.
    pub(crate) provider_identity: String,
    /// Additive exact configured id for persistence. `None` preserves the
    /// legacy root-level custom route even when a same-key table appears.
    pub(crate) provider_exact_id: Option<String>,
    /// Primary provider plus configured fallback providers for this session.
    pub provider_chain: Option<ProviderChain>,
    /// Per-provider auth/local readiness snapshot for the fallback chain (#2574).
    ///
    /// Captured at startup alongside `provider_chain` (where the live `Config` is
    /// in scope). `advance_fallback` consults it to skip chain entries that
    /// cannot serve a turn — hosted providers missing a key — while local
    /// providers (Ollama/vLLM/SGLang) are always ready. Stored as `(provider,
    /// ready)` pairs; lookups fall back to "ready" for providers not present so
    /// an unknown entry is tried rather than silently skipped.
    provider_readiness: Vec<(ApiProvider, bool)>,
    /// Session-local evidence from real provider requests and verification
    /// probes. Unlike `provider_readiness` above, this never treats a saved key
    /// as proof that the endpoint is healthy.
    pub(crate) provider_health: crate::provider_readiness::ProviderReadinessSnapshot,
    /// Human-readable description of the last provider fallback event.
    pub last_fallback_reason: Option<String>,
    /// True when the active provider/base URL accepts arbitrary model IDs
    /// verbatim rather than DeepSeek-only aliases.
    pub model_ids_passthrough: bool,
    /// Resolved provider/model route limits for the active runtime route.
    pub active_route_limits: Option<RouteLimits>,
    /// Exact resolved endpoint for the active runtime route. This stays
    /// separate from persisted config so endpoint-sensitive compatibility
    /// (notably Kimi Code's bare `k3`) is never inferred from a provider name
    /// alone.
    pub active_route_base_url: String,
    /// Provenance for `active_route_limits`' effective context window. This
    /// is an operator-facing receipt, not a claim about provider billing.
    pub active_context_window_source: crate::route_runtime::ContextWindowSource,
    /// User-configured provider context-window override for the active route.
    pub active_context_window_override: Option<u32>,
    /// Pending provider transition for transactional rollback when the next
    /// auth failure indicates the new provider cannot be used.
    pub pending_provider_switch: Option<PendingProviderSwitch>,
    /// Current reasoning-effort tier for DeepSeek thinking mode.
    /// Cycled via Shift+Tab; initialized from config at startup.
    pub reasoning_effort: ReasoningEffort,
    /// Whether the current effort came from an explicit user setting rather
    /// than compatibility inference from a retiring model alias.
    pub(crate) reasoning_effort_explicit: bool,
    /// Last concrete thinking tier chosen while `reasoning_effort` is auto.
    pub last_effective_reasoning_effort: Option<ReasoningEffort>,
    pub workspace: PathBuf,
    /// Immutable plugin catalogue scoped to this App's effective workspace.
    pub plugin_registry: std::sync::Arc<crate::plugins::PluginRegistry>,
    pub config_path: Option<PathBuf>,
    pub config_profile: Option<String>,
    /// Legacy executable plugin-tool directory resolved from the already
    /// loaded configuration. Slash-command inventory must not reload the full
    /// config (and thereby re-read credential-bearing fields) merely to find
    /// this path.
    pub legacy_plugin_tools_dir: Option<PathBuf>,
    pub mcp_config_path: PathBuf,
    pub skills_dir: PathBuf,
    pub skills_scan_codewhale_only: bool,
    /// Path to the user-memory file (#489). Always populated; only
    /// consulted when `use_memory` is `true`.
    pub memory_path: PathBuf,
    /// Whether the user-memory feature is enabled (#489). Mirrors
    /// `Config::memory_enabled()` at app boot. Used by the `# foo`
    /// composer interception (also gated by `moraine_fallback`),
    /// the `/memory` slash command, and tool registration for
    /// `remember`.
    pub use_memory: bool,
    /// True when legacy memory push/inject behavior should stay disabled
    /// because Moraine pull/recall is the configured memory backend.
    pub moraine_fallback: bool,
    pub use_alt_screen: bool,
    pub use_mouse_capture: bool,
    /// When true, plain Up/Down on an empty composer scroll the transcript
    /// instead of navigating input history.  Defaults to `true` when mouse
    /// capture is off: terminals that convert mouse-wheel events to arrow-key
    /// sequences (e.g. Windows CMD without `WT_SESSION`) get page-scrolling
    /// without any explicit config (#1443).
    pub composer_arrows_scroll: bool,
    /// Data-side cap for the `@`-mention popup. The renderer still limits the
    /// visible rows to available terminal height.
    pub mention_menu_limit: usize,
    /// Maximum workspace depth for `@`-mention completion walks. `0` means
    /// unlimited depth.
    pub mention_walk_depth: usize,
    /// `@`-mention completion behavior: fuzzy workspace search or deterministic
    /// directory browser.
    pub mention_menu_behavior: String,
    /// Follow symbolic links during workspace file discovery walks.
    /// When `true`, symlinked directories are traversed, enabling
    /// multi-project workspaces.
    pub workspace_follow_symlinks: bool,
    pub use_bracketed_paste: bool,
    pub use_paste_burst_detection: bool,
    /// Set to `true` the first time a real `Event::Paste` arrives during a
    /// session. Once set, `handle_paste_burst_key` short-circuits — there's
    /// no point running the rapid-keypress heuristic on a terminal that
    /// already delivers paste-as-event correctly. Avoids paste-burst false
    /// positives on Ghostty / iTerm2 / WezTerm / Windows Terminal where
    /// fast typing or IME commits could otherwise be mis-classified as a
    /// paste burst (#1322 follow-up).
    pub bracketed_paste_seen: bool,
    #[allow(dead_code)]
    pub system_prompt: Option<SystemPrompt>,
    pub auto_compact: bool,
    pub auto_compact_user_configured: bool,
    pub auto_compact_threshold_percent: f64,
    pub calm_mode: bool,
    pub low_motion: bool,
    pub constrained_frame_rate: bool,
    pub ocean_started_at: Instant,
    /// Start of the underwater shell's one-shot successful-turn exhale.
    /// Kept separate from the ambient ocean clock so completion can settle
    /// once without restarting or repainting the transcript field.
    pub ocean_completion_started_at: Option<Instant>,
    /// History length at the current turn boundary. Successful completion
    /// uses this stable index to settle only the receipts produced by that
    /// turn, never old transcript rows.
    pub ocean_turn_history_start: usize,
    /// First committed history cell participating in the current one-shot
    /// receipt-settle cascade.
    pub ocean_receipt_settle_start: Option<usize>,
    /// Enables the authored underwater phase and ambient motion system.
    pub fancy_animations: bool,
    /// Typed appearance treatment; appearance is independent from motion
    /// settings, and every underwater treatment keeps ambient life.
    pub ocean_treatment: crate::tui::ocean::OceanTreatment,
    /// Distinct pre-session menu. Once dismissed, the normal idle ocean owns
    /// the empty session and this state stays hidden.
    pub launch: LaunchState,
    /// Mouse-selected launch action, consumed by the async UI loop.
    pub pending_launch_action: Option<crate::tui::underwater::LaunchAction>,
    /// Mouse-selected hotbar slot, consumed by the async UI loop.
    pub pending_hotbar_slot: Option<u8>,
    /// Whether the renderer should wrap each frame in DEC mode 2026
    /// synchronized output. Resolved from `Settings::synchronized_output`
    /// at construction; `auto`/`on` → `true`, `off` → `false`. The Ptyxis
    /// auto-detect path in `Settings::apply_env_overrides` flips `auto`
    /// to `off` before App is built, so by the time we read this flag in
    /// the draw loop the decision is already made. See the
    /// `Settings::synchronized_output` doc for the user-facing knob.
    pub synchronized_output_enabled: bool,
    /// Header status-indicator chip mode. `"cw"` is the static default;
    /// `"whale"` and `"dots"` preserve the animated legacy choices, while
    /// `"off"` hides the chip. Loaded from settings and changed via
    /// `/config status_indicator <cw|whale|dots|off>`.
    pub status_indicator: String,
    pub show_thinking: bool,
    pub verbose_transcript: bool,
    pub show_tool_details: bool,
    pub ui_locale: Locale,
    pub cost_currency: CostCurrency,
    /// Route payment truth. Model pricing alone cannot distinguish metered
    /// API calls from OAuth or token-plan quota.
    pub billing_presentation: crate::route_billing::BillingPresentation,
    pub composer_density: ComposerDensity,
    pub composer_border: bool,
    /// Voice input state — toggled by `/voice` and the voice hotbar action.
    pub voice_enabled: bool,
    /// Auto-send after transcription when the transcript ends with an
    /// explicit send instruction ("send it" / "发送"). Toggled by `/voice-send`.
    pub voice_send_enabled: bool,
    /// AI-assisted dictation that sees the current composer text.
    /// Toggled by `/voice-control`.
    pub voice_control_enabled: bool,
    pub transcript_spacing: TranscriptSpacing,
    pub sidebar_width_percent: u16,
    pub sidebar_focus: SidebarFocus,
    /// Sidebar hover state for mouse tooltip support.
    pub sidebar_hover: SidebarHoverState,
    /// Current hover tooltip text, if any.
    pub sidebar_hover_tooltip: Option<String>,
    /// Last successfully rendered Work panel summary. Transient mutex misses
    /// should not wipe completed checklist/strategy state from the sidebar.
    pub(crate) cached_work_summary: Option<SidebarWorkSummary>,
    /// Browsing context from the last dismissed `/model` picker, so reopening
    /// restores the view mode and highlighted row instead of resetting to the
    /// top (#4109 picker memory). Session-scoped, never persisted.
    pub model_picker_memory: Option<ModelPickerMemory>,
    /// Browsing context from the last dismissed `/provider` picker.
    pub provider_picker_memory: Option<ProviderPickerMemory>,
    /// Last known mouse position for tooltip placement.
    pub last_mouse_pos: Option<(u16, u16)>,
    /// Whether the user is currently dragging the sidebar resize handle.
    pub sidebar_resizing: bool,
    /// Mouse column at the start of a sidebar-resize drag.
    pub sidebar_resize_anchor_x: u16,
    /// Sidebar width in columns at the start of a sidebar-resize drag.
    pub sidebar_resize_anchor_width: u16,
    /// Last sidebar area rendered (for mouse hit-testing the resize handle).
    pub last_sidebar_area: Option<Rect>,
    /// Last total chat/sidebar width considered for sidebar rendering.
    pub last_sidebar_host_width: Option<u16>,
    /// Handle rect painted on the left edge of the sidebar (1 col).
    pub last_sidebar_handle_area: Option<Rect>,
    /// Total horizontal space (chat + sidebar) used to compute the percentage
    /// during sidebar resize drag.
    pub sidebar_resize_total_width: u16,
    /// Sidebar width changed during this drag and needs persistence.
    pub sidebar_width_dirty: bool,
    /// Sidebar focus/hidden state changed and needs persistence.
    pub sidebar_focus_dirty: bool,
    /// Whether the session-context panel is enabled (#504).
    pub context_panel: bool,
    /// Minimum number of consecutive safe tool cells needed for auto-collapse.
    pub tool_collapse_threshold: usize,
    /// Tool runs the user explicitly expanded. Stores original history indices.
    pub expanded_tool_runs: HashSet<usize>,
    /// Current dense tool-run collapse behavior.
    pub tool_collapse_mode: ToolCollapseMode,
    /// File-tree pane state. `None` when hidden; `Some` when visible.
    pub file_tree: Option<crate::tui::file_tree::FileTreeState>,
    /// Whether the file-tree pane was actually rendered in the last frame.
    /// Set false when the terminal is too narrow to show the tree.
    pub file_tree_visible: bool,
    #[allow(dead_code)]
    pub compact_threshold: usize,
    pub max_input_history: usize,
    pub allow_shell: bool,
    pub verbosity: Option<String>,
    pub max_subagents: usize,
    /// Per-SSE-chunk idle timeout for streamed turns, in seconds.
    pub stream_chunk_timeout_secs: u64,
    /// Cached sub-agent snapshots for UI views.
    pub subagent_cache: Vec<SubAgentResult>,
    /// First time this TUI observed each terminal sub-agent card.
    pub subagent_terminal_seen_at: HashMap<String, Instant>,
    /// Last known per-agent progress text for running sub-agents.
    pub agent_progress: HashMap<String, String>,
    /// Agent rows expanded by direct sidebar interaction.
    pub expanded_sidebar_agents: HashSet<String>,
    /// Parent/depth metadata for live progress-only sub-agent rows.
    pub agent_progress_meta: HashMap<String, AgentProgressMeta>,
    /// In-transcript sub-agent card index by `agent_id` (issue #128).
    /// Maps each live sub-agent to the `HistoryCell::SubAgent` it renders
    /// into, so successive mailbox envelopes mutate the same cell rather
    /// than spawning duplicates.
    pub subagent_card_index: HashMap<String, usize>,
    /// History index of the most recent FanoutCard. Sibling sub-agents
    /// spawned by the same `rlm` invocation route into this card; reset
    /// when a fresh fanout-family tool call starts.
    pub last_fanout_card_index: Option<usize>,
    /// Most recently observed sub-agent dispatch tool name (set on
    /// `ToolCallStarted` for `agent` / `rlm` / etc., cleared
    /// after the first `Started` mailbox envelope routes through it).
    pub pending_subagent_dispatch: Option<String>,
    /// Animation anchor for status-strip active sub-agent spinner.
    pub agent_activity_started_at: Option<Instant>,
    /// Monotonic counter for stable agent labels (#3030).
    /// Incremented each time a sub-agent is spawned; used to generate
    /// "Agent 1", "Agent 2", etc.
    pub agent_counter: u64,
    /// Maps raw agent_id to a stable user-facing label (#3030).
    /// Populated when `AgentSpawned` fires; read by sidebar rendering.
    pub agent_label_map: HashMap<String, String>,
    /// Last time a sub-agent progress event triggered a redraw.
    /// Used to throttle redraws under high sub-agent concurrency (#3033).
    pub last_agent_progress_redraw: Option<Instant>,
    /// Last time a workflow `budget_updated` event was allowed to request a
    /// repaint. High-signal workflow events (task/run lifecycle) always paint;
    /// budget-only chatter is paced under fan-out (#4095 residual).
    pub last_workflow_budget_redraw: Option<Instant>,
    pub ui_theme: UiTheme,
    /// Parsed `background_color` setting, kept separately from `ui_theme` so
    /// an explicit override remains distinguishable even when it happens to
    /// equal the current named theme's default surface and can still carry
    /// into previews of other themes.
    pub background_color_override: Option<Color>,
    /// Active named theme. Drives the cell-level color remap in
    /// `tui::color_compat::ColorCompatBackend` so community presets
    /// (Catppuccin, Tokyo Night, Dracula, Gruvbox) propagate to every
    /// render site, not just the handful that read `app.ui_theme`.
    pub theme_id: palette::ThemeId,
    // Onboarding
    pub onboarding: OnboardingState,
    pub onboarding_needs_api_key: bool,
    pub onboarding_provider: ApiProvider,
    pub onboarding_workspace_trust_gate: bool,
    /// True when onboarding opened only because a returning user's configured
    /// provider is missing its key. Esc then exits to the offline composer
    /// instead of walking back through first-run steps.
    pub onboarding_missing_key_recovery: bool,
    /// First-run route receipts used by the mental-model screen's Back action.
    pub onboarding_had_api_key_step: bool,
    pub onboarding_had_trust_step: bool,
    pub api_key_env_only: bool,
    pub api_key_input: String,
    pub api_key_cursor: usize,
    // Hooks system
    pub hooks: HookExecutor,
    #[allow(dead_code)]
    pub yolo: bool,
    /// One-shot YOLO→Act+Bypass migration notice for this session (#0.8.68 M6).
    yolo_compat_notified: bool,
    /// One-shot Shift+Tab/Ctrl+T rebinding notice for this session (#0.8.68 M3).
    keybinding_migration_notified: bool,
    /// Durable Agent-era permission baseline that Plan/YOLO derive from and
    /// restore to (#3386). Refreshed from the live fields whenever the user
    /// leaves Agent mode; see [`base_policy_for_mode`] and `set_mode`.
    mode_prefs: ModeSessionPrefs,
    /// True when config/requirements supplied an approval policy. In that
    /// case the TUI-only Shift+Tab preference must not loosen it.
    approval_policy_locked: bool,
    /// True only when the controlling policy is the user's editable root
    /// config.toml key. An explicit Shift+Tab may migrate that key to the
    /// durable TUI posture; higher-precedence sources remain immutable.
    approval_policy_root_editable: bool,
    /// True only when an organization requirements file owns approval policy.
    /// Unlike a user-owned config key, this source cannot be edited in-app.
    approval_policy_requirements_managed: bool,
    // Clipboard handler
    pub clipboard: ClipboardHandler,
    // Tool approval session allowlist
    pub approval_session_approved: HashSet<String>,
    /// Approval keys (or tool names) the user has denied or aborted in
    /// this session. Subsequent re-requests for the same approval key
    /// auto-deny without re-prompting (#360) — the model can retry a
    /// dangerous command after being told no, but the user shouldn't
    /// have to keep dismissing the same dialog.
    pub approval_session_denied: HashSet<String>,
    pub approval_mode: ApprovalMode,
    // Modal view stack (approval/help/etc.)
    pub view_stack: ViewStack,
    /// Last `request_user_input` prompt, retained so a failed modal submit can reopen (#1198).
    pub pending_user_input_prompt: Option<(String, crate::tools::user_input::UserInputRequest)>,
    /// Esc-Esc backtrack state machine (#133). `Inactive` by default; first
    /// Esc primes, second Esc opens the live-transcript overlay scoped to
    /// previous user messages so the user can rewind a turn.
    pub backtrack: crate::tui::backtrack::BacktrackState,
    /// Current session ID for auto-save updates
    pub current_session_id: Option<String>,
    /// Last non-contended Work snapshot captured in this App. The outer
    /// option distinguishes "never captured" from a captured empty state.
    pub(crate) last_known_work_state: Option<Option<SessionWorkState>>,
    /// Metadata for the active session, cached in memory so automatic
    /// checkpoints never synchronously reload and parse a growing JSON file on
    /// the UI thread.
    pub(crate) current_session_metadata: Option<SessionMetadata>,
    /// Metadata-only registry of large tool outputs produced in this session.
    pub session_artifacts: Vec<ArtifactRecord>,
    /// Trust mode - allow access outside workspace
    pub trust_mode: bool,
    /// Translation mode — when enabled, the model is instructed to respond in
    /// the current locale and a post-hoc translation layer replaces any
    /// remaining English output before it reaches the user.
    pub translation_enabled: bool,
    /// Ordered list of footer items the user wants visible. Sourced from
    /// `tui.status_items` in `~/.deepseek/config.toml` at startup; mutated
    /// live by `/statusline`. The renderer iterates this slice; no item is
    /// hardcoded in the footer code path.
    pub status_items: Vec<crate::config::StatusItem>,
    /// Project documentation (AGENTS.md or CLAUDE.md)
    #[allow(dead_code)]
    pub project_doc: Option<String>,
    /// Plan state for tracking tasks
    pub plan_state: SharedPlanState,
    /// Whether a plan follow-up prompt is waiting for user input
    pub plan_prompt_pending: bool,
    /// Exact graph proposal rendered in the current Plan confirmation. Plan
    /// acceptance must present this identity again or fail closed.
    pub pending_plan_proposal_id: Option<crate::work_graph::ProposalId>,
    /// Whether update_plan was called during the current turn
    pub plan_tool_used_in_turn: bool,
    /// Todo list for `TodoWriteTool`. Read by the plan confirmation modal to
    /// show the active checklist alongside the plan.
    pub todos: SharedTodoList,
    /// Durable runtime services exposed to model-visible task/automation tools.
    pub runtime_services: RuntimeToolServices,
    /// Last MCP manager/discovery snapshot shown in the UI.
    pub mcp_snapshot: Option<crate::mcp::McpManagerSnapshot>,
    /// Number of MCP servers declared in the user's config at app boot.
    /// Used by the footer chip (#502) so a count is visible even before
    /// the user runs `/mcp` for the first time. `0` hides the chip.
    pub mcp_configured_count: usize,
    /// Set after in-TUI MCP config edits because the engine caches its MCP pool.
    pub mcp_restart_required: bool,
    /// Tool execution log
    pub tool_log: Vec<String>,
    /// Active skill to apply to next user message
    pub active_skill: Option<String>,
    /// Content-bound plugin authority carried with `active_skill`, when the
    /// selected skill came from a reviewed plugin bundle.
    pub active_skill_provenance: Option<crate::plugins::types::PluginAuthority>,
    /// Cached (name, description) pairs from the skill registry.
    /// Populated once at startup and refreshed on install/uninstall so
    /// the slash menu can show skills without filesystem I/O on every keystroke.
    pub cached_skills: Vec<(String, String)>,
    /// Tool call cells by tool id (for cells already finalized in `history`).
    /// While a tool call is in flight inside `active_cell`, it is tracked by
    /// `active_tool_entries` instead and migrated here at flush time.
    pub tool_cells: HashMap<String, usize>,
    /// Full tool input/output keyed by history cell index.
    pub tool_details_by_cell: HashMap<usize, ToolDetailRecord>,
    /// Linked context references keyed by the visible user history cell that
    /// introduced them.
    pub context_references_by_cell: HashMap<usize, Vec<SessionContextReference>>,
    /// Session-wide context references persisted with saved sessions.
    pub session_context_references: Vec<SessionContextReference>,
    /// In-flight tool/exec group for the current turn. Mutated in place as
    /// parallel tool calls start and complete; flushed into `history` on
    /// `TurnComplete`.
    pub active_cell: Option<ActiveCell>,
    /// Revision counter for `active_cell`. Combined with `active_cell.revision`
    /// when feeding the transcript cache so cached lines for the synthetic
    /// active-cell row are invalidated on every mutation.
    pub active_cell_revision: u64,
    /// Pending tool details for entries that live inside `active_cell`.
    /// Keyed by tool id rather than cell index because the active cell's
    /// virtual index can shift (orphan completions push real cells in
    /// between). Migrated into `tool_details_by_cell` on flush.
    pub active_tool_details: HashMap<String, ToolDetailRecord>,
    /// Completion timestamps for entries still living inside `active_cell`.
    /// The transcript keeps completed entries until turn flush, but the
    /// sidebar can use these timestamps to let settled live rows expire.
    pub active_tool_entry_completed_at: HashMap<usize, Instant>,
    /// Active exploring cell entry index (within `active_cell.entries`).
    /// `None` once the active cell flushes or no exploring entry exists.
    pub exploring_cell: Option<usize>,
    /// Mapping of exploring tool ids to `(entry index in active_cell, entry
    /// within ExploringCell)`. Used to update individual exploring entries
    /// when their tools complete.
    pub exploring_entries: HashMap<String, (usize, usize)>,
    /// Tool calls that should be ignored by the UI
    pub ignored_tool_calls: HashSet<String>,
    /// Last exec wait command shown (for duplicate suppression)
    pub last_exec_wait_command: Option<String>,
    /// Current streaming assistant cell
    pub streaming_message_index: Option<usize>,
    /// True after a local cancel key has been handled and before the engine's
    /// authoritative TurnComplete arrives. Stream events already queued for
    /// the cancelled turn are ignored so text does not keep appearing after
    /// Ctrl+C/Esc returns focus to the composer.
    pub suppress_stream_events_until_turn_complete: bool,
    /// Index into `active_cell.entries` of the thinking entry currently being
    /// streamed. `None` when no thinking block is in flight. P2.3 routes
    /// thinking into the active cell so it groups visually with tool calls
    /// until the next assistant prose chunk flushes the group into history.
    pub streaming_thinking_active_entry: Option<usize>,
    /// Instant of the last throttled active-cell revision bump for the
    /// in-flight thinking stream (#1620). Reasoning chunks arrive faster than
    /// the eye can read, and each bump invalidates the active cell's wrap
    /// cache, forcing a full re-wrap. We debounce intermediate bumps to a
    /// time window so high-frequency thinking deltas no longer trigger a
    /// re-render per character. `None` means "no bump since the last
    /// finalize" so the first chunk of a block always renders immediately.
    pub thinking_revision_last_bump_at: Option<Instant>,
    /// Newline-gated streaming collector state.
    pub streaming_state: StreamingState,
    /// Live approximate output tokens for the current assistant stream.
    pub streaming_output_token_estimate: u64,
    /// Accumulated reasoning text
    pub reasoning_buffer: String,
    /// Live reasoning header extracted from bold text
    pub reasoning_header: Option<String>,
    /// Last completed reasoning block
    pub last_reasoning: Option<String>,
    /// Tool calls captured for the pending assistant message
    pub pending_tool_uses: Vec<(String, String, Value)>,
    /// User messages queued while a turn is running
    pub queued_messages: VecDeque<QueuedMessage>,
    /// Draft queued message being edited
    pub queued_draft: Option<QueuedMessage>,
    /// Legacy pending-steer bucket retained for session compatibility. New
    /// in-flight input uses Enter for same-turn steering and Tab for queued
    /// follow-ups; Esc only cancels the active turn.
    pub pending_steers: VecDeque<QueuedMessage>,
    /// Engine-rejected steers (e.g. a tool was already running and couldn't be
    /// cancelled cleanly). Surfaced in the pending-input preview so the user
    /// knows the steer was deferred to end-of-turn. Today no engine path
    /// produces these; the field is scaffolding for a future signalling
    /// channel and the bucket renders with a rejected-steer label when
    /// populated.
    pub rejected_steers: VecDeque<String>,
    /// Legacy resend flag for pending steer recovery.
    pub submit_pending_steers_after_interrupt: bool,
    /// Start time for current turn
    pub turn_started_at: Option<Instant>,
    /// Most recent engine event observed for the current turn. This is
    /// separate from `turn_started_at` because the latter drives elapsed-time
    /// UI and must not be reset during long but healthy turns.
    pub turn_last_activity_at: Option<Instant>,
    /// Sum of completed turn durations for this `App` instance (#448
    /// follow-up). Drives the footer's `worked Nh Mm` chip so the
    /// label reflects actual model work, not wall-clock since launch.
    /// Incremented on `TurnComplete` from the elapsed time of the
    /// just-finished turn. Resets per launch.
    pub cumulative_turn_duration: std::time::Duration,
    /// DeepSeek account balance, refreshed once per turn completion.
    /// Shared cell updated by background fetch tasks; read lock in the UI thread.
    pub balance_cell: std::sync::Arc<std::sync::Mutex<Option<crate::pricing::BalanceInfo>>>,
    /// Shared cell for async fleet-profile model-draft delivery. A background
    /// task fills it (model label + drafted profile or a failure reason) so
    /// the drafting network call never parks the event loop (#3757 review).
    #[allow(clippy::type_complexity)]
    /// Monotonic generation for model-draft requests. Bumped on each draft
    /// request and each setup/fleet wizard open, so a draft that lands after
    /// a superseding request or a wizard reopen is dropped rather than
    /// installed into the wrong (or a stale) wizard instance.
    pub draft_gen: std::sync::Arc<std::sync::atomic::AtomicU64>,
    #[allow(clippy::type_complexity)]
    pub fleet_draft_cell: std::sync::Arc<
        std::sync::Mutex<
            Option<(
                u64,
                String,
                // The `(provider, model)` route the operator picked when they
                // pressed `m` (#4093). Carried alongside the async draft so the
                // ratified profile keeps the picked cross-provider route even if
                // the model draft (which is always `provider: None`) omitted or
                // changed it. `None` for an `inherit` pick.
                Option<(String, String)>,
                // The reasoning tier selected when the operator pressed `m`
                // (#4137). `None` means inherit.
                Option<String>,
                Result<Box<crate::fleet::profile::FleetProfileDraft>, String>,
            )>,
        >,
    >,
    /// Shared cell for async constitution model-draft delivery (same pattern
    /// as `fleet_draft_cell`, so the drafting network call never parks the
    /// event loop).
    #[allow(clippy::type_complexity)]
    pub constitution_draft_cell: std::sync::Arc<
        std::sync::Mutex<
            Option<(
                u64,
                String,
                crate::localization::Locale,
                Result<Box<codewhale_config::UserConstitution>, String>,
            )>,
        >,
    >,
    /// Shared cell for async prompt suggestion delivery from background task.
    pub prompt_suggestion_cell: std::sync::Arc<std::sync::Mutex<Option<(u64, String)>>>,
    /// Tracks whether the initial balance fetch has been attempted for this session.
    pub balance_initiated: bool,
    /// Timestamp of the last balance fetch, used to debounce rapid requests.
    pub last_balance_fetch: Option<std::time::Instant>,
    /// Current runtime turn id (if known).
    pub runtime_turn_id: Option<String>,
    /// Current runtime turn status (if known).
    pub runtime_turn_status: Option<String>,
    /// Monotonic turn counter for stable user-facing labels (#3030).
    /// Incremented each time a new turn starts; displayed as "Turn N".
    pub turn_counter: u64,
    /// When the UI accepted a user message but has not observed `TurnStarted` yet.
    pub dispatch_started_at: Option<Instant>,

    /// Cached git context snapshot for the footer.
    pub workspace_context: Option<String>,
    /// Shared cell for async git context updates (#399 S1).
    pub workspace_context_cell: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    /// Timestamp for cached workspace context.
    pub workspace_context_refreshed_at: Option<Instant>,
    /// Cached background tasks for sidebar rendering.
    pub task_panel: Vec<TaskPanelEntry>,
    /// Active decision card (v0.8.43 truth-surface). When set, keyboard input
    /// is routed through the card navigation instead of the composer.
    pub decision_card: Option<crate::tui::widgets::decision_card::DecisionCard>,
    /// Unified Workflow activity surface (#4121). Lives above the composer so
    /// phase/row progress does not flood the chat transcript. Preserved after
    /// completion until the next `RunStarted` replaces it.
    pub workflow_panel: Option<crate::tui::widgets::workflow_panel::WorkflowPanel>,
    /// Wall-clock time when this TUI session started. Used by the Work
    /// sidebar projection to hide completed durable tasks that finished
    /// before the current session (bug #1913).
    pub session_started_at: chrono::DateTime<chrono::Utc>,
    /// Whether the UI needs to be redrawn.
    pub needs_redraw: bool,
    /// When true, the next draw will be a full repaint (terminal clear +
    /// all cells redrawn) instead of a ratatui incremental diff. Used by
    /// theme switches where the diff engine may miss color-only changes
    /// in sidebar cells that were previously rendered with palette constants.
    pub force_next_full_repaint: bool,
    /// When the current thinking block started (for duration tracking).
    pub thinking_started_at: Option<Instant>,
    /// Whether context compaction is currently in progress.
    pub is_compacting: bool,
    /// Whether context purge is currently in progress.
    pub is_purging: bool,
    /// Set when the user scrolls up/down during a streaming turn so subsequent
    /// streamed chunks don't yank the view back to the live tail. Cleared
    /// when the user explicitly returns to bottom or the turn completes.
    pub user_scrolled_during_stream: bool,
    /// Timestamp of the last user message send (for brief visual feedback).
    pub last_send_at: Option<Instant>,
    /// Most recent user prompt accepted for an active engine turn. Ctrl+C can
    /// restore this into an empty composer after cancelling that turn.
    pub last_submitted_prompt: Option<String>,
    /// Startup prompt should be submitted automatically after the engine is ready.
    pub auto_submit_initial_input: bool,
    /// Two-tap quit confirmation. When set, a prior Ctrl+C in idle state has
    /// armed the quit shortcut; a second Ctrl+C before this `Instant` exits
    /// the app, while expiry silently re-arms the prompt for next time.
    /// Stays `None` while a turn is in flight or a modal/picker is open so
    /// Ctrl+C keeps its current "interrupt this turn" semantics in those
    /// states. See [`App::arm_quit`] / [`App::quit_is_armed`].
    pub quit_armed_until: Option<Instant>,

    // === Prefix-Cache Stability Tracking ===
    /// Number of times the prefix (system prompt + tool specs) has changed.
    pub prefix_change_count: u64,
    /// Total number of prefix stability checks performed.
    pub prefix_checks_total: u64,
    /// Current prefix stability percentage, if known.
    pub prefix_stability_pct: Option<u32>,
    /// Description of the last prefix change, if any.
    pub last_prefix_change_desc: Option<String>,
    /// Current pinned prefix combined hash (SHA-256, 64 hex chars).
    /// Updated per-turn via PrefixCacheChange events; surfaced by
    /// `/cache stats` for cache-hit debugging.
    pub last_pinned_prefix_hash: Option<String>,

    // === Transcript filtering (#397) ===
    /// Transcript cells the user has collapsed (hidden from view).
    /// Stores **original** virtual cell indices (pre-filtering).
    pub collapsed_cells: HashSet<usize>,
    /// Thinking cells the user has folded (showing summary instead of full
    /// content). Stores **original** virtual cell indices. Toggled by Space
    /// when the composer is empty and the cursor is on a thinking cell.
    pub folded_thinking: HashSet<usize>,
    /// Mapping from filtered cell index → original virtual index.
    /// Populated during `ChatWidget::new` by filtering out collapsed cells.
    /// Used by `build_context_menu_entries` to convert line-meta indices
    /// back to original indices for the `HideCell` / `ShowCell` actions.
    pub collapsed_cell_map: Vec<usize>,

    /// Whether `/edit` has loaded the last user message into the composer and
    /// the next submit should replace (not append to) the last exchange.
    pub edit_in_progress: bool,

    /// Whether LSP diagnostics are currently enabled. Mirrors the config file
    /// `[lsp].enabled` setting. Toggled at runtime via `/lsp on|off`.
    pub lsp_enabled: bool,
    /// Current-turn LSP repair-loop summary for Ctrl-O Turn Inspector (#4107).
    pub lsp_repair: LspRepairState,
    /// Derived title for the current session shown in the composer border.
    /// Updated when `EngineEvent::SessionUpdated` fires or a saved session is loaded.
    pub session_title: Option<String>,

    /// Post-turn receipt rendered as transient composer chrome.
    /// Set when a turn completes; cleared when a new turn starts or after expiry.
    pub receipt_text: Option<String>,
    pub receipt_started_at: Option<Instant>,
    /// Tool evidence collected during the current turn for the receipt.
    pub tool_evidence: Vec<ToolEvidence>,
}

/// Message queued while the engine is busy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedMessage {
    pub display: String,
    pub skill_instruction: Option<String>,
    pub skill_provenance: Option<crate::plugins::types::PluginAuthority>,
}

/// How a freshly-typed user input should be sent.
///
/// Picked by [`App::decide_submit_disposition`] when the user hits Enter on a
/// non-empty composer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitDisposition {
    /// Engine idle and online: send immediately.
    Immediate,
    /// Park on `queued_messages` (offline, or engine busy — #382).
    Queue,
    /// Explicit steer via Ctrl+Enter (#382). Not returned by `decide_submit_disposition`.
    #[allow(dead_code)]
    Steer,
    /// Park on `queued_messages` for dispatch after TurnComplete.
    /// Legacy path; #382 unified busy states under `Queue`.
    #[allow(dead_code)]
    QueueFollowUp,
}

/// Detailed tool payload attached to a history cell.
#[derive(Debug, Clone)]
pub struct ToolDetailRecord {
    pub tool_id: String,
    pub tool_name: String,
    pub input: Value,
    pub output: Option<String>,
}

/// Lightweight task view for sidebar rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskPanelEntry {
    pub id: String,
    pub status: String,
    pub prompt_summary: String,
    pub duration_ms: Option<u64>,
    pub kind: TaskPanelEntryKind,
    pub stale: bool,
    pub elapsed_since_output_ms: Option<u64>,
    pub owner_agent_id: Option<String>,
    pub owner_agent_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskPanelEntryKind {
    Background,
}

impl QueuedMessage {
    pub fn new(display: String, skill_instruction: Option<String>) -> Self {
        Self {
            display,
            skill_instruction,
            skill_provenance: None,
        }
    }

    #[must_use]
    pub fn with_skill_provenance(
        mut self,
        provenance: Option<crate::plugins::types::PluginAuthority>,
    ) -> Self {
        self.skill_provenance = provenance;
        self
    }

    #[allow(dead_code)] // Tests and queue helpers use the display-only form; send path resolves @mentions.
    pub fn content(&self) -> String {
        if let Some(skill_instruction) = self.skill_instruction.as_ref() {
            format!(
                "{skill_instruction}\n\n---\n\nUser request: {}",
                self.display
            )
        } else {
            self.display.clone()
        }
    }
}

// === Errors ===

/// Errors that can occur while submitting API keys during onboarding.
#[derive(Debug, Error)]
pub enum ApiKeyError {
    /// The provided API key was empty.
    #[error("Failed to save API key: API key cannot be empty")]
    Empty,
    /// Persisting the API key failed.
    #[error("Failed to save API key: {source}")]
    SaveFailed { source: anyhow::Error },
}

// === Deref to ComposerState for backward compat ===

impl std::ops::Deref for App {
    type Target = ComposerState;
    fn deref(&self) -> &Self::Target {
        &self.composer
    }
}

impl std::ops::DerefMut for App {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.composer
    }
}

// === App State ===

fn default_composer_arrows_scroll(use_mouse_capture: bool) -> bool {
    default_composer_arrows_scroll_for_platform(use_mouse_capture, cfg!(windows))
}

fn default_composer_arrows_scroll_for_platform(use_mouse_capture: bool, _is_windows: bool) -> bool {
    !use_mouse_capture
}

impl App {
    /// Advance and return the model-draft generation. Call when a draft is
    /// requested or a setup/fleet wizard opens; a spawned draft that captured
    /// an older generation is dropped on delivery.
    pub fn next_draft_gen(&self) -> u64 {
        self.draft_gen
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1
    }

    /// The current model-draft generation (delivery compares against this).
    #[must_use]
    pub fn current_draft_gen(&self) -> u64 {
        self.draft_gen.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Cap on the session turn-cache history. Holds enough turns to debug a long
    /// session without being so large the on-screen `/cache` table wraps.
    pub const TURN_CACHE_HISTORY_CAP: usize = 50;

    /// Append a per-turn cache-telemetry record, trimming the oldest entry once
    /// the ring exceeds [`Self::TURN_CACHE_HISTORY_CAP`].
    pub fn push_turn_cache_record(&mut self, record: TurnCacheRecord) {
        self.session.turn_cache_history.push_back(record);
        while self.session.turn_cache_history.len() > Self::TURN_CACHE_HISTORY_CAP {
            self.session.turn_cache_history.pop_front();
        }
    }

    pub(crate) fn clear_model_scoped_telemetry(&mut self) {
        self.session.last_prompt_tokens = None;
        self.session.last_completion_tokens = None;
        self.session.last_output_throughput = None;
        self.session.last_prompt_cache_hit_tokens = None;
        self.session.last_prompt_cache_miss_tokens = None;
        self.session.last_reasoning_replay_tokens = None;
        self.session.turn_cache_history.clear();
        self.pending_turn_route = None;
        self.pending_auto_route_receipt = None;
        self.active_turn = None;
        self.last_effective_model = None;
        self.last_effective_provider = None;
        self.last_effective_provider_identity = None;
        self.last_auto_route_receipt = None;
        self.last_pinned_prefix_hash = None;
    }

    pub fn tr(&self, id: MessageId) -> Cow<'static, str> {
        tr(self.ui_locale, id)
    }

    #[cfg(test)]
    pub fn new(options: TuiOptions, config: &Config) -> Self {
        let workspace = options.workspace.clone();
        Self::new_with_plugin_registry(
            options,
            config,
            std::sync::Arc::new(crate::plugins::PluginRegistry::empty(&workspace)),
        )
    }

    #[allow(clippy::too_many_lines)]
    pub fn new_with_plugin_registry(
        options: TuiOptions,
        config: &Config,
        plugin_registry: std::sync::Arc<crate::plugins::PluginRegistry>,
    ) -> Self {
        let TuiOptions {
            model,
            workspace,
            config_path,
            config_profile,
            allow_shell,
            use_alt_screen,
            use_mouse_capture,
            use_bracketed_paste,
            max_subagents,
            skills_dir: global_skills_dir,
            memory_path,
            notes_path: _,
            mcp_config_path,
            use_memory,
            start_in_agent_mode,
            skip_onboarding,
            yolo,
            resume_session_id,
            initial_input,
        } = options;

        // Start from disk-only preferences so one-time migrations can never
        // persist terminal/environment overlays such as NO_ANIMATIONS. Apply
        // those overlays only after any normalized settings write succeeds.
        let mut settings = Settings::load_persisted().unwrap_or_else(|_| Settings::default());
        let legacy_yolo_default = settings.legacy_yolo_default_detected();
        let legacy_yolo_full_access = if legacy_yolo_default {
            let control = config.approval_policy_control(
                config_path.as_deref(),
                config_profile.as_deref(),
                &workspace,
            );
            match control {
                crate::config::ApprovalPolicyControl::Unset => {
                    if let Err(error) = settings.save() {
                        tracing::warn!(
                            "failed to normalize legacy YOLO settings; retrying next launch: {error:#}"
                        );
                    }
                    true
                }
                crate::config::ApprovalPolicyControl::RootConfig => {
                    let active_config_path =
                        crate::config::resolve_load_config_path(config_path.clone());
                    match crate::config_persistence::persist_unset_root_key(
                        active_config_path.as_deref(),
                        "approval_policy",
                    ) {
                        Ok(_) => {
                            if let Err(error) = settings.save() {
                                tracing::warn!(
                                    "removed legacy approval_policy but could not normalize settings; retrying next launch: {error:#}"
                                );
                            }
                            true
                        }
                        Err(error) => {
                            tracing::warn!(
                                "could not migrate legacy YOLO approval policy; keeping the controlling policy: {error:#}"
                            );
                            false
                        }
                    }
                }
                source => {
                    tracing::warn!(
                        "legacy YOLO setting was not allowed to override {}",
                        source.label()
                    );
                    false
                }
            }
        } else {
            false
        };
        settings.apply_env_overrides();
        let launch_visible =
            settings.launch_screen && resume_session_id.is_none() && initial_input.is_none();
        let launch = LaunchState::new(launch_visible, &workspace);

        // If settings.toml exists on disk but couldn't be parsed (we fell back
        // to defaults), surface a warning in the TUI so the user knows their
        // file is broken instead of silently losing all settings.
        let settings_parse_warning = crate::settings::Settings::path().ok().and_then(|p| {
            if p.exists() {
                std::fs::read_to_string(&p).ok().and_then(|raw| {
                    ::toml::from_str::<::toml::Value>(&raw)
                        .err()
                        .map(|e| format!("⚠ settings.toml is malformed — using defaults ({e})"))
                })
            } else {
                None
            }
        });
        let tui_prefs_warning = crate::settings::TuiPrefs::path().ok().and_then(|p| {
            if p.exists() {
                std::fs::read_to_string(&p).ok().and_then(|raw| {
                    ::toml::from_str::<::toml::Value>(&raw)
                        .err()
                        .map(|e| format!("⚠ tui.toml is malformed — using defaults ({e})"))
                })
            } else {
                None
            }
        });

        let mut provider = config.api_provider();

        // Let settings preserve runtime switches only when config/CLI did not
        // explicitly select a provider. A configured provider must not be
        // pushed back to a stale saved setting on restart.
        let config_explicitly_selects_provider = config
            .provider
            .as_deref()
            .is_some_and(|provider| !provider.trim().is_empty());
        let mut provider_identity_record = config
            .active_provider_identity(provider)
            .unwrap_or_else(|_| {
                let key = config.provider_identity_for(provider);
                let exact_id = (!(provider == ApiProvider::Custom
                    && config.uses_legacy_literal_custom_route()))
                .then(|| key.clone());
                crate::config::ProviderIdentity {
                    provider,
                    key,
                    exact_id,
                }
            });
        if !config_explicitly_selects_provider
            && let Some(ref provider_str) = settings.default_provider
            && let Ok(resolved) = config.resolve_provider_identity(provider_str)
        {
            provider = resolved.provider;
            provider_identity_record = resolved;
        }
        let provider_identity = provider_identity_record.key;
        let provider_exact_id = provider_identity_record.exact_id;
        let mut effective_auth_config = config.clone();
        effective_auth_config.provider = Some(provider_identity.clone());
        let model_ids_passthrough = effective_auth_config.model_ids_pass_through();
        let provider_chain = provider
            .kind()
            .map(|kind| ProviderChain::new(kind, &config.fallback_providers))
            .filter(|chain| chain.providers().len() > 1);

        // Snapshot per-provider readiness for the fallback chain (#2574). Uses
        // the same `has_api_key_for` helper the provider picker uses, so hosted
        // providers require a key and self-hosted ones (Ollama/vLLM/SGLang) are
        // reported ready without one. Empty when there is no fallback chain.
        let provider_readiness = provider_chain
            .as_ref()
            .map(|chain| {
                chain
                    .providers()
                    .iter()
                    .map(|kind| {
                        let provider = ApiProvider::from_kind(*kind);
                        (provider, has_api_key_for(config, provider))
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Check if the effective provider has an API key. This must happen
        // after settings.default_provider is applied; otherwise a saved
        // third-party provider can be pushed back into DeepSeek onboarding.
        let needs_api_key = !has_api_key(&effective_auth_config);
        let api_key_env_only =
            crate::config::active_provider_uses_env_only_api_key(&effective_auth_config);
        let was_onboarded = crate::tui::onboarding::is_onboarded();
        let settings_auto_compact = settings.auto_compact;
        let auto_compact_user_configured = Settings::auto_compact_explicitly_configured();
        let auto_compact_threshold_percent = settings.auto_compact_threshold_percent;
        let calm_mode = settings.calm_mode;
        let low_motion = settings.low_motion;
        let constrained_frame_rate = settings.constrained_frame_rate;
        let fancy_animations = settings.fancy_animations;
        let ocean_treatment = crate::tui::ocean::OceanTreatment::parse(&settings.ocean_treatment);
        let work_surface_placement =
            crate::tui::work_surface::WorkSurfacePlacement::parse(&settings.work_surface_placement);
        let synchronized_output_enabled = settings.synchronized_output_enabled();
        let status_indicator = settings.status_indicator.clone();
        let show_thinking = settings.show_thinking;
        let show_tool_details = settings.show_tool_details;
        let ui_locale = resolve_locale(&settings.locale);
        let cost_currency = match (settings.cost_currency.as_str(), ui_locale.tag()) {
            ("usd", "zh-Hans") => CostCurrency::Cny,
            _ => CostCurrency::from_setting(&settings.cost_currency).unwrap_or(CostCurrency::Usd),
        };
        let composer_density = ComposerDensity::from_setting(&settings.composer_density);
        let composer_border = settings.composer_border;
        let composer_vim_enabled = settings
            .composer_vim_mode
            .trim()
            .eq_ignore_ascii_case("vim");
        let transcript_spacing = TranscriptSpacing::from_setting(&settings.transcript_spacing);
        let sidebar_width_percent = settings.sidebar_width_percent;
        let sidebar_focus = SidebarFocus::from_setting(&settings.sidebar_focus);
        let max_input_history = settings.max_input_history;
        let use_paste_burst_detection = settings.paste_burst_detection;
        // Resolve the named theme from settings; unknown values were already
        // normalised to "system" in Settings::load. The background_color
        // setting still overlays on top.
        let theme_id =
            palette::ThemeId::from_name(&settings.theme).unwrap_or(palette::ThemeId::System);
        let mut ui_theme = theme_id.ui_theme();
        let background_color_override = settings
            .background_color
            .as_deref()
            .and_then(palette::parse_hex_rgb_color);
        if let Some(background) = background_color_override {
            ui_theme = ui_theme.with_background_color(background);
        }
        let provider_models = settings.provider_models.clone().unwrap_or_default();
        let model = provider_models
            .get(&provider_identity)
            .cloned()
            .or_else(|| {
                // default_model is a DeepSeek-centric setting; other providers
                // get their model from config.toml / env (e.g. OPENAI_MODEL).
                if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
                    settings.default_model.clone()
                } else {
                    None
                }
            })
            .unwrap_or(model);
        let auto_model = model.trim().eq_ignore_ascii_case("auto");
        let active_context_window_override = config.context_window_for_provider_config(provider);
        let configured_route_base_url = effective_auth_config.deepseek_base_url();
        let (active_route_limits, active_route_base_url, active_context_window_source) =
            if auto_model {
                (
                    active_context_window_override.map(|window| RouteLimits {
                        context_tokens: Some(u64::from(window)),
                        ..RouteLimits::default()
                    }),
                    configured_route_base_url,
                    if active_context_window_override.is_some() {
                        crate::route_runtime::ContextWindowSource::Configured
                    } else {
                        crate::route_runtime::ContextWindowSource::Fallback
                    },
                )
            } else {
                let saved_provider_model = config
                    .provider_config_for(provider)
                    .and_then(|provider| provider.model.as_deref());
                crate::route_runtime::resolve_route_candidate_with_context_metadata(
                    provider,
                    Some(&model),
                    saved_provider_model,
                    Some(configured_route_base_url.clone()),
                    active_context_window_override,
                    None,
                )
                .map(|resolution| {
                    (
                        crate::route_budget::known_route_limits(resolution.candidate.limits()),
                        resolution.candidate.endpoint().base_url.clone(),
                        resolution.context_window.source,
                    )
                })
                .unwrap_or((
                    None,
                    configured_route_base_url,
                    crate::route_runtime::ContextWindowSource::Fallback,
                ))
            };
        let reasoning_effort_explicit =
            settings.reasoning_effort.is_some() || config.reasoning_effort_is_explicit();
        let configured_reasoning_effort = settings
            .reasoning_effort
            .as_deref()
            .or_else(|| config.reasoning_effort());
        let threshold_model = if auto_model {
            DEFAULT_TEXT_MODEL
        } else {
            model.as_str()
        };
        let compact_threshold = crate::route_budget::compaction_threshold_for_route_at_percent(
            provider,
            threshold_model,
            active_route_limits,
            auto_compact_threshold_percent,
        );
        let auto_compact = if auto_compact_user_configured {
            settings_auto_compact
        } else {
            crate::route_budget::auto_compact_default_for_route(
                provider,
                threshold_model,
                active_route_limits,
            )
        };
        let mut reasoning_effort = if auto_model {
            ReasoningEffort::Auto
        } else {
            configured_reasoning_effort.map_or_else(ReasoningEffort::default, |s| {
                ReasoningEffort::from_setting_for_provider(s, provider)
            })
        };
        if !auto_model
            && !reasoning_effort_explicit
            && let Some(effort) = crate::config::legacy_deepseek_alias_effort_for_route(
                provider,
                &effective_auth_config.deepseek_base_url(),
                &model,
            )
        {
            reasoning_effort = ReasoningEffort::from_setting_for_provider(effort, provider);
        }

        // Resolve the saved mode separately from the permission posture.
        let preferred_mode = AppMode::from_setting(&settings.default_mode);
        let yolo_compat = yolo || (preferred_mode == AppMode::Yolo && !start_in_agent_mode);
        let initial_mode = if yolo_compat || start_in_agent_mode {
            AppMode::Agent
        } else {
            preferred_mode
        };
        let needs_workspace_trust = !yolo_compat && crate::tui::onboarding::needs_trust(&workspace);
        let onboarding = initial_onboarding_state(
            skip_onboarding,
            was_onboarded,
            needs_api_key,
            needs_workspace_trust,
        );
        let onboarding_missing_key_recovery = !skip_onboarding && was_onboarded && needs_api_key;
        let onboarding_workspace_trust_gate = onboarding_is_workspace_trust_gate(
            skip_onboarding,
            was_onboarded,
            needs_api_key,
            needs_workspace_trust,
        );

        // Durable Agent-era permission baseline (#3386). Plan/YOLO derive from
        // and restore to this. Legacy Auto inputs parse to Agent; if an older
        // caller still constructs `AppMode::Auto` directly, it projects through
        // the Agent baseline instead of enabling a fourth runtime posture. When
        // the user starts in YOLO the live shell flag is force-enabled below, so
        // the baseline shell value is taken from the interactive default (the
        // pre-mode Agent surface) rather than the YOLO-forced live mirror;
        // otherwise it mirrors the resolved `allow_shell` option, which already
        // carries that same interactive default. Using `interactive_allow_shell()`
        // here keeps the Agent baseline identical regardless of launch mode, so
        // a YOLO -> Agent downshift exposes shell (approval-gated) exactly as
        // documented, while an explicit `allow_shell = false` still hides it.
        // Trust is never part of the Agent baseline (it is YOLO-only authority).
        // Approval mirrors the configured policy.
        let explicit_approval_mode = (!legacy_yolo_full_access)
            .then_some(config.approval_policy.as_deref())
            .flatten()
            .and_then(ApprovalMode::from_config_value);
        let approval_policy_control = if legacy_yolo_full_access {
            ApprovalPolicyControl::Unset
        } else {
            config.approval_policy_control(
                config_path.as_deref(),
                config_profile.as_deref(),
                &workspace,
            )
        };
        let approval_policy_locked = approval_policy_control != ApprovalPolicyControl::Unset;
        let approval_policy_root_editable =
            approval_policy_control == ApprovalPolicyControl::RootConfig;
        let approval_policy_requirements_managed =
            approval_policy_control == ApprovalPolicyControl::Requirements;
        let saved_permission_posture = if approval_policy_locked {
            None
        } else {
            settings
                .permission_posture
                .as_deref()
                .and_then(ApprovalMode::from_config_value)
        };
        let configured_approval_mode = explicit_approval_mode
            .or(saved_permission_posture)
            .unwrap_or_default();
        let mode_prefs = ModeSessionPrefs {
            agent_allow_shell: if yolo_compat || matches!(initial_mode, AppMode::Yolo) {
                config.interactive_allow_shell()
            } else {
                allow_shell
            },
            agent_trust_mode: false,
            // The YOLO-compat launch elevates the *live* approval mirror to
            // Bypass below; the durable Agent baseline keeps the configured
            // policy so a YOLO -> Agent downshift restores it.
            agent_approval_mode: configured_approval_mode,
        };
        let allow_shell = allow_shell || yolo_compat || matches!(initial_mode, AppMode::Yolo);
        let shell_manager = new_shared_shell_manager(workspace.clone());

        // Initialize hooks executor from config, merged with project-local
        // `.codewhale/hooks.toml` (#3026).
        let hooks_config =
            crate::hooks::HooksConfig::load_with_project(config.hooks_config(), &workspace);
        let hooks = HookExecutor::new(hooks_config, workspace.clone());

        // Initialize plan state
        let plan_state = new_shared_plan_state();
        let todos = new_shared_todo_list();
        let work_runtime =
            crate::work_graph::new_shared_work_runtime(todos.clone(), plan_state.clone());

        let skills_scan_codewhale_only = config.skills_config().scan_codewhale_only();
        let skills_dir = resolve_skills_dir(&workspace, &global_skills_dir, config);
        let cached_skills = Self::discover_cached_skills(
            &workspace,
            &skills_dir,
            skills_scan_codewhale_only,
            plugin_registry.as_ref(),
        );

        let input_history = crate::composer_history::load_history();
        let mention_cwd = std::env::current_dir().ok();
        let (initial_input_text, initial_input_cursor, auto_submit_initial_input) =
            match initial_input {
                // #451: pre-populate the composer when invoked via
                // `deepseek pr <N>` (or any future caller that wants to
                // drop the model into a session with context already
                // typed). Cursor lands at the end so Enter sends as-is.
                Some(InitialInput::Prefill(text)) if !text.is_empty() => {
                    let cursor = text.chars().count();
                    (text, cursor, false)
                }
                Some(InitialInput::Submit(text)) if !text.is_empty() => {
                    let cursor = text.chars().count();
                    (text, cursor, true)
                }
                _ => (String::new(), 0, false),
            };
        let mcp_configured_count = crate::mcp::load_config_with_workspace_and_plugins(
            &mcp_config_path,
            &workspace,
            plugin_registry.as_ref(),
        )
        .map(|cfg| cfg.servers.len())
        .unwrap_or(0);
        let mut hotbar_actions = HotbarActionRegistry::with_configured_routes(
            config,
            provider,
            &model,
            &provider_models,
        );
        // #2069: expose the already-discovered skills as bindable hotbar
        // actions. Reuses the startup skill cache, so no extra filesystem I/O.
        hotbar_actions.register_skills(&cached_skills);
        let mut app = Self {
            mode: initial_mode,
            hotbar_actions,
            composer: ComposerState {
                input: initial_input_text,
                cursor_position: initial_input_cursor,
                kill_buffer: String::new(),
                paste_burst: PasteBurst::default(),
                pending_paste_reference: None,
                oversized_paste_full_text: None,
                input_history,
                draft_history: VecDeque::new(),
                clear_undo_buffer: None,
                history_index: None,
                history_navigation_draft: None,
                composer_history_search: None,
                selected_attachment_index: None,
                slash_menu_selected: 0,
                slash_menu_hidden: false,
                mention_menu_selected: 0,
                mention_menu_hidden: false,
                mention_completion_cache: None,
                mention_discovery: crate::tui::mention_completion::MentionDiscovery::default(),
                mention_cwd,
                vim_enabled: composer_vim_enabled,
                vim_mode: VimMode::Normal,
                vim_pending_d: false,
                selection_anchor: None,
            },
            viewport: ViewportState::default(),
            work_surface: crate::tui::work_surface::WorkSurfaceState::with_placement(
                work_surface_placement,
            ),
            hunt: HuntState::default(),
            session: SessionState::default(),
            active_allowed_tools: None,
            pausable: false,
            paused: false,
            paused_quarry: None,
            history: Vec::new(),
            history_version: 0,
            history_revisions: Vec::new(),
            next_history_revision: 1,
            api_messages: Vec::new(),
            is_loading: false,
            last_enter_instant: None,
            provider_wait_incident_logged: false,
            prompt_suggestion: None,
            prompt_suggestion_gen: std::sync::atomic::AtomicU64::new(0),
            offline_mode: false,
            turn_error_posted: false,
            // Surface parse warnings so the user knows their config file is
            // broken instead of silently losing all settings.
            status_message: settings_parse_warning.or(tui_prefs_warning),
            status_toasts: VecDeque::new(),
            sticky_status: None,
            last_status_message_seen: None,
            model,
            provider_models,
            auto_model,
            last_effective_model: None,
            last_effective_provider: None,
            last_effective_provider_identity: None,
            last_auto_route_receipt: None,
            pending_turn_route: None,
            pending_auto_route_receipt: None,
            active_turn: None,
            api_provider: provider,
            provider_identity,
            provider_exact_id,
            provider_chain,
            provider_readiness,
            provider_health: crate::provider_readiness::ProviderReadinessSnapshot::default(),
            last_fallback_reason: None,
            model_ids_passthrough,
            active_route_limits,
            active_route_base_url,
            active_context_window_source,
            active_context_window_override,
            pending_provider_switch: None,
            reasoning_effort,
            reasoning_effort_explicit,
            last_effective_reasoning_effort: None,
            workspace,
            plugin_registry,
            config_path,
            config_profile,
            legacy_plugin_tools_dir: config
                .tools
                .as_ref()
                .and_then(|tools| tools.plugin_dir.as_deref())
                .map(PathBuf::from),
            mcp_config_path: mcp_config_path.clone(),
            skills_dir,
            skills_scan_codewhale_only,
            memory_path,
            use_memory,
            moraine_fallback: config.moraine_fallback(),
            use_alt_screen,
            use_mouse_capture,
            use_bracketed_paste,
            use_paste_burst_detection,
            bracketed_paste_seen: false,
            system_prompt: None,
            auto_compact,
            auto_compact_user_configured,
            auto_compact_threshold_percent,
            calm_mode,
            low_motion,
            constrained_frame_rate,
            ocean_started_at: Instant::now(),
            ocean_completion_started_at: None,
            ocean_turn_history_start: 0,
            ocean_receipt_settle_start: None,
            fancy_animations,
            ocean_treatment,
            launch,
            pending_launch_action: None,
            pending_hotbar_slot: None,
            synchronized_output_enabled,
            status_indicator,
            show_thinking,
            verbose_transcript: false,
            show_tool_details,
            ui_locale,
            cost_currency,
            billing_presentation: crate::route_billing::for_route(config, provider),
            composer_density,
            composer_border,
            voice_enabled: false,
            voice_send_enabled: false,
            voice_control_enabled: false,
            transcript_spacing,
            sidebar_width_percent,
            sidebar_focus,
            sidebar_hover: SidebarHoverState::default(),
            sidebar_hover_tooltip: None,
            cached_work_summary: None,
            model_picker_memory: None,
            provider_picker_memory: None,
            last_mouse_pos: None,
            sidebar_resizing: false,
            sidebar_resize_anchor_x: 0,
            sidebar_resize_anchor_width: 0,
            last_sidebar_area: None,
            last_sidebar_host_width: None,
            last_sidebar_handle_area: None,
            sidebar_resize_total_width: 0,
            sidebar_width_dirty: false,
            sidebar_focus_dirty: false,
            context_panel: settings.context_panel,
            tool_collapse_threshold: 3,
            expanded_tool_runs: HashSet::new(),
            tool_collapse_mode: ToolCollapseMode::from_setting(&settings.tool_collapse_mode),
            file_tree: None,
            file_tree_visible: false,
            compact_threshold,
            max_input_history,
            allow_shell,
            verbosity: config.verbosity.clone(),
            max_subagents,
            stream_chunk_timeout_secs: config.stream_chunk_timeout_secs(),
            subagent_cache: Vec::new(),
            subagent_terminal_seen_at: HashMap::new(),
            agent_progress: HashMap::new(),
            expanded_sidebar_agents: HashSet::new(),
            agent_progress_meta: HashMap::new(),
            subagent_card_index: HashMap::new(),
            last_fanout_card_index: None,
            pending_subagent_dispatch: None,
            agent_activity_started_at: None,
            agent_counter: 0,
            agent_label_map: HashMap::new(),
            last_agent_progress_redraw: None,
            last_workflow_budget_redraw: None,
            ui_theme,
            background_color_override,
            theme_id,
            onboarding,
            onboarding_needs_api_key: needs_api_key,
            onboarding_provider: provider,
            onboarding_workspace_trust_gate,
            onboarding_missing_key_recovery,
            onboarding_had_api_key_step: !was_onboarded && needs_api_key,
            onboarding_had_trust_step: !was_onboarded && needs_workspace_trust,
            api_key_env_only,
            api_key_input: String::new(),
            api_key_cursor: 0,
            hooks,
            yolo: yolo_compat,
            yolo_compat_notified: false,
            keybinding_migration_notified: false,
            mode_prefs,
            approval_policy_locked,
            approval_policy_root_editable,
            approval_policy_requirements_managed,
            clipboard: ClipboardHandler::new(),
            approval_session_approved: HashSet::new(),
            approval_session_denied: HashSet::new(),
            approval_mode: if yolo_compat || matches!(initial_mode, AppMode::Yolo) {
                ApprovalMode::Bypass
            } else {
                configured_approval_mode
            },
            view_stack: ViewStack::new(),
            pending_user_input_prompt: None,
            backtrack: crate::tui::backtrack::BacktrackState::new(),
            current_session_id: None,
            last_known_work_state: None,
            current_session_metadata: None,
            session_artifacts: Vec::new(),
            trust_mode: yolo_compat || initial_mode == AppMode::Yolo,
            translation_enabled: false,
            status_items: config
                .tui
                .as_ref()
                .and_then(|tui| tui.status_items.clone())
                .unwrap_or_else(crate::config::StatusItem::default_footer),
            project_doc: None,
            plan_state,
            plan_prompt_pending: false,
            pending_plan_proposal_id: None,
            plan_tool_used_in_turn: false,
            todos,
            runtime_services: RuntimeToolServices {
                shell_manager: Some(shell_manager),
                work: Some(work_runtime),
                ..RuntimeToolServices::default()
            },
            mcp_snapshot: None,
            // Read the MCP config once at boot to know how many servers
            // the user has declared. The footer chip uses this even when
            // no live snapshot is available (#502). Cheap (just reads
            // the JSON files); errors fall through to zero so a missing
            // or malformed config simply hides the chip.
            mcp_configured_count,
            mcp_restart_required: false,
            tool_log: Vec::new(),
            active_skill: None,
            active_skill_provenance: None,
            cached_skills,
            tool_cells: HashMap::new(),
            tool_details_by_cell: HashMap::new(),
            context_references_by_cell: HashMap::new(),
            session_context_references: Vec::new(),
            active_cell: None,
            active_cell_revision: 0,
            active_tool_details: HashMap::new(),
            active_tool_entry_completed_at: HashMap::new(),
            exploring_cell: None,
            exploring_entries: HashMap::new(),
            ignored_tool_calls: HashSet::new(),
            last_exec_wait_command: None,
            streaming_message_index: None,
            suppress_stream_events_until_turn_complete: false,
            streaming_thinking_active_entry: None,
            thinking_revision_last_bump_at: None,
            streaming_state: StreamingState::new(),
            streaming_output_token_estimate: 0,
            reasoning_buffer: String::new(),
            reasoning_header: None,
            last_reasoning: None,
            pending_tool_uses: Vec::new(),
            queued_messages: VecDeque::new(),
            queued_draft: None,
            pending_steers: VecDeque::new(),
            rejected_steers: VecDeque::new(),
            submit_pending_steers_after_interrupt: false,
            turn_started_at: None,
            turn_last_activity_at: None,
            cumulative_turn_duration: std::time::Duration::ZERO,
            balance_cell: std::sync::Arc::new(std::sync::Mutex::new(None)),
            draft_gen: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            fleet_draft_cell: std::sync::Arc::new(std::sync::Mutex::new(None)),
            constitution_draft_cell: std::sync::Arc::new(std::sync::Mutex::new(None)),
            prompt_suggestion_cell: std::sync::Arc::new(std::sync::Mutex::new(None)),
            balance_initiated: false,
            last_balance_fetch: None,
            runtime_turn_id: None,
            runtime_turn_status: None,
            turn_counter: 0,
            dispatch_started_at: None,
            workspace_context: None,
            workspace_context_cell: std::sync::Arc::new(std::sync::Mutex::new(None)),
            workspace_context_refreshed_at: None,
            task_panel: Vec::new(),
            decision_card: None,
            workflow_panel: None,
            session_started_at: chrono::Utc::now(),
            needs_redraw: true,
            force_next_full_repaint: false,
            thinking_started_at: None,
            is_compacting: false,
            is_purging: false,
            user_scrolled_during_stream: false,
            last_send_at: None,
            last_submitted_prompt: None,
            auto_submit_initial_input,
            quit_armed_until: None,
            prefix_change_count: 0,
            prefix_checks_total: 0,
            prefix_stability_pct: None,
            last_prefix_change_desc: None,
            last_pinned_prefix_hash: None,
            collapsed_cells: HashSet::new(),
            folded_thinking: HashSet::new(),
            collapsed_cell_map: Vec::new(),
            edit_in_progress: false,
            lsp_enabled: config.lsp.as_ref().and_then(|l| l.enabled).unwrap_or(true),
            lsp_repair: LspRepairState::default(),
            composer_arrows_scroll: config
                .tui
                .as_ref()
                .and_then(|tui| tui.composer_arrows_scroll)
                .unwrap_or_else(|| default_composer_arrows_scroll(use_mouse_capture)),
            mention_menu_limit: settings.mention_menu_limit,
            mention_walk_depth: settings.mention_walk_depth,
            mention_menu_behavior: settings.mention_menu_behavior.clone(),
            workspace_follow_symlinks: settings.workspace_follow_symlinks,
            session_title: None,
            receipt_text: None,
            receipt_started_at: None,
            tool_evidence: Vec::new(),
        };
        if yolo_compat {
            app.notify_yolo_compat_once();
        }
        app
    }

    fn discover_cached_skills(
        workspace: &std::path::Path,
        skills_dir: &std::path::Path,
        scan_codewhale_only: bool,
        plugins: &crate::plugins::PluginRegistry,
    ) -> Vec<(String, String)> {
        crate::skills::discover_for_workspace_and_dir_with_mode_and_plugins(
            workspace,
            skills_dir,
            crate::skills::SkillDiscoveryMode::from_codewhale_only(scan_codewhale_only),
            Some(plugins),
        )
        .into_enabled()
        .list()
        .iter()
        .map(|s| (s.name.clone(), s.description.clone()))
        .collect()
    }

    pub fn refresh_skill_cache(&mut self) {
        let skills_dir = self.skills_dir.clone();
        let cached_skills = Self::discover_cached_skills(
            &self.workspace,
            &skills_dir,
            self.skills_scan_codewhale_only,
            self.plugin_registry.as_ref(),
        );
        self.hotbar_actions.replace_skills(&cached_skills);
        self.cached_skills = cached_skills;
    }

    pub fn submit_api_key(&mut self) -> Result<SavedCredential, ApiKeyError> {
        let key = self.api_key_input.trim().to_string();
        if key.is_empty() {
            return Err(ApiKeyError::Empty);
        }

        let saved = if matches!(
            self.onboarding_provider,
            ApiProvider::Deepseek | ApiProvider::DeepseekCN
        ) {
            save_api_key(&key).map_err(|source| ApiKeyError::SaveFailed { source })?
        } else {
            let path = save_api_key_for(self.onboarding_provider, &key)
                .map_err(|source| ApiKeyError::SaveFailed { source })?;
            SavedCredential::ConfigFile(path)
        };
        self.api_key_input.clear();
        self.api_key_cursor = 0;
        self.onboarding_needs_api_key = false;
        self.api_key_env_only = false;
        Ok(saved)
    }

    pub fn finish_onboarding_without_feature_intro(&mut self) {
        self.onboarding = OnboardingState::None;
        if let Err(err) = crate::tui::onboarding::mark_onboarded() {
            self.status_message = Some(format!("Failed to mark onboarding: {err}"));
        }
        self.needs_redraw = true;
    }

    /// Mark the first-run follow-up as seen without inserting a transcript
    /// message. The empty underwater launch surface owns setup guidance; a
    /// synthetic history cell would hide that surface before the user sends
    /// anything.
    pub fn maybe_show_feature_intro(&mut self) {
        if self.onboarding != OnboardingState::None {
            return;
        }
        // Never claim "setup is ready" when auth is still missing — e.g.
        // `--skip-onboarding` with no API key (#3985). Leave the flag unset so
        // the tip can appear after the user finishes provider setup.
        if self.onboarding_needs_api_key {
            return;
        }
        let mut settings = Settings::load_persisted().unwrap_or_default();
        if settings.feature_intro_shown {
            return;
        }
        settings.feature_intro_shown = true;
        if let Err(err) = settings.save() {
            self.status_message = Some(format!("Failed to save feature-intro flag: {err}"));
            // Still show the nudge; the flag write may simply retry next launch.
        }
        self.status_message = Some(self.tr(MessageId::FleetReadyNotice).into_owned());
        self.needs_redraw = true;
    }

    /// Apply a locale tag selected from the onboarding language picker (#566).
    /// Persists the value to settings.toml and immediately
    /// re-resolves `ui_locale` so the rest of onboarding renders in the new
    /// language. `App` doesn't keep `Settings` resident — it loads on entry
    /// and rewrites on exit, mirroring the pattern used by the `/config`
    /// surface.
    pub fn set_locale_from_onboarding(&mut self, tag: &str) -> anyhow::Result<()> {
        let mut settings = Settings::load_persisted().unwrap_or_else(|_| Settings::default());
        settings.set("locale", tag)?;
        settings.save()?;
        self.ui_locale = crate::localization::resolve_locale(&settings.locale);
        self.needs_redraw = true;
        Ok(())
    }

    /// Locale tag currently persisted in settings.toml (or
    /// `"auto"` when no settings file exists). Used by the onboarding
    /// language picker to highlight the current selection without `App`
    /// having to keep `Settings` resident.
    pub fn current_locale_tag(&self) -> String {
        Settings::load()
            .map(|s| s.locale)
            .unwrap_or_else(|_| "auto".to_string())
    }

    pub fn set_mode(&mut self, mode: AppMode) -> bool {
        let requested_mode = mode;
        let mode = match mode {
            AppMode::Yolo => AppMode::Agent,
            other => other,
        };
        let yolo_compat = requested_mode == AppMode::Yolo;
        let previous_mode = self.mode;
        if previous_mode == mode && !yolo_compat && !self.yolo {
            return false;
        }

        self.mode = mode;
        // Mode chip lives in the header — skip redundant status/toast copy.

        // Mode cycling is untangled from permission policy (#3386). The user
        // only edits the durable permission surface while in Agent mode, so
        // refresh the baseline from the live mirrors whenever we leave Agent —
        // before any transient Plan/YOLO policy overwrites them. This subsumes
        // the old per-mode `YoloRestoreState`/`PlanRestoreState` snapshots:
        // cross-mode hops (Plan -> YOLO, YOLO -> Plan) do not touch the baseline,
        // so YOLO's elevated authority never bleeds into the restored Agent
        // surface (#3279).
        if previous_mode.uses_agent_baseline() && !self.yolo {
            self.mode_prefs = ModeSessionPrefs {
                agent_allow_shell: self.allow_shell,
                agent_trust_mode: self.trust_mode,
                agent_approval_mode: self.approval_mode,
            };
        }

        if yolo_compat {
            // Transient full-access mirrors for legacy YOLO entry points; do not
            // persist trust/shell elevation into the durable Agent baseline.
            self.allow_shell = true;
            self.trust_mode = true;
            self.approval_mode = ApprovalMode::Bypass;
            self.yolo = true;
            self.notify_yolo_compat_once();
        } else {
            let policy = base_policy_for_mode(mode, &self.mode_prefs);
            self.allow_shell = policy.allow_shell;
            self.trust_mode = policy.trust_mode;
            self.approval_mode = policy.approval_mode;
            self.yolo = matches!(policy.approval_mode, ApprovalMode::Bypass);
        }

        if mode != AppMode::Plan {
            self.plan_prompt_pending = false;
            self.pending_plan_proposal_id = None;
            self.plan_tool_used_in_turn = false;
        }

        // Execute mode change hooks
        let context = HookContext::new()
            .with_mode(mode.label())
            .with_previous_mode(previous_mode.label())
            .with_workspace(self.workspace.clone())
            .with_model(&self.model);
        let _ = self.hooks.execute(HookEvent::ModeChange, &context);
        self.needs_redraw = true;
        true
    }

    fn notify_yolo_compat_once(&mut self) {
        if self.yolo_compat_notified {
            return;
        }
        self.yolo_compat_notified = true;
        // Per-install suppression: check the persisted flag so the toast
        // appears exactly once across sessions, not every launch.
        if let Ok(settings) = crate::settings::Settings::load()
            && settings.yolo_deprecation_shown
        {
            return;
        }
        // Persist the flag best-effort; toast still fires even if the write
        // fails (retries on the next attempt).
        if let Ok(mut settings) = crate::settings::Settings::load_persisted() {
            settings.yolo_deprecation_shown = true;
            let _ = settings.save();
        }
        self.push_status_toast(
            "Legacy full-access mode is deprecated — use Act + Full Access (Shift+Tab)".to_string(),
            StatusToastLevel::Warning,
            Some(8_000),
        );
    }

    /// One-release migration notice for the Shift+Tab/Ctrl+T rebinding: users
    /// pressing Shift+Tab expecting the old thinking cycle land here first.
    fn notify_keybinding_migration_once(&mut self) {
        if self.keybinding_migration_notified {
            return;
        }
        self.keybinding_migration_notified = true;
        self.push_status_toast(
            "Shift+Tab now cycles permissions — reasoning effort moved to Ctrl+T".to_string(),
            StatusToastLevel::Info,
            Some(8_000),
        );
    }

    /// Whether mode/thinking selection is locked because a turn is in flight.
    ///
    /// While `is_loading`, the model/permission surface the engine is acting on
    /// must not shift underneath it, so user-initiated mode and thinking changes
    /// are refused (#2982). Returns true (and posts a concise status message) if
    /// the change should be rejected — the caller leaves the selection unchanged
    /// so the chip "twitches" back instead of moving.
    fn reject_setting_change_while_busy(&mut self, what: &str) -> bool {
        if self.is_loading {
            self.status_message = Some(format!(
                "{what} is locked while a turn is running — press Esc to interrupt first"
            ));
            self.needs_redraw = true;
            true
        } else {
            false
        }
    }

    /// Cycle through productive modes: Plan → Act → Operate → Plan.
    pub fn cycle_mode(&mut self) {
        if self.reject_setting_change_while_busy("Mode") {
            return;
        }
        let next = self.mode.next();
        let _ = self.set_mode(next);
    }

    /// Cycle through modes in reverse.
    #[allow(dead_code)]
    pub fn cycle_mode_reverse(&mut self) {
        if self.reject_setting_change_while_busy("Mode") {
            return;
        }
        let next = self.mode.previous();
        let _ = self.set_mode(next);
    }

    /// Cycle reasoning-effort through the active provider's distinct tiers.
    pub fn cycle_effort(&mut self) {
        if self.reject_setting_change_while_busy("Thinking") {
            return;
        }
        self.apply_reasoning_effort_cycle();
    }

    /// Advance reasoning effort to the next tier for the active provider and
    /// surface the change: set a status message and refresh the compaction
    /// budget. Shared by the Ctrl+T shortcut (`cycle_effort`) and the hotbar
    /// `reasoning.cycle` action so the two paths cannot drift.
    pub(crate) fn apply_reasoning_effort_cycle(&mut self) {
        let requested = self
            .reasoning_effort
            .cycle_next_for_provider(self.api_provider);
        let effective = self.effective_reasoning_effort_for_active_route(requested);
        let provider = self.provider_identity_for_persistence().to_string();
        if let Some(work) = self.runtime_services.work.clone()
            && let Err(err) = work.record_reasoning_effort_change(
                self.current_session_id.as_deref(),
                requested.into(),
                effective.into(),
                &provider,
            )
        {
            self.status_message = Some(format!(
                "Reasoning effort unchanged: Work receipt failed ({err})"
            ));
            self.needs_redraw = true;
            return;
        }
        self.reasoning_effort = requested;
        self.reasoning_effort_explicit = true;
        self.last_effective_reasoning_effort = None;
        self.update_model_compaction_budget();
        self.status_message = Some(format!(
            "Reasoning effort: {}",
            Self::reasoning_effort_resolution_label(requested, effective, self.api_provider)
        ));
        self.needs_redraw = true;
    }

    /// Cycle the durable Agent permission posture: Ask → Auto-Review → Bypass.
    pub fn cycle_approval_posture(&mut self) -> bool {
        let Some(next) = self.next_approval_posture(false) else {
            return false;
        };
        if self.approval_policy_locked() {
            self.push_status_toast(
                "Permissions are controlled by config or managed requirements".to_string(),
                StatusToastLevel::Warning,
                Some(6_000),
            );
            self.needs_redraw = true;
            return false;
        }
        if let Err(err) = Self::persist_permission_posture(next) {
            self.push_status_toast(
                format!("Permissions were not changed: could not save TUI posture ({err})"),
                StatusToastLevel::Warning,
                Some(8_000),
            );
            self.needs_redraw = true;
            return false;
        }
        self.finish_approval_posture_change(next);
        true
    }

    /// Cycle permissions when the only controlling source is the user's
    /// editable root `config.toml` key. Shift+Tab is an explicit request to
    /// adopt the TUI posture, so persist the next setting first, then remove
    /// the shadowing root key. Roll back the setting if that removal fails.
    pub fn cycle_root_approval_posture(&mut self) -> bool {
        let Some(next) = self.next_approval_posture(true) else {
            return false;
        };
        if !self.approval_policy_root_editable {
            self.push_status_toast(
                "Permissions are controlled by a non-editable policy source".to_string(),
                StatusToastLevel::Warning,
                Some(6_000),
            );
            self.needs_redraw = true;
            return false;
        }

        let mut settings = match Settings::load_persisted() {
            Ok(settings) => settings,
            Err(err) => {
                self.push_status_toast(
                    format!("Permissions were not changed: could not load TUI settings ({err})"),
                    StatusToastLevel::Warning,
                    Some(8_000),
                );
                return false;
            }
        };
        let previous = settings.permission_posture.clone();
        settings.permission_posture = Some(Self::approval_posture_setting(next).to_string());
        if let Err(err) = settings.save() {
            self.push_status_toast(
                format!("Permissions were not changed: could not save TUI posture ({err})"),
                StatusToastLevel::Warning,
                Some(8_000),
            );
            return false;
        }

        let active_config_path = crate::config::resolve_load_config_path(self.config_path.clone());
        if let Err(err) = crate::config_persistence::persist_unset_root_key(
            active_config_path.as_deref(),
            "approval_policy",
        ) {
            settings.permission_posture = previous;
            let rollback = settings.save().err();
            let rollback_note = rollback
                .map(|rollback| format!("; settings rollback also failed: {rollback}"))
                .unwrap_or_default();
            self.push_status_toast(
                format!(
                    "Permissions were not changed: could not release root config policy ({err}){rollback_note}"
                ),
                StatusToastLevel::Warning,
                Some(8_000),
            );
            self.needs_redraw = true;
            return false;
        }

        self.clear_saved_approval_policy_lock();
        self.finish_approval_posture_change(next);
        true
    }

    fn next_approval_posture(&mut self, allow_root_policy: bool) -> Option<ApprovalMode> {
        if self.reject_setting_change_while_busy("Permissions") {
            return None;
        }
        if self.mode == AppMode::Plan {
            self.push_status_toast(
                "Plan is Read Only; switch to Act to change permissions".to_string(),
                StatusToastLevel::Info,
                Some(5_000),
            );
            self.needs_redraw = true;
            return None;
        }
        if allow_root_policy && !self.approval_policy_root_editable {
            return None;
        }
        Some(self.mode_prefs.agent_approval_mode.cycle_permission_next())
    }

    fn approval_posture_setting(mode: ApprovalMode) -> &'static str {
        match mode {
            ApprovalMode::Suggest => "ask",
            ApprovalMode::Auto => "auto-review",
            ApprovalMode::Bypass => "full-access",
            ApprovalMode::Never => "never",
        }
    }

    fn persist_permission_posture(next: ApprovalMode) -> anyhow::Result<()> {
        let mut settings = Settings::load_persisted()?;
        settings.permission_posture = Some(Self::approval_posture_setting(next).to_string());
        settings.save()
    }

    fn finish_approval_posture_change(&mut self, next: ApprovalMode) {
        self.set_agent_approval_posture(next);
        self.needs_redraw = true;
        // Footer permission chip is canonical — no status toast for the new
        // value, only the one-shot rebinding notice.
        self.notify_keybinding_migration_once();
    }

    /// Replace the complete durable Act baseline and project it onto the live
    /// runtime when the current mode uses that baseline. Keeping these three
    /// fields together prevents setup presets from updating a live mirror while
    /// leaving the next Plan → Act transition stale.
    pub fn set_agent_runtime_baseline(
        &mut self,
        allow_shell: bool,
        trust_mode: bool,
        approval_mode: ApprovalMode,
    ) {
        self.mode_prefs = ModeSessionPrefs {
            agent_allow_shell: allow_shell,
            agent_trust_mode: trust_mode,
            agent_approval_mode: approval_mode,
        };
        if self.mode.uses_agent_baseline() {
            let policy = base_policy_for_mode(self.mode, &self.mode_prefs);
            self.allow_shell = policy.allow_shell;
            self.trust_mode = policy.trust_mode;
            self.approval_mode = policy.approval_mode;
            self.yolo = matches!(policy.approval_mode, ApprovalMode::Bypass);
        }
    }

    #[must_use]
    pub(crate) fn agent_trust_baseline(&self) -> bool {
        self.mode_prefs.agent_trust_mode
    }

    /// Update the durable Act shell choice without disturbing trust or
    /// approval. The live mirror changes only while Act owns the runtime.
    pub fn set_agent_shell_access(&mut self, allow_shell: bool) {
        self.set_agent_runtime_baseline(
            allow_shell,
            self.mode_prefs.agent_trust_mode,
            self.mode_prefs.agent_approval_mode,
        );
    }

    /// Update the durable Act approval choice without changing its saved shell
    /// or trust choices. Plan remains read-only.
    pub fn set_agent_approval_posture(&mut self, next: ApprovalMode) {
        self.set_agent_runtime_baseline(
            self.mode_prefs.agent_allow_shell,
            self.mode_prefs.agent_trust_mode,
            next,
        );
    }

    #[must_use]
    pub fn approval_policy_locked(&self) -> bool {
        self.approval_policy_locked
    }

    #[cfg(test)]
    #[must_use]
    pub fn approval_policy_requirements_managed(&self) -> bool {
        self.approval_policy_requirements_managed
    }

    /// Session transitions must never detach live runtime producers. Late
    /// engine, compaction, purge, or background-task events could otherwise
    /// contaminate the replacement session after clear/load/new.
    #[must_use]
    pub fn session_transition_blocked(&self) -> bool {
        self.is_loading
            || self.runtime_turn_status.as_deref() == Some("in_progress")
            || self.is_compacting
            || self.is_purging
            || self
                .task_panel
                .iter()
                .any(|task| matches!(task.status.as_str(), "queued" | "running"))
    }

    /// Whether the interface is asking the user to make a decision. Ambient
    /// motion yields across the whole frame while this is true; freezing one
    /// task marker still leaves distracting movement in peripheral vision.
    #[must_use]
    pub fn attention_hold_active(&self) -> bool {
        !self.view_stack.is_empty()
            || self.pending_user_input_prompt.is_some()
            || self.plan_prompt_pending
            || self
                .task_panel
                .iter()
                .any(|task| matches!(task.status.as_str(), "waiting" | "needs_user"))
    }

    pub fn mark_approval_policy_locked(&mut self) {
        self.approval_policy_locked = true;
        self.approval_policy_root_editable = true;
    }

    pub fn clear_saved_approval_policy_lock(&mut self) {
        if !self.approval_policy_requirements_managed {
            self.approval_policy_locked = false;
            self.approval_policy_root_editable = false;
        }
    }

    /// Execute hooks for a specific event with the given context
    pub fn execute_hooks(&self, event: HookEvent, context: &HookContext) -> Vec<HookResult> {
        self.hooks.execute(event, context)
    }

    /// Create a hook context with common fields pre-populated
    pub fn base_hook_context(&self) -> HookContext {
        HookContext::new()
            .with_mode(self.mode.label())
            .with_workspace(self.workspace.clone())
            .with_model(&self.model)
            .with_session_id(self.hooks.session_id())
            .with_tokens(self.session.total_tokens)
    }

    /// Soft cap on [`Self::history`] length. When history exceeds this count,
    /// the oldest cells are folded into a single placeholder to bound memory
    /// and render cost (#399 S2). The cap is generous — 5000 cells is more
    /// than enough to keep the visible transcript intact across sessions.
    pub const HISTORY_SOFT_CAP: usize = 5_000;

    /// Number of oldest cells to fold when the soft cap fires. Folding in
    /// batches amortizes the cost instead of triggering on every push.
    const HISTORY_FOLD_BATCH: usize = 1_000;

    pub fn add_message(&mut self, msg: HistoryCell) {
        let rev = self.fresh_history_revision();
        self.history.push(msg);
        self.history_revisions.push(rev);
        self.history_version = self.history_version.wrapping_add(1);

        // Bound history length: when the soft cap fires, fold the oldest
        // batch into a single ArchivedContext placeholder.
        self.maybe_fold_history();
        let selection_has_range = self
            .viewport
            .transcript_selection
            .ordered_endpoints()
            .is_some_and(|(start, end)| start != end);
        if self.viewport.transcript_scroll.is_at_tail()
            && !self.viewport.transcript_selection.dragging
            && !selection_has_range
            && !self.user_scrolled_during_stream
        {
            self.scroll_to_bottom();
        }
    }

    /// Add `delta` to the parent-turn session cost and bump the displayed
    /// high-water mark so the footer total never reverses (#244).
    #[allow(dead_code)]
    pub fn accrue_session_cost(&mut self, delta: f64) {
        self.accrue_session_cost_estimate(CostEstimate::usd_only(delta));
    }

    /// Add a dual-currency parent-turn cost estimate.
    pub fn accrue_session_cost_estimate(&mut self, estimate: CostEstimate) {
        self.session.session_cost += estimate.usd;
        self.session.session_cost_cny += estimate.cny;
        self.refresh_displayed_cost_high_water();
    }

    /// Add `delta` to the running sub-agent cost and bump the displayed
    /// high-water mark so the footer total never reverses (#244).
    #[allow(dead_code)]
    pub fn accrue_subagent_cost(&mut self, delta: f64) {
        self.accrue_subagent_cost_estimate(CostEstimate::usd_only(delta));
    }

    /// Add a dual-currency sub-agent/background cost estimate.
    pub fn accrue_subagent_cost_estimate(&mut self, estimate: CostEstimate) {
        self.session.subagent_cost += estimate.usd;
        self.session.subagent_cost_cny += estimate.cny;
        self.refresh_displayed_cost_high_water();
    }

    /// Copy current session/subagent cost accumulators into session metadata
    /// for persistence.
    pub fn sync_cost_to_metadata(&self, metadata: &mut crate::session_manager::SessionMetadata) {
        metadata.cost.session_cost_usd = self.session.session_cost;
        metadata.cost.session_cost_cny = self.session.session_cost_cny;
        metadata.cost.subagent_cost_usd = self.session.subagent_cost;
        metadata.cost.subagent_cost_cny = self.session.subagent_cost_cny;
        metadata.cost.displayed_cost_high_water_usd = self.session.displayed_cost_high_water;
        metadata.cost.displayed_cost_high_water_cny = self.session.displayed_cost_high_water_cny;
        // Persist cumulative turn duration so the footer "worked" chip
        // survives session save/restore (#2038).
        metadata.cumulative_turn_secs = self.cumulative_turn_duration.as_secs();
    }

    /// Recompute the displayed cost high-water mark. Called any time a cost
    /// counter is mutated; never decreases.
    pub fn refresh_displayed_cost_high_water(&mut self) {
        let current = self.session.session_cost + self.session.subagent_cost;
        if current > self.session.displayed_cost_high_water {
            self.session.displayed_cost_high_water = current;
        }
        let current_cny = self.session.session_cost_cny + self.session.subagent_cost_cny;
        if current_cny > self.session.displayed_cost_high_water_cny {
            self.session.displayed_cost_high_water_cny = current_cny;
        }
    }

    /// Read the visible session+sub-agent cost. Guaranteed monotonic across
    /// reconciliation events (cache adjustments, provisional → final swaps)
    /// for the lifetime of one session (#244).
    #[allow(dead_code)]
    pub fn displayed_session_cost(&self) -> f64 {
        self.displayed_session_cost_for_currency(CostCurrency::Usd)
    }

    /// Read the visible session+sub-agent cost in the chosen currency.
    pub fn displayed_session_cost_for_currency(&self, currency: CostCurrency) -> f64 {
        match self.cost_display_currency(currency) {
            CostCurrency::Usd => {
                let current = self.session.session_cost + self.session.subagent_cost;
                current.max(self.session.displayed_cost_high_water)
            }
            CostCurrency::Cny => {
                let current = self.session.session_cost_cny + self.session.subagent_cost_cny;
                current.max(self.session.displayed_cost_high_water_cny)
            }
        }
    }

    pub fn session_cost_for_currency(&self, currency: CostCurrency) -> f64 {
        match self.cost_display_currency(currency) {
            CostCurrency::Usd => self.session.session_cost,
            CostCurrency::Cny => self.session.session_cost_cny,
        }
    }

    pub fn subagent_cost_for_currency(&self, currency: CostCurrency) -> f64 {
        match self.cost_display_currency(currency) {
            CostCurrency::Usd => self.session.subagent_cost,
            CostCurrency::Cny => self.session.subagent_cost_cny,
        }
    }

    pub fn format_cost_amount(&self, amount: f64) -> String {
        crate::pricing::format_cost_amount(amount, self.cost_display_currency(self.cost_currency))
    }

    pub fn format_cost_amount_precise(&self, amount: f64) -> String {
        crate::pricing::format_cost_amount_precise(
            amount,
            self.cost_display_currency(self.cost_currency),
        )
    }

    pub(crate) fn cost_display_currency(&self, currency: CostCurrency) -> CostCurrency {
        if currency == CostCurrency::Cny
            && self.session.session_cost_cny == 0.0
            && self.session.subagent_cost_cny == 0.0
            && self.session.displayed_cost_high_water_cny == 0.0
            && (self.session.session_cost > 0.0
                || self.session.subagent_cost > 0.0
                || self.session.displayed_cost_high_water > 0.0)
        {
            CostCurrency::Usd
        } else {
            currency
        }
    }

    /// Estimated cost saved by the last turn's cache-hit tokens in the
    /// configured display currency.  Returns `None` when the model's pricing
    /// is unknown or there were no cache hits.
    pub fn last_turn_cache_savings(&self) -> Option<f64> {
        let hit_tokens = self.session.last_prompt_cache_hit_tokens?;
        let estimate = crate::pricing::calculate_cache_savings_for_provider(
            self.api_provider,
            &self.model,
            hit_tokens,
        )?;
        Some(match self.cost_currency {
            crate::pricing::CostCurrency::Usd => estimate.usd,
            crate::pricing::CostCurrency::Cny if estimate.cny == 0.0 && estimate.usd > 0.0 => {
                estimate.usd
            }
            crate::pricing::CostCurrency::Cny => estimate.cny,
        })
    }

    /// Fold the oldest [`Self::HISTORY_FOLD_BATCH`] cells into a single
    /// `ArchivedContext` placeholder when history exceeds the soft cap.
    /// Called from [`Self::add_message`]; the caller is responsible for
    /// also removing the folded range from any auxiliary per-cell maps.
    fn maybe_fold_history(&mut self) {
        if self.history.len() <= Self::HISTORY_SOFT_CAP {
            return;
        }

        let fold_count = Self::HISTORY_FOLD_BATCH.min(self.history.len());
        // Don't fold into the very last cell(s) — keep a buffer of
        // non-folded cells so the visible transcript tail stays intact.
        let keep_tail = Self::HISTORY_SOFT_CAP.saturating_sub(Self::HISTORY_FOLD_BATCH);
        if self.history.len().saturating_sub(fold_count) < keep_tail {
            return;
        }

        // Gather the range of cell indices we are folding.
        let folded: Vec<HistoryCell> = self.history.drain(..fold_count).collect();
        let folded_revs: Vec<u64> = self.history_revisions.drain(..fold_count).collect();
        let _ = folded_revs; // revisions are discarded with the cells

        // Shift all per-cell index maps down by `fold_count`.
        self.shift_history_maps_down(fold_count);

        // Build a single placeholder cell summarizing the folded range.
        let total_folded = folded.len();
        let summary = format!(
            "{total_folded} older transcript cells folded to bound memory. \
             Use /sessions to load a prior session snapshot if needed."
        );
        let placeholder = HistoryCell::ArchivedContext {
            level: 0,
            range: format!("cells 0-{}", total_folded.saturating_sub(1)),
            tokens: String::new(),
            density: String::new(),
            model: String::new(),
            timestamp: String::new(),
            summary,
        };

        // Insert the placeholder at the front.
        let rev = self.fresh_history_revision();
        self.history.insert(0, placeholder);
        self.history_revisions.insert(0, rev);
        self.history_version = self.history_version.wrapping_add(1);
        self.needs_redraw = true;
    }

    /// Shift all per-cell index maps down by `n` after removing the first
    /// `n` history cells. Every map key >= n is mapped to key - n; keys < n
    /// are dropped.
    fn shift_history_maps_down(&mut self, n: usize) {
        // tool_cells: HashMap<String, usize>
        self.tool_cells.retain(|_, idx| {
            if *idx >= n {
                *idx -= n;
                true
            } else {
                false
            }
        });

        // tool_details_by_cell: HashMap<usize, ToolDetailRecord>
        self.tool_details_by_cell = std::mem::take(&mut self.tool_details_by_cell)
            .into_iter()
            .filter_map(|(idx, detail)| {
                if idx >= n {
                    Some((idx - n, detail))
                } else {
                    None
                }
            })
            .collect();

        // context_references_by_cell
        self.context_references_by_cell = std::mem::take(&mut self.context_references_by_cell)
            .into_iter()
            .filter_map(|(idx, refs)| {
                if idx >= n {
                    Some((idx - n, refs))
                } else {
                    None
                }
            })
            .collect();
        self.rebuild_session_context_references();

        // subagent_card_index
        self.subagent_card_index.retain(|_, idx| {
            if *idx >= n {
                *idx -= n;
                true
            } else {
                false
            }
        });

        // last_fanout_card_index
        if let Some(ref mut idx) = self.last_fanout_card_index {
            if *idx >= n {
                *idx -= n;
            } else {
                self.last_fanout_card_index = None;
            }
        }

        // collapsed_cells
        self.collapsed_cells = std::mem::take(&mut self.collapsed_cells)
            .into_iter()
            .filter_map(|idx| if idx >= n { Some(idx - n) } else { None })
            .collect();
        self.expanded_tool_runs = std::mem::take(&mut self.expanded_tool_runs)
            .into_iter()
            .filter_map(|idx| if idx >= n { Some(idx - n) } else { None })
            .collect();
        self.collapsed_cell_map.clear();
    }

    /// #3030: return the stable user-facing label for an agent id
    /// ("Agent 3"), assigning the next sequential label on first sight.
    pub(crate) fn ensure_agent_label(&mut self, agent_id: &str) -> String {
        if let Some(label) = self.agent_label_map.get(agent_id) {
            return label.clone();
        }
        self.agent_counter = self.agent_counter.saturating_add(1);
        let label = format!("Agent {}", self.agent_counter);
        self.agent_label_map
            .insert(agent_id.to_string(), label.clone());
        label
    }

    /// #3030: read-only label lookup with raw-id fallback for agents the
    /// label map has never seen.
    pub(crate) fn agent_display_label(&self, agent_id: &str) -> String {
        self.agent_label_map
            .get(agent_id)
            .cloned()
            .unwrap_or_else(|| agent_id.to_string())
    }

    pub fn mark_history_updated(&mut self) {
        self.history_version = self.history_version.wrapping_add(1);
        // Resync per-cell revisions to history.len(). This is the
        // "I-don't-know-which-cell-changed" path: if cells were appended in
        // bulk (e.g. session resume, compaction), every new cell gets a
        // fresh revision; if cells were removed, drop trailing revs. We
        // intentionally do NOT bump revisions for indices that already had
        // one — the cache will reuse those. Callers that mutate a specific
        // cell's content must call `bump_history_cell(idx)` instead.
        self.resync_history_revisions();
        self.needs_redraw = true;
    }

    /// Issue a fresh, monotonically increasing revision counter for a new
    /// history cell. Wrapping is acceptable — collisions are astronomically
    /// rare and at worst trigger one extra re-render.
    fn fresh_history_revision(&mut self) -> u64 {
        let rev = self.next_history_revision;
        self.next_history_revision = self.next_history_revision.wrapping_add(1);
        rev
    }

    /// Bring `history_revisions` back into shape (`history_revisions.len() ==
    /// history.len()`). Pushes fresh revs for newly appended cells, truncates
    /// for cells that were removed. **Does not** invalidate existing entries.
    pub fn resync_history_revisions(&mut self) {
        if self.history_revisions.len() < self.history.len() {
            let needed = self.history.len() - self.history_revisions.len();
            for _ in 0..needed {
                let rev = self.fresh_history_revision();
                self.history_revisions.push(rev);
            }
        } else if self.history_revisions.len() > self.history.len() {
            self.history_revisions.truncate(self.history.len());
        }
    }

    /// Bump the revision counter of a single history cell so the transcript
    /// cache re-renders it on the next frame. Use this whenever a cell's
    /// content (e.g. a streaming Assistant body) is mutated in place.
    pub fn bump_history_cell(&mut self, idx: usize) {
        // Resync first in case callers mutated `history` directly without
        // pushing through `add_message`. After resync, the index is valid
        // (or out of bounds — in which case there's nothing to bump).
        self.resync_history_revisions();
        if let Some(rev) = self.history_revisions.get_mut(idx) {
            let new_rev = self.next_history_revision;
            self.next_history_revision = self.next_history_revision.wrapping_add(1);
            *rev = new_rev;
        }
        self.history_version = self.history_version.wrapping_add(1);
        self.needs_redraw = true;
    }

    /// Append a single history cell, allocating a fresh per-cell revision.
    /// Equivalent to `add_message` but exposed as a generic alias so call
    /// sites currently doing `app.history.push(...)` followed by
    /// `app.mark_history_updated()` can collapse to one helper.
    pub fn push_history_cell(&mut self, cell: HistoryCell) {
        let rev = self.fresh_history_revision();
        self.history.push(cell);
        self.history_revisions.push(rev);
        self.history_version = self.history_version.wrapping_add(1);
        self.maybe_fold_history();
        self.needs_redraw = true;
    }

    /// Append a batch of history cells, allocating fresh revisions.
    pub fn extend_history<I>(&mut self, cells: I)
    where
        I: IntoIterator<Item = HistoryCell>,
    {
        for cell in cells {
            let rev = self.fresh_history_revision();
            self.history.push(cell);
            self.history_revisions.push(rev);
        }
        self.maybe_fold_history();
        self.history_version = self.history_version.wrapping_add(1);
        self.needs_redraw = true;
    }

    /// Clear the history and its session-scoped side indexes. Used by /clear,
    /// session reset, and other "wipe and reload" flows.
    pub fn clear_history(&mut self) {
        self.history.clear();
        self.history_revisions.clear();
        self.context_references_by_cell.clear();
        self.session_context_references.clear();
        self.session_artifacts.clear();
        self.collapsed_cells.clear();
        self.expanded_tool_runs.clear();
        self.collapsed_cell_map.clear();
        self.history_version = self.history_version.wrapping_add(1);
        self.needs_redraw = true;
    }

    /// Pop the trailing history cell, keeping revisions in sync.
    pub fn pop_history(&mut self) -> Option<HistoryCell> {
        let cell = self.history.pop();
        if cell.is_some() {
            self.history_revisions.pop();
            self.context_references_by_cell.remove(&self.history.len());
            self.rebuild_session_context_references();
            self.expanded_tool_runs
                .retain(|idx| *idx < self.history.len());
            self.history_version = self.history_version.wrapping_add(1);
            self.needs_redraw = true;
        }
        cell
    }

    /// Truncate `history` (and the parallel `history_revisions` + auxiliary
    /// per-cell maps) so that only cells with index `< new_len` remain.
    /// Used by Esc-Esc backtrack (#133) to roll the visible transcript
    /// back to a chosen user message. Cells dropped here are gone — the
    /// caller is expected to also trim the matching `api_messages` so the
    /// next turn matches what the user sees.
    pub fn truncate_history_to(&mut self, new_len: usize) {
        if new_len >= self.history.len() {
            return;
        }
        self.history.truncate(new_len);
        if self.history_revisions.len() > new_len {
            self.history_revisions.truncate(new_len);
        }
        // Drop any auxiliary maps keyed on history indices that now point
        // past the new tail. We keep the rest intact so unaffected tool
        // cells continue to render correctly.
        self.tool_cells.retain(|_, idx| *idx < new_len);
        self.tool_details_by_cell.retain(|idx, _| *idx < new_len);
        self.context_references_by_cell
            .retain(|idx, _| *idx < new_len);
        self.rebuild_session_context_references();
        self.subagent_card_index.retain(|_, idx| *idx < new_len);
        if self
            .last_fanout_card_index
            .is_some_and(|idx| idx >= new_len)
        {
            self.last_fanout_card_index = None;
        }
        // Drop collapsed cells that reference indices past the new tail.
        self.collapsed_cells.retain(|idx| *idx < new_len);
        self.expanded_tool_runs.retain(|idx| *idx < new_len);
        self.collapsed_cell_map.clear();
        self.history_version = self.history_version.wrapping_add(1);
        self.needs_redraw = true;
    }

    #[must_use]
    pub fn tool_collapse_active(&self) -> bool {
        self.tool_collapse_threshold > 0 && self.tool_collapse_mode.is_active(self.calm_mode)
    }

    #[must_use]
    pub fn tool_run_start_for_history_index(&self, index: usize) -> Option<usize> {
        if !self.tool_collapse_active() {
            return None;
        }
        let active_entries = self
            .active_cell
            .as_ref()
            .map_or(&[][..], crate::tui::active_cell::ActiveCell::entries);
        if index >= self.history.len().saturating_add(active_entries.len()) {
            return None;
        }
        crate::tui::history::detect_tool_runs_from_slices(
            &self.history,
            active_entries,
            self.tool_collapse_threshold,
        )
        .into_iter()
        .find(|run| index >= run.start && index < run.start.saturating_add(run.count))
        .map(|run| run.start)
    }

    pub fn toggle_tool_run_expansion_at(&mut self, index: usize) -> bool {
        let Some(start) = self.tool_run_start_for_history_index(index) else {
            return false;
        };
        if self.expanded_tool_runs.remove(&start) {
            self.status_message = Some("Tool group collapsed".to_string());
        } else {
            self.expanded_tool_runs.insert(start);
            self.status_message = Some("Tool group expanded".to_string());
        }
        self.mark_history_updated();
        true
    }

    /// Bump the active-cell revision counter and request a redraw.
    ///
    /// Use this whenever an entry inside `active_cell` is mutated. The
    /// transcript cache combines this counter with `history_version` to
    /// produce a per-cell revision so the synthetic active-cell row can be
    /// re-rendered without invalidating committed history cells.
    pub fn bump_active_cell_revision(&mut self) {
        self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
        if let Some(active) = self.active_cell.as_mut() {
            active.bump_revision();
        }
        self.history_version = self.history_version.wrapping_add(1);
        self.needs_redraw = true;
    }

    /// Total number of cells in the *virtual* transcript: `history.len()`
    /// plus active cell entries (if any).
    #[must_use]
    #[allow(dead_code)] // Reserved for renderers that need a unified cell count.
    pub fn virtual_cell_count(&self) -> usize {
        self.history.len() + self.active_cell.as_ref().map_or(0, ActiveCell::entry_count)
    }

    /// The next cell index a freshly-pushed entry would occupy in the virtual
    /// transcript. Used by `register_tool_cell`-style callsites that record
    /// cell-index metadata before the active cell flushes to history.
    #[must_use]
    #[allow(dead_code)] // Reserved for the eventual merged push helper.
    pub fn next_virtual_cell_index(&self) -> usize {
        self.virtual_cell_count()
    }

    #[must_use]
    pub fn original_cell_index_for_rendered(&self, rendered_index: usize) -> usize {
        self.collapsed_cell_map
            .get(rendered_index)
            .copied()
            .unwrap_or(rendered_index)
    }

    /// Resolve a virtual cell index to either a committed history cell or an
    /// active-cell entry. Used by the pager / details lookup code so it can
    /// transparently address still-in-flight cells.
    #[must_use]
    #[allow(dead_code)] // Used by the upcoming pager rewrite (read-only resolver).
    pub fn cell_at_virtual_index(&self, index: usize) -> Option<&HistoryCell> {
        if index < self.history.len() {
            self.history.get(index)
        } else {
            let entry_idx = index - self.history.len();
            self.active_cell
                .as_ref()
                .and_then(|active| active.entries().get(entry_idx))
        }
    }

    /// Resolve the tool-detail record for a committed or still-active virtual
    /// transcript cell.
    #[must_use]
    pub fn tool_detail_record_for_cell(&self, index: usize) -> Option<&ToolDetailRecord> {
        if let Some(detail) = self.tool_details_by_cell.get(&index) {
            return Some(detail);
        }
        self.active_tool_details
            .values()
            .find(|detail| self.tool_cells.get(&detail.tool_id).copied() == Some(index))
    }

    /// Whether a virtual transcript cell can open a meaningful `v` detail
    /// view. Thinking cells render their own raw text inline so there is no
    /// separate "raw" target — only tool / sub-agent cells get the hint.
    #[must_use]
    pub fn cell_has_detail_target(&self, index: usize) -> bool {
        self.tool_detail_record_for_cell(index).is_some()
            || matches!(
                self.cell_at_virtual_index(index),
                Some(HistoryCell::Tool(_) | HistoryCell::SubAgent(_))
            )
    }

    /// Pick the detail target for the current viewport. This is used by the
    /// transcript highlight and footer hint so they agree with `v`.
    #[must_use]
    pub fn detail_cell_index_for_viewport(
        &self,
        top: usize,
        visible: usize,
        line_meta: &[TranscriptLineMeta],
    ) -> Option<usize> {
        let selected_cell = self
            .viewport
            .transcript_selection
            .ordered_endpoints()
            .and_then(|(start, _)| line_meta.get(start.line_index))
            .and_then(TranscriptLineMeta::cell_line)
            .map(|(cell_index, _)| self.original_cell_index_for_rendered(cell_index))
            .filter(|&idx| self.cell_has_detail_target(idx));
        if selected_cell.is_some() {
            return selected_cell;
        }

        let start = top.min(line_meta.len().saturating_sub(1));
        let end = start.saturating_add(visible).min(line_meta.len());
        for meta in line_meta.iter().take(end).skip(start) {
            let Some((cell_index, _)) = meta.cell_line() else {
                continue;
            };
            let cell_index = self.original_cell_index_for_rendered(cell_index);
            if self.cell_has_detail_target(cell_index) {
                return Some(cell_index);
            }
        }

        (0..self.virtual_cell_count())
            .rev()
            .find(|&idx| self.cell_has_detail_target(idx))
    }

    pub fn record_context_references(
        &mut self,
        history_cell: usize,
        message_index: usize,
        references: Vec<ContextReference>,
    ) {
        if references.is_empty() {
            return;
        }
        let records: Vec<SessionContextReference> = references
            .into_iter()
            .map(|reference| SessionContextReference {
                message_index,
                reference,
            })
            .collect();
        self.context_references_by_cell
            .insert(history_cell, records.clone());
        self.rebuild_session_context_references();
        self.needs_redraw = true;
    }

    pub fn sync_context_references_from_session(
        &mut self,
        references: &[SessionContextReference],
        message_to_cell: &HashMap<usize, usize>,
    ) {
        self.context_references_by_cell.clear();
        for record in references {
            let Some(&cell_index) = message_to_cell.get(&record.message_index) else {
                continue;
            };
            self.context_references_by_cell
                .entry(cell_index)
                .or_default()
                .push(record.clone());
        }
        self.rebuild_session_context_references();
    }

    fn rebuild_session_context_references(&mut self) {
        let mut records: Vec<SessionContextReference> = self
            .context_references_by_cell
            .values()
            .flat_map(|records| records.iter().cloned())
            .collect();
        records.sort_by_key(|record| record.message_index);
        self.session_context_references = records;
    }

    /// Mutable variant of [`Self::cell_at_virtual_index`]. Bumps the
    /// appropriate revision counter (active-cell revision when targeting an
    /// in-flight entry, history version otherwise).
    pub fn cell_at_virtual_index_mut(&mut self, index: usize) -> Option<&mut HistoryCell> {
        if index < self.history.len() {
            // Bump only the targeted cell's revision; leave every other
            // cell's cached render intact.
            self.resync_history_revisions();
            if let Some(rev) = self.history_revisions.get_mut(index) {
                let new_rev = self.next_history_revision;
                self.next_history_revision = self.next_history_revision.wrapping_add(1);
                *rev = new_rev;
            }
            self.history_version = self.history_version.wrapping_add(1);
            self.history.get_mut(index)
        } else {
            let entry_idx = index - self.history.len();
            self.active_cell_revision = self.active_cell_revision.wrapping_add(1);
            self.history_version = self.history_version.wrapping_add(1);
            self.active_cell
                .as_mut()
                .and_then(|active| active.entry_mut(entry_idx))
        }
    }

    /// Drain the active cell into history. Companion maps that reference
    /// active-cell entries by virtual index (`tool_cells`,
    /// `tool_details_by_cell`) are rewritten to point at the new history
    /// indices. Idempotent — calling this when there is no active cell is a
    /// no-op.
    ///
    /// Caller is responsible for first marking in-progress entries with the
    /// terminal status they want (e.g. via
    /// [`ActiveCell::mark_in_progress_as_interrupted`]).
    pub fn flush_active_cell(&mut self) {
        let Some(mut active) = self.active_cell.take() else {
            self.streaming_thinking_active_entry = None;
            return;
        };
        if active.is_empty() {
            self.exploring_cell = None;
            self.exploring_entries.clear();
            self.active_tool_details.clear();
            self.active_tool_entry_completed_at.clear();
            self.streaming_thinking_active_entry = None;
            self.bump_active_cell_revision();
            return;
        }

        if let Some(entry_idx) = self.streaming_thinking_active_entry.take()
            && let Some(HistoryCell::Thinking { streaming, .. }) = active.entry_mut(entry_idx)
        {
            *streaming = false;
        }

        let drained = active.drain();
        let base_index = self.history.len();

        let mut details = std::mem::take(&mut self.active_tool_details);
        self.active_tool_entry_completed_at.clear();
        for (tool_id, detail) in details.drain() {
            self.tool_details_by_cell
                .entry(self.tool_cells.get(&tool_id).copied().unwrap_or(base_index))
                .or_insert(detail);
        }

        self.exploring_cell = None;
        self.exploring_entries.clear();

        for cell in drained {
            let rev = self.fresh_history_revision();
            self.history.push(cell);
            self.history_revisions.push(rev);
        }
        self.history_version = self.history_version.wrapping_add(1);
        self.needs_redraw = true;
        let selection_has_range = self
            .viewport
            .transcript_selection
            .ordered_endpoints()
            .is_some_and(|(start, end)| start != end);
        if self.viewport.transcript_scroll.is_at_tail()
            && !self.viewport.transcript_selection.dragging
            && !selection_has_range
            && !self.user_scrolled_during_stream
        {
            self.scroll_to_bottom();
        }
    }

    /// Mark every still-running entry in the active cell as interrupted, then
    /// flush. Convenience helper for cancellation paths.
    pub fn finalize_active_cell_as_interrupted(&mut self) {
        if let Some(active) = self.active_cell.as_mut() {
            active.mark_in_progress_as_interrupted();
        }
        self.flush_active_cell();
        // #4121: interrupt finalizes running workflow children as cancelled
        // and preserves the completed panel until the next run starts.
        if let Some(panel) = self.workflow_panel.as_mut() {
            panel.finalize_interrupt();
            self.needs_redraw = true;
        }
    }

    /// Apply a workflow panel event, creating the panel on first `RunStarted`.
    ///
    /// Returns whether this event should request an immediate repaint.
    /// Budget-only updates always mutate panel state but leave repaint to the
    /// caller so high-frequency fan-out budget ticks can be paced (#4095).
    pub fn apply_workflow_panel_event(
        &mut self,
        event: crate::tui::widgets::workflow_panel::WorkflowPanelEvent,
    ) -> bool {
        use crate::tui::widgets::workflow_panel::{WorkflowPanel, WorkflowPanelEvent};
        let budget_only = matches!(event, WorkflowPanelEvent::BudgetUpdated { .. });
        match (&mut self.workflow_panel, &event) {
            (
                None,
                WorkflowPanelEvent::RunStarted {
                    run_id,
                    workflow_goal,
                    workflow_id,
                    token_budget,
                    at_ms,
                    ..
                },
            ) => {
                let label = workflow_goal
                    .clone()
                    .or_else(|| workflow_id.clone())
                    .unwrap_or_else(|| "workflow".to_string());
                let mut panel = WorkflowPanel::new(run_id.clone(), label, *at_ms);
                panel.locale = self.ui_locale;
                panel.budget_total = *token_budget;
                panel.budget_remaining = *token_budget;
                self.workflow_panel = Some(panel);
            }
            (None, _) => {
                // No panel yet and event is not a start — seed a shell panel
                // so late events still surface rather than being dropped.
                let mut panel = WorkflowPanel::new("workflow", "workflow", 0);
                panel.locale = self.ui_locale;
                panel.apply_event(event);
                self.workflow_panel = Some(panel);
            }
            (Some(panel), _) => {
                panel.apply_event(event);
            }
        }
        if !budget_only {
            self.needs_redraw = true;
        }
        !budget_only
    }

    /// Toggle the workflow panel expand/collapse state. Returns true when a
    /// panel was present and toggled.
    pub fn toggle_workflow_panel(&mut self) -> bool {
        let Some(panel) = self.workflow_panel.as_mut() else {
            return false;
        };
        let _ = panel.toggle_expanded();
        self.needs_redraw = true;
        true
    }

    pub fn push_status_toast(
        &mut self,
        text: impl Into<String>,
        level: StatusToastLevel,
        ttl_ms: Option<u64>,
    ) {
        let toast = StatusToast::new(text, level, ttl_ms);
        self.status_toasts.push_back(toast);
        while self.status_toasts.len() > 24 {
            self.status_toasts.pop_front();
        }
        self.needs_redraw = true;
    }

    /// How long the "press Ctrl+C again to quit" prompt stays armed before it
    /// silently expires.
    pub const QUIT_CONFIRMATION_WINDOW: Duration = Duration::from_secs(2);

    /// Arm the quit confirmation timer. The next Ctrl+C within
    /// [`Self::QUIT_CONFIRMATION_WINDOW`] should exit the app cleanly. Call this only
    /// from idle state — while a turn is in flight or a modal is open Ctrl+C
    /// retains its existing "interrupt this turn" / "close modal" semantics.
    pub fn arm_quit(&mut self) {
        self.quit_armed_until = Some(Instant::now() + Self::QUIT_CONFIRMATION_WINDOW);
        self.needs_redraw = true;
    }

    /// Whether the quit timer is currently armed (i.e. a prior Ctrl+C set it
    /// and it hasn't expired yet).
    pub fn quit_is_armed(&self) -> bool {
        self.quit_armed_until
            .map(|deadline| Instant::now() < deadline)
            .unwrap_or(false)
    }

    /// Clear the quit-armed timer. Call when expiry is detected on a tick or
    /// when the user takes any other action that should disarm the prompt
    /// (typing, sending a message, etc.).
    pub fn disarm_quit(&mut self) {
        if self.quit_armed_until.is_some() {
            self.quit_armed_until = None;
            self.needs_redraw = true;
        }
    }

    /// Tick called from the redraw loop. Lets time-based UI state (the
    /// quit-armed prompt) expire even when no input event is delivered.
    pub fn tick_quit_armed(&mut self) {
        if let Some(deadline) = self.quit_armed_until
            && Instant::now() >= deadline
        {
            self.quit_armed_until = None;
            self.needs_redraw = true;
        }
    }

    pub const RECEIPT_VISIBLE_DURATION: Duration = Duration::from_secs(8);

    pub fn set_receipt_text(&mut self, text: impl Into<String>) {
        self.receipt_text = Some(text.into());
        self.receipt_started_at = Some(Instant::now());
        self.needs_redraw = true;
    }

    pub fn clear_receipt(&mut self) {
        if self.receipt_text.is_some() || self.receipt_started_at.is_some() {
            self.receipt_text = None;
            self.receipt_started_at = None;
            self.needs_redraw = true;
        }
    }

    pub fn active_receipt_text(&self) -> Option<&str> {
        let receipt = self.receipt_text.as_deref()?;
        let started = self.receipt_started_at?;
        (started.elapsed() <= Self::RECEIPT_VISIBLE_DURATION).then_some(receipt)
    }

    /// Tick called from the redraw loop so transient receipts leave the UI
    /// without waiting for the next keypress.
    pub fn tick_receipt(&mut self) {
        if self
            .receipt_started_at
            .is_some_and(|started| started.elapsed() > Self::RECEIPT_VISIBLE_DURATION)
        {
            self.clear_receipt();
        }
    }

    pub fn set_sticky_status(
        &mut self,
        text: impl Into<String>,
        level: StatusToastLevel,
        ttl_ms: Option<u64>,
    ) {
        self.sticky_status = Some(StatusToast::new(text, level, ttl_ms));
        self.needs_redraw = true;
    }

    pub fn clear_sticky_status(&mut self) {
        self.sticky_status = None;
    }

    pub fn set_sidebar_focus(&mut self, focus: SidebarFocus) {
        if self.sidebar_focus != focus {
            self.sidebar_focus = focus;
            self.sidebar_focus_dirty = true;
        }
        self.needs_redraw = true;
    }

    pub fn close_slash_menu(&mut self) {
        self.slash_menu_hidden = true;
        self.needs_redraw = true;
    }

    fn classify_status_text(text: &str) -> (StatusToastLevel, Option<u64>, bool) {
        let lower = text.to_ascii_lowercase();
        let has = |needle: &str| lower.contains(needle);

        if has("offline mode") || has("context critical") {
            return (StatusToastLevel::Warning, None, true);
        }
        if has("error")
            || has("failed")
            || has("denied")
            || has("timeout")
            || has("aborted")
            || has("critical")
        {
            return (StatusToastLevel::Error, Some(15_000), true);
        }
        // A success keyword under a negation ("not saved", "no longer
        // found", "could not enable") is a failure the coarse keyword match
        // would otherwise paint green. Guard it: negated success degrades to
        // a neutral Info toast rather than a misleading Success.
        let negated = has("not ")
            || has("no longer")
            || has("no ")
            || has("could not")
            || has("couldn't")
            || has("cannot")
            || has("can't")
            || has("unable");
        if !negated
            && (has("saved")
                || has("loaded")
                || has("queued")
                || has("found")
                || has("enabled")
                || has("completed"))
        {
            return (StatusToastLevel::Success, Some(5_000), false);
        }
        if has("cancelled") || has("canceled") || has("warning") {
            return (StatusToastLevel::Warning, Some(5_000), false);
        }
        (StatusToastLevel::Info, Some(4_000), false)
    }

    fn is_mode_switch_status_message(message: &str) -> bool {
        message.starts_with("Switched to ") && message.ends_with(" mode")
    }

    pub fn sync_status_message_to_toasts(&mut self) {
        let current = self.status_message.clone();
        if self.last_status_message_seen == current {
            return;
        }
        self.last_status_message_seen = current.clone();

        let Some(message) = current else {
            return;
        };
        if message.trim().is_empty() {
            return;
        }
        if Self::is_mode_switch_status_message(&message) {
            return;
        }

        let (level, ttl_ms, sticky) = Self::classify_status_text(&message);
        if sticky {
            self.set_sticky_status(message, level, ttl_ms);
        } else {
            if matches!(level, StatusToastLevel::Success)
                && self
                    .sticky_status
                    .as_ref()
                    .is_some_and(|toast| matches!(toast.level, StatusToastLevel::Error))
            {
                self.clear_sticky_status();
            }
            self.push_status_toast(message, level, ttl_ms);
        }
    }

    fn prune_expired_status_toasts(&mut self, now: Instant) {
        let queued_before = self.status_toasts.len();
        self.status_toasts.retain(|toast| !toast.is_expired(now));
        let queued_removed = self.status_toasts.len() != queued_before;
        let sticky_removed = self
            .sticky_status
            .as_ref()
            .is_some_and(|toast| toast.is_expired(now));
        if sticky_removed {
            self.sticky_status = None;
        }
        if queued_removed || sticky_removed {
            self.needs_redraw = true;
        }
    }

    /// Up to `limit` currently-active toasts, most recent last (so a stacked
    /// renderer iterating top-to-bottom shows the freshest message at the
    /// bottom, like a chat log). Prunes expired toasts throughout the queue as
    /// a side effect — the same cleanup as `active_status_toast` so callers see
    /// a consistent queue. Whalescale#439.
    pub fn active_status_toasts(&mut self, limit: usize) -> Vec<StatusToast> {
        self.sync_status_message_to_toasts();
        let now = Instant::now();
        self.prune_expired_status_toasts(now);

        let mut out: Vec<StatusToast> = Vec::with_capacity(limit);
        if let Some(sticky) = self.sticky_status.clone() {
            out.push(sticky);
        }
        let take = limit.saturating_sub(out.len());
        let queued: Vec<StatusToast> = self
            .status_toasts
            .iter()
            .rev()
            .take(take)
            .cloned()
            .collect();
        // Iterate in queue order (oldest of the visible window first) so the
        // stacked renderer feels chronological — most recent at the bottom.
        for toast in queued.into_iter().rev() {
            out.push(toast);
        }
        out
    }

    pub fn active_status_toast(&mut self) -> Option<StatusToast> {
        self.sync_status_message_to_toasts();
        let now = Instant::now();
        self.prune_expired_status_toasts(now);

        self.sticky_status
            .clone()
            .or_else(|| self.status_toasts.back().cloned())
    }

    pub fn transcript_render_options(&self) -> TranscriptRenderOptions {
        TranscriptRenderOptions {
            show_thinking: self.show_thinking,
            verbose: self.verbose_transcript,
            show_tool_details: self.show_tool_details,
            calm_mode: self.calm_mode,
            low_motion: self.low_motion,
            spacing: self.transcript_spacing,
        }
    }

    /// Handle terminal resize event.
    pub fn handle_resize(&mut self, _width: u16, _height: u16) {
        let preserved_scroll = (!self.viewport.transcript_scroll.is_at_tail())
            .then_some(self.viewport.last_transcript_top);
        self.viewport.transcript_cache = TranscriptViewCache::new();

        if let Some(top) = preserved_scroll {
            self.viewport.transcript_scroll = TranscriptScroll::at_line(top);
        }

        self.viewport.pending_scroll_delta = 0;
        self.viewport.transcript_selection.clear();

        self.viewport.last_transcript_area = None;
        self.viewport.last_approval_area = None;
        self.viewport.last_transcript_top = 0;
        // Seed visible height from the resize event so paging keys use a
        // useful page size immediately, before the next render updates it.
        self.viewport.last_transcript_visible = (_height as usize).saturating_sub(2).max(1);
        self.viewport.last_transcript_total = 0;
        self.viewport.last_transcript_padding_top = 0;
        self.viewport.jump_to_latest_button_area = None;

        self.mark_history_updated();
    }

    pub fn cursor_byte_index(&self) -> usize {
        byte_index_at_char(&self.input, self.cursor_position)
    }

    /// When the user starts editing a truncated oversized paste, restore the
    /// full text so they can see and edit the complete content (#3263).
    fn auto_expand_oversized_paste(&mut self) {
        if let Some(full) = self.oversized_paste_full_text.take() {
            self.input = full;
            // Clamp cursor to the new length instead of resetting to 0,
            // so the user's position in the truncated preview is preserved.
            self.cursor_position = self.cursor_position.min(char_count(&self.input));
        }
    }

    pub fn insert_str(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.auto_expand_oversized_paste();
        self.delete_selection();
        self.selected_attachment_index = None;
        let cursor = self.cursor_position.min(char_count(&self.input));
        let byte_index = byte_index_at_char(&self.input, cursor);
        self.input.insert_str(byte_index, text);
        self.cursor_position = cursor + char_count(text);
        self.strip_raw_mouse_reports_from_input();
        self.slash_menu_hidden = false;
        self.mention_menu_hidden = false;
        self.mention_menu_selected = 0;
        self.needs_redraw = true;
    }

    pub fn insert_paste_text(&mut self, text: &str) {
        if let Some(pending) = self.paste_burst.flush_before_modified_input() {
            self.insert_str(&pending);
        }
        let normalized = normalize_paste_text(text);
        if !normalized.is_empty() {
            self.insert_str(&normalized);
        }
        self.paste_burst.clear_after_explicit_paste();
        // Large pasted input stays editable and visible until submit. The
        // submit-time safety net consolidates oversized composer content into
        // an @paste-...md mention before dispatch, so no path silently
        // truncates user input.
        // self.consolidate_large_input_if_oversized(); // deferred to submit time
    }

    pub fn insert_media_attachment(&mut self, kind: &str, path: &Path, description: Option<&str>) {
        let reference = media_attachment_reference(kind, path, description);
        let cursor = self.cursor_position.min(char_count(&self.input));
        let byte_index = byte_index_at_char(&self.input, cursor);
        let needs_prefix_newline = self.input[..byte_index]
            .chars()
            .last()
            .is_some_and(|ch| !ch.is_whitespace());
        let needs_suffix_newline = self.input[byte_index..]
            .chars()
            .next()
            .is_some_and(|ch| !ch.is_whitespace());

        let mut inserted = String::new();
        if needs_prefix_newline {
            inserted.push('\n');
        }
        inserted.push_str(&reference);
        if needs_suffix_newline || self.input[byte_index..].is_empty() {
            inserted.push('\n');
        }
        self.insert_str(&inserted);
        self.paste_burst.clear_after_explicit_paste();
    }

    pub fn composer_attachment_count(&self) -> usize {
        crate::tui::file_mention::media_attachment_references(&self.input).len()
    }

    pub fn selected_composer_attachment_index(&self) -> Option<usize> {
        let count = self.composer_attachment_count();
        self.selected_attachment_index
            .filter(|index| *index < count)
    }

    pub fn select_previous_composer_attachment(&mut self) -> bool {
        let count = self.composer_attachment_count();
        if count == 0 {
            self.selected_attachment_index = None;
            return false;
        }

        let next = self
            .selected_composer_attachment_index()
            .map_or(count.saturating_sub(1), |index| index.saturating_sub(1));
        self.selected_attachment_index = Some(next);
        self.cursor_position = 0;
        self.status_message = Some("Attachment selected - Backspace/Delete removes it".to_string());
        self.needs_redraw = true;
        true
    }

    pub fn select_next_composer_attachment(&mut self) -> bool {
        let count = self.composer_attachment_count();
        let Some(index) = self.selected_composer_attachment_index() else {
            return false;
        };
        if index + 1 < count {
            self.selected_attachment_index = Some(index + 1);
            self.status_message =
                Some("Attachment selected - Backspace/Delete removes it".to_string());
        } else {
            self.selected_attachment_index = None;
            self.status_message = Some("Composer focused".to_string());
        }
        self.needs_redraw = true;
        true
    }

    pub fn clear_composer_attachment_selection(&mut self) -> bool {
        if self.selected_attachment_index.take().is_some() {
            self.status_message = Some("Composer focused".to_string());
            self.needs_redraw = true;
            true
        } else {
            false
        }
    }

    pub fn remove_selected_composer_attachment(&mut self) -> bool {
        let references = crate::tui::file_mention::media_attachment_references(&self.input);
        let Some(index) = self
            .selected_composer_attachment_index()
            .filter(|index| *index < references.len())
        else {
            self.selected_attachment_index = None;
            return false;
        };
        let reference = references[index].clone();
        let cursor_byte = byte_index_at_char(&self.input, self.cursor_position);
        let new_cursor_byte = if cursor_byte <= reference.start_byte {
            cursor_byte
        } else if cursor_byte >= reference.end_byte {
            cursor_byte.saturating_sub(reference.end_byte - reference.start_byte)
        } else {
            reference.start_byte
        };

        self.input
            .replace_range(reference.start_byte..reference.end_byte, "");
        self.cursor_position = self.input[..new_cursor_byte.min(self.input.len())]
            .chars()
            .count();
        let remaining = self.composer_attachment_count();
        self.selected_attachment_index = if remaining == 0 {
            None
        } else {
            Some(index.min(remaining.saturating_sub(1)))
        };
        self.slash_menu_hidden = false;
        self.mention_menu_hidden = false;
        self.mention_menu_selected = 0;
        self.status_message = Some(format!("Removed attachment: {}", reference.path));
        self.needs_redraw = true;
        true
    }

    pub fn flush_paste_burst_if_due(&mut self, now: Instant) -> bool {
        match self.paste_burst.flush_if_due(now) {
            FlushResult::Paste(text) => {
                self.insert_str(&text);
                true
            }
            FlushResult::Typed(ch) => {
                self.insert_char(ch);
                true
            }
            FlushResult::None => false,
        }
    }

    pub fn flush_paste_burst_if_enabled(&mut self, now: Instant) -> bool {
        self.use_paste_burst_detection && self.flush_paste_burst_if_due(now)
    }

    pub fn paste_burst_next_flush_delay_if_enabled(&self, now: Instant) -> Option<Duration> {
        if self.use_paste_burst_detection {
            self.paste_burst.next_flush_delay(now)
        } else {
            None
        }
    }

    pub fn flush_paste_burst_before_modified_input_if_enabled(&mut self) -> Option<String> {
        if self.use_paste_burst_detection {
            self.paste_burst.flush_before_modified_input()
        } else {
            None
        }
    }

    pub fn insert_api_key_char(&mut self, c: char) {
        let cursor = self.api_key_cursor.min(char_count(&self.api_key_input));
        let byte_index = byte_index_at_char(&self.api_key_input, cursor);
        self.api_key_input.insert(byte_index, c);
        self.api_key_cursor = cursor + 1;
    }

    pub fn insert_api_key_str(&mut self, text: &str) {
        let sanitized = sanitize_api_key_text(text);
        if sanitized.is_empty() {
            return;
        }
        let cursor = self.api_key_cursor.min(char_count(&self.api_key_input));
        let byte_index = byte_index_at_char(&self.api_key_input, cursor);
        self.api_key_input.insert_str(byte_index, &sanitized);
        self.api_key_cursor = cursor + char_count(&sanitized);
    }

    pub fn delete_api_key_char(&mut self) {
        if self.api_key_cursor == 0 {
            return;
        }
        let target = self.api_key_cursor.saturating_sub(1);
        if remove_char_at(&mut self.api_key_input, target) {
            self.api_key_cursor = target;
        }
    }

    /// Paste from clipboard into input.
    ///
    /// Returns whether content was inserted. In SSH sessions without a
    /// forwarded graphical display, the terminal client owns paste, so direct
    /// clipboard shortcuts surface the local terminal-paste instruction while
    /// `Event::Paste` remains the data path.
    pub fn paste_from_clipboard(&mut self) -> bool {
        if self.clipboard.requires_terminal_paste() {
            self.status_message = Some(self.tr(MessageId::ClipboardSshPasteHint).into_owned());
            return false;
        }
        if let Some(content) = self.clipboard.read(self.workspace.as_path()) {
            self.apply_clipboard_content(content);
            return true;
        }
        false
    }

    pub fn apply_clipboard_content(&mut self, content: ClipboardContent) {
        match content {
            ClipboardContent::Text(text) => {
                self.insert_paste_text(&text);
            }
            ClipboardContent::Image(pasted) => {
                let description = format!("{} ({})", pasted.short_label(), pasted.size_label());
                self.insert_media_attachment("image", &pasted.path, Some(&description));
                self.status_message = Some(format!("Attached image: {description}"));
            }
        }
    }

    pub fn paste_api_key_from_clipboard(&mut self) -> bool {
        if self.clipboard.requires_terminal_paste() {
            self.status_message = Some(self.tr(MessageId::ClipboardSshPasteHint).into_owned());
            return false;
        }
        if let Some(ClipboardContent::Text(text)) = self.clipboard.read(self.workspace.as_path()) {
            self.insert_api_key_str(&text);
            return true;
        }
        false
    }

    pub fn scroll_up(&mut self, amount: usize) {
        let delta = i32::try_from(amount).unwrap_or(i32::MAX);
        self.viewport.pending_scroll_delta =
            self.viewport.pending_scroll_delta.saturating_sub(delta);
        self.user_scrolled_during_stream = true;
        self.needs_redraw = true;
    }

    pub fn scroll_down(&mut self, amount: usize) {
        let delta = i32::try_from(amount).unwrap_or(i32::MAX);
        self.viewport.pending_scroll_delta =
            self.viewport.pending_scroll_delta.saturating_add(delta);
        self.user_scrolled_during_stream = true;
        self.needs_redraw = true;
    }

    pub fn scroll_to_bottom(&mut self) {
        self.viewport.transcript_scroll = TranscriptScroll::to_bottom();
        self.viewport.pending_scroll_delta = 0;
        self.viewport.jump_to_latest_button_area = None;
        self.user_scrolled_during_stream = false;
        self.needs_redraw = true;
    }

    pub fn insert_char(&mut self, c: char) {
        self.clear_input_history_navigation();
        self.auto_expand_oversized_paste();
        self.delete_selection();
        self.selected_attachment_index = None;
        let cursor = self.cursor_position.min(char_count(&self.input));
        let byte_index = byte_index_at_char(&self.input, cursor);
        self.input.insert(byte_index, c);
        self.cursor_position = cursor + 1;
        self.strip_raw_mouse_reports_from_input();
        self.slash_menu_hidden = false;
        self.mention_menu_hidden = false;
        self.mention_menu_selected = 0;
        self.needs_redraw = true;
    }

    fn strip_raw_mouse_reports_from_input(&mut self) {
        if let Some((input, cursor_position)) =
            strip_raw_mouse_report_runs(&self.input, self.cursor_position)
        {
            self.input = input;
            self.cursor_position = cursor_position;
        }
    }

    pub fn delete_char(&mut self) {
        self.clear_input_history_navigation();
        self.auto_expand_oversized_paste();
        if self.delete_selection() {
            return;
        }
        self.selected_attachment_index = None;
        if self.cursor_position == 0 {
            return;
        }
        let target = self.cursor_position.saturating_sub(1);
        let removed = remove_char_at(&mut self.input, target);
        if removed {
            self.cursor_position = target;
            self.slash_menu_hidden = false;
            self.mention_menu_hidden = false;
            self.mention_menu_selected = 0;
            self.needs_redraw = true;
        }
    }

    pub fn delete_char_forward(&mut self) {
        self.clear_input_history_navigation();
        self.auto_expand_oversized_paste();
        if self.delete_selection() {
            return;
        }
        self.selected_attachment_index = None;
        if self.input.is_empty() {
            return;
        }
        let target = self.cursor_position;
        let removed = remove_char_at(&mut self.input, target);
        if !removed {
            self.cursor_position = char_count(&self.input);
        }
        self.slash_menu_hidden = false;
        self.mention_menu_hidden = false;
        self.mention_menu_selected = 0;
        self.needs_redraw = true;
    }

    /// Delete the word before the cursor.
    pub fn delete_word_backward(&mut self) {
        self.clear_input_history_navigation();
        if self.delete_selection() {
            return;
        }
        self.selected_attachment_index = None;
        if self.cursor_position == 0 {
            return;
        }

        let cursor_byte = byte_index_at_char(&self.input, self.cursor_position);
        let mut word_start = cursor_byte;

        while word_start > 0 {
            let Some((prev, ch)) = self.input[..word_start].char_indices().next_back() else {
                break;
            };
            if !ch.is_whitespace() {
                break;
            }
            word_start = prev;
        }

        while word_start > 0 {
            let Some((prev, ch)) = self.input[..word_start].char_indices().next_back() else {
                break;
            };
            if ch.is_whitespace() {
                break;
            }
            word_start = prev;
        }

        if word_start < cursor_byte {
            self.input.replace_range(word_start..cursor_byte, "");
            self.cursor_position = char_count(&self.input[..word_start]);
            self.slash_menu_hidden = false;
            self.mention_menu_hidden = false;
            self.mention_menu_selected = 0;
            self.needs_redraw = true;
        }
    }

    /// Delete from the cursor to the start of the line.
    pub fn delete_to_start_of_line(&mut self) {
        self.clear_input_history_navigation();
        if self.delete_selection() {
            return;
        }
        self.selected_attachment_index = None;
        if self.cursor_position == 0 {
            return;
        }

        let cursor_byte = byte_index_at_char(&self.input, self.cursor_position);
        // Find the start of the current line (last newline or start of string)
        let line_start = self.input[..cursor_byte]
            .rfind('\n')
            .map(|idx| idx + 1)
            .unwrap_or(0);

        if line_start < cursor_byte {
            self.input.replace_range(line_start..cursor_byte, "");
            self.cursor_position = char_count(&self.input[..line_start]);
            self.slash_menu_hidden = false;
            self.mention_menu_hidden = false;
            self.mention_menu_selected = 0;
            self.needs_redraw = true;
        }
    }

    /// Delete the word after the cursor.
    pub fn delete_word_forward(&mut self) {
        self.clear_input_history_navigation();
        if self.delete_selection() {
            return;
        }
        self.selected_attachment_index = None;
        let cursor_byte = byte_index_at_char(&self.input, self.cursor_position);
        if cursor_byte >= self.input.len() {
            return;
        }

        let mut word_end = cursor_byte;
        while word_end < self.input.len() {
            let Some(ch) = self.input[word_end..].chars().next() else {
                break;
            };
            if !ch.is_whitespace() {
                break;
            }
            word_end += ch.len_utf8();
        }

        while word_end < self.input.len() {
            let Some(ch) = self.input[word_end..].chars().next() else {
                break;
            };
            if ch.is_whitespace() {
                break;
            }
            word_end += ch.len_utf8();
        }

        if cursor_byte < word_end {
            self.input.replace_range(cursor_byte..word_end, "");
            self.slash_menu_hidden = false;
            self.mention_menu_hidden = false;
            self.mention_menu_selected = 0;
            self.needs_redraw = true;
        }
    }

    /// Cut from the cursor to the end of the current logical line into the
    /// kill buffer. If the cursor is already at end-of-line and a trailing
    /// newline exists, that newline is consumed so repeated invocations
    /// continue to make progress (matching emacs/codex semantics).
    ///
    /// Returns `true` when bytes were moved into the kill buffer.
    pub fn kill_to_end_of_line(&mut self) -> bool {
        self.clear_input_history_navigation();
        if let Some((start, end)) = self.selection_range() {
            let sb = byte_index_at_char(&self.input, start);
            let eb = byte_index_at_char(&self.input, end);
            self.kill_buffer = self.input[sb..eb].to_string();
            self.delete_selection();
            return true;
        }
        let total_chars = char_count(&self.input);
        let cursor = self.cursor_position.min(total_chars);
        let start_byte = byte_index_at_char(&self.input, cursor);

        // Find the byte offset of the next '\n' (relative to the whole string)
        // or the end of the buffer if no newline exists at/after the cursor.
        let eol_byte = self.input[start_byte..]
            .find('\n')
            .map(|rel| start_byte + rel)
            .unwrap_or_else(|| self.input.len());

        let end_byte = if start_byte == eol_byte {
            // Cursor is at EOL — consume the newline itself if one is there.
            if eol_byte < self.input.len() {
                eol_byte + 1
            } else {
                return false;
            }
        } else {
            eol_byte
        };

        let removed: String = self.input[start_byte..end_byte].to_string();
        if removed.is_empty() {
            return false;
        }

        self.kill_buffer = removed;
        self.input.replace_range(start_byte..end_byte, "");
        // Cursor stays at the same character index (start of removed range).
        self.cursor_position = cursor;
        self.slash_menu_hidden = false;
        self.mention_menu_hidden = false;
        self.mention_menu_selected = 0;
        self.needs_redraw = true;
        true
    }

    /// Insert the contents of the kill buffer at the cursor, advancing it.
    /// The kill buffer is left intact so multiple yanks duplicate the text.
    /// Returns `true` if any text was inserted.
    pub fn yank(&mut self) -> bool {
        if self.kill_buffer.is_empty() {
            return false;
        }
        self.delete_selection();
        self.clear_input_history_navigation();
        let text = self.kill_buffer.clone();
        let cursor = self.cursor_position.min(char_count(&self.input));
        let byte_index = byte_index_at_char(&self.input, cursor);
        self.input.insert_str(byte_index, &text);
        self.cursor_position = cursor + char_count(&text);
        self.slash_menu_hidden = false;
        self.mention_menu_hidden = false;
        self.mention_menu_selected = 0;
        self.needs_redraw = true;
        true
    }

    pub fn move_cursor_left(&mut self) {
        self.cursor_position = self.cursor_position.saturating_sub(1);
        self.needs_redraw = true;
    }

    pub fn move_cursor_right(&mut self) {
        if self.cursor_position < char_count(&self.input) {
            self.cursor_position += 1;
            self.needs_redraw = true;
        }
    }

    pub fn move_cursor_start(&mut self) {
        self.cursor_position = 0;
        self.needs_redraw = true;
    }

    pub fn move_cursor_end(&mut self) {
        self.cursor_position = char_count(&self.input);
        self.needs_redraw = true;
    }

    /// In a multiline composer, jump to the start of the current line.
    /// On single-line input this is equivalent to `move_cursor_start`.
    pub fn move_cursor_line_start(&mut self) {
        let byte_pos = byte_index_at_char(&self.input, self.cursor_position);
        let before = &self.input[..byte_pos];
        if let Some(last_nl_byte) = before.rfind('\n') {
            // Position after the '\n' (start of the current line).
            self.cursor_position = char_count(&self.input[..=last_nl_byte]);
        } else {
            self.cursor_position = 0;
        }
        self.needs_redraw = true;
    }

    /// In a multiline composer, jump to the end of the current line
    /// (just before the next `\n` or at the end of input).
    /// On single-line input this is equivalent to `move_cursor_end`.
    pub fn move_cursor_line_end(&mut self) {
        let search_start = byte_index_at_char(&self.input, self.cursor_position);
        if let Some(offset) = self.input[search_start..].find('\n') {
            self.cursor_position = char_count(&self.input[..search_start + offset]);
        } else {
            self.cursor_position = char_count(&self.input);
        }
        self.needs_redraw = true;
    }

    /// Move forward one word. Skips over the current word then any trailing
    /// whitespace to land on the first character of the next word.
    pub fn move_cursor_word_forward(&mut self) {
        let text = self.input.clone();
        let total = char_count(&text);
        let mut pos = self.cursor_position;
        if pos >= total {
            return;
        }
        // Skip non-whitespace (current word).
        while pos < total {
            let byte = byte_index_at_char(&text, pos);
            let ch = text[byte..].chars().next().unwrap_or(' ');
            if ch.is_whitespace() {
                break;
            }
            pos += 1;
        }
        // Skip whitespace.
        while pos < total {
            let byte = byte_index_at_char(&text, pos);
            let ch = text[byte..].chars().next().unwrap_or(' ');
            if !ch.is_whitespace() {
                break;
            }
            pos += 1;
        }
        self.cursor_position = pos;
        self.needs_redraw = true;
    }

    /// Move backward one word. Skips leading whitespace then the preceding
    /// word to land on its first character.
    pub fn move_cursor_word_backward(&mut self) {
        let text = self.input.clone();
        let mut pos = self.cursor_position;
        if pos == 0 {
            return;
        }
        // Step back one so we're not already at the word start.
        pos -= 1;
        // Skip whitespace.
        while pos > 0 {
            let byte = byte_index_at_char(&text, pos);
            let ch = text[byte..].chars().next().unwrap_or(' ');
            if !ch.is_whitespace() {
                break;
            }
            pos -= 1;
        }
        // Skip non-whitespace.
        while pos > 0 {
            let byte = byte_index_at_char(&text, pos - 1);
            let ch = text[byte..].chars().next().unwrap_or(' ');
            if ch.is_whitespace() {
                break;
            }
            pos -= 1;
        }
        self.cursor_position = pos;
        self.needs_redraw = true;
    }

    // === Selection helpers ===

    /// Return the (start, end) of the active selection, or `None`.
    /// `start` is inclusive, `end` is exclusive; both are char indices.
    pub fn selection_range(&self) -> Option<(usize, usize)> {
        let total = char_count(&self.input);
        let anchor = self.selection_anchor?.min(total);
        let cursor = self.cursor_position.min(total);
        if anchor == cursor {
            return None;
        }
        Some(if anchor < cursor {
            (anchor, cursor)
        } else {
            (cursor, anchor)
        })
    }

    /// Return the selected text, or empty string if no selection.
    pub fn selected_text(&self) -> String {
        self.selection_range()
            .map(|(s, e)| {
                let sb = byte_index_at_char(&self.input, s);
                let eb = byte_index_at_char(&self.input, e);
                self.input[sb..eb].to_string()
            })
            .unwrap_or_default()
    }

    /// Delete the selected text, place cursor at the start of the deleted range.
    /// Returns true if a selection was deleted.
    pub fn delete_selection(&mut self) -> bool {
        let Some((start, end)) = self.selection_range() else {
            return false;
        };
        let sb = byte_index_at_char(&self.input, start);
        let eb = byte_index_at_char(&self.input, end);
        self.input.replace_range(sb..eb, "");
        self.cursor_position = start;
        self.selection_anchor = None;
        self.clear_input_history_navigation();
        self.slash_menu_hidden = false;
        self.mention_menu_hidden = false;
        self.mention_menu_selected = 0;
        self.needs_redraw = true;
        true
    }

    /// Clear the selection without moving the cursor.
    pub fn clear_selection(&mut self) {
        self.selection_anchor = None;
    }

    // === Vim composer mode helpers ===

    /// Move the cursor to the start of the current logical line (vim `0`).
    pub fn vim_move_line_start(&mut self) {
        let text = self.input.clone();
        let cursor_byte = byte_index_at_char(&text, self.cursor_position);
        // Walk backward until we find a newline or the start of the string.
        let line_start_byte = text[..cursor_byte].rfind('\n').map_or(0, |idx| idx + 1);
        self.cursor_position = char_count(&text[..line_start_byte]);
        self.needs_redraw = true;
    }

    /// Move the cursor to the end of the current logical line (vim `$`).
    pub fn vim_move_line_end(&mut self) {
        let text = self.input.clone();
        let cursor_byte = byte_index_at_char(&text, self.cursor_position);
        // Walk forward to the next newline or end-of-string.
        let line_end_char = text[cursor_byte..].find('\n').map_or_else(
            || char_count(&text),
            |rel| char_count(&text[..cursor_byte + rel]),
        );
        self.cursor_position = line_end_char;
        self.needs_redraw = true;
    }

    /// Move forward one word (vim `w`).  Skips over the current word then any
    /// trailing whitespace to land on the first character of the next word.
    pub fn vim_move_word_forward(&mut self) {
        self.move_cursor_word_forward();
    }

    /// Move backward one word (vim `b`).  Skips leading whitespace then the
    /// preceding word to land on its first character.
    pub fn vim_move_word_backward(&mut self) {
        self.move_cursor_word_backward();
    }

    /// Delete the character under the cursor (vim `x`).
    pub fn vim_delete_char_under_cursor(&mut self) {
        self.auto_expand_oversized_paste();
        let total = char_count(&self.input);
        if self.cursor_position >= total {
            return;
        }
        let pos = self.cursor_position;
        remove_char_at(&mut self.input, pos);
        // Keep cursor in bounds after deletion.
        let new_total = char_count(&self.input);
        if self.cursor_position > 0 && self.cursor_position >= new_total {
            self.cursor_position = new_total.saturating_sub(1);
        }
        self.needs_redraw = true;
    }

    /// Delete the entire current logical line (vim `dd`).
    pub fn vim_delete_line(&mut self) {
        let text = self.input.clone();
        let cursor_byte = byte_index_at_char(&text, self.cursor_position);
        let line_start_byte = text[..cursor_byte].rfind('\n').map_or(0, |idx| idx + 1);
        let line_end_byte = text[cursor_byte..]
            .find('\n')
            .map_or(text.len(), |rel| cursor_byte + rel);

        // Include the trailing newline if present, or the leading newline for the
        // very last non-terminated line to avoid leaving a dangling newline.
        let (remove_start, remove_end) = if line_end_byte < text.len() {
            // There is a newline after the line — remove it too.
            (line_start_byte, line_end_byte + 1)
        } else if line_start_byte > 0 {
            // Last line without trailing newline — remove the preceding newline.
            (line_start_byte - 1, line_end_byte)
        } else {
            // Only line in the buffer.
            (line_start_byte, line_end_byte)
        };

        self.input.replace_range(remove_start..remove_end, "");
        self.cursor_position = char_count(&self.input[..remove_start]);
        self.needs_redraw = true;
    }

    /// Enter insert mode at the cursor (vim `i`).
    pub fn vim_enter_insert(&mut self) {
        self.vim_mode = VimMode::Insert;
        self.needs_redraw = true;
    }

    /// Enter insert mode after the cursor (vim `a`).
    pub fn vim_enter_append(&mut self) {
        let total = char_count(&self.input);
        if self.cursor_position < total {
            self.cursor_position += 1;
        }
        self.vim_mode = VimMode::Insert;
        self.needs_redraw = true;
    }

    /// Open a new line below and enter insert mode (vim `o`).
    pub fn vim_open_line_below(&mut self) {
        // Move to end of line, then insert a newline.
        self.vim_move_line_end();
        self.insert_char('\n');
        self.vim_mode = VimMode::Insert;
    }

    /// Return to Normal mode from Insert or Visual (vim `Esc`).
    pub fn vim_enter_normal(&mut self) {
        self.vim_mode = VimMode::Normal;
        self.vim_pending_d = false;
        // In Normal mode the cursor sits on a character, not after the last one.
        let total = char_count(&self.input);
        if self.cursor_position > 0 && self.cursor_position >= total {
            self.cursor_position = total.saturating_sub(1);
        }
        self.needs_redraw = true;
    }

    /// Returns `true` when vim mode is active and the composer is in Normal
    /// mode, which means character keys should NOT be inserted as text.
    #[must_use]
    pub fn vim_is_normal_mode(&self) -> bool {
        self.composer.vim_enabled && self.composer.vim_mode == VimMode::Normal
    }

    /// Returns `true` when vim mode is active and the composer is in Visual mode.
    #[must_use]
    pub fn vim_is_visual_mode(&self) -> bool {
        self.composer.vim_enabled && self.composer.vim_mode == VimMode::Visual
    }

    /// Move the cursor down one logical line within the buffer (vim `j`).
    /// Falls back to history-down when already on the last line.
    pub fn vim_move_down(&mut self) {
        let text = self.input.clone();
        let total = char_count(&text);
        if self.cursor_position >= total {
            self.history_down();
            return;
        }
        let cursor_byte = byte_index_at_char(&text, self.cursor_position);
        let rest = &text[cursor_byte..];
        if let Some(rel_nl) = rest.find('\n') {
            // Column offset on the current line.
            let line_start_byte = text[..cursor_byte].rfind('\n').map_or(0, |i| i + 1);
            let col = char_count(&text[line_start_byte..cursor_byte]);
            let next_line_start = cursor_byte + rel_nl + 1;
            let next_line = &text[next_line_start..];
            let next_line_len = next_line.find('\n').unwrap_or(next_line.len());
            let next_line_char_len =
                char_count(&text[next_line_start..next_line_start + next_line_len]);
            let target_col = col.min(next_line_char_len);
            self.cursor_position = char_count(&text[..next_line_start]) + target_col;
            self.needs_redraw = true;
        } else {
            self.history_down();
        }
    }

    /// Move the cursor up one logical line within the buffer (vim `k`).
    /// Falls back to history-up when already on the first line.
    pub fn vim_move_up(&mut self) {
        let text = self.input.clone();
        let cursor_byte = byte_index_at_char(&text, self.cursor_position);
        if let Some(prev_nl) = text[..cursor_byte].rfind('\n') {
            // Column on the current line.
            let line_start_byte = prev_nl + 1;
            let col = char_count(&text[line_start_byte..cursor_byte]);
            // Find start of the previous line.
            let prev_line_end = prev_nl; // byte of the newline itself
            let prev_start = text[..prev_line_end].rfind('\n').map_or(0, |i| i + 1);
            let prev_line_len = char_count(&text[prev_start..prev_line_end]);
            let target_col = col.min(prev_line_len);
            self.cursor_position = char_count(&text[..prev_start]) + target_col;
            self.needs_redraw = true;
        } else {
            self.history_up();
        }
    }

    pub fn clear_input(&mut self) {
        self.clear_input_history_navigation();
        self.input.clear();
        self.cursor_position = 0;
        // Prevent stale oversized-paste state from leaking when the user
        // clears the composer or navigates to a different input (#3263).
        self.pending_paste_reference = None;
        self.oversized_paste_full_text = None;
        self.selection_anchor = None;
        self.selected_attachment_index = None;
        self.slash_menu_selected = 0;
        self.slash_menu_hidden = false;
        self.paste_burst.clear_after_explicit_paste();
        self.needs_redraw = true;
    }

    pub fn clear_input_recoverable(&mut self) {
        self.stash_current_input_for_recovery();
        self.clear_input();
    }

    pub fn stash_current_input_for_recovery(&mut self) {
        // Before stashing, expand any truncated paste so the saved draft
        // contains the full text, not the truncated preview (#3263).
        self.auto_expand_oversized_paste();
        let draft = self.input.clone();
        if draft.trim().is_empty() {
            self.clear_undo_buffer = None;
            return;
        }
        self.clear_undo_buffer = Some(draft.clone());
        self.remember_draft_for_recovery(draft);
    }

    fn remember_draft_for_recovery(&mut self, draft: String) {
        if draft.trim().is_empty() {
            return;
        }
        self.draft_history.retain(|existing| existing != &draft);
        self.draft_history.push_back(draft);
        while self.draft_history.len() > MAX_DRAFT_HISTORY {
            let _ = self.draft_history.pop_front();
        }
    }

    pub fn start_history_search(&mut self) {
        if self.composer_history_search.is_some() {
            return;
        }
        // Expand any truncated paste first so the history search seed
        // contains the full text, not the truncated preview (#3263).
        self.auto_expand_oversized_paste();
        self.composer_history_search = Some(ComposerHistorySearch::new(
            self.input.clone(),
            self.cursor_position,
        ));
        self.slash_menu_hidden = true;
        self.mention_menu_hidden = true;
        self.paste_burst.clear_after_explicit_paste();
        self.status_message = Some("History search: type to filter, Enter accepts".to_string());
        self.needs_redraw = true;
    }

    pub fn is_history_search_active(&self) -> bool {
        self.composer_history_search.is_some()
    }

    pub fn history_search_query(&self) -> Option<&str> {
        self.composer_history_search
            .as_ref()
            .map(|search| search.query.as_str())
    }

    pub fn history_search_selected_index(&self) -> usize {
        self.composer_history_search
            .as_ref()
            .map_or(0, |search| search.selected)
    }

    pub fn composer_display_input(&self) -> &str {
        self.history_search_query().unwrap_or(&self.input)
    }

    pub fn composer_display_cursor(&self) -> usize {
        self.composer_history_search
            .as_ref()
            .map_or(self.cursor_position, |search| char_count(&search.query))
    }

    pub fn history_search_matches(&self) -> Vec<String> {
        let Some(query) = self.history_search_query() else {
            return Vec::new();
        };
        self.history_search_matches_for_query(query)
    }

    fn history_search_matches_for_query(&self, query: &str) -> Vec<String> {
        let normalized_query = query.trim().to_lowercase();
        let mut seen: HashSet<&str> = HashSet::new();
        let mut matches = Vec::new();

        for candidate in self
            .draft_history
            .iter()
            .rev()
            .chain(self.input_history.iter().rev())
        {
            if candidate.trim().is_empty() || !seen.insert(candidate.as_str()) {
                continue;
            }
            if normalized_query.is_empty() || candidate.to_lowercase().contains(&normalized_query) {
                matches.push(candidate.clone());
            }
        }

        matches
    }

    fn clamp_history_search_selection(&mut self) {
        let Some(search) = self.composer_history_search.as_ref() else {
            return;
        };
        let selected = search.selected;
        let query = search.query.clone();
        let match_count = self.history_search_matches_for_query(&query).len();
        if let Some(search) = self.composer_history_search.as_mut() {
            search.selected = if match_count == 0 {
                0
            } else {
                selected.min(match_count.saturating_sub(1))
            };
        }
    }

    pub fn history_search_insert_char(&mut self, ch: char) {
        if let Some(search) = self.composer_history_search.as_mut() {
            search.query.push(ch);
            search.selected = 0;
            self.status_message = Some("History search: Enter accepts, Esc restores".to_string());
            self.needs_redraw = true;
        }
    }

    pub fn history_search_insert_str(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(search) = self.composer_history_search.as_mut() {
            search.query.push_str(&normalize_paste_text(text));
            search.selected = 0;
            self.status_message = Some("History search: Enter accepts, Esc restores".to_string());
            self.needs_redraw = true;
        }
    }

    pub fn history_search_backspace(&mut self) {
        if let Some(search) = self.composer_history_search.as_mut() {
            search.query.pop();
            search.selected = 0;
            self.needs_redraw = true;
        }
        self.clamp_history_search_selection();
    }

    pub fn history_search_select_previous(&mut self) {
        if let Some(search) = self.composer_history_search.as_mut() {
            search.selected = search.selected.saturating_sub(1);
            self.needs_redraw = true;
        }
    }

    pub fn history_search_select_next(&mut self) {
        let Some(search) = self.composer_history_search.as_ref() else {
            return;
        };
        let query = search.query.clone();
        let selected = search.selected;
        let match_count = self.history_search_matches_for_query(&query).len();
        if let Some(search) = self.composer_history_search.as_mut()
            && match_count > 0
        {
            search.selected = (selected + 1).min(match_count.saturating_sub(1));
            self.needs_redraw = true;
        }
    }

    pub fn accept_history_search(&mut self) -> bool {
        let Some(search) = self.composer_history_search.take() else {
            return false;
        };
        let matches = self.history_search_matches_for_query(&search.query);
        if let Some(selected) = matches
            .get(search.selected.min(matches.len().saturating_sub(1)))
            .cloned()
        {
            self.input = selected;
            self.cursor_position = char_count(&self.input);
            self.history_index = None;
            self.status_message = Some("History match inserted into composer".to_string());
            self.needs_redraw = true;
            true
        } else {
            self.composer_history_search = Some(search);
            self.status_message = Some("No history matches".to_string());
            self.needs_redraw = true;
            false
        }
    }

    pub fn cancel_history_search(&mut self) {
        let Some(search) = self.composer_history_search.take() else {
            return;
        };
        self.input = search.pre_search_input;
        self.cursor_position = search.pre_search_cursor.min(char_count(&self.input));
        self.status_message = Some("History search canceled".to_string());
        self.needs_redraw = true;
    }

    pub fn submit_input(&mut self) -> Option<String> {
        if self.input.trim().is_empty() {
            self.paste_burst.clear_after_explicit_paste();
            return None;
        }
        // Safety net: if any earlier path filled the buffer above the
        // safety cap without going through `insert_paste_text`, fold it
        // into a workspace paste file now (#553). Bracketed pastes hit
        // the consolidation in `insert_paste_text` first, so the user
        // sees the @mention in the composer before submission.
        self.consolidate_large_input_if_oversized();
        // If consolidation created a paste file, restore the full text and
        // append the @mention so the model can read the complete content
        // while the composer stays editable (#3263).
        let mut input = self
            .oversized_paste_full_text
            .take()
            .unwrap_or_else(|| self.input.clone());
        if let Some(reference) = self.pending_paste_reference.take() {
            if !input.is_empty() && !input.ends_with('\n') {
                input.push('\n');
            }
            input.push_str(&reference);
        }
        if !looks_like_slash_command_input(&input) {
            self.input_history.push(input.clone());
            if self.max_input_history == 0 {
                self.input_history.clear();
            } else if self.input_history.len() > self.max_input_history {
                let excess = self.input_history.len() - self.max_input_history;
                self.input_history.drain(0..excess);
            }
            // Mirror to the persisted cross-session history (#366) so
            // arrow-up recall works across restarts. Best-effort write —
            // see `composer_history::append_history` for failure modes.
            crate::composer_history::append_history(&input);
        }
        self.history_index = None;
        self.history_navigation_draft = None;
        self.clear_input();
        Some(input)
    }

    pub fn restore_last_submitted_prompt_if_empty(&mut self) -> bool {
        if !self.input.is_empty() {
            return false;
        }
        let Some(prompt) = self
            .last_submitted_prompt
            .as_deref()
            .filter(|prompt| !prompt.is_empty())
        else {
            return false;
        };

        self.input = prompt.to_string();
        self.cursor_position = char_count(&self.input);
        self.history_index = None;
        self.history_navigation_draft = None;
        self.selected_attachment_index = None;
        self.needs_redraw = true;
        true
    }

    /// Restore the last cleared input if the composer is empty.
    /// Returns `true` if the input was restored.
    pub fn restore_last_cleared_input_if_empty(&mut self) -> bool {
        if !self.input.is_empty() {
            return false;
        }
        let Some(saved) = self.clear_undo_buffer.take().filter(|s| !s.is_empty()) else {
            return false;
        };

        self.input = saved;
        self.cursor_position = char_count(&self.input);
        self.history_index = None;
        self.history_navigation_draft = None;
        self.selected_attachment_index = None;
        self.slash_menu_selected = 0;
        self.slash_menu_hidden = false;
        self.needs_redraw = true;
        self.clear_undo_buffer = None;
        true
    }

    /// Composer-Enter dispatch. Returns `Some(input)` when the press should
    /// fire a submit; `None` when Enter was absorbed (paste-burst Enter
    /// suppression — see #1073).
    ///
    /// Two suppression cases are handled here. Both are silent: nothing
    /// visible happens beyond the text gaining a newline.
    ///
    /// 1. **Burst active.** A paste burst is currently being assembled in
    ///    `paste_burst.buffer`. The Enter is part of the paste content;
    ///    append `\n` to the buffer so the next flush includes it, do not
    ///    submit, and extend the suppression window so a follow-on Enter
    ///    (i.e. the *next* line of a multi-line paste) is also absorbed.
    /// 2. **Window open after flush.** A burst just flushed into
    ///    `self.input`, but the suppression window is still alive. The
    ///    Enter is the trailing newline of that paste, not a submit gesture
    ///    by the user. Insert `\n` directly into the composer text and
    ///    re-arm the window.
    ///
    /// Outside both cases the call falls through to [`Self::submit_input`]
    /// unchanged so normal Enter-to-send behaviour is preserved.
    pub fn handle_composer_enter(&mut self) -> Option<String> {
        if self.use_paste_burst_detection {
            let now = Instant::now();
            if self
                .paste_burst
                .newline_should_insert_instead_of_submit(now)
            {
                if !self.paste_burst.append_newline_if_active(now) {
                    self.insert_char('\n');
                    self.paste_burst.extend_window(now);
                }
                self.needs_redraw = true;
                return None;
            }
        }
        self.submit_input()
    }

    /// Public wrapper around [`Self::consolidate_large_input`] that no-ops
    /// when the current input fits inside the safety cap. Both the paste-
    /// insert path (visible-before-submit) and the submit-time safety net
    /// route through here, so the cap is enforced exactly once even when
    /// both paths fire on the same buffer.
    fn consolidate_large_input_if_oversized(&mut self) {
        if char_count(&self.input) > MAX_SUBMITTED_INPUT_CHARS {
            self.consolidate_large_input();
        }
    }

    /// When the composer input exceeds [`MAX_SUBMITTED_INPUT_CHARS`], write
    /// the full content to a timestamped paste file under
    /// `.codewhale/pastes/` and replace `self.input` with an `@`-mention
    /// pointing at it so the model can read the full content via the
    /// normal file-mention resolution path (#553).
    fn consolidate_large_input(&mut self) {
        let full_input = std::mem::take(&mut self.input);
        self.cursor_position = 0;

        let now = chrono::Local::now();
        let suffix = uuid::Uuid::new_v4().to_string()[..8].to_string();
        let filename = format!("paste-{}-{}.md", now.format("%Y-%m-%d-%H%M%S"), suffix);
        let rel_path = format!(".codewhale/pastes/{filename}");

        let pastes_dir = self.workspace.join(".codewhale/pastes");
        if let Err(e) = std::fs::create_dir_all(&pastes_dir) {
            // Fallback: keep a truncated version so we don't lose the
            // user's input entirely when the filesystem is unhappy.
            self.input = full_input.chars().take(MAX_SUBMITTED_INPUT_CHARS).collect();
            self.cursor_position = char_count(&self.input);
            self.push_status_toast(
                format!("Failed to create paste directory: {e}"),
                StatusToastLevel::Error,
                Some(8_000),
            );
            return;
        }

        let file_path = self.workspace.join(&rel_path);
        if let Err(e) = std::fs::write(&file_path, &full_input) {
            self.input = full_input.chars().take(MAX_SUBMITTED_INPUT_CHARS).collect();
            self.cursor_position = char_count(&self.input);
            self.push_status_toast(
                format!("Failed to write paste file: {e}"),
                StatusToastLevel::Error,
                Some(8_000),
            );
            return;
        }

        // Keep a truncated preview in the composer so the user can still
        // select, copy, and edit it, while the full text is stored for
        // model submission. The @mention is appended at submit time (#3263).
        self.pending_paste_reference = Some(format!("@{rel_path}"));
        self.oversized_paste_full_text = Some(full_input.clone());
        let display_chars = char_count(&full_input).min(MAX_COMPOSER_DISPLAY_CHARS);
        let mut truncated: String = full_input.chars().take(display_chars).collect();
        if char_count(&full_input) > MAX_COMPOSER_DISPLAY_CHARS {
            truncated.push_str("\n\n---\n(content truncated for display — start typing to expand; full text sent to model)");
        }
        self.input = truncated;
        self.cursor_position = 0;
        self.push_status_toast(
            "Large paste backed up to file — the model will receive the full content.",
            StatusToastLevel::Info,
            Some(5_000),
        );
    }

    pub fn queue_message(&mut self, message: QueuedMessage) {
        self.queued_messages.push_back(message);
    }

    pub fn pop_queued_message(&mut self) -> Option<QueuedMessage> {
        self.queued_messages.pop_front()
    }

    pub fn remove_queued_message(&mut self, index: usize) -> Option<QueuedMessage> {
        self.queued_messages.remove(index)
    }

    pub fn queued_message_count(&self) -> usize {
        self.queued_messages.len()
    }

    /// Pop the most-recently queued message back into the composer for editing
    /// (issue #85 — ↑ affordance). The popped message is parked in
    /// [`Self::queued_draft`] so the next Enter re-queues it carrying its
    /// original skill instruction. No-op if the composer already has typed
    /// content or a draft is already being edited — surfacing the affordance
    /// would be ambiguous in either case.
    ///
    /// Returns `true` when the composer state was mutated.
    pub fn pop_last_queued_into_draft(&mut self) -> bool {
        if !self.input.is_empty() || self.queued_draft.is_some() {
            return false;
        }
        let Some(msg) = self.queued_messages.pop_back() else {
            return false;
        };
        self.input = msg.display.clone();
        self.cursor_position = char_count(&self.input);
        self.selected_attachment_index = None;
        self.queued_draft = Some(msg);
        self.needs_redraw = true;
        true
    }

    /// Stop editing a queued follow-up and put the original queued message back
    /// at the tail where [`Self::pop_last_queued_into_draft`] took it from.
    pub fn cancel_queued_draft_edit(&mut self) -> bool {
        let Some(draft) = self.queued_draft.take() else {
            return false;
        };
        self.queued_messages.push_back(draft);
        self.clear_input_recoverable();
        self.needs_redraw = true;
        true
    }

    /// Park a legacy pending steer. New keyboard handling routes running-turn
    /// drafts through Enter (same-turn steer) or Tab (next-turn follow-up).
    #[allow(dead_code)]
    pub fn push_pending_steer(&mut self, message: QueuedMessage) {
        self.pending_steers.push_back(message);
        self.submit_pending_steers_after_interrupt = true;
        self.needs_redraw = true;
    }

    /// Drain the pending-steer queue and clear the resend flag. Returns the
    /// messages in submit order (oldest first).
    pub fn drain_pending_steers(&mut self) -> Vec<QueuedMessage> {
        self.submit_pending_steers_after_interrupt = false;
        if self.pending_steers.is_empty() {
            return Vec::new();
        }
        self.needs_redraw = true;
        self.pending_steers.drain(..).collect()
    }

    /// Decide how to route a fresh composer submit.
    ///
    /// v0.8.68: streaming output queues. Busy-but-waiting turns steer so
    /// Enter can amend the active turn before output starts. A double-tap
    /// Enter within 500 ms triggers Steer while streaming; Ctrl+Enter forces
    /// Steer in all busy states.
    ///
    /// Truth table:
    ///   offline=F, busy=F → Immediate
    ///   offline=F, busy=T, streaming=F → Steer
    ///   offline=F, busy=T, streaming=T → Queue (double-tap → Steer)
    ///   offline=T, busy=* → Queue
    #[must_use]
    pub fn decide_submit_disposition(&self) -> SubmitDisposition {
        if self.offline_mode {
            return SubmitDisposition::Queue;
        }
        if !self.is_loading {
            return SubmitDisposition::Immediate;
        }
        if self.streaming_message_index.is_none() {
            return SubmitDisposition::Steer;
        }
        // Streaming: queue the message. Double-tap Enter within 500 ms
        // triggers Steer via enter_with_double_tap(); see the ui.rs submit
        // handler.
        SubmitDisposition::Queue
    }

    /// Process an Enter keypress with double-tap steering detection.
    ///
    /// When the engine is busy, the first Enter queues the message. A second
    /// Enter within 500 ms triggers Steer (interrupt the current turn to
    /// inject the new instruction immediately). When idle, Enter submits
    /// immediately.
    #[must_use]
    pub fn enter_with_double_tap(&mut self) -> Option<SubmitDisposition> {
        let disposition = self.decide_submit_disposition();
        match disposition {
            SubmitDisposition::Queue => {
                if let Some(instant) = self.last_enter_instant
                    && instant.elapsed() < Duration::from_millis(500)
                {
                    self.last_enter_instant = None;
                    return Some(SubmitDisposition::Steer);
                }
                self.last_enter_instant = Some(Instant::now());
                Some(SubmitDisposition::Queue)
            }
            other => {
                self.last_enter_instant = None;
                Some(other)
            }
        }
    }

    /// Mark the in-flight streaming Assistant cell as interrupted: prepend
    /// `[interrupted]` to whatever streamed so far (so the user can see what
    /// was salvaged) and flip `streaming` off so the spinner halts. No-op if
    /// no Assistant cell is currently streaming.
    ///
    /// Deliberate divergence from openai/codex which discards partial output
    /// on abort — V4 thinking is expensive and the user usually wants to see
    /// what the model produced before steering.
    pub fn finalize_streaming_assistant_as_interrupted(&mut self) {
        let Some(index) = self.streaming_message_index.take() else {
            return;
        };
        if let Some(HistoryCell::Assistant { content, streaming }) = self.history.get_mut(index) {
            *streaming = false;
            if content.is_empty() {
                *content = "[interrupted]".to_string();
            } else if !content.starts_with("[interrupted]") {
                content.insert_str(0, "[interrupted] ");
            }
        }
        self.bump_history_cell(index);
    }

    pub fn history_up(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        if self.history_index.is_none() {
            // Expand truncated paste first so the saved draft contains the
            // full text instead of the truncated preview (#3263).
            self.auto_expand_oversized_paste();
            self.history_navigation_draft = Some(InputHistoryDraft {
                input: self.input.clone(),
                cursor: self.cursor_position,
            });
        }
        let new_index = match self.history_index {
            None => self.input_history.len().saturating_sub(1),
            Some(i) => i.saturating_sub(1),
        };
        self.history_index = Some(new_index);
        self.input = self.input_history[new_index].clone();
        self.cursor_position = char_count(&self.input);
        self.selection_anchor = None;
        self.selected_attachment_index = None;
        self.slash_menu_hidden = false;
        self.paste_burst.clear_after_explicit_paste();
    }

    pub fn history_down(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        match self.history_index {
            None => {}
            Some(i) => {
                if i + 1 < self.input_history.len() {
                    self.history_index = Some(i + 1);
                    self.input = self.input_history[i + 1].clone();
                    self.cursor_position = char_count(&self.input);
                    self.selection_anchor = None;
                    self.selected_attachment_index = None;
                    self.slash_menu_hidden = false;
                    self.paste_burst.clear_after_explicit_paste();
                } else {
                    self.history_index = None;
                    if let Some(draft) = self.history_navigation_draft.take() {
                        self.input = draft.input;
                        self.cursor_position = draft.cursor.min(char_count(&self.input));
                        self.selection_anchor = None;
                        self.selected_attachment_index = None;
                        self.slash_menu_hidden = false;
                        self.paste_burst.clear_after_explicit_paste();
                        self.needs_redraw = true;
                    } else {
                        self.clear_input();
                    }
                }
            }
        }
    }

    fn clear_input_history_navigation(&mut self) {
        self.history_index = None;
        self.history_navigation_draft = None;
    }

    /// Retry a `try_lock` up to `retries` times with a 1ms pause between
    /// attempts. Returns `Some(guard)` on success, `None` if the lock
    /// remains contended after all retries.
    fn retry_lock<T>(
        mutex: &tokio::sync::Mutex<T>,
        retries: u32,
    ) -> Option<tokio::sync::MutexGuard<'_, T>> {
        for _ in 0..retries {
            if let Ok(guard) = mutex.try_lock() {
                return Some(guard);
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        None
    }

    /// Capture the durable Work state without ever converting lock contention
    /// into an empty snapshot.
    pub fn work_state_snapshot(&self) -> Result<Option<SessionWorkState>, String> {
        if let Some(work) = self.runtime_services.work.as_ref() {
            return work
                .capture(self.current_session_id.as_deref())
                .map(|state| {
                    state.map(|state| SessionWorkState {
                        graph: Some(state.graph),
                        todos: state.todos,
                        plan: state.plan,
                    })
                });
        }
        let todos = Self::retry_lock(&self.todos, 100)
            .ok_or_else(|| "To-do state is busy; try saving again".to_string())?;
        let plan = Self::retry_lock(&self.plan_state, 100)
            .ok_or_else(|| "Plan state is busy; try saving again".to_string())?;
        let state = SessionWorkState {
            graph: None,
            todos: todos.snapshot(),
            plan: plan.snapshot(),
        };
        Ok((!state.is_empty()).then_some(state))
    }

    /// Non-blocking snapshot for the render/event loop. Automatic persistence
    /// must skip a contended first save instead of pausing the UI or writing a
    /// false empty state.
    pub fn try_work_state_snapshot(&mut self) -> Result<Option<SessionWorkState>, String> {
        if let Some(work) = self.runtime_services.work.as_ref() {
            let state = work
                .try_capture(self.current_session_id.as_deref())
                .map(|state| {
                    state.map(|state| SessionWorkState {
                        graph: Some(state.graph),
                        todos: state.todos,
                        plan: state.plan,
                    })
                })?;
            self.last_known_work_state = Some(state.clone());
            return Ok(state);
        }
        let todos = self
            .todos
            .try_lock()
            .map_err(|_| "To-do state is busy".to_string())?;
        let plan = self
            .plan_state
            .try_lock()
            .map_err(|_| "Plan state is busy".to_string())?;
        let state = SessionWorkState {
            graph: None,
            todos: todos.snapshot(),
            plan: plan.snapshot(),
        };
        let state = (!state.is_empty()).then_some(state);
        drop(plan);
        drop(todos);
        self.last_known_work_state = Some(state.clone());
        Ok(state)
    }

    /// Atomically replace the live Work state from a saved session.
    pub fn restore_work_state(
        &mut self,
        session_id: &str,
        workspace: &Path,
        state: Option<&SessionWorkState>,
    ) -> Result<(), String> {
        self.pending_plan_proposal_id = None;
        if let Some(work) = self.runtime_services.work.as_ref() {
            let empty = SessionWorkState::default();
            let state = state.unwrap_or(&empty);
            work.restore_with_workspace_owner_bindings(
                session_id,
                workspace,
                state.graph.as_ref(),
                &state.todos,
                &state.plan,
            )?;
            let restored = work.capture(Some(session_id))?;
            let normalized_state = restored.map(|state| SessionWorkState {
                graph: Some(state.graph),
                todos: state.todos,
                plan: state.plan,
            });
            self.cached_work_summary = None;
            self.last_known_work_state = Some(normalized_state);
            return Ok(());
        }
        let (restored_todos, restored_plan) = match state {
            Some(state) => (
                TodoList::from_snapshot(&state.todos)?,
                PlanState::from_snapshot(&state.plan),
            ),
            None => (TodoList::new(), PlanState::default()),
        };
        let normalized_state = SessionWorkState {
            graph: None,
            todos: restored_todos.snapshot(),
            plan: restored_plan.snapshot(),
        };

        let mut todos = Self::retry_lock(&self.todos, 100)
            .ok_or_else(|| "To-do state is busy; session was not restored".to_string())?;
        let mut plan = Self::retry_lock(&self.plan_state, 100)
            .ok_or_else(|| "Plan state is busy; session was not restored".to_string())?;
        *todos = restored_todos;
        *plan = restored_plan;
        drop(plan);
        drop(todos);
        self.cached_work_summary = None;
        self.last_known_work_state =
            Some((!normalized_state.is_empty()).then_some(normalized_state));
        Ok(())
    }

    pub fn clear_todos(&mut self) -> bool {
        if let Some(work) = self.runtime_services.work.as_ref() {
            if !work.clear(self.current_session_id.as_deref()) {
                return false;
            }
            self.cached_work_summary = None;
            self.last_known_work_state = Some(None);
            return true;
        }
        // Acquire both stores before mutating either one. `/clear` must never
        // report success after clearing only half of the Work surface.
        let Some(mut todos) = Self::retry_lock(&self.todos, 100) else {
            return false;
        };
        let Some(mut plan) = Self::retry_lock(&self.plan_state, 100) else {
            return false;
        };
        todos.clear();
        *plan = PlanState::default();
        drop(plan);
        drop(todos);
        self.cached_work_summary = None;
        self.last_known_work_state = Some(None);
        true
    }

    /// Publish a validated Work Graph transaction after a synchronous caller
    /// has completed its atomic session write.
    pub fn publish_pending_work_state(&mut self) -> Result<bool, String> {
        let published = self
            .runtime_services
            .work
            .as_ref()
            .map_or(Ok(false), |work| work.publish_pending_sync())?;
        if published {
            self.cached_work_summary = None;
        }
        Ok(published)
    }

    pub fn update_model_compaction_budget(&mut self) {
        let model = self.effective_model_for_budget().to_string();
        self.compact_threshold = crate::route_budget::compaction_threshold_for_route_at_percent(
            self.api_provider,
            &model,
            self.active_route_limits,
            self.auto_compact_threshold_percent,
        );
        if !self.auto_compact_user_configured {
            self.auto_compact = crate::route_budget::auto_compact_default_for_route(
                self.api_provider,
                &model,
                self.active_route_limits,
            );
        }
    }

    pub fn set_active_route_limits(&mut self, limits: RouteLimits) {
        self.active_route_limits = crate::route_budget::known_route_limits(limits);
    }

    /// Install an already-resolved runtime route receipt in one operation so
    /// endpoint-sensitive reasoning and context reporting cannot drift apart.
    pub fn set_active_route_resolution(
        &mut self,
        base_url: impl Into<String>,
        limits: RouteLimits,
        context_window_source: crate::route_runtime::ContextWindowSource,
    ) {
        self.active_route_base_url = base_url.into();
        self.set_active_route_limits(limits);
        self.active_context_window_source = context_window_source;
    }

    /// Whether the currently selected onboarding route is Kimi Code's
    /// membership-plan `k3` endpoint. The check is deliberately exact: the
    /// Moonshot public API must never inherit Kimi Code plan guidance.
    pub fn onboarding_uses_kimi_code_plan(&self) -> bool {
        crate::config::is_exact_kimi_code_k3_route(
            self.onboarding_provider,
            &self.active_route_base_url,
            &self.model,
        )
    }

    pub fn set_active_context_window_override(&mut self, context_window: Option<u32>) {
        self.active_context_window_override = context_window;
        if context_window.is_some() {
            self.active_context_window_source =
                crate::route_runtime::ContextWindowSource::Configured;
        }
        if self.active_route_limits.is_none() {
            self.active_route_limits = self.context_window_override_limits();
        }
    }

    pub fn context_window_override_limits(&self) -> Option<RouteLimits> {
        self.active_context_window_override
            .map(|window| RouteLimits {
                context_tokens: Some(u64::from(window)),
                ..RouteLimits::default()
            })
    }

    pub fn set_model_selection(&mut self, model: String) {
        let auto_model = model.trim().eq_ignore_ascii_case("auto");
        self.model = if auto_model {
            "auto".to_string()
        } else {
            model
        };
        self.auto_model = auto_model;
        self.last_effective_model = None;
        self.last_effective_provider = None;
        self.last_effective_provider_identity = None;
        self.last_auto_route_receipt = None;
        self.pending_auto_route_receipt = None;
        self.last_effective_reasoning_effort = None;
        if auto_model {
            self.reasoning_effort = ReasoningEffort::Auto;
        } else {
            self.reasoning_effort = self
                .reasoning_effort
                .normalize_for_provider(self.api_provider);
        }
    }

    pub fn model_selection_for_persistence(&self) -> String {
        if self.auto_model || self.model.trim().eq_ignore_ascii_case("auto") {
            "auto".to_string()
        } else {
            self.model.clone()
        }
    }

    /// Atomic latest Auto route metadata for session snapshots. The provider,
    /// exact identity, model, and receipt are either persisted together or
    /// omitted together so a resumed session cannot display a mixed route.
    #[must_use]
    pub(crate) fn auto_route_for_persistence(
        &self,
    ) -> Option<crate::session_manager::SavedAutoRouteReceipt> {
        if !self.auto_model {
            return None;
        }
        let (provider, model, receipt) = (
            self.last_effective_provider?,
            self.last_effective_model.as_ref()?,
            self.last_auto_route_receipt.as_ref()?,
        );
        if model.trim().is_empty() {
            return None;
        }
        let provider_identity = self
            .last_effective_provider_identity
            .clone()
            .unwrap_or_else(|| {
                if provider == ApiProvider::Custom {
                    self.provider_identity_for_persistence().to_string()
                } else {
                    provider.as_str().to_string()
                }
            });
        Some(crate::session_manager::SavedAutoRouteReceipt {
            provider,
            provider_identity,
            model: model.clone(),
            receipt: receipt.clone(),
        })
    }

    #[must_use]
    pub(crate) fn provider_identity_for_persistence(&self) -> &str {
        if self.api_provider == ApiProvider::Custom {
            &self.provider_identity
        } else {
            self.api_provider.as_str()
        }
    }

    #[must_use]
    pub(crate) fn provider_id_for_persistence(&self) -> Option<&str> {
        self.provider_exact_id.as_deref()
    }

    pub(crate) fn set_provider_identity(
        &mut self,
        provider: ApiProvider,
        identity: impl Into<String>,
    ) {
        let identity = identity.into();
        self.api_provider = provider;
        self.provider_exact_id = (!(provider == ApiProvider::Custom
            && identity.eq_ignore_ascii_case(ApiProvider::Custom.as_str())))
        .then(|| identity.clone());
        self.provider_identity = identity;
    }

    pub(crate) fn set_provider_identity_record(
        &mut self,
        identity: crate::config::ProviderIdentity,
    ) {
        self.api_provider = identity.provider;
        self.provider_identity = identity.key;
        self.provider_exact_id = identity.exact_id;
    }

    pub fn accepts_custom_model_ids(&self) -> bool {
        self.model_ids_passthrough
            || crate::config::provider_passes_model_through(self.api_provider)
    }

    pub(crate) fn apply_provider_switch_reasoning_effort(
        &mut self,
        provider: ApiProvider,
        base_url: &str,
        model_override: Option<&str>,
    ) {
        let wire_model = model_override.unwrap_or(&self.model);
        let inferred = model_override.and_then(|model| {
            crate::config::legacy_deepseek_alias_effort_for_route(provider, base_url, model)
        });
        self.reasoning_effort = if self.reasoning_effort_explicit {
            self.reasoning_effort
                .normalize_for_route(provider, base_url, wire_model)
        } else if let Some(effort) = inferred {
            ReasoningEffort::from_setting(effort)
                .normalize_for_route(provider, base_url, wire_model)
        } else {
            self.reasoning_effort
                .normalize_for_route(provider, base_url, wire_model)
        };
        self.last_effective_reasoning_effort = None;
    }

    pub fn effective_model_for_budget(&self) -> &str {
        if self.auto_model {
            return self
                .last_effective_model
                .as_deref()
                .filter(|model| *model != "auto")
                .unwrap_or(DEFAULT_TEXT_MODEL);
        }
        &self.model
    }

    pub fn model_display_label(&self) -> String {
        if self.auto_model {
            if let Some(effective) = self.last_effective_model.as_deref()
                && effective != "auto"
            {
                return format!("auto: {effective}");
            }
            return "auto".to_string();
        }
        self.model.clone()
    }

    /// Provider/model identity used by the in-flight or most recent request.
    /// This is the display contract for auto routing and must match billing.
    #[must_use]
    pub fn effective_route_display(&self) -> (ApiProvider, String) {
        if let Some((provider, model, _)) = self.pending_turn_route.as_ref() {
            return (*provider, model.clone());
        }
        if self.auto_model
            && let (Some(provider), Some(model)) = (
                self.last_effective_provider,
                self.last_effective_model.as_ref(),
            )
        {
            return (provider, model.clone());
        }
        (self.api_provider, self.model_display_label())
    }

    /// Exact non-secret route label for user-visible status surfaces.
    #[must_use]
    pub fn effective_route_identity_display(&self) -> (String, String) {
        let (provider, model) = self.effective_route_display();
        let identity = if provider == ApiProvider::Custom {
            if self.pending_turn_route.is_none() && self.auto_model {
                self.last_effective_provider_identity
                    .as_deref()
                    .unwrap_or_else(|| self.provider_identity_for_persistence())
            } else {
                self.provider_identity_for_persistence()
            }
        } else {
            provider.display_name()
        };
        (identity.to_string(), model)
    }

    fn effective_reasoning_effort_for_active_route(
        &self,
        requested: ReasoningEffort,
    ) -> ReasoningEffort {
        if self.auto_model || requested == ReasoningEffort::Auto {
            return self
                .last_effective_reasoning_effort
                .unwrap_or(ReasoningEffort::Auto);
        }
        requested.normalize_for_route(self.api_provider, &self.active_route_base_url, &self.model)
    }

    fn reasoning_effort_resolution_label(
        requested: ReasoningEffort,
        effective: ReasoningEffort,
        provider: ApiProvider,
    ) -> String {
        if requested == effective {
            return effective.display_label_for_provider(provider).to_string();
        }
        let effective = effective.display_label_for_provider(provider);
        if requested == ReasoningEffort::Auto {
            format!("auto: {effective}")
        } else {
            format!("{}→{effective}", requested.short_label())
        }
    }

    pub fn reasoning_effort_display_label(&self) -> String {
        let requested = if self.auto_model {
            ReasoningEffort::Auto
        } else {
            self.reasoning_effort
        };
        let effective = self.effective_reasoning_effort_for_active_route(requested);
        Self::reasoning_effort_resolution_label(requested, effective, self.api_provider)
    }

    pub fn compaction_config(&self) -> CompactionConfig {
        let mut config = self.compaction_config_for_route(
            self.api_provider,
            self.effective_model_for_budget(),
            self.active_route_limits,
        );
        // These cached fields are the active-route compatibility authority and
        // are updated together by `update_model_compaction_budget`. Commands
        // and embedders may also adjust them directly between route updates.
        config.enabled = self.auto_compact;
        config.token_threshold = self.compact_threshold;
        config
    }

    /// Build compaction policy from one already-resolved provider route.
    ///
    /// Auto routing can select a provider/model whose context limits differ
    /// from the route currently displayed by the app. Callers dispatching that
    /// turn must derive every compaction input from the selected descriptor,
    /// not from the previous route cached in `App`.
    pub(crate) fn compaction_config_for_route(
        &self,
        provider: ApiProvider,
        model: &str,
        route_limits: Option<RouteLimits>,
    ) -> CompactionConfig {
        CompactionConfig {
            enabled: if self.auto_compact_user_configured {
                self.auto_compact
            } else {
                crate::route_budget::auto_compact_default_for_route(provider, model, route_limits)
            },
            token_threshold: crate::route_budget::compaction_threshold_for_route_at_percent(
                provider,
                model,
                route_limits,
                self.auto_compact_threshold_percent,
            ),
            model: model.to_string(),
            effective_context_window: Some(crate::route_budget::route_context_window_tokens(
                provider,
                model,
                route_limits,
            )),
            ..Default::default()
        }
    }

    pub fn fallback_chain_entries(&self) -> Vec<(usize, ApiProvider, bool)> {
        let Some(chain) = &self.provider_chain else {
            return Vec::new();
        };
        let position = chain.position();
        chain
            .providers()
            .iter()
            .enumerate()
            .map(|(index, provider)| (index, ApiProvider::from_kind(*provider), index == position))
            .collect()
    }

    pub fn fallback_chain_position(&self) -> Option<usize> {
        self.provider_chain.as_ref().map(ProviderChain::position)
    }

    pub fn fallback_chain_len(&self) -> usize {
        self.provider_chain
            .as_ref()
            .map_or(0, |chain| chain.providers().len())
    }

    /// Whether a fallback chain entry can serve a turn right now (#2574).
    ///
    /// Mirrors the provider picker's eligibility: hosted providers need a key
    /// (`has_api_key_for`, captured into `provider_readiness` at startup) while
    /// self-hosted providers (Ollama/vLLM/SGLang) are always ready. Providers
    /// absent from the snapshot default to ready so an unknown entry is tried
    /// rather than silently skipped.
    fn fallback_provider_is_ready(&self, provider: ApiProvider) -> bool {
        self.provider_readiness
            .iter()
            .find_map(|(candidate, ready)| (*candidate == provider).then_some(*ready))
            .unwrap_or(true)
    }

    /// Advance to the next *eligible* provider in the fallback chain (#2574).
    ///
    /// Walks the chain from the current position, skipping entries that are not
    /// ready (hosted providers missing auth) and recording a clear note for each
    /// skip. Local providers are always eligible. Returns the first ready
    /// provider, or `None` (with an exhaustion reason) when every remaining entry
    /// is unready or the end of the chain is reached. `ProviderChain::advance`
    /// stays pure — the readiness filtering lives here at the App level.
    ///
    /// Note: auth-rejection (401) failures never reach this path; the caller
    /// excludes them from fallback so a bad key does not silently rotate
    /// providers (see `apply_engine_error_to_app`).
    ///
    /// Local/private policy (#2574): when the chain's primary provider is a
    /// self-hosted / local runtime, cloud candidates are skipped with a clear
    /// note so a local/private route never silently falls back out to a hosted
    /// provider. Self-hosted siblings remain eligible. The policy is anchored
    /// to the original primary; a cloud primary may still hop through a local
    /// runtime and then back to another cloud fallback.
    pub fn advance_fallback(&mut self, reason: impl Into<String>) -> Option<ApiProvider> {
        let reason = reason.into();
        self.provider_chain.as_ref()?;

        let origin_is_local = self
            .provider_chain
            .as_ref()
            .and_then(|chain| chain.providers().first().copied())
            .map(ApiProvider::from_kind)
            .is_some_and(ApiProvider::is_self_hosted);

        let mut skip_notes: Vec<String> = Vec::new();
        let mut chosen: Option<ApiProvider> = None;
        while let Some(next_kind) = self
            .provider_chain
            .as_mut()
            .and_then(ProviderChain::advance)
        {
            let candidate = ApiProvider::from_kind(next_kind);
            if origin_is_local && !candidate.is_self_hosted() {
                skip_notes.push(format!(
                    "skipped {}: local/private policy (no local->cloud fallback)",
                    candidate.as_str()
                ));
                continue;
            }
            if self.fallback_provider_is_ready(candidate) {
                chosen = Some(candidate);
                break;
            }
            skip_notes.push(format!("skipped {}: needs auth", candidate.as_str()));
        }

        let skipped = if skip_notes.is_empty() {
            String::new()
        } else {
            format!(" ({})", skip_notes.join("; "))
        };

        let Some(next_provider) = chosen else {
            let total = self
                .provider_chain
                .as_ref()
                .map_or(0, |chain| chain.providers().len());
            self.last_fallback_reason = Some(format!(
                "Fallback chain exhausted after {total} provider(s): {reason}{skipped}"
            ));
            return None;
        };

        self.set_provider_identity(next_provider, next_provider.as_str());
        self.last_fallback_reason = Some(format!(
            "Fell back to {} after recoverable provider error: {reason}{skipped}",
            next_provider.as_str()
        ));
        Some(next_provider)
    }

    pub fn is_fallback_active(&self) -> bool {
        self.provider_chain
            .as_ref()
            .is_some_and(ProviderChain::is_fallback_active)
    }
}

pub fn media_attachment_reference(kind: &str, path: &Path, description: Option<&str>) -> String {
    match description {
        Some(description) if !description.trim().is_empty() => {
            format!(
                "[Attached {kind}: {} at {}]",
                description.trim(),
                path.display()
            )
        }
        _ => format!("[Attached {kind}: {}]", path.display()),
    }
}

// === Actions ===

/// Actions emitted by the UI event loop.
#[derive(Debug, Clone, PartialEq)]
pub enum AppAction {
    Quit,
    #[allow(dead_code)] // For explicit /save command
    SaveSession(PathBuf),
    #[allow(dead_code)] // For explicit /load command
    LoadSession(PathBuf),
    SyncSession {
        session_id: Option<String>,
        messages: Vec<Message>,
        system_prompt: Option<SystemPrompt>,
        model: String,
        workspace: PathBuf,
        mode: AppMode,
    },
    OpenConfigEditor(ConfigUiMode),
    OpenConfigView,
    /// Open the `/model` two-pane picker (Pro/Flash + Off/High/Max).
    OpenModelPicker,
    /// Open the `/provider` picker modal — DeepSeek / NVIDIA NIM / OpenRouter
    /// / Novita with inline API-key prompt for un-configured providers (#52).
    OpenProviderPicker,
    /// Open the `/provider` picker in setup/catalog mode, optionally focused on
    /// a built-in provider that needs credentials before first use.
    OpenProviderSetup {
        provider: Option<ApiProvider>,
    },
    /// Run the xAI/Grok device-code flow with the TUI temporarily suspended.
    StartXaiDeviceLogin,
    /// Open the `/mode` picker modal for Act / Plan / Operate.
    OpenModePicker,
    /// Refresh the engine prompt after the UI operating mode changes.
    ModeChanged(AppMode),
    /// Synchronize a saved top-level approval policy into the live Config,
    /// then refresh the engine prompt from the App's updated permission mode.
    ApprovalPolicyPersisted {
        policy: Option<String>,
    },
    /// Rebuild the engine's Skill/MCP catalogue from the App's newly replaced
    /// immutable plugin snapshot after trust, enable, revoke, or reload.
    PluginRegistryChanged,
    /// Open the `/statusline` multi-select picker for footer items.
    OpenStatusPicker,
    /// Open the `/feedback` picker for GitHub issue/security destinations.
    OpenFeedbackPicker,
    /// Open the `/theme` picker modal with live preview of every preset.
    OpenThemePicker,
    /// Open the `/fleet` roster — the saved-party view of the agent team.
    OpenFleetRoster,
    /// Open the `/fleet` profile authoring wizard.
    OpenFleetSetup,
    /// Open the `/hotbar` setup wizard.
    OpenHotbarSetup,
    /// Open the constitution-first `/setup` wizard shell.
    OpenSetupWizard,
    /// Open the constitution-first `/setup` wizard at a specific step.
    OpenSetupWizardAt {
        step: codewhale_config::SetupStep,
    },
    /// Record that the bundled/default constitution should be used.
    UseBundledConstitution,
    /// Disable the Hotbar: persist `hotbar = []` and clear the live slots.
    DisableHotbar,
    /// Restore the default recommended Hotbar slots: remove the `hotbar` key so
    /// the resolver falls back to the built-in defaults.
    RestoreHotbarDefaults,
    /// Open an external URL in the system browser.
    OpenExternalUrl {
        url: String,
        label: String,
    },
    /// Send a message to the AI (normal chat mode).
    SendMessage(String),
    /// Cancel a running sub-agent through the engine manager.
    CancelSubAgent {
        agent_id: String,
    },
    /// Update the runtime goal status (`/goal pause|resume|clear|…`) without
    /// dispatching a model turn. The UI layer translates this into
    /// `Op::SetGoalStatus`.
    SetGoalStatus {
        status: crate::tools::goal::GoalStatus,
        clear: bool,
    },
    ListSubAgents,
    FetchModels,
    /// Force a Models.dev live-catalog refresh into ProviderLake (#4187).
    RefreshModelsDevCatalog,
    CacheWarmup,
    /// Switch the active LLM backend (DeepSeek vs NVIDIA NIM) without
    /// restarting the process. The runtime rebuilds its API client from
    /// the updated config. `model` overrides the post-switch model
    /// (already normalized but not yet provider-prefixed).
    SwitchProvider {
        provider: ApiProvider,
        model: Option<String>,
    },
    /// Switch provider+model through the same apply path as a `/model` route
    /// row. Used by Hotbar route slots so dispatch does not hand-mutate config.
    SwitchModelRoute {
        provider: ApiProvider,
        model: String,
    },
    UpdateCompaction(CompactionConfig),
    UpdateStreamChunkTimeout(u64),
    UpdateSubagentRuntimeConfig {
        enabled: bool,
        max_subagents: usize,
        launch_concurrency: usize,
        max_spawn_depth: u32,
        api_timeout_secs: u64,
        heartbeat_timeout_secs: u64,
    },
    /// Open the live transcript overlay through a terminal-safe command path.
    OpenLiveTranscript,
    OpenContextInspector,
    CompactContext,
    PurgeContext,
    TaskAdd {
        prompt: String,
    },
    TaskList,
    TaskShow {
        id: String,
    },
    TaskCancel {
        id: String,
    },
    ShellJob(ShellJobAction),
    Mcp(McpUiAction),
    /// Switch to a different config profile without restarting.
    SwitchProfile {
        /// Profile name to load.
        profile: String,
    },
    /// Switch the workspace used by tools, hooks, tasks, and session metadata.
    SwitchWorkspace {
        workspace: PathBuf,
    },
    /// Record from the microphone and route the transcription into the
    /// composer (or auto-send it). Emitted by `/voice` and the voice hotbar
    /// action; handled in the UI event loop where the live `Config` supplies
    /// provider credentials.
    VoiceCapture,
    /// Export and share the current session as a web URL.
    ShareSession {
        history_len: usize,
        model: String,
        mode: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellJobAction {
    List,
    Show {
        id: String,
    },
    Poll {
        id: String,
        wait: bool,
    },
    SendStdin {
        id: String,
        input: String,
        close: bool,
    },
    Cancel {
        id: String,
    },
    CancelAll,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpUiAction {
    Show,
    Init {
        force: bool,
    },
    AddStdio {
        name: String,
        command: String,
        args: Vec<String>,
    },
    AddHttp {
        name: String,
        url: String,
        transport: Option<String>,
    },
    Enable {
        name: String,
    },
    Disable {
        name: String,
    },
    Remove {
        name: String,
    },
    Login {
        name: String,
        scopes: Vec<String>,
    },
    Logout {
        name: String,
    },
    Validate,
    Reload,
}

#[cfg(test)]
mod tests;
